//! Background queue + retry loop for SaaS stage sync.
//!
//! The renderer thread sees zero HTTP latency — it just calls [`submit`],
//! [`set_target`], [`clear_target`], [`update_token`]. A worker thread owns:
//!   * the `fixture_key → SaaS id` map (so updates / deletes can address rows
//!     even though the renderer keys off the local key);
//!   * the pending-op queue;
//!   * backoff when SaaS is unreachable.
//!
//! Status is published into a shared `SyncStatus` slot the UI reads each
//! frame. Successful stage pulls come back via the [`WorkerEvent`] channel
//! so the renderer can swap its patch atomically.

use std::collections::{HashMap, VecDeque};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::db::Database;
use crate::emerge::{DmxPatch, FixturePatch, Truss};
use crate::stage_sync::{self, StageSyncConfig};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncStatusKind {
    NoStage,
    Idle,
    Syncing,
    Pending(usize),
    Offline { queued: usize, error: String },
}

#[derive(Clone, Debug)]
pub struct SyncStatus {
    pub kind: SyncStatusKind,
    pub last_synced_at: Option<Instant>,
    pub last_error: Option<String>,
}

impl Default for SyncStatus {
    fn default() -> Self {
        Self {
            kind: SyncStatusKind::NoStage,
            last_synced_at: None,
            last_error: None,
        }
    }
}

/// One pending mutation. Created/deleted fixtures key off the renderer-side
/// `fixture_key`; the worker resolves it to the SaaS id at execution time.
#[derive(Clone, Debug)]
pub enum SyncOp {
    Create { key: String, fixture: FixturePatch },
    Update { key: String, fixture: FixturePatch },
    Delete { key: String },
    /// Replace the stage's whole truss layout. Trusses are edited as a set,
    /// so they round-trip as one stage-level PATCH rather than keyed CRUD.
    SetTrusses { trusses: Vec<Truss> },
}

/// One-shot events the worker pushes back at the renderer.
pub enum WorkerEvent {
    /// Stage just fetched (initial pull, or after a reconnect). The renderer
    /// should replace its in-memory patch with this and reseat its diff baseline.
    StagePulled { stage_id: i64, patch: DmxPatch },
}

enum Cmd {
    SetTarget {
        base_url: String,
        token: String,
        stage_id: i64,
    },
    UpdateToken(String),
    ClearTarget,
    Submit(SyncOp),
    SetDb(Database),
    Shutdown,
}

pub struct StageSyncWorker {
    cmd_tx: mpsc::Sender<Cmd>,
    event_rx: mpsc::Receiver<WorkerEvent>,
    status: Arc<Mutex<SyncStatus>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl StageSyncWorker {
    pub fn new() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
        let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>();
        let status = Arc::new(Mutex::new(SyncStatus::default()));
        let status_thread = status.clone();
        let thread = thread::Builder::new()
            .name("stage-sync".into())
            .spawn(move || run(cmd_rx, event_tx, status_thread))
            .ok();
        Self {
            cmd_tx,
            event_rx,
            status,
            thread,
        }
    }

    pub fn set_target(&self, base_url: String, token: String, stage_id: i64) {
        let _ = self.cmd_tx.send(Cmd::SetTarget {
            base_url,
            token,
            stage_id,
        });
    }

    pub fn clear_target(&self) {
        let _ = self.cmd_tx.send(Cmd::ClearTarget);
    }

    pub fn update_token(&self, token: String) {
        let _ = self.cmd_tx.send(Cmd::UpdateToken(token));
    }

    pub fn submit(&self, op: SyncOp) {
        let _ = self.cmd_tx.send(Cmd::Submit(op));
    }

    /// Attach the local DB so the worker can persist/restore its fixture-key ->
    /// SaaS-id map across runs (offline-safe edits, slice 3 part 1).
    pub fn set_db(&self, db: Database) {
        let _ = self.cmd_tx.send(Cmd::SetDb(db));
    }

    pub fn status(&self) -> SyncStatus {
        self.status
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    pub fn poll_events(&self) -> Vec<WorkerEvent> {
        let mut events = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            events.push(ev);
        }
        events
    }
}

impl Drop for StageSyncWorker {
    fn drop(&mut self) {
        // Ask the worker to stop, but DON'T block close on it: the loop can be
        // parked in a 15 s blocking `pull_stage`/`drain_one` HTTP call (e.g.
        // SaaS unreachable), and a plain `join()` would freeze app close for
        // that whole window. The worker has no exit-time flush (Shutdown just
        // returns), so a brief grace + abandon is safe.
        let _ = self.cmd_tx.send(Cmd::Shutdown);
        if let Some(handle) = self.thread.take() {
            crate::thread_util::join_or_abandon(handle, crate::thread_util::SHUTDOWN_GRACE);
        }
    }
}

fn run(
    cmd_rx: mpsc::Receiver<Cmd>,
    event_tx: mpsc::Sender<WorkerEvent>,
    status: Arc<Mutex<SyncStatus>>,
) {
    let mut state = WorkerState::default();
    loop {
        // Drain any queued commands first.
        loop {
            match cmd_rx.try_recv() {
                Ok(cmd) => {
                    if state.handle_command(cmd, &event_tx) {
                        publish(&state, &status);
                        return;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }

        // After the backoff window: if this target has never pulled successfully,
        // retry the pull — self-heals a launch where the token/SaaS was not ready
        // yet (otherwise auto-restore strands the rig as un-profiled dots until a
        // manual re-select). Once pulled, drain one queued op. Pulling before
        // draining also refreshes id_by_key so flushed Updates/Deletes address
        // current rows.
        if state.target.is_some() && state.backoff_until.map_or(true, |t| Instant::now() >= t) {
            if !state.pulled_ok && state.queue.is_empty() {
                // No successful pull yet AND nothing pending to flush -> (re)try the
                // pull. Gating on an empty queue means a fresh patch never clobbers
                // the operator's queued edits: with pending ops we drain them first
                // (below) and only pull once the queue has cleared.
                state.pull_stage(&event_tx);
            } else if !state.queue.is_empty() {
                state.drain_one();
            }
        }

        publish(&state, &status);

        // Block on next command, with a short timeout so we wake up after
        // backoff to retry without spinning.
        let wake_in = state
            .backoff_until
            .and_then(|t| t.checked_duration_since(Instant::now()))
            .unwrap_or_else(|| {
                if state.queue.is_empty() {
                    Duration::from_secs(2)
                } else {
                    Duration::from_millis(50)
                }
            })
            .max(Duration::from_millis(50));
        match cmd_rx.recv_timeout(wake_in) {
            Ok(cmd) => {
                if state.handle_command(cmd, &event_tx) {
                    publish(&state, &status);
                    return;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[derive(Default)]
struct WorkerState {
    target: Option<Target>,
    queue: VecDeque<SyncOp>,
    id_by_key: HashMap<String, i64>,
    backoff_until: Option<Instant>,
    backoff: Duration,
    syncing: bool,
    last_synced_at: Option<Instant>,
    last_error: Option<String>,
    db: Option<Database>,
    /// Whether the current target has had at least one successful pull. Until it
    /// does, the run loop retries `pull_stage` (with backoff) — self-healing a
    /// launch where the token/SaaS was not ready yet, so auto-restore does not
    /// strand the rig as un-profiled dots until a manual re-select.
    pulled_ok: bool,
}

struct Target {
    base_url: String,
    token: String,
    stage_id: i64,
}

impl WorkerState {
    fn handle_command(&mut self, cmd: Cmd, events: &mpsc::Sender<WorkerEvent>) -> bool {
        match cmd {
            Cmd::SetTarget {
                base_url,
                token,
                stage_id,
            } => {
                self.target = Some(Target {
                    base_url,
                    token,
                    stage_id,
                });
                self.queue.clear();
                self.id_by_key.clear();
                self.backoff_until = None;
                self.backoff = Duration::ZERO;
                self.last_error = None;
                self.pulled_ok = false;
                self.pull_stage(events);
            }
            Cmd::UpdateToken(token) => {
                if let Some(t) = self.target.as_mut() {
                    // A genuinely new token (auth refresh / recovery) re-arms the
                    // pull retry: the prior pull may have failed on a stale token, so
                    // we want to try again. Same-token updates (the per-frame norm)
                    // are a no-op, so this never storms.
                    if t.token != token {
                        t.token = token;
                        self.pulled_ok = false;
                    }
                }
            }
            Cmd::ClearTarget => {
                self.target = None;
                self.queue.clear();
                self.id_by_key.clear();
                self.backoff_until = None;
                self.backoff = Duration::ZERO;
                self.last_error = None;
                self.pulled_ok = false;
            }
            Cmd::Submit(op) => {
                self.queue.push_back(op);
            }
            Cmd::SetDb(db) => {
                self.db = Some(db);
            }
            Cmd::Shutdown => return true,
        }
        false
    }

    fn pull_stage(&mut self, events: &mpsc::Sender<WorkerEvent>) {
        let Some(target) = self.target.as_ref() else {
            return;
        };
        let config = StageSyncConfig {
            base_url: target.base_url.clone(),
            token: target.token.clone(),
        };
        self.syncing = true;
        match stage_sync::fetch_stage_patch(&config, target.stage_id) {
            Ok((patch, map)) => {
                self.id_by_key = map;
                // Persist the key->id map so a later offline start can still
                // address existing rows by id (slice 3 part 1: no duplicate Creates).
                if let Some(db) = &self.db {
                    if let Ok(json) = serde_json::to_string(&self.id_by_key) {
                        let _ = db.cache_stage_id_map(target.stage_id, &json);
                    }
                }
                self.pulled_ok = true;
                self.last_synced_at = Some(Instant::now());
                self.last_error = None;
                self.backoff = Duration::ZERO;
                self.backoff_until = None;
                let _ = events.send(WorkerEvent::StagePulled {
                    stage_id: target.stage_id,
                    patch,
                });
            }
            Err(err) => {
                log::warn!("[stage-sync] stage pull failed: {}", err);
                // Offline: restore the last-known key->id map so queued
                // Updates/Deletes address existing rows instead of folding into
                // duplicate Creates when the network returns.
                if let Some(db) = &self.db {
                    if let Some(json) = db.cached_stage_id_map(target.stage_id) {
                        if let Ok(map) = serde_json::from_str::<HashMap<String, i64>>(&json) {
                            self.id_by_key = map;
                        }
                    }
                }
                self.last_error = Some(err);
                self.bump_backoff();
            }
        }
        self.syncing = false;
    }

    fn drain_one(&mut self) {
        let Some(target) = self.target.as_ref() else {
            return;
        };
        let Some(op) = self.queue.pop_front() else {
            return;
        };
        let config = StageSyncConfig {
            base_url: target.base_url.clone(),
            token: target.token.clone(),
        };
        self.syncing = true;
        let outcome = match &op {
            SyncOp::Create { key, fixture } => {
                match stage_sync::create_fixture(&config, target.stage_id, fixture) {
                    Ok(saas_id) => {
                        self.id_by_key.insert(key.clone(), saas_id);
                        Ok(())
                    }
                    Err(err) => Err(err),
                }
            }
            SyncOp::Update { key, fixture } => match self.id_by_key.get(key).copied() {
                Some(saas_id) => {
                    match stage_sync::update_fixture(&config, target.stage_id, saas_id, fixture) {
                        Ok(true) => Ok(()),
                        Ok(false) => {
                            // 404 — the row was deleted on SaaS (e.g. a stale cached
                            // id after an offline start). Re-create it (keep-local)
                            // and drop the stale mapping, so we stop 404ing forever
                            // and the queue (hence the recovery pull) unblocks.
                            self.id_by_key.remove(key);
                            self.queue.push_front(SyncOp::Create {
                                key: key.clone(),
                                fixture: fixture.clone(),
                            });
                            self.syncing = false;
                            return;
                        }
                        Err(err) => Err(err),
                    }
                }
                None => {
                    // Update before Create has been acknowledged — fold the
                    // latest payload into a fresh Create so the row gets
                    // authored once with the current state.
                    self.queue.push_front(SyncOp::Create {
                        key: key.clone(),
                        fixture: fixture.clone(),
                    });
                    self.syncing = false;
                    return;
                }
            },
            SyncOp::Delete { key } => match self.id_by_key.remove(key) {
                Some(saas_id) => {
                    stage_sync::delete_fixture(&config, target.stage_id, saas_id)
                }
                None => {
                    // Never tracked upstream — drop the op silently. Could be
                    // a fixture the operator added then removed before the
                    // first Create cleared the queue.
                    Ok(())
                }
            },
            SyncOp::SetTrusses { trusses } => {
                stage_sync::update_truss_layout(&config, target.stage_id, trusses)
            }
        };
        match outcome {
            Ok(()) => {
                self.last_synced_at = Some(Instant::now());
                self.last_error = None;
                self.backoff = Duration::ZERO;
                self.backoff_until = None;
            }
            Err(err) => {
                log::warn!("[stage-sync] op failed, requeueing: {}", err);
                self.last_error = Some(err);
                self.queue.push_front(op);
                self.bump_backoff();
            }
        }
        self.syncing = false;
    }

    fn bump_backoff(&mut self) {
        self.backoff = match self.backoff {
            Duration::ZERO => Duration::from_secs(1),
            d if d < Duration::from_secs(30) => (d * 2).min(Duration::from_secs(30)),
            _ => Duration::from_secs(30),
        };
        self.backoff_until = Some(Instant::now() + self.backoff);
    }
}

fn publish(state: &WorkerState, slot: &Arc<Mutex<SyncStatus>>) {
    let kind = if state.target.is_none() {
        SyncStatusKind::NoStage
    } else if state.last_error.is_some() && state.backoff_until.is_some() {
        SyncStatusKind::Offline {
            queued: state.queue.len(),
            error: state.last_error.clone().unwrap_or_default(),
        }
    } else if state.syncing {
        SyncStatusKind::Syncing
    } else if !state.queue.is_empty() {
        SyncStatusKind::Pending(state.queue.len())
    } else {
        SyncStatusKind::Idle
    };
    if let Ok(mut s) = slot.lock() {
        s.kind = kind;
        s.last_synced_at = state.last_synced_at;
        s.last_error = state.last_error.clone();
    }
}

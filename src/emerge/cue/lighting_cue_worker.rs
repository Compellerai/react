//! COM-499 lighting-cue background worker.
//!
//! Mirrors `stage_sync_worker`: the render thread never blocks on HTTP. The
//! LOCAL STORE (`db::active_cues`) is the executor's source of truth — the
//! renderer drives from the slot this worker (or an operator launch) seeds, so a
//! running show needs ~zero SaaS calls and keeps driving when the network is
//! pulled. This worker is the ONLINE sync + trigger layer only: on a slow
//! heartbeat it fetches `GET /api/v1/lighting/cues`, mirrors the runnable cues
//! into the local store (cache-from-launches + offline durability), publishes the
//! parsed [`ActiveCue`]s into the shared slot the renderer reads each frame, acks
//! newly `LAUNCH_REQUESTED` cues, and POSTs `/report` per runnable cue. On
//! `SetTarget`/`SetDb` it hydrates the slot from the store so a cold/offline start
//! drives at once. SaaS never computes frames — the REACT strobe slider stays
//! authoritative.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::lighting_cue::{ActiveCue, CapabilityMiss, CueApplyReport, CueDto, CueListResponse};
use crate::db::Database;

// Slow heartbeat: the local store drives the show, so SaaS is contacted only to
// catch SaaS/MCP-originated launches (a nicety) and to flush acks/reports — a
// running show makes ~zero `/cues` traffic. Operator-launched cues are written
// straight to the local store and drive instantly, so this interval is NOT the
// operator's launch latency. SetTarget/SetDb still force an immediate poll.
const POLL_INTERVAL: Duration = Duration::from_millis(15_000);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const EXECUTOR_ID: &str = "react-main";

enum Cmd {
    SetTarget {
        base_url: String,
        token: String,
        stage_id: i64,
    },
    UpdateToken(String),
    /// Attach the local SQLite store (mirror of `StageSyncWorker`'s `SetDb`).
    SetDb(Database),
    /// Operator launched a cached cue locally (zero SaaS): persist + publish now.
    LaunchLocal { launch_id: String, cue_json: String },
    /// Operator stopped a local cue: drop it so the renderer fades it out.
    StopLocal { launch_id: String },
    ClearTarget,
    Shutdown,
}

/// Renderer-facing handle. Cheap, non-blocking methods; the thread owns all HTTP.
pub struct LightingCueWorker {
    cmd_tx: mpsc::Sender<Cmd>,
    cues: Arc<Mutex<Vec<ActiveCue>>>,
    reports: Arc<Mutex<HashMap<String, CueApplyReport>>>,
    thread: Option<JoinHandle<()>>,
}

impl LightingCueWorker {
    pub fn new() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
        let cues = Arc::new(Mutex::new(Vec::new()));
        let reports = Arc::new(Mutex::new(HashMap::new()));
        let cues_t = cues.clone();
        let reports_t = reports.clone();
        let thread = thread::Builder::new()
            .name("lighting-cue".into())
            .spawn(move || run(cmd_rx, cues_t, reports_t))
            .ok();
        Self {
            cmd_tx,
            cues,
            reports,
            thread,
        }
    }

    /// Point the worker at a stage and start polling its cues.
    pub fn set_target(&self, base_url: String, token: String, stage_id: i64) {
        let _ = self.cmd_tx.send(Cmd::SetTarget {
            base_url,
            token,
            stage_id,
        });
    }

    pub fn update_token(&self, token: String) {
        let _ = self.cmd_tx.send(Cmd::UpdateToken(token));
    }

    /// Attach the local SQLite store. The worker then mirrors fetched cues into it
    /// (offline durability + cache-from-launches) and hydrates the renderer slot
    /// from it on `set_target` so a cold/offline start drives at once.
    pub fn set_db(&self, db: Database) {
        let _ = self.cmd_tx.send(Cmd::SetDb(db));
    }

    /// Operator launches a cached cue locally — drives instantly, zero SaaS.
    pub fn launch_local(&self, launch_id: String, cue_json: String) {
        let _ = self.cmd_tx.send(Cmd::LaunchLocal { launch_id, cue_json });
    }

    /// Operator stops a local cue — the renderer fades it from its last-known state.
    pub fn stop_local(&self, launch_id: String) {
        let _ = self.cmd_tx.send(Cmd::StopLocal { launch_id });
    }

    pub fn clear_target(&self) {
        let _ = self.cmd_tx.send(Cmd::ClearTarget);
    }

    /// Current active cues (cloned) for the per-frame resolver.
    pub fn active_cues(&self) -> Vec<ActiveCue> {
        self.cues.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// Publish the latest per-cue apply report (keyed by `launch_id`) for the
    /// worker to POST on its next `/report` tick.
    pub fn publish_reports(&self, reports: HashMap<String, CueApplyReport>) {
        if let Ok(mut slot) = self.reports.lock() {
            *slot = reports;
        }
    }
}

impl Drop for LightingCueWorker {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Cmd::Shutdown);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

struct Target {
    base_url: String,
    token: String,
    stage_id: i64,
}

fn run(
    cmd_rx: mpsc::Receiver<Cmd>,
    cues: Arc<Mutex<Vec<ActiveCue>>>,
    reports: Arc<Mutex<HashMap<String, CueApplyReport>>>,
) {
    let mut target: Option<Target> = None;
    let mut db: Option<Database> = None;
    let mut acked: HashSet<String> = HashSet::new();
    // The last-known live SaaS cue set (incl STOP_REQUESTED for the fade). The
    // published slot is always this ++ the operator-local overlay from the store,
    // so a local launch survives the next SaaS poll.
    let mut live_saas: Vec<ActiveCue> = Vec::new();
    // Poll right away on first target.
    let mut last_poll = Instant::now() - POLL_INTERVAL;
    // Structural commands (SetTarget/SetDb/...) drained mid-poll by
    // `drain_local_cmds` are re-queued here and processed through the normal
    // match below, ahead of the channel, in arrival order.
    let mut queued: VecDeque<Cmd> = VecDeque::new();

    loop {
        let wake = POLL_INTERVAL
            .checked_sub(last_poll.elapsed())
            .unwrap_or_default()
            .max(Duration::from_millis(50));
        let next = if let Some(cmd) = queued.pop_front() {
            Ok(cmd)
        } else {
            cmd_rx.recv_timeout(wake)
        };
        match next {
            Ok(Cmd::SetTarget {
                base_url,
                token,
                stage_id,
            }) => {
                target = Some(Target {
                    base_url,
                    token,
                    stage_id,
                });
                acked.clear();
                clear_shared(&cues, &reports);
                // Drive immediately from the local store while the first fetch is
                // in flight — and indefinitely if offline (the show never waits on
                // SaaS). A successful poll overwrites the SaaS set with fresh state.
                live_saas = if let Some(d) = db.as_ref() {
                    // Switching stages: drop other stages' stale SaaS rows so they
                    // can't resurrect a stopped cue if the operator returns; keep
                    // this stage's rows + all operator-local rows for offline drive.
                    let _ = d.prune_saas_active_cues_other_stages(stage_id);
                    let saas = saas_from_store(d, stage_id);
                    publish_slot(&cues, &saas, Some(d), stage_id);
                    saas
                } else {
                    Vec::new()
                };
                last_poll = Instant::now() - POLL_INTERVAL; // poll on next loop
            }
            Ok(Cmd::UpdateToken(tok)) => {
                if let Some(t) = target.as_mut() {
                    if t.token != tok {
                        t.token = tok;
                        // New token (login / refresh): re-poll promptly so freshly
                        // available connectivity fetches cues AND promotes any
                        // operator-local cues up to SaaS without waiting ~15s.
                        last_poll = Instant::now() - POLL_INTERVAL;
                    }
                }
            }
            Ok(Cmd::SetDb(d)) => {
                db = Some(d);
                // If a stage is already selected, seed from the freshly-attached
                // store now (handles SetDb arriving after SetTarget).
                if let (Some(d), Some(t)) = (db.as_ref(), target.as_ref()) {
                    let _ = d.prune_saas_active_cues_other_stages(t.stage_id);
                    live_saas = saas_from_store(d, t.stage_id);
                    publish_slot(&cues, &live_saas, Some(d), t.stage_id);
                }
            }
            Ok(Cmd::LaunchLocal { launch_id, cue_json }) => {
                // Operator launch: persist as an origin='local' ACTIVE cue and
                // republish so the renderer drives it THIS frame — zero SaaS.
                if let (Some(d), Some(t)) = (db.as_ref(), target.as_ref()) {
                    let _ = d.upsert_active_cue(&launch_id, t.stage_id, "ACTIVE", &cue_json, "local");
                    publish_slot(&cues, &live_saas, Some(d), t.stage_id);
                }
            }
            Ok(Cmd::StopLocal { launch_id }) => {
                // Operator stop: drop the row + republish FIRST — the cue
                // disappears from the slot and the renderer fades it out THIS
                // frame (anti-jank, no hard cut). Only then is a promoted twin's
                // SaaS stop POSTed, best-effort: the old order ran the (up to
                // 10s) POST before unpublishing, so on a flaky venue network an
                // operator's stop on a STROBE cue kept strobing for the full
                // timeout. A failed POST records a durable pending-stop that
                // `poll_once` retries, so an offline stop still reaches SaaS.
                if let (Some(d), Some(t)) = (db.as_ref(), target.as_ref()) {
                    stop_local(d, t, &cues, &live_saas, &launch_id, true);
                }
            }
            Ok(Cmd::ClearTarget) => {
                target = None;
                acked.clear();
                live_saas.clear();
                clear_shared(&cues, &reports);
            }
            Ok(Cmd::Shutdown) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }

        // Don't start a poll while structural commands are still queued — they
        // may change the target/db the poll would run against.
        if queued.is_empty() && last_poll.elapsed() >= POLL_INTERVAL {
            if let Some(t) = target.as_ref() {
                let outcome = poll_once(
                    t,
                    &reports,
                    &mut acked,
                    db.as_ref(),
                    &cues,
                    &live_saas,
                    &cmd_rx,
                );
                let structural = !outcome.deferred.is_empty();
                queued.extend(outcome.deferred);
                if structural {
                    // A SetTarget/SetDb/ClearTarget/... arrived mid-poll: the
                    // fetched set may belong to the outgoing target, so discard
                    // it — the structural command re-seeds and forces a re-poll.
                } else if let Some(saas) = outcome.fetched {
                    live_saas = saas;
                    publish_slot(&cues, &live_saas, db.as_ref(), t.stage_id);
                }
            }
            last_poll = Instant::now();
        }
    }
}

fn clear_shared(
    cues: &Arc<Mutex<Vec<ActiveCue>>>,
    reports: &Arc<Mutex<HashMap<String, CueApplyReport>>>,
) {
    if let Ok(mut c) = cues.lock() {
        c.clear();
    }
    if let Ok(mut r) = reports.lock() {
        r.clear();
    }
}

/// Parse stored cue JSONs into `ActiveCue`s (shared by the SaaS seed + the local
/// overlay merge).
fn cues_from_jsons(jsons: Vec<String>) -> Vec<ActiveCue> {
    jsons
        .iter()
        .filter_map(|json| serde_json::from_str::<CueDto>(json).ok())
        .map(|dto| ActiveCue::from_dto(&dto))
        .collect()
}

/// Build the live SaaS cue set from the local store (origin='saas') for a stage,
/// used to seed it on a cold/offline start so the show drives immediately without
/// waiting on a fetch. Only drivable cues were persisted, so all are runnable.
fn saas_from_store(db: &Database, stage_id: i64) -> Vec<ActiveCue> {
    cues_from_jsons(db.active_cue_jsons_for_stage(stage_id, Some("saas")))
}

/// Publish the renderer slot = live SaaS cues (incl STOP_REQUESTED for the fade)
/// ++ the operator-local overlay (origin='local') from the store. This is the
/// single place the slot is composed, so a local launch survives the SaaS poll
/// and an offline restart re-drives both sets. Cues with an UNDELIVERED stop
/// (`pending_stops`) are filtered out: the operator already stopped them, so
/// their promoted SaaS twin must not re-drive while the `/stop` POST is still
/// being retried.
fn publish_slot(
    cues: &Arc<Mutex<Vec<ActiveCue>>>,
    live_saas: &[ActiveCue],
    db: Option<&Database>,
    stage_id: i64,
) {
    let mut slot_cues: Vec<ActiveCue> = live_saas.to_vec();
    if let Some(db) = db {
        slot_cues.extend(cues_from_jsons(
            db.active_cue_jsons_for_stage(stage_id, Some("local")),
        ));
        let pending = pending_stops(db);
        if !pending.is_empty() {
            slot_cues.retain(|c| !pending.iter().any(|p| p == &c.launch_id));
        }
    }
    if let Ok(mut slot) = cues.lock() {
        *slot = slot_cues;
    }
}

// ---- COM-499 operator stop: unpublish-first ordering + durable pending stops ----

/// settings-KV key holding the JSON array of SaaS launch_ids whose `/stop`
/// POST has not been delivered yet (the operator stopped a promoted local cue
/// while offline / on a flaky link). Retried at the top of every poll;
/// `publish_slot` filters these ids so the twin cannot re-drive meanwhile.
const PENDING_STOPS_KEY: &str = "lighting_cue_pending_stops";

/// Decode the stored pending-stop list (pure — unit-tested without SQLite).
fn parse_pending_stops(raw: Option<String>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn pending_stops(db: &Database) -> Vec<String> {
    parse_pending_stops(db.get_setting(PENDING_STOPS_KEY))
}

fn save_pending_stops(db: &Database, ids: &[String]) {
    let json = serde_json::to_string(ids).unwrap_or_else(|_| "[]".to_string());
    let _ = db.set_setting(PENDING_STOPS_KEY, &json);
}

fn add_pending_stop(db: &Database, server_id: &str) {
    let mut ids = pending_stops(db);
    if !ids.iter().any(|i| i == server_id) {
        ids.push(server_id.to_string());
        save_pending_stops(db, &ids);
    }
}

/// Operator stop of a local cue. The row's promotion link is read BEFORE the
/// row is removed (it lives on the row), then the sequencing core runs — see
/// [`stop_local_sequence`] for the ordering contract. `attempt_post` is false
/// when called from the mid-poll command drain (the network is known-slow
/// there; the pending-stop flush delivers it instead).
fn stop_local(
    db: &Database,
    target: &Target,
    cues: &Arc<Mutex<Vec<ActiveCue>>>,
    live_saas: &[ActiveCue],
    launch_id: &str,
    attempt_post: bool,
) {
    let promoted = db.active_cue_promoted_to(launch_id);
    stop_local_sequence(
        promoted,
        attempt_post,
        || {
            let _ = db.remove_active_cue(launch_id);
            publish_slot(cues, live_saas, Some(db), target.stage_id);
        },
        |server_id| post_stop(&target.base_url, &target.token, server_id),
        |server_id| add_pending_stop(db, server_id),
    );
}

/// Sequencing core of an operator StopLocal (the fix for the 10s stop stall):
///
/// 1. `unpublish` runs FIRST — the cue leaves the store + renderer slot and
///    starts its fade before any network I/O can stall it.
/// 2. A promoted twin's SaaS stop is then POSTed best-effort (when
///    `attempt_post`); a failure — or a skipped attempt — records a durable
///    pending stop that `poll_once` retries until it lands.
///
/// Factored over closures so the ordering is unit-testable without HTTP/SQLite.
fn stop_local_sequence(
    promoted_to: Option<String>,
    attempt_post: bool,
    unpublish: impl FnOnce(),
    post: impl FnOnce(&str) -> Result<(), String>,
    record_pending: impl FnOnce(&str),
) {
    unpublish();
    let Some(server_id) = promoted_to else { return };
    if attempt_post {
        match post(&server_id) {
            Ok(()) => return,
            Err(err) => log::warn!(
                "[lighting-cue] stop of promoted twin {} failed (queued for retry): {}",
                server_id,
                err
            ),
        }
    }
    record_pending(&server_id);
}

/// Retry undelivered `/stop` POSTs (durable pending stops) before a poll, so
/// the subsequent fetch already reflects them. Stops on the FIRST failure —
/// one dead POST means the network is down, and hammering it would stall the
/// worker thread for 10s per remaining id.
fn flush_pending_stops(target: &Target, db: &Database) {
    let ids = pending_stops(db);
    if ids.is_empty() {
        return;
    }
    let mut remaining: Vec<String> = Vec::new();
    let mut iter = ids.into_iter();
    for id in iter.by_ref() {
        match post_stop(&target.base_url, &target.token, &id) {
            Ok(()) => {
                // The twin may have been mirrored into the store before the stop
                // landed — drop it so an offline restart can't re-drive it.
                let _ = db.remove_active_cue(&id);
                log::info!("[lighting-cue] delivered pending stop {}", id);
            }
            Err(err) => {
                log::warn!("[lighting-cue] pending stop {} still undeliverable: {}", id, err);
                remaining.push(id);
                break;
            }
        }
    }
    remaining.extend(iter);
    save_pending_stops(db, &remaining);
}

/// Service operator commands that arrived while poll HTTP was in flight (the
/// fix for launch/stop commands sitting unprocessed for 30-50s behind serial
/// 10s-timeout HTTP calls on a degraded network — defeating the "local drives
/// instantly" promise). `LaunchLocal`/`StopLocal` apply their local-store op +
/// republish immediately; anything structural (SetTarget / SetDb / Shutdown /
/// ...) is deferred back to the run loop in arrival order.
fn drain_local_cmds(
    cmd_rx: &mpsc::Receiver<Cmd>,
    target: &Target,
    db: Option<&Database>,
    cues: &Arc<Mutex<Vec<ActiveCue>>>,
    live_saas: &[ActiveCue],
    deferred: &mut Vec<Cmd>,
) {
    while let Ok(cmd) = cmd_rx.try_recv() {
        match cmd {
            Cmd::LaunchLocal { launch_id, cue_json } => {
                if let Some(d) = db {
                    let _ = d.upsert_active_cue(&launch_id, target.stage_id, "ACTIVE", &cue_json, "local");
                    publish_slot(cues, live_saas, Some(d), target.stage_id);
                }
            }
            Cmd::StopLocal { launch_id } => {
                if let Some(d) = db {
                    // No inline POST mid-poll — we are here precisely because the
                    // network is slow. The pending-stop flush delivers it.
                    stop_local(d, target, cues, live_saas, &launch_id, false);
                }
            }
            other => deferred.push(other),
        }
    }
}

/// What a poll produced: the fetched SaaS set (None on fetch error / offline,
/// so the run loop leaves slot + store untouched), plus any structural
/// commands the mid-poll drain pulled off the channel for the run loop to
/// process (in arrival order).
struct PollOutcome {
    fetched: Option<Vec<ActiveCue>>,
    deferred: Vec<Cmd>,
}

/// Fetch the live SaaS cues, reconcile them into the local store, ack/report.
/// Returns the fetched SaaS `ActiveCue`s (incl STOP_REQUESTED) for the run loop
/// to publish as `live_saas`; `fetched: None` on a fetch error (offline) so the
/// run loop leaves the slot + store untouched and the show keeps driving.
///
/// Every blocking HTTP call here carries a 10s timeout, and a degraded network
/// can chain several — so pending operator commands are drained between calls
/// (`drain_local_cmds`) to keep LaunchLocal/StopLocal instant.
#[allow(clippy::too_many_arguments)]
fn poll_once(
    target: &Target,
    reports: &Arc<Mutex<HashMap<String, CueApplyReport>>>,
    acked: &mut HashSet<String>,
    db: Option<&Database>,
    cues: &Arc<Mutex<Vec<ActiveCue>>>,
    live_saas: &[ActiveCue],
    cmd_rx: &mpsc::Receiver<Cmd>,
) -> PollOutcome {
    let mut deferred: Vec<Cmd> = Vec::new();

    // Deliver any durable pending stops FIRST so the fetch below already
    // reflects them (a stopped-while-offline cue won't come back ACTIVE).
    if let Some(d) = db {
        flush_pending_stops(target, d);
        drain_local_cmds(cmd_rx, target, db, cues, live_saas, &mut deferred);
    }

    let dtos = match fetch_cues(&target.base_url, &target.token, target.stage_id) {
        Ok(dtos) => dtos,
        Err(err) => {
            // Offline / unreachable: leave the renderer slot AND the local store
            // exactly as they are. The show keeps driving the last-known cues
            // (in memory now; from the store after a restart). No SaaS dependency.
            log::warn!("[lighting-cue] cue fetch failed: {}", err);
            drain_local_cmds(cmd_rx, target, db, cues, live_saas, &mut deferred);
            return PollOutcome { fetched: None, deferred };
        }
    };
    drain_local_cmds(cmd_rx, target, db, cues, live_saas, &mut deferred);
    let active: Vec<ActiveCue> = dtos.iter().map(ActiveCue::from_dto).collect();

    // Mirror the drivable cues into the local store (cache-from-launches +
    // offline durability), remember each as a re-launchable definition, then
    // prune SaaS-origin rows the server no longer returns. Operator-local cues
    // (origin = 'local') are left untouched.
    if let Some(db) = db {
        // Persist only cues this rig can actually drive: runnable status AND >=1
        // parsed binding (rejection_reason None), so the offline store never holds
        // a cue the executor rejected.
        let mut keep: HashSet<&str> = HashSet::new();
        for (d, ac) in dtos.iter().zip(active.iter()) {
            if !matches!(d.status.as_str(), "ACTIVE" | "LAUNCH_REQUESTED")
                || ac.rejection_reason().is_some()
            {
                continue;
            }
            keep.insert(d.launch_id.as_str());
            if let Ok(json) = serde_json::to_string(d) {
                let _ =
                    db.upsert_active_cue(&d.launch_id, target.stage_id, &d.status, &json, "saas");
                // cache-from-launches: remember the definition so the operator can
                // re-launch it locally, even offline. Only cache cues with a STABLE
                // re-launchable identity (cue_profile_id / effect_id) — skip id-less
                // ad-hoc cues so the picker doesn't fill with one-off UUID rows. The
                // latest launch's variables win.
                if let Some(key) = d.cue_profile_id.clone().or_else(|| d.effect_id.clone()) {
                    let label = d
                        .metadata
                        .as_ref()
                        .and_then(|m| m.effect_name.clone())
                        .or_else(|| d.cue_profile_id.clone())
                        .or_else(|| d.effect_id.clone())
                        .unwrap_or_else(|| key.clone());
                    let _ = db.upsert_cue_definition(&key, &label, &json, target.stage_id);
                }
            }
        }
        for id in db.active_cue_ids_for_stage(target.stage_id, Some("saas")) {
            if !keep.contains(id.as_str()) {
                let _ = db.remove_active_cue(&id);
            }
        }
    }

    // COM-499 promote-on-connectivity (idempotent): push operator-local cues UP to
    // SaaS so they become the system-of-record (visible to the operator grid / MCP).
    // Each promote stamps the launch_contract with `react_origin_id` = the local
    // launch_id; SaaS stores + echoes it, so on a later fetch we recognise our OWN
    // twin even if the POST response was lost or we crashed mid-promote — and NEVER
    // double-POST. When the twin appears we swap (drop the local row; the twin,
    // origin='saas', drives — identical bindings, so no gap / no double-drive).
    // Offline this never runs: the local cue keeps driving and retries next tick.
    if let Some(db) = db {
        for (lid, cue_json, promoted_to) in db.local_active_cues_for_promote(target.stage_id) {
            let twin = dtos.iter().find(|d| {
                d.launch_contract
                    .as_ref()
                    .and_then(|lc| lc.react_origin_id.as_deref())
                    == Some(lid.as_str())
            });
            match (twin, promoted_to.is_some()) {
                (Some(t), _) => {
                    // Our SaaS twin exists -> swap: drop the local row, the twin drives.
                    let _ = db.remove_active_cue(&lid);
                    log::info!("[lighting-cue] local {} swapped -> saas {}", lid, t.launch_id);
                }
                (None, true) => {
                    // Was promoted, but the twin is gone from SaaS (stopped / reaped
                    // server-side) -> stop locally too; don't strand or re-drive it.
                    let _ = db.remove_active_cue(&lid);
                    log::info!("[lighting-cue] promoted local {} gone from SaaS -> stopped", lid);
                }
                (None, false) => {
                    // Not yet on SaaS -> promote now, stamped for idempotency.
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&cue_json) {
                        // REACT's LaunchContractDto drops launch_contract.cue_profile_id
                        // on round-trip; inject it from the top-level field. Stamp
                        // react_origin_id so we recognise our twin idempotently.
                        let mut lc = value
                            .get("launch_contract")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
                        if let Some(obj) = lc.as_object_mut() {
                            if let Some(cpid) = value.get("cue_profile_id") {
                                if !cpid.is_null() {
                                    obj.insert("cue_profile_id".to_string(), cpid.clone());
                                }
                            }
                            obj.insert(
                                "react_origin_id".to_string(),
                                serde_json::Value::String(lid.clone()),
                            );
                        }
                        let has_bindings = lc
                            .get("bindings")
                            .and_then(|b| b.as_array())
                            .map_or(false, |a| !a.is_empty());
                        let has_cpid = lc.get("cue_profile_id").map_or(false, |v| !v.is_null());
                        if has_bindings && has_cpid {
                            let body = serde_json::json!({
                                "stage_id": target.stage_id,
                                "launch_contract": lc,
                                "variables": value.get("variables"),
                                "requested_by": "react-operator",
                            });
                            match post_launch(&target.base_url, &target.token, &body) {
                                Ok(server_id) => {
                                    let _ = db.set_active_cue_promoted(&lid, &server_id);
                                    log::info!(
                                        "[lighting-cue] promoted local {} -> saas {}",
                                        lid,
                                        server_id
                                    );
                                }
                                // On a lost response the cue may still have committed
                                // server-side; we do NOT re-POST blindly — the twin
                                // check above adopts it next poll via react_origin_id.
                                Err(err) => log::warn!(
                                    "[lighting-cue] promote {} failed (will adopt twin if it landed): {}",
                                    lid,
                                    err
                                ),
                            }
                        } else {
                            log::warn!(
                                "[lighting-cue] cannot promote local {} (launch_contract missing cue_profile_id/bindings)",
                                lid
                            );
                        }
                    }
                }
            }
            // Each promote is a blocking POST — keep operator commands flowing.
            drain_local_cmds(cmd_rx, target, Some(db), cues, live_saas, &mut deferred);
        }
    }

    // Prune acks for cues that are gone, then ack newly launched ones.
    let present: HashSet<String> = active.iter().map(|c| c.launch_id.clone()).collect();
    acked.retain(|id| present.contains(id));
    for cue in active.iter().filter(|c| c.is_runnable()) {
        if cue.status == "LAUNCH_REQUESTED" && !acked.contains(&cue.launch_id) {
            // COM-499 rules 2/4: accept a cue with any runnable binding; reject a
            // cue the rig cannot run at all (server then moves it to
            // STOP_REQUESTED). Either way it is acked exactly once (rule 1).
            let reason = cue.rejection_reason();
            let accepted = reason.is_none();
            match post_ack(
                &target.base_url,
                &target.token,
                &cue.launch_id,
                accepted,
                reason.as_deref(),
            ) {
                Ok(()) => {
                    acked.insert(cue.launch_id.clone());
                    if accepted {
                        log::info!(
                            "[lighting-cue] acked {} (profile={:?} effect={:?} stage={:?} role={:?} strobe_policy={:?})",
                            cue.launch_id,
                            cue.cue_profile_id,
                            cue.effect_id,
                            cue.stage_id,
                            cue.runtime_role,
                            cue.strobe_policy,
                        );
                    } else {
                        log::warn!(
                            "[lighting-cue] rejected {}: {}",
                            cue.launch_id,
                            reason.as_deref().unwrap_or("")
                        );
                    }
                }
                Err(err) => log::warn!("[lighting-cue] ack {} failed: {}", cue.launch_id, err),
            }
            // Each ack is a blocking POST — keep operator commands flowing.
            drain_local_cmds(cmd_rx, target, db, cues, live_saas, &mut deferred);
        }
    }

    // Report the latest per-cue apply state (server stores latest only). Only
    // ACTIVE cues get a normal report: LAUNCH_REQUESTED is not a valid report
    // status (contract), and a just-acked cue is still LAUNCH_REQUESTED in this
    // snapshot — it flips to ACTIVE next poll. STOP_REQUESTED is handled below.
    let snapshot = reports.lock().map(|r| r.clone()).unwrap_or_default();
    for cue in active.iter().filter(|c| c.status == "ACTIVE") {
        let entry = snapshot.get(&cue.launch_id);
        let mut misses = entry
            .map(|r| dedup_misses(&r.capability_misses))
            .unwrap_or_default();
        // Fold in out-of-range variable clamps (A.9) alongside capability misses.
        misses.extend(cue.var_clamp_misses().iter().cloned());
        let body = ReportBody {
            status: cue.status.as_str(),
            active_variables: &cue.variables,
            skipped_bindings: cue.parse_skips(),
            capability_misses: misses,
            last_frame_at: entry.and_then(|r| r.last_frame_at.as_deref()),
            error: entry.and_then(|r| r.error.as_deref()),
            executor_id: EXECUTOR_ID,
        };
        if let Err(err) = post_report(&target.base_url, &target.token, &cue.launch_id, &body) {
            log::warn!("[lighting-cue] report {} failed: {}", cue.launch_id, err);
        }
        // Each report is a blocking POST — keep operator commands flowing.
        drain_local_cmds(cmd_rx, target, db, cues, live_saas, &mut deferred);
    }

    // COM-499 rule #8: a STOP_REQUESTED cue is no longer applied (not runnable),
    // so close it out by reporting STOPPED — otherwise it lingers in the active
    // list forever (the cue never transitions out of STOP_REQUESTED).
    for cue in active.iter().filter(|c| c.status == "STOP_REQUESTED") {
        let body = ReportBody {
            status: "STOPPED",
            active_variables: &cue.variables,
            skipped_bindings: &[],
            // Carry variable clamps into the final report too, so a cue that
            // stops before it was ever ACTIVE still surfaces them (A.9).
            capability_misses: cue.var_clamp_misses().to_vec(),
            last_frame_at: None,
            error: None,
            executor_id: EXECUTOR_ID,
        };
        if let Err(err) = post_report(&target.base_url, &target.token, &cue.launch_id, &body) {
            log::warn!("[lighting-cue] stop-report {} failed: {}", cue.launch_id, err);
        }
        drain_local_cmds(cmd_rx, target, db, cues, live_saas, &mut deferred);
    }

    PollOutcome {
        fetched: Some(active),
        deferred,
    }
}

fn dedup_misses(misses: &[CapabilityMiss]) -> Vec<CapabilityMiss> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for m in misses {
        if seen.insert(m.target.clone()) {
            out.push(m.clone());
        }
    }
    out
}

// ---- HTTP (mirrors the ureq Bearer pattern in stage_sync.rs) ----

/// ISO-8601 UTC timestamp (`…Z`, seconds precision) for ack `observed_at`.
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn fetch_cues(base_url: &str, token: &str, stage_id: i64) -> Result<Vec<CueDto>, String> {
    let url = format!("{}/api/v1/lighting/cues?stage_id={}", base_url, stage_id);
    let resp: CueListResponse = ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .timeout(REQUEST_TIMEOUT)
        .call()
        .map_err(|e| format!("cue fetch: {}", e))?
        .into_json()
        .map_err(|e| format!("cue parse: {}", e))?;
    Ok(resp.cues)
}

fn post_ack(
    base_url: &str,
    token: &str,
    launch_id: &str,
    accepted: bool,
    rejected_reason: Option<&str>,
) -> Result<(), String> {
    let url = format!("{}/api/v1/lighting/cues/{}/ack", base_url, launch_id);
    let body = AckBody {
        accepted,
        rejected_reason,
        executor_id: EXECUTOR_ID,
        observed_at: now_iso(),
    };
    ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send_json(serde_json::to_value(body).map_err(|e| e.to_string())?)
        .map_err(|e| format!("ack: {}", e))?;
    Ok(())
}

fn post_report(base_url: &str, token: &str, launch_id: &str, body: &ReportBody) -> Result<(), String> {
    let url = format!("{}/api/v1/lighting/cues/{}/report", base_url, launch_id);
    ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send_json(serde_json::to_value(body).map_err(|e| e.to_string())?)
        .map_err(|e| format!("report: {}", e))?;
    Ok(())
}

/// POST a cue launch to SaaS (operator-local promote-on-connectivity). Returns the
/// server-minted launch_id from the 201 `cue.launch_id`.
fn post_launch(base_url: &str, token: &str, body: &serde_json::Value) -> Result<String, String> {
    let url = format!("{}/api/v1/lighting/cues", base_url);
    let resp: serde_json::Value = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send_json(body.clone())
        .map_err(|e| format!("launch: {}", e))?
        .into_json()
        .map_err(|e| format!("launch parse: {}", e))?;
    resp.get("cue")
        .and_then(|c| c.get("launch_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "launch response missing cue.launch_id".to_string())
}

/// POST a stop for a SaaS cue (when an operator stops a local cue that was already
/// promoted, so the server cue doesn't re-appear on the next poll).
fn post_stop(base_url: &str, token: &str, launch_id: &str) -> Result<(), String> {
    let url = format!("{}/api/v1/lighting/cues/{}/stop", base_url, launch_id);
    match ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send_json(serde_json::json!({}))
    {
        Ok(_) => Ok(()),
        // A definitive client-error status (cue already deleted/expired on
        // SaaS) means the stop is MOOT — calling it undeliverable would wedge
        // the durable pending-stop queue forever (flush stops on the first
        // failure) and keep filtering this launch_id out of the renderer
        // slot. 408/429 are transient and stay retryable.
        Err(ureq::Error::Status(code, _))
            if (400..500).contains(&code) && code != 408 && code != 429 =>
        {
            log::info!(
                "[lighting-cue] stop {} already moot on SaaS (HTTP {}) — treating as delivered",
                launch_id,
                code
            );
            Ok(())
        }
        Err(e) => Err(format!("stop: {}", e)),
    }
}

#[derive(Serialize)]
struct AckBody<'a> {
    accepted: bool,
    /// Why the executor refused the cue (COM-499 rule 4). `null` on accept.
    rejected_reason: Option<&'a str>,
    executor_id: &'static str,
    /// ISO-8601 UTC time the executor observed/decided the cue.
    observed_at: String,
}

#[derive(Serialize)]
struct ReportBody<'a> {
    status: &'a str,
    /// Live variable snapshot the executor is running with. `None`/`error` are
    /// emitted as JSON `null` so SaaS (which reads them by snake_case) and the
    /// COM-500 operator grid get the full contract shape every report.
    active_variables: &'a serde_json::Map<String, serde_json::Value>,
    skipped_bindings: &'a [String],
    capability_misses: Vec<CapabilityMiss>,
    last_frame_at: Option<&'a str>,
    error: Option<&'a str>,
    executor_id: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_body_carries_full_com499_shape() {
        // Slice 2e: the /report wire body must carry all 7 COM-499 fields with
        // the snake_case keys SaaS's report() reads (active_variables /
        // last_frame_at / error were previously absent), and `error` must
        // serialize as JSON null on success (not be omitted).
        let mut vars = serde_json::Map::new();
        vars.insert("intensity".into(), serde_json::json!(0.82));
        let body = ReportBody {
            status: "ACTIVE",
            active_variables: &vars,
            skipped_bindings: &["missing-prism".to_string()],
            capability_misses: vec![CapabilityMiss {
                target: "Prism".into(),
                reason: "fixture not patched".into(),
            }],
            last_frame_at: Some("2026-05-28T07:00:00Z"),
            error: None,
            executor_id: EXECUTOR_ID,
        };
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["status"], "ACTIVE");
        assert_eq!(v["active_variables"]["intensity"], 0.82);
        assert_eq!(v["skipped_bindings"][0], "missing-prism");
        assert_eq!(v["capability_misses"][0]["target"], "Prism");
        assert_eq!(v["last_frame_at"], "2026-05-28T07:00:00Z");
        assert!(
            v.get("error").map_or(false, |e| e.is_null()),
            "error emitted as null on success, got {:?}",
            v.get("error")
        );
        assert_eq!(v["executor_id"], "react-main");
    }

    #[test]
    fn ack_body_carries_reject_fields() {
        // Slice 2f: ack wire body must carry accepted + rejected_reason +
        // observed_at + executor_id (snake_case SaaS acknowledge() reads), with
        // rejected_reason serialized as JSON null on accept.
        let accept = serde_json::to_value(AckBody {
            accepted: true,
            rejected_reason: None,
            executor_id: EXECUTOR_ID,
            observed_at: "2026-05-28T07:00:00Z".to_string(),
        })
        .unwrap();
        assert_eq!(accept["accepted"], true);
        assert!(accept["rejected_reason"].is_null(), "null reason on accept");
        assert_eq!(accept["executor_id"], "react-main");
        assert_eq!(accept["observed_at"], "2026-05-28T07:00:00Z");

        let reject = serde_json::to_value(AckBody {
            accepted: false,
            rejected_reason: Some("no runnable bindings for this rig: x: ..."),
            executor_id: EXECUTOR_ID,
            observed_at: "2026-05-28T07:00:00Z".to_string(),
        })
        .unwrap();
        assert_eq!(reject["accepted"], false);
        assert_eq!(reject["rejected_reason"], "no runnable bindings for this rig: x: ...");
    }

    // ---- StopLocal sequencing (unpublish-first + durable pending stop) ----

    use std::cell::RefCell;

    /// Run [`stop_local_sequence`] against a recording log so the ORDER of
    /// effects is asserted, not just their presence.
    fn record_stop(promoted: Option<&str>, attempt_post: bool, post_ok: bool) -> Vec<String> {
        let log: RefCell<Vec<String>> = RefCell::new(Vec::new());
        stop_local_sequence(
            promoted.map(|s| s.to_string()),
            attempt_post,
            || log.borrow_mut().push("unpublish".to_string()),
            |id| {
                log.borrow_mut().push(format!("post:{}", id));
                if post_ok { Ok(()) } else { Err("connection timed out".to_string()) }
            },
            |id| log.borrow_mut().push(format!("pending:{}", id)),
        );
        log.into_inner()
    }

    #[test]
    fn stop_local_unpublishes_before_posting() {
        // The fix: the cue must leave the renderer slot BEFORE the (up to 10s)
        // SaaS stop POST — the old order held a strobing cue on screen for the
        // full timeout on a flaky venue network.
        assert_eq!(
            record_stop(Some("saas-1"), true, true),
            vec!["unpublish", "post:saas-1"],
            "unpublish strictly precedes the network call"
        );
    }

    #[test]
    fn failed_stop_post_records_durable_pending_stop() {
        assert_eq!(
            record_stop(Some("saas-1"), true, false),
            vec!["unpublish", "post:saas-1", "pending:saas-1"],
            "a failed POST leaves a durable pending stop for poll-time retry"
        );
    }

    #[test]
    fn unpromoted_stop_never_touches_network() {
        assert_eq!(
            record_stop(None, true, true),
            vec!["unpublish"],
            "a never-promoted local cue stops with zero SaaS traffic"
        );
    }

    #[test]
    fn drained_stop_skips_inline_post_but_queues_pending() {
        // From the mid-poll drain the network is known-slow: no inline POST,
        // straight to the durable pending stop (the flush delivers it).
        assert_eq!(
            record_stop(Some("saas-2"), false, true),
            vec!["unpublish", "pending:saas-2"],
        );
    }

    #[test]
    fn pending_stops_round_trip_and_tolerate_garbage() {
        assert!(parse_pending_stops(None).is_empty());
        assert!(parse_pending_stops(Some("not json".to_string())).is_empty());
        let ids = vec!["a".to_string(), "b".to_string()];
        let json = serde_json::to_string(&ids).unwrap();
        assert_eq!(parse_pending_stops(Some(json)), ids);
    }

    // ---- mid-poll command drain (operator latency under degraded network) ----

    #[test]
    fn drain_consumes_local_ops_and_defers_structural_cmds_in_order() {
        let (tx, rx) = mpsc::channel::<Cmd>();
        tx.send(Cmd::UpdateToken("tok-2".to_string())).unwrap();
        tx.send(Cmd::LaunchLocal {
            launch_id: "local-1".to_string(),
            cue_json: "{}".to_string(),
        })
        .unwrap();
        tx.send(Cmd::ClearTarget).unwrap();
        tx.send(Cmd::Shutdown).unwrap();

        let target = Target {
            base_url: "http://127.0.0.1:1".to_string(),
            token: "tok".to_string(),
            stage_id: 7,
        };
        let cues = Arc::new(Mutex::new(Vec::new()));
        let mut deferred = Vec::new();
        // db = None: the local ops are no-ops store-wise (matching the main
        // handlers, which also require an attached store) but MUST be consumed
        // — only structural commands flow back to the run loop, in order.
        drain_local_cmds(&rx, &target, None, &cues, &[], &mut deferred);

        assert_eq!(deferred.len(), 3, "UpdateToken + ClearTarget + Shutdown deferred");
        assert!(matches!(&deferred[0], Cmd::UpdateToken(t) if t == "tok-2"));
        assert!(matches!(deferred[1], Cmd::ClearTarget));
        assert!(matches!(deferred[2], Cmd::Shutdown));
        // And the channel is fully drained (the LaunchLocal was consumed).
        assert!(rx.try_recv().is_err());
    }
}

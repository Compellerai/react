//! Hue Dedicated Thread
//!
//! Runs Hue light mapping, RGB interpolation, and DTLS streaming on its own
//! thread at the protocol's native FPS (25 for v1, 50 for v2). Decoupled
//! from the main eframe render loop so DTLS sends cannot stall frames.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{self, Sender, Receiver};
use parking_lot::Mutex as PLMutex;

use crate::hue::{
    HueManager, HueConfig, HueState, HueProtocolVersion,
    HueMappingMode, LightRole, CueSnapshot,
};
use crate::lighting::FormFactor;

// ============================================================================
// HUE COMMAND (Main → Hue thread, infrequent)
// ============================================================================

/// Commands sent from the main thread to the Hue thread.
/// Delivered via unbounded crossbeam channel (all commands must arrive).
pub enum HueCommand {
    SetEnabled(bool),
    SetBridgeIp(String),
    SetEntertainmentArea(String),
    SetLightRole(u16, LightRole),
    SetLightFormFactor(u16, Option<FormFactor>),
    SetProtocolVersion(HueProtocolVersion),
    SetMappingMode(HueMappingMode),
    StartPairing,
    StoreCredentials { username: String, clientkey: String },
    Connect,
    Disconnect,
    FetchAreas,
    FetchLights,
    LoadConfig(HueConfig),
    Shutdown,
}

// ============================================================================
// HUE THREAD
// ============================================================================

/// Handle to the dedicated Hue thread.
///
/// Owns the communication channels and shared state. The main thread
/// interacts exclusively through this struct — never touching HueManager directly.
pub struct HueThread {
    /// Send commands to the thread
    command_tx: Sender<HueCommand>,
    /// Latest CueSnapshot (written by OSC thread, read by Hue thread)
    snapshot: Arc<PLMutex<CueSnapshot>>,
    /// Published HueState for UI (written by Hue thread)
    published_state: Arc<PLMutex<HueState>>,
    /// Config changes needing DB persistence (written by Hue thread, taken by main)
    pending_save: Arc<PLMutex<Option<HueConfig>>>,
    /// Thread join handle
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HueThread {
    /// Spawn the Hue thread.
    ///
    /// Takes the shared CueSnapshot Arc from the OSC thread so the Hue thread
    /// can read the latest lighting state without coupling to the main thread.
    pub fn new(snapshot: Arc<PLMutex<CueSnapshot>>) -> Self {
        let (command_tx, command_rx) = crossbeam_channel::unbounded::<HueCommand>();
        let published_state = Arc::new(PLMutex::new(HueState::default()));
        let pending_save = Arc::new(PLMutex::new(None::<HueConfig>));

        // Clone Arcs for the thread closure
        let t_snapshot = Arc::clone(&snapshot);
        let t_state = Arc::clone(&published_state);
        let t_save = Arc::clone(&pending_save);

        let thread = std::thread::Builder::new()
            .name("hue-thread".to_string())
            .spawn(move || {
                Self::thread_loop(command_rx, t_snapshot, t_state, t_save);
            })
            .expect("Failed to spawn Hue thread");

        Self {
            command_tx,
            snapshot,
            published_state,
            pending_save,
            thread: Some(thread),
        }
    }

    // === Commands ===

    /// Send a command to the Hue thread.
    pub fn send_command(&self, cmd: HueCommand) {
        let _ = self.command_tx.send(cmd);
    }

    // === Shared state readers ===

    /// Read the latest HueState (for UI display on main thread).
    pub fn state(&self) -> HueState {
        self.published_state.lock().clone()
    }

    /// Take the pending config save (if any). Returns None if no changes.
    /// The main thread calls this each frame and persists to DB when Some.
    pub fn take_pending_save(&self) -> Option<HueConfig> {
        self.pending_save.lock().take()
    }

    /// Check if the Hue thread is reporting as enabled (reads published state).
    pub fn is_enabled(&self) -> bool {
        self.published_state.lock().enabled
    }

    // === Lifecycle ===

    /// Shut down the Hue thread and wait for it to exit.
    pub fn shutdown(&mut self) {
        // Bounded join: the loop normally exits within a frame, but the DTLS
        // teardown in HueManager's Drop could block — don't let close hang on
        // it (abandon after a short grace; the process exit reclaims it).
        let _ = self.command_tx.send(HueCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            crate::thread_util::join_or_abandon(thread, crate::thread_util::SHUTDOWN_GRACE);
        }
    }

    // === Thread loop ===

    fn thread_loop(
        command_rx: Receiver<HueCommand>,
        snapshot: Arc<PLMutex<CueSnapshot>>,
        published_state: Arc<PLMutex<HueState>>,
        pending_save: Arc<PLMutex<Option<HueConfig>>>,
    ) {
        log::info!("[HueThread] Started");

        let mut manager = HueManager::new();
        let mut target_fps: f32 = manager.config().protocol_version.max_fps();

        loop {
            let tick_start = Instant::now();
            let mut config_changed = false;

            // 1. Drain all commands (non-blocking)
            loop {
                match command_rx.try_recv() {
                    Ok(cmd) => match cmd {
                        HueCommand::Shutdown => {
                            log::info!("[HueThread] Shutdown received");
                            // HueManager's Drop will clean up streamer
                            return;
                        }
                        HueCommand::SetEnabled(enabled) => {
                            manager.set_enabled(enabled);
                            config_changed = true;
                        }
                        HueCommand::SetBridgeIp(ip) => {
                            manager.set_bridge_ip(ip);
                            config_changed = true;
                        }
                        HueCommand::SetEntertainmentArea(area_id) => {
                            manager.set_entertainment_area(area_id);
                            manager.fetch_lights();
                            config_changed = true;
                        }
                        HueCommand::SetLightRole(channel_id, role) => {
                            manager.set_light_role(channel_id, role);
                            config_changed = true;
                        }
                        HueCommand::SetLightFormFactor(channel_id, form_factor) => {
                            manager.set_light_form_factor(channel_id, form_factor);
                            config_changed = true;
                        }
                        HueCommand::SetProtocolVersion(version) => {
                            manager.set_protocol_version(version);
                            target_fps = version.max_fps();
                            config_changed = true;
                        }
                        HueCommand::SetMappingMode(mode) => {
                            manager.set_mapping_mode(mode);
                            config_changed = true;
                        }
                        HueCommand::StartPairing => {
                            manager.start_pairing();
                        }
                        HueCommand::StoreCredentials { username, clientkey } => {
                            manager.store_credentials(username, clientkey);
                            config_changed = true;
                        }
                        HueCommand::Connect => {
                            manager.connect();
                        }
                        HueCommand::Disconnect => {
                            manager.disconnect();
                        }
                        HueCommand::FetchAreas => {
                            manager.fetch_areas();
                        }
                        HueCommand::FetchLights => {
                            manager.fetch_lights();
                        }
                        HueCommand::LoadConfig(config) => {
                            let enabled = config.enabled;
                            let has_creds = !config.username.is_empty() && !config.clientkey.is_empty();
                            let has_area = !config.entertainment_area_id.is_empty();
                            let needs_archetype_fetch = has_creds && has_area
                                && config.light_assignments.iter().any(|a| a.auto_form_factor == FormFactor::Unknown);
                            target_fps = config.protocol_version.max_fps();
                            manager.set_config(config);
                            if enabled {
                                manager.set_enabled(true);
                            }
                            if needs_archetype_fetch {
                                log::info!("[HueThread] Lights have Unknown form factors — fetching archetypes");
                                manager.fetch_lights();
                            }
                        }
                    },
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        log::info!("[HueThread] Command channel disconnected, exiting");
                        return;
                    }
                }
            }

            // 2. Apply pending operations (background thread results)
            config_changed |= manager.apply_pending_credentials();
            manager.apply_pending_connect();
            config_changed |= manager.sync_light_assignments_from_state();

            // 3. If config changed, publish for main thread to persist
            if config_changed {
                *pending_save.lock() = Some(manager.config().clone());
            }

            // 4. Read latest CueSnapshot and update Hue lights
            if manager.is_enabled() {
                let snap = snapshot.lock().clone();
                manager.update(&snap);
            }

            // 5. Publish state for UI
            *published_state.lock() = manager.state();

            // 6. Sleep remaining time to hit target FPS
            let target_interval = Duration::from_secs_f32(1.0 / target_fps);
            let elapsed = tick_start.elapsed();
            if elapsed < target_interval {
                std::thread::sleep(target_interval - elapsed);
            }
        }
    }
}

impl Drop for HueThread {
    fn drop(&mut self) {
        self.shutdown();
    }
}

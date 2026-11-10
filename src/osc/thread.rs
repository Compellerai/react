//! Lighting engine host thread (historically the "OSC thread").
//!
//! Runs the `LightingEngine` (crate::emerge::engine) on its own ~100Hz clock,
//! decoupled from BOTH the main eframe render loop AND the audio feed: the
//! engine republishes the `CueSnapshot` + UI state EVERY tick from its cached
//! input + the CURRENT trigger/operator state, so a held manual trigger
//! (BLINDER / STROBE / UV) reaches every consumer (DMX wire, virtual-stage
//! preview, OSC, Hue, WLED, UI) within one tick — with or without audio.
//!
//! The main thread sends audio input (`send_input`, overwrite-latest) and
//! commands; this thread publishes the results and hands the engine's OSC
//! messages to the `OscManager` output adapter — OSC is one consumer among
//! peers, not the engine's owner.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{self, Sender, Receiver};
use parking_lot::Mutex as PLMutex;

use crate::emerge::cue::TriggerButton;
use crate::emerge::engine::LightingEngine;
use crate::hue::CueSnapshot;
use crate::osc::{OscManager, OscConfig, MaestroData, TargetProfile};

pub use crate::emerge::engine::{LightingInput, OscUiState};

/// Target tick rate for the engine thread (100Hz = 10ms per tick)
const TICK_INTERVAL: Duration = Duration::from_millis(10);

// ============================================================================
// OSC COMMAND (Main → engine thread, infrequent)
// ============================================================================

/// Commands sent from the main thread to the engine thread.
/// Delivered via unbounded crossbeam channel (all commands must arrive).
pub enum OscCommand {
    SetEnabled(bool),
    SetTarget(String, u16),
    UpdateConfig(OscConfig),
    TriggerPress(TriggerButton),
    TriggerRelease(TriggerButton),
    SetStrobeRate(f32),
    FetchMaestroData,
    SendManualCue(String, Vec<rosc::OscType>),
    ForceBuildup,
    SetGlobalBrightness(f32),
    /// Operator UV accent level (0..1) — stamped onto CueSnapshot for the DMX
    /// GigBAR UV layer (par ch4/9 + pod ch22). MIDI-learnable.
    SetUvLevel(f32),
    /// Flip `ManualTriggers::fog_latched` so fog stays running hands-free.
    ToggleFogLatch,
    /// Flip `ManualTriggers::uv_latched` so UV mode stays on hands-free.
    ToggleUvLatch,
    /// COM-499 Phase B4: replace the operator LFO bank (running phase preserved).
    SetLfoBank(crate::emerge::LfoBankConfig),
    Shutdown,
}

// ============================================================================
// ENGINE THREAD HANDLE
// ============================================================================

/// Handle to the dedicated lighting-engine thread.
///
/// Owns the communication channels and shared state. The main thread
/// interacts exclusively through this struct — never touching the engine or
/// the OSC adapter directly.
pub struct OscThread {
    /// Send commands to the thread
    command_tx: Sender<OscCommand>,
    /// Latest audio input for the thread to consume (overwrite-latest pattern)
    input: Arc<PLMutex<Option<LightingInput>>>,
    /// Latest CueSnapshot produced by the thread (shared with Hue thread + main)
    snapshot: Arc<PLMutex<CueSnapshot>>,
    /// UI state produced by the thread
    ui_state: Arc<PLMutex<OscUiState>>,
    /// MaestroData (shared with background fetch thread inside OscManager)
    maestro_data: Arc<std::sync::Mutex<MaestroData>>,
    /// Main thread's copy of the config (for DB persistence)
    config: OscConfig,
    /// Main thread's copy of the enabled flag
    enabled: bool,
    /// Thread join handle
    thread: Option<std::thread::JoinHandle<()>>,
}

impl OscThread {
    /// Spawn the engine thread.
    ///
    /// OSC output starts disabled — send `OscCommand::SetEnabled(true)` to activate.
    pub fn new() -> Self {
        let (command_tx, command_rx) = crossbeam_channel::unbounded::<OscCommand>();
        let input = Arc::new(PLMutex::new(None::<LightingInput>));
        let snapshot = Arc::new(PLMutex::new(CueSnapshot::default()));
        let ui_state = Arc::new(PLMutex::new(OscUiState::default()));

        // Create the adapter on the main thread so we can extract shared state
        let manager = OscManager::new();
        let maestro_data = manager.maestro_data_arc();
        let engine = LightingEngine::new();

        // Clone Arcs for the thread closure
        let t_input = Arc::clone(&input);
        let t_snapshot = Arc::clone(&snapshot);
        let t_ui_state = Arc::clone(&ui_state);

        let thread = std::thread::Builder::new()
            .name("lighting-engine".to_string())
            .spawn(move || {
                Self::thread_loop(engine, manager, command_rx, t_input, t_snapshot, t_ui_state);
            })
            .expect("Failed to spawn lighting engine thread");

        Self {
            command_tx,
            input,
            snapshot,
            ui_state,
            maestro_data,
            config: OscConfig::default(),
            enabled: false,
            thread: Some(thread),
        }
    }

    // === Input ===

    /// Send latest audio input to the engine thread (overwrite-latest pattern).
    /// Called every frame from the main thread.
    pub fn send_input(&self, new_input: LightingInput) {
        *self.input.lock() = Some(new_input);
    }

    // === Commands ===

    /// Send a command to the engine thread.
    pub fn send_command(&self, cmd: OscCommand) {
        let _ = self.command_tx.send(cmd);
    }

    /// COM-499 Phase B4: push a new operator LFO bank to the runtime (the thread
    /// preserves the running phase across the swap). Called on startup with the
    /// persisted bank and whenever the editor changes a slot.
    pub fn set_lfo_bank(&self, bank: crate::emerge::LfoBankConfig) {
        let _ = self.command_tx.send(OscCommand::SetLfoBank(bank));
    }

    // === Shared state readers ===

    /// Read the latest CueSnapshot (for WLED + UI on main thread).
    pub fn snapshot(&self) -> CueSnapshot {
        self.snapshot.lock().clone()
    }

    /// Get the shared CueSnapshot Arc (passed to HueThread at creation).
    pub fn snapshot_arc(&self) -> Arc<PLMutex<CueSnapshot>> {
        Arc::clone(&self.snapshot)
    }

    /// Read the latest UI state.
    pub fn ui_state(&self) -> OscUiState {
        self.ui_state.lock().clone()
    }

    /// Read the latest MaestroData (cloned from shared Arc<Mutex>).
    pub fn maestro_data(&self) -> MaestroData {
        self.maestro_data.lock().unwrap().clone()
    }

    // === Config (main thread's copy for DB persistence) ===

    pub fn config(&self) -> &OscConfig {
        &self.config
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Update enabled state and notify thread.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        let _ = self.command_tx.send(OscCommand::SetEnabled(enabled));
    }

    /// Update target and notify thread. Also updates local config copy.
    pub fn set_target(&mut self, ip: &str, port: u16) {
        self.config.target_ip = ip.to_string();
        self.config.target_port = port;
        let _ = self.command_tx.send(OscCommand::SetTarget(ip.to_string(), port));
    }

    /// Replace config entirely and notify thread.
    pub fn set_config(&mut self, config: OscConfig) {
        self.config = config.clone();
        let _ = self.command_tx.send(OscCommand::UpdateConfig(config));
    }

    /// Update global brightness and notify thread. Keeps main-thread config in sync.
    pub fn set_global_brightness(&mut self, value: f32) {
        self.config.global_brightness = value.clamp(0.0, 1.0);
        let _ = self.command_tx.send(OscCommand::SetGlobalBrightness(value));
    }

    /// Update operator UV accent level and notify thread. Keeps main-thread config in sync.
    pub fn set_uv_level(&mut self, value: f32) {
        self.config.uv_level = value.clamp(0.0, 1.0);
        let _ = self.command_tx.send(OscCommand::SetUvLevel(value));
    }

    // === Lifecycle ===

    /// Shut down the engine thread and wait for it to exit.
    pub fn shutdown(&mut self) {
        let _ = self.command_tx.send(OscCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    // === Thread loop ===

    fn thread_loop(
        mut engine: LightingEngine,
        mut osc: OscManager,
        command_rx: Receiver<OscCommand>,
        input: Arc<PLMutex<Option<LightingInput>>>,
        snapshot: Arc<PLMutex<CueSnapshot>>,
        ui_state: Arc<PLMutex<OscUiState>>,
    ) {
        log::info!("[LightingEngine] Started (target tick: {:?})", TICK_INTERVAL);

        // === Cue-cadence diagnostics ===========================================
        // Confirms whether the CueEngine (and the energy smoothing every light
        // reads) actually runs near the ~60Hz its fixed per-call alpha assumes.
        // `cue` = fresh-input ticks/sec; `republish` = audio-independent ticks
        // (expected ~40/s when the 100Hz loop outpaces a 60fps feed; ~100/s when
        // the feed stalls — by design, the snapshot keeps publishing either way).
        // Always measured; the 1s summary prints in debug builds.
        // REACT_OSC_DIAG=1 forces on, =0 forces off.
        let diag_enabled = match std::env::var("REACT_OSC_DIAG").ok().as_deref() {
            Some("1") => true,
            Some("0") => false,
            _ => cfg!(debug_assertions),
        };
        if diag_enabled {
            log::info!("[OSC-DIAG] Cue-cadence diagnostics ON (1s summary). Healthy: loop ~100Hz, cue ~60Hz, tick_ms <1.");
        }
        let mut diag_window = Instant::now();
        let mut diag_ticks: u64 = 0;
        let mut diag_updates: u64 = 0;
        let mut diag_republish: u64 = 0;
        let mut diag_tick_sum = Duration::ZERO;
        let mut diag_tick_max = Duration::ZERO;
        let mut diag_raw_energy = 0.0f32;
        // Assigned every tick (the snapshot publishes unconditionally now).
        let mut diag_smooth_energy;

        loop {
            let tick_start = Instant::now();
            diag_ticks += 1;

            // 1. Drain all commands (non-blocking). The engine mutates its
            // state; the OSC adapter transports any immediate wire effects.
            loop {
                match command_rx.try_recv() {
                    Ok(cmd) => match cmd {
                        OscCommand::Shutdown => {
                            log::info!("[LightingEngine] Shutdown received");
                            return;
                        }
                        OscCommand::SetEnabled(enabled) => {
                            osc.set_enabled(enabled, engine.config());
                        }
                        OscCommand::SetTarget(ip, port) => {
                            engine.config_mut().target_ip = ip.clone();
                            engine.config_mut().target_port = port;
                            osc.set_target(&ip, port);
                        }
                        OscCommand::UpdateConfig(config) => {
                            // Preserve the old set_config semantics: bounce the
                            // sender across a config swap so a new target takes.
                            let was_enabled = osc.is_enabled();
                            if was_enabled {
                                osc.set_enabled(false, engine.config());
                            }
                            engine.set_config(config);
                            if was_enabled {
                                osc.set_enabled(true, engine.config());
                            }
                        }
                        OscCommand::TriggerPress(button) => {
                            engine.trigger_press(button);
                            // Send trigger messages immediately (don't wait for
                            // the next tick) — the engine republishes the held
                            // state to the snapshot on its own clock.
                            if osc.is_live() {
                                let use_maestro = matches!(
                                    engine.config().target_profile,
                                    TargetProfile::MaestroDMX
                                );
                                let messages =
                                    engine.manual_triggers_mut().generate_messages(use_maestro);
                                log::info!(
                                    "[OSC] Trigger {} pressed - sending {} messages immediately",
                                    button.as_str(),
                                    messages.len()
                                );
                                osc.send_bools(&messages);
                            }
                        }
                        OscCommand::TriggerRelease(button) => {
                            engine.trigger_release(button);
                            if osc.is_live() {
                                let use_maestro = matches!(
                                    engine.config().target_profile,
                                    TargetProfile::MaestroDMX
                                );
                                let messages = engine
                                    .manual_triggers()
                                    .generate_release_messages(button, use_maestro);
                                osc.send_bools(&messages);
                            }
                        }
                        OscCommand::SetStrobeRate(hz) => {
                            engine.set_strobe_rate(hz);
                        }
                        OscCommand::FetchMaestroData => {
                            osc.fetch_maestro_data(engine.config());
                        }
                        OscCommand::SendManualCue(address, args) => {
                            osc.send_manual_cue(&address, args);
                        }
                        OscCommand::ForceBuildup => {
                            let messages = engine.force_buildup();
                            osc.send_args(&messages);
                        }
                        OscCommand::SetGlobalBrightness(value) => {
                            engine.config_mut().global_brightness = value.clamp(0.0, 1.0);
                            osc.send_global_brightness(engine.config().global_brightness);
                        }
                        OscCommand::SetUvLevel(value) => {
                            // No OSC emission — UV is a DMX-only layer; the value
                            // reaches mapping.rs via the engine's snapshot stamp.
                            engine.config_mut().uv_level = value.clamp(0.0, 1.0);
                        }
                        OscCommand::ToggleFogLatch => {
                            // Emit `/triggers/fog` on latch TRANSITIONS so OSC
                            // consumers track the latch like DMX does (DMX reads
                            // fog_active() every frame; OSC fog is edge-driven).
                            // Compare fog_active() before/after the flip — if the
                            // button is also held, the effective state doesn't
                            // change and no message is needed (the release path
                            // in triggers.rs handles the held↔latched handoff).
                            let was_active = engine.manual_triggers().fog_active();
                            engine.manual_triggers_mut().toggle_fog_latch();
                            let now_active = engine.manual_triggers().fog_active();
                            if was_active != now_active {
                                osc.send_manual_cue(
                                    "/triggers/fog",
                                    vec![rosc::OscType::Bool(now_active)],
                                );
                            }
                        }
                        OscCommand::ToggleUvLatch => {
                            // Mirror ToggleFogLatch: emit `/triggers/uv` on
                            // EFFECTIVE transitions only — if the button is
                            // also held, flipping the latch doesn't change
                            // uv_active() and no message is needed (the
                            // release path in triggers.rs handles the
                            // held↔latched handoff).
                            let was_active = engine.manual_triggers().uv_active();
                            engine.manual_triggers_mut().toggle_uv_latch();
                            let now_active = engine.manual_triggers().uv_active();
                            if was_active != now_active {
                                osc.send_manual_cue(
                                    "/triggers/uv",
                                    vec![rosc::OscType::Bool(now_active)],
                                );
                            }
                        }
                        OscCommand::SetLfoBank(bank) => {
                            engine.set_lfo_bank(bank);
                        }
                    },
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        log::info!("[LightingEngine] Command channel disconnected, exiting");
                        return;
                    }
                }
            }

            // 2. Take lighting input from mutex (if new) — just one input to
            // the engine now, not the gate on publishing.
            let maybe_input = { input.lock().take() };
            let fresh = maybe_input.is_some();
            if let Some(ref li) = maybe_input {
                diag_raw_energy = li.analysis.energy_vis;
                osc.observe(li);
            }

            // 3. Engine tick: ALWAYS produces a snapshot + UI state — fresh
            // input advances the engine, a missing input republishes from the
            // cached input + CURRENT trigger/operator state (silencing the
            // bands once the feed has stalled past the staleness window).
            let use_maestro =
                matches!(engine.config().target_profile, TargetProfile::MaestroDMX);
            let tick_t0 = Instant::now();
            let tick = engine.tick(maybe_input, osc.is_live(), use_maestro);
            let tick_dur = tick_t0.elapsed();

            // 4. OSC output adapter transports the tick's messages.
            osc.set_lfo_modulation(tick.lfo_mod);
            osc.emit(&engine, &tick);

            // 5. Publish snapshot + UI state — every tick, audio or not.
            diag_smooth_energy = tick.snapshot.energy;
            *snapshot.lock() = tick.snapshot;
            *ui_state.lock() = tick.ui_state;

            // Diagnostics: record this cycle's timings.
            if fresh {
                diag_updates += 1;
            } else {
                diag_republish += 1;
            }
            diag_tick_sum += tick_dur;
            if tick_dur > diag_tick_max {
                diag_tick_max = tick_dur;
            }

            // 6. Sleep remaining time to hit target tick rate
            let elapsed = tick_start.elapsed();
            if elapsed < TICK_INTERVAL {
                std::thread::sleep(TICK_INTERVAL - elapsed);
            }

            // === Cue-cadence diagnostics: 1s rolling summary ===
            // loop = engine iterations/sec (~100 healthy). cue = fresh-input
            // ticks/sec — the rate the fixed-alpha energy smoothing assumes
            // is 60. If cue collapses well below 60, that IS the audio->light
            // lag: the smoothing time-constant stretches by the same ratio.
            // republish ticks keep trigger/operator state flowing regardless.
            if diag_enabled {
                let win = diag_window.elapsed();
                if win >= Duration::from_secs(1) {
                    let secs = win.as_secs_f32().max(0.001);
                    let tick_avg = if diag_ticks > 0 {
                        diag_tick_sum.as_secs_f32() * 1000.0 / diag_ticks as f32
                    } else { 0.0 };
                    log::info!(
                        "[OSC-DIAG] loop={:.1}Hz cue={:.1}Hz tick_ms(avg/max)={:.2}/{:.2} ticks={} updates={} republish={} | energy raw={:.3} smooth={:.3}",
                        diag_ticks as f32 / secs,
                        diag_updates as f32 / secs,
                        tick_avg, diag_tick_max.as_secs_f32() * 1000.0,
                        diag_ticks, diag_updates, diag_republish,
                        diag_raw_energy, diag_smooth_energy,
                    );
                    diag_window = Instant::now();
                    diag_ticks = 0;
                    diag_updates = 0;
                    diag_republish = 0;
                    diag_tick_sum = Duration::ZERO;
                    diag_tick_max = Duration::ZERO;
                }
            }
        }
    }
}

impl Drop for OscThread {
    fn drop(&mut self) {
        self.shutdown();
    }
}

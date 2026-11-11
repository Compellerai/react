//! DMX output adapter for REACT lighting intent.
//!
//! This module deliberately keeps protocol addressing separate from Hue, OSC,
//! and WLED. REACT produces lighting intent (CueSnapshot); the DMX adapter
//! translates that intent through a fixture patch into universe/channel bytes.
//! That keeps venue patch data explicit and makes blackout testable.

mod artnet;
mod mapping;
mod ola;
mod usb;
pub mod virtual_stage;

pub use artnet::{build_artdmx_packet, ArtNetSender};
pub use ola::OlaDmxSender;
pub use usb::UsbDmxSender;
// contracts moved to crate::emerge::contracts; shim the surface here so existing
// crate::dmx::X callers keep resolving until the de-shim slice.
use crate::emerge::contracts::{
    ChannelDefinition, ChannelMap, DmxFrame, DmxPatch, DmxPayload, DmxAttribute, FixtureCapability,
    CdjPerformanceState, DeckPlaybackState, FixtureHousing, FixtureKind, FixtureOverride, FixtureOverrides, FixturePatch, FixtureProfile, FixtureRole, OutputExpressionKind, ScreenConfig, ScreenFit, SegmentPattern, StageFixturePresetKind, StageViewPreset, StageViewPresetKind, StageVisualKind, ShowTheme, StageVec3, ThemeIntent, ThemeState, TrackCreativeNotes, Truss, TrussAxis, TrussKind, VenueKind,
    DMX_CHANNEL_COUNT, DMX_MAX_CHANNEL, DMX_MIN_CHANNEL,
};
// Intent engine moved to crate::emerge::intent; shim its public surface here so
// existing crate::dmx::X callers keep resolving until the de-shim slice.
use crate::emerge::intent::{
    ArcSignals, ArcState, CueLaneIntent, CuePriority, FixtureBrain, FixtureBrainContext,
    FixtureFamily, FixtureFrameContext, FixtureFamilyCapabilities, FixtureGenreProfile,
    PhaseState, ShowClockSnapshot, ShowClockSource, TopologyState,
};
use crate::emerge::intent::{render_fixture_intent, renders_as_galaxy_effect, FixtureIntentState};
pub use mapping::render_dmx_frame;
use crate::emerge::contracts::{segments_for_profile, SegmentChannels};
use crate::emerge::intent::{
    add_bipolar, bipolar_wave, OscillatorBinding, OscillatorFrameSnapshot,
    OscillatorInterval, OscillatorShape,
};
use crate::emerge::intent::{add_clamped, LfoBankConfig, LfoModulation, LfoTarget, LfoWaveform};
use crate::emerge::profiles::{load_ofl_fixture_profile, FixtureCommerceMetadata, FixtureProfileSource};
pub use virtual_stage::VirtualStageBackend;

use std::env;
use std::fmt;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use crate::hue::CueSnapshot;
use crate::render::LightingMetrics;
use crate::db::Database;
use crate::stage_sync::StageSummary;
use crate::stage_sync_worker::{StageSyncWorker, SyncOp, SyncStatus, SyncStatusKind, WorkerEvent};
// Engine modules moved to crate::emerge; alias them so this adapter's body keeps
// using the short contracts::/profiles::/lighting_cue::/lighting_cue_worker:: paths.
use crate::emerge::{contracts, profiles};
use crate::emerge::cue::{lighting_cue, lighting_cue_worker};
use crate::emerge::cue::show_brain::ShowBrain;
use crate::emerge::cue::{link_clock, prolink_clock, ClockInputs, ClockOverride};

/// Quiet window between the last fixture-patch change and the auto-flush to
/// SaaS. Long enough that a drag is recorded once at the end, short enough
/// that the operator sees the sync land before moving on.
const SYNC_DEBOUNCE: Duration = Duration::from_millis(1000);
/// Stage-list cache TTL — the dropdown re-pulls past this point.
const STAGE_LIST_TTL: Duration = Duration::from_secs(60);

/// Environment-driven manager for the first demo slice.
///
/// Disabled by default. To demo from a laptop:
///
/// Direct Art-Net smoke:
/// - REACT_DMX_ARTNET=1
/// - REACT_DMX_ARTNET_TARGET=192.168.1.50:6454
///
/// OLA smoke through Compeller's public ola-rs crate:
/// - REACT_DMX_BACKEND=ola
/// - REACT_DMX_OLA_HOST=127.0.0.1
/// - REACT_DMX_OLA_PORT=9010
///
/// USB (Open DMX / FT232 cable) — bit-bang DMX512 straight to the dongle:
/// - REACT_DMX_BACKEND=usb
/// - REACT_DMX_USB_PORT=/dev/cu.usbserial-XXXX   (optional; auto-detected if unset)
///
/// Shared optional patch controls:
/// - REACT_DMX_ARTNET_UNIVERSE=0 or REACT_DMX_UNIVERSE=0
/// - REACT_DMX_ARTNET_ADDR=1 or REACT_DMX_ADDR=1
pub struct DmxManager {
    enabled: bool,
    output: Option<DmxOutput>,
    /// The active DMX output backend — drives the DMX panel status + the runtime
    /// switch in [`Self::set_output_backend`].
    backend: DmxBackend,
    /// DMX start address (from `REACT_DMX_ADDR`); kept for logging + reconfigure.
    start_address: u16,
    virtual_stage: VirtualStageBackend,
    patch: DmxPatch,
    last_frame: Instant,
    min_interval: Duration,
    /// TRIG-DIAG probe: last emit time for the override-render log (1Hz while
    /// a manual trigger override is held). Measures what actually landed on
    /// the wire frame + preview glyph intents.
    trig_probe_at: Option<Instant>,
    /// Free-running wall clock (seconds) for the manual STROBE square wave —
    /// advanced by the real per-frame dt so the strobe flashes regardless of
    /// audio (the show's `frame_time` freezes when nothing is playing).
    strobe_clock: f32,
    /// Smoothed silence master (1.0 audio-active -> 0.0 confirmed silence), driven by
    /// the app SilenceGate. The gate reads RAW band energy (the truth); the smoothed
    /// `snapshot.energy` floors high in silence, so without this the rig never
    /// settles dark in a quiet break. Folded into global brightness at render time.
    silence_gain: f32,
    diag_frame: u64,
    show_brain: ShowBrain,
    clock_inputs: ClockInputs,
    /// Resolved ATLABASE hardware profile per fixture id, from the NAS profile
    /// library. Consumed by the operator virtual-stage panel for the per-
    /// fixture attribute table and for kind-based render overrides.
    fixture_profiles: std::collections::HashMap<String, contracts::FixtureProfile>,
    /// NAS hardware-profile library. Kept resident so the +Fixture palette
    /// can list every available profile, not just the ones already on the
    /// loaded stage.
    profile_library: profiles::FixtureProfileLibrary,
    /// Operator-selected stage (None = the "Select stage…" placeholder).
    selected_stage_id: Option<i64>,
    /// Whether the current login may EDIT the selected stage. `false` for a shared
    /// admin template picked by a non-admin → the auto-flush is suppressed and the
    /// in-stage edit affordances are disabled (the server 403s the edit regardless).
    /// `true` for personal stages and for admins; default `true` (no stage selected
    /// / older SaaS) so the pre-split edit-everything behavior is preserved.
    selected_stage_editable: bool,
    /// Logged-in user is a Compeller admin (mirrored from AuthManager by app.rs).
    /// Admins may edit shared stages, so this overlays `editable = true` on shared
    /// rows even when a cached per-stage flag is stale — the cause of an admin being
    /// unable to drag shared fixtures after a token went stale. Server re-enforces.
    is_admin: bool,
    /// Snapshot of the patch as last reconciled with SaaS — used to diff for
    /// per-fixture Create / Update / Delete ops.
    last_synced_patch: Option<DmxPatch>,
    /// Snapshot of the patch as observed last tick — used to reset the
    /// debounce window each time the operator nudges something.
    last_seen_patch: Option<DmxPatch>,
    /// First moment the patch diverged from `last_synced_patch`. After
    /// `SYNC_DEBOUNCE` of stability the auto-flush enqueues ops.
    dirty_since: Option<Instant>,
    /// Cached list of stages for the Stage: dropdown.
    stage_list: Vec<StageSummary>,
    stage_list_fetched_at: Option<Instant>,
    /// Background sync worker that owns the actual HTTP traffic.
    sync_worker: StageSyncWorker,
    /// Last rendered DMX frame, kept so the virtual-stage preview can read
    /// real per-segment color (pixel bars) directly from the output bytes.
    last_dmx_frame: Option<DmxFrame>,
    /// COM-499: background lighting-cue executor (polls /cues, acks, reports).
    cue_worker: lighting_cue_worker::LightingCueWorker,
    /// COM-499 rule 8 fade-out (renderer-owned): retained render-only clones of
    /// recently-stopped cues, each ramped to black over STOP_FADE_SECS then
    /// dropped. Keyed by launch_id; value is (forced-runnable clone, fade start).
    cue_fades: std::collections::HashMap<String, (lighting_cue::ActiveCue, Instant)>,
    /// COM-499 fade-on-disappearance: the previous frame's REAL runnable cues
    /// (launch_id -> cue), so a cue that vanishes from the slot (operator-local
    /// stop, or a SaaS stop never observed as STOP_REQUESTED) fades from its
    /// last-known bindings instead of hard-cutting to black (anti-jank).
    last_runnable: std::collections::HashMap<String, lighting_cue::ActiveCue>,
    /// Monotonic counter for minting operator-local cue launch ids.
    local_cue_seq: u64,
    /// COM-499 rig-wide autonomous look rotation: `look_index` advances on a
    /// polarity-quadrant change, a phrase boundary (~every 2 bars off the show
    /// clock beat), or a drop/energy edge, so the no-cue autopilot cycles colour +
    /// segment effect instead of sitting on one look. The `look_last_*`/armed
    /// fields are the edge-detector memory.
    look_index: u32,
    look_last_polarity: u8,
    look_last_phrase: i64,
    look_drop_armed: bool,
    /// Mover choreography: picks the figure for the musical arc + tonality,
    /// crossfades between figures, and tracks the harmonic steering (aim /
    /// chord throw / tension gather). Ticked once per frame beside the look
    /// rotation; its frame is stamped onto the snapshot so the wire and the
    /// preview move identically.
    movement: crate::emerge::MovementDirector,
    /// Integrated autopilot tightening-chase cycle and the show-clock beat it
    /// was last advanced at (`lighting_cue::advance_auto_chase`): the head
    /// must accelerate as tension shortens the period, and only an integrator
    /// is continuous under a varying period — `fract(abs_beats / period)`
    /// teleports. Stamped onto the snapshot beside `look_index`.
    auto_chase_cycle: f64,
    auto_chase_last_beat: f64,
    /// Local persistence handle (SQLite), attached after the DB opens (set_db).
    /// Enables offline stage caching — stages render with no internet; SaaS stays
    /// authority when online. `None` in headless/test paths (caching no-ops).
    db: Option<Database>,
    /// Whether startup auto-restore of the last selected stage has run (once), so
    /// picking "Select stage…" stays sticky instead of snapping back.
    stage_restore_attempted: bool,
    /// The venue currently being rendered. Concert renders `self.patch` (the
    /// SaaS/selected stage); Wedding renders `wedding_patch`. Switched via
    /// [`Self::set_active_venue`].
    active_venue: VenueKind,
    /// The Wedding venue's REACT-local scene (a screen + uplights). Kept SEPARATE
    /// from `self.patch` so the SaaS sync — which `clear()`s and reloads
    /// `self.patch` on every stage select / pull — can never corrupt it. The
    /// render + operator edits address whichever patch the active venue selects;
    /// `self.patch` is never swapped, so concert sync stays correct unconditionally.
    wedding_patch: DmxPatch,
}

/// Resolve each fixture's `profile_id` against the in-memory NAS library.
/// Fixtures whose profile is missing keep their built-in defaults.
fn resolve_stage_profiles(
    library: &profiles::FixtureProfileLibrary,
    patch: &DmxPatch,
    from_saas: bool,
) -> std::collections::HashMap<String, contracts::FixtureProfile> {
    let mut resolved = std::collections::HashMap::new();
    if library.is_empty() {
        return resolved;
    }
    for fixture in &patch.fixtures {
        if fixture.profile_id.is_empty() {
            continue;
        }
        match library.resolve(&fixture.profile_id, &fixture.mode) {
            Some(profile) => {
                log::info!(
                    "[DMX] fixture '{}' -> profile {} [{:?}] {} ch, mode '{}'",
                    fixture.name, fixture.profile_id, profile.kind, profile.channel_count, profile.mode,
                );
                resolved.insert(fixture.id.clone(), profile);
            }
            None if from_saas => log::warn!(
                "[DMX] fixture '{}' profile_id '{}' not found in the NAS hardware library",
                fixture.name, fixture.profile_id,
            ),
            None => {}
        }
    }
    resolved
}

impl DmxManager {
    pub fn from_env() -> Self {
        let backend = DmxBackend::from_env();
        let universe = env_u16("REACT_DMX_UNIVERSE")
            .or_else(|| env_u16("REACT_DMX_ARTNET_UNIVERSE"))
            .unwrap_or(0);
        let start_address = env_u16("REACT_DMX_ADDR")
            .or_else(|| env_u16("REACT_DMX_ARTNET_ADDR"))
            .unwrap_or(1);

        let virtual_enabled = env_flag("REACT_VIRTUAL_STAGE");
        // First-load state: no stage selected, no fixtures. The operator
        // picks one from the Stage: dropdown, which pulls the SaaS patch.
        let patch = DmxPatch { universe, fixtures: vec![], trusses: vec![], venue: None };
        let output = match backend {
            DmxBackend::Disabled => None,
            DmxBackend::ArtNet => Self::build_artnet_output(universe, start_address, None),
            DmxBackend::Ola => Self::build_ola_output(universe, start_address, None),
            DmxBackend::Usb => Self::build_usb_output(universe, start_address, None),
        };
        let virtual_stage = VirtualStageBackend::new(virtual_enabled, patch.clone());

        let profile_library = profiles::FixtureProfileLibrary::scan_default();
        log::info!(
            "[DMX] hardware-profile library: {} profile(s) available",
            profile_library.len()
        );
        let fixture_profiles = resolve_stage_profiles(&profile_library, &patch, false);

        let enabled = output.is_some() || virtual_stage.is_enabled();
        Self {
            enabled,
            output,
            backend,
            start_address,
            virtual_stage,
            patch,
            last_frame: Instant::now() - Duration::from_secs(1),
            strobe_clock: 0.0,
            min_interval: Duration::from_millis(33),
            trig_probe_at: None,
            silence_gain: 1.0,
            diag_frame: 0,
            show_brain: ShowBrain::new(),
            clock_inputs: ClockInputs::default(),
            fixture_profiles,
            profile_library,
            selected_stage_id: None,
            selected_stage_editable: true,
            is_admin: false,
            last_synced_patch: None,
            last_seen_patch: None,
            dirty_since: None,
            stage_list: Vec::new(),
            stage_list_fetched_at: None,
            sync_worker: StageSyncWorker::new(),
            last_dmx_frame: None,
            cue_worker: lighting_cue_worker::LightingCueWorker::new(),
            cue_fades: std::collections::HashMap::new(),
            last_runnable: std::collections::HashMap::new(),
            local_cue_seq: 0,
            look_index: 0,
            movement: crate::emerge::MovementDirector::new(),
            auto_chase_cycle: 0.0,
            auto_chase_last_beat: 0.0,
            look_last_polarity: 0,
            look_last_phrase: 0,
            look_drop_armed: true,
            db: None,
            stage_restore_attempted: false,
            active_venue: VenueKind::ConcertStage,
            wedding_patch: DmxPatch { universe: 0, fixtures: Vec::new(), trusses: Vec::new(), venue: None },
        }
    }


    fn build_artnet_output(
        universe: u16,
        start_address: u16,
        explicit: Option<String>,
    ) -> Option<DmxOutput> {
        // Explicit target (DMX panel) wins, then REACT_DMX_ARTNET_TARGET.
        let raw = explicit
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| env::var("REACT_DMX_ARTNET_TARGET").ok());
        let target = raw.as_deref().and_then(parse_artnet_target);
        match target {
            Some(addr) => match ArtNetSender::new(addr) {
                Ok(sender) => {
                    log::info!("[DMX] Art-Net output enabled -> {} universe={} addr={}", addr, universe, start_address);
                    Some(DmxOutput::ArtNet(sender))
                }
                Err(err) => {
                    log::warn!("[DMX] Art-Net output disabled: failed to bind UDP socket: {}", err);
                    None
                }
            },
            None => {
                log::warn!("[DMX] Art-Net backend selected but no valid target. Set the DMX panel Target field or REACT_DMX_ARTNET_TARGET (e.g. 192.168.1.50:6454)");
                None
            }
        }
    }

    fn build_ola_output(
        universe: u16,
        start_address: u16,
        explicit: Option<String>,
    ) -> Option<DmxOutput> {
        // Explicit host:port (DMX panel) wins, then REACT_DMX_OLA_HOST/PORT, then defaults.
        let (host, port) = match explicit
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            Some(ep) => parse_ola_endpoint(&ep),
            None => {
                let host = env::var("REACT_DMX_OLA_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
                let port = env_u16("REACT_DMX_OLA_PORT").unwrap_or(9010);
                (host, port)
            }
        };
        match OlaDmxSender::connect(host.clone(), port) {
            Ok(sender) => {
                log::info!("[DMX] OLA output enabled via ola-rs -> {}:{} universe={} addr={}", host, port, universe, start_address);
                Some(DmxOutput::Ola(sender))
            }
            Err(err) => {
                log::warn!("[DMX] OLA output disabled: could not connect to olad at {}:{}: {}", host, port, err);
                None
            }
        }
    }

    fn build_usb_output(
        universe: u16,
        start_address: u16,
        explicit_port: Option<String>,
    ) -> Option<DmxOutput> {
        // Explicit port (DMX panel) wins, then REACT_DMX_USB_PORT, then auto-detect.
        let port = explicit_port
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .or_else(|| {
                env::var("REACT_DMX_USB_PORT")
                    .ok()
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
            })
            .or_else(UsbDmxSender::autodetect_port);
        match port {
            Some(port) => match UsbDmxSender::open(port.clone()) {
                Ok(sender) => {
                    log::info!(
                        "[DMX] USB (Open DMX) output enabled -> {} universe={} addr={}",
                        port, universe, start_address
                    );
                    Some(DmxOutput::Usb(sender))
                }
                Err(err) => {
                    log::warn!("[DMX] USB output disabled: could not open {}: {}", port, err);
                    None
                }
            },
            None => {
                log::warn!("[DMX] USB backend selected but no FTDI serial port found. Set REACT_DMX_USB_PORT=/dev/cu.usbserial-XXXX");
                None
            }
        }
    }

    /// Snapshot of the live output for the operator DMX panel.
    pub fn output_status(&self) -> DmxOutputStatus {
        let connected = match &self.output {
            Some(DmxOutput::Usb(sender)) => sender.connected(),
            Some(_) => true,
            None => false,
        };
        let detail = match &self.output {
            Some(DmxOutput::Usb(sender)) => sender.port_name().to_string(),
            Some(DmxOutput::ArtNet(sender)) => sender.target().to_string(),
            Some(DmxOutput::Ola(sender)) => sender.endpoint(),
            None => String::new(),
        };
        DmxOutputStatus { backend: self.backend, connected, detail }
    }

    /// Switch the live DMX output backend at runtime (from the DMX panel) and
    /// persist the choice. Drops the current output first (`open_dmx`'s `DMXSerial`
    /// stops its thread on drop, releasing a USB cable before any re-open), then
    /// rebuilds. `endpoint` is the backend-specific connection string: USB → serial
    /// port path, Art-Net → `ip:port`, OLA → `host:port`. `None`/empty falls back
    /// to env/auto-detect.
    pub fn set_output_backend(&mut self, backend: DmxBackend, endpoint: Option<String>) {
        // Re-selecting the SAME, still-live USB cable is a no-op: open_dmx releases
        // the serial port ASYNCHRONOUSLY (its agent thread frees it up to ~23 ms after
        // the handle drops — there is no synchronous Drop), so dropping + immediately
        // re-opening the same port would race to EBUSY. This makes the startup restore
        // (from_env already opened the cable) and a redundant Connect harmless, while a
        // real change (different port/backend) or a dead handle still rebuilds — so an
        // unplug -> replug still reconnects normally.
        if backend == DmxBackend::Usb {
            if let Some(DmxOutput::Usb(sender)) = &self.output {
                let want = endpoint.as_deref().map(|p| p.trim()).filter(|p| !p.is_empty());
                if want.map_or(true, |p| p == sender.port_name()) && sender.connected() {
                    return;
                }
            }
        }
        self.output = None;
        let universe = self.patch.universe;
        self.output = match backend {
            DmxBackend::Disabled => None,
            DmxBackend::ArtNet => Self::build_artnet_output(universe, self.start_address, endpoint.clone()),
            DmxBackend::Ola => Self::build_ola_output(universe, self.start_address, endpoint.clone()),
            DmxBackend::Usb => Self::build_usb_output(universe, self.start_address, endpoint.clone()),
        };
        self.backend = backend;
        self.enabled = self.output.is_some() || self.virtual_stage.is_enabled();
        if let Some(db) = &self.db {
            let _ = db.set_dmx_backend(backend.as_key());
            // Persist the endpoint under the backend-specific key so each backend
            // remembers its own last target across sessions.
            if let Some(ep) = endpoint.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
                let _ = match backend {
                    DmxBackend::Usb => db.set_dmx_usb_port(ep),
                    DmxBackend::ArtNet => db.set_dmx_artnet_target(ep),
                    DmxBackend::Ola => db.set_dmx_ola_endpoint(ep),
                    DmxBackend::Disabled => Ok(()),
                };
            }
        }
        log::info!(
            "[DMX] backend set to {} (output {})",
            backend.as_key(),
            if self.output.is_some() { "up" } else { "down" }
        );
    }

    /// Apply a DMX backend persisted from a previous session (DMX panel), overriding
    /// the env-derived default. Called once after the DB attaches. No-op if nothing
    /// was persisted, so the existing env-var smoke path is preserved.
    pub fn restore_output_backend(&mut self) {
        // Read the persisted backend + its endpoint, ending the immutable `self.db`
        // borrow before the &mut self call to set_output_backend.
        let (backend, endpoint) = match &self.db {
            Some(db) => {
                let Some(key) = db.get_dmx_backend() else { return };
                let backend = DmxBackend::from_key(&key);
                let endpoint = match backend {
                    DmxBackend::Usb => db.get_dmx_usb_port(),
                    DmxBackend::ArtNet => db.get_dmx_artnet_target(),
                    DmxBackend::Ola => db.get_dmx_ola_endpoint(),
                    DmxBackend::Disabled => None,
                };
                (backend, endpoint)
            }
            None => return,
        };
        self.set_output_backend(backend, endpoint);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Set the external clock candidates (Pro DJ Link / Ableton Link) plus the
    /// operator override. Fed each frame from the lighting loop.
    pub fn set_clock_inputs(&mut self, inputs: ClockInputs) {
        self.clock_inputs = inputs;
    }

    /// Set the genre tempo-range prior (min,max BPM) for the autonomous tempo
    /// estimator, or `None` to disable. Only guides REACT's derived clock — an
    /// external Link/Pro DJ clock still overrides it in the clock selector.
    pub fn set_tempo_hint(&mut self, range: Option<(f32, f32)>) {
        self.show_brain.set_tempo_hint(range);
    }

    /// Seed the derived-clock tempo from an externally KNOWN BPM (SaaS track
    /// metadata / a CDJ handoff) so beat-locked output is right from beat one.
    pub fn seed_tempo(&mut self, bpm: f32) {
        self.show_brain.seed_tempo(bpm);
    }

    /// Push the operator's genre selection into the show-brain lane gates
    /// (`GenreProfile::fixture_profile`), so the genre dropdown shapes the
    /// physical rig — strobe/laser lanes, gobo stillness, mover depth — not
    /// just the OSC/CueEngine path.
    pub fn set_genre_fixture_profile(&mut self, genre: crate::emerge::FixtureGenreProfile) {
        self.show_brain.set_genre(genre);
    }

    /// Push the operator style posture into the show brain (license clamp +
    /// genre-gate composition). Pushed per frame from the config alongside
    /// the genre profile, so UI and MIDI changes both land within a frame.
    pub fn set_lighting_style(&mut self, style: crate::emerge::LightingStyle) {
        self.show_brain.set_style(style);
    }

    /// The posture actually driving the engine: the operator's pin, or the
    /// Auto resolver's current flip (Chill/Energetic/Banger). Consumed by
    /// the cue-engine gates, the snapshot tension cap, and the UI dropdown's
    /// live display.
    pub fn effective_lighting_style(&self) -> crate::emerge::LightingStyle {
        self.show_brain.effective_style()
    }

    /// COM-499 Phase B2: resolve the authoritative show clock this frame ABOVE the
    /// render throttle/enable guards in `update`, so it advances at full frame
    /// rate even when DMX rendering is disabled — the app stamps the returned
    /// clock onto `CueSnapshot` for ALL providers (Hue/WLED/OSC) + the operator
    /// LFO bank. The throttled `update`/`tick` then consumes the same resolution
    /// (resolve once per `frame_time`). Call once per frame after
    /// `set_clock_inputs`, before building the CueSnapshot fan-out.
    pub fn resolve_clock(
        &mut self,
        snapshot: &CueSnapshot,
        metrics: Option<&LightingMetrics>,
    ) -> ShowClockSnapshot {
        let metrics = metrics.copied().unwrap_or_default();
        self.show_brain
            .resolve_clock(snapshot, &metrics, &self.clock_inputs)
    }

    /// REACT's audio-derived tempo — `(bpm, confidence)` — for the clock UI.
    pub fn derived_tempo(&self) -> (f32, f32) {
        self.show_brain.tempo_estimate()
    }

    /// The show brain's current (debounced) musical arc — the single source of
    /// truth the CueEngine buildup macro keys on. Valid even when DMX output
    /// is disabled: the brain ticks regardless (see `update`), only rendering
    /// and wire emission are gated.
    pub fn current_arc(&self) -> crate::emerge::ArcState {
        self.show_brain.current_arc()
    }

    /// The show brain's intensity license (genre-free percussiveness ×
    /// spectral fullness) — same always-on availability as `current_arc`.
    pub fn intensity_license(&self) -> f32 {
        self.show_brain.intensity_license()
    }

    /// Show-brain dynamics 0..1: how big the passage is on the set's own
    /// scale. The app stamps it onto the next engine frame's snapshot.
    pub fn dynamics(&self) -> f32 {
        self.show_brain.dynamics()
    }

    pub fn virtual_stage_enabled(&self) -> bool {
        self.virtual_stage.is_enabled()
    }

    /// Runtime switch for the virtual stage preview backend, driven by the
    /// Lighting-menu toggle. When disabled, no per-frame fixture-state
    /// evaluation runs. DMX output (Art-Net / OLA) is independent of this — it
    /// follows the `output` configuration, not the preview toggle.
    ///
    /// Returns `true` when this call transitions the stage from off to on, so
    /// the caller can react to the stage loading (e.g. open the menu).
    pub fn set_virtual_stage_enabled(&mut self, enabled: bool) -> bool {
        let was_enabled = self.virtual_stage.is_enabled();
        if was_enabled != enabled {
            self.virtual_stage.set_enabled(enabled);
        }
        self.enabled = self.output.is_some() || self.virtual_stage.is_enabled();
        enabled && !was_enabled
    }

    pub fn fixture_intents(&self) -> &[FixtureIntentState] {
        self.virtual_stage.latest_intents()
    }

    /// Fixtures to DRAW in the preview — 1:1 with [`Self::fixture_intents`]. A
    /// GigBAR expands into one glyph per section here (the editable patch stays
    /// one fixture).
    pub fn glyph_fixtures(&self) -> &[crate::emerge::contracts::FixturePatch] {
        self.virtual_stage.glyph_fixtures()
    }

    /// Editable-fixture index each preview glyph belongs to (click/selection).
    pub fn glyph_parents(&self) -> &[usize] {
        self.virtual_stage.glyph_parents()
    }

    pub fn fixture_patches(&self) -> &[FixturePatch] {
        &self.active_patch().fixtures
    }

    /// Resolved ATLABASE hardware profile per fixture id. Operator virtual-
    /// stage panel reads this for the per-fixture attribute table and for
    /// kind-based render overrides (e.g. force a Scorpion to render as a
    /// laser when its profile.kind says so).
    pub fn fixture_profiles(&self) -> &std::collections::HashMap<String, contracts::FixtureProfile> {
        &self.fixture_profiles
    }

    /// Mutable fixture patch — the operator virtual-stage view edits each
    /// fixture's stage-space `position` and `rotation` (mounting aim) through
    /// this. Those fields are renderer-only: `render_dmx_frame` maps intent to
    /// channel bytes and `render_fixture_intent` keys off `role`/`visual_kind`,
    /// so neither reads position/rotation — edits stay preview-local with no
    /// DMX-output or intent side effects.
    pub fn fixture_patch_mut(&mut self) -> &mut DmxPatch {
        self.active_patch_mut()
    }

    /// NAS hardware-profile library — used by the +Fixture palette to list
    /// every profile available, not just the ones already on the stage.
    pub fn profile_library(&self) -> &profiles::FixtureProfileLibrary {
        &self.profile_library
    }

    /// Cached SaaS stages for the Stage: dropdown.
    pub fn available_stages(&self) -> &[StageSummary] {
        &self.stage_list
    }

    pub fn selected_stage_id(&self) -> Option<i64> {
        self.selected_stage_id
    }

    /// Whether the patch the operator is currently editing may be persisted. The
    /// REACT-local Wedding demo (`wedding_patch`) is never synced, so it is always
    /// freely editable; the SaaS stage in `self.patch` honors the per-stage edit
    /// gate. Drives the in-stage edit affordances (drag / aim / +Fixture).
    pub fn active_patch_editable(&self) -> bool {
        if self.active_venue == VenueKind::WeddingRoom
            && self.patch.venue != Some(VenueKind::WeddingRoom)
        {
            true // the local demo room — local-only, nothing to flush
        } else {
            self.selected_stage_editable
        }
    }

    pub fn sync_status(&self) -> SyncStatus {
        self.sync_worker.status()
    }

    /// Attach the local persistence handle once the DB has opened — enables
    /// offline stage caching. Forwards to the sync worker so it can persist/restore
    /// its fixture-key -> SaaS-id map (offline-safe edits). Call once at startup.
    pub fn set_db(&mut self, db: Database) {
        self.sync_worker.set_db(db.clone());
        // COM-499: the cue worker mirrors fetched cues into this store, hydrates
        // from it on (re)start so a running show drives offline, and promotes
        // operator-local cues up to SaaS when connectivity returns.
        self.cue_worker.set_db(db.clone());
        self.db = Some(db);
    }

    /// COM-499 operator-launch-local: fire a cached cue with ZERO SaaS. Clones the
    /// stored definition with a fresh local launch_id + ACTIVE status and hands it
    /// to the cue worker, which persists it (origin='local') and drives it this
    /// frame. No-op without a selected stage, DB, or known definition.
    pub fn launch_local_cue(&mut self, def_key: &str) {
        let stage_id = match self.selected_stage_id {
            Some(sid) => sid,
            None => return,
        };
        let cue_json = match self.db.as_ref().and_then(|db| db.cue_definition_json(def_key)) {
            Some(j) => j,
            None => return,
        };
        let mut value: serde_json::Value = match serde_json::from_str(&cue_json) {
            Ok(v) => v,
            Err(_) => return,
        };
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        self.local_cue_seq = self.local_cue_seq.wrapping_add(1);
        let launch_id = format!("local-{}-{}", millis, self.local_cue_seq);
        value["launch_id"] = serde_json::Value::String(launch_id.clone());
        value["status"] = serde_json::Value::String("ACTIVE".to_string());
        value["stage_id"] = serde_json::json!(stage_id);
        if let Ok(json) = serde_json::to_string(&value) {
            log::info!("[DMX] operator launched local cue {} (def {})", launch_id, def_key);
            self.cue_worker.launch_local(launch_id, json);
        }
    }

    /// Operator stops an operator-local cue; the renderer fades it out from its
    /// last-known bindings (anti-jank).
    pub fn stop_local_cue(&self, launch_id: &str) {
        log::info!("[DMX] operator stopped local cue {}", launch_id);
        self.cue_worker.stop_local(launch_id.to_string());
    }

    /// Cached cue definitions (`(def_key, label)`) for the selected stage — the
    /// operator's local-launch picker. Empty without a stage / DB.
    pub fn cached_cue_defs(&self) -> Vec<(String, String)> {
        match (self.db.as_ref(), self.selected_stage_id) {
            (Some(db), Some(sid)) => db.cue_definitions_for_stage(sid),
            _ => Vec::new(),
        }
    }

    /// Currently-active operator-local cues (`(launch_id, label)`) for the selected
    /// stage so the operator can stop them; label derived from profile/effect id.
    pub fn active_local_cues(&self) -> Vec<(String, String)> {
        let (db, sid) = match (self.db.as_ref(), self.selected_stage_id) {
            (Some(db), Some(sid)) => (db, sid),
            _ => return Vec::new(),
        };
        db.active_cue_jsons_for_stage(sid, Some("local"))
            .iter()
            .filter_map(|json| serde_json::from_str::<serde_json::Value>(json).ok())
            .filter_map(|v| {
                let launch_id = v.get("launch_id")?.as_str()?.to_string();
                let label = v
                    .get("metadata")
                    .and_then(|m| m.get("effect_name"))
                    .and_then(|s| s.as_str())
                    .or_else(|| v.get("cue_profile_id").and_then(|s| s.as_str()))
                    .or_else(|| v.get("effect_id").and_then(|s| s.as_str()))
                    .unwrap_or("cue")
                    .to_string();
                Some((launch_id, label))
            })
            .collect()
    }

    /// On startup, auto-open the last stage the operator used (persisted across
    /// runs) so REACT comes up showing the rig — rendered from the local cache,
    /// so it works with no internet. Fires at most once, and never if a stage is
    /// already selected, so picking "Select stage…" stays sticky. Safe to call
    /// every frame; no-ops until the DB is attached, then exactly once.
    /// NOTE: retained but NOT called — startup now forces explicit stage selection
    /// (the call in app.rs was removed). Re-add that call to restore auto-open.
    #[allow(dead_code)]
    pub fn restore_last_stage(&mut self, token: &str) {
        if self.stage_restore_attempted || self.selected_stage_id.is_some() {
            return;
        }
        let last = match self.db.as_ref() {
            Some(db) => db.last_stage_id(),
            None => return, // DB not attached yet — retry next frame.
        };
        self.stage_restore_attempted = true;
        if let Some(id) = last {
            // Best-effort edit permission from the cached list (default editable);
            // refresh_stage_list corrects it once the live list lands.
            let editable = self
                .stage_list
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.editable)
                .unwrap_or(true);
            self.select_stage(id, editable, token);
        }
    }

    /// Last rendered DMX frame — the virtual-stage preview reads pixel-bar
    /// per-segment color from it so the bar on screen matches the wire.
    pub fn last_dmx_frame(&self) -> Option<&DmxFrame> {
        self.last_dmx_frame.as_ref()
    }

    /// Refresh the stage-list cache via a one-shot HTTP call. Cheap enough to
    /// call when the operator opens the Stage: dropdown — no spinner needed.
    /// Respects an in-memory TTL so opening the dropdown repeatedly within a
    /// minute does not re-hit the API.
    pub fn refresh_stage_list(&mut self, token: &str) {
        if let Some(when) = self.stage_list_fetched_at {
            if when.elapsed() < STAGE_LIST_TTL && !self.stage_list.is_empty() {
                return;
            }
        }
        let config = crate::stage_sync::StageSyncConfig {
            base_url: crate::stage_sync::api_base(),
            token: token.to_string(),
        };
        match crate::stage_sync::list_stages(&config) {
            Ok(stages) => {
                self.stage_list = stages;
                self.stage_list_fetched_at = Some(Instant::now());
                // An admin may edit shared stages — overlay that before deriving the
                // selected stage's gate, so a freshly-pulled list is admin-correct.
                self.apply_admin_editable();
                // Keep the selected stage's edit permission fresh — an admin status
                // or sharing change between sessions flips read-only without a
                // re-select.
                self.refresh_selected_editable();
                // Cache for the offline Stage: dropdown.
                if let Some(db) = &self.db {
                    if let Ok(json) = serde_json::to_string(&self.stage_list) {
                        let _ = db.cache_stage_list(&json);
                    }
                }
            }
            Err(err) => {
                log::warn!("[DMX] stage list refresh failed: {}", err);
                // Offline / unreachable: fall back to the cached list so the
                // dropdown still shows the operator's stages.
                if self.stage_list.is_empty() {
                    if let Some(json) = self.db.as_ref().and_then(|db| db.cached_stage_list()) {
                        if let Ok(stages) = serde_json::from_str::<Vec<StageSummary>>(&json) {
                            self.stage_list = stages;
                            self.apply_admin_editable();
                            // Re-derive read-only from the cached list too — else the
                            // selected stage keeps its select-time editable flag while
                            // the (stale) cached list is what the dropdown shows.
                            self.refresh_selected_editable();
                        }
                    }
                }
            }
        }
    }

    /// Mirror the logged-in user's admin status (set by app.rs from AuthManager).
    /// Admins may edit shared stages, so re-mark shared rows editable and refresh
    /// the selected stage's gate — an admin is then never locked out by a stale
    /// per-stage `editable` flag. No-op when unchanged.
    pub fn set_admin(&mut self, is_admin: bool) {
        if self.is_admin == is_admin {
            return;
        }
        self.is_admin = is_admin;
        self.apply_admin_editable();
        self.refresh_selected_editable();
    }

    /// For an admin, overlay `editable = true` on every shared stage in the cached
    /// list so all editable lookups (select / restore / refresh) agree. OR-only —
    /// it never removes edit permission, and the server still enforces writes.
    fn apply_admin_editable(&mut self) {
        if !self.is_admin {
            return;
        }
        for s in &mut self.stage_list {
            if s.shared {
                s.editable = true;
            }
        }
    }

    /// Re-derive `selected_stage_editable` from the current `stage_list` for the
    /// selected stage. No-op (keeps the select-time value) when the selected stage
    /// isn't in the list.
    fn refresh_selected_editable(&mut self) {
        if let Some(sel) = self.selected_stage_id {
            if let Some(s) = self.stage_list.iter().find(|s| s.id == sel) {
                self.selected_stage_editable = s.editable;
            }
        }
    }

    /// Operator picked a stage from the dropdown. Clears local fixtures,
    /// tells the sync worker to pull, and waits for the StagePulled event
    /// (handled in `poll`).
    pub fn select_stage(&mut self, stage_id: i64, editable: bool, token: &str) {
        if self.selected_stage_id == Some(stage_id) {
            // Re-selecting the same stage triggers a fresh pull — useful if
            // the operator changed the rig on /adminStage and wants REACT to
            // catch up without restarting.
        }
        self.selected_stage_id = Some(stage_id);
        // Shared admin templates picked by a non-admin are read-only: suppress the
        // auto-flush + disable the in-stage edit affordances (the server 403s too).
        self.selected_stage_editable = editable;
        self.patch.fixtures.clear();
        self.patch.trusses.clear();
        // Drop the previous stage's authored venue too — a cleared patch claims
        // no venue, so `stage_venue()` stays accurate until the pull/cache lands.
        self.patch.venue = None;
        self.last_synced_patch = None;
        self.last_seen_patch = None;
        self.dirty_since = None;
        self.rebuild_backend();
        self.fixture_profiles.clear();
        self.sync_worker
            .set_target(crate::stage_sync::api_base(), token.to_string(), stage_id);
        self.cue_worker
            .set_target(crate::stage_sync::api_base(), token.to_string(), stage_id);
        // Remember the choice (startup restore) and render the cached patch
        // immediately — the stage appears instantly and works fully offline; the
        // worker's pull overwrites it with fresh SaaS data when the network responds.
        if let Some(db) = &self.db {
            let _ = db.set_last_stage_id(stage_id);
        }
        self.apply_cached_patch(stage_id);
    }

    /// Operator picked "Select stage…" — clear local state and stop syncing.
    pub fn clear_selected_stage(&mut self) {
        self.selected_stage_id = None;
        self.selected_stage_editable = true; // nothing selected → no read-only gate
        self.patch.fixtures.clear();
        self.patch.trusses.clear();
        self.patch.venue = None;
        self.last_synced_patch = None;
        self.last_seen_patch = None;
        self.dirty_since = None;
        self.rebuild_backend();
        self.fixture_profiles.clear();
        self.sync_worker.clear_target();
        self.cue_worker.clear_target();
    }

    /// Clone a shared admin template into the account's own editable "My <venue>"
    /// stage, then select the new copy so the operator immediately edits their own
    /// stage. Drives the dropdown's per-shared-row "Clone to My Stages" button.
    pub fn clone_stage(&mut self, source_id: i64, token: &str) {
        let config = crate::stage_sync::StageSyncConfig {
            base_url: crate::stage_sync::api_base(),
            token: token.to_string(),
        };
        match crate::stage_sync::clone_stage(&config, source_id) {
            Ok(new_id) => {
                log::info!(
                    "[DMX] cloned shared stage {} -> personal stage {}",
                    source_id,
                    new_id
                );
                // Force the dropdown to re-pull so the new "My …" row appears, then
                // select it — the clone is the account's own stage, so editable.
                self.stage_list_fetched_at = None;
                self.select_stage(new_id, true, token);
            }
            Err(err) => log::warn!("[DMX] clone shared stage {} failed: {}", source_id, err),
        }
    }

    /// Apply a fetched-or-cached fixture patch as the live patch: resolve NAS
    /// profiles, back-fill each ChannelMap from its profile (the stage_sync DTO
    /// path defaults to `ChannelMap::rgbw_dimmer` when SaaS authors no `channels`
    /// blob, which is only right for plain pars — any fixture with
    /// Pan/Tilt/Strobe/per-cell channels would otherwise look channel-less), rebuild
    /// the preview, and reset the sync baseline so the freshly-applied patch is not
    /// mistaken for a dirty local edit.
    /// COM-499: a "Strobe" fixture that actually emits colour (RGB main channels
    /// like the 200W RGB Strobe in 3/9-ch, OR per-cell segments like its 24-ch
    /// mode) is a colour wash that *also* strobes, not a white-only strobe. Left as
    /// Strobe it renders sat=0 white AND sits in the gated Strobe family — and that
    /// gate zeroes `intent.dimmer`, which the segmented path then multiplies into
    /// every cell — so the harmonic-colour autopilot can never colour OR even light
    /// it un-cued. Re-type those to a coloured LedPanel; a true white-only strobe
    /// (`has_color` false) stays Strobe. Durable fix belongs in the SaaS fixture
    /// def (flagged) — this is the REACT-side normalization so it reads right today.
    fn normalize_rgb_strobe(visual_kind: StageVisualKind, has_color: bool) -> StageVisualKind {
        if visual_kind == StageVisualKind::Strobe && has_color {
            StageVisualKind::LedPanel
        } else {
            visual_kind
        }
    }

    fn apply_patch(&mut self, mut patch: DmxPatch) {
        self.fixture_profiles = resolve_stage_profiles(&self.profile_library, &patch, true);
        for fixture in &mut patch.fixtures {
            if let Some(profile) = self.fixture_profiles.get(&fixture.id) {
                fixture.channels = ChannelMap::from_profile(fixture.start_address, profile);
                // Self-heal the address-allocator footprint for SaaS-pulled /
                // cached stages (their payloads don't carry it): the profile's
                // channel count includes trailing Custom channels that the
                // ChannelMap drops, and under-counting them overlapped
                // neighboring fixtures. max() so a future explicit (larger)
                // reservation supplied by SaaS is never shrunk here.
                fixture.footprint = fixture.footprint.max(profile.channel_count);
            }
            // COM-499: re-type a colour-emitting "Strobe" (the 200W RGB Strobe, in
            // 3/9-ch via main RGB or 24-ch via segments) to a coloured LedPanel so
            // the harmonic autopilot can colour + light it. Applied at this single
            // patch chokepoint so cached + SaaS-pulled patches normalize identically
            // (no spurious auto-flush diff).
            let has_color = fixture.channels.red.is_some()
                || fixture.channels.green.is_some()
                || fixture.channels.blue.is_some()
                || self
                    .fixture_profiles
                    .get(&fixture.id)
                    .map(|p| !segments_for_profile(p, fixture.start_address).is_empty())
                    .unwrap_or(false);
            fixture.visual_kind = Self::normalize_rgb_strobe(fixture.visual_kind, has_color);
        }
        self.last_synced_patch = Some(patch.clone());
        self.last_seen_patch = Some(patch.clone());
        self.dirty_since = None;
        self.patch = patch;
        // Rebuild from the ACTIVE patch — if Wedding is live, a concert pull must
        // not replace the Wedding backend with concert intents.
        self.rebuild_backend();
    }

    /// Load this stage's cached patch (if any) and apply it immediately, so the
    /// rig renders offline / before the SaaS pull lands. A later `StagePulled`
    /// overwrites it with fresh SaaS data when online. No-op without a cache hit.
    fn apply_cached_patch(&mut self, stage_id: i64) {
        let cached = self.db.as_ref().and_then(|db| db.cached_stage_patch(stage_id));
        if let Some(json) = cached {
            if let Ok(patch) = serde_json::from_str::<DmxPatch>(&json) {
                self.apply_patch(patch);
            }
        }
    }

    /// Which venue is currently being rendered.
    pub fn active_venue(&self) -> VenueKind {
        self.active_venue
    }

    /// The venue the *selected SaaS stage* authored. `Some(WeddingRoom)` makes the
    /// Wedding room authoritative — the app loop forces it so a wedding-stage pick
    /// doesn't snap back to the operator's Concert toggle. `None` (no stage
    /// selected, or the stage didn't declare a venue) leaves the local Venue
    /// toggle in charge, so the REACT-local "Wedding Room" demo stays reachable.
    pub fn stage_venue(&self) -> Option<VenueKind> {
        if self.selected_stage_id.is_some() {
            self.patch.venue
        } else {
            None
        }
    }

    /// The patch the active venue renders + the operator edits. A *selected SaaS
    /// stage* — concert or wedding — always lives in `self.patch` (the single sync
    /// target). A wedding stage authors `venue == WeddingRoom`, so when Wedding is
    /// active and `self.patch` is that stage, render + edit it (its edits flush
    /// back like any stage). Otherwise Wedding shows the REACT-local demo room in
    /// `wedding_patch`, which is never synced. `self.patch` is never swapped, so
    /// the SaaS sync stays correct regardless of venue.
    fn active_patch(&self) -> &DmxPatch {
        match self.active_venue {
            VenueKind::WeddingRoom if self.patch.venue == Some(VenueKind::WeddingRoom) => {
                &self.patch
            }
            VenueKind::WeddingRoom => &self.wedding_patch,
            VenueKind::ConcertStage => &self.patch,
        }
    }

    fn active_patch_mut(&mut self) -> &mut DmxPatch {
        match self.active_venue {
            VenueKind::WeddingRoom if self.patch.venue == Some(VenueKind::WeddingRoom) => {
                &mut self.patch
            }
            VenueKind::WeddingRoom => &mut self.wedding_patch,
            VenueKind::ConcertStage => &mut self.patch,
        }
    }

    /// Rebuild the intent backend from the ACTIVE venue's patch. Called on a
    /// venue switch and after any `apply_patch`, so a concert sync pull (which
    /// writes `self.patch`) never disturbs the Wedding backend, and a venue
    /// switch refreshes intents for the newly-active scene.
    fn rebuild_backend(&mut self) {
        let active = self.active_patch().clone();
        self.virtual_stage = VirtualStageBackend::new(self.virtual_stage.is_enabled(), active);
    }

    /// Switch which venue is rendered. The selected SaaS stage stays in
    /// `self.patch` (untouched — sync keeps working). The REACT-local Wedding demo
    /// lives in `wedding_patch`, seeded with a screen + uplights the first time —
    /// but only when no SaaS *wedding* stage occupies `self.patch` (else that
    /// authored rig is the Wedding scene and the demo must not shadow it). Only the
    /// intent backend is rebuilt (to the now-active patch). No-op if unchanged.
    pub fn set_active_venue(&mut self, venue: VenueKind) {
        if venue == self.active_venue {
            return;
        }
        self.active_venue = venue;
        if venue == VenueKind::WeddingRoom
            && self.wedding_patch.fixtures.is_empty()
            && self.patch.venue != Some(VenueKind::WeddingRoom)
        {
            self.wedding_patch = DmxPatch::demo_wedding_room(self.patch.universe, 1);
        }
        self.rebuild_backend();
    }

    /// Per-frame poll: drain worker events (apply pulled patches) and run
    /// the auto-flush debounce so any operator-side fixture mutations land
    /// on SaaS without a manual Save click.
    pub fn poll(&mut self, token: Option<&str>) {
        // Sync only ever touches `self.patch`, the single selected SaaS stage —
        // concert OR wedding (a wedding stage authors `venue == WeddingRoom` and
        // occupies `self.patch` like any stage). `wedding_patch` holds only the
        // REACT-local demo room, which is intentionally never synced. So the flush
        // below is correct regardless of the active venue — no special-casing.
        for event in self.sync_worker.poll_events() {
            match event {
                WorkerEvent::StagePulled { stage_id, patch } => {
                    if self.selected_stage_id != Some(stage_id) {
                        continue; // late pull for a stage we already left
                    }
                    self.apply_patch(patch);
                    // Cache the fresh SaaS patch so this stage renders offline next
                    // time (SaaS stays authority; this is the local fallback copy).
                    if let Some(db) = &self.db {
                        if let Ok(json) = serde_json::to_string(&self.patch) {
                            let _ = db.cache_stage_patch(stage_id, &json, None);
                        }
                    }
                }
            }
        }

        if self.selected_stage_id.is_none() {
            return;
        }
        if let Some(t) = token {
            self.sync_worker.update_token(t.to_string());
            self.cue_worker.update_token(t.to_string());
        }

        // Read-only stage (a shared admin template the current login can't edit):
        // never diff/flush `self.patch`. Any local nudges stay ephemeral and the
        // next pull restores the canonical layout; the server 403s the edit anyway.
        // (The REACT-local Wedding demo lives in `wedding_patch`, which this flush
        // never touches, so it stays freely editable regardless of this flag.)
        if !self.selected_stage_editable {
            self.dirty_since = None;
            return;
        }

        // Patch change tracking — reset the debounce timer each time the
        // operator nudges something.
        let changed_since_last_tick = match self.last_seen_patch.as_ref() {
            Some(prev) => prev != &self.patch,
            None => !self.patch.fixtures.is_empty(),
        };
        if changed_since_last_tick {
            self.last_seen_patch = Some(self.patch.clone());
            self.dirty_since = Some(Instant::now());
        }

        let baseline_diverged = self
            .last_synced_patch
            .as_ref()
            .map_or(!self.patch.fixtures.is_empty(), |b| b != &self.patch);
        if !baseline_diverged {
            self.dirty_since = None;
            return;
        }
        let dirty_for = match self.dirty_since {
            Some(t) => t.elapsed(),
            None => {
                self.dirty_since = Some(Instant::now());
                return;
            }
        };
        if dirty_for < SYNC_DEBOUNCE {
            return;
        }

        self.flush_dirty_ops();
    }

    fn flush_dirty_ops(&mut self) {
        let baseline = self.last_synced_patch.clone().unwrap_or_else(|| DmxPatch {
            universe: self.patch.universe,
            fixtures: vec![],
            // Match current trusses in the never-synced fallback so a seeded
            // default layout is not spuriously pushed before a real edit.
            trusses: self.patch.trusses.clone(),
            venue: self.patch.venue,
        });
        let baseline_by_key: std::collections::HashMap<&str, &FixturePatch> =
            baseline.fixtures.iter().map(|f| (f.id.as_str(), f)).collect();
        let current_by_key: std::collections::HashMap<&str, &FixturePatch> = self
            .patch
            .fixtures
            .iter()
            .map(|f| (f.id.as_str(), f))
            .collect();

        // Adds + updates
        for fixture in &self.patch.fixtures {
            match baseline_by_key.get(fixture.id.as_str()) {
                None => self.sync_worker.submit(SyncOp::Create {
                    key: fixture.id.clone(),
                    fixture: fixture.clone(),
                }),
                Some(prev) if *prev != fixture => self.sync_worker.submit(SyncOp::Update {
                    key: fixture.id.clone(),
                    fixture: fixture.clone(),
                }),
                Some(_) => {}
            }
        }
        // Deletes
        for prev in &baseline.fixtures {
            if !current_by_key.contains_key(prev.id.as_str()) {
                self.sync_worker.submit(SyncOp::Delete {
                    key: prev.id.clone(),
                });
            }
        }
        // Trusses round-trip as a whole-layout PATCH (not per-row), so diff the
        // set and push once when it changed.
        if self.patch.trusses != baseline.trusses {
            self.sync_worker.submit(SyncOp::SetTrusses {
                trusses: self.patch.trusses.clone(),
            });
        }

        self.last_synced_patch = Some(self.patch.clone());
        self.dirty_since = None;
    }

    /// Render the latest lighting intent and send an ArtDmx packet.
    ///
    /// `metrics` carries the lighting signals (`beat_confidence`, `raw_energy`,
    /// …) the show brain needs that the `CueSnapshot` does not. `None` falls
    /// back to defaults, keeping the brain conservative rather than panicking.
    pub fn update(
        &mut self,
        snapshot: &CueSnapshot,
        metrics: Option<&LightingMetrics>,
        overrides: &FixtureOverrides,
        silent: bool,
    ) {
        let dt = self.last_frame.elapsed();
        if dt < self.min_interval {
            return;
        }
        self.last_frame = Instant::now();

        // Advance the wall-clock strobe phase by the REAL frame dt (capped so a
        // hitch can't jump it) — this is what lets the manual STROBE button
        // flash the rig even in silence, where `snapshot.frame_time` is frozen.
        // The rate is the operator's "Strobe Rate" slider (1–20 Hz).
        let strobe_hz = snapshot.strobe_rate_hz.clamp(1.0, 20.0);
        self.strobe_clock = (self.strobe_clock + dt.as_secs_f32().min(0.1)).rem_euclid(1.0e6);
        let strobe_on = (self.strobe_clock * strobe_hz).rem_euclid(1.0) < 0.5;

        // Silence master: ease toward 0 when the app SilenceGate confirms silence,
        // snap back when audio returns. The gate is the truth (raw band energy); the
        // smoothed `snapshot.energy` floors high in silence, so this is what lets the
        // rig — a dense bar especially — settle dark in a quiet break instead of
        // riding the floor. Folded into global brightness below.
        const SILENCE_FADE_OUT_TAU: f32 = 0.45;
        const SILENCE_FADE_IN_TAU: f32 = 0.08;
        let dt_s = dt.as_secs_f32().min(0.1);
        let (target, tau) = if silent { (0.0, SILENCE_FADE_OUT_TAU) } else { (1.0, SILENCE_FADE_IN_TAU) };
        self.silence_gain += (target - self.silence_gain) * (1.0 - (-dt_s / tau).exp());

        // Classify the musical arc and gate every fixture lane. Observed-only
        // for now (see diagnostics below); slice 1's render wiring makes the
        // renderer consume these lanes.
        let metrics = metrics.copied().unwrap_or_default();
        let brain = self.show_brain.tick(snapshot, &metrics, &self.clock_inputs);

        // COM-499 autonomous look rotation (Koob): advance a rig-wide look_index on
        // a polarity-quadrant change, a phrase boundary (~every 2 bars off the show
        // clock beat), or a drop edge, so the no-cue autopilot cycles colour +
        // segment effect across the rig instead of sitting on one look.
        let phrase_idx = (brain.clock.beat / 8.0).floor() as i64;
        let polarity_changed = snapshot.polarity_quadrant != self.look_last_polarity;
        let phrase_changed = phrase_idx != self.look_last_phrase;
        let drop_edge = snapshot.drop_impact > 0.6 && self.look_drop_armed;
        if polarity_changed || phrase_changed || drop_edge {
            self.look_index = self.look_index.wrapping_add(1);
        }
        self.look_last_polarity = snapshot.polarity_quadrant;
        self.look_last_phrase = phrase_idx;
        if snapshot.drop_impact > 0.6 {
            self.look_drop_armed = false;
        } else if snapshot.drop_impact < 0.3 {
            self.look_drop_armed = true;
        }
        // Advance the autopilot tightening-chase integrator at the CURRENT
        // tension-shortened period (every frame, whatever the active tier, so
        // the head is mid-sweep when the chase tier engages). The integrated
        // cycle is the only head source that's continuous while tension varies
        // the period — see lighting_cue::advance_auto_chase.
        let (cycle, last_beat) = crate::emerge::cue::lighting_cue::advance_auto_chase(
            self.auto_chase_cycle,
            self.auto_chase_last_beat,
            brain.clock.beat,
            snapshot.tension_bank,
        );
        self.auto_chase_cycle = cycle;
        self.auto_chase_last_beat = last_beat;
        // Mover choreography: one director decides the figure + harmonic
        // steering for the whole rig this frame (arc from the show brain,
        // chords from the snapshot, real frame dt for the crossfades).
        // Dynamics is this frame's reading, straight from the brain that was
        // just ticked: the copy on the engine snapshot is a frame behind, and
        // a path that never feeds the engine would not carry one at all.
        let snapshot = &{
            let mut s = snapshot.clone();
            s.dynamics = self.show_brain.dynamics();
            s
        };
        let movement = self.movement.tick(
            snapshot,
            brain.arc_state,
            brain.clock.beat,
            brain.clock.tempo_bpm,
            dt_s,
        );
        // Shadow `snapshot` with a clone carrying the look_index + chase cycle
        // + movement frame, so the renderer + virtual-stage preview consume
        // them with no further call-site plumbing.
        let snapshot = &{
            let mut s = snapshot.clone();
            s.look_index = self.look_index;
            s.auto_chase_cycle = self.auto_chase_cycle;
            s.strobe_on = strobe_on;
            s.movement = movement;
            s
        };

        // The brain ticks (arc classification + look rotation, above) even with
        // DMX output disabled: `current_arc` drives the CueEngine buildup macro
        // for EVERY provider (OSC/Hue/WLED), not just the wire. Only rendering
        // and emission are gated here.
        if !self.enabled {
            return;
        }

        let mut cues = self.cue_worker.active_cues();
        // COM-499 rule 8 stop/FADE (renderer-owned, zero cross-thread coupling): a
        // cue leaving the runnable set ramps a render-only clone's fade_factor 1→0
        // over STOP_FADE_SECS instead of hard-cutting to black. Two entry paths:
        //  (a) observed STOP_REQUESTED in the published slot, and
        //  (b) DISAPPEARANCE — a cue runnable last frame is simply gone this frame
        //      (operator-local stop, or a SaaS stop never seen as STOP_REQUESTED
        //      under the slow ~15s heartbeat). Both fade; neither snaps (anti-jank).
        let fade_now = std::time::Instant::now();
        let present_ids: std::collections::HashSet<String> =
            cues.iter().map(|c| c.launch_id.clone()).collect();
        let live_ids: std::collections::HashSet<String> = cues
            .iter()
            .filter(|c| c.is_runnable())
            .map(|c| c.launch_id.clone())
            .collect();
        let still_stopping: std::collections::HashSet<String> = cues
            .iter()
            .filter(|c| c.status == "STOP_REQUESTED")
            .map(|c| c.launch_id.clone())
            .collect();
        // (a) observed STOP_REQUESTED -> start a fade clone (forced runnable).
        for cue in &cues {
            if cue.status == "STOP_REQUESTED" && !self.cue_fades.contains_key(&cue.launch_id) {
                let mut clone = cue.clone();
                clone.status = "ACTIVE".to_string(); // render-only: drive it through the fade
                self.cue_fades.insert(cue.launch_id.clone(), (clone, fade_now));
            }
        }
        // (b) vanished-while-runnable -> fade from its last-known bindings.
        for (id, last) in &self.last_runnable {
            if !present_ids.contains(id) && !self.cue_fades.contains_key(id) {
                self.cue_fades.insert(id.clone(), (last.clone(), fade_now));
            }
        }
        self.cue_fades.retain(|id, (clone, started)| {
            // A re-launch (runnable again) drops the fade — the live cue takes over.
            if live_ids.contains(id) {
                return false;
            }
            let elapsed = fade_now.duration_since(*started).as_secs_f32();
            clone.fade_factor = (1.0 - elapsed / lighting_cue::STOP_FADE_SECS).max(0.0);
            if still_stopping.contains(id) {
                // Still STOP_REQUESTED in the slot: hold the clone (at 0 once ramped)
                // so it is never re-captured/re-faded; dropped when it finally leaves
                // STOP_REQUESTED (a later frame falls through to the else branch).
                true
            } else {
                // Vanished from the slot: drop once the ramp completes.
                elapsed < lighting_cue::STOP_FADE_SECS
            }
        });
        // Snapshot this frame's REAL runnable cues (BEFORE appending fade clones) so
        // next frame can detect disappearance.
        self.last_runnable = cues
            .iter()
            .filter(|c| c.is_runnable())
            .map(|c| (c.launch_id.clone(), c.clone()))
            .collect();
        cues.extend(self.cue_fades.values().map(|(c, _)| c.clone()));
        let mut cue_reports: std::collections::HashMap<String, lighting_cue::CueApplyReport> =
            std::collections::HashMap::new();
        // Fold the silence master into global brightness for the render only — it
        // scales just the intensity channels and correctly skips lasers. Clone the
        // snapshot only while the gate is actually engaged. A manual operator
        // override (BLINDER/STROBE/UV) BYPASSES it — those buttons must fire with
        // or without music, so the quiet-section auto-dim must not black them out.
        let silenced_snapshot;
        let render_snapshot = if self.silence_gain < 0.999 && !snapshot.manual_light_override() {
            let mut s = snapshot.clone();
            s.global_brightness *= self.silence_gain;
            silenced_snapshot = s;
            &silenced_snapshot
        } else {
            snapshot
        };
        let frame = mapping::render_dmx_frame_with_cues(
            render_snapshot,
            &self.patch,
            &self.fixture_profiles,
            Some(&brain),
            &cues,
            &mut cue_reports,
            overrides,
        );
        // COM-499 report.last_frame_at: every cue applied_bindings touched this
        // frame gets stamped with the frame's wall-clock time, so the worker's
        // next /report carries a real executor heartbeat per cue.
        let frame_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        for report in cue_reports.values_mut() {
            report.last_frame_at = Some(frame_at.clone());
        }
        self.cue_worker.publish_reports(cue_reports);
        // Retain the rendered frame so the virtual-stage preview can read true
        // per-segment color straight off the wire (pixel bars). 512 bytes.
        self.last_dmx_frame = Some(frame.clone());
        // Sync the preview to the LIVE active patch each frame so operator drags
        // (which mutate active_patch) show immediately — otherwise the preview
        // renders the stage-load snapshot and a moved fixture only appears after a
        // restart.
        let live_patch = self.active_patch().clone();
        self.virtual_stage.set_patch(live_patch);
        // Preview honesty: feed the SAME silence-dimmed snapshot the wire rendered
        // from (so the on-screen rig settles in a quiet break exactly as the
        // fixtures do) plus the profiles, so a GigBAR expands into its 8 section
        // glyphs on-screen just as it expands into 8 sections on the wire.
        self.virtual_stage.update(render_snapshot, Some(&brain), overrides, &self.fixture_profiles);

        // TRIG-DIAG probe: while a manual override (BLINDER/STROBE/UV) is held,
        // log what actually landed on the wire frame + the preview glyph
        // intents — ground truth at the render output, 1Hz.
        if render_snapshot.manual_light_override()
            && self
                .trig_probe_at
                .map(|t| t.elapsed().as_secs_f32() >= 1.0)
                .unwrap_or(true)
        {
            self.trig_probe_at = Some(Instant::now());
            let lit_bytes = frame.data.iter().filter(|&&b| b > 0).count();
            let max_byte = frame.data.iter().copied().max().unwrap_or(0);
            let glyphs = self.virtual_stage.latest_intents();
            let glyph_lit = glyphs
                .iter()
                .filter(|i| i.dimmer > 0.5 || i.white > 0.5 || i.uv > 0.1)
                .count();
            let max_dim = glyphs.iter().map(|i| i.dimmer).fold(0.0f32, f32::max);
            let muted = self
                .patch
                .fixtures
                .iter()
                .filter(|f| overrides.get(&f.id).mute)
                .count();
            log::info!(
                "[TRIG-DIAG-DMX] override render: wire(lit={}/512 max={}) glyphs(lit={}/{} max_dim={:.2}) patch_fixtures={} output={} backend={:?} silence_gain={:.2} any_solo={} muted={} strobe_on={} blackout_active={} gb={:.2}",
                lit_bytes,
                max_byte,
                glyph_lit,
                glyphs.len(),
                max_dim,
                self.patch.fixtures.len(),
                self.output.is_some(),
                self.backend,
                self.silence_gain,
                overrides.any_solo(&self.patch),
                muted,
                render_snapshot.strobe_on,
                render_snapshot.blackout_active,
                render_snapshot.global_brightness,
            );
        }
        if let Some(output) = self.output.as_mut() {
            if let Err(err) = output.send_frame(frame.universe, &frame.data) {
                log::warn!("[DMX] output send failed: {}", err);
                self.enabled = false;
                return;
            }
        }

        self.diag_frame += 1;
        if self.diag_frame % 120 == 1 {
            log::info!(
                "[DMX] frame={} universe={} energy={:.2} tension={:.2} blackout={} arc={:?} laser={} strobe={}",
                self.diag_frame,
                frame.universe,
                snapshot.energy,
                snapshot.tension,
                frame.blackout,
                brain.arc_state,
                brain.lane(FixtureFamily::Laser).eligible,
                brain.lane(FixtureFamily::Strobe).eligible,
            );
        }
    }
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmxBackend {
    Disabled,
    ArtNet,
    Ola,
    Usb,
}

impl DmxBackend {
    fn from_env() -> Self {
        Self::from_values(
            env::var("REACT_DMX_BACKEND").ok(),
            env_flag("REACT_DMX_OLA"),
            env_flag("REACT_DMX_ARTNET"),
            env_flag("REACT_DMX_USB"),
        )
    }

    fn from_values(
        backend: Option<String>,
        ola_enabled: bool,
        artnet_enabled: bool,
        usb_enabled: bool,
    ) -> Self {
        match backend.map(|v| v.to_ascii_lowercase()) {
            Some(value) if value == "ola" => Self::Ola,
            Some(value) if value == "artnet" || value == "art-net" => Self::ArtNet,
            Some(value) if value == "usb" || value == "opendmx" || value == "open-dmx" => Self::Usb,
            Some(value) if value == "off" || value == "disabled" || value == "0" => Self::Disabled,
            Some(value) => {
                log::warn!("[DMX] Unknown REACT_DMX_BACKEND='{}'; DMX disabled", value);
                Self::Disabled
            }
            None if ola_enabled => Self::Ola,
            None if artnet_enabled => Self::ArtNet,
            None if usb_enabled => Self::Usb,
            None => Self::Disabled,
        }
    }

    /// Stable key for persistence + the `REACT_DMX_BACKEND` value.
    pub fn as_key(self) -> &'static str {
        match self {
            Self::Disabled => "off",
            Self::ArtNet => "artnet",
            Self::Ola => "ola",
            Self::Usb => "usb",
        }
    }

    /// Parse a persisted key back to a backend (unknown -> Disabled).
    pub fn from_key(key: &str) -> Self {
        Self::from_values(Some(key.to_string()), false, false, false)
    }

    /// Operator-facing label for the DMX panel.
    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "Off",
            Self::ArtNet => "Art-Net",
            Self::Ola => "OLA",
            Self::Usb => "USB (Open DMX)",
        }
    }
}

/// Snapshot of the live DMX output for the operator DMX panel.
#[derive(Clone, Debug)]
pub struct DmxOutputStatus {
    pub backend: DmxBackend,
    /// Output is up (USB cable present / socket bound / olad reachable).
    pub connected: bool,
    /// Human hint: USB port path, Art-Net target, or OLA host.
    pub detail: String,
}

enum DmxOutput {
    ArtNet(ArtNetSender),
    Ola(OlaDmxSender),
    Usb(UsbDmxSender),
}

impl DmxOutput {
    fn send_frame(&mut self, universe: u16, data: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::ArtNet(sender) => sender.send_frame(universe, data),
            Self::Ola(sender) => sender.send_frame(universe, data),
            Self::Usb(sender) => sender.send_frame(universe, data),
        }
    }
}

impl fmt::Debug for DmxOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ArtNet(_) => f.write_str("DmxOutput::ArtNet"),
            Self::Ola(_) => f.write_str("DmxOutput::Ola"),
            Self::Usb(_) => f.write_str("DmxOutput::Usb"),
        }
    }
}

fn env_u16(name: &str) -> Option<u16> {
    env::var(name).ok().and_then(|v| v.parse::<u16>().ok())
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Parse an Art-Net target. Accepts `ip:port`, or a bare `ip` (defaulting to the
/// standard Art-Net UDP port 6454). Returns `None` for empty/unparseable input.
fn parse_artnet_target(raw: &str) -> Option<SocketAddr> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(addr) = raw.parse::<SocketAddr>() {
        return Some(addr);
    }
    format!("{}:6454", raw).parse::<SocketAddr>().ok()
}

/// Split an OLA `host:port` endpoint. A bare host defaults to olad's RPC port
/// 9010; an unparseable port also falls back to 9010.
fn parse_ola_endpoint(raw: &str) -> (String, u16) {
    let raw = raw.trim();
    match raw.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => {
            (host.to_string(), port.trim().parse::<u16>().unwrap_or(9010))
        }
        _ => (raw.to_string(), 9010),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hue::CueSnapshot;

    #[test]
    fn parse_artnet_target_accepts_socket_and_bare_ip() {
        // ip:port parses as given.
        assert_eq!(
            parse_artnet_target("192.168.1.50:6454"),
            Some("192.168.1.50:6454".parse().unwrap())
        );
        // Bare IP defaults to the standard Art-Net port 6454.
        assert_eq!(
            parse_artnet_target("10.0.0.7"),
            Some("10.0.0.7:6454".parse().unwrap())
        );
        // Whitespace is tolerated.
        assert_eq!(
            parse_artnet_target("  2.0.0.1:6454 "),
            Some("2.0.0.1:6454".parse().unwrap())
        );
        // Empty / non-numeric host -> None (hostnames aren't valid Art-Net targets).
        assert_eq!(parse_artnet_target(""), None);
        assert_eq!(parse_artnet_target("not-an-ip"), None);
    }

    #[test]
    fn parse_ola_endpoint_splits_host_and_port() {
        assert_eq!(parse_ola_endpoint("127.0.0.1:9010"), ("127.0.0.1".to_string(), 9010));
        // Bare host defaults to olad's RPC port 9010.
        assert_eq!(parse_ola_endpoint("olad.local"), ("olad.local".to_string(), 9010));
        // Unparseable port falls back to 9010.
        assert_eq!(parse_ola_endpoint("127.0.0.1:abc"), ("127.0.0.1".to_string(), 9010));
        // A non-default port is honored.
        assert_eq!(parse_ola_endpoint("192.168.0.9:9090"), ("192.168.0.9".to_string(), 9090));
    }

    #[test]
    fn rgb_strobe_retypes_to_ledpanel_white_strobe_stays() {
        // A colour-emitting 200W RGB Strobe (has_color) -> re-typed to a coloured
        // LedPanel so the autopilot colours + lights it. A true white-only strobe
        // (no colour) stays a Strobe. Non-strobe kinds are untouched.
        assert_eq!(
            DmxManager::normalize_rgb_strobe(StageVisualKind::Strobe, true),
            StageVisualKind::LedPanel,
            "a colour-emitting strobe becomes a coloured LedPanel"
        );
        assert_eq!(
            DmxManager::normalize_rgb_strobe(StageVisualKind::Strobe, false),
            StageVisualKind::Strobe,
            "a white-only strobe stays a Strobe"
        );
        assert_eq!(
            DmxManager::normalize_rgb_strobe(StageVisualKind::Wash, true),
            StageVisualKind::Wash,
            "non-strobe kinds are untouched"
        );
    }

    #[test]
    fn manager_is_disabled_without_env() {
        std::env::remove_var("REACT_DMX_ARTNET");
        std::env::remove_var("REACT_DMX_OLA");
        std::env::remove_var("REACT_DMX_BACKEND");
        let manager = DmxManager::from_env();
        assert!(!manager.is_enabled());
    }

    #[test]
    fn backend_selector_preserves_legacy_artnet_env() {
        assert_eq!(DmxBackend::from_values(None, false, true, false), DmxBackend::ArtNet);
    }

    #[test]
    fn backend_selector_accepts_ola() {
        assert_eq!(
            DmxBackend::from_values(Some("ola".to_string()), false, true, false),
            DmxBackend::Ola
        );
    }

    #[test]
    fn backend_selector_accepts_usb() {
        // Explicit REACT_DMX_BACKEND=usb (and the opendmx alias) selects USB...
        assert_eq!(
            DmxBackend::from_values(Some("usb".to_string()), false, false, false),
            DmxBackend::Usb
        );
        assert_eq!(
            DmxBackend::from_values(Some("opendmx".to_string()), false, false, false),
            DmxBackend::Usb
        );
        // ...as does the REACT_DMX_USB flag with no explicit backend.
        assert_eq!(DmxBackend::from_values(None, false, false, true), DmxBackend::Usb);
    }

    /// A disabled, output-less manager for pure in-process logic tests (venue
    /// routing, demo-seed guard). No DMX / sync / db side effects fire.
    fn venue_test_manager() -> DmxManager {
        DmxManager {
            enabled: false,
            output: None,
            backend: DmxBackend::Disabled,
            silence_gain: 1.0,
            start_address: 1,
            virtual_stage: VirtualStageBackend::new(false, DmxPatch::demo_rgbw_wash(0, 1)),
            patch: DmxPatch::demo_rgbw_wash(0, 1),
            last_frame: Instant::now() - Duration::from_secs(1),
            strobe_clock: 0.0,
            min_interval: Duration::from_millis(0),
            trig_probe_at: None,
            diag_frame: 0,
            show_brain: ShowBrain::new(),
            clock_inputs: ClockInputs::default(),
            fixture_profiles: std::collections::HashMap::new(),
            profile_library: profiles::FixtureProfileLibrary::default(),
            selected_stage_id: None,
            selected_stage_editable: true,
            is_admin: false,
            last_synced_patch: None,
            last_seen_patch: None,
            dirty_since: None,
            stage_list: Vec::new(),
            stage_list_fetched_at: None,
            sync_worker: StageSyncWorker::new(),
            last_dmx_frame: None,
            cue_worker: lighting_cue_worker::LightingCueWorker::new(),
            cue_fades: std::collections::HashMap::new(),
            last_runnable: std::collections::HashMap::new(),
            local_cue_seq: 0,
            look_index: 0,
            movement: crate::emerge::MovementDirector::new(),
            auto_chase_cycle: 0.0,
            auto_chase_last_beat: 0.0,
            look_last_polarity: 0,
            look_last_phrase: 0,
            look_drop_armed: true,
            db: None,
            stage_restore_attempted: false,
            active_venue: VenueKind::ConcertStage,
            wedding_patch: DmxPatch { universe: 0, fixtures: Vec::new(), trusses: Vec::new(), venue: None },
        }
    }

    /// An empty patch that authors the Wedding venue — i.e. what a selected SaaS
    /// wedding stage looks like sitting in `self.patch`.
    fn wedding_stage_patch(universe: u16) -> DmxPatch {
        DmxPatch {
            universe,
            fixtures: Vec::new(),
            trusses: Vec::new(),
            venue: Some(VenueKind::WeddingRoom),
        }
    }

    /// A small rig patched the way the concert stages are: the real bundled
    /// hardware profiles (8-cell panels in all three modes, both beam modes,
    /// scan lasers) on top of the demo stage's fixtures and trusses.
    fn concert_like_patch() -> DmxPatch {
        const PANEL: &str = "0443a2c9-6ca7-486e-a573-3e7c8a3b4367";
        const BEAM: &str = "01cc2583-e793-4efc-8b49-448ac7d4ab21";
        const LASER: &str = "3f14db8f-d9ef-4955-9dbd-a6c2c4b31086";
        let fx = |name: &str, profile: &str, mode: &str, kind: StageVisualKind, addr: u16, x: f32| {
            let moving = kind == StageVisualKind::Beam;
            FixturePatch {
                id: name.to_string(),
                name: name.to_string(),
                universe: 0,
                start_address: addr,
                profile_id: profile.to_string(),
                mode: mode.to_string(),
                position: StageVec3 { x, y: 1.0, z: if moving { 11.0 } else { 8.0 } },
                rotation: if moving {
                    StageVec3 { x: 42.0, y: 86.0, z: 0.0 }
                } else {
                    StageVec3::zero()
                },
                scale: StageVec3 { x: 1.0, y: if moving { 15.0 } else { 1.0 }, z: 1.0 },
                // SaaS authors nearly everything as Accent — panels included.
                role: FixtureRole::Accent,
                visual_kind: kind,
                tags: vec![],
                channels: ChannelMap::default(),
                footprint: 0,
                screen: None,
            }
        };
        let mut patch = DmxPatch::demo_virtual_stage(0, 200);
        patch.venue = Some(VenueKind::ConcertStage);
        patch.fixtures.extend([
            fx("panel-a", PANEL, "24 Channel", StageVisualKind::LedPanel, 1, -7.0),
            fx("panel-b", PANEL, "24 Channel", StageVisualKind::LedPanel, 25, 7.0),
            fx("panel-9", PANEL, "9 Channel", StageVisualKind::LedPanel, 49, -4.5),
            fx("panel-3", PANEL, "3 Channel", StageVisualKind::LedPanel, 58, 4.5),
            fx("beam-a", BEAM, "20 Channel", StageVisualKind::Beam, 61, -2.0),
            fx("beam-b", BEAM, "20 Channel", StageVisualKind::Beam, 81, 2.0),
            fx("beam-c", BEAM, "16 Channel", StageVisualKind::Beam, 101, 0.0),
            fx("laser-a", LASER, "16 CHANNEL", StageVisualKind::Laser, 117, -6.0),
            fx("laser-b", LASER, "16 CHANNEL", StageVisualKind::Laser, 133, 6.0),
        ]);
        patch
    }

    /// Frame `i` (30 fps) of a loud synthetic song: a chord change every 16
    /// frames, alternating major/minor and polarity, a build that drops every
    /// 32, and the manual strobe / blinder / UV triggers held for a few frames.
    fn song_frame(i: u32) -> CueSnapshot {
        let t = i as f32 / 30.0;
        let bar = (i / 16) as u8;
        let root = [0u8, 5, 7, 9, 2, 6][bar as usize % 6];
        let step = i % 16;
        CueSnapshot {
            energy: 0.9,
            energy_slow: 0.85,
            reactivity: 1.0,
            global_brightness: 1.0,
            trust: 1.0,
            intensity_license: 1.0,
            bass: 0.7,
            mid: 0.5,
            high: 0.4,
            kick: i % 8 == 0,
            onset: i % 4 == 0,
            dominant_pc: root,
            chord_root: root,
            chord_change_env: if step < 4 { 1.0 - step as f32 / 4.0 } else { 0.0 },
            chord_root_motion: bar % 7,
            chord_root_dir: if bar % 2 == 0 { 1 } else { -1 },
            harmonic_rhythm: 0.5,
            tonnetz_strength: 0.8,
            tonnetz_hue: root as f32 / 12.0,
            tonnetz_x: 0.6 * t.cos(),
            tonnetz_y: 0.6 * t.sin(),
            major_smooth: if bar % 2 == 0 { 0.8 } else { 0.2 },
            stability: 0.5,
            tension_bank: (i % 32) as f32 / 32.0,
            drop_impact: if i % 32 == 31 { 1.0 } else { 0.0 },
            polarity_quadrant: bar % 4,
            trigger_strobe: (40..48).contains(&i),
            trigger_blinder: (56..60).contains(&i),
            trigger_uv: (64..68).contains(&i),
            frame_time: t,
            dt: 1.0 / 30.0,
            frame_seq: i as u64,
            ..CueSnapshot::default()
        }
    }

    /// What picking a stage with the preview open does, minus the window and
    /// the wire: apply the patch, render frames of a loud song with changing
    /// harmony, and draw the whole stage through egui. Returns the most DMX
    /// bytes lit in any frame. Test builds keep integer-overflow checks on,
    /// exactly like a `cargo run` debug build, so arithmetic that only survives
    /// by wrapping in a release build panics here instead of in front of the
    /// operator.
    fn draw_stage_headless(patch: DmxPatch, library: profiles::FixtureProfileLibrary) -> usize {
        let mut m = venue_test_manager();
        m.profile_library = library;
        m.apply_patch(patch);
        m.set_virtual_stage_enabled(true);
        assert!(m.output.is_none(), "headless: nothing may reach a rig");

        let ctx = egui::Context::default();
        let mut view = crate::ui::VirtualStageView::default();
        let pipeline: std::sync::Arc<parking_lot::Mutex<Option<crate::render::RenderPipeline>>> =
            std::sync::Arc::new(parking_lot::Mutex::new(None));
        let mut overrides = FixtureOverrides::default();
        let mut venue = VenueKind::ConcertStage;
        let library_profiles = m.profile_library().all_profiles();
        let mut most_lit = 0;

        for i in 0..96u32 {
            let t = i as f32 / 30.0;
            let step = i % 16;
            let snapshot = song_frame(i);
            // Walk every autopilot look, then the far end of the counter.
            m.look_index = if i >= 90 { u32::MAX - (i - 90) } else { i / 6 };
            m.update(&snapshot, None, &overrides, false);

            let states = m.fixture_intents().to_vec();
            let glyphs = m.glyph_fixtures().to_vec();
            let parents = m.glyph_parents().to_vec();
            let profiles = m.fixture_profiles().clone();
            let frame = m.last_dmx_frame().cloned();
            assert_eq!(states.len(), glyphs.len(), "one intent per drawn glyph");
            assert!(!glyphs.is_empty(), "the preview has fixtures to draw");
            if let Some(f) = &frame {
                most_lit = most_lit.max(f.data.iter().filter(|&&b| b > 0).count());
            }

            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440.0, 900.0),
                )),
                time: Some(t as f64),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                view.show(
                    ctx,
                    crate::emerge::StageViewPresetKind::Foh,
                    1.0,
                    0.0,
                    0.0,
                    &pipeline,
                    m.fixture_patch_mut(),
                    &mut overrides,
                    &glyphs,
                    &states,
                    &parents,
                    &profiles,
                    &library_profiles,
                    true,
                    frame.as_ref(),
                    step < 8,
                    true,
                    &mut venue,
                    true,
                );
            });
        }
        most_lit
    }

    /// Regression: choosing a stage with lit 8-cell panels crashed a debug
    /// build, because the panel's dot shimmer multiplied its way past
    /// `u32::MAX` on the third cell.
    #[test]
    fn the_stage_preview_draws_a_lit_rig_without_panicking() {
        let lit = draw_stage_headless(concert_like_patch(), profiles::FixtureProfileLibrary::bundled());
        assert!(lit > 60, "the rig is actually lit while it is drawn: {lit} bytes");
    }

    /// Heads daisy-chained on one DMX address are sent one pan/tilt. However
    /// each copy is placed on the stage, the preview has to draw every one of
    /// them travelling exactly as far as that address is sent — the beam levels
    /// once worked each copy out from its own placed aim, so the preview showed
    /// several tilts where the rig could only ever get one.
    #[test]
    fn heads_sharing_an_address_are_drawn_as_far_as_they_are_sent() {
        let mut patch = concert_like_patch();
        // Like the concert stages: more copies hung on beam-a's address, and
        // beam-c's address stood on the floor firing up — each copy placed at
        // its own angle.
        let copy = |patch: &mut DmxPatch, of: &str, name: &str, aim: f32, x: f32| {
            let mut head = patch.fixtures.iter().find(|f| f.id == of).expect("source head").clone();
            head.id = name.to_string();
            head.name = name.to_string();
            head.rotation.x = aim;
            head.position.x = x;
            patch.fixtures.push(head);
        };
        copy(&mut patch, "beam-a", "beam-a2", 15.0, -3.0);
        copy(&mut patch, "beam-a", "beam-a3", 30.0, -1.0);
        patch.fixtures.iter_mut().find(|f| f.id == "beam-c").expect("beam-c").rotation.x = 118.0;
        copy(&mut patch, "beam-c", "beam-c2", 126.0, 1.0);
        copy(&mut patch, "beam-c", "beam-c3", 122.0, -1.0);

        let mut m = venue_test_manager();
        m.profile_library = profiles::FixtureProfileLibrary::bundled();
        m.apply_patch(patch);
        m.set_virtual_stage_enabled(true);
        assert!(m.output.is_none(), "headless: nothing may reach a rig");
        let overrides = FixtureOverrides::default();

        let (mut worst, mut furthest, mut heads) = (0.0f32, 0.0f32, 0usize);
        let mut lowest_aerial = f32::MAX;
        for i in 0..96u32 {
            m.update(&song_frame(i), None, &overrides, false);
            let frame = m.last_dmx_frame().expect("a rendered frame").clone();
            for (fx, drawn) in m.glyph_fixtures().iter().zip(m.fixture_intents()) {
                // The demo stage's own heads have no hardware profile, and
                // so no 16-bit position to read back.
                let Some(profile) = m.fixture_profiles().get(&fx.id) else { continue };
                if fx.visual_kind != StageVisualKind::Beam {
                    continue;
                }
                // Degrees from the head's own centre, off the 16-bit wire value.
                let sent = |coarse: Option<u16>, axis: &str, travel_deg: f32| {
                    let byte = |addr: u16| frame.data[addr as usize - 1] as u32;
                    let fine = mapping::fine_channel(profile, fx, axis).expect("a fine channel");
                    let v16 = byte(coarse.expect("a coarse channel")) << 8 | byte(fine);
                    (v16 as f32 / 65535.0 - 0.5) * travel_deg
                };
                let sent_pan = sent(fx.channels.pan, "pan", mapping::HEAD_PAN_RANGE_DEG);
                let sent_tilt = sent(fx.channels.tilt, "tilt", mapping::HEAD_TILT_RANGE_DEG);
                // Degrees from the placed aim, as the preview draws it.
                let drawn_pan = (drawn.pan - 0.5) * crate::emerge::MOVER_PAN_WINDOW_DEG;
                let drawn_tilt = (drawn.tilt - 0.5) * crate::emerge::MOVER_TILT_WINDOW_DEG;
                worst = worst.max((sent_pan - drawn_pan).abs()).max((sent_tilt - drawn_tilt).abs());
                furthest = furthest.max(drawn_tilt.abs());
                if fx.rotation.x > 100.0 {
                    lowest_aerial = lowest_aerial.min(fx.rotation.x + drawn_tilt);
                }
                heads += 1;
            }
        }
        assert_eq!(heads, 96 * 7, "every copy of every head was compared, every frame");
        assert!(furthest > 20.0, "the heads actually went somewhere: {furthest} deg of tilt");
        assert!(worst < 0.05, "a head was drawn {worst} deg away from what its address was sent");
        // The copies firing up share one tilt too, so the horizon is kept by
        // the one placed lowest and the rest ride above it.
        assert!(lowest_aerial >= 89.9, "an up-firing copy was flown down to {lowest_aerial} deg");
    }

    #[test]
    fn update_noops_when_disabled() {
        let mut manager = venue_test_manager();
        manager.update(&CueSnapshot::default(), None, &FixtureOverrides::default(), false);
        assert_eq!(manager.diag_frame, 0);
    }

    #[test]
    fn admin_overlay_marks_shared_stages_editable() {
        use crate::stage_sync::StageSummary;
        let shared = |editable: bool| StageSummary {
            id: 1, name: "Shared Wedding".into(), slug: Some("react-wedding-demo".into()),
            active: false, venue: None, shared: true, editable,
        };
        let personal = StageSummary {
            id: 2, name: "My Wedding".into(), slug: None,
            active: false, venue: None, shared: false, editable: true,
        };

        // Admin: a shared stage with a stale `editable=false` becomes editable.
        let mut admin = venue_test_manager();
        admin.stage_list = vec![shared(false), personal.clone()];
        admin.set_admin(true);
        assert!(admin.stage_list[0].editable, "admin -> shared stage editable");
        assert!(admin.stage_list[1].editable, "personal stays editable");

        // Non-admin: the shared stage stays read-only (server is authoritative).
        let mut guest = venue_test_manager();
        guest.stage_list = vec![shared(false)];
        guest.set_admin(false);
        assert!(!guest.stage_list[0].editable, "non-admin: shared stays read-only");
    }

    #[test]
    fn wedding_saas_stage_renders_from_self_patch() {
        // A selected SaaS wedding stage occupies `self.patch` (venue == WeddingRoom)
        // and must be the rendered/edited scene when Wedding is active — NOT the
        // local demo in `wedding_patch`.
        let mut m = venue_test_manager();
        m.patch = wedding_stage_patch(1);
        m.wedding_patch = DmxPatch::demo_wedding_room(0, 1); // local demo present
        m.active_venue = VenueKind::WeddingRoom;
        assert_eq!(m.active_patch().venue, Some(VenueKind::WeddingRoom));
        assert_eq!(
            m.active_patch().universe,
            1,
            "renders self.patch (the SaaS stage), not the demo room"
        );
    }

    #[test]
    fn wedding_demo_renders_when_no_saas_wedding() {
        // No SaaS wedding stage in self.patch (a concert stage / none) -> Wedding
        // shows the REACT-local demo room.
        let mut m = venue_test_manager();
        m.patch = DmxPatch {
            universe: 7,
            fixtures: Vec::new(),
            trusses: Vec::new(),
            venue: Some(VenueKind::ConcertStage),
        };
        m.wedding_patch = DmxPatch::demo_wedding_room(0, 1);
        m.active_venue = VenueKind::WeddingRoom;
        assert_eq!(m.active_patch().venue, None, "the demo room carries venue None");
        assert!(
            !m.active_patch().fixtures.is_empty(),
            "renders the seeded demo, not the empty concert patch"
        );
    }

    #[test]
    fn set_active_venue_keeps_saas_wedding_stage() {
        // Switching to Wedding must NOT seed the local demo over a SaaS wedding
        // stage already loaded in self.patch.
        let mut m = venue_test_manager();
        m.patch = wedding_stage_patch(1);
        m.set_active_venue(VenueKind::WeddingRoom);
        assert!(
            m.wedding_patch.fixtures.is_empty(),
            "demo not seeded over the SaaS wedding stage"
        );
        assert_eq!(
            m.active_patch().venue,
            Some(VenueKind::WeddingRoom),
            "renders the SaaS wedding stage"
        );
    }

    #[test]
    fn set_active_venue_seeds_demo_without_saas_wedding() {
        // With no SaaS wedding stage, switching to Wedding seeds the local demo
        // (a screen + four uplights).
        let mut m = venue_test_manager();
        m.set_active_venue(VenueKind::WeddingRoom);
        assert_eq!(
            m.wedding_patch.fixtures.len(),
            5,
            "local demo seeded: screen + 4 uplights"
        );
    }

    #[test]
    fn stage_venue_is_gated_on_selection() {
        let mut m = venue_test_manager();
        m.patch = wedding_stage_patch(1);
        m.selected_stage_id = None;
        assert_eq!(m.stage_venue(), None, "no selection -> no authoritative venue");
        m.selected_stage_id = Some(42);
        assert_eq!(
            m.stage_venue(),
            Some(VenueKind::WeddingRoom),
            "selected stage's authored venue is authoritative"
        );
    }
}

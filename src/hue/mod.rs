//! Philips Hue Entertainment API integration for lighting control
//!
//! Uses the Hue Entertainment API (DTLS/UDP streaming at 25fps) for low-latency
//! direct RGB control of Hue lights. This module consumes the same CueEngine state
//! as the OSC output but maps it to per-light RGB values via the Entertainment API.
//!
//! ## Architecture
//!
//! - **HueManager**: Coordinator — owns bridge state, streaming thread, config
//! - **bridge**: Discovery (mDNS / manual IP), push-link auth, entertainment config fetch
//! - **streaming**: DTLS 1.2 connection to bridge port 2100, binary packet builder
//! - **mappings**: CueEngine → RGB translation (palette, zone, triggers, spatial effects)

pub mod bridge;
pub mod streaming;
pub mod mappings;
pub mod thread;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::lighting::FormFactor;
use crate::emerge::{CueEngine, EnergyZone, Palette, GenreProfile, AutoBlinderPhase};
use crate::emerge::TriggerButton;
use crate::render::{HeroAct, LightingMetrics};
use bridge::ChannelPosition;

// ============================================================================
// TYPES
// ============================================================================

/// Hue Entertainment API protocol version
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HueProtocolVersion {
    /// v1: HTTP REST API + 9-byte per-light packets, max 10 lights, 25fps
    V1,
    /// v2: HTTPS CLIP API + 7-byte per-channel packets with UUID, max 20 channels, 50fps
    V2,
}

impl Default for HueProtocolVersion {
    fn default() -> Self {
        HueProtocolVersion::V2
    }
}

impl HueProtocolVersion {
    pub fn as_str(&self) -> &'static str {
        match self {
            HueProtocolVersion::V1 => "v1 (Legacy)",
            HueProtocolVersion::V2 => "v2 (Recommended)",
        }
    }

    pub fn all() -> &'static [HueProtocolVersion] {
        &[HueProtocolVersion::V2, HueProtocolVersion::V1]
    }

    pub fn max_fps(&self) -> f32 {
        match self {
            HueProtocolVersion::V1 => 25.0,
            HueProtocolVersion::V2 => 50.0,
        }
    }
}

/// Hue mapping mode: determines how CueSnapshot is translated to light colors
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HueMappingMode {
    /// Existing CueEngine-based mapping (palettes, zones, triggers, envelopes)
    Reactive,
    /// HueFlow-style: bass→red, mids→green, highs→blue, spatially distributed
    MultiBand,
}

impl Default for HueMappingMode {
    fn default() -> Self {
        HueMappingMode::Reactive
    }
}

impl HueMappingMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            HueMappingMode::Reactive => "Reactive (Compeller)",
            HueMappingMode::MultiBand => "MultiBand (HueFlow)",
        }
    }

    pub fn all() -> &'static [HueMappingMode] {
        &[HueMappingMode::Reactive, HueMappingMode::MultiBand]
    }
}

/// Role assigned to a Hue light for spatial effects
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LightRole {
    /// Follows palette + zone (base mood lighting)
    Wash,
    /// Responds to triggers + dominant pitch changes (reactive)
    Accent,
    /// Slow, follows palette only (background fill)
    Ambient,
    /// Smooth architectural underglow (steady brightness, kick punch, no breathing)
    Strip,
}

impl LightRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            LightRole::Wash => "Wash",
            LightRole::Accent => "Accent",
            LightRole::Ambient => "Ambient",
            LightRole::Strip => "Strip",
        }
    }

    pub fn all() -> &'static [LightRole] {
        &[LightRole::Wash, LightRole::Accent, LightRole::Ambient, LightRole::Strip]
    }
}

impl Default for LightRole {
    fn default() -> Self {
        LightRole::Wash
    }
}

/// Pairing state for push-link auth flow
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingState {
    /// Not paired, idle
    Idle,
    /// Waiting for user to press bridge button
    WaitingForButton,
    /// Successfully paired (credentials obtained)
    Paired,
    /// Pairing failed
    Failed(String),
}

impl PairingState {
    pub fn as_str(&self) -> &'static str {
        match self {
            PairingState::Idle => "Not Paired",
            PairingState::WaitingForButton => "Press bridge button...",
            PairingState::Paired => "Paired",
            PairingState::Failed(_) => "Failed",
        }
    }
}

/// Light assignment: maps a Hue light to a role
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct HueLightAssignment {
    /// Light/channel ID (v1: resource ID as u16, v2: channel_id as u8 stored in u16)
    pub channel_id: u16,
    /// Human-readable light name
    pub name: String,
    /// Assigned role for lighting behavior
    pub role: LightRole,
    /// Auto-detected form factor from bridge API archetype
    #[serde(default)]
    pub auto_form_factor: FormFactor,
    /// User override for form factor (takes precedence over auto-detected)
    #[serde(default)]
    pub form_factor_override: Option<FormFactor>,
    /// 3D position in entertainment area (v2 only, None for v1)
    #[serde(default)]
    pub position: Option<ChannelPosition>,
}

impl HueLightAssignment {
    /// Effective form factor: user override wins, then auto-detected
    pub fn form_factor(&self) -> FormFactor {
        self.form_factor_override.unwrap_or(self.auto_form_factor)
    }
}

/// Persistent configuration for Hue integration
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct HueConfig {
    pub enabled: bool,
    pub bridge_ip: String,
    pub username: String,
    pub clientkey: String,
    pub entertainment_area_id: String,
    pub light_assignments: Vec<HueLightAssignment>,
    /// Protocol version (v1 legacy or v2 recommended)
    #[serde(default)]
    pub protocol_version: HueProtocolVersion,
    /// v2 DTLS PSK identity (from GET /auth/v1 response header).
    /// Different from username — the bridge returns this per-application ID.
    #[serde(default)]
    pub application_id: String,
    /// Mapping mode: Reactive (CueEngine) or MultiBand (HueFlow-style)
    #[serde(default)]
    pub mapping_mode: HueMappingMode,
}

impl Default for HueConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bridge_ip: String::new(),
            username: String::new(),
            clientkey: String::new(),
            entertainment_area_id: String::new(),
            light_assignments: Vec::new(),
            protocol_version: HueProtocolVersion::default(),
            application_id: String::new(),
            mapping_mode: HueMappingMode::default(),
        }
    }
}

/// Runtime state for UI display
#[derive(Clone, Debug)]
pub struct HueState {
    pub enabled: bool,
    pub connected: bool,
    pub streaming: bool,
    pub connecting: bool,
    pub pairing: PairingState,
    pub bridge_ip: String,
    pub bridge_name: String,
    pub has_credentials: bool,
    pub entertainment_area_id: String,
    /// Available entertainment areas: (id, name)
    pub areas: Vec<(String, String)>,
    /// Available lights in selected area: (light_id, name)
    pub lights: Vec<(u16, String)>,
    /// Current light assignments
    pub light_assignments: Vec<HueLightAssignment>,
    pub error: Option<String>,
    /// Credentials from background pairing thread, awaiting pickup
    pub pending_username: Option<String>,
    pub pending_clientkey: Option<String>,
    /// Current protocol version
    pub protocol_version: HueProtocolVersion,
    /// Current mapping mode
    pub mapping_mode: HueMappingMode,
}

impl Default for HueState {
    fn default() -> Self {
        Self {
            enabled: false,
            connected: false,
            streaming: false,
            connecting: false,
            pairing: PairingState::Idle,
            bridge_ip: String::new(),
            bridge_name: String::new(),
            has_credentials: false,
            entertainment_area_id: String::new(),
            areas: Vec::new(),
            lights: Vec::new(),
            light_assignments: Vec::new(),
            error: None,
            pending_username: None,
            pending_clientkey: None,
            protocol_version: HueProtocolVersion::default(),
            mapping_mode: HueMappingMode::default(),
        }
    }
}

/// Snapshot of CueEngine state for Hue mapping (avoids tight coupling)
#[derive(Clone, Debug)]
pub struct CueSnapshot {
    pub palette: Palette,
    pub zone: EnergyZone,
    pub energy: f32,
    /// Slow-smoothed energy (0-1) — an RMS-style loudness envelope, distinct
    /// from the instantaneous `energy`. Drives oscillator/LFO depth so the
    /// virtual stage breathes with sustained music and goes still in silence.
    pub energy_slow: f32,
    pub tension: f32,
    /// ADAPTIVE tension 0..1: dynamics above the track's own slow baseline.
    /// Wall-of-sound material rails the absolute `tension` at ~1.0 for whole
    /// songs; breathing consumers (the UV layer) read this instead.
    pub tension_dyn: f32,
    pub balance: f32,
    pub stability: f32,
    pub energy_slope: f32,
    pub major_smooth: f32,
    pub dominant_pc: u8,
    /// All-notes chromatic colour blend (the "obey all present notes"
    /// alternative to the single dominant pitch). `all_notes_color` is the
    /// mode flag (true = blend, set when the operator un-checks "Dom" and the
    /// harmonic engine is live); `all_notes_hue` is the dominant-emphasised
    /// energy-weighted chromatic centroid (0–1, same `pc/12` wheel the
    /// dominant uses); `all_notes_sat` is a saturation multiplier (spread
    /// chord → whiter, single note → vivid). Computed at the OSC-thread
    /// snapshot site where the 12-bin chroma is available.
    pub all_notes_color: bool,
    pub all_notes_hue: f32,
    pub all_notes_sat: f32,
    pub dominant_flash_active: bool,
    pub polarity_flash_active: bool,
    pub polarity_quadrant: u8,
    pub strobe_active: bool,
    /// Free-running global-strobe square wave (true = ON/flash phase), advanced
    /// on a WALL clock by the DMX manager so the manual STROBE button flashes
    /// the rig regardless of whether audio is playing. Gated by
    /// `trigger_strobe`; consumed by `apply_global_overrides` (flat) and the
    /// segment path (pixel bars).
    pub strobe_on: bool,
    /// Operator strobe rate (Hz, 1–20) — the DMX manager builds `strobe_on`'s
    /// square wave at this rate. Driven by the UI "Strobe Rate" slider.
    pub strobe_rate_hz: f32,
    /// Master brightness ceiling (0..1) from the EFFECTIVE lighting style
    /// (Banger 1.0 / Energetic 0.8 / Chill 0.5). Caps every emitter so the rig
    /// reads dimmer in calm styles. 1.0 = no cap.
    pub style_brightness_ceiling: f32,
    pub blackout_active: bool,
    pub palette_bias: crate::emerge::PaletteBias,
    // Manual triggers
    pub trigger_strobe: bool,
    pub trigger_blackout: bool,
    pub trigger_blinder: bool,
    /// UV mode — only UV-capable emitters lit (their UV channels driven at
    /// `uv_level`), everything else dark: "blackout, except blacklight".
    /// Manual trigger, hold or latch. Blackout beats it; a blinder hold
    /// interrupts it (UV resumes on release).
    pub trigger_uv: bool,
    pub trigger_fog: bool,
    /// Auto-blinder is active (BlindersHold phase — latch is on)
    pub auto_blinder_active: bool,
    /// Overdrive strobe is active (fast beat-quantized chaos)
    pub overdrive_strobe_active: bool,
    // Musical arc state
    pub excitement: f32,
    pub tension_bank: f32,
    /// Monotonic seconds since engine creation (for breathing sine, etc.)
    pub frame_time: f32,
    /// Seconds between the two most recent engine frames (Instant-derived).
    /// For consumers that integrate (phase accumulators, EMAs): differencing
    /// the f32 `frame_time` quantizes to frame-interval scale after ~3 days of
    /// 24/7 uptime, while this delta stays precise forever. 0.0 = unknown
    /// (first frame / default snapshot) — consumers should treat as "no advance".
    pub dt: f32,
    /// Monotonic engine-frame ordinal (one per CueEngine::process). The exact
    /// frame identity for resolve-once-per-frame guards: f32 `frame_time`
    /// equality misidentifies distinct frames as duplicates once its ULP
    /// exceeds the frame interval (~3 days of uptime). 0 = unknown/default.
    pub frame_seq: u64,
    /// Integrated pixel-bar chase-head phase (0..1) from the CueEngine: rate
    /// rides smoothed energy, advanced by rate*dt. Consumed by `segment_rgbw`
    /// instead of `frame_time * rate(energy)` (which teleports the head on
    /// every energy change once frame_time is large).
    pub chase_head: f32,
    // Envelope values (0-1, smooth exponential decay from trigger)
    pub strobe_env: f32,
    pub dominant_env: f32,
    pub polarity_env: f32,
    // Reactivity system
    /// Master reactivity (0-1): controls brightness floor, drive gain, threshold scaling
    pub reactivity: f32,
    /// Master Global Brightness (0-1): a final dimmer applied to every output —
    /// OSC, DMX, virtual stage. The OSC slider drives this; 1.0 = no change.
    pub global_brightness: f32,
    /// Operator UV accent level (0..1) scaling the GigBAR UV layer
    /// (par ch4/9 + pod ch22); MIDI-learnable. Default 0.35.
    pub uv_level: f32,
    /// Bass pump envelope (0-1): kick-triggered brightness boost, decays ~150ms
    pub bass_pump: f32,
    /// Drop impact level (0-1): sustains high brightness while energy stays high after macro transition
    pub drop_impact: f32,
    /// Current macro phase (Normal, BlindersHold, OverdriveStrobe, Recover)
    pub macro_phase: AutoBlinderPhase,
    // Normalized audio band levels (0-1, from AudioAnalysis, for MultiBand mapping mode)
    /// Bass band energy (0-1, fast-smoothed, independently normalized)
    pub bass: f32,
    /// Mid band energy (0-1, fast-smoothed, independently normalized)
    pub mid: f32,
    /// High band energy (0-1, fast-smoothed, independently normalized)
    pub high: f32,
    /// Kick drum detected this frame
    pub kick: bool,
    /// Onset/transient detected this frame (spectral-flux based, stamped from
    /// LightingMetrics.onset by the OSC thread). Distinct from `kick` — lets an
    /// LFO/cue fire transient-gated bursts (strobe/laser accents) on non-kick hits.
    pub onset: bool,

    // === HARMONIC FIELD STATE (emerge field → autonomous lighting; stamped from
    // LightingMetrics by the OSC thread, same seam as `onset`). ===
    /// Harmonic field rotation phase (0-1, wraps) — field-derived master motion
    /// phase. Drives the mover tilt drift (energy-gated). Neutral 0.0 with no engine.
    pub phase: f32,
    /// Quaternion port angles, NORMALIZED 0-1. Movers aim their pan center at these
    /// lattice anchors; ports 0 & 2 = θ and θ+π (mirror pair). Neutral `[0.5; 4]`
    /// with no engine so the pan formula centers instead of slamming a head to 0.
    pub qc_ports: [f32; 4],
    /// Emerge journey phase (HeroAct) — governs the B1 lighting macro (intensity
    /// + movement depth). Stamped from LightingMetrics; defaults to Birth.
    pub journey_phase: HeroAct,
    /// Seconds in the current journey phase — shapes the macro over time.
    pub journey_phase_time: f32,
    /// Harmonic coherence (0-1) — B2. Gates colour saturation + movement depth
    /// (low = atonal/noisy = desaturated/relaxed; energy still owns brightness).
    /// Stamped from LightingMetrics; neutral `1.0` (= B2 no-op) with no engine.
    pub trust: f32,
    /// Quaternion overlay strength (0-1) — B3. Punches the mover harmonic-snap
    /// accent on lattice lock-in. 0 = Part A baseline accent (no-op).
    pub qc_strength: f32,
    /// Active quaternion port (0-3) — B3. The mover pair tracks this live lattice
    /// anchor; 0 = Part A 0/2 base (no-op).
    pub qc_active_port: u8,
    /// Symmetry order N (12/24) — B4. Mover phase-grouping width (how many movers
    /// move as one phrase unit). Reduces to Part A counter-phase on the 2-mover rig.
    pub symmetry: u8,

    // === TONNETZ STATE (B5; stamped from LightingMetrics.chroma by the OSC thread) ===
    /// Smooth circle-of-fifths hue position (0-1) — harmonic-motion colour drift.
    pub tonnetz_hue: f32,
    /// Tonal confidence (0-1) — centroid concentration; gates the hue drift so an
    /// atonal field doesn't smear colour.
    pub tonnetz_strength: f32,
    /// Voice-leading "surprise" (0-1) — how far the harmonic centre jumped vs its
    /// settled baseline; self-enveloping, drives a colour pop on a harmonic change.
    pub tonnetz_delta: f32,
    /// Chord quality label (B5 all-in): 0 none, 1 major, 2 minor, 3 dominant,
    /// 4 diminished — drives colour temperature. From osc/chord.rs (hysteresis-stable).
    pub chord_quality: u8,
    /// Active neo-Riemannian move (0 none, 1 P, 2 R, 3 L) — drives a chromatic-
    /// mediant hue jolt while its envelope decays.
    pub chord_prl_move: u8,
    /// Decaying P/R/L accent envelope (0-1).
    pub chord_prl_env: f32,
    /// Root pitch class (0-11) of the last committed chord — held through a
    /// no-chord gap. Only meaningful while `chord_quality != 0` (or just after).
    pub chord_root: u8,
    /// Accent envelope (0-1) for ANY chord-to-chord change — the everyday
    /// I→IV / V→I moves the P/R/L envelope ignores. Drives the mover throw and
    /// the colour bloom.
    pub chord_change_env: f32,
    /// Circle-of-fifths distance the root travelled at the last change (0-6):
    /// harmonic distance, so a reaction can be sized to the move.
    pub chord_root_motion: u8,
    /// Side of the circle the root moved to: +1 sharpward, -1 flatward, 0 none.
    pub chord_root_dir: i8,
    /// Harmonic rhythm (0-1): how busy the chord changes are. Sets the pace of
    /// the mover figures.
    pub harmonic_rhythm: f32,
    /// Harmonic position on the circle of fifths as a point in the unit disc
    /// (angle = key area, radius = how committed). Movers lean toward it.
    pub tonnetz_x: f32,
    pub tonnetz_y: f32,
    /// Mover choreography for this frame (figure crossfade + harmonic
    /// steering), stamped by the DMX manager's `MovementDirector`. The default
    /// is the legacy sweep with no steering, so any path without a director
    /// (Hue/WLED/OSC snapshots, tests) moves exactly as before.
    pub movement: crate::emerge::MovementFrame,

    // === COLLATZ STATE (from Mythos engine, available to all lighting outputs) ===
    /// Collatz heat (0-1): accumulated instability
    pub collatz_heat: f32,
    /// Collatz phase (0=contraction, 1=expansion)
    pub collatz_phase: f32,
    /// Collatz intensity (0-1): current sequence magnitude
    pub collatz_intensity: f32,
    /// Collatz breath (0-1): smoothed expansion-contraction cycle
    pub collatz_breath: f32,
    /// Collatz act: 0=Dormant, 1=Rising, 2=Peak, 3=Falling
    pub collatz_act: u8,
    /// COM-499 rig-wide autonomous "look" index: rotated by the show-brain on
    /// polarity / phrase / drop events so the no-cue autopilot cycles colour and
    /// segment effect instead of sitting on one look. Set by DmxManager per frame.
    pub look_index: u32,
    /// Integrated cycle for the autopilot tightening chase (see
    /// `lighting_cue::advance_auto_chase`): advanced per frame at the
    /// tension-shortened period so the head ACCELERATES through a build
    /// instead of teleporting (`fract(abs_beats / varying_period)` jumps —
    /// same bug class `chase_head` fixed). Set by DmxManager per frame; 0.0
    /// on non-DMX snapshot paths, which never evaluate the autopilot chase.
    pub auto_chase_cycle: f64,

    // === SHOW CLOCK (COM-499 Phase B2) ===
    /// Resolved show-clock beat position (Link / Pro DJ / audio-derived),
    /// stamped by the OSC thread so ALL providers (DMX/Hue/WLED/OSC) and the
    /// operator LFO bank read one authoritative beat. Soft-consumer copy; the
    /// DMX path reads the clock in-frame. NEVER read by ShowBrain/TempoEstimator
    /// (would close a clock->snapshot->estimator feedback loop).
    pub clock_beat: f64,
    /// Resolved-clock confidence (0..1), silence-gated upstream so soft
    /// consumers don't lock to a ghost tempo during a quiet passage.
    pub clock_confidence: f32,
    /// Intensity license 0..1 (genre-free percussiveness × spectral fullness,
    /// stamped by the OSC thread from the show brain). Movers crossfade on it
    /// between harmonic-anchor chasing (high) and a slow FOH scan (low) —
    /// atonal walls (post-rock) wander the lattice anchors, so chasing them
    /// reads as "jumping all over" exactly when the music asks for calm.
    pub intensity_license: f32,
    /// Dynamics 0..1 (stamped from the show brain, like the license): how big
    /// the passage is against the loudest sustained passage of the last
    /// several minutes. ~0.3 a breakdown, ~0.7 drums on their own, 1.0
    /// everything in. The rig settles under it — dimmer, how many beams are
    /// lit, how far and how fast the heads move. 1.0 (the Default) is exactly
    /// the rig's behaviour before this existed, so any path that does not
    /// stamp it is unchanged.
    pub dynamics: f32,
}

impl CueSnapshot {
    /// A manual operator override (BLINDER / STROBE / UV) is held. These are
    /// top-level overrides: they must fire WITH OR WITHOUT music and beat
    /// whatever the autonomous show is doing — so the silence gate (auto-dim in
    /// quiet) and the style brightness ceiling are BYPASSED while one is held,
    /// while the operator's Global Brightness master still applies. (BLACKOUT is
    /// the separate frame early-return, already music-independent.)
    pub fn manual_light_override(&self) -> bool {
        self.trigger_blinder || self.trigger_strobe || self.trigger_uv
    }

    /// Build from a CueEngine and manual trigger state
    pub fn from_cue_engine(
        cue: &CueEngine,
        genre: &GenreProfile,
        reactivity: f32,
        global_brightness: f32,
        uv_level: f32,
        trigger_strobe: bool,
        trigger_blackout: bool,
        trigger_blinder: bool,
        trigger_uv: bool,
        trigger_fog: bool,
    ) -> Self {
        let (energy, tension, balance, stability) = cue.smoothed_metrics();
        Self {
            palette: cue.current_palette(),
            zone: cue.current_zone(),
            energy,
            energy_slow: cue.zone_energy(),
            tension,
            tension_dyn: cue.tension_dyn(),
            balance,
            stability,
            energy_slope: cue.energy_slope(),
            major_smooth: cue.major_smooth(),
            dominant_pc: cue.last_dominant_pc(),
            // Defaults; the OSC thread overwrites these at the snapshot site
            // where the 12-bin chroma is in scope (osc/thread.rs).
            all_notes_color: false,
            all_notes_hue: 0.0,
            all_notes_sat: 1.0,
            dominant_flash_active: cue.dominant_flash_active(),
            polarity_flash_active: cue.polarity_flash_active(),
            polarity_quadrant: cue.last_quadrant(),
            strobe_active: cue.strobe_hold_active(),
            // Default off; the DMX manager stamps the wall-clock square wave.
            strobe_on: false,
            // Overwritten by the OSC thread from the operator's rate slider.
            strobe_rate_hz: 10.0,
            style_brightness_ceiling: 1.0,
            blackout_active: cue.blackout_active(),
            palette_bias: genre.palette_bias,
            trigger_strobe,
            trigger_blackout,
            trigger_blinder: trigger_blinder || cue.auto_blinder_active(),
            trigger_uv,
            trigger_fog,
            auto_blinder_active: cue.auto_blinder_active(),
            overdrive_strobe_active: cue.overdrive_strobe_active(),
            excitement: cue.excitement(),
            tension_bank: cue.tension_bank(),
            frame_time: cue.frame_time(),
            dt: cue.last_dt(),
            frame_seq: cue.frame_seq(),
            chase_head: cue.chase_head(),
            strobe_env: cue.strobe_env(),
            dominant_env: cue.dominant_env(),
            polarity_env: cue.polarity_env(),
            reactivity,
            global_brightness,
            uv_level,
            bass_pump: cue.bass_pump(),
            drop_impact: cue.drop_impact_level(),
            macro_phase: cue.auto_blinder_phase(),
            // Normalized audio band levels default to 0; populated by OSC thread from AudioAnalysis
            bass: 0.0,
            mid: 0.0,
            high: 0.0,
            kick: false,
            onset: false,
            // Harmonic field state default neutral; stamped by the OSC thread from LightingMetrics
            phase: 0.0,
            qc_ports: [0.5; 4],
            journey_phase: HeroAct::Birth,
            journey_phase_time: 0.0,
            trust: 1.0,
            qc_strength: 0.0,
            qc_active_port: 0,
            symmetry: 12,
            tonnetz_hue: 0.0,
            tonnetz_strength: 0.0,
            tonnetz_delta: 0.0,
            chord_quality: 0,
            chord_prl_move: 0,
            chord_prl_env: 0.0,
            chord_root: 0,
            chord_change_env: 0.0,
            chord_root_motion: 0,
            chord_root_dir: 0,
            harmonic_rhythm: 0.0,
            tonnetz_x: 0.0,
            tonnetz_y: 0.0,
            movement: crate::emerge::MovementFrame::default(),
            // Collatz fields default to 0; populated by OSC thread from LightingMetrics
            collatz_heat: 0.0,
            collatz_phase: 0.0,
            collatz_intensity: 0.0,
            collatz_breath: 0.0,
            collatz_act: 0,
            look_index: 0,
            auto_chase_cycle: 0.0,
            // Show clock default 0; stamped by the OSC thread (COM-499 Phase B2)
            clock_beat: 0.0,
            clock_confidence: 0.0,
            // Stamped by the OSC thread; 1.0 = full license (classic behavior).
            intensity_license: 1.0,
            dynamics: 1.0,
        }
    }
}

impl Default for CueSnapshot {
    fn default() -> Self {
        Self {
            palette: Palette::CoolA,
            zone: EnergyZone::Silent,
            energy: 0.0,
            energy_slow: 0.0,
            tension: 0.0,
            tension_dyn: 0.0,
            balance: 0.5,
            stability: 0.0,
            energy_slope: 0.0,
            major_smooth: 0.5,
            dominant_pc: 0,
            all_notes_color: false,
            all_notes_hue: 0.0,
            all_notes_sat: 1.0,
            dominant_flash_active: false,
            polarity_flash_active: false,
            polarity_quadrant: 0,
            strobe_active: false,
            strobe_on: false,
            strobe_rate_hz: 10.0,
            style_brightness_ceiling: 1.0,
            blackout_active: false,
            palette_bias: crate::emerge::PaletteBias::default(),
            trigger_strobe: false,
            trigger_blackout: false,
            trigger_blinder: false,
            trigger_uv: false,
            trigger_fog: false,
            auto_blinder_active: false,
            overdrive_strobe_active: false,
            excitement: 0.0,
            tension_bank: 0.0,
            frame_time: 0.0,
            dt: 0.0,
            frame_seq: 0,
            chase_head: 0.0,
            strobe_env: 0.0,
            dominant_env: 0.0,
            polarity_env: 0.0,
            reactivity: 0.5,
            global_brightness: 1.0,
            uv_level: 0.35,
            bass_pump: 0.0,
            drop_impact: 0.0,
            macro_phase: AutoBlinderPhase::Normal,
            bass: 0.0,
            mid: 0.0,
            high: 0.0,
            kick: false,
            onset: false,
            phase: 0.0,
            qc_ports: [0.5; 4],
            journey_phase: HeroAct::Birth,
            journey_phase_time: 0.0,
            trust: 1.0,
            qc_strength: 0.0,
            qc_active_port: 0,
            symmetry: 12,
            tonnetz_hue: 0.0,
            tonnetz_strength: 0.0,
            tonnetz_delta: 0.0,
            chord_quality: 0,
            chord_prl_move: 0,
            chord_prl_env: 0.0,
            chord_root: 0,
            chord_change_env: 0.0,
            chord_root_motion: 0,
            chord_root_dir: 0,
            harmonic_rhythm: 0.0,
            tonnetz_x: 0.0,
            tonnetz_y: 0.0,
            movement: crate::emerge::MovementFrame::default(),
            collatz_heat: 0.0,
            collatz_phase: 0.0,
            collatz_intensity: 0.0,
            collatz_breath: 0.0,
            collatz_act: 0,
            look_index: 0,
            auto_chase_cycle: 0.0,
            clock_beat: 0.0,
            clock_confidence: 0.0,
            intensity_license: 1.0,
            dynamics: 1.0,
        }
    }
}

// ============================================================================
// HUE MANAGER
// ============================================================================

/// Result from background connect thread
struct ConnectResult {
    streamer: Option<streaming::HueStreamer>,
    error: Option<String>,
    /// v2: application_id fetched during connect (to cache/persist)
    application_id: Option<String>,
}

/// Coordinates Hue bridge connection, DTLS streaming, and CueEngine → RGB mapping
pub struct HueManager {
    config: HueConfig,
    state: Arc<Mutex<HueState>>,
    streamer: Option<streaming::HueStreamer>,
    /// Pending streamer from background connect thread
    pending_connect: Arc<Mutex<Option<ConnectResult>>>,
    last_frame_time: Instant,
    /// Last time we actually transmitted a packet to the bridge (for keepalive timeout safety)
    last_packet_time: Instant,
    /// Last transmitted (smoothed) RGB values, for epsilon-based send gating
    last_sent_rgb: Vec<(u16, f32, f32, f32)>,
    /// Last sent packet for keepalive resend
    last_packet: Option<Vec<u8>>,
    /// Sequence number for entertainment packets (wraps at 255)
    sequence: u8,
    /// Per-light current RGB for smooth interpolation (channel_id, r, g, b)
    current_rgb: Vec<(u16, f32, f32, f32)>,
    /// Frame counter for throttled diagnostic logging
    diag_frame: u64,
}

impl HueManager {
    /// Bridge times out entertainment if no packet for ~10s; resend at 2s
    const KEEPALIVE_INTERVAL: f32 = 2.0;

    /// Get max FPS based on protocol version
    fn max_fps(&self) -> f32 {
        self.config.protocol_version.max_fps()
    }

    /// Get min frame interval based on protocol version
    fn min_frame_interval(&self) -> f32 {
        1.0 / self.max_fps()
    }

    pub fn new() -> Self {
        Self {
            config: HueConfig::default(),
            state: Arc::new(Mutex::new(HueState::default())),
            streamer: None,
            pending_connect: Arc::new(Mutex::new(None)),
            last_frame_time: Instant::now(),
            last_packet_time: Instant::now(),
            last_sent_rgb: Vec::new(),
            last_packet: None,
            sequence: 0,
            current_rgb: Vec::new(),
            diag_frame: 0,
        }
    }

    pub fn config(&self) -> &HueConfig {
        &self.config
    }

    pub fn set_config(&mut self, config: HueConfig) {
        self.config = config;
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get current state snapshot for UI (merges runtime state with config)
    pub fn state(&self) -> HueState {
        let mut state = self.state.lock().unwrap().clone();
        // Overlay config fields so UI can display them
        state.enabled = self.config.enabled;
        state.bridge_ip = self.config.bridge_ip.clone();
        state.has_credentials = !self.config.username.is_empty() && !self.config.clientkey.is_empty();
        state.entertainment_area_id = self.config.entertainment_area_id.clone();
        // Config's light_assignments is the source of truth (has user's role choices)
        if !self.config.light_assignments.is_empty() {
            state.light_assignments = self.config.light_assignments.clone();
            // Also populate lights list from config if state is empty (e.g. after restart)
            if state.lights.is_empty() {
                state.lights = self.config.light_assignments.iter()
                    .map(|a| (a.channel_id, a.name.clone()))
                    .collect();
            }
        }
        state.protocol_version = self.config.protocol_version;
        state.mapping_mode = self.config.mapping_mode;
        // If we have persisted credentials, show as paired
        if state.has_credentials && state.pairing == PairingState::Idle {
            state.pairing = PairingState::Paired;
        }
        state
    }

    /// Set enabled state
    pub fn set_enabled(&mut self, enabled: bool) {
        self.config.enabled = enabled;
        if !enabled {
            self.disconnect();
        }
        log::info!("[Hue] {}", if enabled { "Enabled" } else { "Disabled" });
    }

    /// Set bridge IP and reset connection
    pub fn set_bridge_ip(&mut self, ip: String) {
        if self.config.bridge_ip != ip {
            self.disconnect();
            self.config.bridge_ip = ip.clone();
            log::info!("[Hue] Bridge IP set to: {}", ip);
        }
    }

    /// Set entertainment area and update light list
    pub fn set_entertainment_area(&mut self, area_id: String) {
        self.config.entertainment_area_id = area_id.clone();
        // Disconnect streaming if active (need to restart with new area)
        if self.streamer.is_some() {
            self.disconnect();
        }
        log::info!("[Hue] Entertainment area set to: {}", area_id);
    }

    /// Set role for a specific light
    pub fn set_light_role(&mut self, channel_id: u16, role: LightRole) {
        if let Some(assignment) = self.config.light_assignments.iter_mut()
            .find(|a| a.channel_id == channel_id) {
            assignment.role = role;
        }
    }

    /// Set or clear form factor override for a specific light
    pub fn set_light_form_factor(&mut self, channel_id: u16, form_factor: Option<FormFactor>) {
        if let Some(assignment) = self.config.light_assignments.iter_mut()
            .find(|a| a.channel_id == channel_id) {
            assignment.form_factor_override = form_factor;
        }
    }

    /// Set protocol version (v1 or v2). Disconnects if streaming, clears area ID
    /// (v1 group IDs and v2 UUIDs are incompatible), and triggers area re-fetch.
    pub fn set_protocol_version(&mut self, version: HueProtocolVersion) {
        if self.config.protocol_version != version {
            log::info!("[Hue] Protocol version changed to {:?} — clearing area & lights", version);
            self.disconnect();
            self.config.protocol_version = version;
            // v1 group IDs (e.g. "200") are not v2 UUIDs — must re-select
            self.config.entertainment_area_id.clear();
            self.config.light_assignments.clear();
            {
                let mut state = self.state.lock().unwrap();
                state.areas.clear();
                state.lights.clear();
                state.light_assignments.clear();
                state.entertainment_area_id.clear();
            }
            // Auto-fetch areas with new protocol
            self.fetch_areas();
        }
    }

    /// Set mapping mode (Reactive or MultiBand)
    pub fn set_mapping_mode(&mut self, mode: HueMappingMode) {
        if self.config.mapping_mode != mode {
            self.config.mapping_mode = mode;
            log::info!("[Hue] Mapping mode set to {:?}", mode);
        }
    }

    /// Start push-link pairing flow (async, updates state)
    pub fn start_pairing(&self) {
        let ip = self.config.bridge_ip.clone();
        if ip.is_empty() {
            let mut state = self.state.lock().unwrap();
            state.pairing = PairingState::Failed("No bridge IP set".to_string());
            state.error = Some("No bridge IP set".to_string());
            return;
        }

        {
            let mut state = self.state.lock().unwrap();
            state.pairing = PairingState::WaitingForButton;
            state.error = None;
        }

        let state = Arc::clone(&self.state);
        std::thread::spawn(move || {
            log::info!("[Hue] Starting push-link pairing with bridge at {}", ip);
            match bridge::push_link_pair(&ip) {
                Ok((username, clientkey)) => {
                    let mut s = state.lock().unwrap();
                    s.pairing = PairingState::Paired;
                    s.error = None;
                    s.pending_username = Some(username.clone());
                    s.pending_clientkey = Some(clientkey.clone());
                    log::info!("[Hue] Pairing successful! username={}", username);
                }
                Err(e) => {
                    let mut s = state.lock().unwrap();
                    s.pairing = PairingState::Failed(e.clone());
                    s.error = Some(e.clone());
                    log::error!("[Hue] Pairing failed: {}", e);
                }
            }
        });
    }

    /// Fetch entertainment areas from bridge (dispatches v1 or v2 based on protocol)
    pub fn fetch_areas(&self) {
        let ip = self.config.bridge_ip.clone();
        let username = self.config.username.clone();
        let protocol = self.config.protocol_version;
        if ip.is_empty() || username.is_empty() {
            return;
        }

        let state = Arc::clone(&self.state);
        std::thread::spawn(move || {
            log::info!("[Hue] Fetching entertainment areas from {} (protocol={:?})", ip, protocol);
            let result = match protocol {
                HueProtocolVersion::V2 => bridge::fetch_entertainment_areas_v2(&ip, &username),
                HueProtocolVersion::V1 => bridge::fetch_entertainment_areas(&ip, &username),
            };
            match result {
                Ok(areas) => {
                    let mut s = state.lock().unwrap();
                    s.areas = areas;
                    s.error = None;
                    log::info!("[Hue] Found {} entertainment areas", s.areas.len());
                }
                Err(e) => {
                    let mut s = state.lock().unwrap();
                    s.error = Some(e.clone());
                    log::error!("[Hue] Failed to fetch areas: {}", e);
                }
            }
        });
    }

    /// Fetch lights for the currently selected entertainment area (dispatches v1 or v2)
    pub fn fetch_lights(&self) {
        let ip = self.config.bridge_ip.clone();
        let username = self.config.username.clone();
        let area_id = self.config.entertainment_area_id.clone();
        let protocol = self.config.protocol_version;
        if ip.is_empty() || username.is_empty() || area_id.is_empty() {
            return;
        }

        let state = Arc::clone(&self.state);
        std::thread::spawn(move || {
            log::info!("[Hue] Fetching lights for area {} (protocol={:?})", area_id, protocol);
            match protocol {
                HueProtocolVersion::V2 => {
                    match bridge::fetch_area_lights_v2(&ip, &username, &area_id) {
                        Ok(channel_infos) => {
                            let mut s = state.lock().unwrap();
                            s.lights = channel_infos.iter().map(|info| (info.channel_id as u16, info.name.clone())).collect();
                            s.light_assignments = channel_infos.iter().map(|info| {
                                let auto_ff = crate::lighting::form_factor_from_hue_archetype(&info.archetype);
                                log::info!("[Hue/v2] Channel {} '{}' archetype='{}' pos=({:.2},{:.2},{:.2}) -> {:?}",
                                    info.channel_id, info.name, info.archetype,
                                    info.position.x, info.position.y, info.position.z, auto_ff);
                                HueLightAssignment {
                                    channel_id: info.channel_id as u16,
                                    name: info.name.clone(),
                                    role: LightRole::Wash,
                                    auto_form_factor: auto_ff,
                                    form_factor_override: None,
                                    position: Some(info.position),
                                }
                            }).collect();
                            s.error = None;
                            log::info!("[Hue/v2] Found {} channels in area", s.lights.len());
                        }
                        Err(e) => {
                            let mut s = state.lock().unwrap();
                            s.error = Some(e.clone());
                            log::error!("[Hue/v2] Failed to fetch channels: {}", e);
                        }
                    }
                }
                HueProtocolVersion::V1 => {
                    match bridge::fetch_area_lights(&ip, &username, &area_id) {
                        Ok(light_infos) => {
                            let mut s = state.lock().unwrap();
                            s.lights = light_infos.iter().map(|info| (info.id, info.name.clone())).collect();
                            s.light_assignments = light_infos.iter().map(|info| {
                                let auto_ff = crate::lighting::form_factor_from_hue_archetype(&info.archetype);
                                log::info!("[Hue] Light {} '{}' archetype='{}' -> {:?}",
                                    info.id, info.name, info.archetype, auto_ff);
                                HueLightAssignment {
                                    channel_id: info.id,
                                    name: info.name.clone(),
                                    role: LightRole::Wash,
                                    auto_form_factor: auto_ff,
                                    form_factor_override: None,
                                    position: None,
                                }
                            }).collect();
                            s.error = None;
                            log::info!("[Hue] Found {} lights in area", s.lights.len());
                        }
                        Err(e) => {
                            let mut s = state.lock().unwrap();
                            s.error = Some(e.clone());
                            log::error!("[Hue] Failed to fetch lights: {}", e);
                        }
                    }
                }
            }
        });
    }

    /// Connect DTLS streaming to bridge (runs in background thread)
    pub fn connect(&mut self) {
        if self.streamer.is_some() {
            return;
        }
        // Check if already connecting
        {
            let state = self.state.lock().unwrap();
            if state.connecting {
                return;
            }
        }

        let ip = self.config.bridge_ip.clone();
        let username = self.config.username.clone();
        let clientkey = self.config.clientkey.clone();
        let area_id = self.config.entertainment_area_id.clone();

        if ip.is_empty() || username.is_empty() || clientkey.is_empty() || area_id.is_empty() {
            log::warn!("[Hue] Connect failed — ip={} username={} clientkey={} area_id={}",
                if ip.is_empty() { "(empty)" } else { "(set)" },
                if username.is_empty() { "(empty)" } else { "(set)" },
                if clientkey.is_empty() { "(empty)" } else { "(set)" },
                if area_id.is_empty() { "(empty)" } else { "(set)" },
            );
            let mut state = self.state.lock().unwrap();
            state.error = Some("Missing bridge credentials or area selection".to_string());
            return;
        }

        // Set connecting state
        {
            let mut state = self.state.lock().unwrap();
            state.connecting = true;
            state.error = None;
        }

        let pending = Arc::clone(&self.pending_connect);
        let state = Arc::clone(&self.state);
        let protocol = self.config.protocol_version;
        let application_id = self.config.application_id.clone();

        std::thread::spawn(move || {
            // Step 1: Deactivate any stale streaming session
            log::info!("[Hue] Deactivating stale entertainment session (protocol={:?})...", protocol);
            match protocol {
                HueProtocolVersion::V2 => { let _ = bridge::deactivate_entertainment_v2(&ip, &username, &area_id); }
                HueProtocolVersion::V1 => { let _ = bridge::deactivate_entertainment(&ip, &username, &area_id); }
            }
            std::thread::sleep(std::time::Duration::from_millis(500));

            // Step 1.5 (v2 only): Fetch application_id if not cached
            let psk_identity = match protocol {
                HueProtocolVersion::V2 => {
                    let app_id = if application_id.is_empty() {
                        log::info!("[Hue/v2] No cached application_id — fetching from /auth/v1...");
                        match bridge::fetch_application_id(&ip, &username) {
                            Ok(id) => {
                                log::info!("[Hue/v2] Fetched application_id: {}", id);
                                id
                            }
                            Err(e) => {
                                log::error!("[Hue/v2] Failed to fetch application_id: {}", e);
                                let mut s = state.lock().unwrap();
                                s.connecting = false;
                                s.error = Some(format!("Failed to fetch application_id: {}", e));
                                return;
                            }
                        }
                    } else {
                        log::info!("[Hue/v2] Using cached application_id: {}", application_id);
                        application_id.clone()
                    };
                    app_id
                }
                HueProtocolVersion::V1 => username.clone(),
            };

            // Step 2: Activate entertainment mode via REST/CLIP API
            log::info!("[Hue] Activating entertainment mode for area {} (protocol={:?})...", area_id, protocol);
            let activate_result = match protocol {
                HueProtocolVersion::V2 => bridge::activate_entertainment_v2(&ip, &username, &area_id),
                HueProtocolVersion::V1 => bridge::activate_entertainment(&ip, &username, &area_id),
            };
            if let Err(e) = activate_result {
                log::error!("[Hue] Failed to activate entertainment: {}", e);
                let mut s = state.lock().unwrap();
                s.connecting = false;
                s.error = Some(format!("Failed to activate entertainment: {}", e));
                return;
            }

            // Step 3: Probe port 2100 to check if entertainment mode is actually listening
            log::info!("[Hue] Waiting 3s for bridge to open DTLS port...");
            std::thread::sleep(std::time::Duration::from_secs(3));

            // Quick UDP probe: send a dummy byte and see if we get "Connection refused"
            match std::net::UdpSocket::bind("0.0.0.0:0") {
                Ok(probe) => {
                    let _ = probe.connect(format!("{}:2100", ip));
                    let _ = probe.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                    match probe.send(&[0u8]) {
                        Ok(_) => {
                            // Check for ICMP port unreachable (shows up on next recv)
                            let mut buf = [0u8; 64];
                            match probe.recv(&mut buf) {
                                Ok(n) => log::info!("[Hue] Port 2100 probe: got {} bytes response", n),
                                Err(ref e) if e.raw_os_error() == Some(61) => {
                                    log::error!("[Hue] Port 2100 probe: Connection refused — bridge is NOT listening. Entertainment activation may have failed.");
                                }
                                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                                    log::info!("[Hue] Port 2100 probe: no response (timeout) — port appears open (DTLS expected)");
                                }
                                Err(e) => log::info!("[Hue] Port 2100 probe: recv error: {} (kind={:?})", e, e.kind()),
                            }
                        }
                        Err(e) => log::warn!("[Hue] Port 2100 probe: send failed: {}", e),
                    }
                }
                Err(e) => log::warn!("[Hue] Port 2100 probe: bind failed: {}", e),
            }

            // Step 4: DTLS handshake with retries
            //   PSK identity: v2 = application_id, v1 = username
            let delays_ms = [1000, 2000, 2000];
            let mut last_err = String::new();

            for (attempt, delay) in delays_ms.iter().enumerate() {
                if attempt > 0 {
                    log::info!("[Hue] Waiting {}ms before retry...", delay);
                    std::thread::sleep(std::time::Duration::from_millis(*delay));
                }

                log::info!("[Hue] DTLS handshake attempt {}/{} (identity={})...", attempt + 1, delays_ms.len(), psk_identity);
                match streaming::HueStreamer::new(&ip, &psk_identity, &clientkey) {
                    Ok(streamer) => {
                        log::info!("[Hue] DTLS streaming connected to {}", ip);
                        let mut p = pending.lock().unwrap();
                        *p = Some(ConnectResult {
                            streamer: Some(streamer),
                            error: None,
                            application_id: if protocol == HueProtocolVersion::V2 {
                                Some(psk_identity.clone())
                            } else {
                                None
                            },
                        });
                        return; // Success
                    }
                    Err(e) => {
                        log::warn!("[Hue] DTLS attempt {} failed: {}", attempt + 1, e);
                        last_err = e;
                    }
                }
            }

            // All attempts failed
            log::error!("[Hue] DTLS connection failed after {} attempts: {}", delays_ms.len(), last_err);
            let mut s = state.lock().unwrap();
            s.connecting = false;
            s.error = Some(format!("DTLS connection failed: {}", last_err));
        });
    }

    /// Pick up streamer from background connect thread.
    /// Returns true if a new connection was established.
    /// Call this each frame from the main loop.
    pub fn apply_pending_connect(&mut self) -> bool {
        let result = {
            let mut pending = self.pending_connect.lock().unwrap();
            pending.take()
        };

        if let Some(result) = result {
            let mut state = self.state.lock().unwrap();
            state.connecting = false;

            if let Some(streamer) = result.streamer {
                self.streamer = Some(streamer);
                state.connected = true;
                state.streaming = true;
                state.error = None;
                drop(state);
                // Cache the application_id from v2 auth (for persistence)
                if let Some(app_id) = result.application_id {
                    if self.config.application_id != app_id {
                        log::info!("[Hue/v2] Caching application_id: {}", app_id);
                        self.config.application_id = app_id;
                    }
                }
                // Refresh light list from bridge (detect changes)
                self.fetch_lights();
                // Send initial all-black frame
                self.send_black_frame();
                return true;
            }
            if let Some(error) = result.error {
                state.error = Some(error);
            }
        }
        false
    }

    /// Send an all-black frame (used on initial connect)
    fn send_black_frame(&mut self) {
        if let Some(ref mut streamer) = self.streamer {
            if self.config.light_assignments.is_empty() { return; }
            self.sequence = self.sequence.wrapping_add(1);
            let packet = match self.config.protocol_version {
                HueProtocolVersion::V2 => {
                    let channels: Vec<streaming::HueChannelColor> = self.config.light_assignments.iter()
                        .map(|a| streaming::HueChannelColor::from_f32(a.channel_id as u8, 0.0, 0.0, 0.0))
                        .collect();
                    streaming::build_entertainment_packet_v2(
                        self.sequence,
                        &self.config.entertainment_area_id,
                        &channels,
                    )
                }
                HueProtocolVersion::V1 => {
                    let lights: Vec<streaming::HueLightColor> = self.config.light_assignments.iter()
                        .map(|a| streaming::HueLightColor::from_f32(a.channel_id, 0.0, 0.0, 0.0))
                        .collect();
                    streaming::build_entertainment_packet(self.sequence, &lights)
                }
            };
            streamer.send(&packet);
        }
    }

    /// Disconnect streaming
    pub fn disconnect(&mut self) {
        if let Some(_streamer) = self.streamer.take() {
            // Deactivate entertainment mode on bridge
            if !self.config.bridge_ip.is_empty() && !self.config.username.is_empty() && !self.config.entertainment_area_id.is_empty() {
                match self.config.protocol_version {
                    HueProtocolVersion::V2 => {
                        let _ = bridge::deactivate_entertainment_v2(
                            &self.config.bridge_ip,
                            &self.config.username,
                            &self.config.entertainment_area_id,
                        );
                    }
                    HueProtocolVersion::V1 => {
                        let _ = bridge::deactivate_entertainment(
                            &self.config.bridge_ip,
                            &self.config.username,
                            &self.config.entertainment_area_id,
                        );
                    }
                }
            }
            // _streamer dropped here → DtlsConnection::drop calls SSL_shutdown
        }
        self.last_packet = None;
        self.last_packet_time = Instant::now();
        self.last_sent_rgb.clear();
        let mut state = self.state.lock().unwrap();
        state.connected = false;
        state.streaming = false;
        state.connecting = false;
        log::info!("[Hue] Disconnected");
    }

    /// Update Hue lights with current CueEngine state (called each frame)
    ///
    /// Throttles to protocol max FPS (25 for v1, 50 for v2).
    /// Includes keepalive: resends last frame if idle > 2 seconds.
    /// Applies per-light RGB interpolation with role-dependent speeds.
    pub fn update(&mut self, snapshot: &CueSnapshot) {
        if !self.config.enabled {
            return;
        }

        if self.streamer.is_none() {
            return;
        }

        let elapsed_tick = self.last_frame_time.elapsed().as_secs_f32();
        let elapsed_tx = self.last_packet_time.elapsed().as_secs_f32();

        // Keepalive: resend last packet if idle > 2 seconds
        if elapsed_tx > Self::KEEPALIVE_INTERVAL {
            if let Some(ref packet) = self.last_packet.clone() {
                self.streamer.as_mut().unwrap().send(packet);
                self.last_packet_time = Instant::now();
                return;
            }
        }

        // Throttle to max FPS (25 for v1, 50 for v2)
        if elapsed_tick < self.min_frame_interval() {
            return;
        }
        self.last_frame_time = Instant::now();

        // Config is the source of truth for assignments (synced from state each frame)
        let assignments = &self.config.light_assignments;
        if assignments.is_empty() {
            return;
        }

        // Compute target RGB per light from mappings (includes per-stage diagnostic logging)
        self.diag_frame += 1;
        let dt = elapsed_tick;

        // Buildup detection is now unified in CueEngine; Hue reads macro_phase from snapshot.
        let target_colors = match self.config.mapping_mode {
            HueMappingMode::Reactive => mappings::map_snapshot_to_lights(snapshot, assignments, self.diag_frame),
            HueMappingMode::MultiBand => mappings::map_multiband_to_lights(snapshot, assignments),
        };

        // Manual emergency triggers and macro phases bypass interpolation.
        // Buildup (BlindersHold) needs instant response for 8Hz strobe.
        // Pump (OverdriveStrobe) needs instant response for bass-pumped brightness.
        let instant = snapshot.trigger_blackout
            || snapshot.trigger_blinder
            || snapshot.trigger_strobe
            || snapshot.trigger_uv
            || snapshot.blackout_active
            || snapshot.macro_phase != AutoBlinderPhase::Normal;

        // Ensure current_rgb is sized correctly; seed from first target (not black)
        if self.current_rgb.len() != assignments.len() {
            self.current_rgb = target_colors.iter()
                .enumerate()
                .map(|(idx, tc)| {
                    let (r, g, b) = tc.as_f32();
                    (assignments[idx].channel_id, r, g, b)
                })
                .collect();
            self.last_sent_rgb = self.current_rgb.clone();
            self.last_packet_time = Instant::now();
        }

        // dt-based interpolation: alpha = 1 - exp(-dt / tau)
        // This gives frame-rate-independent smoothing.
        let diag = self.diag_frame % 50 == 1;

        // Interpolate current toward target with role-dependent time constants.
        // Only manual emergency triggers bypass interpolation.
        let light_colors: Vec<streaming::HueLightColor> = target_colors.iter()
            .enumerate()
            .map(|(idx, target)| {
                let (tr, tg, tb) = target.as_f32();

                if instant {
                    self.current_rgb[idx] = (assignments[idx].channel_id, tr, tg, tb);
                    return streaming::HueLightColor::from_f32(assignments[idx].channel_id, tr, tg, tb);
                }

                let (_, cr, cg, cb) = self.current_rgb[idx];

                // Tau (time constant in seconds) per role.
                // MultiBand mode uses much shorter tau — it's raw audio-driven
                // and needs punchy response. Reactive mode uses asymmetric tau:
                // fast attack (respond to energy) but slow decay (ease off gently)
                // so Hue bulbs don't look like they're switching off abruptly.
                let (tau_attack, tau_decay) = match self.config.mapping_mode {
                    HueMappingMode::MultiBand => (0.06, 0.06), // 60ms symmetric — audio-reactive
                    HueMappingMode::Reactive => match assignments[idx].role {
                        LightRole::Wash    => (0.08, 0.40), // Punchy attack, smooth fade
                        LightRole::Accent  => (0.06, 0.30), // Near-instant attack, quick fade
                        LightRole::Strip   => (0.07, 0.35), // Fast attack, moderate fade
                        LightRole::Ambient => (0.12, 0.60), // Slightly softer, still responsive
                    },
                };

                // Per-channel asymmetric interpolation: pick tau based on direction
                let compute_channel = |current: f32, target: f32| -> f32 {
                    let tau = if target > current { tau_attack } else { tau_decay };
                    let alpha = 1.0 - (-dt / tau).exp();
                    current + (target - current) * alpha
                };
                let nr = compute_channel(cr, tr);
                let ng = compute_channel(cg, tg);
                let nb = compute_channel(cb, tb);

                self.current_rgb[idx] = (assignments[idx].channel_id, nr, ng, nb);

                // Log interpolation lag for first light (target vs actual)
                if diag && idx == 0 {
                    let delta = ((tr - nr).abs() + (tg - ng).abs() + (tb - nb).abs()) / 3.0;
                    log::info!(
                        "[Hue] interp: dt={:.3}s tau_atk={:.2} tau_dec={:.2} target=({:.3},{:.3},{:.3}) actual=({:.3},{:.3},{:.3}) avg_delta={:.4}",
                        dt, tau_attack, tau_decay, tr, tg, tb, nr, ng, nb, delta,
                    );
                }

                streaming::HueLightColor::from_f32(assignments[idx].channel_id, nr, ng, nb)
            })
            .collect();

        
// Epsilon-based send gating:
// Hue bridges/bulbs often look worse when spammed with tiny deltas.
// We still guarantee periodic refresh so slow motion (breathing) remains smooth.
let mut avg_delta = 1.0_f32;
if self.last_sent_rgb.len() == self.current_rgb.len() && !self.current_rgb.is_empty() {
    let mut sum = 0.0_f32;
    for (i, (_, r, g, b)) in self.current_rgb.iter().enumerate() {
        let (_, lr, lg, lb) = self.last_sent_rgb[i];
        sum += (r - lr).abs() + (g - lg).abs() + (b - lb).abs();
    }
    avg_delta = sum / (self.current_rgb.len() as f32 * 3.0);
}

// If changes are extremely small and we sent recently, skip this frame.
// Force a send at least every 0.5s to keep slow breathing visible.
// MultiBand mode uses a tighter epsilon — it's raw audio-driven and needs
// to push every frame to keep up with rapid band changes.
let epsilon = match self.config.mapping_mode {
    HueMappingMode::MultiBand => 0.001,
    HueMappingMode::Reactive => 0.002, // Tighter — don't drop visible changes
};
let max_idle = match self.config.mapping_mode {
    HueMappingMode::MultiBand => 0.08,
    HueMappingMode::Reactive => 0.10, // Force send every 100ms for breathing
};
if !instant && avg_delta < epsilon && elapsed_tx < max_idle {
    return;
}

// Build and send entertainment packet (v1 or v2)
        self.sequence = self.sequence.wrapping_add(1);
        let packet = match self.config.protocol_version {
            HueProtocolVersion::V2 => {
                let channels: Vec<streaming::HueChannelColor> = light_colors.iter()
                    .map(|lc| streaming::HueChannelColor {
                        channel_id: lc.light_id as u8,
                        r: lc.r,
                        g: lc.g,
                        b: lc.b,
                    })
                    .collect();
                streaming::build_entertainment_packet_v2(
                    self.sequence,
                    &self.config.entertainment_area_id,
                    &channels,
                )
            }
            HueProtocolVersion::V1 => {
                streaming::build_entertainment_packet(self.sequence, &light_colors)
            }
        };
        self.streamer.as_mut().unwrap().send(&packet);
        self.last_packet_time = Instant::now();
        self.last_sent_rgb = self.current_rgb.clone();
        self.last_packet = Some(packet);
    }

    /// Store credentials from successful pairing (called from UI action handler)
    pub fn store_credentials(&mut self, username: String, clientkey: String) {
        self.config.username = username;
        self.config.clientkey = clientkey;
    }

    /// Sync light assignments from background state to config.
    /// Preserves user's role selections for existing lights; new lights get default role.
    /// Returns true if config was updated (caller should persist to db).
    /// Call this each frame from the main loop.
    pub fn sync_light_assignments_from_state(&mut self) -> bool {
        let state = self.state.lock().unwrap();
        if state.light_assignments.is_empty() {
            return false;
        }
        // Check if state and config have the same lights
        let state_ids: Vec<u16> = state.light_assignments.iter().map(|a| a.channel_id).collect();
        let config_ids: Vec<u16> = self.config.light_assignments.iter().map(|a| a.channel_id).collect();
        if state_ids == config_ids {
            return false; // Already in sync
        }
        // Merge: keep existing roles from config, add new lights from state
        let state_assignments = state.light_assignments.clone();
        drop(state);

        let mut merged = Vec::new();
        for sa in &state_assignments {
            if let Some(existing) = self.config.light_assignments.iter().find(|c| c.channel_id == sa.channel_id) {
                // Preserve user's role and form factor override, but update auto_form_factor from fresh fetch
                let mut kept = existing.clone();
                kept.auto_form_factor = sa.auto_form_factor;
                merged.push(kept);
            } else {
                // New light, use default role from state
                merged.push(sa.clone());
            }
        }
        log::info!("[Hue] Synced light assignments: {} lights (was {})",
            merged.len(), self.config.light_assignments.len());
        self.config.light_assignments = merged;
        true
    }

    /// Check for pending credentials from background pairing thread and apply them.
    /// Also auto-fetches entertainment areas once credentials are available.
    /// Returns true if credentials were applied (caller should persist to db).
    /// Call this each frame from the main loop.
    pub fn apply_pending_credentials(&mut self) -> bool {
        let mut state = self.state.lock().unwrap();
        if let (Some(username), Some(clientkey)) = (state.pending_username.take(), state.pending_clientkey.take()) {
            log::info!("[Hue] Applying credentials from pairing: username={}", username);
            self.config.username = username;
            self.config.clientkey = clientkey;
            drop(state);
            // Auto-fetch areas now that we have credentials
            self.fetch_areas();
            true
        } else {
            false
        }
    }
}

impl Default for HueManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for HueManager {
    fn drop(&mut self) {
        self.disconnect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cue_snapshot_default_uv_level() {
        // Operator UV accent master defaults to 0.35 — the GigBAR UV layer
        // (par ch4/9 + pod ch22) reads this off the snapshot in dmx/mapping.rs.
        let snap = CueSnapshot::default();
        assert!((snap.uv_level - 0.35).abs() < 1e-6);
    }

    #[test]
    fn from_cue_engine_stamps_operator_masters() {
        // reactivity / global_brightness / uv_level are operator state passed
        // straight through from_cue_engine onto the snapshot, unmodified.
        let cue = CueEngine::new();
        let genre = GenreProfile::default();
        let snap = CueSnapshot::from_cue_engine(
            &cue, &genre, 0.7, 0.9, 0.35, false, false, false, false, false,
        );
        assert!((snap.reactivity - 0.7).abs() < 1e-6);
        assert!((snap.global_brightness - 0.9).abs() < 1e-6);
        assert!((snap.uv_level - 0.35).abs() < 1e-6);
    }
}

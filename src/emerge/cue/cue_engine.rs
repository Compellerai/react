//! Cue Engine - Generic 3-Layer OSC State Machine for Lighting Control
//!
//! DMX-software-agnostic output using standard OSC patterns under `/react/` namespace.
//! Any lighting controller (Maestro, QLC+, GrandMA, etc.) can map these addresses.
//!
//! ## Layer 1: Palette (color mood, slow transitions)
//! - ICE (0): balance > 0.7 — sparse/ambient, cool whites
//! - COOL_A (1): balance 0.4-0.7 — moderate activity
//! - COOL_B (2): balance < 0.4 — dense/saturated
//!
//! ## Layer 2: Zone (intensity, medium transitions)
//! - SILENT (0): energy < 0.02
//! - STILL (1): energy 0.02-0.15
//! - LOW (2): energy 0.15-0.4
//! - MID (3): energy 0.4-0.7
//! - HIGH (4): energy > 0.7
//!
//! ## Layer 3: Accents (triggers, fast)
//! - STROBE: fires on kick when zone >= HIGH
//! - BLACKOUT: active when zone == SILENT for 500ms

use std::time::Instant;
use rosc::OscType;

use crate::render::{HarmonicLabel, LightingMetrics};
use crate::emerge::{ArcState, KeySignal};
use crate::osc::OscConfig;

// ============================================================================
// SMOOTHING RATES (per-frame alpha at ~60fps)
// ============================================================================

/// Energy smoothing attack: fast (~60ms τ) — responds to beats quickly
const SMOOTH_ENERGY_ATTACK: f32 = 0.25;
/// Energy smoothing decay: faster (~40ms τ) — drops quicker between beats for punchier lights
const SMOOTH_ENERGY_DECAY: f32 = 0.35;
/// Zone energy smoothing: slow (~800ms τ) — stable zone selection, not beat-reactive
/// At 60fps: tau = 1/(0.02*60) ≈ 0.83s. Section changes (verse→chorus) take ~2-5s
/// to fully register, preventing zone flickering on individual beats.
const SMOOTH_ZONE_ENERGY: f32 = 0.02;
/// Balance smoothing: slow for stable colors (~400ms τ)
const SMOOTH_BALANCE: f32 = 0.04;
/// Tension smoothing: responsive (~120ms τ)
const SMOOTH_TENSION: f32 = 0.14;
/// Stability smoothing: slow (~500ms τ)
const SMOOTH_STABILITY: f32 = 0.03;

// ============================================================================
// PALETTE THRESHOLDS (balance-driven)
// ============================================================================

/// Balance threshold for ICE (sparse/ambient): balance > this
const PALETTE_ICE_THRESH: f32 = 0.70;
/// Balance threshold for COOL_B (dense/saturated): balance < this
const PALETTE_COOL_B_THRESH: f32 = 0.40;
/// Hysteresis deadband (±0.05)
const PALETTE_HYSTERESIS: f32 = 0.05;

// ============================================================================
// ZONE THRESHOLDS (energy-driven)
// ============================================================================

/// Zone boundaries (energy thresholds)
const ZONE_SILENT: f32 = 0.02;
const ZONE_STILL: f32 = 0.15;
const ZONE_LOW: f32 = 0.40;
const ZONE_MID: f32 = 0.70;
/// Hysteresis (±0.05 on all thresholds)
const ZONE_HYSTERESIS: f32 = 0.05;

// ============================================================================
// STABILITY GATE
// ============================================================================

/// Minimum stability required for zone changes
const STABILITY_GATE_THRESH: f32 = 0.30;
/// Time stability must be above threshold before zone changes allowed (seconds)
const STABILITY_GATE_TIME: f32 = 0.20;

// ============================================================================
// ACCENT THRESHOLDS
// ============================================================================

/// Strobe cooldown between triggers (seconds)
const STROBE_COOLDOWN: f32 = 0.40;
/// Strobe pulse duration (seconds) — must be > Hue frame time (40ms) for visibility
const STROBE_DURATION: f32 = 0.15;
/// Blackout delay after entering SILENT (seconds)
const BLACKOUT_DELAY: f32 = 0.50;

// ============================================================================
// RAW SILENCE DETECTION (true audio absence)
// ============================================================================

/// Raw energy threshold for silence detection (same as video SilenceGate)
/// Below this = true silence (no audio signal), above = audio present
const RAW_SILENCE_THRESHOLD: f32 = 0.0003;
/// Seconds of sustained raw silence before forcing SILENT zone
/// Shorter than video gate (2s vs 4s) since lighting should respond faster
const RAW_SILENCE_HOLD: f32 = 2.0;

// ============================================================================
// MUSICAL RESTRAINT CONSTANTS
// ============================================================================

/// Maximum strobes allowed per 30-second window
const MAX_STROBES_PER_30S: u16 = 8;
/// Minimum seconds between "big moment" effects (drop flash, etc.)
const MIN_BIG_MOMENT_SPACING: f32 = 15.0;

/// Tension bank cost: strobe trigger
const STROBE_COST: f32 = 0.08;
/// Tension bank cost: polarity flash
const POLARITY_COST: f32 = 0.05;
/// Tension bank cost: dominant accent
const DOMINANT_COST: f32 = 0.03;
/// Tension bank cost: drop flash (expensive — big moment)
const DROP_COST: f32 = 0.30;

/// Length of extended energy history buffer (~5s at 60fps)
const ENERGY_HISTORY_LONG_LEN: usize = 300;
/// Frames of positive long slope needed before drop detection triggers (>2s)
const DROP_BUILD_FRAMES: u16 = 120;
/// Energy jump threshold in last 15 frames to confirm a drop
const DROP_ENERGY_JUMP: f32 = 0.25;

// Adaptive tension (see the `tension_dyn` field): baseline time constant +
// gain mapping (tension − baseline) into 0..1.
const TENSION_BASE_TAU_S: f32 = 30.0;
const TENSION_DYN_GAIN: f32 = 3.0;

// ============================================================================
// MACRO STATE MACHINE CONSTANTS (arc-driven buildup)
// ============================================================================
// The macro is the PERFORMER (BlindersHold → Pump → Recover, the approved
// white-coil/detonate/recover sequence); the show brain's arc classifier is
// the CONDUCTOR. The old dual-EMA energy-trend detector lived here because
// pre-emerge lighting had no better signal — it couldn't tell "tension
// coiling" from "build cresting" (both are rising energy) and spent the white
// blast during the coil. Entry now keys on the arc.

/// Band LP smoothing alpha (~333ms tau) — same as energy for frequency bands
const MACRO_BAND_ALPHA: f32 = 0.05;
/// Beat confidence: enter beat_ok (hysteresis high)
const MACRO_BEAT_OK_ENTER: f32 = 0.35;
/// Beat confidence: exit beat_ok (hysteresis low)
const MACRO_BEAT_OK_EXIT: f32 = 0.25;

/// Seconds the show-brain arc must hold PreDropTension before BlindersHold
/// engages — rides through 1-2 frame classification flicker without delaying
/// the hold past the crest. (The arc itself is already debounced and built
/// from slow EMAs; this is a thin edge guard, not a second detector.)
const MACRO_PREDROP_SUSTAIN_S: f32 = 0.3;
/// BlindersHold fizzle: seconds the arc must sit OUTSIDE PreDropTension (and
/// not in a drop) before the hold releases gracefully to Recover. A build
/// that dissolves instead of dropping must never fire the Pump — the old
/// trend-falloff exit pumped on every fizzle.
const MACRO_FIZZLE_S: f32 = 1.0;
/// Hard ceiling on a BlindersHold: even a stalled crest (a DJ riding the
/// filter forever) releases the room after this long.
const MACRO_HOLD_MAX_S: f32 = 8.0;
/// Pump hold duration (seconds) — bass pumping after drop
const MACRO_PUMP_HOLD_S: f32 = 4.0;
/// Cool hold duration (seconds) — ease-back after pump
const MACRO_COOL_HOLD_S: f32 = 3.0;

// ============================================================================
// TIMED PALETTE CYCLING
// ============================================================================

/// Base interval for timed palette cycling at palette_change_rate = 1.0.
/// `palette_change_rate` is a reactivity MULTIPLIER (genre.rs: 0.5 = slow,
/// 2.0 = fast), so the effective interval is `base / rate` — rate 2.0 cycles
/// twice as fast as 1.0, rate 0.5 half as fast.
const PALETTE_CYCLE_BASE_INTERVAL_S: f32 = 30.0;
/// After a timed palette cycle fires, suppress the balance-driven palette
/// machine for this long so the cycled palette actually survives. Without the
/// hold, update_palette recomputes from balance_smooth on the very next frame
/// and reverts the cycle — contradictory /react/palette messages ~16ms apart.
const PALETTE_CYCLE_HOLD_S: f32 = 8.0;

// ============================================================================
// ENUMS
// ============================================================================

/// Macro state machine phase for buildup → pump → cool → normal
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoBlinderPhase {
    /// Normal operation — no macro active
    Normal,
    /// Buildup detected — 8Hz white strobe
    BlindersHold,
    /// Post-buildup — bass³ × 1.5 pumping (the drop)
    OverdriveStrobe,
    /// Post-pump — 75% brightness ease-back
    Recover,
}

impl AutoBlinderPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            AutoBlinderPhase::Normal => "Normal",
            AutoBlinderPhase::BlindersHold => "Buildup",
            AutoBlinderPhase::OverdriveStrobe => "Pump",
            AutoBlinderPhase::Recover => "Cool",
        }
    }

    pub fn index(&self) -> i32 {
        match self {
            AutoBlinderPhase::Normal => 0,
            AutoBlinderPhase::BlindersHold => 1,
            AutoBlinderPhase::OverdriveStrobe => 2,
            AutoBlinderPhase::Recover => 3,
        }
    }
}

/// Color palette mood (Layer 1)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Palette {
    /// Sparse/ambient, cool whites (balance > 0.7)
    Ice = 0,
    /// Moderate activity (balance 0.4-0.7)
    CoolA = 1,
    /// Dense/saturated (balance < 0.4)
    CoolB = 2,
}

impl Palette {
    pub fn as_str(&self) -> &'static str {
        match self {
            Palette::Ice => "ICE",
            Palette::CoolA => "COOL_A",
            Palette::CoolB => "COOL_B",
        }
    }

    pub fn index(&self) -> i32 {
        *self as i32
    }
}

/// Intensity zone (Layer 2)
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EnergyZone {
    /// Silence (energy < 0.02)
    Silent = 0,
    /// Ambient/minimal (energy 0.02-0.15)
    Still = 1,
    /// Building energy (energy 0.15-0.4)
    Low = 2,
    /// Active/dancing (energy 0.4-0.7)
    Mid = 3,
    /// Peak energy (energy > 0.7)
    High = 4,
}

impl EnergyZone {
    pub fn as_str(&self) -> &'static str {
        match self {
            EnergyZone::Silent => "SILENT",
            EnergyZone::Still => "STILL",
            EnergyZone::Low => "LOW",
            EnergyZone::Mid => "MID",
            EnergyZone::High => "HIGH",
        }
    }

    pub fn index(&self) -> i32 {
        *self as i32
    }
}

// ============================================================================
// CUE ENGINE
// ============================================================================

/// Generic 3-layer OSC state machine for lighting control
pub struct CueEngine {
    // === SMOOTHED VALUES ===
    energy_smooth: f32,
    /// Slow-smoothed energy for zone selection (won't flicker on individual beats)
    zone_energy: f32,
    balance_smooth: f32,
    tension_smooth: f32,
    stability_smooth: f32,
    /// Slow per-track tension baseline (τ≈30 s) — see `tension_dyn`.
    tension_base: f32,
    /// ADAPTIVE tension: how far tension sits above its own slow baseline,
    /// 0..1. The lighting tension reads atonality-ish spectral character, so
    /// a wall-of-sound track rails the ABSOLUTE level at ~1.0 for minutes
    /// (post-rock) — breathing consumers (the UV layer) read this instead,
    /// so even an all-tension song keeps its internal dynamics. Absolute
    /// gates keep reading `tension_smooth`.
    tension_dyn: f32,
    /// Seconds between the two most recent `process` calls (Instant-derived,
    /// so it stays precise at any uptime — unlike differencing the f32
    /// `frame_time`, whose ULP reaches frame-interval scale after ~3 days).
    last_dt: f32,
    /// Integrated pixel-bar chase-head phase (0..1): advances by rate*dt each
    /// frame, where the rate rides smoothed energy. Stateful on purpose —
    /// `frame_time * rate(energy)` teleports the head on every energy change
    /// once frame_time is large.
    chase_head: f32,

    // === LAYER 1: PALETTE ===
    palette: Palette,

    // === LAYER 2: ZONE ===
    zone: EnergyZone,
    /// Time stability has been above gate threshold
    stability_gate_timer: f32,
    /// Whether stability gate is open (zone changes allowed)
    stability_gate_open: bool,

    // === LAYER 3: ACCENTS ===
    /// Strobe cooldown remaining
    strobe_cooldown: f32,
    /// Strobe pulse remaining (> 0 means strobe active)
    strobe_pulse: f32,
    /// Last strobe intensity (for UI)
    strobe_intensity: f32,
    /// Golden-ratio low-discrepancy phase for the probabilistic strobe gate
    /// (see next_strobe_dither). Persistent accumulator in [0,1) — stays
    /// precise at any uptime, unlike deriving the dither from frame_counter.
    strobe_dither: f32,
    /// Blackout active — stage forced dark on sustained true silence
    blackout_active: bool,
    /// Sustained-audio timer that debounces the blackout re-light
    blackout_hold_time: f32,

    // === HARMONIC STATE ===
    harmonic_label: HarmonicLabel,
    last_key_root: u8,

    // === TIMING ===
    last_update: Instant,
    /// Frame counter for OSC emit throttling and periodic diagnostics.
    /// (NOT used for strobe randomization — that's `strobe_dither`, which
    /// stays f32-precise at any uptime; a counter-derived dither does not.)
    frame_counter: u64,

    // === UI ACTIVITY ===
    zone_trigger_times: [Instant; 5],

    // === POLARITY ACCENT ===
    /// Active polarity flash duration remaining
    polarity_flash_timer: f32,
    /// Cooldown between polarity flashes
    polarity_cooldown: f32,
    /// Last seen polarity quadrant
    last_quadrant: u8,

    // === DOMINANT ACCENT ===
    /// Active dominant pitch flash duration remaining
    dominant_flash_timer: f32,
    /// Cooldown between dominant flashes
    dominant_cooldown: f32,
    /// Last dominant pitch class
    last_dominant_pc: u8,

    // === TONALITY STATE ===
    /// Smoothed major likelihood (0-1)
    major_smooth: f32,

    // === ENERGY SLOPE ===
    /// Ring buffer of energy history (~0.5s at 60fps)
    energy_history: [f32; 30],
    /// Current write position in energy history
    energy_history_idx: usize,
    /// Calculated energy slope (-1 to +1, positive = building)
    energy_slope: f32,

    // === EXCITEMENT RAMP (musical arc tracker) ===
    /// 0.0-1.0, builds over seconds during high-energy music, decays slowly
    excitement: f32,

    // === TENSION BANK (currency for effects) ===
    /// 0.0-1.0, accumulates during tension, spent on effects
    tension_bank: f32,

    // === FATIGUE PREVENTION ===
    /// Strobes fired in the current 30-second window
    strobe_count_window: u16,
    /// Start of the current 30-second strobe counting window
    strobe_window_start: Instant,
    /// Last time a "big" effect was used (drop flash, full-room burst)
    last_big_moment: Instant,

    // === EXTENDED ENERGY SLOPE ===
    /// ~5s ring buffer at 60fps for build/drop detection
    energy_history_long: [f32; 300],
    /// Current write position in long energy history
    energy_history_long_idx: usize,
    /// Consecutive frames with positive long slope (for drop detection)
    building_frames: u16,

    // === MONOTONIC TIME ===
    /// Monotonic seconds since engine creation (for breathing sine, etc.)
    frame_time: f32,
    /// Creation instant for calculating frame_time
    creation_instant: Instant,

    // === ENVELOPE VALUES (smooth 0-1 for downstream consumers like Hue) ===
    /// Strobe envelope (exponential decay from 1.0 on trigger)
    strobe_env: f32,
    /// Dominant flash envelope (exponential decay from 1.0 on trigger)
    dominant_env: f32,
    /// Polarity flash envelope (exponential decay from 1.0 on trigger)
    polarity_env: f32,

    // === CURRENT PATTERN ===
    /// Last pattern index sent via /react/pattern (-1 = none sent yet)
    current_pattern: i32,

    // === ZONE HISTOGRAM (rolling 30s self-monitoring) ===
    /// Count of frames in each zone over the current 30s window
    zone_histogram: [u32; 5],
    /// Total frames in current histogram window
    zone_histogram_total: u32,
    /// When the current histogram window started
    zone_histogram_start: Instant,

    // === MACRO STATE MACHINE (buildup → pump → cool → normal) ===
    /// Current macro phase
    macro_phase: AutoBlinderPhase,
    /// Smoothed bass band
    macro_low_lp: f32,
    /// Smoothed mid band
    macro_mid_lp: f32,
    /// Smoothed high band
    macro_high_lp: f32,
    /// Beat confident (with hysteresis: enter 0.35, exit 0.25)
    macro_beat_ok: bool,
    /// Show-brain arc stamped per frame via `set_arc` (one frame stale across
    /// the thread seam — musically nothing). The macro's single source of
    /// musical truth: BlindersHold keys on PreDropTension, Pump on Drop.
    current_arc: ArcState,
    /// Previous frame's arc — for drop-EDGE detection, so a long sustained
    /// Drop enters Pump once instead of retriggering every Recover→Normal lap.
    last_arc: ArcState,
    /// Seconds the arc has held PreDropTension while Normal (entry edge guard).
    predrop_sustain_s: f32,
    /// Seconds the arc has sat outside PreDropTension during a BlindersHold
    /// (fizzle release — see MACRO_FIZZLE_S).
    hold_fizzle_s: f32,
    /// Time spent in current phase (Hold cap / Pump / Cool hold timers)
    buildup_phase_timer: f32,
    /// True when the current BlindersHold was operator-forced (force_buildup).
    /// A live disarm releases only ARC-entered holds — a hold the operator
    /// explicitly fired plays out.
    hold_forced: bool,
    /// Set true by update_drop_detection() on the frame a drop fires, cleared next frame
    drop_detected_this_frame: bool,
    /// Show-brain INTENSITY LICENSE stamped per frame via
    /// `set_intensity_license` (default 1.0 = classic behavior for un-stamped
    /// producers/tests). Genre-free percussiveness × spectral fullness: shapes
    /// Pump (bloom vs detonation) and thins strobe probability continuously —
    /// an EDM vocal bridge softens the show while the kick grid persists.
    intensity_license: f32,
    /// The EFFECTIVE style posture stamped per frame via
    /// `set_effective_style` — the operator's pin, or the Auto resolver's
    /// flip (Chill/Energetic/Banger). Gates the buildup macro, the 8Hz
    /// BlindersHold, and the kick strobe. Default Auto = permissive =
    /// classic behavior for un-stamped producers/tests.
    effective_style: crate::emerge::LightingStyle,
    /// Computed at Pump entry: may this Pump express as STROBING (par strobe
    /// band, overdrive consumers)? Needs a strobe-permitting genre AND a beat
    /// to strobe to. The brightness ride happens regardless.
    overdrive_strobe_flavor: bool,
    /// The atmosphere-shaped ceiling drop_impact sustains at while energy
    /// stays high (the sustain loop used to re-pin 1.0, which would defeat
    /// the bloom ceiling every frame).
    drop_impact_ceiling: f32,

    // === PALETTE/PATTERN CYCLING TIMERS ===
    /// Timer for palette cycling (seconds since last palette change)
    palette_change_timer: f32,
    /// Timer for pattern cycling (seconds since last pattern change)
    pattern_change_timer: f32,
    /// `frame_time` deadline until which the balance-driven palette machine is
    /// suppressed after a timed cycle (set by update_palette_cycling, checked
    /// by update_palette). 0.0 = no hold active.
    palette_cycle_hold_until: f32,

    // === BASS PUMP (kick-triggered brightness envelope) ===
    /// Bass pump envelope (0-1): fires on every kick, decays ~150ms.
    /// Guarantees drops feel strong regardless of macro state.
    bass_pump_env: f32,

    // === DROP IMPACT ===
    /// Drop impact level (0.0-1.0): forces high brightness after macro transitions.
    /// Held at 1.0 while energy stays high, decays (~0.8s tau) when energy drops.
    drop_impact_level: f32,

    // === RAW SILENCE DETECTION ===
    /// Timer for sustained raw silence detection.
    /// Accumulates when raw_energy < RAW_SILENCE_THRESHOLD, resets on any audio.
    /// When >= RAW_SILENCE_HOLD, forces zone to SILENT (true audio absence).
    raw_silence_timer: f32,
}

impl CueEngine {
    /// Create a new cue engine
    pub fn new() -> Self {
        let now = Instant::now();
        let past = now - std::time::Duration::from_secs(10);

        Self {
            energy_smooth: 0.0,
            zone_energy: 0.0,
            balance_smooth: 0.5,
            tension_smooth: 0.0,
            stability_smooth: 0.0,
            tension_base: 0.0,
            tension_dyn: 0.0,
            last_dt: 0.0,
            chase_head: 0.0,

            palette: Palette::CoolA,
            zone: EnergyZone::Silent,
            stability_gate_timer: 0.0,
            stability_gate_open: false,

            strobe_cooldown: 0.0,
            strobe_pulse: 0.0,
            strobe_intensity: 0.0,
            strobe_dither: 0.0,
            blackout_active: false,
            blackout_hold_time: 0.0,

            harmonic_label: HarmonicLabel::Atonal,
            last_key_root: 0,

            last_update: now,
            frame_counter: 0,
            zone_trigger_times: [past; 5],

            // Polarity accent state
            polarity_flash_timer: 0.0,
            polarity_cooldown: 0.0,
            last_quadrant: 0,

            // Dominant accent state
            dominant_flash_timer: 0.0,
            dominant_cooldown: 0.0,
            last_dominant_pc: 0,

            // Tonality state
            major_smooth: 0.5,

            // Energy slope state
            energy_history: [0.0; 30],
            energy_history_idx: 0,
            energy_slope: 0.0,

            // Excitement ramp — start slightly above zero so lights respond immediately
            excitement: 0.15,

            // Tension bank
            tension_bank: 0.0,

            // Fatigue prevention
            strobe_count_window: 0,
            strobe_window_start: now,
            last_big_moment: past,

            // Extended energy slope
            energy_history_long: [0.0; 300],
            energy_history_long_idx: 0,
            building_frames: 0,

            // Monotonic time
            frame_time: 0.0,
            creation_instant: now,

            // Envelope values
            strobe_env: 0.0,
            dominant_env: 0.0,
            polarity_env: 0.0,

            // Current pattern
            current_pattern: -1,

            // Zone histogram
            zone_histogram: [0; 5],
            zone_histogram_total: 0,
            zone_histogram_start: now,

            // Macro state machine
            macro_phase: AutoBlinderPhase::Normal,
            macro_low_lp: 0.0,
            macro_mid_lp: 0.0,
            macro_high_lp: 0.0,
            macro_beat_ok: false,
            current_arc: ArcState::Silence,
            last_arc: ArcState::Silence,
            predrop_sustain_s: 0.0,
            hold_fizzle_s: 0.0,
            buildup_phase_timer: 0.0,
            hold_forced: false,
            drop_detected_this_frame: false,
            intensity_license: 1.0,
            effective_style: crate::emerge::LightingStyle::Auto,
            overdrive_strobe_flavor: false,
            drop_impact_ceiling: 1.0,

            // Palette/pattern cycling timers
            palette_change_timer: 0.0,
            pattern_change_timer: 0.0,
            palette_cycle_hold_until: 0.0,

            // Bass pump
            bass_pump_env: 0.0,

            // Drop impact
            drop_impact_level: 0.0,

            // Raw silence detection
            raw_silence_timer: 0.0,
        }
    }

    // === PUBLIC GETTERS ===

    pub fn current_zone(&self) -> EnergyZone {
        self.zone
    }

    pub fn current_palette(&self) -> Palette {
        self.palette
    }

    pub fn current_harmonic(&self) -> HarmonicLabel {
        self.harmonic_label
    }

    pub fn strobe_hold_active(&self) -> bool {
        self.strobe_pulse > 0.0
    }

    pub fn blackout_active(&self) -> bool {
        self.blackout_active
    }

    pub fn stability_latch(&self) -> bool {
        self.stability_gate_open
    }

    pub fn smoothed_metrics(&self) -> (f32, f32, f32, f32) {
        (self.energy_smooth, self.tension_smooth, self.balance_smooth, self.stability_smooth)
    }

    /// Slow-smoothed energy (0-1) — the zone classifier's hysteresis signal,
    /// reused as an RMS-style loudness envelope for the virtual-stage
    /// oscillators. Lags `energy_smooth` by design (SMOOTH_ZONE_ENERGY).
    pub fn zone_energy(&self) -> f32 {
        self.zone_energy
    }

    /// Get polarity flash timer (> 0 means flash is active)
    pub fn polarity_flash_active(&self) -> bool {
        self.polarity_flash_timer > 0.0
    }

    /// Get last polarity quadrant (0=E, 1=N, 2=W, 3=S)
    pub fn last_quadrant(&self) -> u8 {
        self.last_quadrant
    }

    /// Get dominant flash timer (> 0 means flash is active)
    pub fn dominant_flash_active(&self) -> bool {
        self.dominant_flash_timer > 0.0
    }

    /// Get last dominant pitch class (0-11)
    pub fn last_dominant_pc(&self) -> u8 {
        self.last_dominant_pc
    }

    /// Get smoothed major likelihood (0-1)
    pub fn major_smooth(&self) -> f32 {
        self.major_smooth
    }

    /// Get energy slope (-1 to +1, positive = building)
    pub fn energy_slope(&self) -> f32 {
        self.energy_slope
    }

    /// Get excitement ramp (0-1, musical arc tracker)
    pub fn excitement(&self) -> f32 {
        self.excitement
    }

    /// Get tension bank balance (0-1, currency for effects)
    pub fn tension_bank(&self) -> f32 {
        self.tension_bank
    }

    /// Get monotonic time in seconds since engine creation
    pub fn frame_time(&self) -> f32 {
        self.frame_time
    }

    /// Seconds between the two most recent `process` calls (Instant-derived).
    pub fn last_dt(&self) -> f32 {
        self.last_dt
    }

    /// Monotonic engine-frame ordinal (one increment per `process`). Stamped
    /// onto `CueSnapshot` so consumers can deduplicate frames EXACTLY — f32
    /// `frame_time` equality breaks after ~1.5-3 days of uptime, when its ULP
    /// exceeds the frame interval and distinct frames print identical times.
    pub fn frame_seq(&self) -> u64 {
        self.frame_counter
    }

    /// Integrated pixel-bar chase-head phase (0..1). See the field doc.
    pub fn chase_head(&self) -> f32 {
        self.chase_head
    }

    /// Strobe envelope (0-1, smooth exponential decay from trigger)
    pub fn strobe_env(&self) -> f32 {
        self.strobe_env
    }

    /// Dominant flash envelope (0-1, smooth exponential decay from trigger)
    pub fn dominant_env(&self) -> f32 {
        self.dominant_env
    }

    /// Polarity flash envelope (0-1, smooth exponential decay from trigger)
    pub fn polarity_env(&self) -> f32 {
        self.polarity_env
    }

    /// Get the last pattern index sent via /react/pattern (-1 = none sent yet)
    pub fn current_pattern(&self) -> i32 {
        self.current_pattern
    }

    /// Whether buildup strobe is active (BlindersHold phase)
    pub fn auto_blinder_active(&self) -> bool {
        matches!(self.macro_phase, AutoBlinderPhase::BlindersHold)
    }

    /// Whether overdrive strobe is active. False during an atmospheric Pump
    /// (a crescendo BLOOM) and for strobe-banning genres — the brightness
    /// ride still happens; only the strobe FLAVOR is withheld.
    pub fn overdrive_strobe_active(&self) -> bool {
        matches!(self.macro_phase, AutoBlinderPhase::OverdriveStrobe) && self.overdrive_strobe_flavor
    }

    /// Adaptive tension 0..1 (dynamics above the track's own baseline).
    pub fn tension_dyn(&self) -> f32 {
        self.tension_dyn
    }

    /// Stamp the show brain's intensity license for this frame (genre-free
    /// atmosphere × fullness signal; shapes Pump into bloom vs detonation and
    /// thins strobe probability).
    pub fn set_intensity_license(&mut self, license: f32) {
        self.intensity_license =
            if license.is_finite() { license.clamp(0.0, 1.0) } else { 1.0 };
    }

    /// Stamp the effective style posture (operator pin, or the Auto
    /// resolver's flip) for this frame's macro/strobe gates.
    pub fn set_effective_style(&mut self, style: crate::emerge::LightingStyle) {
        self.effective_style = style;
    }

    /// Get current macro phase
    pub fn auto_blinder_phase(&self) -> AutoBlinderPhase {
        self.macro_phase
    }

    /// Stamp the show brain's debounced musical arc for this frame. The macro
    /// keys its transitions on this — the CueEngine no longer second-guesses
    /// what a build is with its own energy trend. Stamped from `LightingInput`
    /// each frame (the brain ticks even when DMX output is disabled).
    pub fn set_arc(&mut self, arc: ArcState) {
        self.current_arc = arc;
    }

    /// Force-enter BlindersHold (Buildup) from external trigger (MIDI button / UI).
    /// Skips buildup detection entirely. Returns OSC messages to send.
    /// No-op if already in a macro phase.
    pub fn force_buildup(&mut self) -> Vec<(String, Vec<rosc::OscType>)> {
        use rosc::OscType;
        if self.macro_phase != AutoBlinderPhase::Normal {
            log::info!("[MACRO] force_buildup ignored — already in {:?}", self.macro_phase);
            return vec![];
        }

        self.macro_phase = AutoBlinderPhase::BlindersHold;
        self.predrop_sustain_s = 0.0;
        self.hold_fizzle_s = 0.0;
        self.buildup_phase_timer = 0.0;
        self.hold_forced = true;

        log::info!("[MACRO] FORCE BUILDUP — manual trigger (arc={:?})", self.current_arc);

        vec![
            ("/react/macro/phase".to_string(), vec![OscType::Int(AutoBlinderPhase::BlindersHold.index())]),
            ("/triggers/blinder".to_string(), vec![OscType::Bool(true)]),
        ]
    }

    /// Bass pump envelope (0-1, fires on kick, decays ~150ms)
    pub fn bass_pump(&self) -> f32 {
        self.bass_pump_env
    }

    /// Drop impact level (0.0-1.0): sustains while energy is high, decays when music backs off
    pub fn drop_impact_level(&self) -> f32 {
        self.drop_impact_level
    }

    /// Get zone histogram percentages [Silent, Still, Low, Mid, High] for the current 30s window
    pub fn zone_histogram_pcts(&self) -> [f32; 5] {
        if self.zone_histogram_total == 0 {
            return [0.0; 5];
        }
        let total = self.zone_histogram_total as f32;
        [
            self.zone_histogram[0] as f32 / total,
            self.zone_histogram[1] as f32 / total,
            self.zone_histogram[2] as f32 / total,
            self.zone_histogram[3] as f32 / total,
            self.zone_histogram[4] as f32 / total,
        ]
    }

    /// Get activity level for each zone (0.0-1.0, fades over 500ms)
    pub fn zone_activity(&self) -> [f32; 5] {
        const FADE_MS: f32 = 500.0;
        let mut activity = [0.0f32; 5];
        for i in 0..5 {
            let elapsed_ms = self.zone_trigger_times[i].elapsed().as_secs_f32() * 1000.0;
            activity[i] = (1.0 - elapsed_ms / FADE_MS).max(0.0);
        }
        activity
    }

    // === MAIN UPDATE ===

    /// Process lighting metrics and generate OSC messages
    pub fn process(
        &mut self,
        metrics: &LightingMetrics,
        key_signal: &KeySignal,
        config: &OscConfig,
    ) -> Vec<(String, Vec<OscType>)> {
        let mut messages = Vec::new();

        // Calculate dt
        let now = Instant::now();
        let dt = (now - self.last_update).as_secs_f32().min(0.1);
        self.last_update = now;
        self.last_dt = dt;

        // Update monotonic frame time
        self.frame_time = self.creation_instant.elapsed().as_secs_f32();

        // === SMOOTH ALL INPUTS ===
        let energy_alpha = if metrics.energy > self.energy_smooth { SMOOTH_ENERGY_ATTACK } else { SMOOTH_ENERGY_DECAY };
        self.energy_smooth += (metrics.energy - self.energy_smooth) * energy_alpha;
        // Advance the integrated chase head by rate*dt (rate rides energy).
        self.chase_head =
            (self.chase_head + (0.45 + self.energy_smooth * 1.9) * dt).rem_euclid(1.0);
        self.zone_energy += (metrics.energy - self.zone_energy) * SMOOTH_ZONE_ENERGY;
        self.balance_smooth += (metrics.balance - self.balance_smooth) * SMOOTH_BALANCE;
        self.tension_smooth += (metrics.tension - self.tension_smooth) * SMOOTH_TENSION;
        // Adaptive tension: measure against the track's own slow baseline so
        // railed absolute tension still yields internal dynamics.
        let tension_base_alpha = 1.0 - (-dt / TENSION_BASE_TAU_S).exp();
        self.tension_base += (self.tension_smooth - self.tension_base) * tension_base_alpha;
        self.tension_dyn =
            ((self.tension_smooth - self.tension_base) * TENSION_DYN_GAIN).clamp(0.0, 1.0);
        self.stability_smooth += (metrics.stability - self.stability_smooth) * SMOOTH_STABILITY;

        // === UPDATE STABILITY GATE ===
        self.update_stability_gate(dt);

        // === REACTIVE FEATURES (early, for use by other layers) ===
        self.update_tonality(metrics, config);
        self.update_energy_slope(metrics, config, &mut messages);

        // === MUSICAL ARC (before layers, gates downstream behavior) ===
        self.update_excitement(dt);
        self.update_tension_bank(dt);
        self.update_fatigue_window();
        self.update_extended_energy_slope(metrics);
        self.update_drop_detection(&mut messages);

        // === MACRO STATE MACHINE (buildup → blinder → overdrive → recover) ===
        // Note: runs BEFORE clearing drop_detected_this_frame so it can react to drops
        self.update_macro(dt, metrics, config, &mut messages);

        // Clear per-frame flags (AFTER macro state machine has seen them)
        self.drop_detected_this_frame = false;

        // === BASS PUMP (kick-triggered brightness envelope) ===
        // Fires on every kick (not zone-gated) to guarantee drops feel strong.
        // Suppressed during BlindersHold (blinder has its own brightness).
        {
            let r = config.reactivity;
            let drive_gain = 0.8 + (1.8 - 0.8) * r; // lerp(0.8, 1.8, r)
            if metrics.kick && self.macro_phase != AutoBlinderPhase::BlindersHold {
                let bass_drive = drive_gain * self.macro_low_lp;
                let pump = bass_drive.powf(1.8).clamp(0.0, 1.0);
                self.bass_pump_env = self.bass_pump_env.max(pump); // Don't interrupt a stronger pump
            }
            // Exponential decay (~150ms tau)
            self.bass_pump_env *= (-dt / 0.15).exp();
            if self.bass_pump_env < 0.005 { self.bass_pump_env = 0.0; }
        }

        // === DROP IMPACT ===
        // Sustains high brightness as long as energy is high after a macro transition.
        // Held at 1.0 while energy stays energetic (zone >= Mid or energy above threshold).
        // Decays with ~0.8s tau only when energy genuinely drops.
        // This prevents the "weak for 3 seconds" gap between macro exit and normal brightness catching up.
        if self.drop_impact_level > 0.0 {
            let energy_high = matches!(self.current_zone(), EnergyZone::Mid | EnergyZone::High)
                || self.energy_smooth > 0.35;
            if energy_high {
                // Energy is still high — sustain impact at the entry ceiling
                // (re-pinning 1.0 here would defeat the atmosphere bloom).
                self.drop_impact_level = self.drop_impact_ceiling;
            } else {
                // Energy dropped — decay quickly (~0.8s tau)
                self.drop_impact_level *= (-dt / 0.8).exp();
                if self.drop_impact_level < 0.02 {
                    self.drop_impact_level = 0.0;
                    log::info!("[MACRO] DROP_IMPACT fully decayed (energy backed off)");
                }
            }
        }

        // === LAYER 1: PALETTE ===
        self.update_palette(config, &mut messages);

        // Increment frame counter (emit throttling + periodic diagnostics)
        self.frame_counter += 1;

        // === RAW SILENCE DETECTION (force SILENT zone on true audio absence) ===
        // Uses raw (pre-normalization) energy to detect actual silence vs ambient noise.
        // Normalized energy is useless here — adaptive normalizer maps noise to ~0.5.
        if metrics.raw_energy < RAW_SILENCE_THRESHOLD {
            self.raw_silence_timer += dt;
            if self.raw_silence_timer >= RAW_SILENCE_HOLD && self.zone != EnergyZone::Silent {
                // Force transition to SILENT zone
                self.zone = EnergyZone::Silent;

                // Send zone change messages
                messages.push((
                    "/react/zone".to_string(),
                    vec![OscType::Int(EnergyZone::Silent.index())],
                ));
                messages.push((
                    "/react/zone/name".to_string(),
                    vec![OscType::String("SILENT".to_string())],
                ));

                // Send pattern_silent for DMX
                if !config.zone_cue_address.is_empty() {
                    messages.push((
                        config.zone_cue_address.clone(),
                        vec![OscType::Int(config.pattern_silent)],
                    ));
                }

                log::info!(
                    "[OSC] Zone: SILENT (raw silence {:.1}s, raw_energy={:.6})",
                    RAW_SILENCE_HOLD,
                    metrics.raw_energy
                );
            }
        } else {
            // Audio detected — reset silence timer
            self.raw_silence_timer = 0.0;
        }

        // === LAYER 2: ZONE ===
        self.update_zone(config, &mut messages);

        // === ZONE HISTOGRAM (rolling 30s self-monitoring) ===
        self.update_zone_histogram();

        // === TIMED PALETTE/PATTERN CYCLING ===
        self.update_palette_cycling(dt, config, &mut messages);
        self.update_pattern_cycling(dt, config, &mut messages);

        // === LAYER 3: ACCENTS ===
        self.update_strobe(dt, metrics, config, &mut messages);
        self.update_blackout(dt, config, &mut messages);

        // === REACTIVE ACCENTS ===
        self.update_polarity_flash(dt, metrics, config, &mut messages);
        self.update_dominant_accent(dt, metrics, config, &mut messages);

        // === HARMONIC STATE ===
        self.update_harmonic(key_signal, &mut messages);

        // === ENVELOPE DECAY (smooth values for downstream consumers) ===
        // Strobe: hold at 1.0 during pulse, exponential release after (~120ms tau)
        if self.strobe_pulse > 0.0 {
            self.strobe_env = 1.0;
        } else {
            self.strobe_env *= (-dt / 0.12).exp();
            if self.strobe_env < 0.01 { self.strobe_env = 0.0; }
        }
        // Dominant: exponential decay (~250ms tau for visible color bloom)
        if self.dominant_flash_timer <= 0.0 {
            self.dominant_env *= (-dt / 0.25).exp();
            if self.dominant_env < 0.01 { self.dominant_env = 0.0; }
        }
        // Polarity: exponential decay (~200ms tau)
        if self.polarity_flash_timer <= 0.0 {
            self.polarity_env *= (-dt / 0.20).exp();
            if self.polarity_env < 0.01 { self.polarity_env = 0.0; }
        }

        // === EMIT NEW OSC (throttled to ~6Hz) ===
        if self.frame_counter % 10 == 0 {
            messages.push((
                "/react/excitement".to_string(),
                vec![OscType::Float(self.excitement)],
            ));
            messages.push((
                "/react/tension/bank".to_string(),
                vec![OscType::Float(self.tension_bank)],
            ));
        }

        messages
    }

    // === PRIVATE HELPERS ===

    /// Update rolling 30s zone histogram for distribution monitoring.
    /// Logs every 30s with percentage breakdown.
    fn update_zone_histogram(&mut self) {
        let idx = self.zone.index() as usize;
        if idx < 5 {
            self.zone_histogram[idx] += 1;
            self.zone_histogram_total += 1;
        }

        // Every 30 seconds: log distribution and reset
        if self.zone_histogram_start.elapsed().as_secs_f32() >= 30.0 && self.zone_histogram_total > 0 {
            let total = self.zone_histogram_total as f32;
            let pcts: Vec<f32> = self.zone_histogram.iter().map(|&c| c as f32 / total * 100.0).collect();
            log::info!(
                "[ZoneHist 30s] Silent={:.0}% Still={:.0}% Low={:.0}% Mid={:.0}% High={:.0}% (n={})",
                pcts[0], pcts[1], pcts[2], pcts[3], pcts[4], self.zone_histogram_total
            );

            // Warn if High dominates (normalizer may need tuning)
            if pcts[4] > 70.0 {
                log::warn!("[ZoneHist] High zone > 70% — normalizer or thresholds may need adjustment");
            }

            // Reset window
            self.zone_histogram = [0; 5];
            self.zone_histogram_total = 0;
            self.zone_histogram_start = Instant::now();
        }
    }

    fn update_stability_gate(&mut self, dt: f32) {
        if self.stability_smooth >= STABILITY_GATE_THRESH {
            self.stability_gate_timer += dt;
            if self.stability_gate_timer >= STABILITY_GATE_TIME {
                self.stability_gate_open = true;
            }
        } else {
            self.stability_gate_timer = 0.0;
            self.stability_gate_open = false;
        }
    }

    fn update_palette(&mut self, config: &OscConfig, messages: &mut Vec<(String, Vec<OscType>)>) {
        // A timed palette cycle (update_palette_cycling) owns the palette for
        // a hold window. Without this, the balance-driven reassignment below
        // reverts the cycled palette on the very next frame — two
        // contradictory /react/palette messages ~16ms apart, every cycle.
        if self.frame_time < self.palette_cycle_hold_until {
            return;
        }

        // Determine target palette based on balance with hysteresis
        let target = match self.palette {
            Palette::Ice => {
                if self.balance_smooth < PALETTE_ICE_THRESH - PALETTE_HYSTERESIS {
                    if self.balance_smooth < PALETTE_COOL_B_THRESH {
                        Palette::CoolB
                    } else {
                        Palette::CoolA
                    }
                } else {
                    Palette::Ice
                }
            }
            Palette::CoolA => {
                if self.balance_smooth > PALETTE_ICE_THRESH + PALETTE_HYSTERESIS {
                    Palette::Ice
                } else if self.balance_smooth < PALETTE_COOL_B_THRESH - PALETTE_HYSTERESIS {
                    Palette::CoolB
                } else {
                    Palette::CoolA
                }
            }
            Palette::CoolB => {
                if self.balance_smooth > PALETTE_COOL_B_THRESH + PALETTE_HYSTERESIS {
                    if self.balance_smooth > PALETTE_ICE_THRESH {
                        Palette::Ice
                    } else {
                        Palette::CoolA
                    }
                } else {
                    Palette::CoolB
                }
            }
        };

        if target != self.palette {
            self.palette = target;

            // Send palette index from config (for DMX software mapping)
            let palette_idx = match self.palette {
                Palette::Ice => config.palette_ice_index,
                Palette::CoolA => config.palette_cool_a_index,
                Palette::CoolB => config.palette_cool_b_index,
            };
            messages.push((
                "/react/palette".to_string(),
                vec![OscType::Int(palette_idx)],
            ));

            // Send palette name from config
            let palette_name = match self.palette {
                Palette::Ice => config.palette_ice_name.clone(),
                Palette::CoolA => config.palette_cool_a_name.clone(),
                Palette::CoolB => config.palette_cool_b_name.clone(),
            };
            messages.push((
                "/react/palette/name".to_string(),
                vec![OscType::String(palette_name.clone())],
            ));

            // Send tonality warmth for palette modulation (0 = cool/minor, 1 = warm/major)
            if config.enable_tonality_palettes {
                messages.push((
                    "/react/palette/warmth".to_string(),
                    vec![OscType::Float(self.major_smooth)],
                ));
            }

            log::info!("[OSC] Palette: {} idx={} ({}) (balance={:.0}%, major={:.0}%)",
                self.palette.as_str(), palette_idx, palette_name,
                self.balance_smooth * 100.0, self.major_smooth * 100.0);
        }
    }

    fn update_zone(&mut self, config: &OscConfig, messages: &mut Vec<(String, Vec<OscType>)>) {
        // Skip if raw silence forced us to SILENT (handled in process())
        // This prevents energy-based zone logic from overriding true silence detection.
        if self.raw_silence_timer >= RAW_SILENCE_HOLD {
            return;
        }

        // Get zone thresholds from genre profile
        let t = &config.genre_profile.zone_thresholds;
        // Use slow zone_energy for zone selection (prevents beat-rate flickering)
        let e = self.zone_energy;
        let h = ZONE_HYSTERESIS;

        // Zone boundaries from genre profile:
        // SILENT: energy < ZONE_SILENT (fixed at 0.02)
        // STILL: energy < still_max
        // LOW: energy < low_max
        // MID: energy < mid_max
        // HIGH: energy >= mid_max

        let target = match self.zone {
            EnergyZone::Silent => {
                if e > ZONE_SILENT + h {
                    if e > t.mid_max + h { EnergyZone::High }
                    else if e > t.low_max + h { EnergyZone::Mid }
                    else if e > t.still_max + h { EnergyZone::Low }
                    else { EnergyZone::Still }
                } else {
                    EnergyZone::Silent
                }
            }
            EnergyZone::Still => {
                if e < ZONE_SILENT - h { EnergyZone::Silent }
                else if e > t.still_max + h {
                    if e > t.mid_max + h { EnergyZone::High }
                    else if e > t.low_max + h { EnergyZone::Mid }
                    else { EnergyZone::Low }
                } else {
                    EnergyZone::Still
                }
            }
            EnergyZone::Low => {
                if e < ZONE_SILENT - h { EnergyZone::Silent }
                else if e < t.still_max - h { EnergyZone::Still }
                else if e > t.low_max + h {
                    if e > t.mid_max + h { EnergyZone::High }
                    else { EnergyZone::Mid }
                } else {
                    EnergyZone::Low
                }
            }
            EnergyZone::Mid => {
                if e < ZONE_SILENT - h { EnergyZone::Silent }
                else if e < t.still_max - h { EnergyZone::Still }
                else if e < t.low_max - h { EnergyZone::Low }
                else if e > t.mid_max + h { EnergyZone::High }
                else { EnergyZone::Mid }
            }
            EnergyZone::High => {
                if e < ZONE_SILENT - h { EnergyZone::Silent }
                else if e < t.low_max - h { EnergyZone::Low }
                else if e < t.mid_max - h { EnergyZone::Mid }
                else { EnergyZone::High }
            }
        };

        // Stability gate applies to UPWARD transitions only. The gate's input
        // is chroma stability, which says nothing about whether energy has
        // genuinely fallen — an atonal/noisy passage used to close the gate
        // indefinitely (no timeout) and wedge the zone at e.g. HIGH for
        // minutes, which in turn pinned drop_impact_level at 1.0 via the
        // energy_high zone check in process(). De-escalation is always
        // allowed; escalation still waits for the gate to open.
        if !self.stability_gate_open && target > self.zone {
            return;
        }

        if target != self.zone {
            let old_zone = self.zone;
            self.zone = target;

            // Update UI activity
            let idx = self.zone.index() as usize;
            if idx < 5 {
                self.zone_trigger_times[idx] = Instant::now();
            }

            // Send zone index
            messages.push((
                "/react/zone".to_string(),
                vec![OscType::Int(self.zone.index())],
            ));

            // Send zone name
            messages.push((
                "/react/zone/name".to_string(),
                vec![OscType::String(self.zone.as_str().to_string())],
            ));

            // Send cue index on the configurable cue address.
            // Uses zone_cue_overrides if defined, otherwise UI pattern mappings.
            if !config.zone_cue_address.is_empty() {
                let zone_name = self.zone.as_str().to_string();
                let cue_idx = config.zone_cue_overrides.iter()
                    .find(|o| o.zone == zone_name)
                    .map(|o| o.cue_index)
                    .unwrap_or_else(|| match self.zone {
                        EnergyZone::Silent => config.pattern_silent,
                        EnergyZone::Still => config.pattern_still,
                        EnergyZone::Low => config.pattern_low,
                        EnergyZone::Mid => config.pattern_mid,
                        EnergyZone::High => config.pattern_high,
                    });
                messages.push((
                    config.zone_cue_address.clone(),
                    vec![OscType::Int(cue_idx)],
                ));
            }

            // Send pattern index for non-SILENT zones
            // Use genre-specific preferred_patterns if set (>= 0), fall back to config
            if self.zone != EnergyZone::Silent {
                let genre = &config.genre_profile;
                let zone_idx = self.zone.index() as usize; // 1=Still, 2=Low, 3=Mid, 4=High
                let genre_pattern = if zone_idx >= 1 && zone_idx <= 4 {
                    genre.preferred_patterns[zone_idx - 1]
                } else {
                    -1
                };
                let pattern_idx = if genre_pattern >= 0 {
                    genre_pattern
                } else {
                    match self.zone {
                        EnergyZone::Silent => 0,
                        EnergyZone::Still => config.pattern_still,
                        EnergyZone::Low => config.pattern_low,
                        EnergyZone::Mid => config.pattern_mid,
                        EnergyZone::High => config.pattern_high,
                    }
                };
                messages.push((
                    "/react/pattern".to_string(),
                    vec![OscType::Int(pattern_idx)],
                ));
                self.current_pattern = pattern_idx;
            }

            log::info!(
                "[OSC] Zone: {} → {} (zone_energy={:.0}%, fast_energy={:.0}%)",
                old_zone.as_str(),
                self.zone.as_str(),
                self.zone_energy * 100.0,
                self.energy_smooth * 100.0
            );
        }
    }

    fn update_strobe(&mut self, dt: f32, metrics: &LightingMetrics, config: &OscConfig, messages: &mut Vec<(String, Vec<OscType>)>) {
        let genre = &config.genre_profile;

        // Decay timers
        self.strobe_cooldown = (self.strobe_cooldown - dt).max(0.0);
        self.strobe_pulse = (self.strobe_pulse - dt).max(0.0);

        // During BlindersHold (Buildup): suppress normal strobes — the macro handles
        // its own 8Hz strobe via /react/macro/strobe
        if self.macro_phase == AutoBlinderPhase::BlindersHold {
            return;
        }

        // Check if strobe is enabled by genre profile
        if !genre.strobe_enabled {
            return;
        }

        // Style gate: the kick strobe reaches the physical rig via
        // strobe_active and is only THINNED by the license, so the ban must
        // be explicit — Chill never, Energetic only as a drop accent. The
        // manual STROBE trigger bypasses this path entirely (operator wins).
        let arc_is_drop = matches!(
            self.current_arc,
            ArcState::Drop | ArcState::SustainPeak
        );
        if !self.effective_style.allows_kick_strobe(arc_is_drop) {
            return;
        }

        // Normal / Pump / Cool strobe behavior — all use the same logic.
        // Pump doesn't want aggressive overdrive chaos; normal strobe rules apply.
        let zone_ok = genre.strobe_zone_minimum.zone_meets_minimum(self.zone.index());

        // Strobe fires on kick when zone meets minimum, the sensitivity check
        // passes, and the 30s fatigue cap (MAX_STROBES_PER_30S) has headroom.
        // The window counter resets every 30s in update_fatigue_window.
        if zone_ok
            && metrics.kick
            && self.strobe_cooldown <= 0.0
            && self.strobe_count_window < MAX_STROBES_PER_30S
        {
            // Sensitivity roll with slope boost + excitement boost
            let slope_boost = self.get_strobe_slope_boost(config);
            let excitement_boost = if self.excitement > 0.7 { 0.15 } else { 0.0 };
            // Licensed by the music's moment: a vocal bridge thins strobe
            // probability continuously even though the kick grid persists.
            let effective_sensitivity = (genre.strobe_sensitivity + slope_boost + excitement_boost)
                .min(1.0)
                * self.intensity_license;
            let random_val = self.next_strobe_dither();
            if random_val < effective_sensitivity {
                self.strobe_intensity = self.energy_smooth;
                self.strobe_pulse = STROBE_DURATION;
                self.strobe_cooldown = STROBE_COOLDOWN;
                self.strobe_env = 1.0;
                self.strobe_count_window += 1;

                messages.push((
                    "/react/trigger/strobe".to_string(),
                    vec![OscType::Float(self.strobe_intensity)],
                ));

                log::debug!("[OSC] Strobe: intensity={:.2} (genre: {})", self.strobe_intensity, genre.subcategory);
            }
        }

        // Send strobe off when pulse ends
        if self.strobe_pulse <= 0.0 && self.strobe_intensity > 0.0 {
            self.strobe_intensity = 0.0;
        }
    }

    fn update_blackout(&mut self, dt: f32, config: &OscConfig, messages: &mut Vec<(String, Vec<OscType>)>) {
        let genre = &config.genre_profile;

        // Suppress blackout during macro phases. The macro state machine owns
        // the lighting through buildups/drops — a blackout there would kill it.
        if self.macro_phase != AutoBlinderPhase::Normal {
            if self.blackout_active {
                self.blackout_active = false;
                self.blackout_hold_time = 0.0;
                messages.push((
                    "/react/trigger/blackout".to_string(),
                    vec![OscType::Int(0)],
                ));
                log::info!("[OSC] Blackout OFF (suppressed by macro phase={:?})", self.macro_phase);
            }
            return;
        }

        // Genre can disable blackout-on-silence entirely.
        if !genre.blackout_on_silence {
            if self.blackout_active {
                self.blackout_active = false;
                self.blackout_hold_time = 0.0;
                messages.push((
                    "/react/trigger/blackout".to_string(),
                    vec![OscType::Int(0)],
                ));
                log::info!("[OSC] Blackout OFF (genre disabled)");
            }
            return;
        }

        // Blackout follows TRUE raw silence directly via `raw_silence_timer` —
        // NOT the slow zone classifier. The timer resets to 0 the instant any
        // audio returns and counts up through silence, so the stage re-lights
        // and darkens promptly. `blackout_hold_time` debounces the re-light so
        // a one-frame transient cannot blip the stage back on.
        let blackout_delay = genre.blackout_delay_ms as f32 / 1000.0;
        const BLACKOUT_RELIGHT_S: f32 = 0.12;

        if self.raw_silence_timer <= 0.0 {
            // Audio present this frame — re-light once it has briefly held.
            if self.blackout_active {
                self.blackout_hold_time += dt;
                if self.blackout_hold_time >= BLACKOUT_RELIGHT_S {
                    self.blackout_active = false;
                    self.blackout_hold_time = 0.0;
                    messages.push((
                        "/react/trigger/blackout".to_string(),
                        vec![OscType::Int(0)],
                    ));
                    log::info!("[OSC] Blackout OFF (audio returned)");
                }
            }
        } else {
            // Silent this frame — reset the re-light debounce, and engage the
            // blackout once silence has been sustained for the genre delay.
            self.blackout_hold_time = 0.0;
            if self.raw_silence_timer >= blackout_delay && !self.blackout_active {
                self.blackout_active = true;
                messages.push((
                    "/react/trigger/blackout".to_string(),
                    vec![OscType::Int(1)],
                ));
                log::info!(
                    "[OSC] Blackout ON (raw silence {:.2}s)",
                    self.raw_silence_timer
                );
            }
        }
    }

    fn update_harmonic(&mut self, key_signal: &KeySignal, messages: &mut Vec<(String, Vec<OscType>)>) {
        let label = key_signal.harmonic_label;
        if label != self.harmonic_label {
            self.harmonic_label = label;
            let idx = match label {
                HarmonicLabel::Stable => 0,
                HarmonicLabel::Rotating => 1,
                HarmonicLabel::Ambiguous => 2,
                HarmonicLabel::Atonal => 3,
            };
            messages.push((
                "/react/harmonic".to_string(),
                vec![OscType::Int(idx)],
            ));
        }

        let key_root = key_signal.key_root;
        if key_root != self.last_key_root && key_signal.confidence > 0.3 {
            self.last_key_root = key_root;
            messages.push((
                "/react/key".to_string(),
                vec![OscType::Int(key_root as i32)],
            ));
        }
    }

    // ========================================================================
    // REACTIVE LIGHTING FEATURES
    // ========================================================================

    /// Update polarity-based flash effect
    /// Fires on significant quadrant transitions (E↔W or N↔S)
    fn update_polarity_flash(&mut self, dt: f32, metrics: &LightingMetrics, config: &OscConfig, messages: &mut Vec<(String, Vec<OscType>)>) {
        if !config.enable_polarity_flash {
            return;
        }

        // Decay timers
        self.polarity_cooldown = (self.polarity_cooldown - dt).max(0.0);
        self.polarity_flash_timer = (self.polarity_flash_timer - dt).max(0.0);

        // Detect quadrant change (only significant changes: E↔W or N↔S are 2 apart)
        if metrics.polarity_changed && self.polarity_cooldown <= 0.0 {
            let q_diff = (metrics.polarity_quadrant as i8 - self.last_quadrant as i8).abs();
            // Only flash on "opposite" quadrant changes (2 apart on the circle)
            if q_diff == 2 {
                let duration_s = config.polarity_flash_duration_ms as f32 / 1000.0;
                let cooldown_s = config.polarity_flash_cooldown_ms as f32 / 1000.0;

                self.polarity_flash_timer = duration_s;
                self.polarity_cooldown = cooldown_s;
                self.polarity_env = 1.0;

                messages.push((
                    "/react/accent/polarity".to_string(),
                    vec![OscType::Float(0.8)],
                ));
                log::debug!(
                    "[OSC] Polarity flash: Q{} → Q{}",
                    self.last_quadrant,
                    metrics.polarity_quadrant
                );
            }
            self.last_quadrant = metrics.polarity_quadrant;
        }
    }

    /// Update dominant pitch change accent
    /// Fires when the dominant pitch class changes with stability gating
    fn update_dominant_accent(&mut self, dt: f32, metrics: &LightingMetrics, config: &OscConfig, messages: &mut Vec<(String, Vec<OscType>)>) {
        if !config.enable_dominant_accent {
            return;
        }

        // Decay timers
        self.dominant_cooldown = (self.dominant_cooldown - dt).max(0.0);
        self.dominant_flash_timer = (self.dominant_flash_timer - dt).max(0.0);

        // Flash on dominant pitch change (with stability gate and zone minimum)
        if metrics.dominant_changed
            && self.stability_gate_open
            && self.dominant_cooldown <= 0.0
            && self.zone >= EnergyZone::Low
        {
            let duration_s = config.dominant_flash_duration_ms as f32 / 1000.0;
            let cooldown_s = config.dominant_flash_cooldown_ms as f32 / 1000.0;

            self.dominant_flash_timer = duration_s;
            self.dominant_cooldown = cooldown_s;
            self.dominant_env = 1.0;

            // Send pitch class for color mapping on the receiving end
            messages.push((
                "/react/accent/dominant".to_string(),
                vec![OscType::Int(metrics.dominant_pc as i32)],
            ));
            log::debug!(
                "[OSC] Dominant accent: pc={} (was {})",
                metrics.dominant_pc,
                self.last_dominant_pc
            );
        }
        self.last_dominant_pc = metrics.dominant_pc;
    }

    /// Update energy slope detection for build-up/breakdown detection
    /// Positive slope = energy building, negative = dropping
    fn update_energy_slope(&mut self, metrics: &LightingMetrics, config: &OscConfig, messages: &mut Vec<(String, Vec<OscType>)>) {
        if !config.enable_energy_slope {
            self.energy_slope = 0.0;
            return;
        }

        // Store current energy in history buffer
        self.energy_history[self.energy_history_idx] = metrics.energy;
        self.energy_history_idx = (self.energy_history_idx + 1) % 30;

        // Calculate slope: compare recent 10 frames vs older 10 frames
        // This gives us ~166ms vs ~333-500ms ago at 60fps
        let mut recent_sum = 0.0f32;
        let mut older_sum = 0.0f32;

        for i in 0..10 {
            // Recent samples: last 10 written (before current write position)
            let recent_idx = (self.energy_history_idx + 30 - 1 - i) % 30;
            recent_sum += self.energy_history[recent_idx];

            // Older samples: 20-30 positions back
            let older_idx = (self.energy_history_idx + 30 - 20 - i) % 30;
            older_sum += self.energy_history[older_idx];
        }

        let recent_avg = recent_sum / 10.0;
        let older_avg = older_sum / 10.0;

        // Scale slope for sensitivity (raw difference would be -1 to +1 at extreme)
        self.energy_slope = ((recent_avg - older_avg) * 3.0).clamp(-1.0, 1.0);

        // Send slope value periodically (every ~10 frames to reduce OSC traffic)
        if self.energy_history_idx % 10 == 0 {
            messages.push((
                "/react/energy/slope".to_string(),
                vec![OscType::Float(self.energy_slope)],
            ));
        }
    }

    /// Update tonality smoothing (major/minor likelihood)
    fn update_tonality(&mut self, metrics: &LightingMetrics, _config: &OscConfig) {
        // Smooth major likelihood with slow time constant (~400ms)
        self.major_smooth += (metrics.major_likelihood - self.major_smooth) * 0.04;
    }

    /// Advance the strobe-gate dither and return the new value in [0,1).
    ///
    /// Golden-ratio low-discrepancy sequence held in a persistent phase
    /// accumulator: `phase = fract(phase + 0.618034)`. Because the accumulator
    /// never leaves [0,1), f32 precision is constant regardless of uptime.
    /// The previous form — `fract(frame_counter as f32 * 0.618033)` — silently
    /// degraded on a 24/7 stream: once the product exceeds 2^23 (~31-63h at
    /// 60fps) an f32 has no fractional bits left, fract() returns 0.0 forever,
    /// and the `random_val < sensitivity` gate passed on EVERY kick.
    fn next_strobe_dither(&mut self) -> f32 {
        self.strobe_dither = (self.strobe_dither + 0.618_034).fract();
        self.strobe_dither
    }

    /// Get energy slope for strobe boost during build-ups
    fn get_strobe_slope_boost(&self, config: &OscConfig) -> f32 {
        if !config.enable_energy_slope {
            return 0.0;
        }
        // Only boost during positive slope (building energy)
        if self.energy_slope > 0.1 {
            0.2 // Boost strobe sensitivity by 20% during build-ups
        } else {
            0.0
        }
    }

    // ========================================================================
    // MUSICAL ARC: EXCITEMENT, TENSION BANK, FATIGUE, DROP DETECTION
    // ========================================================================

    /// Update excitement ramp — 0-1 value representing how "activated" lighting should be.
    /// Builds slowly during high-energy music, decays during calm passages.
    /// ALL reactive behavior is modulated by excitement.
    fn update_excitement(&mut self, dt: f32) {
        let dt_norm = dt * 60.0; // Normalize to ~60fps frame equivalent

        if self.zone >= EnergyZone::Mid {
            // Active music: build excitement (~5s to reach 0.5 from zero)
            self.excitement += 0.003 * dt_norm;
        } else if self.zone >= EnergyZone::Low {
            // Building energy: slow build (~16s to reach 0.5)
            self.excitement += 0.001 * dt_norm;
        } else {
            // Quiet/silent: decay (~3s to drop from 1.0 to 0.0)
            self.excitement -= 0.005 * dt_norm;
        }

        // Energy slope accelerates excitement during builds
        if self.energy_slope > 0.2 {
            self.excitement += 0.004 * dt_norm;
        }

        // Proportional decay: higher excitement → faster decay.
        // Prevents pegging at 1.0 but not used for buildup detection.
        self.excitement -= 0.005 * self.excitement * dt_norm;

        self.excitement = self.excitement.clamp(0.0, 1.0);
    }

    /// Update tension bank — "currency" that accumulates from musical tension
    /// and gets spent on effects. Prevents infinite strobes.
    fn update_tension_bank(&mut self, dt: f32) {
        let dt_norm = dt * 60.0;

        // Accumulate from tension
        self.tension_bank += self.tension_smooth * 0.002 * dt_norm;
        // Builds add to bank
        self.tension_bank += self.energy_slope.max(0.0) * 0.003 * dt_norm;

        // Natural drain: prevents tension_bank from pegging at 1.0 forever.
        // ~45s to fully drain from 1.0 with no input. Accumulation is faster,
        // so it still fills during builds — but drains during calm/post-drop sections.
        self.tension_bank -= 0.0004 * dt_norm;

        self.tension_bank = self.tension_bank.clamp(0.0, 1.0);
    }

    /// Reset strobe count window every 30 seconds
    fn update_fatigue_window(&mut self) {
        if self.strobe_window_start.elapsed().as_secs_f32() >= 30.0 {
            self.strobe_count_window = 0;
            self.strobe_window_start = Instant::now();
        }
    }

    /// Spend from tension bank if affordable. Returns true if spent.
    fn spend_tension(&mut self, cost: f32) -> bool {
        if self.tension_bank >= cost {
            self.tension_bank -= cost;
            true
        } else {
            false
        }
    }

    /// Check if a "big moment" effect can fire (spacing check)
    fn big_moment_available(&self) -> bool {
        self.last_big_moment.elapsed().as_secs_f32() >= MIN_BIG_MOMENT_SPACING
    }

    /// Extended energy slope: 300-frame (~5s) ring buffer for build/drop detection
    fn update_extended_energy_slope(&mut self, metrics: &LightingMetrics) {
        self.energy_history_long[self.energy_history_long_idx] = metrics.energy;
        self.energy_history_long_idx = (self.energy_history_long_idx + 1) % ENERGY_HISTORY_LONG_LEN;

        // Calculate long slope: compare recent 30 frames vs older 30 frames
        let mut recent_sum = 0.0f32;
        let mut older_sum = 0.0f32;
        for i in 0..30 {
            let recent_idx = (self.energy_history_long_idx + ENERGY_HISTORY_LONG_LEN - 1 - i) % ENERGY_HISTORY_LONG_LEN;
            recent_sum += self.energy_history_long[recent_idx];
            let older_idx = (self.energy_history_long_idx + ENERGY_HISTORY_LONG_LEN - 150 - i) % ENERGY_HISTORY_LONG_LEN;
            older_sum += self.energy_history_long[older_idx];
        }
        let recent_avg = recent_sum / 30.0;
        let older_avg = older_sum / 30.0;
        let long_slope = recent_avg - older_avg;

        // Track consecutive building frames
        if long_slope > 0.02 {
            self.building_frames = self.building_frames.saturating_add(1);
        } else {
            self.building_frames = 0;
        }
    }

    /// Macro state machine: NORMAL → BUILDUP (BlindersHold) → PUMP (OverdriveStrobe) → COOL (Recover) → NORMAL
    ///
    /// Uses dual-EMA energy trend detection (ported from HueManager):
    /// - Fast EMA (tau=0.3s) tracks "where energy is now"
    /// - Slow EMA (tau=10s) tracks "where energy has been"
    /// - When fast > slow by a margin, energy is trending UP = buildup
    /// - No cooldown timer — let the music dictate when buildups happen
    fn update_macro(&mut self, dt: f32, metrics: &LightingMetrics, config: &OscConfig, messages: &mut Vec<(String, Vec<OscType>)>) {
        // The macro runs only when BOTH the operator AUTO master arm and the
        // style posture allow it (Chill = the phase machine never fires; the
        // manual force_buildup() path stays live — operator always wins).
        let macro_armed =
            config.enable_buildup_macro && self.effective_style.allows_macro();
        // === A) UPDATE SMOOTHED SIGNALS ===
        self.macro_low_lp += (metrics.bass - self.macro_low_lp) * MACRO_BAND_ALPHA;
        self.macro_mid_lp += (metrics.mid - self.macro_mid_lp) * MACRO_BAND_ALPHA;
        self.macro_high_lp += (metrics.high - self.macro_high_lp) * MACRO_BAND_ALPHA;

        // Beat confidence with hysteresis (prevents flapping at threshold)
        if self.macro_beat_ok {
            if metrics.beat_confidence < MACRO_BEAT_OK_EXIT {
                self.macro_beat_ok = false;
            }
        } else {
            if metrics.beat_confidence > MACRO_BEAT_OK_ENTER {
                self.macro_beat_ok = true;
            }
        }

        // === B) ARC-DRIVEN BUILD DETECTION ===
        // The show brain's arc classifier (tension rise, bass withdrawal,
        // onset density, beat confidence — debounced, dt-correct) is the
        // single musical authority. BlindersHold keys on PreDropTension: the
        // CREST (bass dropped out, tension plateaued, drop imminent) — not
        // plain Buildup, which is the UV/tension-coil's territory. Tension
        // scenarios that aren't builds no longer fire the white blast.
        let arc = self.current_arc;
        let arc_drop = matches!(arc, ArcState::Drop | ArcState::SustainPeak);
        let drop_edge =
            arc_drop && !matches!(self.last_arc, ArcState::Drop | ArcState::SustainPeak);
        let crest = arc == ArcState::PreDropTension;
        // Raw-silence belt-and-braces: the arc force-Silences on raw silence
        // already; this keeps a stale stamp from entering during a gap.
        let raw_silent = self.raw_silence_timer > 0.0;
        // Crest dwell only accrues while ARMED: otherwise a long disarmed
        // crest banks dwell and re-arming fires BlindersHold the very next
        // frame off a tiny toggle tap. Disarmed = zeroed, so re-arm always
        // requires a fresh MACRO_PREDROP_SUSTAIN_S of held crest.
        if crest
            && !raw_silent
            && macro_armed
            && self.macro_phase == AutoBlinderPhase::Normal
        {
            self.predrop_sustain_s += dt;
        } else if !crest || !macro_armed {
            self.predrop_sustain_s = 0.0;
        }

        // === C) STATE MACHINE TRANSITIONS ===
        let prev_phase = self.macro_phase;

        match self.macro_phase {
            AutoBlinderPhase::Normal => {
                // Both AUTO entries below sit behind the operator master arm
                // (enable_buildup_macro). Disarmed = the phase machine stays
                // silent so the bare show-brain lanes are observable; the
                // manual force_buildup() path stays live.
                //
                // NORMAL → PUMP: the drop arrived without a held crest (subtle
                // build, or the genre opted out of the blinder). EDGE-detected
                // so a long sustained Drop arc enters Pump once, not on every
                // Recover→Normal lap. Deliberately NOT gated on
                // enable_buildup_blinder — Pump is bass-riding brightness,
                // not a blinder strobe, so blinder-averse genres still get a
                // genre-appropriate drop response. drop_detected_this_frame is
                // cleared in process() AFTER update_macro runs, so we see it.
                if self.drop_detected_this_frame || drop_edge {
                    if !macro_armed {
                        // Disarmed: the brain's drop PERCEPTION must survive
                        // even though the performance is silenced. drop_impact
                        // is the arc classifier's only road to ArcState::Drop
                        // and the strobe/wash/prism lanes are drop-gated —
                        // without this pulse a disarmed brain is drop-BLIND
                        // and the "bare lanes" A/B observes a coupling
                        // artifact, not EMeRGE. (Decay is macro-independent,
                        // handled in process().)
                        //
                        // The pulse is atmosphere-shaped like an armed Pump
                        // entry, and it REFRESHES drop_impact_ceiling:
                        // process()'s sustain loop re-pins the level to the
                        // ceiling every high-energy frame, and a disarmed
                        // brain never runs a Pump entry to clear a stale
                        // bloom ceiling.
                        self.drop_impact_ceiling = 0.35 + 0.65 * self.intensity_license;
                        self.drop_impact_level = self.drop_impact_ceiling;
                        log::info!(
                            "[MACRO] disarmed — drop perceived (impact pulse), phase machine silent (arc={:?})",
                            arc
                        );
                    } else {
                        self.macro_phase = AutoBlinderPhase::OverdriveStrobe;
                        self.buildup_phase_timer = 0.0;
                        self.predrop_sustain_s = 0.0;
                        // Atmosphere shapes the detonation: with no percussive
                        // grid a crest is a BLOOM — brightness lifts but stays
                        // under the 0.6 threshold the hardware strobe textures
                        // key on — and Pump's strobe flavor needs BOTH a
                        // strobe-permitting genre and a beat to strobe to.
                        self.drop_impact_ceiling = 0.35 + 0.65 * self.intensity_license;
                        self.drop_impact_level = self.drop_impact_ceiling;
                        self.overdrive_strobe_flavor =
                            config.genre_profile.strobe_enabled && self.intensity_license >= 0.4;

                        messages.push((
                            "/react/macro/phase".to_string(),
                            vec![OscType::Int(AutoBlinderPhase::OverdriveStrobe.index())],
                        ));

                        log::info!(
                            "[MACRO] PUMP (direct drop, no crest held) — arc={:?}, detector_pulse={}",
                            arc, self.drop_detected_this_frame
                        );
                    }
                }
                // NORMAL → BLINDERS HOLD: the arc has held PreDropTension past
                // the edge guard — the drop is loaded; coil the room in white.
                // Gated on the genre profile: ambient-style profiles set
                // enable_buildup_blinder=false and must never get the 8Hz
                // blinder strobe + sustained full-white hold. The manual
                // force_buildup() operator path stays ungated by design.
                else if macro_armed
                    && config.genre_profile.enable_buildup_blinder
                    // Style gate: Energetic keeps the macro for Pump drop
                    // response but never the sustained 8Hz white coil.
                    && self.effective_style.allows_blinder_hold()
                    && !raw_silent
                    // Atmosphere gate: railed-tension walls (post-rock) sit in
                    // PreDropTension-adjacent states constantly — with no
                    // percussive grid + thin spectrum there is no drop to
                    // load, so the white coil must not cycle. Same threshold
                    // as Pump's strobe flavor.
                    && self.intensity_license >= 0.4
                    && self.predrop_sustain_s >= MACRO_PREDROP_SUSTAIN_S
                {
                    self.macro_phase = AutoBlinderPhase::BlindersHold;
                    self.buildup_phase_timer = 0.0;
                    self.predrop_sustain_s = 0.0;
                    self.hold_fizzle_s = 0.0;
                    self.hold_forced = false;

                    messages.push((
                        "/react/macro/phase".to_string(),
                        vec![OscType::Int(AutoBlinderPhase::BlindersHold.index())],
                    ));
                    messages.push((
                        "/triggers/blinder".to_string(),
                        vec![OscType::Bool(true)],
                    ));

                    log::info!(
                        "[MACRO] BUILDUP — arc=PreDropTension held {:.1}s, energy={:.3}",
                        MACRO_PREDROP_SUSTAIN_S, metrics.energy
                    );
                }
            }

            AutoBlinderPhase::BlindersHold => {
                self.buildup_phase_timer += dt;

                if (!macro_armed || !self.effective_style.allows_blinder_hold())
                    && !self.hold_forced
                {
                    // Operator disarmed mid-hold (master arm OR a style that
                    // bans the hold, e.g. Auto→Energetic) — "off means off":
                    // release an ARC-entered hold NOW via the fizzle path
                    // (Recover, no Pump) instead of strobing on for up to
                    // MACRO_HOLD_MAX_S. A force_buildup() hold is exempt: the
                    // operator fired it explicitly, so it plays out.
                    self.macro_phase = AutoBlinderPhase::Recover;
                    self.buildup_phase_timer = 0.0;

                    messages.push((
                        "/react/macro/phase".to_string(),
                        vec![OscType::Int(AutoBlinderPhase::Recover.index())],
                    ));
                    messages.push((
                        "/triggers/blinder".to_string(),
                        vec![OscType::Bool(false)],
                    ));

                    log::info!(
                        "[MACRO] DISARMED mid-hold — releasing to Recover without Pump"
                    );
                } else if self.drop_detected_this_frame || arc_drop {
                    // The drop landed — detonate (atmosphere-shaped, same as
                    // the direct-drop entry).
                    let held_s = self.buildup_phase_timer;
                    self.macro_phase = AutoBlinderPhase::OverdriveStrobe;
                    self.buildup_phase_timer = 0.0;
                    self.drop_impact_ceiling = 0.35 + 0.65 * self.intensity_license;
                    self.drop_impact_level = self.drop_impact_ceiling;
                    self.overdrive_strobe_flavor =
                        config.genre_profile.strobe_enabled && self.intensity_license >= 0.4;

                    messages.push((
                        "/react/macro/phase".to_string(),
                        vec![OscType::Int(AutoBlinderPhase::OverdriveStrobe.index())],
                    ));
                    messages.push((
                        "/triggers/blinder".to_string(),
                        vec![OscType::Bool(false)],
                    ));

                    log::info!(
                        "[MACRO] PUMP — drop landed (arc={:?}), held_buildup={:.1}s",
                        arc, held_s
                    );
                } else {
                    // Fizzle watch: the build dissolved without a drop. Release
                    // gracefully to Recover with NO Pump — the old trend-falloff
                    // exit fired the drop response on every fizzled build.
                    if crest {
                        self.hold_fizzle_s = 0.0;
                    } else {
                        self.hold_fizzle_s += dt;
                    }
                    if self.hold_fizzle_s >= MACRO_FIZZLE_S
                        || self.buildup_phase_timer >= MACRO_HOLD_MAX_S
                    {
                        self.macro_phase = AutoBlinderPhase::Recover;
                        self.buildup_phase_timer = 0.0;

                        messages.push((
                            "/react/macro/phase".to_string(),
                            vec![OscType::Int(AutoBlinderPhase::Recover.index())],
                        ));
                        messages.push((
                            "/triggers/blinder".to_string(),
                            vec![OscType::Bool(false)],
                        ));

                        log::info!(
                            "[MACRO] FIZZLE — build dissolved without a drop (arc={:?}), releasing without Pump",
                            arc
                        );
                    }
                }

                // Emit 8Hz strobe OSC signal during buildup
                let strobe_hz = 8.0;
                let strobe_phase = (self.frame_time * strobe_hz).fract();
                let strobe_val = if strobe_phase < 0.5 { 1.0_f32 } else { 0.0_f32 };
                messages.push((
                    "/react/macro/strobe".to_string(),
                    vec![OscType::Float(strobe_val)],
                ));
            }

            AutoBlinderPhase::OverdriveStrobe => {
                self.buildup_phase_timer += dt;

                // A drop landing OUT of a thin bridge refills the spectrum on
                // the downbeat, but the entry frame still saw the bridge
                // license — let the detonation swell up as the license
                // recovers (ceiling/flavor ratchet UP only; a fading outro
                // doesn't un-detonate).
                self.drop_impact_ceiling = self
                    .drop_impact_ceiling
                    .max(0.35 + 0.65 * self.intensity_license);
                if config.genre_profile.strobe_enabled && self.intensity_license >= 0.4 {
                    self.overdrive_strobe_flavor = true;
                }
                // Style panic cut: flipping to Chill mid-Pump must stop the
                // strobe flavor NOW, not after MACRO_PUMP_HOLD_S rides out
                // (the flavor is latched at entry and only ratchets up).
                if !self.effective_style.allows_macro() {
                    self.overdrive_strobe_flavor = false;
                }

                // Pump hold: stay for MACRO_PUMP_HOLD_S, then transition to Cool
                if self.buildup_phase_timer >= MACRO_PUMP_HOLD_S {
                    self.macro_phase = AutoBlinderPhase::Recover;
                    self.buildup_phase_timer = 0.0;

                    messages.push((
                        "/react/macro/phase".to_string(),
                        vec![OscType::Int(AutoBlinderPhase::Recover.index())],
                    ));

                    log::info!(
                        "[MACRO] COOL — pump held {:.1}s, arc={:?}",
                        MACRO_PUMP_HOLD_S, arc
                    );
                }

                // Emit bass pump OSC signal during pump: bass³ × 1.5
                let bass = metrics.bass.clamp(0.0, 1.0);
                let pump_val = (bass * bass * bass * 1.5).clamp(0.0, 1.0);
                messages.push((
                    "/react/macro/pump".to_string(),
                    vec![OscType::Float(pump_val)],
                ));
            }

            AutoBlinderPhase::Recover => {
                self.buildup_phase_timer += dt;

                // Cool hold: stay for MACRO_COOL_HOLD_S, then return to Normal
                if self.buildup_phase_timer >= MACRO_COOL_HOLD_S {
                    self.macro_phase = AutoBlinderPhase::Normal;
                    self.predrop_sustain_s = 0.0;

                    messages.push((
                        "/react/macro/phase".to_string(),
                        vec![OscType::Int(AutoBlinderPhase::Normal.index())],
                    ));

                    log::info!("[MACRO] NORMAL — cool phase complete");
                }
            }
        }

        // Drop-edge tracking for the next frame (after all arms have seen it).
        self.last_arc = arc;

        // === D) PERIODIC DIAGNOSTIC (~every 2s) ===
        if self.frame_counter % 120 == 0
            && self.macro_phase != AutoBlinderPhase::Normal
        {
            log::info!(
                "[MACRO] phase={}, arc={:?}, low={:.3}, mid={:.3}, high={:.3}, beat_ok={}, phase_t={:.1}, fizzle={:.1}",
                self.macro_phase.as_str(),
                arc,
                self.macro_low_lp, self.macro_mid_lp, self.macro_high_lp,
                self.macro_beat_ok, self.buildup_phase_timer, self.hold_fizzle_s
            );
        }
        if self.frame_counter % 120 == 0
            && self.macro_phase == AutoBlinderPhase::Normal
            && self.predrop_sustain_s > 0.0
        {
            log::info!(
                "[MACRO] crest holding {:.2}s/{:.1}s (arc={:?}, energy={:.3})",
                self.predrop_sustain_s, MACRO_PREDROP_SUSTAIN_S, arc, metrics.energy
            );
        }

        // Log phase transitions
        if self.macro_phase != prev_phase {
            // (transition already logged above in each arm)
        }
    }

    /// Detect drops: sustained build followed by sudden energy jump
    fn update_drop_detection(&mut self, messages: &mut Vec<(String, Vec<OscType>)>) {
        if self.building_frames < DROP_BUILD_FRAMES {
            return;
        }

        // Check for energy jump in last 15 frames
        let mut min_energy = f32::MAX;
        let mut max_energy = f32::MIN;
        for i in 0..15 {
            let idx = (self.energy_history_long_idx + ENERGY_HISTORY_LONG_LEN - 1 - i) % ENERGY_HISTORY_LONG_LEN;
            let e = self.energy_history_long[idx];
            min_energy = min_energy.min(e);
            max_energy = max_energy.max(e);
        }

        if (max_energy - min_energy) > DROP_ENERGY_JUMP {
            // DROP DETECTED — check if we can afford it
            if self.tension_bank >= DROP_COST && self.big_moment_available() {
                // Drop spends excitement (the buildup energy was "released").
                // Brief spike to 0.85 then the natural decay brings it down,
                // allowing the next buildup to register as excitement rises again.
                self.excitement = 0.85;
                self.tension_bank -= DROP_COST;
                self.last_big_moment = Instant::now();
                self.building_frames = 0;
                self.drop_detected_this_frame = true;

                messages.push((
                    "/react/drop".to_string(),
                    vec![OscType::Float(1.0)],
                ));

                log::info!(
                    "[OSC] DROP DETECTED — excitement=1.0, tension_bank={:.2}",
                    self.tension_bank
                );
            }
        }
    }

    // ========================================================================
    // TIMED PALETTE/PATTERN CYCLING
    // ========================================================================

    /// Cycle palettes on a timer based on genre `palette_change_rate`.
    ///
    /// `palette_change_rate` is a reactivity MULTIPLIER (genre.rs documents
    /// 0.5 = slow, 2.0 = fast), so the interval is
    /// `PALETTE_CYCLE_BASE_INTERVAL_S / rate` — rate 2.0 cycles twice as fast
    /// as 1.0, rate 0.5 half as fast. (A previous version treated the value
    /// as a 0-1 rate lerped onto 60s..10s, which clamped every profile above
    /// 1.0 onto the same 10s interval.)
    ///
    /// When a cycle fires, the balance-driven palette machine is suppressed
    /// for PALETTE_CYCLE_HOLD_S via `palette_cycle_hold_until` so the cycled
    /// palette actually survives (see update_palette).
    fn update_palette_cycling(&mut self, dt: f32, config: &OscConfig, messages: &mut Vec<(String, Vec<OscType>)>) {
        let rate = config.genre_profile.palette_change_rate;
        if rate <= 0.0 {
            return;
        }

        self.palette_change_timer += dt;
        // Interval = base / rate. Rate clamped so a malformed profile can't
        // produce a near-zero or near-infinite interval (0.1 → 300s, 4.0 → 7.5s).
        let interval = PALETTE_CYCLE_BASE_INTERVAL_S / rate.clamp(0.1, 4.0);

        if self.palette_change_timer >= interval {
            self.palette_change_timer = 0.0;
            // Hold off the balance-driven machine so the cycle sticks.
            self.palette_cycle_hold_until = self.frame_time + PALETTE_CYCLE_HOLD_S;

            // Cycle to next palette: Ice → CoolA → CoolB → Ice
            let next = match self.palette {
                Palette::Ice => Palette::CoolA,
                Palette::CoolA => Palette::CoolB,
                Palette::CoolB => Palette::Ice,
            };
            self.palette = next;

            let palette_idx = match self.palette {
                Palette::Ice => config.palette_ice_index,
                Palette::CoolA => config.palette_cool_a_index,
                Palette::CoolB => config.palette_cool_b_index,
            };
            messages.push((
                "/react/palette".to_string(),
                vec![OscType::Int(palette_idx)],
            ));

            let palette_name = match self.palette {
                Palette::Ice => config.palette_ice_name.clone(),
                Palette::CoolA => config.palette_cool_a_name.clone(),
                Palette::CoolB => config.palette_cool_b_name.clone(),
            };
            messages.push((
                "/react/palette/name".to_string(),
                vec![OscType::String(palette_name.clone())],
            ));

            log::info!(
                "[OSC] Palette cycled: {} idx={} ({}) (rate={:.1}, interval={:.0}s)",
                self.palette.as_str(), palette_idx, palette_name, rate, interval
            );
        }
    }

    /// Cycle patterns on a timer based on genre pattern_change_rate.
    /// Rate 0.0 = every 60s, rate 1.0 = every 10s.
    fn update_pattern_cycling(&mut self, dt: f32, config: &OscConfig, messages: &mut Vec<(String, Vec<OscType>)>) {
        let rate = config.genre_profile.pattern_change_rate;
        if rate <= 0.0 || self.zone == EnergyZone::Silent {
            return;
        }

        self.pattern_change_timer += dt;
        let interval = 60.0 + (10.0 - 60.0) * rate.clamp(0.0, 1.0);

        if self.pattern_change_timer >= interval {
            self.pattern_change_timer = 0.0;

            // Pick next pattern from genre's preferred set for the current zone
            let genre = &config.genre_profile;
            let zone_idx = self.zone.index() as usize;
            if zone_idx >= 1 && zone_idx <= 4 {
                let genre_pattern = genre.preferred_patterns[zone_idx - 1];
                if genre_pattern >= 0 && genre_pattern != self.current_pattern {
                    self.current_pattern = genre_pattern;
                    messages.push((
                        "/react/pattern".to_string(),
                        vec![OscType::Int(genre_pattern)],
                    ));
                    log::info!(
                        "[OSC] Pattern cycled: idx={} zone={} (rate={:.1}, interval={:.0}s)",
                        genre_pattern, self.zone.as_str(), rate, interval
                    );
                }
            }
        }
    }
}

impl Default for CueEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// OscConfig with the default genre profile (Tech House: strobe_enabled,
    /// strobe_zone_minimum=Mid, enable_buildup_blinder=true).
    fn test_config() -> OscConfig {
        OscConfig::default()
    }

    /// Metrics with audible raw energy (well above RAW_SILENCE_THRESHOLD)
    /// and the given normalized energy.
    fn metrics_with_energy(energy: f32) -> LightingMetrics {
        LightingMetrics {
            energy,
            raw_energy: 0.01,
            ..Default::default()
        }
    }

    // === Finding: strobe dither must stay precise at any uptime =============

    #[test]
    fn strobe_dither_stays_in_unit_interval_and_varied() {
        let mut engine = CueEngine::new();
        // Seed the accumulator at arbitrary points — the update rule is
        // uptime-independent, which is the fix. (The old form,
        // fract(frame_counter * 0.618033), collapsed to a constant 0.0 once
        // the product crossed 2^23 ≈ 31-63h of 60fps uptime; we verify the
        // RULE here instead of grinding 13M iterations.)
        for seed in [0.0_f32, 0.123, 0.5, 0.618, 0.999_999] {
            engine.strobe_dither = seed;
            let mut min = f32::MAX;
            let mut max = f32::MIN;
            let mut below_half = 0u32;
            const N: u32 = 1000;
            for _ in 0..N {
                let v = engine.next_strobe_dither();
                assert!(
                    (0.0..1.0).contains(&v),
                    "dither {} escaped [0,1) from seed {}",
                    v,
                    seed
                );
                min = min.min(v);
                max = max.max(v);
                if v < 0.5 {
                    below_half += 1;
                }
            }
            // Low-discrepancy: the sequence must cover [0,1) evenly — a
            // collapsed accumulator would pin min == max.
            assert!(min < 0.05, "min {} too high from seed {}", min, seed);
            assert!(max > 0.95, "max {} too low from seed {}", max, seed);
            assert!(
                (400..=600).contains(&below_half),
                "distribution skewed: {}/{} below 0.5 from seed {}",
                below_half,
                N,
                seed
            );
        }
    }

    // === Finding: MAX_STROBES_PER_30S fatigue cap must be enforced ==========

    #[test]
    fn strobe_fatigue_cap_limits_strobes_per_window() {
        let mut engine = CueEngine::new();
        let mut config = test_config();
        config.genre_profile.strobe_enabled = true;
        config.genre_profile.strobe_sensitivity = 1.0; // dither gate always passes
        engine.zone = EnergyZone::High;

        let metrics = LightingMetrics {
            kick: true,
            energy: 0.9,
            raw_energy: 0.01,
            ..Default::default()
        };

        let mut fired = 0usize;
        for _ in 0..(MAX_STROBES_PER_30S as usize * 5) {
            let mut messages = Vec::new();
            // dt=1.0 clears the 0.4s cooldown between attempts. The 30s
            // fatigue window is wall-clock (Instant) and update_fatigue_window
            // is not called here, so the window cannot reset mid-test.
            engine.update_strobe(1.0, &metrics, &config, &mut messages);
            fired += messages
                .iter()
                .filter(|(a, _)| a == "/react/trigger/strobe")
                .count();
        }
        assert_eq!(
            fired,
            MAX_STROBES_PER_30S as usize,
            "fatigue cap must bound strobes per 30s window"
        );
        assert_eq!(engine.strobe_count_window, MAX_STROBES_PER_30S);
    }

    // === Finding: enable_buildup_blinder=false must block auto-buildup ======

    /// Drive `update_macro` for `frames` at 60fps with a fixed arc + metrics.
    fn run_macro(
        engine: &mut CueEngine,
        config: &OscConfig,
        arc: ArcState,
        metrics: &LightingMetrics,
        frames: usize,
    ) -> Vec<(String, Vec<OscType>)> {
        let mut all = Vec::new();
        for _ in 0..frames {
            engine.set_arc(arc);
            let mut messages = Vec::new();
            engine.update_macro(1.0 / 60.0, metrics, config, &mut messages);
            all.extend(messages);
        }
        all
    }

    #[test]
    fn buildup_blinder_disabled_genre_never_enters_blinders_hold() {
        let mut engine = CueEngine::new();
        let mut config = test_config();
        config.genre_profile.enable_buildup_blinder = false;

        // A held crest — the exact condition that enters BlindersHold.
        let metrics = metrics_with_energy(0.85);
        let msgs = run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 120);
        assert!(
            msgs.iter().all(|(a, _)| a != "/triggers/blinder"),
            "blinder message emitted despite enable_buildup_blinder=false"
        );
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::Normal,
            "genre with enable_buildup_blinder=false must never auto-enter BlindersHold"
        );

        // Control: identical crest with the flag on must enter the buildup.
        let mut engine = CueEngine::new();
        config.genre_profile.enable_buildup_blinder = true;
        run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 30);
        assert_eq!(engine.macro_phase, AutoBlinderPhase::BlindersHold);

        // Manual operator path stays ungated regardless of the genre flag.
        let mut engine = CueEngine::new();
        let msgs = engine.force_buildup();
        assert_eq!(engine.macro_phase, AutoBlinderPhase::BlindersHold);
        assert!(!msgs.is_empty());
    }

    // === Finding: enable_buildup_macro=false (master arm) silences ALL auto entries

    #[test]
    fn buildup_macro_disarmed_blocks_all_auto_entries() {
        let mut engine = CueEngine::new();
        let mut config = test_config(); // enable_buildup_blinder = true
        config.enable_buildup_macro = false;
        let metrics = metrics_with_energy(0.85);

        // A held crest — the BlindersHold entry — must not fire while disarmed,
        // even with the genre blinder flag on.
        let msgs = run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 120);
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::Normal,
            "disarmed macro must never auto-enter BlindersHold"
        );
        assert!(
            msgs.iter().all(|(a, _)| a != "/triggers/blinder" && a != "/react/macro/phase"),
            "no macro messages while disarmed"
        );

        // A direct drop edge — the Pump entry — must not fire either. Groove
        // frames first so Drop arrives as a real edge.
        run_macro(&mut engine, &config, ArcState::Groove, &metrics, 10);
        let msgs = run_macro(&mut engine, &config, ArcState::Drop, &metrics, 30);
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::Normal,
            "disarmed macro must never auto-enter Pump on a drop"
        );
        assert!(msgs.iter().all(|(a, _)| a != "/react/macro/phase"));

        // Control: identical crest with the master arm on enters the hold.
        let mut engine = CueEngine::new();
        config.enable_buildup_macro = true;
        run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 30);
        assert_eq!(engine.macro_phase, AutoBlinderPhase::BlindersHold);
    }

    #[test]
    fn buildup_macro_disarmed_keeps_manual_force_and_in_flight_sequence() {
        // The master arm gates ENTRY, not the operator or an in-flight
        // sequence: force_buildup still coils the room, and the forced hold
        // still detonates into Pump when the drop lands.
        let mut engine = CueEngine::new();
        let mut config = test_config();
        config.enable_buildup_macro = false;
        let metrics = metrics_with_energy(0.85);

        let msgs = engine.force_buildup();
        assert_eq!(engine.macro_phase, AutoBlinderPhase::BlindersHold);
        assert!(!msgs.is_empty());

        // Hold rides through non-crest frames (fizzle window not yet elapsed)…
        run_macro(&mut engine, &config, ArcState::Groove, &metrics, 5);
        assert_eq!(engine.macro_phase, AutoBlinderPhase::BlindersHold);

        // …and the drop still detonates the forced sequence while disarmed.
        run_macro(&mut engine, &config, ArcState::Drop, &metrics, 2);
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::OverdriveStrobe,
            "forced hold must still pump on the drop while disarmed"
        );
    }

    #[test]
    fn buildup_macro_disarmed_drop_perception_survives() {
        // Disarming silences the PERFORMANCE, not the brain's perception:
        // drop_impact is the arc classifier's only road to ArcState::Drop and
        // the strobe/wash/prism lanes are drop-gated, so a disarmed drop must
        // still pulse it — otherwise the "bare lanes" A/B observes a
        // drop-blind brain (a coupling artifact), not EMeRGE.
        let mut engine = CueEngine::new();
        let mut config = test_config();
        config.enable_buildup_macro = false;
        let metrics = metrics_with_energy(0.85);

        run_macro(&mut engine, &config, ArcState::Groove, &metrics, 10);
        assert_eq!(engine.drop_impact_level, 0.0);

        let msgs = run_macro(&mut engine, &config, ArcState::Drop, &metrics, 2);
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::Normal,
            "phase machine stays silent while disarmed"
        );
        assert!(
            (engine.drop_impact_level - 1.0).abs() < 1e-6,
            "disarmed brain must still perceive the drop"
        );
        assert!(msgs.iter().all(|(a, _)| a != "/react/macro/phase"));
    }

    #[test]
    fn buildup_macro_rearm_requires_fresh_crest() {
        // Crest dwell must not bank while disarmed — otherwise tapping AUTO
        // mid-crest slams the room white the very next frame off a toggle the
        // operator reads as an arm switch, not a fire button.
        let mut engine = CueEngine::new();
        let mut config = test_config();
        config.enable_buildup_macro = false;
        let metrics = metrics_with_energy(0.85);

        run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 240);
        assert_eq!(
            engine.predrop_sustain_s, 0.0,
            "crest dwell must not accrue while disarmed"
        );

        // Re-arm mid-crest: no instant fire…
        config.enable_buildup_macro = true;
        run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 6); // 0.1s
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::Normal,
            "re-arm must not fire off banked dwell"
        );

        // …until a fresh MACRO_PREDROP_SUSTAIN_S of held crest accrues.
        run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 30);
        assert_eq!(engine.macro_phase, AutoBlinderPhase::BlindersHold);
    }

    #[test]
    fn buildup_macro_disarm_releases_auto_hold_but_not_forced_hold() {
        // "Off means off": disarming mid-hold releases an ARC-entered
        // BlindersHold immediately via the fizzle path (Recover, no Pump)
        // instead of strobing on for up to MACRO_HOLD_MAX_S.
        let mut engine = CueEngine::new();
        let mut config = test_config();
        let metrics = metrics_with_energy(0.85);

        run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 30);
        assert_eq!(engine.macro_phase, AutoBlinderPhase::BlindersHold);

        config.enable_buildup_macro = false;
        let msgs = run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 1);
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::Recover,
            "disarm must release an arc-entered hold immediately, without Pump"
        );
        assert!(
            msgs.iter().any(|(a, args)| a == "/triggers/blinder"
                && matches!(args.first(), Some(OscType::Bool(false)))),
            "release must drop the blinder signal"
        );

        // A hold the operator forced explicitly is exempt and plays out.
        let mut engine = CueEngine::new();
        engine.force_buildup();
        run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 30);
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::BlindersHold,
            "forced hold must survive a disarm"
        );
    }

    // === Operator style postures (Auto/Chill/Energetic/Banger) ==============

    #[test]
    fn chill_bans_kick_strobe_and_macro() {
        // Chill on a strobe-permitting genre with a kick storm in the High
        // zone: the kick strobe (which reaches the physical rig via
        // strobe_active) and the whole macro phase machine must stay silent —
        // but drop PERCEPTION survives, atmosphere-shaped.
        let mut engine = CueEngine::new();
        let mut config = test_config();
        config.genre_profile.strobe_enabled = true;
        config.genre_profile.strobe_sensitivity = 1.0;
        engine.set_effective_style(crate::emerge::LightingStyle::Chill);
        engine.set_intensity_license(0.15); // what the show brain clamps to
        engine.zone = EnergyZone::High;

        let kick_metrics = LightingMetrics {
            kick: true,
            energy: 0.9,
            raw_energy: 0.01,
            ..Default::default()
        };
        let mut fired = 0usize;
        for _ in 0..120 {
            let mut messages = Vec::new();
            engine.update_strobe(1.0, &kick_metrics, &config, &mut messages);
            fired += messages
                .iter()
                .filter(|(a, _)| a == "/react/trigger/strobe")
                .count();
        }
        assert_eq!(fired, 0, "chill never fires the kick strobe");

        // Held crest then drop: no BlindersHold, no Pump — but the drop
        // pulse still registers (bloom-shaped by the chill license).
        let metrics = metrics_with_energy(0.85);
        run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 120);
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::Normal,
            "chill never auto-enters BlindersHold"
        );
        run_macro(&mut engine, &config, ArcState::Groove, &metrics, 10);
        run_macro(&mut engine, &config, ArcState::Drop, &metrics, 2);
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::Normal,
            "chill never auto-enters Pump"
        );
        let expected_bloom = 0.35 + 0.65 * 0.15;
        assert!(
            (engine.drop_impact_level - expected_bloom).abs() < 1e-6,
            "chill still perceives the drop as a bloom, got {}",
            engine.drop_impact_level
        );
    }

    #[test]
    fn energetic_macro_pumps_without_blinder_hold() {
        // Energetic keeps the macro for drop response but never the
        // sustained 8Hz white coil: a held crest that enters BlindersHold in
        // Auto stays Normal, while a real drop edge still detonates Pump.
        let mut engine = CueEngine::new();
        let config = test_config();
        engine.set_effective_style(crate::emerge::LightingStyle::Energetic);
        let metrics = metrics_with_energy(0.85);

        run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 120);
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::Normal,
            "energetic never auto-enters BlindersHold"
        );

        run_macro(&mut engine, &config, ArcState::Groove, &metrics, 10);
        run_macro(&mut engine, &config, ArcState::Drop, &metrics, 2);
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::OverdriveStrobe,
            "energetic still pumps on the drop"
        );
    }

    #[test]
    fn energetic_kick_strobe_drop_only() {
        // The zone-gated kick strobe is a sustained-section texture in Auto;
        // Energetic restricts it to drop accents.
        let mut engine = CueEngine::new();
        let mut config = test_config();
        config.genre_profile.strobe_enabled = true;
        config.genre_profile.strobe_sensitivity = 1.0;
        engine.set_effective_style(crate::emerge::LightingStyle::Energetic);
        engine.zone = EnergyZone::High;

        let kick_metrics = LightingMetrics {
            kick: true,
            energy: 0.9,
            raw_energy: 0.01,
            ..Default::default()
        };
        let fire_count = |engine: &mut CueEngine, arc: ArcState, frames: usize| {
            engine.set_arc(arc);
            let mut fired = 0usize;
            for _ in 0..frames {
                let mut messages = Vec::new();
                engine.update_strobe(1.0, &kick_metrics, &config, &mut messages);
                fired += messages
                    .iter()
                    .filter(|(a, _)| a == "/react/trigger/strobe")
                    .count();
            }
            fired
        };

        assert_eq!(
            fire_count(&mut engine, ArcState::Groove, 30),
            0,
            "energetic kick strobe stays silent outside the drop"
        );
        assert!(
            fire_count(&mut engine, ArcState::Drop, 30) > 0,
            "energetic kick strobe fires as a drop accent"
        );
    }

    #[test]
    fn mid_hold_style_switch_releases_hold() {
        // Switching Auto -> Energetic during an arc-entered BlindersHold
        // releases the 8Hz coil immediately via the fizzle path (Recover, no
        // Pump); a force_buildup() hold is operator-fired and plays out.
        let mut engine = CueEngine::new();
        let config = test_config();
        let metrics = metrics_with_energy(0.85);

        run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 30);
        assert_eq!(engine.macro_phase, AutoBlinderPhase::BlindersHold);

        engine.set_effective_style(crate::emerge::LightingStyle::Energetic);
        let msgs = run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 1);
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::Recover,
            "style switch must release an arc-entered hold without Pump"
        );
        assert!(
            msgs.iter().any(|(a, args)| a == "/triggers/blinder"
                && matches!(args.first(), Some(OscType::Bool(false)))),
            "release must drop the blinder signal"
        );

        // Forced hold: exempt, even under a hold-banning style.
        let mut engine = CueEngine::new();
        engine.force_buildup();
        run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 30);
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::BlindersHold,
            "forced hold must survive a style ban"
        );
    }

    // === The conductor/performer split: only the arc decides what a build is =

    #[test]
    fn energy_swell_alone_never_fires_the_blinder() {
        // THE retired failure mode: tension scenarios / energy swells used to
        // trip the dual-EMA trend detector and spend the white blast during
        // the coil. With arc-driven entry, even a full Buildup classification
        // doesn't blind the room — only the PreDropTension CREST does.
        let mut engine = CueEngine::new();
        let config = test_config(); // enable_buildup_blinder = true
        let hot = metrics_with_energy(0.95);

        for arc in [ArcState::Groove, ArcState::Buildup, ArcState::Breakdown, ArcState::Cooldown] {
            let msgs = run_macro(&mut engine, &config, arc, &hot, 240); // 4s each
            assert_eq!(
                engine.macro_phase,
                AutoBlinderPhase::Normal,
                "arc {arc:?} with hot energy must not enter the macro"
            );
            assert!(
                msgs.iter().all(|(a, _)| a != "/triggers/blinder"),
                "no blinder during {arc:?}"
            );
        }
    }

    #[test]
    fn predrop_crest_enters_hold_and_drop_detonates() {
        let mut engine = CueEngine::new();
        let config = test_config();
        let metrics = metrics_with_energy(0.8);

        // A 1-frame PreDropTension blip is classification flicker — no entry.
        run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 1);
        run_macro(&mut engine, &config, ArcState::Buildup, &metrics, 10);
        assert_eq!(engine.macro_phase, AutoBlinderPhase::Normal, "1-frame crest blip ignored");

        // A held crest (>= MACRO_PREDROP_SUSTAIN_S) coils the room.
        let msgs = run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 30);
        assert_eq!(engine.macro_phase, AutoBlinderPhase::BlindersHold);
        assert!(
            msgs.iter()
                .any(|(a, args)| a == "/triggers/blinder"
                    && matches!(args.first(), Some(OscType::Bool(true)))),
            "hold announces the blinder"
        );

        // The drop lands — detonate: Pump + drop impact pinned + blinder off.
        let msgs = run_macro(&mut engine, &config, ArcState::Drop, &metrics, 2);
        assert_eq!(engine.macro_phase, AutoBlinderPhase::OverdriveStrobe);
        assert_eq!(engine.drop_impact_level, 1.0);
        assert!(
            msgs.iter()
                .any(|(a, args)| a == "/triggers/blinder"
                    && matches!(args.first(), Some(OscType::Bool(false)))),
            "detonation releases the blinder"
        );
    }

    #[test]
    fn fizzled_build_releases_without_pump() {
        // A build that dissolves instead of dropping must release gracefully:
        // Recover, blinder off, and NO drop response — the old trend-falloff
        // exit fired the Pump on every fizzle.
        let mut engine = CueEngine::new();
        let config = test_config();
        let metrics = metrics_with_energy(0.7);

        run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 30);
        assert_eq!(engine.macro_phase, AutoBlinderPhase::BlindersHold);

        // Crest dissolves into Groove for >= MACRO_FIZZLE_S.
        let msgs = run_macro(&mut engine, &config, ArcState::Groove, &metrics, 70);
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::Recover,
            "fizzle releases to Recover, not Pump"
        );
        assert_eq!(engine.drop_impact_level, 0.0, "no drop impact on a fizzle");
        assert!(
            msgs.iter()
                .any(|(a, args)| a == "/triggers/blinder"
                    && matches!(args.first(), Some(OscType::Bool(false)))),
            "fizzle releases the blinder"
        );
        assert!(
            msgs.iter().all(|(a, args)| !(a == "/react/macro/phase"
                && matches!(args.first(),
                    Some(OscType::Int(i)) if *i == AutoBlinderPhase::OverdriveStrobe.index()))),
            "no Pump phase announced on a fizzle"
        );
    }

    #[test]
    fn stalled_crest_hits_the_hold_ceiling() {
        // A DJ riding the filter forever: PreDropTension never resolves. The
        // hold must release at MACRO_HOLD_MAX_S regardless.
        let mut engine = CueEngine::new();
        let config = test_config();
        let metrics = metrics_with_energy(0.8);

        run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 30);
        assert_eq!(engine.macro_phase, AutoBlinderPhase::BlindersHold);

        let frames = (MACRO_HOLD_MAX_S * 60.0) as usize + 10;
        run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, frames);
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::Recover,
            "stalled crest released at the hold ceiling"
        );
        assert_eq!(engine.drop_impact_level, 0.0, "ceiling release is not a drop");
    }

    #[test]
    fn long_sustained_drop_enters_pump_once() {
        // The arc holds Drop for a long section: Pump fires on the EDGE, runs
        // its course (Pump 4s + Recover 3s), and must NOT retrigger while the
        // same drop persists — only a fresh drop edge re-enters.
        let mut engine = CueEngine::new();
        let config = test_config();
        let metrics = metrics_with_energy(0.9);

        run_macro(&mut engine, &config, ArcState::Groove, &metrics, 10);
        run_macro(&mut engine, &config, ArcState::Drop, &metrics, 2);
        assert_eq!(engine.macro_phase, AutoBlinderPhase::OverdriveStrobe, "drop edge pumps");

        // Ride the same Drop arc through Pump + Recover and back to Normal.
        let frames = ((MACRO_PUMP_HOLD_S + MACRO_COOL_HOLD_S) * 60.0) as usize + 20;
        run_macro(&mut engine, &config, ArcState::Drop, &metrics, frames);
        assert_eq!(engine.macro_phase, AutoBlinderPhase::Normal);
        run_macro(&mut engine, &config, ArcState::Drop, &metrics, 120);
        assert_eq!(
            engine.macro_phase,
            AutoBlinderPhase::Normal,
            "sustained drop must not retrigger Pump without a fresh edge"
        );

        // A fresh edge (leave the drop, come back) re-enters.
        run_macro(&mut engine, &config, ArcState::Cooldown, &metrics, 10);
        run_macro(&mut engine, &config, ArcState::Drop, &metrics, 2);
        assert_eq!(engine.macro_phase, AutoBlinderPhase::OverdriveStrobe, "fresh edge pumps again");
    }

    #[test]
    fn atmospheric_pump_is_a_bloom_not_a_detonation() {
        // A post-rock crescendo crest: drop arrives but the material has no
        // percussive grid. Pump still runs (brightness lifts) but the impact
        // ceiling stays under the 0.6 threshold the hardware strobe textures
        // key on, and the strobe FLAVOR is withheld — even for a
        // strobe-permitting genre. The sustain loop must hold the bloom
        // ceiling, not re-pin 1.0.
        let mut engine = CueEngine::new();
        let mut config = test_config();
        config.genre_profile.strobe_enabled = true;
        engine.set_intensity_license(0.1);

        let metrics = metrics_with_energy(0.9);
        run_macro(&mut engine, &config, ArcState::Drop, &metrics, 2);
        assert_eq!(engine.macro_phase, AutoBlinderPhase::OverdriveStrobe, "the bloom still pumps");
        assert!(
            engine.drop_impact_level < 0.6,
            "atmospheric crest blooms under the strobe-texture threshold, got {}",
            engine.drop_impact_level
        );
        assert!(
            !engine.overdrive_strobe_active(),
            "no strobe flavor without a percussive grid"
        );

        // EDM control: full percussiveness + strobe-permitting genre = the
        // classic detonation.
        let mut engine = CueEngine::new();
        engine.set_intensity_license(1.0);
        run_macro(&mut engine, &config, ArcState::Drop, &metrics, 2);
        assert_eq!(engine.drop_impact_level, 1.0);
        assert!(engine.overdrive_strobe_active(), "percussive drop keeps the strobe flavor");

        // Strobe-banning genre withholds the flavor even on percussive drops.
        let mut engine = CueEngine::new();
        engine.set_intensity_license(1.0);
        config.genre_profile.strobe_enabled = false;
        run_macro(&mut engine, &config, ArcState::Drop, &metrics, 2);
        assert_eq!(engine.drop_impact_level, 1.0, "brightness ride is genre-independent");
        assert!(!engine.overdrive_strobe_active(), "genre strobe ban gates the flavor");
    }

    #[test]
    fn adaptive_tension_breathes_where_absolute_tension_rails() {
        // Wall-of-sound material rails absolute tension at ~1.0 for minutes.
        // The adaptive signal must decay toward 0 as the baseline catches up,
        // then spike when tension steps ABOVE the established wall.
        let mut engine = CueEngine::new();
        // Drive the smoothing + baseline directly (process() uses wall-clock
        // dt; the tension math is private-field testable like the macro).
        let dt = 1.0 / 30.0;
        let step = |e: &mut CueEngine, tension: f32, secs: f32| {
            let frames = (secs / dt) as usize;
            for _ in 0..frames {
                e.tension_smooth += (tension - e.tension_smooth) * SMOOTH_TENSION;
                let a = 1.0 - (-dt / TENSION_BASE_TAU_S).exp();
                e.tension_base += (e.tension_smooth - e.tension_base) * a;
                e.tension_dyn =
                    ((e.tension_smooth - e.tension_base) * TENSION_DYN_GAIN).clamp(0.0, 1.0);
            }
        };

        step(&mut engine, 1.0, 5.0);
        let early = engine.tension_dyn();
        assert!(early > 0.3, "fresh tension reads dynamic, got {early}");

        step(&mut engine, 1.0, 90.0); // the wall persists
        assert!(
            engine.tension_dyn() < 0.25,
            "railed tension adapts away, got {}",
            engine.tension_dyn()
        );
    }

    #[test]
    fn stale_crest_arc_during_raw_silence_does_not_enter() {
        // Belt-and-braces: the arc force-Silences on raw silence upstream,
        // but a stale PreDropTension stamp across the thread seam must not
        // coil the room during a quiet gap.
        let mut engine = CueEngine::new();
        let config = test_config();
        engine.raw_silence_timer = 2.0;
        let metrics = metrics_with_energy(0.5);
        run_macro(&mut engine, &config, ArcState::PreDropTension, &metrics, 120);
        assert_eq!(engine.macro_phase, AutoBlinderPhase::Normal);
    }

    // === Finding: drop with no buildup must get a macro response ============

    #[test]
    fn drop_without_buildup_enters_pump_and_pins_drop_impact() {
        let mut engine = CueEngine::new();
        let mut config = test_config();
        // Even with the buildup blinder disabled — Pump is bass-riding
        // brightness, not a blinder, so the genre opt-out must not block it.
        config.genre_profile.enable_buildup_blinder = false;

        engine.drop_detected_this_frame = true;
        let metrics = metrics_with_energy(0.9);
        let mut messages = Vec::new();
        engine.update_macro(1.0 / 60.0, &metrics, &config, &mut messages);

        assert_eq!(engine.macro_phase, AutoBlinderPhase::OverdriveStrobe);
        assert_eq!(engine.drop_impact_level, 1.0);
        assert!(
            messages.iter().any(|(a, args)| a == "/react/macro/phase"
                && matches!(args.first(),
                    Some(OscType::Int(i)) if *i == AutoBlinderPhase::OverdriveStrobe.index())),
            "drop response must announce the Pump phase over OSC"
        );
    }

    // === Finding: timed palette cycle vs balance-driven palette machine =====

    #[test]
    fn cycled_palette_survives_hold_window_under_steady_balance() {
        let mut engine = CueEngine::new();
        let config = test_config();

        // Steady mid-range balance: the balance machine wants CoolA.
        engine.balance_smooth = 0.55;
        engine.palette = Palette::CoolA;
        engine.frame_time = 100.0;

        // Fire a timed cycle: CoolA → CoolB.
        let rate = config.genre_profile.palette_change_rate;
        let interval = PALETTE_CYCLE_BASE_INTERVAL_S / rate.clamp(0.1, 4.0);
        engine.palette_change_timer = interval; // due now
        let mut messages = Vec::new();
        engine.update_palette_cycling(0.001, &config, &mut messages);
        assert_eq!(engine.palette, Palette::CoolB);
        assert!(messages.iter().any(|(a, _)| a == "/react/palette"));

        // Inside the hold window the balance machine must not revert it
        // (pre-fix it reverted on the very next frame).
        for i in 0..10 {
            engine.frame_time = 100.0 + (i as f32) * (PALETTE_CYCLE_HOLD_S / 10.0) * 0.99;
            let mut msgs = Vec::new();
            engine.update_palette(&config, &mut msgs);
            assert_eq!(
                engine.palette,
                Palette::CoolB,
                "balance machine reverted the cycled palette inside the hold window"
            );
            assert!(msgs.is_empty(), "no palette messages expected during the hold");
        }

        // After the hold expires, balance ownership resumes (reverts to CoolA).
        engine.frame_time = 100.0 + PALETTE_CYCLE_HOLD_S + 0.1;
        let mut msgs = Vec::new();
        engine.update_palette(&config, &mut msgs);
        assert_eq!(engine.palette, Palette::CoolA);
    }

    /// Drives update_palette_cycling for `seconds` of simulated time at the
    /// given rate; returns true if a cycle fired.
    fn palette_cycles_within(rate: f32, seconds: f32) -> bool {
        let mut engine = CueEngine::new();
        let mut config = test_config();
        config.genre_profile.palette_change_rate = rate;
        let start = engine.palette;
        let dt = 0.1;
        let steps = (seconds / dt) as usize;
        for _ in 0..steps {
            let mut msgs = Vec::new();
            engine.update_palette_cycling(dt, &config, &mut msgs);
            if engine.palette != start {
                return true;
            }
        }
        false
    }

    #[test]
    fn palette_change_rate_is_a_speed_multiplier() {
        // genre.rs documents palette_change_rate as a 0.5-2.0 reactivity
        // multiplier. With base 30s: rate 2.0 → 15s interval (twice as fast
        // as 1.0), rate 0.5 → 60s interval (half as fast).
        assert!(palette_cycles_within(2.0, 16.0));
        assert!(!palette_cycles_within(2.0, 14.0));
        assert!(!palette_cycles_within(0.5, 55.0));
        assert!(palette_cycles_within(0.5, 61.0));
    }

    // === Finding: stability gate must not block zone DE-escalation ==========

    #[test]
    fn zone_steps_down_with_stability_gate_closed() {
        let mut engine = CueEngine::new();
        let config = test_config();

        engine.zone = EnergyZone::High;
        engine.stability_gate_open = false; // atonal/noisy passage
        engine.zone_energy = 0.05; // energy has collapsed

        let mut messages = Vec::new();
        engine.update_zone(&config, &mut messages);
        assert!(
            engine.zone < EnergyZone::High,
            "closed stability gate must not block DE-escalation (zone wedged at {:?})",
            engine.zone
        );
        assert!(messages.iter().any(|(a, _)| a == "/react/zone"));
    }

    #[test]
    fn zone_upward_transition_still_waits_for_stability_gate() {
        let mut engine = CueEngine::new();
        let config = test_config();

        engine.zone = EnergyZone::Still;
        engine.stability_gate_open = false;
        engine.zone_energy = 0.9;

        let mut messages = Vec::new();
        engine.update_zone(&config, &mut messages);
        assert_eq!(
            engine.zone,
            EnergyZone::Still,
            "escalation must still respect the stability gate"
        );

        engine.stability_gate_open = true;
        let mut messages = Vec::new();
        engine.update_zone(&config, &mut messages);
        assert_eq!(engine.zone, EnergyZone::High);
    }
}

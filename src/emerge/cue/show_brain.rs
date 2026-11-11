//! Stateful show-brain bridge.
//!
//! `FixtureBrain` (see `fixture_brain.rs`) is a pure, stateless eligibility
//! evaluator. It needs `ArcSignals`, `TopologyState` and a `ShowClockSnapshot`
//! as inputs, and it advertises per-lane cooldown windows it cannot enforce on
//! its own (it holds no state). This module is the stateful seam that:
//!
//!   1. derives those inputs per frame from `CueSnapshot` + `LightingMetrics`,
//!   2. ticks `FixtureBrain` and collects one `CueLaneIntent` per family,
//!   3. enforces the musical/safety cooldowns the evaluator only advertises.
//!
//! Keeping the state here leaves `FixtureBrain` pure and fully unit-tested.
//!
//! v1 scope: inputs are coarse but honest — `coherent_field` is a hysteresis
//! latch on harmonic `stability`, the arc-rise signals are fast-vs-slow EMA
//! differences. EMeRGE topology plumbing (RingDive, fold24, named topologies)
//! is a later refinement; none of it is required for the laser/strobe gate.

use std::collections::VecDeque;

use crate::emerge::contracts::{FixtureCapability, FixtureRole, StageVisualKind};
use crate::emerge::{
    ArcSignals, ArcState, CueLaneIntent, FixtureBrain, FixtureFamily, FixtureFamilyCapabilities,
    FixtureGenreProfile, PhaseState, ShowClockSnapshot, ShowClockSource, TopologyState,
};
use crate::hue::CueSnapshot;
use crate::render::LightingMetrics;

// ── Tuning surface ──────────────────────────────────────────────────────────
// These are the knobs to adjust while watching the arc classifier track a real
// track. Kept together so tuning is one edit, not a scavenger hunt.

/// Fallback tempo until the kick-driven estimator has heard enough beats — and
/// where the estimate is anchored if kicks vanish (ambient breakdowns).
const TEMPO_BPM: f32 = 122.0;

// Tempo estimator (`TempoEstimator`) — derives BPM by autocorrelating the
// continuous low-band onset envelope (the analyzer's kick-envelope flux) over a
// sliding window. Robust to the extra/missing onsets a boolean kick suffers.
/// Onset-detection-function resample rate (Hz). The live onset feed (~30 Hz,
/// jittery `frame_time`) is resampled onto this fixed grid so autocorrelation
/// lags are clean integers.
const ODF_FS: f32 = 100.0;
/// ODF window length in samples (~6 s at `ODF_FS`) — a sharp autocorrelation
/// peak that still re-converges within a couple of seconds.
const ODF_WINDOW: usize = 600;
/// Autocorrelation lag search range. `bpm = 60 * ODF_FS / lag`, so lag 35 ≈ 171
/// BPM and lag 100 = 60 BPM; the winning BPM is then folded into 85–170.
const ODF_LAG_MIN: usize = 35;
const ODF_LAG_MAX: usize = 100;
/// Canonical tempo octave the estimate is folded into, so half-/double-time
/// onset patterns converge on the same musical BPM.
const TEMPO_FOLD_LOW: f32 = 85.0;
const TEMPO_FOLD_HIGH: f32 = 170.0;
/// EMA applied to the folded BPM once locked (smooths display jitter only).
const TEMPO_SMOOTH_ALPHA: f32 = 0.30;
/// Re-seed: snap to a freshly-detected tempo (rather than EMA-dragging) when it
/// disagrees with the smoothed estimate beyond this fraction — a fast re-lock
/// on a song change.
const TEMPO_RESEED_TOL: f32 = 0.06;
/// Seconds of SUSTAINED, well-supported (conf >= 0.5) disagreement before the
/// snap. The old gate counted 4 consecutive estimates — at the ~60 Hz feed
/// that was ~67 ms of wall clock, so a snare-roll fill could snap the global
/// BPM (and with it the octave) and snap right back. Estimates from a window
/// sliding 1-2 cells are ~fully correlated, so "consecutive" carries no
/// evidence; sustained TIME does. This also makes the octave choice sticky —
/// flipping double-/half-time mid-song is the worst-case visible artifact.
/// 1.0 s: long enough to outlast a snare-roll fill (sub-second), short enough
/// that a deliberate track change re-locks in ~window + 1-2 s.
const TEMPO_RESEED_S: f32 = 1.0;
/// Estimates below this confidence neither move the EMA nor count toward a
/// reseed. The autocorrelation of quiet residual flux during a breakdown /
/// ambient bridge still returns *something*; chasing it random-walked the BPM
/// (and the derived clock's rate with it). Hold the last locked tempo instead
/// — which is the behavior the docs always promised.
const TEMPO_TRACK_MIN_CONF: f32 = 0.2;
/// Windowed ODF energy below this is treated as silence (no estimate emitted).
const ODF_SILENCE_EPS: f32 = 1e-6;
/// Adaptive onset gate: an ODF cell is kept only if it reaches this fraction of
/// the recent peak onset. The analyzer's `rise` is a dense flux that wiggles on
/// every bass transient; gating drops that continuous low-level noise so only
/// prominent beat onsets enter the autocorrelation.
const ODF_ONSET_FRAC: f32 = 0.5;
/// Decay (per committed cell) of the running onset peak the gate tracks.
const ODF_PEAK_DECAY: f32 = 0.997;
/// 4 beats/bar × 8 bars/phrase. Used as the show-clock `quantum` so
/// `PhaseState::phrase` cycles once per musical phrase.
const BEATS_PER_PHRASE: f32 = 32.0;

/// Time constant (seconds) of the "slow" reference EMA each rise signal is
/// measured against. The per-tick alpha is derived from dt
/// (`1 - exp(-dt/τ)`) so the arc classifier's calibration survives cadence
/// changes; equals the old fixed 0.012/frame at the ~30 Hz DMX tick.
const RISE_SLOW_TAU_S: f32 = 2.8;
/// Gain applied to a (fast − slow) delta before clamping into 0‥1.
const RISE_GAIN: f32 = 6.0;
/// `energy_slope` is already an energy derivative; it needs its own gain.
const ENERGY_RISE_GAIN: f32 = 3.0;
/// Onset density is an events-per-second RATE estimate: EMA time constant +
/// the rate that reads as full-scale 1.0. Rate-based so the buildup score is
/// invariant to tick cadence — the old boolean duty-cycle EMA halved its
/// signal when the frame rate doubled. 5 Hz full-scale reproduces the old
/// 30 Hz calibration exactly (duty × 6 == rate / 5 at 30 Hz).
const ONSET_RATE_TAU_S: f32 = 0.66;
const ONSET_RATE_FULL_HZ: f32 = 5.0;

// Percussiveness ("is there a beat to speak of"): a slow blend of onset rate
// (how often transients land) and beat confidence (whether they form a grid).
// Sustained atmospheric material — post-rock crescendo walls, ambient pads —
// scores near 0 even at high energy, and the aggressive vocabulary (strobe/
// blinder lanes, drop detonations, tempo re-locks) stands down so crescendos
// read as BLOOMS. Four-on-the-floor material scores ~1 and nothing changes.
/// Onset rate that reads as fully percussive.
const PERC_RATE_FULL_HZ: f32 = 2.0;
/// Blend weight of the rate term (the rest is beat confidence).
const PERC_RATE_WEIGHT: f32 = 0.6;
/// Smoothing time constant — slow enough not to flap inside a song section,
/// fast enough that drums entering mid-song open the vocabulary within ~8 s.
const PERC_TAU_S: f32 = 4.0;
/// Below this INTENSITY LICENSE, the strobe + blinder lanes stand down
/// regardless of genre.
const PERC_LANE_MIN: f32 = 0.25;
/// Below this DYNAMICS, the strobe, blinder and laser lanes stand down: the
/// passage is low on the set's own scale (drums on their own read ~0.7, a
/// bass-cut blend ~0.5, a breakdown ~0.25).
const LANE_DYNAMICS_MIN: f32 = 0.55;

// Intensity license: percussiveness × spectral fullness — the genre-free
// "how hard may we hit right now" dial. Fullness reads the RAW bass and high
// bands against their own decaying peaks: in a drop both ride near peak
// (license ≈ percussiveness); in a vocal bridge the hats/bass thin while the
// kick grid persists, so the license falls and the aggressive vocabulary
// (strobe probability, Pump detonation, licensed mover depth) softens —
// then re-arms within a beat or two of the spectrum refilling. Genre stays a
// taste VETO/ceiling; this owns the moment-to-moment dynamics.
/// Per-band peak envelope time constant (the self-calibrating reference).
/// Long enough to REMEMBER the last drop across a typical bridge/breakdown
/// (~10-30 s) — a faster reference adapts to the bridge itself and reads it
/// as full; slow enough to recalibrate to a genuinely quieter next track
/// within ~1.5 min.
const FULLNESS_PEAK_TAU_S: f32 = 40.0;
/// License smoothing: fast attack (a drop must detonate promptly)...
const LICENSE_ATTACK_TAU_S: f32 = 0.5;
/// ...slower release (a one-bar lull is not a bridge).
const LICENSE_DECAY_TAU_S: f32 = 3.0;
/// License floor blend: license = percussiveness × (floor + (1-floor)·fullness),
/// so a thinned spectrum softens the show without zeroing it outright.
const LICENSE_FLOOR_BLEND: f32 = 0.35;
/// Tempo beat-evidence ramp: below the FLOOR the estimator HOLDS its lock
/// outright (autocorrelation peaks on sustained walls carry FAKE confidence —
/// textural periodicity masquerading as rhythm; the Mogwai 62.4 BPM ghost).
/// Between FLOOR and FULL, tracking/reseed speed scales linearly — soft drums
/// buried under a wall (post-rock once the kit enters) still pull the clock
/// to the real tempo, just patiently. A hard cliff here left the clock stuck
/// on the 122 default through entire songs whose drums never cleared it.
const TEMPO_EVIDENCE_FLOOR: f32 = 0.1;
const TEMPO_EVIDENCE_FULL: f32 = 0.6;

/// Harmonic-stability hysteresis for `TopologyState::coherent_field`.
const COHERENT_ENTER: f32 = 0.70;
const COHERENT_EXIT: f32 = 0.50;
/// How long `coherent_field` must hold before it counts as a `topology_lock`.
const TOPOLOGY_LOCK_DWELL_S: f32 = 1.2;

/// Bars of musical cooldown armed when the arc leaves a drop. Keeps intense
/// lanes restrained so the next buildup means something.
const DROP_COOLDOWN_BARS: f32 = 4.0;

/// Seconds the arc holds `Drop` through a momentary `drop_impact` dip before it
/// will accept a non-drop classification. Bridges frame-level flicker; a
/// genuine drop end (sustained non-drop) still closes out after this window.
const DROP_DEBOUNCE_S: f32 = 1.2;

/// Tension at/above this level counts as a "plateau" the laser gate watches:
/// hold it long enough (see `fixture_brain::LASER_TENSION_PLATEAU_DWELL_S`) and
/// the laser lane is held out so it fires as RELEASE on the drop, not into the
/// build (Koob: lasers on big grooving moments, held back through tense builds).
/// Hysteresis exit: once armed, the plateau only disarms below `_EXIT`, so a
/// smoothed tension oscillating around the enter level cannot keep resetting the
/// dwell to zero (the coherent-field latch uses the same enter/exit trick).
const TENSION_PLATEAU_LEVEL: f32 = 0.70;
const TENSION_PLATEAU_EXIT: f32 = 0.60;

/// Sustained-laser exposure cap. The laser lane is `momentary:false`, so the
/// momentary cooldown path (`enforce_cooldowns`) never bounds it and its
/// advertised `safety_max_duty_cycle` would otherwise be inert. The laser may
/// stay continuously on for at most `LASER_MAX_ON_S`, then is forced dark for
/// `LASER_REFRACTORY_S`. The ratio holds continuous-on duty ~0.29 (<= the laser
/// intent's 0.30) AND makes a long groove read as recurring punctuation rather
/// than wallpaper — a hard eye-safety ceiling independent of musical taste on a
/// 24/7 public laser rig.
const LASER_MAX_ON_S: f32 = 4.0;
const LASER_REFRACTORY_S: f32 = 10.0;
/// Seconds the laser lane must be CONTINUOUSLY dark before the continuous-on
/// budget forgets its start time. Without this debounce a single ineligible
/// frame (one 33 ms stability/beat-confidence dip) reset the cap, so a
/// flicker-eligible lane could sustain ~100% duty forever — the "hard
/// exposure ceiling" leaked exactly on the noisy live signals it exists for.
const LASER_DARK_RESET_S: f32 = 0.75;

/// Family iteration order. `evaluate_all` returns lanes in this order, and
/// `ShowBrainFrame::lanes` is indexed by it.
const FAMILY_ORDER: [FixtureFamily; 6] = [
    FixtureFamily::Wash,
    FixtureFamily::Mover,
    FixtureFamily::Gobo,
    FixtureFamily::Laser,
    FixtureFamily::Strobe,
    FixtureFamily::Blinder,
];

/// Which clock source the selector chose this frame — for display + diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockSource {
    /// Internal clock derived from the audio engine's `frame_time`.
    Derived,
    /// Master CDJ via Pro DJ Link.
    ProDjLink,
    /// Ableton Link session on the LAN.
    Link,
}

/// Operator override for which clock drives the show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClockOverride {
    /// Highest-priority live source wins: Link > Pro DJ Link > derived.
    #[default]
    Auto,
    ForceDerived,
    ForceProDjLink,
    ForceLink,
}

/// External clock candidates handed to the show brain each frame. `None` means
/// that source is not live right now. The derived clock is always available and
/// built internally, so it is not carried here.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClockInputs {
    pub prolink: Option<ShowClockSnapshot>,
    pub link: Option<ShowClockSnapshot>,
    pub mode: ClockOverride,
}

/// One per-frame show-brain result: the classified arc, the gated per-family
/// lane intent, and the clock that drove this frame.
#[derive(Clone, Debug)]
pub struct ShowBrainFrame {
    pub arc_state: ArcState,
    /// Lane intent per family, indexed by `FAMILY_ORDER`.
    pub lanes: [CueLaneIntent; 6],
    /// The clock snapshot used this frame (the selector's winner).
    pub clock: ShowClockSnapshot,
    /// Which source the selector picked.
    pub clock_source: ClockSource,
}

impl ShowBrainFrame {
    /// Lane intent for one fixture family.
    pub fn lane(&self, family: FixtureFamily) -> &CueLaneIntent {
        &self.lanes[family_index(family)]
    }
}

/// Stateful per-frame show brain. Hold one instance for the lifetime of the
/// lighting session and `tick` it once per frame.
pub struct ShowBrain {
    frame_id: u64,

    // ArcSignals — slow EMAs the rise signals are measured against.
    rise_initialized: bool,
    tension_slow: f32,
    high_slow: f32,
    bass_slow: f32,
    /// Smoothed onset rate in events/second (rate-based so the arc gains are
    /// invariant to tick cadence — see `ONSET_RATE_FULL_HZ`).
    onset_rate_hz: f32,
    /// Percussiveness 0..1 (see the PERC_* constants): the atmosphere signal
    /// consumed by the lane governor, the tempo-trust gate, and (via the app
    /// seam) the CueEngine's Pump shaping. Starts OPEN (1.0 — benefit of the
    /// doubt; percussive behavior is the historical default) and decays within
    /// ~10 s once the material proves atmospheric.
    percussiveness: f32,
    /// Decaying peak envelopes of the RAW bass/high bands — the
    /// self-calibrating references spectral fullness is measured against.
    bass_peak: f32,
    high_peak: f32,
    /// Intensity license 0..1 (see the LICENSE_* constants) AFTER the style
    /// clamp — the value every consumer reads. Starts OPEN like
    /// percussiveness.
    intensity_license: f32,
    /// The UNCLAMPED license EMA. The auto-style resolver classifies from
    /// this (classifying from the clamped value would lock Chill in:
    /// clamp → low reading → stays Chill), and an operator style release
    /// snaps consumers straight back to the music's true level.
    raw_license: f32,
    /// How big the passage is on the set's own scale (see `cue::dynamics`).
    /// A measurement, not a permission: no style clamps it.
    dynamics: super::dynamics::DynamicsMeter,
    /// Tests only: hold dynamics at 1.0, i.e. the rig as it was before
    /// dynamics existed, to measure the two against each other.
    #[cfg(test)]
    pub(crate) dynamics_held_open: bool,
    /// Operator style posture: clamps the license band and composes the
    /// genre lane gates most-restrictive-wins. Auto delegates to the
    /// resolver below.
    style: crate::emerge::LightingStyle,
    /// Resolves Auto into a concrete posture (Chill/Energetic/Banger) from
    /// the unclamped license + arc, with hysteresis and dwell.
    auto_resolver: crate::emerge::style::AutoStyleResolver,
    /// Previous tick's debounced arc — the resolver's drop fast-path input
    /// (the license is computed before this tick's arc classification).
    last_debounced_arc: ArcState,
    /// Sum of engine-frame deltas since the last `tick`. `resolve_clock` runs
    /// above the render throttle (~60 Hz) while `tick` runs below it (~30 Hz);
    /// accumulating the precise per-frame dts here gives the arc EMAs an exact
    /// inter-tick dt at any uptime (differencing f32 frame_time quantizes to
    /// frame-interval scale after ~3 days).
    tick_dt_accum: f32,

    // TopologyState — coherent-field hysteresis latch.
    coherent_latched: bool,
    coherent_since: f32,

    // Sustained-tension plateau tracker — `frame_time` tension first crossed
    // TENSION_PLATEAU_LEVEL (-1 = not currently in a plateau).
    tension_above_since: f32,

    // Sustained-laser exposure cap — continuous-on tracking + forced refractory.
    // `laser_on_since` = `frame_time` the laser lane became continuously eligible
    // (-1 = currently off); `laser_refractory_until` = `frame_time` the forced
    // post-cap gap clears.
    laser_on_since: f32,
    laser_refractory_until: f32,
    /// `frame_time` the laser lane went (and stayed) dark; the continuous-on
    /// budget only resets after `LASER_DARK_RESET_S` of real darkness so a
    /// 1-frame eligibility flicker can't relaunch it. -1 = lit / not tracking.
    laser_dark_since: f32,

    // Musical arc cooldown — bars remaining after a drop.
    prev_arc: ArcState,
    cooldown_bars_remaining: f32,
    prev_bar_phase: f32,
    // `frame_time` the arc was last a drop — for the Drop debounce.
    last_drop_seen: f32,

    // Per-family momentary-fire cooldown — `frame_time` of the last fire.
    last_fired: [f32; 6],

    // Audio-derived tempo for the derived show clock.
    tempo: TempoEstimator,

    /// Per-lane gate profile derived from the operator's genre selection
    /// (`GenreProfile::fixture_profile`). Default (mid-permissive) until the
    /// app pushes one — previously this was hardcoded to default in `tick`, so
    /// the genre dropdown shaped OSC/CueEngine output but never the DMX lanes.
    genre: FixtureGenreProfile,

    // ── f64 musical-time integration ────────────────────────────────────────
    // `frame_time` is f32: its ULP reaches frame-interval scale after ~1.5-3
    // days of 24/7 uptime, and recomputing `beat = t * bpm/60` teleports the
    // absolute beat count on every tempo wobble (±3 beats at 1 h for a 0.05
    // BPM EMA step). Integrate instead: advance by precise per-frame deltas so
    // tempo changes bend the clock RATE without any phase discontinuity.
    /// Integrated show time (s) from per-frame deltas (`cue.dt` when stamped).
    time_f64: f64,
    /// Last `frame_time` consumed — the dt fallback when `cue.dt` is 0.
    last_now: Option<f32>,
    /// Integrated derived-clock beat count.
    beat_f64: f64,
    /// Integrated Ableton Link beat count — adopted from the bridge snapshot
    /// when Link appears, then advanced at the session tempo (the bridge
    /// free-runs `frame_time * bpm`, which jumps on tempo changes).
    link_beat: f64,
    link_live: bool,
    /// Accumulated whole bars under a Pro DJ Link master. The bridge supplies
    /// only beat-in-bar (0..4); the cue grammar's long-period oscillators and
    /// `phrase_count` need a monotone absolute count.
    prolink_beat_base: f64,
    prolink_prev_bar_beat: f64,
    prolink_live: bool,

    // COM-499 Phase B2: the clock resolved this frame, cached so the (throttled)
    // render `tick` consumes the same value the app produced when it resolved the
    // clock above the render throttle — resolve once per frame_time, consume
    // twice, one estimator, no divergence.
    last_resolved: (ShowClockSnapshot, ClockSource),
    last_resolved_time: f32,
    /// Engine-frame ordinal of the last resolve (`cue.frame_seq`). The primary
    /// resolve-once-per-frame key: f32 `frame_time` equality misreads DISTINCT
    /// frames as duplicates once its ULP exceeds the frame interval (~3 days
    /// of 24/7 uptime), which would silently drop dt from the integrated
    /// clocks and run them slow. 0 = no seq seen (tests/default snapshots fall
    /// back to the frame_time comparison).
    last_resolved_seq: u64,
}

impl Default for ShowBrain {
    fn default() -> Self {
        Self::new()
    }
}

impl ShowBrain {
    pub fn new() -> Self {
        Self {
            frame_id: 0,
            rise_initialized: false,
            tension_slow: 0.0,
            high_slow: 0.0,
            bass_slow: 0.0,
            onset_rate_hz: 0.0,
            percussiveness: 1.0,
            bass_peak: 0.0,
            high_peak: 0.0,
            intensity_license: 1.0,
            raw_license: 1.0,
            dynamics: super::dynamics::DynamicsMeter::new(),
            #[cfg(test)]
            dynamics_held_open: false,
            style: crate::emerge::LightingStyle::Auto,
            auto_resolver: crate::emerge::style::AutoStyleResolver::new(),
            last_debounced_arc: ArcState::Silence,
            tick_dt_accum: 0.0,
            coherent_latched: false,
            coherent_since: 0.0,
            tension_above_since: -1.0,
            laser_on_since: -1.0,
            laser_refractory_until: 0.0,
            laser_dark_since: -1.0,
            prev_arc: ArcState::Silence,
            cooldown_bars_remaining: 0.0,
            prev_bar_phase: 0.0,
            last_drop_seen: -1.0e6,
            // Far in the past so the first fire of every family is allowed.
            last_fired: [-1.0e6; 6],
            tempo: TempoEstimator::new(),
            genre: FixtureGenreProfile::default(),
            time_f64: 0.0,
            last_now: None,
            beat_f64: 0.0,
            link_beat: 0.0,
            link_live: false,
            prolink_beat_base: 0.0,
            prolink_prev_bar_beat: 0.0,
            prolink_live: false,
            last_resolved: (ShowClockSnapshot::default(), ClockSource::Derived),
            last_resolved_time: -1.0,
            last_resolved_seq: 0,
        }
    }

    /// True when `cue` is the same engine frame the cached resolve came from.
    /// Keyed on the exact `frame_seq` when stamped; falls back to f32
    /// `frame_time` equality for producers/tests that don't stamp it.
    fn already_resolved(&self, cue: &CueSnapshot) -> bool {
        if cue.frame_seq != 0 {
            cue.frame_seq == self.last_resolved_seq
        } else {
            cue.frame_time == self.last_resolved_time
        }
    }

    /// REACT's audio-derived tempo — `(bpm, confidence)` — for the lighting
    /// clock UI. Always available; `confidence` is 0 until enough kicks land.
    pub fn tempo_estimate(&self) -> (f32, f32) {
        (self.tempo.bpm, self.tempo.confidence)
    }

    /// The debounced musical arc from the most recent `tick` (`Silence` before
    /// the first). The single source of truth the CueEngine buildup macro keys
    /// on — one brain decides what a build is.
    pub fn current_arc(&self) -> ArcState {
        self.prev_arc
    }

    /// Percussiveness 0..1 — the atmosphere signal ("is there a beat to speak
    /// of"). Sustained walls/pads read ~0, four-on-the-floor reads ~1. Shapes
    /// the CueEngine's Pump (bloom vs detonation) via the app seam.
    pub fn percussiveness(&self) -> f32 {
        self.percussiveness
    }

    /// Intensity license 0..1 — percussiveness × spectral fullness, the
    /// genre-free "how hard may we hit right now" dial (an EDM vocal bridge
    /// softens it while the kick grid persists). Shapes the CueEngine Pump,
    /// strobe probability, and licensed mover depth.
    pub fn intensity_license(&self) -> f32 {
        self.intensity_license
    }

    /// Dynamics 0..1 — how big the passage is against the loudest sustained
    /// passage of the last several minutes: ~0.3 a breakdown, ~0.7 drums on
    /// their own, 1.0 everything in. Where the license says how hard the rig
    /// MAY hit, this says how big the music IS; the rig settles under it.
    pub fn dynamics(&self) -> f32 {
        #[cfg(test)]
        if self.dynamics_held_open {
            return 1.0;
        }
        self.dynamics.value()
    }

    /// The UNCLAMPED license dial — the music's true level before the style
    /// clamp. What the auto-style resolver classifies from; useful for
    /// debugging/metering the dial independent of the active posture.
    pub fn raw_intensity_license(&self) -> f32 {
        self.raw_license
    }

    /// Set the genre tempo-range prior (min,max BPM) for the autonomous tempo
    /// estimator, or `None` to disable guidance. Only shapes REACT's *derived*
    /// clock — an external Link/Pro DJ clock still wins in `select_clock`.
    pub fn set_tempo_hint(&mut self, range: Option<(f32, f32)>) {
        self.tempo.set_range(range);
    }

    /// Seed the tempo estimator from an externally KNOWN BPM (SaaS track
    /// metadata, a CDJ handoff): snaps the smoothed BPM so beat-locked cues
    /// run at the right rate from beat one instead of after the ~6 s
    /// autocorrelation warm-up. Live tracking continues from the seed, so a
    /// stale or wrong seed self-corrects.
    pub fn seed_tempo(&mut self, bpm: f32) {
        self.tempo.seed(bpm);
    }

    /// Set the per-lane genre gate profile (from the operator's genre
    /// selection — `GenreProfile::fixture_profile`). This is what lets the
    /// genre dropdown close the strobe/laser lanes on the PHYSICAL rig.
    pub fn set_genre(&mut self, genre: FixtureGenreProfile) {
        self.genre = genre;
    }

    /// Set the operator style posture (trigger-row dropdown / MIDI). Takes
    /// effect the same frame: the license output clamp lands instantly
    /// (panic-cut / instant-heat semantics — the unclamped EMA keeps
    /// tracking the music underneath) and the genre composition is
    /// re-derived per tick. Auto delegates to the posture resolver.
    pub fn set_style(&mut self, style: crate::emerge::LightingStyle) {
        self.style = style;
    }

    /// The posture actually driving the engine this frame: the operator's
    /// pinned style, or — under Auto — whatever the resolver has flipped to
    /// (always a concrete Chill/Energetic/Banger).
    pub fn effective_style(&self) -> crate::emerge::LightingStyle {
        if self.style == crate::emerge::LightingStyle::Auto {
            self.auto_resolver.resolved()
        } else {
            self.style
        }
    }

    /// COM-499 Phase B2: resolve the authoritative show clock for this frame —
    /// advance the kick-driven tempo estimator, derive the audio clock, and
    /// select Link / Pro DJ / derived — caching it so the (throttled) render
    /// `tick` consumes the same value. Safe to call every frame ABOVE the DMX
    /// render throttle so all providers + the LFO bank get a live clock even when
    /// DMX rendering is disabled: the estimator resamples the onset envelope onto
    /// a fixed-rate grid keyed off `frame_time`, so resolving at 60 Hz vs the 30 Hz
    /// render cadence yields the same BPM. Resolves once per `frame_time` (tick skips
    /// re-resolving when the app already resolved this frame).
    pub fn resolve_clock(
        &mut self,
        cue: &CueSnapshot,
        metrics: &LightingMetrics,
        clock: &ClockInputs,
    ) -> ShowClockSnapshot {
        // One resolve per engine frame: a repeat call on the same frame
        // consumes the cache and must never re-advance the time/beat
        // integration below.
        if self.already_resolved(cue) {
            return self.last_resolved.0;
        }
        let now = cue.frame_time.max(0.0);
        // Per-frame delta: prefer the engine-stamped `cue.dt` (Instant-derived,
        // precise at any uptime); fall back to differencing frame_time for
        // producers/tests that don't stamp it (fine while frame_time is small).
        let dt = if cue.dt > 0.0 {
            cue.dt.min(0.5) as f64
        } else {
            match self.last_now {
                Some(prev) => (((now - prev).max(0.0)) as f64).min(0.5),
                None => 0.0,
            }
        };
        self.last_now = Some(now);
        self.time_f64 += dt;
        self.tick_dt_accum += dt as f32;

        // Hand the estimator the beat-evidence gate before it commits: with no
        // percussive grid it must HOLD its lock, not chase textural ghosts.
        self.tempo.beat_evidence = self.percussiveness;
        self.tempo.update(metrics.onset_strength, self.time_f64);
        // Integrate the derived beat at the CURRENT tempo: an estimate change
        // bends the clock rate; the phase/beat count never jumps.
        self.beat_f64 += dt * self.tempo.bpm.max(1.0) as f64 / 60.0;
        let derived = derived_clock(self.beat_f64, self.tempo.bpm, self.tempo.confidence);
        let inputs = self.monotonic_external_clocks(clock, dt);
        let resolved = select_clock(derived, &inputs);
        self.last_resolved = resolved;
        self.last_resolved_time = cue.frame_time;
        self.last_resolved_seq = cue.frame_seq;
        resolved.0
    }

    /// Rebase the external clock candidates onto monotone absolute beat
    /// counts. The whole cue grammar (oscillator periods, Wipe/Bank cycle
    /// floors, StepChase indices, `PhaseState::phrase_count`) assumes `beat`
    /// grows without bound:
    ///
    /// * Pro DJ Link supplies beat-in-bar (0..4) — used raw, every effect with
    ///   a period over one bar wedged and patterns never rotated under a CDJ
    ///   master. Accumulate bars across wraps instead.
    /// * Ableton Link free-runs `frame_time * bpm`, which teleports the beat
    ///   on any session-tempo change (and inherits f32 frame_time). Integrate
    ///   at the session tempo instead.
    fn monotonic_external_clocks(&mut self, inputs: &ClockInputs, dt: f64) -> ClockInputs {
        let mut out = *inputs;
        if let Some(mut s) = out.prolink {
            let bar_beat = s.beat; // [0,4) from prolink_clock
            if !self.prolink_live {
                self.prolink_beat_base = 0.0;
            } else if bar_beat + 2.0 < self.prolink_prev_bar_beat {
                // Wrapped into a new bar. (A small backward step is interp
                // jitter from `since_beat_secs`, not a wrap — ignore those.)
                self.prolink_beat_base += 4.0;
            }
            self.prolink_prev_bar_beat = bar_beat;
            self.prolink_live = true;
            s.beat = self.prolink_beat_base + bar_beat;
            // With a monotone beat the CDJ clock can carry the same 32-beat
            // phrase quantum as the other sources, so phrase-gated effects
            // (blinder hits, gobo/laser slot rotation) behave identically
            // under every clock. The bar-local snapshot froze phrase_count at
            // 0 and degraded the phrase gate to an every-bar gate.
            s.quantum = BEATS_PER_PHRASE;
            out.prolink = Some(s);
        } else {
            self.prolink_live = false;
        }
        if let Some(mut s) = out.link {
            if !self.link_live {
                // Adopt the bridge's current beat so we re-join, not rewind.
                self.link_beat = s.beat;
            } else {
                self.link_beat += dt * s.tempo_bpm.max(1.0) as f64 / 60.0;
            }
            self.link_live = true;
            s.beat = self.link_beat;
            s.phase = (s.beat.fract() as f32).rem_euclid(1.0);
            out.link = Some(s);
        } else {
            self.link_live = false;
        }
        out
    }

    /// Classify the musical arc and produce gated, cooldown-enforced lane
    /// intent for every fixture family.
    pub fn tick(
        &mut self,
        cue: &CueSnapshot,
        metrics: &LightingMetrics,
        clock: &ClockInputs,
    ) -> ShowBrainFrame {
        let now = cue.frame_time.max(0.0);
        // Resolve the show clock once per engine frame. The app resolves it
        // above the DMX render throttle (so every provider + the LFO bank gets
        // it); if that already happened this frame, consume the cache rather
        // than re-updating the estimator. A standalone tick (tests) resolves here.
        if !self.already_resolved(cue) {
            self.resolve_clock(cue, metrics, clock);
        }
        let (show_clock, clock_source) = self.last_resolved;
        let arc_signals = self.arc_signals(cue, metrics);
        let topology = self.topology(cue, metrics, now);
        // The EFFECTIVE style (operator pin, or the Auto resolver's posture)
        // composes the genre lane gates most-restrictive-wins (Chill zeroes
        // strobe/laser tolerances under the fixture-brain hard gates; Banger
        // is identity — it never re-opens a genre's bans).
        let genre = self.effective_style().compose_genre(self.genre);

        // Arc detection is stateful here: debounce the raw classification so a
        // momentary `drop_impact` dip cannot flip Drop -> Cooldown mid-pump.
        let phase = PhaseState::from_show_clock(show_clock);
        let raw_arc = FixtureBrain.detect_arc_state(cue, metrics, phase, arc_signals);
        let arc_state = self.debounce_arc(raw_arc, now);
        // Feed the resolver's drop fast-path next tick (the license +
        // resolver run before this classification).
        self.last_debounced_arc = arc_state;

        let frame = FixtureBrain.tick_with_arc(
            self.frame_id,
            cue,
            metrics,
            topology,
            show_clock,
            arc_state,
            arc_signals,
            genre,
        );
        self.frame_id = self.frame_id.wrapping_add(1);

        let caps = all_family_caps();
        let lanes_vec = FixtureBrain.evaluate_all(&frame.brain, &caps);
        let mut lanes: [CueLaneIntent; 6] = lanes_vec
            .try_into()
            .expect("evaluate_all returns one lane per family in FAMILY_ORDER");

        // Atmosphere governor first: with no percussive grid (sustained
        // walls, ambient passages) the strobe + blinder lanes stand down
        // REGARDLESS of genre — a post-rock crescendo is a bloom, not a
        // detonation. Runs before the cooldown stamp so a demoted lane never
        // burns its cooldown.
        self.enforce_atmosphere(&mut lanes);
        // Nor is a low passage the place for the detonating lanes, grid or
        // no grid.
        self.enforce_low_passage(&mut lanes);
        // Exposure budget BEFORE cooldown stamping, keyed off the macro's
        // BlindersHold latch (not the blinder lane's 1-frame eligibility
        // pulse): stamping first burned the strobe's full musical cooldown on
        // a fire that was then demoted (a phantom cooldown), and the pulse
        // meant the yield only ever covered the single co-fire frame instead
        // of the whole hold.
        Self::enforce_exposure_budget(&mut lanes, cue.auto_blinder_active);
        self.enforce_cooldowns(&mut lanes, now);
        // The laser is `momentary:false`, so enforce_cooldowns skips it — bound
        // its continuous-on time + flash exposure explicitly here.
        self.enforce_laser_duty(&mut lanes, now);
        self.track_arc(frame.brain.arc_state, frame.brain.phase.bar);

        // Diagnostic (~1.5 s cadence): the arc, the raw gate inputs, and the
        // intense-lane verdict + reason — enough to recalibrate the gates to
        // REACT's real signal range without guessing.
        if self.frame_id % 90 == 0 {
            let laser = &lanes[family_index(FixtureFamily::Laser)];
            let strobe = &lanes[family_index(FixtureFamily::Strobe)];
            let topo = frame.brain.topology;
            log::info!(
                "[ShowBrain] arc={:?} | dynamics={:.2} license={:.2} energy={:.2} stab m/c={:.2}/{:.2} beat={:.2} drop={:.2} cooldown_lock={:.0} coherent={} lock={} | laser={} ({}) strobe={} ({})",
                frame.brain.arc_state,
                arc_signals.dynamics,
                self.intensity_license,
                metrics.energy,
                metrics.stability,
                cue.stability,
                metrics.beat_confidence,
                cue.drop_impact,
                arc_signals.musical_cooldown_bar_lock,
                topo.coherent_field,
                topo.topology_lock,
                laser.eligible,
                laser.reason,
                strobe.eligible,
                strobe.reason,
            );
        }

        ShowBrainFrame {
            arc_state: frame.brain.arc_state,
            lanes,
            clock: show_clock,
            clock_source,
        }
    }

    /// Derive the arc-shape signals (slopes + densities) the arc classifier
    /// sums into a buildup score.
    fn arc_signals(&mut self, cue: &CueSnapshot, metrics: &LightingMetrics) -> ArcSignals {
        if !self.rise_initialized {
            self.tension_slow = cue.tension;
            self.high_slow = cue.high;
            self.bass_slow = cue.bass;
            self.rise_initialized = true;
        }
        // Precise inter-tick dt accumulated by resolve_clock (resolve runs at
        // ~60 Hz, tick at the ~30 Hz render cadence). dt-derived alphas keep
        // every arc time constant invariant to cadence and uptime.
        let dt = self.tick_dt_accum.clamp(0.0, 0.5);
        self.tick_dt_accum = 0.0;
        let slow_alpha = 1.0 - (-dt / RISE_SLOW_TAU_S).exp();

        // A rise = how far the current value sits above its own slow EMA.
        let tension_rise = ((cue.tension - self.tension_slow) * RISE_GAIN).clamp(0.0, 1.0);
        let high_rise = ((cue.high - self.high_slow) * RISE_GAIN).clamp(0.0, 1.0);
        // Bass withdrawal = bass falling below its slow EMA (the pre-drop dropout).
        let bass_withdrawal = ((self.bass_slow - cue.bass) * RISE_GAIN).clamp(0.0, 1.0);

        self.tension_slow += slow_alpha * (cue.tension - self.tension_slow);
        self.high_slow += slow_alpha * (cue.high - self.high_slow);
        self.bass_slow += slow_alpha * (cue.bass - self.bass_slow);

        // Onset density: an events-per-second rate estimate, normalized so
        // ONSET_RATE_FULL_HZ reads as 1.0 (cadence-invariant by construction).
        let rate_alpha = 1.0 - (-dt / ONSET_RATE_TAU_S).exp();
        let inst_rate = if dt > 1e-4 {
            (if metrics.onset || metrics.kick { 1.0 } else { 0.0 }) / dt
        } else {
            0.0
        };
        self.onset_rate_hz += rate_alpha * (inst_rate - self.onset_rate_hz);
        let onset_density = (self.onset_rate_hz / ONSET_RATE_FULL_HZ).clamp(0.0, 1.0);

        // Percussiveness: slow blend of "transients land often" and "they
        // form a grid". See the PERC_* constants for what consumes it.
        let perc_target = (PERC_RATE_WEIGHT
            * (self.onset_rate_hz / PERC_RATE_FULL_HZ).clamp(0.0, 1.0)
            + (1.0 - PERC_RATE_WEIGHT) * metrics.beat_confidence.clamp(0.0, 1.0))
        .clamp(0.0, 1.0);
        let perc_alpha = 1.0 - (-dt / PERC_TAU_S).exp();
        self.percussiveness += perc_alpha * (perc_target - self.percussiveness);

        // Spectral fullness → intensity license (see the LICENSE_* constants).
        let peak_decay = (-dt / FULLNESS_PEAK_TAU_S).exp();
        self.bass_peak = (self.bass_peak * peak_decay).max(metrics.raw_bass.max(0.0));
        self.high_peak = (self.high_peak * peak_decay).max(metrics.raw_high.max(0.0));
        let band_rel = |band: f32, peak: f32| {
            if peak > 1e-6 { (band / peak).clamp(0.0, 1.0) } else { 0.0 }
        };
        let fullness = 0.5 * band_rel(metrics.raw_bass, self.bass_peak)
            + 0.5 * band_rel(metrics.raw_high, self.high_peak);
        let license_target =
            self.percussiveness * (LICENSE_FLOOR_BLEND + (1.0 - LICENSE_FLOOR_BLEND) * fullness);
        let license_tau = if license_target > self.raw_license {
            LICENSE_ATTACK_TAU_S
        } else {
            LICENSE_DECAY_TAU_S
        };
        let license_alpha = 1.0 - (-dt / license_tau).exp();
        // The EMA evolves UNCLAMPED — the resolver classifies from it, and a
        // style release snaps output straight to the music's true level.
        self.raw_license += license_alpha * (license_target - self.raw_license);

        // Auto posture resolver: measured flips between Chill/Energetic/
        // Banger on sustained evidence + hysteresis + dwell; a real drop
        // (previous tick's debounced arc — the license runs before this
        // tick's classification) fast-promotes straight to Banger. Runs
        // every frame even when the operator has pinned a posture, so
        // flipping back to Auto resumes from live state, not stale state.
        let arc_is_peak = matches!(
            self.last_debounced_arc,
            ArcState::Drop | ArcState::SustainPeak
        );
        self.auto_resolver.update(dt, self.raw_license, arc_is_peak);

        // Style output clamp — lands instantly on a switch (Chill mid-drop =
        // panic cut, pinned Banger = instant heat). Every consumer
        // (atmosphere governor, ArcSignals -> mover licensing, the
        // CueSnapshot SCAN/flash/galaxy paths, cue-engine Pump ceiling +
        // strobe flavor + probability) reads this one clamped value.
        // Auto-resolved postures use auto_clamp_license: resolved Banger
        // does NOT floor, so the within-song dial (bridge softening, drop
        // re-arming) keeps working inside an auto-Banger section.
        self.intensity_license = if self.style == crate::emerge::LightingStyle::Auto {
            self.effective_style().auto_clamp_license(self.raw_license)
        } else {
            self.style.clamp_license(self.raw_license)
        };

        // Dynamics: the same raw bands, against a memory of minutes.
        self.dynamics.update(dt, metrics.raw_energy, metrics.raw_bass, metrics.raw_high);
        let dynamics = self.dynamics();

        // The CueEngine already publishes the energy derivative directly.
        let energy_rise = (cue.energy_slope * ENERGY_RISE_GAIN).clamp(0.0, 1.0);

        // Sustained-tension plateau: seconds tension has held at/above the
        // plateau level. The laser gate uses this to hold the lane out through a
        // long tense build so it fires as release on the drop.
        let now = cue.frame_time.max(0.0);
        // Plateau watches ADAPTIVE tension: a railed wall (absolute ~1.0 for
        // whole songs) used to keep the plateau armed forever; a real build's
        // tension sits above its own baseline only while it builds.
        let tension_plateau_s = if self.tension_above_since >= 0.0 {
            // Already armed: hold until tension falls below the EXIT level, so a
            // smoothed tension dithering around the enter level keeps accruing.
            if cue.tension_dyn < TENSION_PLATEAU_EXIT {
                self.tension_above_since = -1.0;
                0.0
            } else {
                (now - self.tension_above_since).max(0.0)
            }
        } else if cue.tension_dyn >= TENSION_PLATEAU_LEVEL {
            // Newly armed this frame — the dwell starts accruing next frame.
            self.tension_above_since = now;
            0.0
        } else {
            0.0
        };

        ArcSignals {
            energy_rise,
            tension_rise,
            onset_density,
            high_rise,
            bass_withdrawal,
            musical_cooldown_bar_lock: self.cooldown_bars_remaining.max(0.0),
            tension_plateau_s,
            intensity_license: self.intensity_license,
            dynamics,
        }
    }

    /// Drop the redundant beat-strobe flicker while a Blinder hold owns the room:
    /// when the Blinder lane is eligible, a co-firing autonomous Strobe yields (a
    /// beat strobe layered on a full-white blinder adds no visible energy and only
    /// stacks white). This is NOT a full white-out governor — the blinder
    /// full-white blast (apply_global_overrides) and the volumetric haze bloom on
    /// the drop wash are intended/realistic (Koob: "that's how it looks in RL")
    /// and are deliberately left alone. The operator strobe trigger is upstream of
    /// this and unaffected — it overrides the lane gate at the intent seam, not here.
    /// Atmosphere governor: see the call site in `tick`. Strobe + blinder are
    /// the only lanes governed — washes/movers/gobos carry atmospheric
    /// material well, and the laser's coherence gates already favor sustained
    /// tonal content.
    fn enforce_atmosphere(&self, lanes: &mut [CueLaneIntent; 6]) {
        if self.intensity_license >= PERC_LANE_MIN {
            return;
        }
        for family in [FixtureFamily::Strobe, FixtureFamily::Blinder] {
            let lane = &mut lanes[family_index(family)];
            if lane.eligible {
                lane.eligible = false;
                lane.intensity = 0.0;
                lane.reason = "no percussive grid (atmosphere)";
            }
        }
    }

    /// Low-passage governor: strobe, blinder and laser stand down while the
    /// music is low on the set's own scale (see `cue::dynamics`). A breakdown
    /// with a busy hi-hat line still has a grid, so the atmosphere governor
    /// lets it through — but it is not a moment to detonate, and holding
    /// these lanes back is what gives them somewhere to go on the drop.
    fn enforce_low_passage(&self, lanes: &mut [CueLaneIntent; 6]) {
        if self.dynamics() >= LANE_DYNAMICS_MIN {
            return;
        }
        for family in [FixtureFamily::Strobe, FixtureFamily::Blinder, FixtureFamily::Laser] {
            let lane = &mut lanes[family_index(family)];
            if lane.eligible {
                lane.eligible = false;
                lane.intensity = 0.0;
                lane.reason = "low passage (dynamics)";
            }
        }
    }

    /// `blinder_hold` is the macro's BlindersHold latch (true for the whole
    /// hold) — the blinder LANE is momentary, so its `eligible` is a 1-frame
    /// pulse and keying off it alone only ever covered the co-fire frame.
    fn enforce_exposure_budget(lanes: &mut [CueLaneIntent; 6], blinder_hold: bool) {
        if blinder_hold || lanes[family_index(FixtureFamily::Blinder)].eligible {
            let strobe = &mut lanes[family_index(FixtureFamily::Strobe)];
            if strobe.eligible {
                strobe.eligible = false;
                strobe.reason = "strobe yielded to blinder (exposure budget)";
            }
        }
    }

    /// Bound the sustained laser lane (`momentary:false`, so enforce_cooldowns
    /// and its advertised `safety_max_duty_cycle` never touch it): cap continuous
    /// on-time at `LASER_MAX_ON_S`, then force the lane dark for `LASER_REFRACTORY_S`.
    /// This makes a long groove punctuate rather than running the laser — and the
    /// audience's eyes — continuously, and is a hard exposure ceiling regardless of
    /// musical eligibility on a public rig.
    fn enforce_laser_duty(&mut self, lanes: &mut [CueLaneIntent; 6], now: f32) {
        let laser = &mut lanes[family_index(FixtureFamily::Laser)];
        if now < self.laser_refractory_until {
            // Inside the forced gap — dark regardless of musical eligibility.
            laser.eligible = false;
            laser.intensity = 0.0;
            laser.reason = "laser refractory (continuous-exposure cap)";
            self.laser_on_since = -1.0;
            self.laser_dark_since = -1.0;
        } else if laser.eligible {
            self.laser_dark_since = -1.0;
            if self.laser_on_since < 0.0 {
                self.laser_on_since = now;
            } else if now - self.laser_on_since >= LASER_MAX_ON_S {
                // Hit the continuous-on ceiling — force dark + arm the refractory.
                laser.eligible = false;
                laser.intensity = 0.0;
                laser.reason = "laser refractory (continuous-exposure cap)";
                self.laser_refractory_until = now + LASER_REFRACTORY_S;
                self.laser_on_since = -1.0;
            }
        } else if self.laser_on_since >= 0.0 {
            // Ineligible while a continuous-on budget is open. A 1-frame
            // eligibility flicker (a momentary stability/beat-confidence dip)
            // must NOT relaunch the budget — that leak let a flickering lane
            // hold ~100% duty forever. Forget the start time only after the
            // lane has stayed dark for a real gap. (The budget keeps counting
            // wall time across the brief gap — the conservative direction for
            // an eye-safety ceiling.)
            if self.laser_dark_since < 0.0 {
                self.laser_dark_since = now;
            } else if now - self.laser_dark_since >= LASER_DARK_RESET_S {
                self.laser_on_since = -1.0;
                self.laser_dark_since = -1.0;
            }
        }
    }

    /// Derive a coarse `TopologyState`. `coherent_field` is a hysteresis latch
    /// on harmonic stability; `expansion`/`heat`/`intensity` ride the Collatz
    /// fields already carried on the cue snapshot.
    fn topology(&mut self, cue: &CueSnapshot, metrics: &LightingMetrics, now: f32) -> TopologyState {
        let stability = metrics.stability.max(cue.stability);
        if self.coherent_latched {
            if stability < COHERENT_EXIT {
                self.coherent_latched = false;
            }
        } else if stability > COHERENT_ENTER {
            self.coherent_latched = true;
            self.coherent_since = now;
        }
        let topology_lock =
            self.coherent_latched && (now - self.coherent_since) >= TOPOLOGY_LOCK_DWELL_S;

        TopologyState {
            coherent_field: self.coherent_latched,
            topology_lock,
            expansion: cue.collatz_breath.clamp(0.0, 1.0),
            heat: cue.collatz_heat.clamp(0.0, 1.0),
            intensity: cue.collatz_intensity.clamp(0.0, 1.0),
        }
    }

    /// Suppress an eligible momentary fire while its lane is still inside the
    /// cooldown window it advertised on its last fire.
    fn enforce_cooldowns(&mut self, lanes: &mut [CueLaneIntent; 6], now: f32) {
        for lane in lanes.iter_mut() {
            if !(lane.eligible && lane.momentary) {
                continue;
            }
            let idx = family_index(lane.family);
            let window_s =
                lane.safety_cooldown_ms.max(lane.musical_cooldown_ms) as f32 / 1000.0;
            if now - self.last_fired[idx] < window_s {
                lane.eligible = false;
                lane.intensity = 0.0;
                lane.reason = "lane cooldown active";
            } else {
                self.last_fired[idx] = now;
            }
        }
    }

    /// Track the arc across frames: count the post-drop cooldown down on bar
    /// boundaries, and re-arm it whenever the arc leaves a drop.
    fn track_arc(&mut self, arc: ArcState, bar_phase: f32) {
        // A bar boundary is a wrap of the bar phase from ~1.0 back toward 0.0.
        if bar_phase < self.prev_bar_phase && self.cooldown_bars_remaining > 0.0 {
            self.cooldown_bars_remaining -= 1.0;
        }
        self.prev_bar_phase = bar_phase;

        let was_drop = matches!(self.prev_arc, ArcState::Drop | ArcState::SustainPeak);
        let is_drop = matches!(arc, ArcState::Drop | ArcState::SustainPeak);
        if was_drop && !is_drop {
            self.cooldown_bars_remaining = DROP_COOLDOWN_BARS;
        }
        self.prev_arc = arc;
    }

    /// Hold `Drop` through momentary `drop_impact` dips: once the raw arc has
    /// been a drop, keep reporting `Drop` until the music has been genuinely
    /// non-drop for `DROP_DEBOUNCE_S`. Kills the Drop<->Cooldown flicker that
    /// otherwise chops a sustained pump into 4-bar phantom cooldowns.
    fn debounce_arc(&mut self, raw: ArcState, now: f32) -> ArcState {
        if matches!(raw, ArcState::Drop | ArcState::SustainPeak) {
            self.last_drop_seen = now;
            raw
        } else if now - self.last_drop_seen < DROP_DEBOUNCE_S {
            ArcState::Drop
        } else {
            raw
        }
    }
}

/// Build the derived `ShowClockSnapshot` from the INTEGRATED beat count and
/// the audio-estimated `bpm`. The beat is integrated (`+= dt·bpm/60`) by
/// `resolve_clock`, never recomputed from absolute time — recomputing
/// `t·bpm/60` teleported the count by `t·Δbpm/60` on every tempo wobble.
/// `confidence` is the tempo estimator's, so the clock reads as soft until
/// enough beats have been heard.
fn derived_clock(beat: f64, bpm: f32, confidence: f32) -> ShowClockSnapshot {
    ShowClockSnapshot {
        tempo_bpm: bpm.max(1.0),
        beat,
        phase: (beat.fract() as f32).rem_euclid(1.0),
        quantum: BEATS_PER_PHRASE,
        running: true,
        peer_count: 0,
        confidence: confidence.clamp(0.0, 1.0),
        source: ShowClockSource::AudioEngine,
    }
}

/// Audio-derived tempo for the derived clock. Tracks the interval between kick
/// onsets — REACT's kick detector gives clean discrete beats for the
/// four-on-the-floor material REACT targets — takes the robust centre of the
/// recent window, octave-folds it, and smooths. No external sync needed: this
/// is the always-available fallback when neither Link nor a CDJ is present.
#[derive(Clone, Debug)]
struct TempoEstimator {
    /// Fixed-rate onset-envelope ring (`ODF_FS` Hz). The live onset feed (~30 Hz,
    /// jittery `frame_time`) is resampled onto this grid so lags are clean.
    odf: VecDeque<f32>,
    /// Time of the last `update` (f64 integrated show time — f32 ULP reaches
    /// the frame interval after ~3 days and would zero/burst the grid advance).
    /// `None` until the first sample.
    last_time: Option<f64>,
    /// Max onset since the last grid cell was committed (peak-hold, so a sharp
    /// onset between grid samples isn't lost when the feed is slower than `ODF_FS`).
    pending_peak: f32,
    /// Decaying running peak onset — the reference the adaptive onset gate uses
    /// to drop continuous low-level flux.
    odf_peak: f32,
    /// Fractional grid-cell remainder, carried between updates so the resampled
    /// grid rate averages EXACTLY `ODF_FS` regardless of the (jittery, non-100 Hz)
    /// feed rate. A per-update `round()` would bias the rate — and every BPM with
    /// it — by the frame-rate rounding error.
    grid_accum: f32,
    /// Current smoothed BPM estimate.
    bpm: f32,
    /// 0‥1 confidence from the autocorrelation peak sharpness.
    confidence: f32,
    /// Accumulated SECONDS of well-supported disagreement with `bpm` beyond
    /// `TEMPO_RESEED_TOL` (grid time, so it is feed-rate invariant).
    disagree_s: f32,
    /// The candidate tempo the disagreement time is accruing toward — a new
    /// estimate that disagrees with BOTH the lock and this candidate resets
    /// the accumulator (scattered candidates = noise, not a new song).
    disagree_bpm: f32,
    /// Beat evidence 0..1 (the show brain's percussiveness), stamped before
    /// each `update`. Below `TEMPO_EVIDENCE_MIN` the estimator HOLDS its lock:
    /// the ACF confidence gate protects against noise, but sustained walls
    /// produce textural periodicities with FAKE confidence (the Mogwai 62.4
    /// BPM ghost) that only an onset-level signal can unmask. Defaults to 1.0
    /// so direct unit-test feeds behave classically.
    beat_evidence: f32,
    /// Throttle counter for the diagnostic log.
    log_tick: u32,
    /// Optional genre tempo-range prior (min,max BPM). When set, the estimator
    /// biases its autocorrelation toward — and folds its octave around — this
    /// range (resolves double-/half-time). `None` => plain 85-170 fold.
    tempo_range: Option<(f32, f32)>,
}

impl TempoEstimator {
    fn new() -> Self {
        Self {
            odf: VecDeque::with_capacity(ODF_WINDOW),
            last_time: None,
            pending_peak: 0.0,
            odf_peak: 0.0,
            grid_accum: 0.0,
            bpm: TEMPO_BPM,
            confidence: 0.0,
            disagree_s: 0.0,
            disagree_bpm: 0.0,
            beat_evidence: 1.0,
            log_tick: 0,
            tempo_range: None,
        }
    }

    /// Seed from an externally KNOWN BPM (track metadata / CDJ handoff): snap
    /// the smoothed tempo and grant provisional confidence. Live
    /// autocorrelation keeps tracking from the seed, so a stale or wrong seed
    /// self-corrects within the normal reseed window.
    fn seed(&mut self, bpm: f32) {
        if bpm.is_finite() && (30.0..=300.0).contains(&bpm) {
            self.bpm = bpm;
            self.confidence = self.confidence.max(0.6);
            self.disagree_s = 0.0;
        }
    }

    /// Set (or clear) the genre tempo-range prior. Invalid or zero-width ranges
    /// collapse to `None` (no guidance — plain autocorrelation).
    fn set_range(&mut self, range: Option<(f32, f32)>) {
        self.tempo_range = match range {
            Some((lo, hi)) if lo > 0.0 && hi > lo => Some((lo, hi)),
            _ => None,
        };
    }

    /// Feed one frame. `onset` is the analyzer's continuous low-band onset
    /// strength (kick-envelope flux); `now` is the cue's monotonic `frame_time`.
    /// Resamples onset onto the `ODF_FS` grid (peak-held across the feed gap),
    /// then autocorrelates the window for the dominant beat period.
    fn update(&mut self, onset: f32, now: f64) {
        let onset = if onset.is_finite() { onset.max(0.0) } else { 0.0 };
        self.pending_peak = self.pending_peak.max(onset);

        let prev = match self.last_time {
            Some(t) => t,
            None => {
                self.last_time = Some(now);
                return;
            }
        };
        let dt = (now - prev) as f32; // small delta — exact in f32
        // Same frame, or a clock reset/rewind — resync without committing.
        if dt <= 0.0 {
            self.last_time = Some(now);
            return;
        }
        self.last_time = Some(now);
        // Advance the fixed-rate grid by the FRACTIONAL cells elapsed, carrying the
        // remainder so the long-run grid rate is exactly `ODF_FS` (a per-update
        // round() biases the rate by the frame-rate rounding error — that was
        // reading a 116 track as ~129).
        self.grid_accum += dt * ODF_FS;
        let whole = self.grid_accum.floor();
        self.grid_accum -= whole;
        let cells = (whole as usize).min(ODF_WINDOW);
        if cells == 0 {
            return; // sub-cell update; the onset stays in `pending_peak`
        }
        let peak = self.pending_peak;
        self.pending_peak = 0.0;
        // Adaptive onset gate: keep this onset only if it stands out above the
        // recent peak, so continuous bass flux (the dense `rise` signal) is
        // dropped and the ODF becomes a clean beat-onset train.
        self.odf_peak = (self.odf_peak * ODF_PEAK_DECAY).max(peak);
        let gated = if peak >= ODF_ONSET_FRAC * self.odf_peak { peak } else { 0.0 };
        // Onset goes in the cell at `now` (consistent phase per onset), zeros in
        // the intervening cells — a sharp impulse train autocorrelates cleanly.
        for i in 0..cells {
            if self.odf.len() == ODF_WINDOW {
                self.odf.pop_front();
            }
            self.odf.push_back(if i + 1 == cells { gated } else { 0.0 });
        }
        if self.odf.len() < ODF_WINDOW {
            return;
        }

        let Some((bpm, confidence)) = autocorr_bpm(&self.odf, self.tempo_range) else {
            // Near-silent / no periodicity: hold the BPM, decay confidence.
            self.confidence *= 0.95;
            return;
        };

        // Texture guard: beat evidence scales how fast the estimator may
        // move (see the TEMPO_EVIDENCE_* ramp). At the floor the lock holds
        // outright — the estimate's own confidence can't be trusted on
        // sustained walls, so don't adopt it either; decay gently.
        let evidence_scale = ((self.beat_evidence - TEMPO_EVIDENCE_FLOOR)
            / (TEMPO_EVIDENCE_FULL - TEMPO_EVIDENCE_FLOOR))
            .clamp(0.0, 1.0);
        if evidence_scale <= 0.0 {
            self.confidence *= 0.995;
            self.disagree_s = 0.0;
            return;
        }

        let rel = (bpm - self.bpm).abs() / self.bpm.max(1.0);
        let window_s = cells as f32 / ODF_FS; // grid time this estimate spans
        if confidence >= TEMPO_TRACK_MIN_CONF && rel <= TEMPO_RESEED_TOL {
            // In-tolerance drift: track it, weighting the EMA by confidence so
            // strong estimates converge fast and marginal ones barely move the
            // lock (an unconditional EMA random-walked through breakdowns).
            self.disagree_s = 0.0;
            self.bpm +=
                TEMPO_SMOOTH_ALPHA * confidence.min(1.0) * evidence_scale * (bpm - self.bpm);
        } else if confidence >= TEMPO_TRACK_MIN_CONF && rel > TEMPO_RESEED_TOL {
            // Supported disagreement: accumulate TIME (not estimate count)
            // toward a snap — but only while successive estimates agree on
            // ONE candidate tempo. Consistency is the discriminator that
            // separates "new song at a different tempo, mid confidence"
            // (re-lock eventually, even if conf never clears 0.5 on busy
            // offbeat material) from "aperiodic noise that happens to clear
            // the confidence floor" (candidates scatter, the accumulator
            // keeps resetting, the lock holds). Confidence scales the accrual
            // rate — full speed at conf >= 0.5, slower below. The locked
            // tempo HOLDS throughout (no EMA drag toward the candidate), so a
            // transient ACF flip (a snare-roll fill) leaves the lock and its
            // octave untouched.
            let same_candidate =
                (bpm - self.disagree_bpm).abs() / self.disagree_bpm.max(1.0) <= TEMPO_RESEED_TOL;
            if !same_candidate {
                self.disagree_bpm = bpm;
                self.disagree_s = 0.0;
            }
            self.disagree_s += window_s * (confidence / 0.5).min(1.0) * evidence_scale;
            if self.disagree_s >= TEMPO_RESEED_S {
                self.bpm = bpm;
                self.disagree_s = 0.0;
            }
        }
        // else: weak estimate (breakdown/noise) — hold the locked tempo.
        self.confidence = confidence;

        self.log_tick = self.log_tick.wrapping_add(1);
        if self.log_tick % 30 == 0 {
            log::debug!(
                "[Tempo] bpm={:.1} conf={:.2} (raw {:.1})",
                self.bpm, self.confidence, bpm
            );
        }
    }
}

/// Autocorrelate the onset-envelope window and return `(folded BPM, confidence)`,
/// or `None` when the window carries no usable periodicity (near-silence). A
/// comb/harmonic score prefers the fundamental beat lag over its subdivisions
/// (suppressing the half-time/8th-note false peaks that biased the old median
/// estimator high); a parabolic fit around the peak gives sub-sample precision.
fn autocorr_bpm(odf: &VecDeque<f32>, range: Option<(f32, f32)>) -> Option<(f32, f32)> {
    let n = odf.len();
    let max_lag = ODF_LAG_MAX.min(n / 3);
    if max_lag < ODF_LAG_MIN + 1 {
        return None;
    }
    let mean = odf.iter().sum::<f32>() / n as f32;
    let xs: Vec<f32> = odf.iter().map(|v| v - mean).collect();
    let energy: f32 = xs.iter().map(|v| v * v).sum();
    if energy < ODF_SILENCE_EPS {
        return None;
    }
    // Normalized autocorrelation r(lag), computed up to 3× the search max so the
    // comb below can read r(2·lag) and r(3·lag).
    let r_max = (max_lag * 3).min(n - 1);
    let mut r = vec![0.0f32; r_max + 1];
    for (lag, slot) in r.iter_mut().enumerate().take(r_max + 1).skip(ODF_LAG_MIN) {
        let mut acc = 0.0;
        for t in lag..n {
            acc += xs[t] * xs[t - lag];
        }
        *slot = acc / energy;
    }
    // Comb score: a true beat lag has aligned 2× and 3× multiples — this is what
    // rejects the half-time/sub-beat false peaks.
    let comb = |lag: usize| -> f32 {
        let mut s = r[lag];
        if 2 * lag <= r_max {
            s += 0.5 * r[2 * lag];
        }
        if 3 * lag <= r_max {
            s += 0.33 * r[3 * lag];
        }
        s
    };
    // Genre prior (optional): bias the comb score toward lags whose octave-folded
    // BPM sits near the genre's tempo range, so on melodic/ambiguous material the
    // genre-appropriate beat wins over a faster texture. `None` => weight 1.0
    // everywhere (unbiased — plain autocorrelation, unchanged behavior).
    let center: Option<f32> = match range {
        Some((lo, hi)) if lo > 0.0 && hi > lo => Some((lo * hi).sqrt()),
        _ => None,
    };
    let sigma = match range {
        Some((lo, hi)) if lo > 0.0 && hi > lo => (0.5 * (hi / lo).ln()).max(0.10),
        _ => 1.0,
    };
    let weight = |lag: usize| -> f32 {
        match center {
            Some(c) => {
                let d = octave_log_dist(60.0 * ODF_FS / lag as f32, c) / sigma;
                (-0.5 * d * d).exp()
            }
            None => 1.0,
        }
    };
    let mut best = ODF_LAG_MIN;
    let mut best_score = comb(ODF_LAG_MIN) * weight(ODF_LAG_MIN);
    for lag in (ODF_LAG_MIN + 1)..=max_lag {
        let s = comb(lag) * weight(lag);
        if s > best_score {
            best_score = s;
            best = lag;
        }
    }
    // Parabolic interpolation around the peak for sub-sample lag precision.
    let lag_f = if best > ODF_LAG_MIN && best < max_lag {
        let (a, b, c) = (r[best - 1], r[best], r[best + 1]);
        let denom = a - 2.0 * b + c;
        let delta = if denom.abs() > 1e-9 { 0.5 * (a - c) / denom } else { 0.0 };
        best as f32 + delta.clamp(-0.5, 0.5)
    } else {
        best as f32
    };
    if lag_f <= 0.0 {
        return None;
    }
    let raw = 60.0 * ODF_FS / lag_f;
    let bpm = match center {
        Some(c) => fold_to_range(raw, c),
        None => fold_tempo(raw),
    };
    // Confidence: peak height vs the strongest competing peak (excluding the
    // peak's own neighborhood and its octaves). The ACF of a real onset train
    // has FAT harmonic peaks several cells wide, so exclude a ±2 neighborhood
    // around the half/double/triple lags — the old exact-match exclusion left
    // r[2·best±1..2] (and ALL half lags of odd `best`, which straddle two
    // cells) counting as "competition", deflating confidence on correctly
    // locked tempos and starving the conf-gated reseed/track paths.
    let peak = r[best].max(0.0);
    let mut secondary = 0.0f32;
    let b = best as i32;
    let half = (best as f32 / 2.0).round() as i32;
    for lag in ODF_LAG_MIN..=max_lag {
        let l = lag as i32;
        let near_peak = (l - b).abs() <= 2;
        let near_octave =
            (l - 2 * b).abs() <= 2 || (l - half).abs() <= 2 || (l - 3 * b).abs() <= 2;
        if !near_peak && !near_octave {
            secondary = secondary.max(r[lag]);
        }
    }
    let confidence = if peak > 1e-4 {
        (peak * (1.0 - (secondary / peak).clamp(0.0, 1.0))).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Some((bpm, confidence))
}

/// Fold a raw BPM into the canonical tempo octave, so half-/double-time kick
/// patterns converge on the same musical tempo.
fn fold_tempo(mut bpm: f32) -> f32 {
    if !bpm.is_finite() || bpm <= 0.0 {
        return TEMPO_BPM;
    }
    while bpm < TEMPO_FOLD_LOW {
        bpm *= 2.0;
    }
    while bpm > TEMPO_FOLD_HIGH {
        bpm /= 2.0;
    }
    bpm
}

/// Octave-folded log distance between `bpm` and `target`, in natural-log units.
/// Treats tempos an octave apart as identical (the residual after rounding the
/// log2 ratio to the nearest integer) so the genre prior never penalizes a true
/// beat just for sitting one doubling away from the genre center.
fn octave_log_dist(bpm: f32, target: f32) -> f32 {
    if !(bpm > 0.0) || !(target > 0.0) {
        return 0.0;
    }
    let octaves = (bpm / target).log2();
    (octaves - octaves.round()).abs() * std::f32::consts::LN_2
}

/// Fold `raw` BPM to the ×2^k octave nearest `target` in log space — the
/// genre-guided replacement for the fixed 85–170 fold. `k = round(log2(target/raw))`.
/// e.g. raw 150 / target≈79 (Lo-Fi 70–90) → 75; raw 75 / target≈144 (Trap) → 150.
fn fold_to_range(raw: f32, target: f32) -> f32 {
    if !raw.is_finite() || raw <= 0.0 {
        return TEMPO_BPM;
    }
    let k = (target / raw).log2().round();
    raw * 2f32.powf(k)
}

/// Pick the winning clock for this frame. `derived` is always available;
/// `link`/`prolink` are used only when live. A forced source that is not live
/// falls through to the same priority order, so the show always has a clock.
/// Priority: Link > Pro DJ Link > derived.
fn select_clock(
    derived: ShowClockSnapshot,
    inputs: &ClockInputs,
) -> (ShowClockSnapshot, ClockSource) {
    match inputs.mode {
        ClockOverride::ForceDerived => return (derived, ClockSource::Derived),
        ClockOverride::ForceLink => {
            if let Some(s) = inputs.link {
                return (s, ClockSource::Link);
            }
        }
        ClockOverride::ForceProDjLink => {
            if let Some(s) = inputs.prolink {
                return (s, ClockSource::ProDjLink);
            }
        }
        ClockOverride::Auto => {}
    }
    // Auto, or a forced source that is not live → highest-priority live source.
    if let Some(s) = inputs.link {
        (s, ClockSource::Link)
    } else if let Some(s) = inputs.prolink {
        (s, ClockSource::ProDjLink)
    } else {
        (derived, ClockSource::Derived)
    }
}

/// Build a `ShowClockSnapshot` from a Pro DJ Link master-deck beat. `bpm` and
/// `beat_in_measure` (1‑4) come from the CDJ; `since_beat_secs` is the time
/// since the last beat packet, used to interpolate the intra-beat phase.
pub fn prolink_clock(bpm: f32, beat_in_measure: u8, since_beat_secs: f32) -> ShowClockSnapshot {
    let bpm = bpm.max(1.0);
    let beat_phase = (since_beat_secs.max(0.0) * bpm / 60.0).fract();
    // beat_in_measure is 1‑4; place us within a 4-beat bar.
    let bar_beat = (beat_in_measure.max(1) - 1) as f64 + beat_phase as f64;
    ShowClockSnapshot {
        tempo_bpm: bpm,
        beat: bar_beat,
        phase: beat_phase,
        // Pro DJ Link carries beat-in-bar, not phrase — quantum is the bar.
        quantum: 4.0,
        running: true,
        peer_count: 1,
        confidence: 1.0,
        source: ShowClockSource::ExternalBridge,
    }
}

/// Build a `ShowClockSnapshot` from an Ableton Link session tempo. The Link
/// listener decodes the session BPM only (not the beat grid), so the beat
/// phase here free-runs at that tempo off the audio engine's `frame_time`:
/// tempo-accurate, but not yet downbeat-aligned to Ableton's bar 1 (decoding
/// Link's beat origin is a later refinement).
pub fn link_clock(bpm: f32, frame_time: f32) -> ShowClockSnapshot {
    let bpm = bpm.max(1.0);
    let beats = (frame_time.max(0.0) as f64) * bpm as f64 / 60.0;
    ShowClockSnapshot {
        tempo_bpm: bpm,
        beat: beats,
        phase: (beats.fract() as f32).rem_euclid(1.0),
        quantum: BEATS_PER_PHRASE,
        running: true,
        peer_count: 1,
        confidence: 1.0,
        source: ShowClockSource::ExternalBridge,
    }
}

fn family_index(family: FixtureFamily) -> usize {
    match family {
        FixtureFamily::Wash => 0,
        FixtureFamily::Mover => 1,
        FixtureFamily::Gobo => 2,
        FixtureFamily::Laser => 3,
        FixtureFamily::Strobe => 4,
        FixtureFamily::Blinder => 5,
    }
}

/// Full capability set per family, so the evaluator's capability gates pass.
/// The demo rig's fixtures are single-purpose; capability discovery from the
/// real `FixturePatch` is a slice-3 refinement.
fn all_family_caps() -> Vec<FixtureFamilyCapabilities> {
    FAMILY_ORDER
        .iter()
        .map(|&family| {
            let caps = match family {
                FixtureFamily::Wash => vec![FixtureCapability::Dimmer, FixtureCapability::Rgb],
                FixtureFamily::Mover => {
                    vec![FixtureCapability::PanTilt, FixtureCapability::Dimmer]
                }
                FixtureFamily::Gobo => vec![FixtureCapability::Gobo, FixtureCapability::Dimmer],
                FixtureFamily::Laser => vec![FixtureCapability::LaserMacro],
                FixtureFamily::Strobe => vec![FixtureCapability::Strobe],
                FixtureFamily::Blinder => {
                    vec![FixtureCapability::Dimmer, FixtureCapability::White]
                }
            };
            FixtureFamilyCapabilities::new(family, caps)
        })
        .collect()
}

/// Map a fixture's render identity (`role` + `visual_kind`) to a show-brain
/// lane family. The live patch model has only three roles, so `visual_kind`
/// carries most of the signal; movers are the `Accent` role. This mapping is
/// revisited in slice 3 when wash/mover/gobo/blinder join the bridge.
pub fn family_for(role: FixtureRole, visual_kind: StageVisualKind) -> FixtureFamily {
    match visual_kind {
        StageVisualKind::Laser => FixtureFamily::Laser,
        StageVisualKind::Strobe => FixtureFamily::Strobe,
        StageVisualKind::Gobo => FixtureFamily::Gobo,
        StageVisualKind::Beam | StageVisualKind::Wash | StageVisualKind::LedPanel => match role {
            FixtureRole::Accent => FixtureFamily::Mover,
            _ => FixtureFamily::Wash,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cue with audio playing, a coherent field, and a drop in progress.
    fn drop_cue() -> CueSnapshot {
        CueSnapshot {
            energy: 0.9,
            energy_slow: 0.8,
            stability: 0.85,
            drop_impact: 0.7,
            frame_time: 0.0,
            kick: true,
            ..Default::default()
        }
    }

    fn drop_metrics() -> LightingMetrics {
        LightingMetrics {
            energy: 0.9,
            stability: 0.85,
            beat_confidence: 0.9,
            raw_energy: 0.01,
            kick: true,
            onset: true,
            ..Default::default()
        }
    }

    #[test]
    fn silence_keeps_intense_lanes_dark() {
        let mut brain = ShowBrain::new();
        let frame = brain.tick(&CueSnapshot::default(), &LightingMetrics::default(), &ClockInputs::default());

        assert_eq!(frame.arc_state, ArcState::Silence);
        assert!(!frame.lane(FixtureFamily::Laser).eligible);
        assert!(!frame.lane(FixtureFamily::Strobe).eligible);
    }

    #[test]
    fn drop_with_coherent_field_fires_laser() {
        // Lane-machinery test: pin the full vocabulary (under Auto a fresh
        // brain starts in resolved Chill, which bans the laser by design).
        let mut brain = ShowBrain::new();
        brain.set_style(crate::emerge::LightingStyle::Banger);
        let frame = brain.tick(&drop_cue(), &drop_metrics(), &ClockInputs::default());

        assert_eq!(frame.arc_state, ArcState::Drop);
        assert!(frame.lane(FixtureFamily::Laser).eligible);
    }

    #[test]
    fn cooldown_suppresses_immediate_strobe_refire() {
        // Lane-machinery test: pin the full vocabulary (Auto starts chill).
        let mut brain = ShowBrain::new();
        brain.set_style(crate::emerge::LightingStyle::Banger);
        let cue = drop_cue();
        let metrics = drop_metrics();

        // First drop frame: the strobe is allowed to fire.
        let first = brain.tick(&cue, &metrics, &ClockInputs::default());
        assert!(first.lane(FixtureFamily::Strobe).eligible);

        // Same frame_time, still inside the cooldown window: suppressed.
        let second = brain.tick(&cue, &metrics, &ClockInputs::default());
        let strobe = second.lane(FixtureFamily::Strobe);
        assert!(!strobe.eligible);
        assert_eq!(strobe.reason, "lane cooldown active");
    }

    #[test]
    fn tick_consumes_the_app_resolved_clock() {
        // COM-499 Phase B2: the app resolves the clock once per frame above the
        // render throttle; the render `tick` at the same frame_time must consume
        // that exact resolution (no re-derive, no double estimator update).
        let mut brain = ShowBrain::new();
        let cue = drop_cue();
        let metrics = drop_metrics();
        let inputs = ClockInputs::default();
        let resolved = brain.resolve_clock(&cue, &metrics, &inputs);
        let frame = brain.tick(&cue, &metrics, &inputs);
        assert_eq!(frame.clock.beat, resolved.beat);
        assert_eq!(frame.clock.tempo_bpm, resolved.tempo_bpm);
        assert_eq!(frame.clock_source, ClockSource::Derived);
    }

    fn clock_at(bpm: f32) -> ShowClockSnapshot {
        ShowClockSnapshot { tempo_bpm: bpm, ..Default::default() }
    }

    #[test]
    fn auto_picks_derived_when_no_external_source() {
        let (clk, src) = select_clock(clock_at(122.0), &ClockInputs::default());
        assert_eq!(src, ClockSource::Derived);
        assert_eq!(clk.tempo_bpm, 122.0);
    }

    #[test]
    fn auto_prefers_link_over_prodjlink() {
        let inputs = ClockInputs {
            prolink: Some(clock_at(128.0)),
            link: Some(clock_at(140.0)),
            mode: ClockOverride::Auto,
        };
        let (clk, src) = select_clock(clock_at(122.0), &inputs);
        assert_eq!(src, ClockSource::Link);
        assert_eq!(clk.tempo_bpm, 140.0);
    }

    #[test]
    fn auto_uses_prodjlink_when_no_link() {
        let inputs = ClockInputs { prolink: Some(clock_at(128.0)), ..Default::default() };
        let (clk, src) = select_clock(clock_at(122.0), &inputs);
        assert_eq!(src, ClockSource::ProDjLink);
        assert_eq!(clk.tempo_bpm, 128.0);
    }

    #[test]
    fn force_prodjlink_beats_an_available_link() {
        let inputs = ClockInputs {
            prolink: Some(clock_at(128.0)),
            link: Some(clock_at(140.0)),
            mode: ClockOverride::ForceProDjLink,
        };
        let (_, src) = select_clock(clock_at(122.0), &inputs);
        assert_eq!(src, ClockSource::ProDjLink);
    }

    #[test]
    fn forcing_an_absent_source_falls_back_gracefully() {
        // Force Link, but Link is not live — fall through to the next live source.
        let inputs = ClockInputs {
            prolink: Some(clock_at(128.0)),
            link: None,
            mode: ClockOverride::ForceLink,
        };
        let (_, src) = select_clock(clock_at(122.0), &inputs);
        assert_eq!(src, ClockSource::ProDjLink);
    }

    #[test]
    fn force_derived_always_wins() {
        let inputs = ClockInputs {
            prolink: Some(clock_at(128.0)),
            link: Some(clock_at(140.0)),
            mode: ClockOverride::ForceDerived,
        };
        let (clk, src) = select_clock(clock_at(122.0), &inputs);
        assert_eq!(src, ClockSource::Derived);
        assert_eq!(clk.tempo_bpm, 122.0);
    }

    #[test]
    fn fold_tempo_lands_in_canonical_octave() {
        assert!((fold_tempo(60.0) - 120.0).abs() < 0.01); // 60 → ×2
        assert!((fold_tempo(200.0) - 100.0).abs() < 0.01); // 200 → ÷2
        assert!((fold_tempo(128.0) - 128.0).abs() < 0.01); // already in range
        assert!((fold_tempo(75.0) - 150.0).abs() < 0.01); // 75 → ×2
    }

    #[test]
    fn fold_tempo_rejects_nonsense() {
        assert_eq!(fold_tempo(0.0), TEMPO_BPM);
        assert_eq!(fold_tempo(f32::NAN), TEMPO_BPM);
    }

    #[test]
    fn fold_to_range_unit() {
        // center = sqrt(lo*hi): Lo-Fi/Chill 70–90 → ~79.4.
        let c_lofi = (70.0_f32 * 90.0).sqrt();
        assert!((fold_to_range(150.0, c_lofi) - 75.0).abs() < 0.01, "150→75");
        assert!((fold_to_range(160.0, c_lofi) - 80.0).abs() < 0.01, "160→80");
        assert!((fold_to_range(75.0, c_lofi) - 75.0).abs() < 0.01, "75 stays");
        // Trap 130–160 → ~144.2: a 75 BPM read folds UP an octave to 150.
        let c_trap = (130.0_f32 * 160.0).sqrt();
        assert!((fold_to_range(75.0, c_trap) - 150.0).abs() < 0.01, "75→150");
        // Nonsense folds to the safe default.
        assert_eq!(fold_to_range(0.0, c_trap), TEMPO_BPM);
        assert_eq!(fold_to_range(f32::NAN, c_trap), TEMPO_BPM);
    }

    #[test]
    fn octave_log_dist_treats_octaves_as_near() {
        // Same pitch class an octave apart → ~0 distance; a tritone-ish offset is large.
        assert!(octave_log_dist(75.0, 150.0) < 1e-4, "octave is near-zero");
        assert!(octave_log_dist(150.0, 150.0) < 1e-6, "identity is zero");
        assert!(octave_log_dist(106.0, 150.0) > 0.2, "half-octave is far");
    }

    /// Drive the estimator with a synthetic onset envelope at `bpm` for `secs`,
    /// sampled at `ODF_FS`. `offbeat` adds a weaker hit on every half-beat — the
    /// off-beat/8th-note contamination that biased the old median estimator high.
    fn feed_tempo(est: &mut TempoEstimator, bpm: f32, secs: f32, offbeat: bool, start: f64) {
        let beat = (60.0 / bpm) as f64;
        let steps = (secs * ODF_FS) as u32;
        for i in 0..steps {
            let now = start + i as f64 / ODF_FS as f64; // continuous clock across segments
            let phase = ((now % beat) / beat) as f32; // 0..1 within the beat
            let onset = if phase < 0.04 {
                1.0
            } else if offbeat && (phase - 0.5).abs() < 0.04 {
                0.4
            } else {
                0.0
            };
            est.update(onset, now);
        }
    }

    fn assert_locks(bpm: f32, offbeat: bool) {
        let mut est = TempoEstimator::new();
        feed_tempo(&mut est, bpm, 10.0, offbeat, 0.0);
        let err = (est.bpm - bpm).abs() / bpm;
        assert!(err < 0.02, "expected ~{bpm} BPM, got {} (offbeat={offbeat})", est.bpm);
    }

    #[test]
    fn autocorr_locks_common_tempos() {
        assert_locks(90.0, false);
        assert_locks(116.0, false);
        assert_locks(128.0, false);
        assert_locks(150.0, false);
    }

    #[test]
    fn autocorr_rejects_offbeat_contamination() {
        // 116/128 with a weaker hit on every off-beat (the kick-doubling that made
        // the old median estimator read ~125.8) — must still lock to the beat.
        assert_locks(116.0, true);
        assert_locks(128.0, true);
    }

    #[test]
    fn autocorr_folds_half_time() {
        // A 60 BPM beat folds up into the 85–170 canonical octave (→ 120).
        let mut est = TempoEstimator::new();
        feed_tempo(&mut est, 60.0, 12.0, false, 0.0);
        assert!((est.bpm - 120.0).abs() < 120.0 * 0.03, "bpm was {}", est.bpm);
    }

    #[test]
    fn autocorr_reseeds_on_tempo_change() {
        let mut est = TempoEstimator::new();
        feed_tempo(&mut est, 100.0, 10.0, false, 0.0);
        assert!((est.bpm - 100.0).abs() < 4.0, "first lock: {}", est.bpm);
        // New song (continuous clock): must re-lock fast (snap), not EMA-crawl.
        feed_tempo(&mut est, 140.0, 10.0, false, 10.0);
        assert!((est.bpm - 140.0).abs() < 6.0, "re-lock: {}", est.bpm);
    }

    #[test]
    fn autocorr_realistic_116_with_bass_noise() {
        // Mimic the real feed: ~60 fps, a strong onset on each 116 BPM beat PLUS
        // continuous low-level bass flux on other frames (the kick-envelope `rise`
        // is dense, not a clean impulse). Reproduces the field report (~129).
        let mut est = TempoEstimator::new();
        let fps = 60.0_f64;
        let beat = 60.0 / 116.0_f64;
        let mut x: u32 = 0x1234_5678;
        let steps = (20.0 * fps) as u32;
        for i in 0..steps {
            let now = i as f64 / fps;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let noise = (x & 0xff) as f32 / 255.0 * 0.4;
            let phase = ((now % beat) / beat) as f32;
            let onset = if phase < 0.05 { 1.0 } else { noise };
            est.update(onset, now);
        }
        assert!((est.bpm - 116.0).abs() < 116.0 * 0.025, "got {}", est.bpm);
    }

    #[test]
    fn no_hint_preserves_canonical_octave() {
        // `None` range => unchanged behavior: a 60 BPM beat folds to 120 (85–170).
        let mut est = TempoEstimator::new();
        assert!(est.tempo_range.is_none());
        feed_tempo(&mut est, 60.0, 12.0, false, 0.0);
        assert!((est.bpm - 120.0).abs() < 120.0 * 0.03, "bpm was {}", est.bpm);
    }

    #[test]
    fn hint_resolves_double_time_low() {
        // A clean 75 BPM beat (the Mogwai "Take Me Somewhere Nice" case). With no
        // hint it folds UP into the 85–170 octave (→ 150). A Lo-Fi/Chill genre hint
        // (70–90) keeps it at the true 75 — proving the prior resolves the octave.
        let mut plain = TempoEstimator::new();
        feed_tempo(&mut plain, 75.0, 12.0, false, 0.0);
        assert!((plain.bpm - 150.0).abs() < 150.0 * 0.03, "no-hint bpm was {}", plain.bpm);

        let mut hinted = TempoEstimator::new();
        hinted.set_range(Some((70.0, 90.0)));
        feed_tempo(&mut hinted, 75.0, 12.0, false, 0.0);
        assert!((hinted.bpm - 75.0).abs() < 75.0 * 0.03, "hinted bpm was {}", hinted.bpm);
    }

    #[test]
    fn hint_resolves_double_time_high() {
        // Same 75 BPM beat, but a Trap hint (130–160) pushes the octave UP to 150.
        let mut hinted = TempoEstimator::new();
        hinted.set_range(Some((130.0, 160.0)));
        feed_tempo(&mut hinted, 75.0, 12.0, false, 0.0);
        assert!((hinted.bpm - 150.0).abs() < 150.0 * 0.03, "hinted bpm was {}", hinted.bpm);
    }

    #[test]
    fn set_range_filters_invalid() {
        // Zero-width / inverted / non-positive ranges collapse to None (no guidance).
        let mut est = TempoEstimator::new();
        est.set_range(Some((120.0, 120.0)));
        assert!(est.tempo_range.is_none(), "zero-width → None");
        est.set_range(Some((160.0, 120.0)));
        assert!(est.tempo_range.is_none(), "inverted → None");
        est.set_range(Some((0.0, 120.0)));
        assert!(est.tempo_range.is_none(), "non-positive lo → None");
        est.set_range(Some((70.0, 90.0)));
        assert_eq!(est.tempo_range, Some((70.0, 90.0)), "valid kept");
        est.set_range(None);
        assert!(est.tempo_range.is_none(), "explicit None");
    }

    #[test]
    fn autocorr_low_confidence_on_noise() {
        // Deterministic pseudo-noise ODF — no periodicity → low confidence.
        let mut est = TempoEstimator::new();
        let mut x: u32 = 0x9e37_79b9;
        for i in 0..900u32 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5; // xorshift32
            let onset = (x & 0xffff) as f32 / 65535.0;
            est.update(onset, i as f64 / ODF_FS as f64);
        }
        assert!(est.confidence < 0.4, "confidence was {}", est.confidence);
    }

    #[test]
    fn weak_confidence_estimates_do_not_walk_the_bpm() {
        // Lock to a clean 128, then feed pure aperiodic noise (a breakdown /
        // ambient bridge): the locked tempo must HOLD. The old unconditional
        // EMA chased whatever the noisy ACF peak said within ~150 ms.
        let mut est = TempoEstimator::new();
        feed_tempo(&mut est, 128.0, 10.0, false, 0.0);
        let locked = est.bpm;
        assert!((locked - 128.0).abs() < 3.0, "locked {locked}");
        let mut x: u32 = 0xdead_beef;
        for i in 0..1500u32 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let onset = (x & 0xffff) as f32 / 65535.0 * 0.3;
            est.update(onset, 10.0 + i as f64 / ODF_FS as f64);
        }
        assert!(
            (est.bpm - locked).abs() < locked * 0.04,
            "BPM random-walked through a beatless passage: {} -> {}",
            locked,
            est.bpm
        );
    }

    #[test]
    fn seed_snaps_bpm_and_keeps_tracking() {
        let mut est = TempoEstimator::new();
        est.seed(98.0);
        assert_eq!(est.bpm, 98.0);
        assert!(est.confidence >= 0.6, "seed grants provisional confidence");
        // Nonsense seeds are rejected.
        est.seed(f32::NAN);
        est.seed(1000.0);
        assert_eq!(est.bpm, 98.0);
        // Live audio at a different tempo still re-locks (stale seed corrects).
        feed_tempo(&mut est, 128.0, 12.0, false, 0.0);
        assert!((est.bpm - 128.0).abs() < 4.0, "re-locked off a stale seed: {}", est.bpm);
    }

    #[test]
    fn exposure_budget_demotes_strobe_under_blinder() {
        use crate::emerge::CuePriority;
        // The white-on-white drop stack: light both Blinder and Strobe. The
        // governor keeps the Blinder (the bigger PhraseIntense white event) and
        // yields the Strobe, so the rig cannot clip to full white.
        let mut lanes: [CueLaneIntent; 6] =
            std::array::from_fn(|_| CueLaneIntent::inactive(FixtureFamily::Wash, CuePriority::Bed, "seed"));
        lanes[family_index(FixtureFamily::Blinder)].eligible = true;
        lanes[family_index(FixtureFamily::Strobe)].eligible = true;

        ShowBrain::enforce_exposure_budget(&mut lanes, false);

        assert!(lanes[family_index(FixtureFamily::Blinder)].eligible, "blinder keeps priority");
        assert!(!lanes[family_index(FixtureFamily::Strobe)].eligible, "strobe yields to blinder");
        assert_eq!(
            lanes[family_index(FixtureFamily::Strobe)].reason,
            "strobe yielded to blinder (exposure budget)"
        );
    }

    #[test]
    fn exposure_budget_leaves_strobe_when_no_blinder() {
        use crate::emerge::CuePriority;
        let mut lanes: [CueLaneIntent; 6] =
            std::array::from_fn(|_| CueLaneIntent::inactive(FixtureFamily::Wash, CuePriority::Bed, "seed"));
        lanes[family_index(FixtureFamily::Strobe)].eligible = true; // blinder stays inactive

        ShowBrain::enforce_exposure_budget(&mut lanes, false);

        assert!(lanes[family_index(FixtureFamily::Strobe)].eligible, "strobe survives without a blinder");
    }

    #[test]
    fn exposure_budget_holds_through_the_blinder_latch() {
        use crate::emerge::CuePriority;
        // The blinder LANE is momentary — its eligibility is a 1-frame pulse,
        // so during most of a BlindersHold the lane reads ineligible. The
        // strobe yield must key off the macro latch and cover the WHOLE hold.
        let mut lanes: [CueLaneIntent; 6] =
            std::array::from_fn(|_| CueLaneIntent::inactive(FixtureFamily::Wash, CuePriority::Bed, "seed"));
        lanes[family_index(FixtureFamily::Strobe)].eligible = true; // blinder lane NOT eligible

        ShowBrain::enforce_exposure_budget(&mut lanes, true); // macro latch on

        assert!(
            !lanes[family_index(FixtureFamily::Strobe)].eligible,
            "strobe yields for the whole BlindersHold, not just the co-fire frame"
        );
    }

    #[test]
    fn laser_duty_caps_continuous_on() {
        use crate::emerge::CuePriority;
        // The laser is momentary:false, so enforce_cooldowns never bounds it — a
        // held-eligible groove would otherwise run the laser continuously. The
        // duty cap must force a refractory gap so it punctuates instead.
        let mut brain = ShowBrain::new();
        let live_laser = || {
            let mut lanes: [CueLaneIntent; 6] =
                std::array::from_fn(|_| CueLaneIntent::inactive(FixtureFamily::Wash, CuePriority::Bed, "seed"));
            let laser = &mut lanes[family_index(FixtureFamily::Laser)];
            laser.eligible = true;
            laser.reason = "live";
            lanes
        };
        let is_on = |lanes: &[CueLaneIntent; 6]| lanes[family_index(FixtureFamily::Laser)].eligible;

        // Held eligible from t=0: on up to the cap...
        let mut lanes = live_laser();
        brain.enforce_laser_duty(&mut lanes, 0.0);
        assert!(is_on(&lanes), "laser on at t=0");
        let mut lanes = live_laser();
        brain.enforce_laser_duty(&mut lanes, LASER_MAX_ON_S - 0.1);
        assert!(is_on(&lanes), "laser still on just before the cap");
        // ...forced dark once it has been continuously on past the cap...
        let mut lanes = live_laser();
        brain.enforce_laser_duty(&mut lanes, LASER_MAX_ON_S + 0.1);
        assert!(!is_on(&lanes), "laser forced dark at the continuous-on cap");
        // ...held dark through the refractory even though still eligible...
        let mut lanes = live_laser();
        brain.enforce_laser_duty(&mut lanes, LASER_MAX_ON_S + 0.1 + LASER_REFRACTORY_S - 1.0);
        assert!(!is_on(&lanes), "laser dark during the refractory gap");
        // ...and allowed again after the refractory clears.
        let mut lanes = live_laser();
        brain.enforce_laser_duty(&mut lanes, LASER_MAX_ON_S + 0.2 + LASER_REFRACTORY_S);
        assert!(is_on(&lanes), "laser fires again after the refractory");
    }

    #[test]
    fn laser_duty_cap_survives_eligibility_flicker() {
        use crate::emerge::CuePriority;
        // A 1-frame ineligible blip (momentary stability dip) must NOT
        // relaunch the 4 s continuous-on budget — the old code reset the start
        // time on ANY ineligible frame, so a flickering lane held ~100% duty
        // forever and the "hard exposure ceiling" leaked.
        let mut brain = ShowBrain::new();
        let live_laser = || {
            let mut lanes: [CueLaneIntent; 6] = std::array::from_fn(|_| {
                CueLaneIntent::inactive(FixtureFamily::Wash, CuePriority::Bed, "seed")
            });
            let laser = &mut lanes[family_index(FixtureFamily::Laser)];
            laser.eligible = true;
            laser.reason = "live";
            lanes
        };
        let dark_laser = || {
            let lanes: [CueLaneIntent; 6] = std::array::from_fn(|_| {
                CueLaneIntent::inactive(FixtureFamily::Wash, CuePriority::Bed, "seed")
            });
            lanes
        };
        let is_on = |lanes: &[CueLaneIntent; 6]| lanes[family_index(FixtureFamily::Laser)].eligible;

        let mut lanes = live_laser();
        brain.enforce_laser_duty(&mut lanes, 0.0); // budget opens
        let mut lanes = live_laser();
        brain.enforce_laser_duty(&mut lanes, 3.9);
        assert!(is_on(&lanes), "still inside the budget");
        let mut lanes = dark_laser();
        brain.enforce_laser_duty(&mut lanes, 3.93); // 1-frame flicker (< dark-reset)
        let mut lanes = live_laser();
        brain.enforce_laser_duty(&mut lanes, LASER_MAX_ON_S + 0.1);
        assert!(!is_on(&lanes), "budget survived the flicker — cap still enforced");

        // A REAL dark gap (>= LASER_DARK_RESET_S) does reset the budget.
        let mut brain = ShowBrain::new();
        let mut lanes = live_laser();
        brain.enforce_laser_duty(&mut lanes, 0.0);
        let mut lanes = dark_laser();
        brain.enforce_laser_duty(&mut lanes, 3.0); // goes dark...
        let mut lanes = dark_laser();
        brain.enforce_laser_duty(&mut lanes, 3.0 + LASER_DARK_RESET_S + 0.05); // ...and stays dark
        let mut lanes = live_laser();
        brain.enforce_laser_duty(&mut lanes, LASER_MAX_ON_S + 1.0);
        assert!(is_on(&lanes), "a real gap re-opens a fresh budget");
    }

    #[test]
    fn derived_clock_phase_is_continuous_across_tempo_changes() {
        // Old bug: beats were recomputed as frame_time * bpm / 60, so every
        // tempo-estimate change teleported the absolute beat count by
        // t·Δbpm/60 (±3 beats at 1 h uptime for a 0.05 BPM step) — beat-synced
        // strobes and phrase hits wandered with uptime. The integrated clock
        // bends its RATE on a tempo change; the count stays continuous.
        let mut brain = ShowBrain::new();
        let inputs = ClockInputs::default();
        let metrics = LightingMetrics::default();
        let mut cue = CueSnapshot { dt: 1.0 / 60.0, ..Default::default() };
        let mut last_beat = 0.0f64;
        for i in 1..=600 {
            cue.frame_time = i as f32 / 60.0;
            if i == 300 {
                // Mid-run re-lock, as a metadata seed or reseed snap would do.
                brain.seed_tempo(150.0);
            }
            let clk = brain.resolve_clock(&cue, &metrics, &inputs);
            let advance = clk.beat - last_beat;
            assert!(advance >= 0.0, "beat is monotone (frame {i})");
            assert!(
                advance < 0.1,
                "per-frame advance stays continuous through the tempo change (frame {i}: {advance})"
            );
            last_beat = clk.beat;
        }
        // The rate genuinely bent: ~5 s at the 122 default + ~5 s at 150.
        assert!(
            last_beat > 15.0 && last_beat < 25.0,
            "integrated total reflects both tempos, got {last_beat}"
        );
    }

    #[test]
    fn prolink_beat_is_monotone_across_bar_wraps() {
        // The CDJ bridge supplies beat-in-bar (0..4). Fed raw into the cue
        // grammar it wedged every effect with a period over one bar and froze
        // phrase_count at 0. The resolver must accumulate an absolute count.
        let mut brain = ShowBrain::new();
        let metrics = LightingMetrics::default();
        let mut cue = CueSnapshot { dt: 1.0 / 60.0, ..Default::default() };
        let mut prev = -1.0f64;
        for i in 1..=400 {
            cue.frame_time = i as f32 / 60.0;
            let t = i as f32 / 60.0;
            // 120 BPM => 2 beats/s; the 4-beat bar wraps every 2 s.
            let abs_beat = t * 2.0;
            let beat_in_measure = ((abs_beat as u32) % 4 + 1) as u8;
            let since_beat = abs_beat.fract() / 2.0;
            let inputs = ClockInputs {
                prolink: Some(prolink_clock(120.0, beat_in_measure, since_beat)),
                ..Default::default()
            };
            let clk = brain.resolve_clock(&cue, &metrics, &inputs);
            assert!(
                clk.beat >= prev,
                "monotone beat under a CDJ clock: {prev} -> {} (frame {i})",
                clk.beat
            );
            assert_eq!(clk.quantum, BEATS_PER_PHRASE, "CDJ clock carries the phrase quantum");
            prev = clk.beat;
        }
        // ~6.7 s at 2 beats/s ≈ 13 beats — far past the 0..4 bar-local range.
        assert!(prev > 8.0, "absolute beat accumulated across bars, got {prev}");
    }

    #[test]
    fn genre_profile_gates_the_physical_lanes() {
        // The operator's genre must reach the DMX lane gates: an ambient-style
        // fixture profile keeps laser/strobe closed on the exact drop frame
        // that opens them under the default profile (see
        // drop_with_coherent_field_fires_laser).
        let mut brain = ShowBrain::new();
        brain.set_genre(FixtureGenreProfile {
            strobe_tolerance: 0.0,
            laser_tolerance: 0.15,
            ..FixtureGenreProfile::default()
        });
        let frame = brain.tick(&drop_cue(), &drop_metrics(), &ClockInputs::default());
        assert!(!frame.lane(FixtureFamily::Laser).eligible, "chill genre closes the laser lane");
        assert!(!frame.lane(FixtureFamily::Strobe).eligible, "chill genre closes the strobe lane");
    }

    #[test]
    fn chill_clamps_license_and_kills_intense_lanes() {
        // Chill on an AGGRESSIVE genre with material that licenses hard: the
        // style must cap the license band AND close the laser/strobe lanes
        // on the exact drop frame that opens them in Auto — the
        // most-restrictive-wins composition.
        let mut brain = ShowBrain::new();
        brain.set_genre(FixtureGenreProfile {
            strobe_tolerance: 0.85,
            laser_tolerance: 0.8,
            density: 0.85,
            ..FixtureGenreProfile::default()
        });
        brain.set_style(crate::emerge::LightingStyle::Chill);
        run_material(&mut brain, 12.0, 25, 0.8); // beat-driven, full spectrum
        assert!(
            brain.intensity_license() <= 0.15 + 1e-6,
            "chill caps the license even on licensing material, got {}",
            brain.intensity_license()
        );

        let frame = brain.tick(&drop_cue(), &drop_metrics(), &ClockInputs::default());
        assert!(!frame.lane(FixtureFamily::Laser).eligible, "chill never fires lasers");
        assert!(!frame.lane(FixtureFamily::Strobe).eligible, "chill never fires strobes");
        assert!(!frame.lane(FixtureFamily::Blinder).eligible, "chill never fires blinders");

        // The clamp is what holds the output down, not a dead dial: pinning
        // a hotter posture releases it immediately. (The raw dial kept
        // tracking the music underneath the whole time.)
        assert!(
            brain.raw_intensity_license() > 0.6,
            "the raw dial tracked the music under the chill clamp, got {}",
            brain.raw_intensity_license()
        );
        brain.set_style(crate::emerge::LightingStyle::Energetic);
        run_material(&mut brain, 1.0, 25, 0.8);
        assert!(
            brain.intensity_license() > 0.4,
            "releasing the chill clamp re-arms the output license, got {}",
            brain.intensity_license()
        );
    }

    #[test]
    fn auto_mode_resolves_postures_from_material() {
        // Default operator style (Auto): the resolver is wired end-to-end —
        // it flips the EFFECTIVE posture from the material itself. Chill is
        // the default and the steady state; the precise z-driven transitions
        // (Energetic lift, drop→Banger, no-floor) are unit-tested on the
        // resolver. This asserts the show-brain plumbing: posture drives the
        // license clamp + genre gates.
        let mut brain = ShowBrain::new();
        assert_eq!(brain.effective_style(), crate::emerge::LightingStyle::Chill);

        // Sustained quiet wall stays Chill — clamp + genre zeroing follow,
        // lanes dark.
        run_material_bands(&mut brain, 20.0, 0, 0.1, 0.01, 0.004);
        assert_eq!(
            brain.effective_style(),
            crate::emerge::LightingStyle::Chill,
            "a sustained wall stays chill"
        );
        assert!(
            brain.intensity_license() <= 0.15 + 1e-6,
            "auto-chill clamps the license, got {}",
            brain.intensity_license()
        );
        let frame = brain.tick(&drop_cue(), &drop_metrics(), &ClockInputs::default());
        assert!(!frame.lane(FixtureFamily::Laser).eligible, "auto-chill bans lasers");
        assert!(!frame.lane(FixtureFamily::Strobe).eligible, "auto-chill bans strobes");

        // The raw dial keeps tracking the music UNDER the auto-chill clamp
        // (the wall's low energy, then a percussive lift) — proving the
        // resolver clamps the OUTPUT without freezing the underlying signal,
        // so a return to a hotter posture would snap straight back.
        run_material(&mut brain, 6.0, 25, 0.8);
        assert!(
            brain.raw_intensity_license() > 0.4,
            "the raw dial tracks the music under the auto-chill clamp, got {}",
            brain.raw_intensity_license()
        );
    }

    #[test]
    fn banger_floor_holds_but_silence_stays_dark() {
        // Banger floors the license on material that would soften it (thin
        // wall: no grid, thin spectrum) — but TRUE silence still reads dark
        // because the arc classifier and energy gates are license-independent.
        let mut brain = ShowBrain::new();
        brain.set_style(crate::emerge::LightingStyle::Banger);
        run_material_bands(&mut brain, 12.0, 0, 0.1, 0.01, 0.004); // wall
        assert!(
            brain.intensity_license() >= 0.85,
            "banger floors the license on a wall, got {}",
            brain.intensity_license()
        );

        // Silence: default snapshot/metrics force the Silence arc; every
        // intense lane stays dark despite the floored license.
        let frame = brain.tick(
            &CueSnapshot::default(),
            &LightingMetrics::default(),
            &ClockInputs::default(),
        );
        assert_eq!(frame.arc_state, ArcState::Silence, "raw silence wins over the floor");
        assert!(!frame.lane(FixtureFamily::Laser).eligible);
        assert!(!frame.lane(FixtureFamily::Strobe).eligible);
        assert!(!frame.lane(FixtureFamily::Blinder).eligible);
    }

    /// Run real ticks at 60fps for `secs`, flagging an onset every
    /// `onset_every` frames (0 = never), with raw band levels for the
    /// spectral-fullness side of the intensity license.
    fn run_material_bands(
        brain: &mut ShowBrain,
        secs: f32,
        onset_every: u32,
        beat_conf: f32,
        raw_bass: f32,
        raw_high: f32,
    ) {
        let frames = (secs * 60.0) as u32;
        let base = brain.last_now.unwrap_or(0.0);
        for i in 1..=frames {
            let cue = CueSnapshot {
                energy: 0.8,
                energy_slow: 0.7,
                frame_time: base + i as f32 / 60.0,
                dt: 1.0 / 60.0,
                ..Default::default()
            };
            let percussive = onset_every > 0 && i % onset_every == 0;
            let metrics = LightingMetrics {
                energy: 0.8,
                raw_energy: 0.01,
                raw_bass,
                raw_high,
                beat_confidence: beat_conf,
                kick: percussive,
                onset: percussive,
                ..Default::default()
            };
            brain.tick(&cue, &metrics, &ClockInputs::default());
        }
    }

    fn run_material(brain: &mut ShowBrain, secs: f32, onset_every: u32, beat_conf: f32) {
        // Full-spectrum material by default (license follows percussiveness).
        run_material_bands(brain, secs, onset_every, beat_conf, 0.08, 0.06);
    }

    #[test]
    fn percussiveness_separates_walls_from_beats() {
        // Four-on-the-floor-ish material (onset ~2.4/s + confident beat) must
        // read percussive; a sustained wall (no onsets, no grid) must read
        // atmospheric — at the SAME energy. This is the signal that lets both
        // ends of the energy spectrum run autonomously.
        let mut edm = ShowBrain::new();
        run_material(&mut edm, 12.0, 25, 0.8); // onset every 25 frames ≈ 2.4/s
        assert!(
            edm.percussiveness > 0.6,
            "beat-driven material reads percussive, got {}",
            edm.percussiveness
        );

        let mut wall = ShowBrain::new();
        run_material(&mut wall, 12.0, 0, 0.1); // sustained wall, no grid
        assert!(
            wall.percussiveness < PERC_LANE_MIN,
            "an atmospheric wall reads non-percussive, got {}",
            wall.percussiveness
        );
    }

    #[test]
    fn license_softens_in_a_vocal_bridge_and_rearms_for_the_drop() {
        // The genre-free within-song dial: a dynamic EDM track's vocal bridge
        // keeps the kick GRID (percussiveness stays high) but thins the hats
        // and lightens the bass — the intensity license must fall (softer
        // strobes/movers/Pump) and recover within ~2 s of the spectrum
        // refilling at the drop. Asserted on the RAW dial: the style clamp
        // sits on top of it (the auto resolver's posture-level reading of
        // the same dial is covered by auto_mode_resolves_postures_*).
        let mut brain = ShowBrain::new();
        // Full-power section: dense grid, bass + hats riding at peak.
        run_material_bands(&mut brain, 12.0, 25, 0.8, 0.08, 0.06);
        let full = brain.raw_intensity_license();
        assert!(full > 0.7, "full-spectrum drop section licenses hard, got {full}");

        // Vocal bridge: SAME kick grid + confidence, hats nearly gone, bass
        // pulled back.
        run_material_bands(&mut brain, 10.0, 25, 0.8, 0.03, 0.008);
        let bridge = brain.raw_intensity_license();
        assert!(
            bridge < 0.55,
            "bridge softens the license while the grid persists, got {bridge}"
        );
        assert!(
            brain.percussiveness() > 0.6,
            "the kick grid keeps percussiveness high through the bridge, got {}",
            brain.percussiveness()
        );

        // The drop refills the spectrum — fast attack re-arms within ~2 s.
        run_material_bands(&mut brain, 2.0, 25, 0.8, 0.09, 0.07);
        assert!(
            brain.raw_intensity_license() > 0.7,
            "license re-arms for the drop, got {}",
            brain.raw_intensity_license()
        );
    }

    #[test]
    fn atmosphere_stands_strobe_and_blinder_down() {
        // Drop-shaped frame that WOULD fire the strobe lane — but the material
        // has proven atmospheric, so strobe/blinder stand down regardless of
        // genre, while the laser (coherence-gated, suits sustained material)
        // keeps its own verdict.
        let mut brain = ShowBrain::new();
        run_material(&mut brain, 12.0, 0, 0.1); // prove the material atmospheric
        assert!(brain.percussiveness < PERC_LANE_MIN);

        let frame = brain.tick(&drop_cue(), &drop_metrics(), &ClockInputs::default());
        assert!(
            !frame.lane(FixtureFamily::Strobe).eligible,
            "no strobes without a percussive grid"
        );
        assert!(
            !frame.lane(FixtureFamily::Blinder).eligible,
            "no blinders without a percussive grid"
        );

        // Unit-level: the governor demotes an ELIGIBLE lane with its reason
        // (in the full tick an earlier gate may already have claimed it).
        use crate::emerge::CuePriority;
        let mut lanes: [CueLaneIntent; 6] = std::array::from_fn(|_| {
            CueLaneIntent::inactive(FixtureFamily::Wash, CuePriority::Bed, "seed")
        });
        lanes[family_index(FixtureFamily::Strobe)].eligible = true;
        brain.enforce_atmosphere(&mut lanes);
        let strobe = &lanes[family_index(FixtureFamily::Strobe)];
        assert!(!strobe.eligible);
        assert_eq!(strobe.reason, "no percussive grid (atmosphere)");

        // Control: a fresh brain (benefit-of-the-doubt percussiveness) with
        // the vocabulary pinned open fires — proving the stand-down above
        // came from the atmosphere governor, not the lane machinery.
        let mut fresh = ShowBrain::new();
        fresh.set_style(crate::emerge::LightingStyle::Banger);
        let frame = fresh.tick(&drop_cue(), &drop_metrics(), &ClockInputs::default());
        assert!(frame.lane(FixtureFamily::Strobe).eligible, "percussive default unchanged");
    }

    #[test]
    fn tempo_holds_lock_without_beat_evidence() {
        // Lock cleanly at 128, then feed a textural pseudo-rhythm (the kind a
        // sustained wall autocorrelates at, with FAKE confidence) while beat
        // evidence is low: the lock must hold. Then evidence returns and the
        // estimator re-locks normally.
        let mut est = TempoEstimator::new();
        feed_tempo(&mut est, 128.0, 10.0, false, 0.0);
        let locked = est.bpm;
        assert!((locked - 128.0).abs() < 3.0, "locked {locked}");

        est.beat_evidence = 0.1; // wall: no percussive grid
        feed_tempo(&mut est, 62.4, 15.0, false, 10.0); // the Mogwai ghost
        assert!(
            (est.bpm - locked).abs() < locked * 0.02,
            "tempo chased a textural ghost without beat evidence: {} -> {}",
            locked,
            est.bpm
        );

        est.beat_evidence = 0.9; // drums come in
        feed_tempo(&mut est, 116.0, 12.0, false, 25.0);
        assert!(
            (est.bpm - 116.0).abs() < 6.0,
            "estimator re-locks once evidence returns: {}",
            est.bpm
        );
    }

    #[test]
    fn buried_drums_still_pull_the_clock_patiently() {
        // Post-rock with the kit playing UNDER the wall: evidence sits
        // mid-ramp (~0.4). The clock must still glide to the real tempo —
        // slower than clean material, but it must not stay stuck on the
        // default for the whole song (the hard-cliff regression).
        let mut est = TempoEstimator::new();
        est.beat_evidence = 0.4;
        feed_tempo(&mut est, 115.0, 20.0, false, 0.0);
        assert!(
            (est.bpm - 115.0).abs() < 6.0,
            "mid-evidence drums pull the clock off the 122 default, got {}",
            est.bpm
        );
    }

    // ── Dynamics ────────────────────────────────────────────────────────────

    /// Run ticks at 60fps for `secs` on a kick grid with the given raw bands.
    fn run_bands(brain: &mut ShowBrain, secs: f32, bass: f32, mid: f32, high: f32) -> ShowBrainFrame {
        let frames = (secs * 60.0) as u32;
        let base = brain.last_now.unwrap_or(0.0);
        let mut frame = None;
        for i in 1..=frames {
            let cue = CueSnapshot {
                energy: 0.9,
                energy_slow: 0.8,
                stability: 0.9,
                frame_time: base + i as f32 / 60.0,
                dt: 1.0 / 60.0,
                ..Default::default()
            };
            let metrics = LightingMetrics {
                energy: 0.9,
                stability: 0.9,
                raw_energy: bass + mid + high,
                raw_bass: bass,
                raw_high: high,
                beat_confidence: 0.9,
                kick: i % 25 == 0,
                onset: i % 25 == 0,
                ..Default::default()
            };
            frame = Some(brain.tick(&cue, &metrics, &ClockInputs::default()));
        }
        frame.expect("at least one tick")
    }

    #[test]
    fn dynamics_follows_the_low_end_and_closes_the_detonating_lanes() {
        let mut brain = ShowBrain::new();
        run_bands(&mut brain, 60.0, 0.0100, 0.0030, 0.00040);
        assert!(brain.dynamics() > 0.95, "everything in: {}", brain.dynamics());

        // The same grid and the same relative energy, with the low end and
        // the tops out: pads and a vocal.
        let low = run_bands(&mut brain, 12.0, 0.0008, 0.0022, 0.00010);
        assert!(brain.dynamics() < LANE_DYNAMICS_MIN, "low end out: {}", brain.dynamics());
        assert_eq!(low.arc_state, ArcState::Breakdown, "hot relative energy is still a breakdown");
        for family in [FixtureFamily::Strobe, FixtureFamily::Blinder, FixtureFamily::Laser] {
            assert!(!low.lane(family).eligible, "{family:?} stands down in a low passage");
        }

        // It comes straight back with the bass.
        run_bands(&mut brain, 1.0, 0.0100, 0.0030, 0.00040);
        assert!(brain.dynamics() > 0.9, "a second after the drop: {}", brain.dynamics());
    }

    #[test]
    fn a_feed_with_no_band_split_leaves_dynamics_open() {
        // Older producers and most tests carry a level but no bands.
        let mut brain = ShowBrain::new();
        run_material(&mut brain, 5.0, 25, 0.8);
        let mut bare = ShowBrain::new();
        for i in 1..=300 {
            let cue = CueSnapshot { energy: 0.8, frame_time: i as f32 / 60.0, dt: 1.0 / 60.0, ..Default::default() };
            let metrics = LightingMetrics { energy: 0.8, raw_energy: 0.01, ..Default::default() };
            bare.tick(&cue, &metrics, &ClockInputs::default());
        }
        assert_eq!(bare.dynamics(), 1.0);
    }
}

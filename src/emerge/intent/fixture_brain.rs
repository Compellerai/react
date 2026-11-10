//! FixtureBrain lane eligibility evaluator.
//!
//! This is intentionally pure and side-effect free. It does not send DMX,
//! Art-Net, OLA, OSC, Hue, WLED, or Virtual Stage output. The first safe slice
//! gives REACT a concrete fixture-family intent API while keeping all real
//! output paths untouched.

use super::oscillator::OscillatorFrameSnapshot;
use crate::emerge::contracts::FixtureCapability;
use crate::hue::CueSnapshot;
use crate::emerge::AutoBlinderPhase;
use crate::render::LightingMetrics;

/// Laser "big groove" permission (Koob: lasers on big grooving moments, not only
/// drops). A groove counts as "big" only when it is high-energy, beat-confident,
/// harmonically coherent, AND tonally committed (polarity / major-minor decided,
/// not the ambiguous "—" state). Thresholds are calibrated against REACT's live
/// 0-1 signal range and are the natural future MIDI-bindable depth knobs.
const LASER_GROOVE_ENERGY: f32 = 0.55;
const LASER_GROOVE_BEAT: f32 = 0.50;
/// `|major_likelihood - 0.5|` must reach this for the tonal mood to count as
/// "committed" — filters out the ambiguous, no-clear-polarity moments.
const LASER_MOOD_COMMIT: f32 = 0.12;
/// Seconds of sustained-tension plateau after which the laser lane is held out
/// (fires as release on the drop instead of into the build).
const LASER_TENSION_PLATEAU_DWELL_S: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixtureFamily {
    Wash,
    Mover,
    Gobo,
    Laser,
    Strobe,
    Blinder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CuePriority {
    Bed = 10,
    Motion = 20,
    Pattern = 30,
    CoherentIntense = 70,
    BeatIntense = 80,
    PhraseIntense = 90,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowClockSource {
    AudioEngine,
    ExternalBridge,
    Manual,
    Fallback,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShowClockSnapshot {
    pub tempo_bpm: f32,
    pub beat: f64,
    pub phase: f32,
    pub quantum: f32,
    pub running: bool,
    pub peer_count: u16,
    pub confidence: f32,
    pub source: ShowClockSource,
}

impl Default for ShowClockSnapshot {
    fn default() -> Self {
        Self {
            tempo_bpm: 122.0,
            beat: 0.0,
            phase: 0.5,
            quantum: 4.0,
            running: true,
            peer_count: 0,
            confidence: 1.0,
            source: ShowClockSource::AudioEngine,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArcState {
    Intro,
    Groove,
    Buildup,
    PreDropTension,
    Drop,
    SustainPeak,
    Cooldown,
    Breakdown,
    Silence,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArcSignals {
    pub energy_rise: f32,
    pub tension_rise: f32,
    pub onset_density: f32,
    pub high_rise: f32,
    pub bass_withdrawal: f32,
    pub musical_cooldown_bar_lock: f32,
    /// Seconds harmonic tension has held at/above the plateau level. The laser
    /// gate holds the lane out past a dwell so it fires as release on the drop.
    pub tension_plateau_s: f32,
    /// Intensity license 0..1 (genre-free): percussiveness × spectral
    /// fullness, from the show brain. Scales the aggressive vocabulary
    /// moment-to-moment — an EDM vocal bridge (kick running, hats/bass thin)
    /// softens mover depth here even though the grid persists. 1.0 = full
    /// license (the Default, so un-stamped tests keep classic behavior).
    pub intensity_license: f32,
}

impl Default for ArcSignals {
    fn default() -> Self {
        Self {
            energy_rise: 0.0,
            tension_rise: 0.0,
            onset_density: 0.0,
            high_rise: 0.0,
            bass_withdrawal: 0.0,
            musical_cooldown_bar_lock: 0.0,
            tension_plateau_s: 0.0,
            intensity_license: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhaseState {
    pub beat: f32,
    pub bar: f32,
    pub phrase: f32,
    /// Integer phrase ordinal on the show-clock grid (= floor(beat / quantum)).
    /// The discrete gobo/laser slot picker reads this (via
    /// `OscillatorFrameSnapshot`) so it advances on real phrase boundaries
    /// (COM-499 Phase A D5), not the 122-BPM fallback grid.
    pub phrase_count: u32,
    /// Absolute, MONOTONE show-clock beat count this phase state derives
    /// from (= `ShowClockSnapshot::beat`; every clock source is rebased
    /// monotone upstream). Carried so the oscillators can run fractional
    /// frequency ratios (`with_ratio(<1.0)`) off an UNWRAPPED interval count —
    /// a wrapped [0,1) base compresses ratio 0.5 into [0, 0.5) and snaps to 0
    /// at every parent-interval boundary. 0.0 = unknown (default/legacy
    /// producers); consumers fall back to the wrapped phases.
    pub beats_abs: f64,
}

impl Default for PhaseState {
    fn default() -> Self {
        Self { beat: 0.5, bar: 0.5, phrase: 0.5, phrase_count: 0, beats_abs: 0.0 }
    }
}

impl PhaseState {
    pub fn from_show_clock(clock: ShowClockSnapshot) -> Self {
        let quantum = clock.quantum.max(1.0);
        let beat = clock.phase.rem_euclid(1.0);
        let bar = ((clock.beat / 4.0).fract() as f32).rem_euclid(1.0);
        let phrases = clock.beat / quantum as f64;
        let phrase = (phrases.fract() as f32).rem_euclid(1.0);
        // Phrase ordinal on the real clock grid, so the gobo/laser slot picker
        // advances on true phrase boundaries, not the DEMO_BPM (122) grid.
        let phrase_count = phrases.max(0.0).floor() as u32;
        Self { beat, bar, phrase, phrase_count, beats_abs: clock.beat.max(0.0) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TopologyState {
    pub coherent_field: bool,
    pub topology_lock: bool,
    pub expansion: f32,
    pub heat: f32,
    pub intensity: f32,
}

impl Default for TopologyState {
    fn default() -> Self {
        Self {
            coherent_field: false,
            topology_lock: false,
            expansion: 0.0,
            heat: 0.0,
            intensity: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FixtureGenreProfile {
    pub aggression: f32,
    pub density: f32,
    pub movement: f32,
    pub color_saturation: f32,
    pub strobe_tolerance: f32,
    pub laser_tolerance: f32,
}

impl Default for FixtureGenreProfile {
    fn default() -> Self {
        Self {
            aggression: 0.5,
            density: 0.5,
            movement: 0.5,
            color_saturation: 0.75,
            strobe_tolerance: 0.35,
            laser_tolerance: 0.45,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FixtureFamilyCapabilities {
    pub family: FixtureFamily,
    pub capabilities: Vec<FixtureCapability>,
}

impl FixtureFamilyCapabilities {
    pub fn new(family: FixtureFamily, capabilities: Vec<FixtureCapability>) -> Self {
        Self { family, capabilities }
    }

    fn has(&self, capability: FixtureCapability) -> bool {
        self.capabilities.iter().any(|item| item == &capability)
    }

    fn has_any(&self, capabilities: &[FixtureCapability]) -> bool {
        capabilities.iter().any(|capability| self.has(capability.clone()))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FixtureBrainContext<'a> {
    pub cue: &'a CueSnapshot,
    pub metrics: &'a LightingMetrics,
    pub topology: TopologyState,
    pub show_clock: ShowClockSnapshot,
    pub phase: PhaseState,
    pub arc_state: ArcState,
    pub arc_signals: ArcSignals,
    pub genre: FixtureGenreProfile,
}

/// Immutable per-frame FixtureBrain context.
///
/// Build this once per main frame, then render every fixture family from this
/// same value. Individual fixtures must not decide eligibility, arc timing, or
/// cooldowns on their own threads. Backend threads are allowed only as transport
/// adapters that consume already-decided frame intent.
#[derive(Clone, Copy, Debug)]
pub struct FixtureFrameContext<'a> {
    /// Monotonic id for one FixtureBrain tick. Everything rendered from this
    /// context belongs to the same frame and must carry this same id.
    pub frame_id: u64,
    /// Intended show-clock time for this event. Actual packet fire/send attempt
    /// times are measured separately with monotonic REACT timestamps.
    pub event_time_ms: u64,
    pub brain: FixtureBrainContext<'a>,
    pub oscillator: OscillatorFrameSnapshot,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CueLaneIntent {
    pub family: FixtureFamily,
    pub eligible: bool,
    pub priority: CuePriority,
    pub intensity: f32,
    pub color_anchor: f32,
    pub movement: f32,
    pub momentary: bool,
    /// Non-negotiable protection window for humans, fixtures, and hardware.
    pub safety_cooldown_ms: u32,
    /// Taste/pacing window that prevents repeated musical punctuation.
    pub musical_cooldown_ms: u32,
    /// Non-negotiable duty-cycle ceiling for intense physical fixtures.
    pub safety_max_duty_cycle: f32,
    pub reason: &'static str,
}

impl CueLaneIntent {
    pub(crate) fn inactive(family: FixtureFamily, priority: CuePriority, reason: &'static str) -> Self {
        Self {
            family,
            eligible: false,
            priority,
            intensity: 0.0,
            color_anchor: 0.0,
            movement: 0.0,
            momentary: false,
            safety_cooldown_ms: 0,
            musical_cooldown_ms: 0,
            safety_max_duty_cycle: 1.0,
            reason,
        }
    }
}

#[derive(Default)]
pub struct FixtureBrain;

impl FixtureBrain {
    pub fn tick<'a>(
        &self,
        frame_id: u64,
        cue: &'a CueSnapshot,
        metrics: &'a LightingMetrics,
        topology: TopologyState,
        show_clock: ShowClockSnapshot,
        arc_signals: ArcSignals,
        genre: FixtureGenreProfile,
    ) -> FixtureFrameContext<'a> {
        let phase = PhaseState::from_show_clock(show_clock);
        let arc_state = self.detect_arc_state(cue, metrics, phase, arc_signals);
        self.tick_with_arc(frame_id, cue, metrics, topology, show_clock, arc_state, arc_signals, genre)
    }

    /// Like `tick`, but the caller supplies the arc state instead of letting
    /// `detect_arc_state` classify it fresh. The stateful show-brain uses this
    /// to feed a *debounced* arc, so a momentary `drop_impact` dip cannot chop
    /// a sustained drop into phantom cooldowns.
    pub fn tick_with_arc<'a>(
        &self,
        frame_id: u64,
        cue: &'a CueSnapshot,
        metrics: &'a LightingMetrics,
        topology: TopologyState,
        show_clock: ShowClockSnapshot,
        arc_state: ArcState,
        arc_signals: ArcSignals,
        genre: FixtureGenreProfile,
    ) -> FixtureFrameContext<'a> {
        let phase = PhaseState::from_show_clock(show_clock);
        let event_time_ms = (cue.frame_time.max(0.0) * 1000.0).round() as u64;
        FixtureFrameContext {
            frame_id,
            event_time_ms,
            brain: FixtureBrainContext {
                cue,
                metrics,
                topology,
                show_clock,
                phase,
                arc_state,
                arc_signals,
                genre,
            },
            // COM-499 Phase A: lock the autopilot oscillators to the resolved
            // show clock (`phase`, derived from `show_clock` above) so every
            // breath/sweep rides the track's real beat grid, not a 122-BPM wall
            // clock. The energy_slow silence-gate is preserved in `from_clock`.
            oscillator: OscillatorFrameSnapshot::from_clock(cue, phase),
        }
    }

    pub fn detect_arc_state(
        &self,
        cue: &CueSnapshot,
        metrics: &LightingMetrics,
        phase: PhaseState,
        signals: ArcSignals,
    ) -> ArcState {
        if metrics.raw_energy < 0.0003 || cue.energy < 0.03 {
            return ArcState::Silence;
        }
        if signals.musical_cooldown_bar_lock > 0.0 || cue.macro_phase == AutoBlinderPhase::Recover {
            return ArcState::Cooldown;
        }
        if cue.drop_impact > 0.62 || cue.macro_phase == AutoBlinderPhase::OverdriveStrobe {
            return ArcState::Drop;
        }

        let phrase_ramp = if phase.phrase > 0.62 { phase.phrase } else { 0.0 };
        let buildup_score = signals.energy_rise
            + signals.tension_rise
            + signals.onset_density
            + signals.high_rise
            + signals.bass_withdrawal
            + phrase_ramp;
        if buildup_score >= 3.15 && cue.energy > 0.28 {
            // The crest claim reads ADAPTIVE tension (dynamics above the
            // track's own baseline), never the absolute level: wall-of-sound
            // material rails absolute tension at ~1.0 for whole songs, which
            // crowned every swell a "crest" and cycled the buildup macro
            // constantly (the lit BUILDUP trigger on Mogwai). A real build
            // pushes tension ABOVE its baseline exactly when it matters, so
            // EDM/rock crests still qualify. The phrase-tail fallback keeps a
            // dynamics floor for the same reason — a phrase boundary on a
            // railed wall is not a crest.
            if cue.tension_dyn > 0.74 || (phase.phrase > 0.84 && cue.tension_dyn > 0.35) {
                return ArcState::PreDropTension;
            }
            return ArcState::Buildup;
        }

        if cue.energy > 0.82 && metrics.beat_confidence > 0.65 {
            return ArcState::SustainPeak;
        }
        if cue.energy < 0.16 {
            return ArcState::Breakdown;
        }
        if cue.energy < 0.34 {
            return ArcState::Intro;
        }
        ArcState::Groove
    }

    pub fn evaluate(
        &self,
        ctx: &FixtureBrainContext<'_>,
        fixture: &FixtureFamilyCapabilities,
    ) -> CueLaneIntent {
        match fixture.family {
            FixtureFamily::Wash => self.evaluate_wash(ctx, fixture),
            FixtureFamily::Mover => self.evaluate_mover(ctx, fixture),
            FixtureFamily::Gobo => self.evaluate_gobo(ctx, fixture),
            FixtureFamily::Laser => self.evaluate_laser(ctx, fixture),
            FixtureFamily::Strobe => self.evaluate_strobe(ctx, fixture),
            FixtureFamily::Blinder => self.evaluate_blinder(ctx, fixture),
        }
    }

    pub fn evaluate_all(
        &self,
        ctx: &FixtureBrainContext<'_>,
        fixtures: &[FixtureFamilyCapabilities],
    ) -> Vec<CueLaneIntent> {
        fixtures.iter().map(|fixture| self.evaluate(ctx, fixture)).collect()
    }

    fn evaluate_wash(
        &self,
        ctx: &FixtureBrainContext<'_>,
        fixture: &FixtureFamilyCapabilities,
    ) -> CueLaneIntent {
        if !fixture.has(FixtureCapability::Dimmer) {
            return CueLaneIntent::inactive(FixtureFamily::Wash, CuePriority::Bed, "wash missing dimmer");
        }
        if !fixture.has_any(&[FixtureCapability::Rgb, FixtureCapability::White, FixtureCapability::ColorWheel]) {
            return CueLaneIntent::inactive(FixtureFamily::Wash, CuePriority::Bed, "wash missing color capability");
        }

        CueLaneIntent {
            family: FixtureFamily::Wash,
            eligible: true,
            priority: CuePriority::Bed,
            intensity: wash_intensity(ctx),
            color_anchor: ctx.cue.dominant_pc as f32 / 12.0,
            movement: (ctx.phase.bar * 0.18 + ctx.topology.expansion * 0.22).clamp(0.0, 1.0),
            momentary: matches!(ctx.arc_state, ArcState::Drop),
            safety_cooldown_ms: 0,
            musical_cooldown_ms: 0,
            safety_max_duty_cycle: 1.0,
            reason: wash_reason(ctx.arc_state),
        }
    }

    fn evaluate_mover(
        &self,
        ctx: &FixtureBrainContext<'_>,
        fixture: &FixtureFamilyCapabilities,
    ) -> CueLaneIntent {
        if !fixture.has(FixtureCapability::PanTilt) {
            return CueLaneIntent::inactive(FixtureFamily::Mover, CuePriority::Motion, "mover missing pan/tilt");
        }
        if ctx.metrics.raw_energy < 0.0003 {
            return CueLaneIntent::inactive(FixtureFamily::Mover, CuePriority::Motion, "mover silent gate");
        }

        let (intensity, movement, reason) = mover_arc_values(ctx);
        CueLaneIntent {
            family: FixtureFamily::Mover,
            eligible: true,
            priority: CuePriority::Motion,
            intensity,
            color_anchor: ctx.cue.dominant_pc as f32 / 12.0,
            movement,
            momentary: matches!(ctx.arc_state, ArcState::Drop),
            safety_cooldown_ms: 0,
            musical_cooldown_ms: 0,
            safety_max_duty_cycle: 1.0,
            reason,
        }
    }

    fn evaluate_gobo(
        &self,
        ctx: &FixtureBrainContext<'_>,
        fixture: &FixtureFamilyCapabilities,
    ) -> CueLaneIntent {
        if !fixture.has(FixtureCapability::Gobo) {
            return CueLaneIntent::inactive(FixtureFamily::Gobo, CuePriority::Pattern, "gobo missing capability");
        }
        if matches!(ctx.arc_state, ArcState::Cooldown) {
            return CueLaneIntent::inactive(FixtureFamily::Gobo, CuePriority::Pattern, "gobo musical cooldown decay gate");
        }
        let phrase_hit = near_phase_zero(ctx.phase.phrase, 0.08) || near_phase_zero(ctx.phase.bar, 0.06);
        let harmonic_turn = ctx.metrics.dominant_changed || ctx.metrics.polarity_changed || ctx.cue.dominant_flash_active;
        let buildup_prepare = matches!(ctx.arc_state, ArcState::Buildup | ArcState::PreDropTension)
            && ctx.phase.phrase > 0.70;
        if !(phrase_hit || harmonic_turn || buildup_prepare) {
            return CueLaneIntent::inactive(FixtureFamily::Gobo, CuePriority::Pattern, "gobo waiting for phrase or harmonic turn");
        }

        CueLaneIntent {
            family: FixtureFamily::Gobo,
            eligible: true,
            priority: CuePriority::Pattern,
            intensity: (0.28 + ctx.metrics.energy * 0.40 + ctx.metrics.tension * 0.22).clamp(0.0, 0.92),
            color_anchor: ctx.cue.dominant_pc as f32 / 12.0,
            movement: (0.18 + ctx.topology.heat * 0.24 + ctx.phase.bar * 0.18).clamp(0.0, 1.0),
            momentary: phrase_hit,
            safety_cooldown_ms: 0,
            musical_cooldown_ms: 1500,
            safety_max_duty_cycle: 0.45,
            reason: if buildup_prepare { "gobo buildup prepare eligible" } else { "gobo phrase/harmonic lane eligible" },
        }
    }

    fn evaluate_laser(
        &self,
        ctx: &FixtureBrainContext<'_>,
        fixture: &FixtureFamilyCapabilities,
    ) -> CueLaneIntent {
        if !fixture.has(FixtureCapability::LaserMacro) {
            return CueLaneIntent::inactive(FixtureFamily::Laser, CuePriority::CoherentIntense, "laser missing macro capability");
        }
        // Sustained-tension plateau: hold the laser OUT while tension sits maxed
        // through a long build, so it reads as RELEASE when the music drops
        // rather than firing into the tension (Koob). A real drop/peak is exempt
        // — that IS the release — so the lane frees the instant tension lets go.
        if ctx.arc_signals.tension_plateau_s >= LASER_TENSION_PLATEAU_DWELL_S
            && !matches!(ctx.arc_state, ArcState::Drop | ArcState::SustainPeak)
        {
            return CueLaneIntent::inactive(FixtureFamily::Laser, CuePriority::CoherentIntense, "laser held through sustained-tension plateau");
        }
        if matches!(ctx.arc_state, ArcState::Cooldown) {
            return CueLaneIntent::inactive(FixtureFamily::Laser, CuePriority::CoherentIntense, "laser musical cooldown decay gate");
        }
        if ctx.metrics.stability < 0.40 || ctx.cue.stability < 0.35 {
            return CueLaneIntent::inactive(FixtureFamily::Laser, CuePriority::CoherentIntense, "laser stability gate");
        }
        if ctx.metrics.energy < 0.30 || ctx.metrics.beat_confidence < 0.40 {
            return CueLaneIntent::inactive(FixtureFamily::Laser, CuePriority::CoherentIntense, "laser energy/beat gate");
        }
        if ctx.genre.laser_tolerance < 0.25 {
            return CueLaneIntent::inactive(FixtureFamily::Laser, CuePriority::CoherentIntense, "laser genre tolerance gate");
        }

        // Permission: the laser fires on a real DROP/peak (the release, its own
        // permission even before the field latches coherent) OR on a big,
        // locked-in GROOVE — a high-energy, beat-confident, harmonically coherent
        // pocket with a COMMITTED tonal mood (not the ambiguous "—"). This is the
        // widening from drop-only: lasers on big grooving moments, in concert
        // with polarity/tonality, not steady wallpaper.
        let drop_release = matches!(ctx.arc_state, ArcState::Drop | ArcState::SustainPeak);
        // `major_likelihood` is 0..1 with 0.5 the live NEUTRAL ("—", no committed
        // mood — KeyDetector seeds + falls back to 0.5). A committed mood is a real
        // distance from neutral in either direction. NB: a default-constructed
        // LightingMetrics has major_likelihood=0.0 (strong-minor) which reads as
        // committed — only reachable off the live path, but don't feed defaults here.
        let mood_committed = (ctx.metrics.major_likelihood - 0.5).abs() >= LASER_MOOD_COMMIT;
        // Deliberately keyed on coherent_field (not topology_lock): the laser lane
        // no longer consults topology_lock since the drop-only gate was widened.
        let big_groove = matches!(ctx.arc_state, ArcState::Groove)
            && ctx.topology.coherent_field
            && ctx.metrics.energy >= LASER_GROOVE_ENERGY
            && ctx.metrics.beat_confidence >= LASER_GROOVE_BEAT
            && mood_committed;
        if !(drop_release || big_groove) {
            return CueLaneIntent::inactive(FixtureFamily::Laser, CuePriority::CoherentIntense, "laser waiting for drop release or big groove");
        }

        let base = 0.18 + ctx.metrics.energy * 0.34 + ctx.topology.intensity * 0.34 + ctx.genre.laser_tolerance * 0.14;
        CueLaneIntent {
            family: FixtureFamily::Laser,
            eligible: true,
            priority: CuePriority::CoherentIntense,
            intensity: base.clamp(0.0, 0.84),
            color_anchor: ctx.cue.dominant_pc as f32 / 12.0,
            movement: (0.15 + ctx.topology.expansion * 0.55 + ctx.phase.bar * 0.12).clamp(0.0, 0.86),
            // Sustained — the laser holds and runs its routine for the whole
            // open window, rather than firing a one-shot blip.
            momentary: false,
            safety_cooldown_ms: 2000,
            musical_cooldown_ms: 6000,
            safety_max_duty_cycle: 0.30,
            reason: if drop_release { "laser drop release routine eligible" } else { "laser big-groove routine eligible" },
        }
    }

    fn evaluate_strobe(
        &self,
        ctx: &FixtureBrainContext<'_>,
        fixture: &FixtureFamilyCapabilities,
    ) -> CueLaneIntent {
        if !fixture.has(FixtureCapability::Strobe) {
            return CueLaneIntent::inactive(FixtureFamily::Strobe, CuePriority::BeatIntense, "strobe missing capability");
        }
        if matches!(ctx.arc_state, ArcState::Cooldown) {
            return CueLaneIntent::inactive(FixtureFamily::Strobe, CuePriority::BeatIntense, "strobe musical cooldown lock");
        }
        if matches!(ctx.arc_state, ArcState::Buildup | ArcState::PreDropTension) {
            return CueLaneIntent::inactive(FixtureFamily::Strobe, CuePriority::BeatIntense, "strobe suppressed during buildup");
        }
        if !matches!(ctx.arc_state, ArcState::Drop | ArcState::SustainPeak) {
            return CueLaneIntent::inactive(FixtureFamily::Strobe, CuePriority::BeatIntense, "strobe waiting for drop arc");
        }
        if ctx.metrics.energy < 0.45 || ctx.cue.drop_impact < 0.35 {
            return CueLaneIntent::inactive(FixtureFamily::Strobe, CuePriority::BeatIntense, "strobe high-energy/drop gate");
        }
        if ctx.metrics.stability < 0.30 || ctx.metrics.beat_confidence < 0.50 {
            return CueLaneIntent::inactive(FixtureFamily::Strobe, CuePriority::BeatIntense, "strobe stability/beat gate");
        }
        if !(ctx.metrics.kick || ctx.metrics.onset || ctx.cue.overdrive_strobe_active) {
            return CueLaneIntent::inactive(FixtureFamily::Strobe, CuePriority::BeatIntense, "strobe waiting for kick/onset/drop punctuation");
        }
        if !near_phase_zero(ctx.phase.beat, 0.08) {
            return CueLaneIntent::inactive(FixtureFamily::Strobe, CuePriority::BeatIntense, "strobe beat phase gate");
        }
        if ctx.genre.strobe_tolerance < 0.30 {
            return CueLaneIntent::inactive(FixtureFamily::Strobe, CuePriority::BeatIntense, "strobe genre tolerance gate");
        }

        CueLaneIntent {
            family: FixtureFamily::Strobe,
            eligible: true,
            priority: CuePriority::BeatIntense,
            intensity: (0.48 + ctx.metrics.energy * 0.28 + ctx.cue.drop_impact * 0.20).clamp(0.0, 0.96),
            color_anchor: ctx.cue.dominant_pc as f32 / 12.0,
            movement: 0.0,
            momentary: true,
            safety_cooldown_ms: 2500,
            musical_cooldown_ms: 8000,
            safety_max_duty_cycle: 0.08,
            reason: "strobe stable high-energy beat punctuation eligible",
        }
    }

    fn evaluate_blinder(
        &self,
        ctx: &FixtureBrainContext<'_>,
        fixture: &FixtureFamilyCapabilities,
    ) -> CueLaneIntent {
        if !fixture.has_any(&[FixtureCapability::Dimmer, FixtureCapability::White]) {
            return CueLaneIntent::inactive(FixtureFamily::Blinder, CuePriority::PhraseIntense, "blinder missing dimmer/white capability");
        }
        if matches!(ctx.arc_state, ArcState::Cooldown) {
            return CueLaneIntent::inactive(FixtureFamily::Blinder, CuePriority::PhraseIntense, "blinder musical cooldown lock");
        }
        if matches!(ctx.arc_state, ArcState::Buildup | ArcState::PreDropTension) {
            return CueLaneIntent::inactive(FixtureFamily::Blinder, CuePriority::PhraseIntense, "blinder suppressed during buildup");
        }
        if ctx.arc_state != ArcState::Drop {
            return CueLaneIntent::inactive(FixtureFamily::Blinder, CuePriority::PhraseIntense, "blinder waiting for drop arc");
        }
        let phrase_hit = near_phase_zero(ctx.phase.phrase, 0.035);
        let drop_hit = ctx.cue.trigger_blinder
            || ctx.cue.auto_blinder_active
            || ctx.cue.macro_phase == AutoBlinderPhase::BlindersHold;
        if !(phrase_hit && drop_hit) {
            return CueLaneIntent::inactive(FixtureFamily::Blinder, CuePriority::PhraseIntense, "blinder waiting for phrase/drop hit");
        }
        if ctx.metrics.energy < 0.88 || ctx.cue.drop_impact < 0.50 || ctx.metrics.stability < 0.68 {
            return CueLaneIntent::inactive(FixtureFamily::Blinder, CuePriority::PhraseIntense, "blinder intensity/stability gate");
        }
        if ctx.genre.density < 0.35 {
            return CueLaneIntent::inactive(FixtureFamily::Blinder, CuePriority::PhraseIntense, "blinder genre density gate");
        }

        CueLaneIntent {
            family: FixtureFamily::Blinder,
            eligible: true,
            priority: CuePriority::PhraseIntense,
            intensity: (0.72 + ctx.metrics.energy * 0.18 + ctx.cue.drop_impact * 0.10).clamp(0.0, 1.0),
            color_anchor: ctx.cue.dominant_pc as f32 / 12.0,
            movement: 0.0,
            momentary: true,
            safety_cooldown_ms: 10000,
            musical_cooldown_ms: 45000,
            safety_max_duty_cycle: 0.03,
            reason: "blinder rare phrase/drop punctuation eligible",
        }
    }
}

fn wash_intensity(ctx: &FixtureBrainContext<'_>) -> f32 {
    let base = 0.16 + ctx.cue.energy_slow * 0.42 + ctx.metrics.energy * 0.30;
    match ctx.arc_state {
        ArcState::Buildup | ArcState::PreDropTension => (base + ctx.arc_signals.energy_rise * 0.16).clamp(0.0, 0.94),
        ArcState::Drop => (base * 0.72 + ctx.cue.drop_impact * 0.42).clamp(0.0, 1.0),
        ArcState::Cooldown => (base * 0.58).clamp(0.12, 0.58),
        ArcState::Silence => 0.0,
        _ => base.clamp(0.0, 0.86),
    }
}

fn wash_reason(arc_state: ArcState) -> &'static str {
    match arc_state {
        ArcState::Buildup | ArcState::PreDropTension => "wash buildup rise eligible",
        ArcState::Drop => "wash drop duck/bloom eligible",
        ArcState::Cooldown => "wash musical cooldown bed return eligible",
        ArcState::Silence => "wash silence floor",
        _ => "wash bed eligible",
    }
}

fn mover_arc_values(ctx: &FixtureBrainContext<'_>) -> (f32, f32, &'static str) {
    // The genre movement contribution is licensed by the music's moment:
    // a vocal bridge (license low) leans the movers back even on a Hardstyle
    // profile; the drop re-arms them. Energy/phrase terms stay unlicensed —
    // they're the baseline breathing, not the aggressive vocabulary.
    let licensed_movement = ctx.genre.movement * ctx.arc_signals.intensity_license.clamp(0.0, 1.0);
    let base_intensity = 0.25 + ctx.metrics.energy * 0.45 + licensed_movement * 0.20;
    let base_movement = 0.20 + ctx.phase.bar * 0.22 + ctx.phase.phrase * 0.18 + licensed_movement * 0.30;
    match ctx.arc_state {
        ArcState::Buildup | ArcState::PreDropTension => (
            (base_intensity + ctx.arc_signals.tension_rise * 0.14).clamp(0.0, 0.96),
            (base_movement + ctx.arc_signals.onset_density * 0.18).clamp(0.0, 1.0),
            "mover buildup tighten/accelerate eligible",
        ),
        ArcState::Drop => (
            (base_intensity + 0.16).clamp(0.0, 1.0),
            (base_movement + 0.24).clamp(0.0, 1.0),
            "mover drop snap/chase eligible",
        ),
        ArcState::Cooldown => (
            (base_intensity * 0.50).clamp(0.0, 0.55),
            (base_movement * 0.42).clamp(0.0, 0.50),
            "mover musical cooldown decay eligible",
        ),
        _ => (
            base_intensity.clamp(0.0, 0.90),
            base_movement.clamp(0.0, 1.0),
            "mover oscillator lane eligible",
        ),
    }
}

fn near_phase_zero(phase: f32, width: f32) -> bool {
    let phase = phase.rem_euclid(1.0);
    phase <= width || phase >= 1.0 - width
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> LightingMetrics {
        LightingMetrics {
            energy: 0.9,
            stability: 0.86,
            beat_confidence: 0.9,
            kick: true,
            onset: true,
            raw_energy: 0.01,
            dominant_pc: 5,
            ..Default::default()
        }
    }

    fn cue() -> CueSnapshot {
        CueSnapshot {
            energy: 0.9,
            energy_slow: 0.72,
            stability: 0.84,
            dominant_pc: 5,
            drop_impact: 0.72,
            reactivity: 1.0,
            ..Default::default()
        }
    }

    fn ctx<'a>(cue: &'a CueSnapshot, metrics: &'a LightingMetrics) -> FixtureBrainContext<'a> {
        FixtureBrainContext {
            cue,
            metrics,
            topology: TopologyState {
                coherent_field: true,
                topology_lock: true,
                expansion: 0.7,
                heat: 0.5,
                intensity: 0.8,
            },
            show_clock: ShowClockSnapshot { beat: 1.0, phase: 0.01, quantum: 4.0, ..Default::default() },
            phase: PhaseState { beat: 0.01, bar: 0.25, phrase: 0.01, phrase_count: 0, beats_abs: 0.0 },
            arc_state: ArcState::Drop,
            arc_signals: ArcSignals::default(),
            genre: FixtureGenreProfile {
                strobe_tolerance: 0.7,
                laser_tolerance: 0.8,
                density: 0.75,
                ..Default::default()
            },
        }
    }

    #[test]
    fn phase_state_derives_from_internal_show_clock() {
        let clock = ShowClockSnapshot { beat: 9.0, phase: 0.25, quantum: 8.0, ..Default::default() };
        let phase = PhaseState::from_show_clock(clock);

        assert_eq!(phase.beat, 0.25);
        assert_eq!(phase.bar, 0.25);
        assert_eq!(phase.phrase, 0.125);
        // The MONOTONE absolute beat rides along so the oscillators can run
        // fractional frequency ratios off an unwrapped interval count.
        assert_eq!(phase.beats_abs, 9.0);
    }

    #[test]
    fn show_clock_defaults_to_audio_engine_source() {
        let clock = ShowClockSnapshot::default();

        assert_eq!(clock.source, ShowClockSource::AudioEngine);
        assert!(clock.confidence >= 1.0);
    }

    #[test]
    fn tick_builds_one_frame_context_for_all_fixtures() {
        let cue = cue();
        let metrics = metrics();
        let topology = TopologyState {
            coherent_field: true,
            topology_lock: true,
            expansion: 0.7,
            heat: 0.5,
            intensity: 0.8,
        };
        let show_clock = ShowClockSnapshot { beat: 1.0, phase: 0.01, quantum: 4.0, ..Default::default() };
        let genre = FixtureGenreProfile {
            strobe_tolerance: 0.7,
            laser_tolerance: 0.8,
            density: 0.75,
            ..Default::default()
        };
        let frame = FixtureBrain.tick(42, &cue, &metrics, topology, show_clock, ArcSignals::default(), genre);
        let fixtures = vec![
            FixtureFamilyCapabilities::new(
                FixtureFamily::Wash,
                vec![FixtureCapability::Dimmer, FixtureCapability::Rgb],
            ),
            FixtureFamilyCapabilities::new(
                FixtureFamily::Strobe,
                vec![FixtureCapability::Strobe],
            ),
        ];

        let intents = FixtureBrain.evaluate_all(&frame.brain, &fixtures);

        assert_eq!(intents.len(), 2);
        assert_eq!(frame.frame_id, 42);
        let expected_event_time_ms = (frame.brain.cue.frame_time.max(0.0) * 1000.0).round() as u64;
        assert_eq!(frame.event_time_ms, expected_event_time_ms);
        assert_eq!(frame.brain.arc_state, ArcState::Drop);
    }

    #[test]
    fn detects_buildup_from_rising_shape() {
        let cue = CueSnapshot {
            energy: 0.62,
            tension: 0.70,
            ..Default::default()
        };
        let metrics = LightingMetrics {
            energy: 0.62,
            raw_energy: 0.01,
            beat_confidence: 0.82,
            ..Default::default()
        };
        let arc = FixtureBrain.detect_arc_state(
            &cue,
            &metrics,
            PhaseState { beat: 0.5, bar: 0.7, phrase: 0.78, phrase_count: 0, beats_abs: 0.0 },
            ArcSignals {
                energy_rise: 0.75,
                tension_rise: 0.68,
                onset_density: 0.62,
                high_rise: 0.58,
                bass_withdrawal: 0.52,
                musical_cooldown_bar_lock: 0.0,
                tension_plateau_s: 0.0,
                intensity_license: 1.0,
            },
        );

        assert_eq!(arc, ArcState::Buildup);
    }

    #[test]
    fn railed_wall_tension_never_claims_the_crest() {
        // Wall-of-sound material rails ABSOLUTE tension at ~1.0 while its
        // ADAPTIVE tension decays to ~0 — a swell on a wall must classify as
        // (at most) Buildup, never PreDropTension, even at a phrase tail.
        // A genuine crest (tension above its own baseline) still qualifies.
        let buildup_signals = ArcSignals {
            energy_rise: 0.75,
            tension_rise: 0.68,
            onset_density: 0.62,
            high_rise: 0.58,
            bass_withdrawal: 0.52,
            musical_cooldown_bar_lock: 0.0,
            tension_plateau_s: 0.0,
            intensity_license: 1.0,
        };
        let phase_tail =
            PhaseState { beat: 0.5, bar: 0.7, phrase: 0.9, phrase_count: 0, beats_abs: 0.0 };
        let metrics = LightingMetrics { raw_energy: 0.01, ..Default::default() };

        let wall = CueSnapshot {
            energy: 0.7,
            tension: 1.0,     // railed absolute
            tension_dyn: 0.05, // no dynamics above baseline
            ..Default::default()
        };
        assert_eq!(
            FixtureBrain.detect_arc_state(&wall, &metrics, phase_tail, buildup_signals),
            ArcState::Buildup,
            "railed wall at a phrase tail is a bloom, not a crest"
        );

        let crest = CueSnapshot {
            energy: 0.7,
            tension: 0.9,
            tension_dyn: 0.8, // genuinely above its own baseline
            ..Default::default()
        };
        assert_eq!(
            FixtureBrain.detect_arc_state(&crest, &metrics, phase_tail, buildup_signals),
            ArcState::PreDropTension,
            "a real build still claims the crest"
        );
    }

    #[test]
    fn musical_cooldown_locks_intense_lanes() {
        let cue = cue();
        let metrics = metrics();
        let mut ctx = ctx(&cue, &metrics);
        ctx.arc_state = ArcState::Cooldown;

        let strobe = FixtureFamilyCapabilities::new(
            FixtureFamily::Strobe,
            vec![FixtureCapability::Strobe],
        );
        let blinder = FixtureFamilyCapabilities::new(
            FixtureFamily::Blinder,
            vec![FixtureCapability::Dimmer, FixtureCapability::White],
        );

        assert_eq!(FixtureBrain.evaluate(&ctx, &strobe).reason, "strobe musical cooldown lock");
        assert_eq!(FixtureBrain.evaluate(&ctx, &blinder).reason, "blinder musical cooldown lock");
    }

    #[test]
    fn buildup_suppresses_both_intense_lanes() {
        let cue = cue();
        let metrics = metrics();
        let mut ctx = ctx(&cue, &metrics);
        ctx.arc_state = ArcState::Buildup;

        let strobe = FixtureFamilyCapabilities::new(
            FixtureFamily::Strobe,
            vec![FixtureCapability::Strobe],
        );
        let laser = FixtureFamilyCapabilities::new(
            FixtureFamily::Laser,
            vec![FixtureCapability::LaserMacro],
        );

        let strobe_intent = FixtureBrain.evaluate(&ctx, &strobe);
        let laser_intent = FixtureBrain.evaluate(&ctx, &laser);

        assert!(!strobe_intent.eligible);
        assert_eq!(strobe_intent.reason, "strobe suppressed during buildup");
        // The laser does not tease during the buildup — it fires on the drop
        // release or a big groove, and a buildup is neither.
        assert!(!laser_intent.eligible);
        assert_eq!(laser_intent.reason, "laser waiting for drop release or big groove");
    }

    #[test]
    fn laser_requires_coherent_topology_lock() {
        let cue = cue();
        let metrics = metrics();
        let mut ctx = ctx(&cue, &metrics);
        ctx.topology.coherent_field = false;
        ctx.topology.topology_lock = false;
        // Not a drop — so the drop-permission path cannot satisfy the gate.
        ctx.arc_state = ArcState::Groove;

        let fixture = FixtureFamilyCapabilities::new(
            FixtureFamily::Laser,
            vec![FixtureCapability::LaserMacro],
        );
        let intent = FixtureBrain.evaluate(&ctx, &fixture);

        assert!(!intent.eligible);
        // Coherence moved into the permission gate; a groove with no coherent
        // field falls through to the unified drop-or-groove permission denial.
        assert_eq!(intent.reason, "laser waiting for drop release or big groove");
    }

    #[test]
    fn laser_allows_coherent_locked_field() {
        let cue = cue();
        let metrics = metrics();
        let ctx = ctx(&cue, &metrics);
        let fixture = FixtureFamilyCapabilities::new(
            FixtureFamily::Laser,
            vec![FixtureCapability::LaserMacro],
        );
        let intent = FixtureBrain.evaluate(&ctx, &fixture);

        assert!(intent.eligible);
        assert_eq!(intent.priority, CuePriority::CoherentIntense);
        assert!(intent.musical_cooldown_ms >= 6000);
        assert!(intent.safety_max_duty_cycle <= 0.30);
    }

    #[test]
    fn laser_fires_on_big_groove() {
        // Koob: lasers on big grooving moments, not only drops. A high-energy,
        // beat-confident, coherent groove with a committed tonal mood
        // (metrics() defaults major_likelihood=0.0 => strongly minor => committed)
        // now earns the laser even though the arc is Groove, not Drop.
        let cue = cue();
        let metrics = metrics();
        let mut ctx = ctx(&cue, &metrics); // coherent_field = true
        ctx.arc_state = ArcState::Groove;

        let fixture = FixtureFamilyCapabilities::new(
            FixtureFamily::Laser,
            vec![FixtureCapability::LaserMacro],
        );
        let intent = FixtureBrain.evaluate(&ctx, &fixture);

        assert!(intent.eligible, "big groove should fire lasers: {}", intent.reason);
        assert_eq!(intent.reason, "laser big-groove routine eligible");
    }

    #[test]
    fn laser_skips_ambiguous_groove() {
        // The "—" moment: no committed polarity/mood (major_likelihood == 0.5).
        // A groove with an undecided harmonic mood does NOT earn the laser.
        let cue = cue();
        let mut metrics = metrics();
        metrics.major_likelihood = 0.5;
        let mut ctx = ctx(&cue, &metrics);
        ctx.arc_state = ArcState::Groove;

        let fixture = FixtureFamilyCapabilities::new(
            FixtureFamily::Laser,
            vec![FixtureCapability::LaserMacro],
        );
        let intent = FixtureBrain.evaluate(&ctx, &fixture);

        assert!(!intent.eligible);
        assert_eq!(intent.reason, "laser waiting for drop release or big groove");
    }

    #[test]
    fn laser_held_through_sustained_tension() {
        // A sustained-tension plateau holds the laser out (so it reads as
        // release on the drop) even on a would-be big groove.
        let cue = cue();
        let metrics = metrics();
        let mut ctx = ctx(&cue, &metrics);
        ctx.arc_state = ArcState::Groove;
        ctx.arc_signals = ArcSignals { tension_plateau_s: 3.0, ..ArcSignals::default() };

        let fixture = FixtureFamilyCapabilities::new(
            FixtureFamily::Laser,
            vec![FixtureCapability::LaserMacro],
        );
        let intent = FixtureBrain.evaluate(&ctx, &fixture);

        assert!(!intent.eligible);
        assert_eq!(intent.reason, "laser held through sustained-tension plateau");
    }

    #[test]
    fn laser_releases_on_drop_despite_tension_plateau() {
        // The drop IS the release — a long tension plateau must not keep the
        // laser suppressed once the music actually drops.
        let cue = cue();
        let metrics = metrics();
        let mut ctx = ctx(&cue, &metrics); // arc_state defaults to Drop
        ctx.arc_signals = ArcSignals { tension_plateau_s: 5.0, ..ArcSignals::default() };

        let fixture = FixtureFamilyCapabilities::new(
            FixtureFamily::Laser,
            vec![FixtureCapability::LaserMacro],
        );
        let intent = FixtureBrain.evaluate(&ctx, &fixture);

        assert!(intent.eligible, "drop must release the laser after a plateau: {}", intent.reason);
        assert_eq!(intent.reason, "laser drop release routine eligible");
    }

    #[test]
    fn strobe_requires_stable_on_beat_punctuation() {
        let cue = cue();
        let mut metrics = metrics();
        metrics.stability = 0.20;
        let ctx = ctx(&cue, &metrics);
        let fixture = FixtureFamilyCapabilities::new(
            FixtureFamily::Strobe,
            vec![FixtureCapability::Strobe],
        );
        let intent = FixtureBrain.evaluate(&ctx, &fixture);

        assert!(!intent.eligible);
        assert_eq!(intent.reason, "strobe stability/beat gate");
    }

    #[test]
    fn strobe_blocks_off_beat_hits() {
        let cue = cue();
        let metrics = metrics();
        let mut ctx = ctx(&cue, &metrics);
        ctx.phase.beat = 0.42;
        let fixture = FixtureFamilyCapabilities::new(
            FixtureFamily::Strobe,
            vec![FixtureCapability::Strobe],
        );
        let intent = FixtureBrain.evaluate(&ctx, &fixture);

        assert!(!intent.eligible);
        assert_eq!(intent.reason, "strobe beat phase gate");
    }

    #[test]
    fn blinder_is_rarer_than_strobe_and_requires_phrase_drop() {
        let mut cue = cue();
        cue.macro_phase = AutoBlinderPhase::Normal;
        cue.auto_blinder_active = false;
        cue.trigger_blinder = false;
        let metrics = metrics();
        let ctx = ctx(&cue, &metrics);
        let fixture = FixtureFamilyCapabilities::new(
            FixtureFamily::Blinder,
            vec![FixtureCapability::Dimmer, FixtureCapability::White],
        );
        let intent = FixtureBrain.evaluate(&ctx, &fixture);

        assert!(!intent.eligible);
        assert_eq!(intent.reason, "blinder waiting for phrase/drop hit");
    }

    #[test]
    fn blinder_phrase_drop_has_strict_limits() {
        let mut cue = cue();
        cue.auto_blinder_active = true;
        cue.macro_phase = AutoBlinderPhase::BlindersHold;
        let metrics = metrics();
        let ctx = ctx(&cue, &metrics);
        let fixture = FixtureFamilyCapabilities::new(
            FixtureFamily::Blinder,
            vec![FixtureCapability::Dimmer, FixtureCapability::White],
        );
        let intent = FixtureBrain.evaluate(&ctx, &fixture);

        assert!(intent.eligible);
        assert_eq!(intent.priority, CuePriority::PhraseIntense);
        assert!(intent.musical_cooldown_ms >= 45000);
        assert!(intent.safety_max_duty_cycle <= 0.03);
    }
}

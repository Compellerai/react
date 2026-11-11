//! Tiny deterministic oscillator core for COM-409A.
//!
//! This slice is deliberately generic and visible-performance-first: one frame
//! snapshot supplies beat/bar/phrase phases, then hardcoded demo-rig bindings
//! sample saw/sine/triangle/square carriers from that same snapshot.

use super::fixture_brain::PhaseState;
use crate::hue::CueSnapshot;

const DEMO_BPM: f32 = 122.0;
const BEATS_PER_BAR: f32 = 4.0;
const BARS_PER_PHRASE: f32 = 8.0;

/// Oscillator depth-gate edges, in slow-energy (`CueSnapshot::energy_slow`)
/// units. Below `OSC_GATE_SILENCE` the LFOs are fully still — this kills the
/// wall-clock wobble that read as ~2s audio lag. At/above `OSC_GATE_FULL` they
/// run at full designed depth, so a playing show looks exactly as before.
const OSC_GATE_SILENCE: f32 = 0.06;
const OSC_GATE_FULL: f32 = 0.40;

/// Flow-blend edges, in slow-energy (`CueSnapshot::energy_slow`) units.
/// The depth gate above decides HOW DEEP the LFOs swing; this decides HOW
/// FAST. Rates were beat/bar/phrase-locked constants at any energy — a quiet
/// passage kept the full beat-clock wobble, just shallower, which read as
/// rigid/too-quick on mellow and groove material. Below `OSC_FLOW_LO` every
/// non-Square oscillator runs fully half-time; at/above `OSC_FLOW_HI` it runs
/// at its designed rate, byte-identical to before. In between, `sample()`
/// crossfades the two — both waves ride the same unwrapped clock so each is
/// phase-continuous, and the blend weight is the already-smoothed energy_slow,
/// so rate changes glide (the mover-SCAN crossfade pattern: never retune a
/// ratio in place, sample twins and blend).
const OSC_FLOW_LO: f32 = 0.12;
const OSC_FLOW_HI: f32 = 0.55;
/// The slow twin's ratio multiplier: 0.5 = one octave down (half-time).
const OSC_FLOW_HALF: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OscillatorShape {
    Sine,
    Triangle,
    Saw,
    Square,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OscillatorInterval {
    Beat,
    Bar,
    Phrase,
}

#[derive(Clone, Copy, Debug)]
pub struct OscillatorBinding {
    pub shape: OscillatorShape,
    pub interval: OscillatorInterval,
    pub ratio: f32,
    pub phase: f32,
    pub pulse_width: f32,
    pub amount: f32,
}

impl OscillatorBinding {
    pub const fn new(shape: OscillatorShape, interval: OscillatorInterval, amount: f32) -> Self {
        Self {
            shape,
            interval,
            ratio: 1.0,
            phase: 0.0,
            pulse_width: 0.5,
            amount,
        }
    }

    /// Frequency ratio against the binding's interval: 2.0 = twice per
    /// interval, 0.5 = one cycle every TWO intervals. Fractional ratios are
    /// genuinely slower — `sample()` runs them off the unwrapped absolute
    /// interval count, so a 0.5 triangle is a continuous 2-beat breath, not a
    /// per-beat sawtooth snapping at every parent boundary.
    pub const fn with_ratio(mut self, ratio: f32) -> Self {
        self.ratio = ratio;
        self
    }

    pub const fn with_phase(mut self, phase: f32) -> Self {
        self.phase = phase;
        self
    }

    pub const fn with_pulse_width(mut self, pulse_width: f32) -> Self {
        self.pulse_width = pulse_width;
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub struct OscillatorFrameSnapshot {
    beat_phase: f32,
    bar_phase: f32,
    phrase_phase: f32,
    /// UNWRAPPED absolute interval counts (beats / bars / phrases since the
    /// clock origin), mirroring exactly how the wrapped phases above were
    /// derived on each constructor path. `sample()` needs these for
    /// fractional frequency ratios: a `with_ratio(0.5)` binding must see
    /// `fract(unwrapped * 0.5)` — running the ratio against an
    /// already-wrapped [0,1) phase compresses the cycle into [0, ratio) and
    /// snaps to 0 at every parent-interval boundary. For integer ratios the
    /// two formulations agree (`fract((k + p) * n) == fract(n * p)` for
    /// integer k, n), so the unwrapped path is a strict generalisation.
    /// 0.0 = absolute beat source unknown (old producers, default
    /// snapshots) — `sample()` falls back to the wrapped phases.
    beats_abs: f64,
    bars_abs: f64,
    phrases_abs: f64,
    /// Integer phrase ordinal for the discrete gobo/laser slot picker. On the
    /// clock path (`from_clock`) this is the real phrase count (carried on
    /// `PhaseState`); on the `from_cue` fallback it is the 122-BPM
    /// `phrase_index`. Read via [`OscillatorFrameSnapshot::phrase_count`].
    phrase_count: u32,
    /// Slow-energy depth gate (0-1). Every `sample()` is scaled by this so the
    /// oscillators ride an RMS-style envelope instead of a free-running clock.
    energy_env: f32,
    /// Flow blend (0-1): 0 = designed rate (high energy), 1 = fully half-time
    /// (quiet-but-playing). See `OSC_FLOW_LO`/`OSC_FLOW_HI`.
    flow: f32,
}

/// Smoothstepped 0-1 ramp of `energy_slow` between two edges.
fn smooth_ramp(energy_slow: f32, lo: f32, hi: f32) -> f32 {
    let t = ((energy_slow - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl OscillatorFrameSnapshot {
    /// Build once per frame, then sample every binding from this immutable value
    /// so the demo rig stays phase-coherent.
    pub fn from_cue(snapshot: &CueSnapshot) -> Self {
        let beats = snapshot.frame_time.max(0.0) * DEMO_BPM / 60.0;
        Self {
            beat_phase: beats.fract(),
            bar_phase: (beats / BEATS_PER_BAR).fract(),
            phrase_phase: (beats / (BEATS_PER_BAR * BARS_PER_PHRASE)).fract(),
            // Unwrapped counts from the SAME 122-BPM grid as the wrapped
            // phases above — integer-ratio bindings stay byte-identical, and
            // fractional ratios get a genuine multi-interval period even on
            // this pre-clock fallback path. (Not `clock_beat`: mixing the
            // OSC-stamped resolved clock with frame_time-derived wrapped
            // phases would desync the integer and fractional parts — the
            // exact boundary-snap class of bug this field exists to fix.)
            beats_abs: beats as f64,
            bars_abs: (beats / BEATS_PER_BAR) as f64,
            phrases_abs: (beats / (BEATS_PER_BAR * BARS_PER_PHRASE)) as f64,
            phrase_count: phrase_index(snapshot),
            // Gate LFO depth on slow (RMS-style) energy: still in silence,
            // full while music plays. The instantaneous audio response lives
            // in the intent-layer base terms — deliberately not in the
            // oscillators.
            energy_env: smooth_ramp(snapshot.energy_slow, OSC_GATE_SILENCE, OSC_GATE_FULL),
            flow: 1.0 - smooth_ramp(snapshot.energy_slow, OSC_FLOW_LO, OSC_FLOW_HI),
        }
    }

    /// COM-499 Phase A — like [`Self::from_cue`], but the beat/bar/phrase phases
    /// come from the resolved show clock (Ableton Link / Pro DJ Link /
    /// audio-derived tempo) instead of the `DEMO_BPM` wall clock, so every
    /// autopilot breath/sweep locks to the track's real beat grid. The
    /// `energy_slow` silence depth-gate is byte-identical to `from_cue`
    /// (clock-independent) — the oscillators still go still in quiet.
    /// `from_cue` stays the deterministic fallback for pre-clock frames + tests.
    ///
    /// `beat_phase` is `PhaseState::beat` (= `clock.phase`, the Link/ProDJ
    /// quantum-snapped within-beat fraction), NOT `frame_time`. `bar`/`phrase`
    /// track the real clock; `phrase` follows the clock quantum (4 on Pro DJ,
    /// 32 on Link/derived) rather than the hardcoded 32 — an intended
    /// tempo-lock improvement, not a regression. The discrete gobo/laser slot
    /// picker also locks, via `PhaseState::phrase_count` carried on
    /// `phrase_count` here (COM-499 Phase A D5).
    pub fn from_clock(snapshot: &CueSnapshot, phase: PhaseState) -> Self {
        // Unwrapped interval counts mirroring `PhaseState::from_show_clock`'s
        // wrapped derivation: bars = beat/4, phrases = beat/quantum (the
        // quantum itself isn't carried, but `phrase_count + phrase`
        // reconstructs the unwrapped phrase count exactly). `beats_abs <= 0`
        // means the producer carried no absolute beat (default `PhaseState`,
        // legacy callers) — zero all three so `sample()` falls back to the
        // wrapped phases as one consistent set.
        let beats_abs = phase.beats_abs.max(0.0);
        let (bars_abs, phrases_abs) = if beats_abs > 0.0 {
            (
                beats_abs / BEATS_PER_BAR as f64,
                phase.phrase_count as f64 + phase.phrase.rem_euclid(1.0) as f64,
            )
        } else {
            (0.0, 0.0)
        };
        Self {
            beat_phase: phase.beat,
            bar_phase: phase.bar,
            phrase_phase: phase.phrase,
            beats_abs,
            bars_abs,
            phrases_abs,
            phrase_count: phase.phrase_count,
            energy_env: smooth_ramp(snapshot.energy_slow, OSC_GATE_SILENCE, OSC_GATE_FULL),
            flow: 1.0 - smooth_ramp(snapshot.energy_slow, OSC_FLOW_LO, OSC_FLOW_HI),
        }
    }

    pub fn sample(&self, binding: OscillatorBinding) -> f32 {
        let ratio = binding.ratio.max(0.0);
        // Flow crossfade: blend the designed rate with a half-time twin by
        // slow energy, so quiet/groove sections breathe instead of ticking at
        // the full beat clock. Square is exempt — gate semantics (deck strobe
        // checks the sign) must stay crisp, and blending two pulse trains
        // unions their on-windows into MORE firing, not less. Both twins ride
        // the same unwrapped clock, so each is phase-continuous and the blend
        // weight (smoothed upstream) glides — never retune a ratio in place.
        //
        // The blend also requires an unwrapped count: on the wrapped fallback
        // (no absolute beat source) a half-ratio twin compresses into
        // [0, 0.5) and snaps at every parent boundary — the exact bug class
        // the unwrapped field fixed — so legacy/default producers stay
        // bit-for-bit on the historical full-rate path at every energy.
        let unwrapped = match binding.interval {
            OscillatorInterval::Beat => self.beats_abs,
            OscillatorInterval::Bar => self.bars_abs,
            OscillatorInterval::Phrase => self.phrases_abs,
        };
        let wave = if self.flow > 0.0
            && binding.shape != OscillatorShape::Square
            && unwrapped > 0.0
        {
            let fast = self.wave_at_ratio(binding, ratio);
            let slow = self.wave_at_ratio(binding, ratio * OSC_FLOW_HALF);
            fast + (slow - fast) * self.flow
        } else {
            self.wave_at_ratio(binding, ratio)
        };
        // `energy_env` gates depth — the oscillators ride the slow energy
        // envelope, so they go still in silence rather than wobbling on a
        // wall clock and masking the instantaneous response.
        wave * binding.amount.clamp(-1.0, 1.0) * self.energy_env
    }

    /// Raw carrier wave for `binding` at an explicit frequency `ratio`
    /// (before amount/depth scaling) — the twin-sampling primitive `sample()`
    /// crossfades for the flow blend.
    fn wave_at_ratio(&self, binding: OscillatorBinding, ratio: f32) -> f32 {
        // Fractional ratios (<1.0, or any non-integer) must run off the
        // UNWRAPPED interval count: `fract(unwrapped * ratio + phase)` is the
        // only formulation where ratio 0.5 yields a continuous 2-interval
        // cycle. Multiplying the already-wrapped [0,1) base compressed the
        // cycle into [0, ratio) and snapped to 0 at every parent boundary —
        // the "2-beat triangle breath" was actually a per-beat sawtooth.
        // Integer ratios are unchanged (`fract((k + p) * n) == fract(n * p)`).
        let phase = self.cycle_phase(binding.interval, ratio, binding.phase);
        bipolar_wave(binding.shape, phase, binding.pulse_width)
    }

    /// Raw carrier phase for something running `ratio` cycles per `interval`,
    /// offset by `phase` — the clock position before any waveform is applied.
    /// The mover figures read this directly (a circle needs sine AND cosine of
    /// one phase; a bounce needs the beat position itself), so they stay
    /// locked to the same beat/bar/phrase grid as every `sample()` binding.
    ///
    /// Fractional ratios run off the UNWRAPPED interval count (see
    /// [`Self::wave_at_ratio`]); with no absolute beat source it falls back
    /// to the wrapped phases.
    pub fn cycle_phase(&self, interval: OscillatorInterval, ratio: f32, phase: f32) -> f32 {
        let unwrapped = match interval {
            OscillatorInterval::Beat => self.beats_abs,
            OscillatorInterval::Bar => self.bars_abs,
            OscillatorInterval::Phrase => self.phrases_abs,
        };
        if unwrapped > 0.0 {
            ((unwrapped * ratio as f64 + phase as f64).fract()) as f32
        } else {
            // No absolute beat source (old producers / default snapshots):
            // the original wrapped-phase path, so nothing breaks — fractional
            // ratios keep their historical truncating behaviour here.
            let base = match interval {
                OscillatorInterval::Beat => self.beat_phase,
                OscillatorInterval::Bar => self.bar_phase,
                OscillatorInterval::Phrase => self.phrase_phase,
            };
            (base * ratio + phase).fract()
        }
    }

    /// Integer phrase ordinal for phrase-quantized discrete choices (gobo slot,
    /// laser macro). Clock-locked on the `from_clock` path, 122-BPM fallback on
    /// `from_cue`.
    pub fn phrase_count(&self) -> u32 {
        self.phrase_count
    }

    /// Slow-energy depth gate (0-1) for this frame — the COM-409A silence
    /// envelope every `sample()` is scaled by. Exposed so field-derived intent
    /// terms (e.g. the mover tilt's field-phase drift) ride the SAME gate and go
    /// still in silence instead of wobbling on the field while no music plays.
    pub fn energy_env(&self) -> f32 {
        self.energy_env
    }
}

/// Integer phrase counter on the `DEMO_BPM` (122) grid — the FALLBACK phrase
/// ordinal used by [`OscillatorFrameSnapshot::from_cue`] when no show clock is
/// resolved. The live clock path instead carries the real phrase ordinal via
/// `PhaseState::phrase_count` (COM-499 Phase A D5), so on a real clock the
/// gobo/laser slot picker advances on true phrase boundaries.
pub(super) fn phrase_index(snapshot: &CueSnapshot) -> u32 {
    let beats = snapshot.frame_time.max(0.0) * DEMO_BPM / 60.0;
    (beats / (BEATS_PER_BAR * BARS_PER_PHRASE)).floor() as u32
}

pub fn bipolar_wave(shape: OscillatorShape, phase: f32, pulse_width: f32) -> f32 {
    let p = phase.fract();
    match shape {
        OscillatorShape::Sine => (p * std::f32::consts::TAU).sin(),
        OscillatorShape::Triangle => 1.0 - 4.0 * (p - 0.5).abs(),
        OscillatorShape::Saw => p * 2.0 - 1.0,
        OscillatorShape::Square => {
            if p < pulse_width.clamp(0.01, 0.99) { 1.0 } else { -1.0 }
        }
    }
}

pub fn add_bipolar(base: f32, delta: f32) -> f32 {
    (base + delta).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hue::CueSnapshot;

    #[test]
    fn wave_shapes_are_bipolar_and_deterministic() {
        assert!((bipolar_wave(OscillatorShape::Sine, 0.25, 0.5) - 1.0).abs() < 0.0001);
        assert!((bipolar_wave(OscillatorShape::Triangle, 0.5, 0.5) - 1.0).abs() < 0.0001);
        assert!((bipolar_wave(OscillatorShape::Saw, 0.5, 0.5) - 0.0).abs() < 0.0001);
        assert_eq!(bipolar_wave(OscillatorShape::Square, 0.25, 0.5), 1.0);
        assert_eq!(bipolar_wave(OscillatorShape::Square, 0.75, 0.5), -1.0);
    }

    #[test]
    fn frame_snapshot_samples_repeatably() {
        let cue = CueSnapshot { frame_time: 12.25, energy_slow: 1.0, ..Default::default() };
        let frame = OscillatorFrameSnapshot::from_cue(&cue);
        let binding = OscillatorBinding::new(
            OscillatorShape::Sine,
            OscillatorInterval::Bar,
            0.25,
        )
        .with_ratio(0.5)
        .with_phase(0.125);
        assert_eq!(frame.sample(binding), frame.sample(binding));
    }

    #[test]
    fn square_width_controls_pulse_duty() {
        let cue = CueSnapshot { frame_time: 0.0, energy_slow: 1.0, ..Default::default() };
        let frame = OscillatorFrameSnapshot::from_cue(&cue);
        let binding = OscillatorBinding::new(
            OscillatorShape::Square,
            OscillatorInterval::Beat,
            1.0,
        )
        .with_pulse_width(0.2);
        assert_eq!(frame.sample(binding), 1.0);
    }

    #[test]
    fn from_clock_uses_clock_phases_not_wallclock() {
        // Phases come from the show clock; frame_time is irrelevant on this path.
        let cue = CueSnapshot { frame_time: 9999.0, energy_slow: 1.0, ..Default::default() };
        let phase = PhaseState { beat: 0.5, bar: 0.25, phrase: 0.1, phrase_count: 7, beats_abs: 0.0 };
        let frame = OscillatorFrameSnapshot::from_clock(&cue, phase);
        assert_eq!(frame.beat_phase, 0.5);
        assert_eq!(frame.bar_phase, 0.25);
        assert_eq!(frame.phrase_phase, 0.1);
        // The clock's phrase ordinal flows through to the discrete slot picker.
        assert_eq!(frame.phrase_count(), 7);
    }

    #[test]
    fn from_clock_preserves_silence_gate_byte_identically() {
        // The energy_slow depth-gate is clock-independent: same energy_slow =>
        // same energy_env as from_cue, so silence still stills the LFOs. Exact
        // float-eq is intentional — both paths run identical arithmetic.
        for e in [0.0_f32, 0.06, 0.2, 0.4, 1.0] {
            let cue = CueSnapshot { frame_time: 3.21, energy_slow: e, ..Default::default() };
            let from_cue = OscillatorFrameSnapshot::from_cue(&cue);
            let from_clock = OscillatorFrameSnapshot::from_clock(&cue, PhaseState::default());
            assert_eq!(
                from_cue.energy_env, from_clock.energy_env,
                "energy_env must match from_cue for energy_slow={e}"
            );
        }
    }

    #[test]
    fn from_clock_locks_to_show_clock_quantum() {
        // A Pro-DJ-style clock (quantum 4) at beat 9.0, within-beat phase 0.25.
        let clock = crate::emerge::ShowClockSnapshot {
            beat: 9.0,
            phase: 0.25,
            quantum: 4.0,
            ..Default::default()
        };
        let phase = PhaseState::from_show_clock(clock);
        let cue = CueSnapshot { frame_time: 0.0, energy_slow: 1.0, ..Default::default() };
        let frame = OscillatorFrameSnapshot::from_clock(&cue, phase);
        // beat_phase = clock.phase (within-beat fraction), NOT clock.beat.fract().
        assert!((frame.beat_phase - 0.25).abs() < 1e-6);
        // phrase tracks the clock quantum (9/4 = 2.25 -> .25), not the fixed 32.
        assert!((frame.phrase_phase - 0.25).abs() < 1e-6);
    }

    /// Sample a Beat-interval binding at an absolute clock beat position.
    fn sample_at_beat(binding: OscillatorBinding, beat: f64) -> f32 {
        let clock = crate::emerge::ShowClockSnapshot {
            beat,
            phase: (beat.fract() as f32).rem_euclid(1.0),
            quantum: 4.0,
            ..Default::default()
        };
        let cue = CueSnapshot { energy_slow: 1.0, ..Default::default() };
        OscillatorFrameSnapshot::from_clock(&cue, PhaseState::from_show_clock(clock))
            .sample(binding)
    }

    #[test]
    fn ratio_below_one_runs_a_continuous_two_beat_cycle() {
        // with_ratio(0.5) on Beat must read as ONE smooth 2-beat triangle
        // breath. The old wrapped-base math (`fract(beat_phase * 0.5)`)
        // compressed the cycle into [0, 0.5) and snapped to 0 at every beat
        // boundary — a per-beat sawtooth, not a breath.
        let binding = OscillatorBinding::new(
            OscillatorShape::Triangle,
            OscillatorInterval::Beat,
            1.0,
        )
        .with_ratio(0.5);
        // Triangle over the 2-beat period: -1 at beats 0/2, 0 at the halves,
        // +1 at beat 1. (Beat 0.0 has no absolute count yet and takes the
        // wrapped fallback, which agrees at the origin.)
        assert!((sample_at_beat(binding, 0.0) - -1.0).abs() < 1e-4);
        assert!((sample_at_beat(binding, 0.5) - 0.0).abs() < 1e-4);
        assert!((sample_at_beat(binding, 1.0) - 1.0).abs() < 1e-4);
        assert!((sample_at_beat(binding, 1.5) - 0.0).abs() < 1e-4);
        assert!((sample_at_beat(binding, 2.0) - -1.0).abs() < 1e-4);
        // No snap crossing the parent boundary: the old code jumped from +1
        // straight to -1 across beat 1.0.
        let before = sample_at_beat(binding, 1.0 - 1e-3);
        let after = sample_at_beat(binding, 1.0 + 1e-3);
        assert!(
            (before - after).abs() < 0.02,
            "ratio 0.5 must be continuous across the beat boundary: {before} vs {after}"
        );
    }

    #[test]
    fn integer_ratios_keep_the_wrapped_phase_behavior() {
        // fract((k + p) * n) == fract(n * p) for integer k, n — ratio 1.0 and
        // 2.0 bindings observe the same phase whether driven by the unwrapped
        // count or the old wrapped base. Guards the no-regression contract of
        // the unwrapped-interval fix on every interval.
        let cue = CueSnapshot { energy_slow: 1.0, ..Default::default() };
        let clock = crate::emerge::ShowClockSnapshot {
            beat: 37.25,
            phase: 0.25,
            quantum: 4.0,
            ..Default::default()
        };
        let frame = OscillatorFrameSnapshot::from_clock(&cue, PhaseState::from_show_clock(clock));
        for ratio in [1.0_f32, 2.0] {
            for (interval, wrapped) in [
                (OscillatorInterval::Beat, frame.beat_phase),
                (OscillatorInterval::Bar, frame.bar_phase),
                (OscillatorInterval::Phrase, frame.phrase_phase),
            ] {
                let binding = OscillatorBinding::new(OscillatorShape::Saw, interval, 1.0)
                    .with_ratio(ratio)
                    .with_phase(0.125);
                // The pre-fix formulation, computed explicitly.
                let old = bipolar_wave(
                    OscillatorShape::Saw,
                    (wrapped * ratio + 0.125).fract(),
                    0.5,
                );
                let new = frame.sample(binding);
                assert!(
                    (new - old).abs() < 1e-3,
                    "integer ratio {ratio} on {interval:?} must match the wrapped path: {new} vs {old}"
                );
            }
        }
    }

    #[test]
    fn fractional_ratio_falls_back_to_wrapped_phase_without_a_beat_source() {
        // Default PhaseState carries no absolute beat (beats_abs 0.0): the
        // sample must take the historical wrapped path so old producers and
        // default snapshots are bit-for-bit unaffected by the unwrapped fix.
        let cue = CueSnapshot { energy_slow: 1.0, ..Default::default() };
        let frame = OscillatorFrameSnapshot::from_clock(&cue, PhaseState::default());
        let binding = OscillatorBinding::new(
            OscillatorShape::Triangle,
            OscillatorInterval::Beat,
            1.0,
        )
        .with_ratio(0.5);
        let expected = bipolar_wave(
            OscillatorShape::Triangle,
            (frame.beat_phase * 0.5).fract(),
            0.5,
        );
        assert_eq!(frame.sample(binding), expected);
    }

    /// Frame at a known clock beat with a given slow energy.
    fn frame_at(beat: f64, energy_slow: f32) -> OscillatorFrameSnapshot {
        let clock = crate::emerge::ShowClockSnapshot {
            beat,
            phase: (beat.fract() as f32).rem_euclid(1.0),
            quantum: 4.0,
            ..Default::default()
        };
        let cue = CueSnapshot { energy_slow, ..Default::default() };
        OscillatorFrameSnapshot::from_clock(&cue, PhaseState::from_show_clock(clock))
    }

    #[test]
    fn flow_blend_runs_half_time_at_low_energy() {
        // Below OSC_FLOW_LO the blend sits fully on the half-time twin: a
        // ratio-1.0 sine on Beat must sample as the SAME motion one octave
        // slower (× the depth gate) — quiet sections breathe, not tick.
        let frame = frame_at(9.25, 0.10); // audible (> gate silence), < FLOW_LO
        let binding =
            OscillatorBinding::new(OscillatorShape::Sine, OscillatorInterval::Beat, 1.0);
        let half_phase = (9.25_f64 * 0.5).fract() as f32;
        let expected =
            bipolar_wave(OscillatorShape::Sine, half_phase, 0.5) * frame.energy_env;
        assert!(
            (frame.sample(binding) - expected).abs() < 1e-5,
            "low energy must sample the half-time twin"
        );
    }

    #[test]
    fn flow_blend_keeps_high_energy_byte_identical() {
        // At/above OSC_FLOW_HI the flow blend is fully off — playing shows at
        // energy keep the exact pre-flow output at the designed rate.
        for e in [OSC_FLOW_HI, 0.8, 1.0] {
            let frame = frame_at(9.25, e);
            let binding =
                OscillatorBinding::new(OscillatorShape::Sine, OscillatorInterval::Beat, 1.0);
            let full_phase = (9.25_f64).fract() as f32;
            let expected =
                bipolar_wave(OscillatorShape::Sine, full_phase, 0.5) * frame.energy_env;
            assert!(
                (frame.sample(binding) - expected).abs() < 1e-5,
                "energy_slow={e} must run at the designed rate"
            );
        }
    }

    #[test]
    fn flow_blend_leaves_square_rate_crisp() {
        // Square is exempt from the flow blend even at low energy: gate
        // consumers read the SIGN of the sample, and crossfading two pulse
        // trains unions their on-windows into MORE firing, not slower.
        let frame = frame_at(9.25, 0.10);
        let binding =
            OscillatorBinding::new(OscillatorShape::Square, OscillatorInterval::Beat, 1.0);
        let full_phase = (9.25_f64).fract() as f32; // 0.25 < pw 0.5 → +1
        let expected =
            bipolar_wave(OscillatorShape::Square, full_phase, 0.5) * frame.energy_env;
        assert_eq!(frame.sample(binding), expected);
    }

    #[test]
    fn flow_blend_skips_the_wrapped_fallback_path() {
        // No absolute beat source (default PhaseState): even at low energy
        // the sample must stay bit-for-bit on the historical full-rate
        // wrapped formulation — a half-ratio twin on the wrapped base snaps
        // at every parent boundary (reads FASTER, the opposite of flow).
        let cue = CueSnapshot { energy_slow: 0.10, ..Default::default() };
        let frame = OscillatorFrameSnapshot::from_clock(&cue, PhaseState::default());
        let binding = OscillatorBinding::new(
            OscillatorShape::Triangle,
            OscillatorInterval::Beat,
            1.0,
        );
        let expected = bipolar_wave(OscillatorShape::Triangle, frame.beat_phase, 0.5)
            * frame.energy_env;
        assert_eq!(frame.sample(binding), expected);
    }

    #[test]
    fn flow_blend_is_continuous_across_the_groove_ramp() {
        // The blend weight is a smoothstep of the (upstream-smoothed) slow
        // energy: adjacent energy values must give nearby samples — a glide,
        // never a rate cliff.
        let binding =
            OscillatorBinding::new(OscillatorShape::Sine, OscillatorInterval::Beat, 1.0);
        let mut prev = frame_at(9.1, 0.12).sample(binding);
        let mut e = 0.13_f32;
        while e <= 0.60 {
            let cur = frame_at(9.1, e).sample(binding);
            assert!(
                (cur - prev).abs() < 0.06,
                "rate blend stepped {prev} → {cur} at energy_slow={e}"
            );
            prev = cur;
            e += 0.01;
        }
    }

    #[test]
    fn from_cue_fractional_ratio_is_continuous_across_its_beat_grid() {
        // The 122-BPM fallback path also carries an unwrapped count (from the
        // same frame_time grid as its wrapped phases), so a ratio-0.5 binding
        // is continuous across the fallback grid's beat boundary too.
        let beat_secs = 60.0 / DEMO_BPM; // one 122-BPM beat
        let binding = OscillatorBinding::new(
            OscillatorShape::Triangle,
            OscillatorInterval::Beat,
            1.0,
        )
        .with_ratio(0.5);
        let sample_at = |t: f32| {
            let cue = CueSnapshot { frame_time: t, energy_slow: 1.0, ..Default::default() };
            OscillatorFrameSnapshot::from_cue(&cue).sample(binding)
        };
        let before = sample_at(beat_secs * 5.0 - 0.001);
        let after = sample_at(beat_secs * 5.0 + 0.001);
        assert!(
            (before - after).abs() < 0.05,
            "from_cue ratio 0.5 must not snap at the 122-BPM beat boundary: {before} vs {after}"
        );
    }
}

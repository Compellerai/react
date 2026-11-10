//! COM-499 Phase B — operator LFO bank (rustjay-style modulation matrix).
//!
//! Eight independent, free-running bipolar LFO slots, each routable to a global
//! modulation target. This module is the pure ENGINE: the slot model, the five
//! waveforms, beat-locked vs free-run phase, the additive per-target sum, and
//! the persistence shape. It does NOT wire into the render loop or the UI — B2
//! evaluates the bank at the `osc/thread.rs` CueSnapshot-assembly seam and
//! applies the modulation ADDITIVELY on top of the COM-499 autopilot; B3 makes
//! depth/rate MIDI-bindable; B4 adds the egui editor + persistence.
//!
//! Design notes folded in from the wb1o0gjkf review:
//!  * tempo-sync phase is derived DIRECTLY from the show-clock beat (drift-free,
//!    re-anchored every frame) — never dt-integrated. dt-integration is reserved
//!    for free-run (Hz) slots, with a clamped dt so a stalled frame can't fling
//!    the phase.
//!  * `division` is a bounds-checked index (corrupt persisted/MIDI data must not
//!    panic the render path on the 4 live streams); `rate_hz` is finite-guarded
//!    before use.
//!  * fixed flat target accumulators (no ParameterDescriptor/HashMap registry —
//!    8 fixed slots into a handful of global targets don't need one).
//!  * `#[serde(default)]` container + a manual `Default` so a saved bank survives
//!    a schema addition (a v1.1 field never wipes the slot); `#[serde(other)]`
//!    target so an unknown future target loads as `Unknown`, not a hard error.

use serde::{Deserialize, Serialize};

/// Beats-per-cycle for the tempo-sync divisions (1/16 .. 8 of a beat ..),
/// indexed by `LfoSlot::division`. At 120 BPM a `1/4` cycle (0.25 beats) is the
/// rustjay default and yields ~8 Hz.
pub const BEAT_DIVISIONS: [f32; 8] = [0.0625, 0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0];

/// Index of the `1/4`-beat division — the default for a freshly-enabled slot
/// (decision D3: new slots are tempo-synced 1/4, "locked to the music" out of
/// the box).
const DIV_QUARTER: u8 = 2;

/// Default modulation depth for a fresh slot.
const DEFAULT_AMPLITUDE: f32 = 0.3;

/// The five rustjay waveforms, all producing bipolar `[-1, 1]` output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LfoWaveform {
    #[default]
    Sine,
    Triangle,
    /// Rising linear ramp, instant reset.
    Ramp,
    /// Falling linear ramp, instant reset.
    Saw,
    Square,
}

/// Where a slot routes its modulation. v1 targets are the global knobs that
/// already (Brightness/Reactivity) or will (Saturation/HueShift, via the B2
/// colour-grade pass) fan out to all four lighting outputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LfoTarget {
    #[default]
    None,
    Brightness,
    Reactivity,
    Saturation,
    HueShift,
    /// Forward-compat: a target written by a newer build deserializes here
    /// instead of failing the whole bank load.
    #[serde(other)]
    Unknown,
}

/// One LFO slot. Config fields persist; the three runtime fields are
/// `#[serde(skip)]` so phase survives a config edit in memory but is never
/// written (B4 copies them across a slot replace for click-free retargeting).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LfoSlot {
    pub enabled: bool,
    pub waveform: LfoWaveform,
    pub target: LfoTarget,
    /// Modulation depth — the bipolar output is `wave * amplitude`.
    pub amplitude: f32,
    /// Phase offset in turns `[0, 1)` (the UI presents 0–360°). Applied at
    /// sample time so it never corrupts the running phase accumulator.
    pub phase_offset: f32,
    /// Beat-lock to the show clock when true; free-run at `rate_hz` otherwise.
    pub tempo_sync: bool,
    /// Index into [`BEAT_DIVISIONS`] (bounds-checked at use).
    pub division: u8,
    /// Free-run frequency in Hz (used when `tempo_sync` is false, or while the
    /// tempo estimate is not yet confident).
    pub rate_hz: f32,
    #[serde(skip)]
    pub phase: f32,
    #[serde(skip)]
    pub output: f32,
}

impl Default for LfoSlot {
    fn default() -> Self {
        Self {
            enabled: false,
            waveform: LfoWaveform::Sine,
            target: LfoTarget::None,
            amplitude: DEFAULT_AMPLITUDE,
            phase_offset: 0.0,
            tempo_sync: true,
            division: DIV_QUARTER,
            rate_hz: 1.0,
            phase: 0.0,
            output: 0.0,
        }
    }
}

impl LfoSlot {
    /// Advance the phase and recompute the bipolar output (0.0 when disabled).
    ///
    /// `dt` — frame delta seconds (free-run slots). `clock_beat` — the resolved
    /// show-clock beat position (tempo-sync slots). `tempo_confident` — gates
    /// tempo-sync: while the tempo estimate is not yet trusted (show start /
    /// post-silence) a sync slot free-runs at its Hz rate to avoid an audible
    /// rate-wobble as the estimate converges (decision D7).
    pub fn update(&mut self, dt: f32, clock_beat: f64, tempo_confident: bool) -> f32 {
        if !self.enabled {
            self.output = 0.0;
            return 0.0;
        }
        if self.tempo_sync && tempo_confident {
            // Beat-locked: derive phase straight from the clock so it never
            // drifts and re-anchors every frame (quantum-snap is a no-op).
            // `.rem_euclid` in f64 BEFORE the f32 cast keeps multi-day clock
            // beats precise. `division` is bounds-checked — corrupt data must
            // not panic the live render path.
            let bpc = BEAT_DIVISIONS
                .get(self.division as usize)
                .copied()
                .unwrap_or(0.25)
                .max(1e-4) as f64;
            self.phase = (clock_beat / bpc).rem_euclid(1.0) as f32;
        } else {
            // Free-run: integrate a finite, positive Hz rate over a clamped dt
            // so a stalled or dropped frame can't fling the phase.
            let rate = if self.rate_hz.is_finite() && self.rate_hz > 0.0 {
                self.rate_hz
            } else {
                0.0
            };
            let dt = if dt.is_finite() { dt.clamp(0.0, 0.1) } else { 0.0 };
            self.phase = (self.phase + rate * dt).rem_euclid(1.0);
        }
        let sample_phase = (self.phase + self.phase_offset).rem_euclid(1.0);
        self.output = lfo_wave(self.waveform, sample_phase, 0.5) * self.amplitude;
        self.output
    }
}

/// The summed per-target modulation for one frame. Flat named accumulators
/// (not a registry) — applied additively on top of the autopilot at the
/// fan-out seam in B2, clamped at each apply site to the target's own range.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LfoModulation {
    pub brightness: f32,
    pub reactivity: f32,
    pub saturation: f32,
    pub hue_shift: f32,
}

/// The eight-slot bank. Persisted as a single JSON blob (B4).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LfoBankConfig {
    pub slots: [LfoSlot; 8],
}

impl Default for LfoBankConfig {
    fn default() -> Self {
        Self { slots: [LfoSlot::default(); 8] }
    }
}

impl LfoBankConfig {
    /// Advance every enabled slot and sum its output into the matching target
    /// accumulator. Multiple slots on one target sum (rustjay additive matrix);
    /// the final clamp happens at each apply site in B2.
    pub fn eval(&mut self, dt: f32, clock_beat: f64, tempo_confident: bool) -> LfoModulation {
        let mut m = LfoModulation::default();
        for slot in self.slots.iter_mut() {
            let out = slot.update(dt, clock_beat, tempo_confident);
            match slot.target {
                LfoTarget::Brightness => m.brightness += out,
                LfoTarget::Reactivity => m.reactivity += out,
                LfoTarget::Saturation => m.saturation += out,
                LfoTarget::HueShift => m.hue_shift += out,
                LfoTarget::None | LfoTarget::Unknown => {}
            }
        }
        m
    }
}

/// Sample a waveform at `phase` (turns), bipolar `[-1, 1]`. Distinct from the
/// autopilot's `oscillator::bipolar_wave` — that one is a 4-variant
/// `OscillatorShape` (rising Saw only) on a different consumer; this carries the
/// full five rustjay shapes (rising Ramp AND falling Saw).
pub fn lfo_wave(shape: LfoWaveform, phase: f32, pulse_width: f32) -> f32 {
    let p = phase.rem_euclid(1.0);
    match shape {
        LfoWaveform::Sine => (p * std::f32::consts::TAU).sin(),
        LfoWaveform::Triangle => 1.0 - 4.0 * (p - 0.5).abs(),
        LfoWaveform::Ramp => p * 2.0 - 1.0,
        LfoWaveform::Saw => 1.0 - p * 2.0,
        LfoWaveform::Square => {
            if p < pulse_width.clamp(0.01, 0.99) {
                1.0
            } else {
                -1.0
            }
        }
    }
}

/// Add a bipolar LFO delta onto a base value, clamped to `[min, max]`. The FINAL
/// clamp for an LFO-modulated parameter — distinct from `oscillator::add_bipolar`
/// (hardwired `[0, 1]`, autopilot path); the LFO clamp range is the target's own.
pub fn add_clamped(base: f32, delta: f32, min: f32, max: f32) -> f32 {
    (base + delta).clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled(slot: LfoSlot) -> LfoSlot {
        LfoSlot { enabled: true, ..slot }
    }

    #[test]
    fn lfo_wave_covers_five_bipolar_shapes() {
        assert!((lfo_wave(LfoWaveform::Sine, 0.25, 0.5) - 1.0).abs() < 1e-5);
        assert!((lfo_wave(LfoWaveform::Triangle, 0.5, 0.5) - 1.0).abs() < 1e-5);
        // Ramp rises -1 -> 1; Saw falls 1 -> -1 (the rustjay distinction).
        assert!((lfo_wave(LfoWaveform::Ramp, 0.0, 0.5) + 1.0).abs() < 1e-5);
        assert!((lfo_wave(LfoWaveform::Saw, 0.0, 0.5) - 1.0).abs() < 1e-5);
        assert_eq!(lfo_wave(LfoWaveform::Square, 0.25, 0.5), 1.0);
        assert_eq!(lfo_wave(LfoWaveform::Square, 0.75, 0.5), -1.0);
        // Bounded for every shape across the cycle.
        for shape in [
            LfoWaveform::Sine,
            LfoWaveform::Triangle,
            LfoWaveform::Ramp,
            LfoWaveform::Saw,
            LfoWaveform::Square,
        ] {
            for i in 0..=20 {
                let v = lfo_wave(shape, i as f32 / 20.0, 0.5);
                assert!((-1.0..=1.0).contains(&v), "{shape:?} out of range: {v}");
            }
        }
    }

    #[test]
    fn tempo_sync_phase_tracks_clock_drift_free() {
        // 1/4-beat division (0.25 beats/cycle): clock beat 0.125 -> half a cycle.
        let mut slot = enabled(LfoSlot { tempo_sync: true, division: DIV_QUARTER, ..Default::default() });
        slot.update(0.016, 0.125, true);
        assert!((slot.phase - 0.5).abs() < 1e-6, "phase {}", slot.phase);
        // Re-anchors from the clock every frame regardless of dt (no drift).
        slot.update(10.0, 0.25, true);
        assert!(slot.phase.abs() < 1e-6 || (slot.phase - 1.0).abs() < 1e-6, "phase {}", slot.phase);
    }

    #[test]
    fn free_run_accumulates_over_clamped_dt() {
        // dt is clamped to a 0.1s max (the stall guard), so steps stay <= 0.1.
        let mut slot = enabled(LfoSlot { tempo_sync: false, rate_hz: 1.0, ..Default::default() });
        slot.update(0.05, 0.0, true);
        assert!((slot.phase - 0.05).abs() < 1e-6, "phase {}", slot.phase);
        slot.update(0.05, 0.0, true);
        assert!((slot.phase - 0.10).abs() < 1e-6, "phase {}", slot.phase);
        // A huge dt is clamped to 0.1 — phase advances 0.1, not 10 cycles.
        let before = slot.phase;
        slot.update(10.0, 0.0, true);
        let delta = (slot.phase - before).rem_euclid(1.0);
        assert!((delta - 0.1).abs() < 1e-6, "clamped delta {delta}");
    }

    #[test]
    fn low_confidence_sync_slot_free_runs() {
        // tempo_sync but NOT confident -> free-runs at rate_hz (no clock lock).
        let mut slot = enabled(LfoSlot { tempo_sync: true, rate_hz: 2.0, division: DIV_QUARTER, ..Default::default() });
        slot.update(0.1, 999.0, false); // big clock_beat must be ignored
        assert!((slot.phase - 0.2).abs() < 1e-6, "phase {}", slot.phase);
    }

    #[test]
    fn disabled_slot_outputs_zero() {
        let mut slot = LfoSlot { enabled: false, amplitude: 1.0, ..Default::default() };
        assert_eq!(slot.update(0.1, 1.0, true), 0.0);
        assert_eq!(slot.output, 0.0);
    }

    #[test]
    fn division_out_of_range_does_not_panic() {
        // Corrupt persisted/MIDI division must not index OOB into [_; 8].
        let mut slot = enabled(LfoSlot { tempo_sync: true, division: 255, ..Default::default() });
        let out = slot.update(0.016, 3.0, true);
        assert!(out.is_finite());
        assert!(slot.phase.is_finite());
    }

    #[test]
    fn non_finite_rate_is_ignored() {
        for bad in [f32::INFINITY, f32::NAN, -1.0_f32] {
            let mut slot = enabled(LfoSlot { tempo_sync: false, rate_hz: bad, ..Default::default() });
            slot.update(0.1, 0.0, true);
            assert!(slot.phase.is_finite(), "phase went non-finite for rate {bad}");
        }
    }

    #[test]
    fn eval_sums_targets_then_caller_clamps() {
        let mut bank = LfoBankConfig::default();
        // Two slots both routed to Brightness sum; one to Reactivity.
        bank.slots[0] = enabled(LfoSlot { target: LfoTarget::Brightness, waveform: LfoWaveform::Sine,
            tempo_sync: true, division: DIV_QUARTER, amplitude: 0.5, ..Default::default() });
        bank.slots[1] = enabled(LfoSlot { target: LfoTarget::Brightness, waveform: LfoWaveform::Sine,
            tempo_sync: true, division: DIV_QUARTER, amplitude: 0.5, ..Default::default() });
        bank.slots[2] = enabled(LfoSlot { target: LfoTarget::Reactivity, ..Default::default() });
        // clock beat 0.0625 -> phase 0.25 -> sine == 1.0, so each Brightness slot outputs 0.5.
        let m = bank.eval(0.016, 0.0625, true);
        assert!((m.brightness - 1.0).abs() < 1e-5, "brightness {}", m.brightness);
        // Caller clamps the additive result to the target's range.
        assert_eq!(add_clamped(0.6, m.brightness, 0.0, 1.0), 1.0);
        assert!((add_clamped(0.6, -0.3, 0.0, 1.0) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn default_bank_is_eight_disabled_and_silent() {
        let mut bank = LfoBankConfig::default();
        assert_eq!(bank.slots.len(), 8);
        assert!(bank.slots.iter().all(|s| !s.enabled));
        assert_eq!(bank.eval(0.016, 4.0, true), LfoModulation::default());
    }

    #[test]
    fn slot_blob_missing_fields_default_and_unknown_target_survives() {
        // Forward-compat: a partial blob fills missing fields from Default (a
        // future field addition never wipes a saved slot), and an unknown target
        // string loads as Unknown rather than failing the whole bank load.
        let slot: LfoSlot =
            serde_json::from_str(r#"{"enabled":true,"target":"Brightness"}"#).unwrap();
        assert!(slot.enabled);
        assert_eq!(slot.target, LfoTarget::Brightness);
        assert!((slot.amplitude - DEFAULT_AMPLITUDE).abs() < 1e-6); // from Default
        assert!(slot.tempo_sync); // from Default
        assert_eq!(slot.division, DIV_QUARTER); // from Default

        let future: LfoSlot = serde_json::from_str(r#"{"target":"SomeFutureTarget"}"#).unwrap();
        assert_eq!(future.target, LfoTarget::Unknown);
    }

    #[test]
    fn bank_json_round_trips_and_skips_runtime_fields() {
        let mut bank = LfoBankConfig::default();
        bank.slots[0] = enabled(LfoSlot {
            target: LfoTarget::HueShift,
            waveform: LfoWaveform::Saw,
            phase: 0.42,  // runtime — must NOT serialize
            output: 0.9,  // runtime — must NOT serialize
            ..Default::default()
        });
        let json = serde_json::to_string(&bank).unwrap();
        assert!(!json.contains("\"phase\""), "runtime phase must be skipped");
        assert!(!json.contains("\"output\""), "runtime output must be skipped");
        let back: LfoBankConfig = serde_json::from_str(&json).unwrap();
        // Config round-trips; runtime fields come back at Default (0.0).
        assert_eq!(back.slots[0].target, LfoTarget::HueShift);
        assert_eq!(back.slots[0].waveform, LfoWaveform::Saw);
        assert_eq!(back.slots[0].phase, 0.0);
        assert_eq!(back.slots[0].output, 0.0);
    }
}

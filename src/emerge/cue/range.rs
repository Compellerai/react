//! Range expansion for the mood signals.
//!
//! Tension, stability, balance and the major/minor lean each come out of the
//! analyzer squeezed into a narrow band of their nominal 0..1: tension mostly
//! 0.05–0.3, stability 0.7–0.95, major likelihood 0.4–0.6. Lighting that reads
//! them straight barely moves. A [`RangeExpander`] tracks the band a signal is
//! actually using in this song (a slow low/high pair, fast to widen, slow to
//! narrow) and maps it back onto 0..1, so the calmest moment of a track reads
//! near 0 and its most tense near 1.
//!
//! Two guards keep that honest:
//! - a minimum span, so a signal that is not moving is not blown up into a
//!   full swing of noise;
//! - a share of the absolute value, so a song that is calm from end to end
//!   still reads calm rather than having its calmest bar called 0 and its
//!   least-calm called 1.
//!
//! The expanded signals ride on [`Mood`], beside (not instead of) the raw
//! ones: many consumers have thresholds tuned against the raw ranges.

/// The mood signals on their full 0..1 range. See the module docs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mood {
    /// How tense the moment is against this song's own range.
    pub tension: f32,
    /// How unsettled the harmony is: 1 − stability, expanded.
    pub instability: f32,
    /// Major (1) vs minor (0) lean, expanded around neutral 0.5.
    pub valence: f32,
    /// How decisively the music leans major or minor: 0 is ambiguous,
    /// 1 the clearest lean this song has shown.
    pub polarity_strength: f32,
    /// Spectral balance, expanded.
    pub balance: f32,
}

impl Default for Mood {
    fn default() -> Self {
        Self { tension: 0.0, instability: 0.0, valence: 0.5, polarity_strength: 0.0, balance: 0.5 }
    }
}

/// Tracks the band a signal is using and maps it onto 0..1.
#[derive(Clone, Debug)]
pub struct RangeExpander {
    lo: f32,
    hi: f32,
    started: bool,
    /// Smallest band the signal is stretched over.
    min_span: f32,
    /// Share of the absolute value in the output (0 = fully relative).
    absolute: f32,
}

/// Time to widen the band to a new extreme.
const WIDEN_TAU_S: f32 = 1.0;
/// Time for the band to forget an extreme it has not revisited.
const NARROW_TAU_S: f32 = 45.0;

impl RangeExpander {
    pub const fn new(min_span: f32, absolute: f32) -> Self {
        Self { lo: 0.0, hi: 0.0, started: false, min_span, absolute }
    }

    /// Feed `x` (nominally 0..1) after `dt` seconds; returns it on the song's
    /// own 0..1 range.
    pub fn update(&mut self, x: f32, dt: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        if !self.started {
            self.started = true;
            self.lo = x;
            self.hi = x;
        }
        let widen = 1.0 - (-dt.max(0.0) / WIDEN_TAU_S).exp();
        let narrow = 1.0 - (-dt.max(0.0) / NARROW_TAU_S).exp();
        self.lo += (x - self.lo) * if x < self.lo { widen } else { narrow };
        self.hi += (x - self.hi) * if x > self.hi { widen } else { narrow };
        self.map(x)
    }

    /// `x` on the current band, without learning from it.
    pub fn map(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        let span = (self.hi - self.lo).max(self.min_span);
        let mid = 0.5 * (self.lo + self.hi);
        let relative = ((x - (mid - 0.5 * span)) / span).clamp(0.0, 1.0);
        (self.absolute * x + (1.0 - self.absolute) * relative).clamp(0.0, 1.0)
    }
}

// ── Intensity: how big this moment is ──────────────────────────────────────

/// Dynamics at or below which a passage counts as fully settled, and at or
/// above which it is at full weight (the same band `movement::dynamics_lift`
/// settles the rig over).
const DYNAMICS_SETTLED: f32 = 0.25;
const DYNAMICS_OPEN: f32 = 0.90;
/// Adaptive energy over which a full-weight passage goes from its floor to
/// its top: the groove sits around the middle, a peak near the top.
const PUSH_LO: f32 = 0.15;
const PUSH_HI: f32 = 0.85;
/// Where a full-weight passage at low relative energy sits.
const FULL_WEIGHT_FLOOR: f32 = 0.55;
/// How far a drop's impact lifts the moment.
const DROP_LIFT: f32 = 0.45;

fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One number for how big the moment is, 0..1, that every output's level can
/// hang off. `dynamics` (absolute: how big the passage is on the set's own
/// scale) decides how much of the range is open; adaptive `energy` (where in
/// that passage the music is) places it inside; a drop's impact lifts it to
/// the top. A breakdown reads near 0, a groove ~0.7, a drop 1.
pub fn intensity(dynamics: f32, energy: f32, drop_impact: f32) -> f32 {
    let weight = ((dynamics - DYNAMICS_SETTLED) / (DYNAMICS_OPEN - DYNAMICS_SETTLED)).clamp(0.0, 1.0);
    let push = smoothstep(PUSH_LO, PUSH_HI, energy);
    let level = weight * (FULL_WEIGHT_FLOOR + (1.0 - FULL_WEIGHT_FLOOR) * push);
    (level + DROP_LIFT * drop_impact.clamp(0.0, 1.0) * weight.max(0.5)).clamp(0.0, 1.0)
}

/// Contrast curve applied to an `intensity`-shaped level: reactivity sets
/// how hard the rig separates quiet from loud. 0 is gentle (lows lifted),
/// 0.5 close to linear, 1 deep (quiet passages near dark). 0 maps to 0 and
/// 1 to 1 at every setting: the peak is always full.
pub fn contrast_shape(level: f32, reactivity: f32) -> f32 {
    let gamma = 0.6 + 1.2 * reactivity.clamp(0.0, 1.0);
    level.clamp(0.0, 1.0).powf(gamma)
}

/// The expanders behind [`Mood`], one per signal, owned by the cue engine.
#[derive(Clone, Debug)]
pub struct MoodTracker {
    tension: RangeExpander,
    instability: RangeExpander,
    valence: RangeExpander,
    polarity: RangeExpander,
    balance: RangeExpander,
    mood: Mood,
}

impl Default for MoodTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Below this energy the music is treated as silent: the mood relaxes to
/// neutral instead of reading "no chroma" as maximal instability.
const SILENT_ENERGY: f32 = 0.02;
/// How quickly the mood relaxes in silence.
const SILENT_TAU_S: f32 = 1.5;

impl MoodTracker {
    pub fn new() -> Self {
        Self {
            tension: RangeExpander::new(0.12, 0.15),
            instability: RangeExpander::new(0.15, 0.15),
            valence: RangeExpander::new(0.10, 0.2),
            polarity: RangeExpander::new(0.10, 0.2),
            balance: RangeExpander::new(0.15, 0.3),
            mood: Mood::default(),
        }
    }

    /// Advance on the cue engine's smoothed signals.
    pub fn update(&mut self, energy: f32, tension: f32, stability: f32, major: f32, balance: f32, dt: f32) -> Mood {
        if energy < SILENT_ENERGY {
            let k = 1.0 - (-dt.max(0.0) / SILENT_TAU_S).exp();
            let rest = Mood::default();
            let m = &mut self.mood;
            m.tension += (rest.tension - m.tension) * k;
            m.instability += (rest.instability - m.instability) * k;
            m.valence += (rest.valence - m.valence) * k;
            m.polarity_strength += (rest.polarity_strength - m.polarity_strength) * k;
            m.balance += (rest.balance - m.balance) * k;
            return self.mood;
        }
        let valence = self.valence.update(major, dt);
        self.mood = Mood {
            tension: self.tension.update(tension, dt),
            instability: self.instability.update(1.0 - stability, dt),
            valence,
            polarity_strength: self.polarity.update((major - 0.5).abs() * 2.0, dt),
            balance: self.balance.update(balance, dt),
        };
        self.mood
    }

    pub fn mood(&self) -> Mood {
        self.mood
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn intensity_runs_from_a_breakdown_to_a_drop() {
        let breakdown = intensity(0.30, 0.5, 0.0);
        let groove = intensity(1.0, 0.5, 0.0);
        let peak = intensity(1.0, 0.85, 0.0);
        let drop = intensity(1.0, 0.6, 0.77);
        assert!(breakdown < 0.12, "{breakdown}");
        assert!((0.6..0.8).contains(&groove), "{groove}");
        assert!(peak > 0.97 && drop > 0.97, "{peak} {drop}");
        // A drop called in a low passage is not a full-scale drop.
        assert!(intensity(0.3, 0.6, 0.77) < 0.4);
    }

    #[test]
    fn reactivity_changes_contrast_not_the_peak() {
        for r in [0.0, 0.2, 0.5, 0.8, 1.0] {
            assert!((contrast_shape(1.0, r) - 1.0).abs() < 1e-6);
            assert_eq!(contrast_shape(0.0, r), 0.0);
        }
        assert!(contrast_shape(0.2, 0.0) > contrast_shape(0.2, 0.5));
        assert!(contrast_shape(0.2, 0.5) > contrast_shape(0.2, 1.0));
    }

    #[test]
    fn a_narrow_signal_is_stretched_over_the_full_range() {
        // Tension the way the analyzer reports it: 0.05 to 0.30.
        let mut x = RangeExpander::new(0.12, 0.25);
        let (mut lo, mut hi) = (1.0f32, 0.0f32);
        for i in 0..(120.0 / DT) as usize {
            let t = i as f32 * DT;
            let raw = 0.175 + 0.125 * (t * std::f32::consts::TAU / 16.0).sin();
            let out = x.update(raw, DT);
            if t > 30.0 {
                lo = lo.min(out);
                hi = hi.max(out);
            }
        }
        assert!(hi - lo > 0.7, "spans {lo:.2}..{hi:.2}");
        assert!(lo < 0.1, "the calmest moment reads calm: {lo:.2}");
    }

    #[test]
    fn a_still_signal_is_not_blown_up_into_noise() {
        let mut x = RangeExpander::new(0.12, 0.25);
        let (mut lo, mut hi) = (1.0f32, 0.0f32);
        for i in 0..(60.0 / DT) as usize {
            let wobble = 0.01 * ((i as f32 * 12.9898).sin() * 43_758.547).fract();
            let out = x.update(0.2 + wobble, DT);
            if i as f32 * DT > 10.0 {
                lo = lo.min(out);
                hi = hi.max(out);
            }
        }
        assert!(hi - lo < 0.15, "jitter of 0.01 spans {lo:.2}..{hi:.2}");
    }

    #[test]
    fn mood_signals_span_their_range() {
        // A song that moves between a settled groove and a tense, unsettled
        // build every 20 s, on the analyzer's compressed scales.
        let mut tracker = MoodTracker::new();
        let mut seen = [(1.0f32, 0.0f32); 4];
        for i in 0..(180.0 / DT) as usize {
            let t = i as f32 * DT;
            let s = 0.5 + 0.5 * (t * std::f32::consts::TAU / 40.0).sin();
            let m = tracker.update(0.6, 0.05 + 0.25 * s, 0.95 - 0.25 * s, 0.4 + 0.2 * s, 0.5, DT);
            if t > 60.0 {
                for (slot, v) in [m.tension, m.instability, m.valence, m.polarity_strength].into_iter().enumerate() {
                    seen[slot] = (seen[slot].0.min(v), seen[slot].1.max(v));
                }
            }
        }
        for (name, (lo, hi)) in ["tension", "instability", "valence", "polarity"].iter().zip(seen) {
            let floor = if *name == "polarity" { 0.6 } else { 0.8 };
            assert!(hi - lo >= floor, "{name} spans {lo:.2}..{hi:.2}");
        }
    }

    #[test]
    fn silence_relaxes_the_mood_instead_of_reading_as_chaos() {
        let mut tracker = MoodTracker::new();
        for _ in 0..600 {
            tracker.update(0.6, 0.3, 0.7, 0.6, 0.5, DT);
        }
        // Silence: stability reads 0 (no chroma), which must not be 'unsettled'.
        let mut m = tracker.mood();
        for _ in 0..600 {
            m = tracker.update(0.0, 0.0, 0.0, 0.5, 0.5, DT);
        }
        assert!(m.instability < 0.05 && m.tension < 0.05, "{m:?}");
    }
}

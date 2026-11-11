//! EALP B5 — Tonnetz centroid + voice-leading "surprise" tracker.
//!
//! The 12-bin chroma vector is embedded on TWO tonal circles — the circle of
//! fifths (smooth key motion) and the circle of major thirds (quality motion) —
//! as chroma-weighted vector means. The vector mean gives toroidal wrap for free
//! and collapses to ~0 magnitude on a spread/atonal chroma (a built-in tonal
//! confidence). The fifths-centroid ANGLE drives smooth colour; the frame-vs-slow
//! -baseline DISTANCE over both circles is the harmonic-SURPRISE signal: a key or
//! quality jump moves the centroid away from its settled position, the baseline
//! lags, the distance spikes, then relaxes as the baseline catches up.
//!
//! No chord detection, no labels — just WHERE the harmonic centre-of-mass sits
//! and HOW FAR it moved, which is all a colour+mover rig can express. The spike
//! (Brain decision b4fd6830) confirmed the plan's "needs chord detection" flag is
//! overstated for lighting and this lightweight embedding is the right cut.
//!
//! On quality changes (verified by review): the centroid is a single point per
//! circle, but E vs Eb (a root-fixed C-maj -> C-min) sit ~45deg apart on BOTH the
//! fifths and thirds circles, so the combined delta is large (reviewer-measured
//! ~0.8, well above the surprise threshold) — a quality change DOES register
//! here, treated as a genuine harmonic event. The all-in chord layer
//! (osc/chord.rs) adds the explicit major/minor LABEL on top, for colour
//! temperature, and the P/R/L move type for accent character.

use std::f32::consts::TAU;

/// Circle-of-fifths angle for a pitch class (pc*7 mod 12 steps around the circle).
fn fifths_angle(pc: usize) -> f32 {
    ((pc * 7) % 12) as f32 / 12.0 * TAU
}

/// Circle-of-major-thirds angle for a pitch class (pc*4 mod 12).
fn thirds_angle(pc: usize) -> f32 {
    ((pc * 4) % 12) as f32 / 12.0 * TAU
}

/// A harmonic-centre embedding of one chroma frame (cartesian centroids on the
/// two tonal circles). Magnitude near 0 == no committed tonal centre (atonal).
#[derive(Clone, Copy, Debug, Default)]
pub struct TonnetzCentroid {
    pub fcx: f32,
    pub fcy: f32,
    pub tcx: f32,
    pub tcy: f32,
}

impl TonnetzCentroid {
    /// Chroma-weighted vector mean on both tonal circles, normalised by the chroma
    /// sum so the result lies on/inside the unit circle. A flat (atonal) chroma
    /// yields ~0 magnitude — the embedding self-reports low tonal confidence.
    pub fn from_chroma(chroma: &[f32; 12]) -> Self {
        let sum: f32 = chroma.iter().map(|c| c.max(0.0)).sum();
        if sum <= 1e-6 {
            return Self::default();
        }
        let inv = 1.0 / sum;
        let mut c = Self::default();
        for (pc, &raw) in chroma.iter().enumerate() {
            let w = raw.max(0.0) * inv;
            let (fa, ta) = (fifths_angle(pc), thirds_angle(pc));
            c.fcx += w * fa.cos();
            c.fcy += w * fa.sin();
            c.tcx += w * ta.cos();
            c.tcy += w * ta.sin();
        }
        c
    }

    /// Fifths-centroid hue position (0-1) — the smooth circle-of-fifths colour.
    pub fn fifths_hue(&self) -> f32 {
        (self.fcy.atan2(self.fcx) / TAU).rem_euclid(1.0)
    }

    /// Fifths concentration (0-1) — tonal confidence; low when atonal/spread.
    pub fn fifths_strength(&self) -> f32 {
        self.fcx.hypot(self.fcy).clamp(0.0, 1.0)
    }
}

/// What the tracker publishes each frame for the lighting seam.
#[derive(Clone, Copy, Debug, Default)]
pub struct TonnetzFrame {
    /// 0-1 smooth circle-of-fifths position (drives a gentle harmonic hue drift).
    pub hue: f32,
    /// 0-1 tonal confidence (centroid concentration; ~0 on atonal/silence).
    pub strength: f32,
    /// 0-1ish harmonic-surprise: current centroid vs slow baseline, over both
    /// circles. Self-enveloping (spikes on a jump, relaxes as the baseline settles)
    /// so a consumer needs no decay timer of its own.
    pub delta: f32,
    /// Where the harmony SITS on the circle of fifths, as a point in the unit
    /// disc (lightly smoothed). The angle is the key area, the radius is how
    /// committed the harmony is — an atonal wash collapses to the centre, a
    /// clear chord sits out toward the rim. A 2-D position a mover can aim at
    /// directly: chords a fifth apart are 30° apart, a distant modulation
    /// crosses the disc.
    pub x: f32,
    pub y: f32,
}

/// Stateful per-frame tracker: holds a slow-EMA baseline of the centroid so a
/// genuine harmonic jump registers as a distance spike (the surprise signal).
#[derive(Clone, Copy, Debug, Default)]
pub struct TonnetzTracker {
    baseline: TonnetzCentroid,
    primed: bool,
    /// Lightly smoothed fifths centroid — the published harmonic position.
    pos_x: f32,
    pos_y: f32,
}

/// Slow baseline so a harmonic jump spikes the delta before the baseline catches
/// up (~0.5s envelope at ~47fps). Tunable.
const BASELINE_EMA: f32 = 0.04;
/// Smoothing of the published harmonic position (~100ms at 60fps): enough to
/// take inner-voice flutter off a mover's aim point, quick enough that a chord
/// change still reads as a move. The movement director slews on top.
const POSITION_EMA: f32 = 0.15;

impl TonnetzTracker {
    pub fn update(&mut self, chroma: &[f32; 12]) -> TonnetzFrame {
        let c = TonnetzCentroid::from_chroma(chroma);
        if !self.primed {
            self.baseline = c;
            self.pos_x = c.fcx;
            self.pos_y = c.fcy;
            self.primed = true;
        }
        self.pos_x += (c.fcx - self.pos_x) * POSITION_EMA;
        self.pos_y += (c.fcy - self.pos_y) * POSITION_EMA;
        let delta = ((c.fcx - self.baseline.fcx).powi(2)
            + (c.fcy - self.baseline.fcy).powi(2)
            + (c.tcx - self.baseline.tcx).powi(2)
            + (c.tcy - self.baseline.tcy).powi(2))
        .sqrt()
        .clamp(0.0, 1.0);
        // Baseline eases toward the current centroid so the surprise relaxes.
        self.baseline.fcx += (c.fcx - self.baseline.fcx) * BASELINE_EMA;
        self.baseline.fcy += (c.fcy - self.baseline.fcy) * BASELINE_EMA;
        self.baseline.tcx += (c.tcx - self.baseline.tcx) * BASELINE_EMA;
        self.baseline.tcy += (c.tcy - self.baseline.tcy) * BASELINE_EMA;
        TonnetzFrame {
            hue: c.fifths_hue(),
            strength: c.fifths_strength(),
            delta,
            x: self.pos_x,
            y: self.pos_y,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_hot(pc: usize) -> [f32; 12] {
        let mut c = [0.0; 12];
        c[pc] = 1.0;
        c
    }

    #[test]
    fn single_pitch_sits_on_its_fifths_angle_at_full_strength() {
        let c = TonnetzCentroid::from_chroma(&one_hot(0)); // C
        assert!((c.fifths_strength() - 1.0).abs() < 1e-5, "one-hot = full concentration");
        assert!((c.fifths_hue() - 0.0).abs() < 1e-5, "C at fifths angle 0");
        // G (pc 7) is one step around the circle of fifths from C -> hue ~1/12.
        let g = TonnetzCentroid::from_chroma(&one_hot(7));
        assert!((g.fifths_hue() - 1.0 / 12.0).abs() < 1e-4, "G one fifth from C");
    }

    #[test]
    fn flat_or_empty_chroma_is_atonal_zero_strength() {
        assert!(TonnetzCentroid::from_chroma(&[1.0; 12]).fifths_strength() < 1e-3, "uniform = no centre");
        assert_eq!(TonnetzCentroid::from_chroma(&[0.0; 12]).fifths_strength(), 0.0, "empty = neutral");
    }

    #[test]
    fn tracker_spikes_on_jump_and_relaxes_when_steady() {
        let mut t = TonnetzTracker::default();
        let c_maj = {
            let mut c = [0.0; 12];
            c[0] = 1.0;
            c[4] = 0.8;
            c[7] = 0.9;
            c
        }; // C E G
        let f0 = t.update(&c_maj); // prime: baseline == current -> no surprise
        assert!(f0.delta < 1e-5, "primed frame has no surprise");
        for _ in 0..20 {
            assert!(t.update(&c_maj).delta < 0.05, "steady harmony = low surprise");
        }
        // Jump to a tritone-distant key (F# major-ish): delta spikes.
        let fs_maj = {
            let mut c = [0.0; 12];
            c[6] = 1.0;
            c[10] = 0.8;
            c[1] = 0.9;
            c
        };
        let jump = t.update(&fs_maj);
        assert!(jump.delta > 0.1, "harmonic jump spikes the surprise: {}", jump.delta);
        let mut last = jump.delta;
        for _ in 0..60 {
            last = t.update(&fs_maj).delta;
        }
        assert!(last < jump.delta, "surprise relaxes as the baseline settles");
    }

    #[test]
    fn harmonic_position_tracks_the_key_area_and_collapses_when_atonal() {
        let triad = |root: usize| {
            let mut c = [0.0; 12];
            c[root] = 1.0;
            c[(root + 4) % 12] = 0.8;
            c[(root + 7) % 12] = 0.9;
            c
        };
        let settle = |t: &mut TonnetzTracker, c: &[f32; 12]| {
            let mut f = TonnetzFrame::default();
            for _ in 0..120 {
                f = t.update(c);
            }
            f
        };
        let mut t = TonnetzTracker::default();
        let c = settle(&mut t, &triad(0));
        let g = settle(&mut t, &triad(7));
        let fs = settle(&mut t, &triad(6));
        // A clear triad sits well out from the centre.
        assert!(c.x.hypot(c.y) > 0.4, "a triad is a committed position");
        // A fifth away is a short walk; the tritone is across the disc.
        let near = (g.x - c.x).hypot(g.y - c.y);
        let far = (fs.x - c.x).hypot(fs.y - c.y);
        assert!(near < far * 0.5, "C→G ({near}) is much closer than C→F# ({far})");
        // The position glides rather than teleporting on a change.
        let first = t.update(&triad(0));
        let step = (first.x - fs.x).hypot(first.y - fs.y);
        assert!(step < far * 0.5, "one frame covers only part of the move: {step}");
        // An atonal wash has no position: it drains to the centre.
        let flat = settle(&mut t, &[1.0; 12]);
        assert!(flat.x.hypot(flat.y) < 0.02, "atonal collapses to the centre");
    }

    #[test]
    fn frame_outputs_stay_in_range() {
        let mut t = TonnetzTracker::default();
        for &c in &[[0.0; 12], [1.0; 12], one_hot(3), one_hot(8)] {
            let f = t.update(&c);
            assert!((0.0..1.0).contains(&f.hue) || f.hue == 0.0);
            assert!((0.0..=1.0).contains(&f.strength));
            assert!((0.0..=1.0).contains(&f.delta));
        }
    }
}

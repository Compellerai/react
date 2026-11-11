//! Dynamics — how big the music is right now, on the set's own scale.
//!
//! Every other loudness signal the lighting reads is adaptive on purpose. The
//! analyzer renormalizes energy over 5-50 seconds, and the intensity license
//! measures spectral fullness against a 40-second peak. That keeps a quiet
//! song from reading as dark and a loud one from pinning everything, but it
//! also means a long breakdown drifts back up to "medium", a stripped intro
//! beat reads as full, and nothing remembers how big the last drop was. The
//! rig then never settles, and has nowhere left to go when the music does
//! arrive.
//!
//! The meter here keeps that memory. It follows the RAW level (before any
//! normalization) of three bands and measures each, in dB, against the
//! loudest sustained passage of the last several minutes:
//!
//! * the **low end** carries half the weight: the kick and the bassline are
//!   what a room feels as "it's in", and they are exactly what a breakdown, a
//!   build or a DJ's bass swap takes away;
//! * the **mids** and the **highs** share the rest, so a kick on its own (a
//!   mix-in intro) reads as a groove rather than as the full track.
//!
//! The result is 0..1: about 0.3 for a breakdown of pads and vocals, about
//! 0.7 for a drums-only intro, 1.0 when everything is in. Consumers treat
//! 1.0 as "exactly what the rig did before this existed" and settle below it.
//!
//! The reference is a memory, not a calibration: it rises to a new loudest
//! passage in a few seconds and forgets it over minutes, so a quieter record
//! later in the set becomes the new full scale instead of staying dim all
//! night. Silence freezes it, so a gap between tracks costs nothing.

/// Raw band-sum below this is silence: the same line the arc classifier and
/// the cue engine draw.
const SILENCE_RAW: f32 = 0.0003;

/// The level being judged follows the music quickly on the way up (a drop
/// has to land on the beat) and holds through the gaps between kicks on the
/// way down.
const LEVEL_ATTACK_TAU_S: f32 = 0.08;
const LEVEL_RELEASE_TAU_S: f32 = 1.5;

/// The reference: up to a new loudest passage within a few seconds — slow
/// enough that one crash or a mic pop does not move it far — and down over
/// minutes, so a breakdown, or a whole quieter section of a set, is still
/// measured against the last time everything was in.
const REF_ATTACK_TAU_S: f32 = 4.0;
const REF_RELEASE_TAU_S: f32 = 300.0;

/// dB below the reference at which a band counts for nothing, and at which
/// it counts as fully in. The low end swings furthest, so it gets the widest
/// ramp. "Full" sits a little under 0 dB because a steady passage rides just
/// below its own peaks.
const LOW_FLOOR_DB: f32 = -14.0;
const LOW_FULL_DB: f32 = -2.0;
const MID_FLOOR_DB: f32 = -12.0;
const MID_FULL_DB: f32 = -2.0;
const HIGH_FLOOR_DB: f32 = -12.0;
const HIGH_FULL_DB: f32 = -2.0;

/// How much each band counts.
const LOW_WEIGHT: f32 = 0.50;
const MID_WEIGHT: f32 = 0.25;
const HIGH_WEIGHT: f32 = 0.25;

/// The published value rises fast and settles gently: a drop is immediate,
/// a breakdown eases the rig down over a couple of bars.
const VALUE_ATTACK_TAU_S: f32 = 0.15;
const VALUE_RELEASE_TAU_S: f32 = 2.0;

/// Until the meter has heard a while of music it has nothing to compare
/// against, and the first thing it hears would read as the loudest ever.
/// Hold the reading at a groove until then.
const WARMUP_CAP: f32 = 0.75;
const WARMUP_HOLD_S: f32 = 15.0;
const WARMUP_OPEN_S: f32 = 45.0;

fn ease(dt: f32, tau: f32) -> f32 {
    1.0 - (-dt.max(0.0) / tau.max(1e-3)).exp()
}

/// 0..1 for a level `db` between `floor_db` and `full_db`.
fn ramp(db: f32, floor_db: f32, full_db: f32) -> f32 {
    ((db - floor_db) / (full_db - floor_db)).clamp(0.0, 1.0)
}

/// One band: an envelope follower and the long-memory reference it is
/// measured against.
#[derive(Clone, Copy, Debug, Default)]
struct Band {
    level: f32,
    reference: f32,
}

impl Band {
    fn update(&mut self, dt: f32, raw: f32, audible: bool) {
        let raw = raw.max(0.0);
        let tau = if raw > self.level { LEVEL_ATTACK_TAU_S } else { LEVEL_RELEASE_TAU_S };
        self.level += (raw - self.level) * ease(dt, tau);
        if !audible {
            return;
        }
        if self.reference <= 0.0 {
            self.reference = self.level;
        } else {
            let tau = if self.level > self.reference { REF_ATTACK_TAU_S } else { REF_RELEASE_TAU_S };
            self.reference += (self.level - self.reference) * ease(dt, tau);
        }
    }

    /// The band's level relative to its reference, in dB (0 = at the
    /// reference). A band the set has never used reads as absent.
    fn relative_db(&self) -> f32 {
        if self.reference <= 1e-9 || self.level <= 1e-9 {
            return -120.0;
        }
        20.0 * (self.level / self.reference).log10()
    }
}

/// See the module docs. Feed it once per frame; read [`DynamicsMeter::value`].
#[derive(Clone, Copy, Debug)]
pub struct DynamicsMeter {
    low: Band,
    mid: Band,
    high: Band,
    value: f32,
    /// Seconds of audible music heard so far.
    heard_s: f32,
}

impl Default for DynamicsMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl DynamicsMeter {
    pub fn new() -> Self {
        Self {
            low: Band::default(),
            mid: Band::default(),
            high: Band::default(),
            // Open until proven otherwise, like the intensity license: a rig
            // that has heard nothing yet behaves as it always has.
            value: 1.0,
            heard_s: 0.0,
        }
    }

    /// Advance by `dt` seconds with this frame's raw band levels: the sum of
    /// all three bands, the bass band and the high band (what
    /// `LightingMetrics` carries). Returns the new reading.
    pub fn update(&mut self, dt: f32, raw_energy: f32, raw_bass: f32, raw_high: f32) -> f32 {
        let dt = dt.clamp(0.0, 0.5);
        let audible = raw_energy >= SILENCE_RAW;
        // A producer with no band split (a bare level, or no metrics at all)
        // gives the meter nothing to measure. Stay where we are — open, on a
        // rig that has never been fed — rather than read it as a breakdown.
        let no_bands = audible && raw_bass <= 0.0 && raw_high <= 0.0;
        if no_bands || (!audible && self.heard_s <= 0.0) {
            return self.value;
        }
        let raw_mid = (raw_energy - raw_bass - raw_high).max(0.0);
        self.low.update(dt, raw_bass, audible);
        self.mid.update(dt, raw_mid, audible);
        self.high.update(dt, raw_high, audible);

        let mut target = if audible {
            LOW_WEIGHT * ramp(self.low.relative_db(), LOW_FLOOR_DB, LOW_FULL_DB)
                + MID_WEIGHT * ramp(self.mid.relative_db(), MID_FLOOR_DB, MID_FULL_DB)
                + HIGH_WEIGHT * ramp(self.high.relative_db(), HIGH_FLOOR_DB, HIGH_FULL_DB)
        } else {
            0.0
        };
        if audible {
            self.heard_s += dt;
            let open = ((self.heard_s - WARMUP_HOLD_S) / (WARMUP_OPEN_S - WARMUP_HOLD_S)).clamp(0.0, 1.0);
            target = target.min(WARMUP_CAP + (1.0 - WARMUP_CAP) * open);
        }
        let tau = if target > self.value { VALUE_ATTACK_TAU_S } else { VALUE_RELEASE_TAU_S };
        self.value += (target - self.value) * ease(dt, tau);
        self.value = self.value.clamp(0.0, 1.0);
        self.value
    }

    /// The current reading, 0..1.
    pub fn value(&self) -> f32 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 30.0;

    /// Raw band levels for a passage, as amplitudes like the analyzer's.
    #[derive(Clone, Copy)]
    struct Mix {
        bass: f32,
        mid: f32,
        high: f32,
    }

    const FULL: Mix = Mix { bass: 0.0100, mid: 0.0030, high: 0.00040 };
    /// Pads and a vocal: no kick, no bassline, hardly any tops.
    const BREAKDOWN: Mix = Mix { bass: 0.0008, mid: 0.0022, high: 0.00010 };
    /// A mix-in intro: the kick at full weight, hats, nothing else.
    const DRUMS_ONLY: Mix = Mix { bass: 0.0072, mid: 0.0009, high: 0.00024 };
    /// The DJ has the low EQ cut during a blend.
    const BASS_CUT: Mix = Mix { bass: 0.0015, mid: 0.0028, high: 0.00038 };

    fn run(meter: &mut DynamicsMeter, mix: Mix, secs: f32) -> f32 {
        let frames = (secs / DT) as usize;
        let mut value = meter.value();
        for i in 0..frames {
            // A four-on-the-floor pump on the low band: full on the beat,
            // down to 55% between kicks (122 BPM).
            let beat = (i as f32 * DT * 122.0 / 60.0).fract();
            let pump = 1.0 - 0.45 * beat;
            let (b, m, h) = (mix.bass * pump, mix.mid, mix.high);
            value = meter.update(DT, b + m + h, b, h);
        }
        value
    }

    fn warmed() -> DynamicsMeter {
        let mut meter = DynamicsMeter::new();
        run(&mut meter, FULL, 90.0);
        meter
    }

    #[test]
    fn a_full_passage_reads_as_full() {
        let meter = warmed();
        assert!(meter.value() > 0.95, "full groove read {}", meter.value());
    }

    #[test]
    fn a_breakdown_reads_low_and_stays_low_however_long_it_lasts() {
        let mut meter = warmed();
        let early = run(&mut meter, BREAKDOWN, 8.0);
        assert!(early < 0.35, "8 s into a breakdown read {early}");
        // The adaptive energy has drifted back to "medium" by now. This must not.
        let late = run(&mut meter, BREAKDOWN, 82.0);
        assert!(late < 0.40, "90 s into a breakdown read {late}");
    }

    #[test]
    fn the_drop_lands_within_a_beat() {
        let mut meter = warmed();
        run(&mut meter, BREAKDOWN, 32.0);
        let after = run(&mut meter, FULL, 0.5);
        assert!(after > 0.80, "half a second after the drop read {after}");
    }

    #[test]
    fn stripped_passages_sit_between_a_breakdown_and_the_full_track() {
        let mut meter = warmed();
        let drums = run(&mut meter, DRUMS_ONLY, 20.0);
        assert!((0.50..0.85).contains(&drums), "drums-only read {drums}");
        let mut meter = warmed();
        let cut = run(&mut meter, BASS_CUT, 20.0);
        assert!((0.30..0.65).contains(&cut), "bass-cut read {cut}");
        let mut meter = warmed();
        let breakdown = run(&mut meter, BREAKDOWN, 20.0);
        assert!(breakdown < cut && cut < drums, "{breakdown} < {cut} < {drums}");
    }

    #[test]
    fn silence_reads_zero_and_does_not_move_the_reference() {
        let mut meter = warmed();
        for _ in 0..(120.0 / DT) as usize {
            meter.update(DT, 0.0, 0.0, 0.0);
        }
        assert!(meter.value() < 0.01, "silence read {}", meter.value());
        // Two minutes of nothing, then the same breakdown: still a breakdown.
        let after = run(&mut meter, BREAKDOWN, 10.0);
        assert!(after < 0.40, "a breakdown after a long gap read {after}");
    }

    #[test]
    fn a_quieter_record_becomes_the_new_full_scale_in_time() {
        let mut meter = warmed();
        let quieter = Mix { bass: FULL.bass * 0.6, mid: FULL.mid * 0.6, high: FULL.high * 0.6 };
        let at_first = run(&mut meter, quieter, 20.0);
        assert!(at_first < 0.85, "4.4 dB down read {at_first} straight away");
        let later = run(&mut meter, quieter, 600.0);
        assert!(later > 0.90, "ten minutes on it still read {later}");
    }

    #[test]
    fn the_first_seconds_of_a_session_are_held_at_a_groove() {
        let mut meter = DynamicsMeter::new();
        let early = run(&mut meter, DRUMS_ONLY, 10.0);
        assert!(early <= WARMUP_CAP + 0.01, "10 s in read {early}");
        let later = run(&mut meter, DRUMS_ONLY, 60.0);
        assert!(later > 0.95, "with nothing louder heard it should open up: {later}");
    }

    #[test]
    fn one_loud_hit_does_not_reset_the_scale() {
        let mut meter = warmed();
        let before = meter.value();
        // A crash three times the level, for a tenth of a second.
        for _ in 0..3 {
            meter.update(DT, 0.045, 0.030, 0.0012);
        }
        let after = run(&mut meter, FULL, 6.0);
        assert!(after > before - 0.12, "a single hit dropped full from {before} to {after}");
    }
}

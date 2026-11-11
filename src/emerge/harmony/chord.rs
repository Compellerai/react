//! EALP B5 (all-in) — lightweight real-time triad detection + neo-Riemannian
//! move classification from the 12-bin chroma.
//!
//! The chroma-centroid (osc/tonnetz.rs) already captures harmonic POSITION and
//! MOTION — including quality changes, which spike the centroid delta (a root-
//! fixed C-maj->C-min moves both tonal circles ~0.77). So this module deliberately
//! does NOT try to re-detect "a harmonic change happened." It adds the two things
//! the centroid cannot: an explicit chord QUALITY LABEL (major / minor / dominant
//! / diminished) for colour temperature, and a neo-Riemannian P/R/L MOVE TYPE for
//! accent character (the expressive parsimonious moves — Parallel, Relative,
//! Leading-tone). General harmonic motion intensity stays the centroid's job.
//!
//! Detection is cosine template matching over the chroma (level-invariant), gated
//! by a confidence floor (a flat/atonal chroma scores ~0.5 against every template
//! -> None) and a switch hysteresis (a candidate must persist N frames before it
//! commits) so the label is stable, never jittery. No key signature, no inversion
//! handling, no 7th-vs-triad-quality beyond the dominant 7 — lighting needs root +
//! quality + move, not a full harmonic analysis.

/// Chord quality the detector distinguishes. `as_u8` is the stamp encoding
/// (0 reserved for "no chord").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChordQuality {
    Major,
    Minor,
    Dominant,
    Diminished,
}

impl ChordQuality {
    pub fn as_u8(self) -> u8 {
        match self {
            ChordQuality::Major => 1,
            ChordQuality::Minor => 2,
            ChordQuality::Dominant => 3,
            ChordQuality::Diminished => 4,
        }
    }
    /// Pitch-class offsets from the root for this quality's template.
    fn intervals(self) -> &'static [usize] {
        match self {
            ChordQuality::Major => &[0, 4, 7],
            ChordQuality::Minor => &[0, 3, 7],
            ChordQuality::Dominant => &[0, 4, 7, 10],
            ChordQuality::Diminished => &[0, 3, 6],
        }
    }
}

const QUALITIES: [ChordQuality; 4] = [
    ChordQuality::Major,
    ChordQuality::Minor,
    ChordQuality::Dominant,
    ChordQuality::Diminished,
];

/// A detected chord: root pitch class (0-11) + quality. No inversion/octave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub root: u8,
    pub quality: ChordQuality,
}

/// Neo-Riemannian move between two triads. Only the three parsimonious
/// quality-changing transforms are special-cased (they share two common tones and
/// read as chromatic-mediant colour shifts); every other change is `Other` and is
/// left to the centroid-delta surprise to express.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChordMove {
    #[default]
    None,
    Same,
    /// Parallel: same root, major<->minor (C-maj <-> C-min).
    Parallel,
    /// Relative: C-maj <-> A-min (a minor third down) and inverse.
    Relative,
    /// Leading-tone exchange: C-maj <-> E-min (a major third up) and inverse.
    LeadingTone,
    /// Any other change (diatonic root move, dom7/dim, distant leap).
    Other,
}

impl ChordMove {
    pub fn as_u8(self) -> u8 {
        match self {
            ChordMove::None | ChordMove::Same | ChordMove::Other => 0,
            ChordMove::Parallel => 1,
            ChordMove::Relative => 2,
            ChordMove::LeadingTone => 3,
        }
    }
    /// True for the parsimonious P/R/L transforms — the expressive moves that earn
    /// a dedicated accent (everything else defers to the centroid surprise).
    pub fn is_prl(self) -> bool {
        matches!(self, ChordMove::Parallel | ChordMove::Relative | ChordMove::LeadingTone)
    }
}

/// Classify the neo-Riemannian relationship between two triads (P/R/L), else Other.
/// Dominant/diminished involvement -> Other (P/R/L is defined on maj/min triads).
pub fn classify_move(prev: Chord, cur: Chord) -> ChordMove {
    if prev == cur {
        return ChordMove::Same;
    }
    use ChordQuality::{Major, Minor};
    let d = ((cur.root as i32 - prev.root as i32).rem_euclid(12)) as usize;
    match (prev.quality, cur.quality) {
        (Major, Minor) => match d {
            0 => ChordMove::Parallel,
            9 => ChordMove::Relative,
            4 => ChordMove::LeadingTone,
            _ => ChordMove::Other,
        },
        (Minor, Major) => match d {
            0 => ChordMove::Parallel,
            3 => ChordMove::Relative,
            8 => ChordMove::LeadingTone,
            _ => ChordMove::Other,
        },
        _ => ChordMove::Other,
    }
}

fn cosine(chroma: &[f32; 12], template: &[f32; 12]) -> f32 {
    let mut dot = 0.0;
    let mut na = 0.0;
    let mut nb = 0.0;
    for i in 0..12 {
        dot += chroma[i] * template[i];
        na += chroma[i] * chroma[i];
        nb += template[i] * template[i];
    }
    if na <= 1e-9 || nb <= 1e-9 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// Minimum cosine to accept a chord. A flat/atonal chroma scores ~0.5 against
/// every template, so this floor rejects "no real chord" frames.
const CONFIDENCE_MIN: f32 = 0.62;

/// Single-frame best-chord detection (no hysteresis). Returns None when no
/// template clears the confidence floor (atonal / silence / ambiguous).
pub fn detect(chroma: &[f32; 12]) -> Option<Chord> {
    let mut best: Option<(Chord, f32)> = None;
    for root in 0..12usize {
        for q in QUALITIES {
            let mut template = [0.0f32; 12];
            for &iv in q.intervals() {
                template[(root + iv) % 12] = 1.0;
            }
            let score = cosine(chroma, &template);
            if best.map(|(_, s)| score > s).unwrap_or(true) {
                best = Some((Chord { root: root as u8, quality: q }, score));
            }
        }
    }
    best.filter(|&(_, s)| s >= CONFIDENCE_MIN).map(|(c, _)| c)
}

/// What the tracker publishes each frame for the lighting seam.
#[derive(Clone, Copy, Debug, Default)]
pub struct ChordFrame {
    /// 0 = no chord, 1 = major, 2 = minor, 3 = dominant, 4 = diminished.
    pub quality: u8,
    /// Active (decaying) neo-Riemannian move: 0 none, 1 P, 2 R, 3 L.
    pub prl_move: u8,
    /// Accent envelope (0-1) for the P/R/L move, set to 1.0 on commit and decayed.
    pub prl_env: f32,
    /// Root pitch class (0-11) of the last committed chord. Held through a
    /// no-chord gap so consumers keep a stable anchor; only meaningful once a
    /// chord has been heard (`quality != 0` now or earlier).
    pub root: u8,
    /// Accent envelope (0-1) for ANY chord-to-chord change — set to 1.0 when
    /// a different chord commits and decayed. The P/R/L envelope above only
    /// fires on three special moves; this one fires on the everyday ones too
    /// (I→IV, V→I, ii→V …), which is most of what a progression is.
    pub change_env: f32,
    /// How far the root travelled on the circle of fifths at the last change
    /// (0-6): 0 = same root (quality change), 1 = a fifth/fourth, 2 = a whole
    /// step, 3 = a minor third, 4 = a major third, 5 = a semitone, 6 = the
    /// tritone. Harmonic distance, so a consumer can size its reaction.
    pub root_motion: u8,
    /// Direction of that travel: +1 sharpward (up a fifth), -1 flatward (up a
    /// fourth), 0 for no root move or the tritone (which has no side).
    pub root_dir: i8,
    /// Harmonic rhythm (0-1): how busy the chord changes are. ~0.15 for a
    /// change every four seconds, ~0.5 for one a second, 1.0 for two a second
    /// or faster; decays to 0 on a held chord.
    pub rhythm: f32,
}

/// Signed circle-of-fifths travel between two roots: `(distance 0-6,
/// direction)`. Up a fifth is +1, up a fourth is -1; the tritone is the far
/// side of the circle and has no direction.
pub fn fifths_travel(prev_root: u8, cur_root: u8) -> (u8, i8) {
    let semis = (cur_root as i32 - prev_root as i32).rem_euclid(12);
    let steps = (semis * 7).rem_euclid(12); // 0..11 sharpward steps
    match steps {
        0 => (0, 0),
        1..=5 => (steps as u8, 1),
        6 => (6, 0),
        _ => ((12 - steps) as u8, -1),
    }
}

/// Frames a candidate must persist before it commits (debounce ~130ms @ 47fps).
const SWITCH_FRAMES: u8 = 6;
/// Per-frame decay of the P/R/L accent envelope (~0.5s tail).
const PRL_DECAY: f32 = 0.92;
/// Per-frame decay of the any-change accent envelope (~0.7s tail) — a touch
/// longer than the P/R/L tail so a mover has time to travel on it.
const CHANGE_DECAY: f32 = 0.94;
/// Per-frame leak of the harmonic-rhythm accumulator (~3s memory at 60fps).
const RHYTHM_LEAK: f32 = 0.995;
/// Accumulator level that reads as "fully busy" (two changes a second).
const RHYTHM_FULL: f32 = 6.0;
/// Per-frame smoothing of the published rhythm so it glides between changes
/// instead of sawing with every commit.
const RHYTHM_SMOOTH: f32 = 0.03;

/// Stateful tracker: hysteresis-stable current chord + a decaying P/R/L accent
/// envelope. Lives per-thread (osc/thread), fed the slew-limited chroma each frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct ChordTracker {
    current: Option<Chord>,
    candidate: Option<Chord>,
    cand_count: u8,
    prl_move: ChordMove,
    prl_env: f32,
    /// The last chord that committed, kept across no-chord gaps so a change
    /// is measured chord-to-chord (C → silence → F still reads as C → F, and
    /// C → silence → C is no change at all).
    last_chord: Option<Chord>,
    change_env: f32,
    root_motion: u8,
    root_dir: i8,
    rhythm_acc: f32,
    rhythm: f32,
}

impl ChordTracker {
    pub fn update(&mut self, chroma: &[f32; 12]) -> ChordFrame {
        // Decay the move accent first; clear it once spent.
        self.prl_env *= PRL_DECAY;
        if self.prl_env < 0.01 {
            self.prl_env = 0.0;
            self.prl_move = ChordMove::None;
        }

        let det = detect(chroma);
        if det == self.candidate {
            self.cand_count = self.cand_count.saturating_add(1);
        } else {
            self.candidate = det;
            self.cand_count = 1;
        }

        // Any-change accent + harmonic rhythm decay every frame, like prl_env.
        self.change_env *= CHANGE_DECAY;
        if self.change_env < 0.01 {
            self.change_env = 0.0;
        }
        self.rhythm_acc *= RHYTHM_LEAK;

        if self.cand_count >= SWITCH_FRAMES && self.candidate != self.current {
            if let (Some(prev), Some(cur)) = (self.current, self.candidate) {
                let mv = classify_move(prev, cur);
                if mv.is_prl() {
                    self.prl_move = mv;
                    self.prl_env = 1.0;
                }
            }
            if let Some(cur) = self.candidate {
                // Measured against the last chord HEARD, not `current` — a
                // no-chord gap between two chords must not hide the change.
                if let Some(prev) = self.last_chord {
                    if prev != cur {
                        let (dist, dir) = fifths_travel(prev.root, cur.root);
                        self.root_motion = dist;
                        self.root_dir = dir;
                        self.change_env = 1.0;
                        self.rhythm_acc += 1.0;
                    }
                }
                self.last_chord = Some(cur);
            }
            self.current = self.candidate;
        }
        let rhythm_target = (self.rhythm_acc / RHYTHM_FULL).clamp(0.0, 1.0);
        self.rhythm += (rhythm_target - self.rhythm) * RHYTHM_SMOOTH;

        ChordFrame {
            quality: self.current.map(|c| c.quality.as_u8()).unwrap_or(0),
            prl_move: self.prl_move.as_u8(),
            prl_env: self.prl_env,
            root: self.last_chord.map(|c| c.root).unwrap_or(0),
            change_env: self.change_env,
            root_motion: self.root_motion,
            root_dir: self.root_dir,
            rhythm: self.rhythm,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triad(root: usize, q: ChordQuality) -> [f32; 12] {
        let mut c = [0.0; 12];
        for &iv in q.intervals() {
            c[(root + iv) % 12] = 1.0;
        }
        c
    }

    #[test]
    fn detects_clean_triads_by_root_and_quality() {
        assert_eq!(detect(&triad(0, ChordQuality::Major)), Some(Chord { root: 0, quality: ChordQuality::Major }));
        assert_eq!(detect(&triad(0, ChordQuality::Minor)), Some(Chord { root: 0, quality: ChordQuality::Minor }));
        assert_eq!(detect(&triad(0, ChordQuality::Dominant)), Some(Chord { root: 0, quality: ChordQuality::Dominant }));
        assert_eq!(detect(&triad(0, ChordQuality::Diminished)), Some(Chord { root: 0, quality: ChordQuality::Diminished }));
        // A real-world-ish noisy A minor still resolves to A minor.
        let mut a_min = triad(9, ChordQuality::Minor);
        a_min[0] = 0.2; // a little leakage
        assert_eq!(detect(&a_min).map(|c| (c.root, c.quality)), Some((9, ChordQuality::Minor)));
    }

    #[test]
    fn flat_or_empty_chroma_is_no_chord() {
        assert_eq!(detect(&[1.0; 12]), None, "uniform chroma = no chord");
        assert_eq!(detect(&[0.0; 12]), None, "silence = no chord");
    }

    #[test]
    fn classify_move_labels_the_three_prl_transforms() {
        let cmaj = Chord { root: 0, quality: ChordQuality::Major };
        assert_eq!(classify_move(cmaj, Chord { root: 0, quality: ChordQuality::Minor }), ChordMove::Parallel);
        assert_eq!(classify_move(cmaj, Chord { root: 9, quality: ChordQuality::Minor }), ChordMove::Relative);
        assert_eq!(classify_move(cmaj, Chord { root: 4, quality: ChordQuality::Minor }), ChordMove::LeadingTone);
        // Same-quality diatonic move + distant leaps are Other (centroid handles them).
        assert_eq!(classify_move(cmaj, Chord { root: 7, quality: ChordQuality::Major }), ChordMove::Other);
        assert_eq!(classify_move(cmaj, Chord { root: 6, quality: ChordQuality::Major }), ChordMove::Other);
        assert_eq!(classify_move(cmaj, cmaj), ChordMove::Same);
    }

    #[test]
    fn tracker_is_hysteresis_stable_and_fires_prl_envelope() {
        let mut t = ChordTracker::default();
        let cmaj = triad(0, ChordQuality::Major);
        // Commit C major after the debounce window (commit lands on the Nth frame).
        for _ in 0..SWITCH_FRAMES - 1 {
            t.update(&cmaj);
        }
        let f = t.update(&cmaj);
        assert_eq!(f.quality, ChordQuality::Major.as_u8(), "C major commits after debounce");
        // A single noisy frame must NOT switch the label (hysteresis).
        let noisy = triad(6, ChordQuality::Diminished);
        assert_eq!(t.update(&noisy).quality, ChordQuality::Major.as_u8(), "one frame can't flip it");
        // Re-settle on C major.
        for _ in 0..SWITCH_FRAMES {
            t.update(&cmaj);
        }
        // Hold C minor long enough -> commit, and the Parallel move fires its accent.
        let cmin = triad(0, ChordQuality::Minor);
        for _ in 0..SWITCH_FRAMES - 1 {
            t.update(&cmin);
        }
        let fired = t.update(&cmin);
        assert_eq!(fired.quality, ChordQuality::Minor.as_u8(), "switched to C minor");
        assert_eq!(fired.prl_move, ChordMove::Parallel.as_u8(), "C-maj->C-min is a Parallel move");
        assert!(fired.prl_env > 0.0, "the P/R/L accent envelope is live on commit");
        // The envelope decays back to zero when the harmony holds steady.
        let mut env = fired.prl_env;
        for _ in 0..80 {
            env = t.update(&cmin).prl_env;
        }
        assert_eq!(env, 0.0, "P/R/L accent relaxes when steady");
    }

    /// Hold `chroma` until the tracker commits it; returns the commit frame.
    fn commit(t: &mut ChordTracker, chroma: &[f32; 12]) -> ChordFrame {
        let mut f = ChordFrame::default();
        for _ in 0..SWITCH_FRAMES {
            f = t.update(chroma);
        }
        f
    }

    #[test]
    fn fifths_travel_measures_harmonic_distance_and_side() {
        assert_eq!(fifths_travel(0, 0), (0, 0), "same root");
        assert_eq!(fifths_travel(0, 7), (1, 1), "C→G is one step sharpward");
        assert_eq!(fifths_travel(0, 5), (1, -1), "C→F is one step flatward");
        assert_eq!(fifths_travel(0, 2), (2, 1), "C→D is two fifths up");
        assert_eq!(fifths_travel(0, 9), (3, 1), "C→A is three fifths up");
        assert_eq!(fifths_travel(0, 3), (3, -1), "C→Eb is three fifths down");
        assert_eq!(fifths_travel(0, 6), (6, 0), "the tritone has no side");
        assert_eq!(fifths_travel(11, 4), (1, -1), "wraps the octave: B→E");
    }

    #[test]
    fn everyday_chord_changes_fire_the_change_envelope_with_root_motion() {
        let mut t = ChordTracker::default();
        let first = commit(&mut t, &triad(0, ChordQuality::Major));
        assert_eq!(first.root, 0);
        assert_eq!(first.change_env, 0.0, "the first chord heard is not a change");

        // C → F: a plain diatonic move the P/R/L layer ignores entirely.
        let to_f = commit(&mut t, &triad(5, ChordQuality::Major));
        assert_eq!(to_f.root, 5, "root follows the progression");
        assert_eq!(to_f.prl_env, 0.0, "not a P/R/L move");
        assert!(to_f.change_env > 0.9, "but it IS a change: {}", to_f.change_env);
        assert_eq!((to_f.root_motion, to_f.root_dir), (1, -1), "one fifth flatward");

        // F → G: a whole step = two fifths sharpward.
        let to_g = commit(&mut t, &triad(7, ChordQuality::Major));
        assert_eq!((to_g.root_motion, to_g.root_dir), (2, 1));

        // Holding the chord lets the accent relax to exactly zero.
        let mut held = to_g;
        for _ in 0..120 {
            held = t.update(&triad(7, ChordQuality::Major));
        }
        assert_eq!(held.change_env, 0.0, "accent relaxes on a held chord");
        assert_eq!(held.root, 7, "root holds");
    }

    #[test]
    fn a_no_chord_gap_neither_hides_nor_invents_a_change() {
        let mut t = ChordTracker::default();
        commit(&mut t, &triad(0, ChordQuality::Major));
        // Drop to silence long enough to commit "no chord" and spend the env.
        let mut gap = ChordFrame::default();
        for _ in 0..120 {
            gap = t.update(&[0.0; 12]);
        }
        assert_eq!(gap.quality, 0, "no chord in the gap");
        assert_eq!(gap.root, 0, "root is held through the gap");
        // Same chord returns: nothing changed.
        let back = commit(&mut t, &triad(0, ChordQuality::Major));
        assert_eq!(back.change_env, 0.0, "C → silence → C is not a change");
        // A different chord after another gap still registers.
        for _ in 0..120 {
            t.update(&[0.0; 12]);
        }
        let moved = commit(&mut t, &triad(9, ChordQuality::Minor));
        assert!(moved.change_env > 0.9, "C → silence → Am is a change");
        assert_eq!((moved.root_motion, moved.root_dir), (3, 1));
    }

    #[test]
    fn harmonic_rhythm_rises_with_busy_changes_and_settles_on_a_held_chord() {
        let mut t = ChordTracker::default();
        let prog = [0usize, 5, 7, 9];
        // Slow: one change every ~4 s at 60 fps.
        let mut slow = 0.0;
        for bar in 0..8 {
            for _ in 0..240 {
                slow = t.update(&triad(prog[bar % 4], ChordQuality::Major)).rhythm;
            }
        }
        // Fast: one change every ~0.5 s.
        let mut t = ChordTracker::default();
        let mut fast = 0.0;
        for bar in 0..64 {
            for _ in 0..30 {
                fast = t.update(&triad(prog[bar % 4], ChordQuality::Major)).rhythm;
            }
        }
        assert!(slow < 0.3, "slow progression reads calm: {slow}");
        assert!(fast > 0.7, "busy progression reads busy: {fast}");
        // Park on one chord: the rhythm drains.
        let mut parked = fast;
        for _ in 0..1800 {
            parked = t.update(&triad(0, ChordQuality::Major)).rhythm;
        }
        assert!(parked < 0.05, "held chord drains the rhythm: {parked}");
    }
}

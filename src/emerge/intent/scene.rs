//! Scenes — the look the whole rig holds, and how one look becomes the next.
//!
//! Before this module the rig had a "look index" that stepped every two bars,
//! on every polarity flip and on every drop. Each step moved every fixture's
//! hue a third of the way round the wheel in a single frame, swapped the
//! panels' palette and pattern, and spun the beams' colour wheels. A look
//! lasted about four seconds and every change was a cut.
//!
//! A festival rig does the opposite. It holds one look — a palette of one or
//! two colours, a chase, a place for the beams — for a whole phrase, and it
//! changes look on purpose: a slow melt into the breakdown, a sweep across the
//! stage on the phrase, a flash of white on the drop.
//!
//! * A [`Scene`] is one such look: a [`Look`] (palette, levels and chase for
//!   the colour body and the beams) plus what the beams do with their wheels
//!   ([`BeamLook`]). The library is in [`scene`]; [`scene_for`] maps the
//!   [`Section`] of the music and the harmony to a scene.
//! * The [`SceneDirector`] is the only stateful part. It runs once per frame
//!   beside the movement director, decides when the scene changes and by which
//!   [`Transition`], and tracks everything that has to glide: the key colour,
//!   the polarity swap, the drop flash, the beams' move-in-black.
//! * It stamps a [`SceneFrame`] on the snapshot. Everything a fixture then does
//!   with it is a pure function of the frame, the clock and where the fixture
//!   stands on the stage ([`StagePlace`]) — the same contract as the movement
//!   frame, so the wire and the preview cannot drift.
//!
//! Nothing is ever cut except on the beat, by design. During a transition the
//! old and the new look are both rendered and crossfaded channel by channel,
//! like a console fading between two cues; a look arriving while another is
//! still fading in simply fades over it.
//!
//! The harmony still chooses the colours. The rig's primary colour is the
//! key's colour, held until the key has really moved and then glided; the
//! second colour is a musical interval away from it (a fifth, a third, the
//! tritone); each phrase steps the primary through the notes of the triad;
//! and a polarity flip exchanges the two colours. The small harmonic motions
//! (chord kicks, Tonnetz drift, the dissonance fan) ride on top unchanged.
//!
//! A default `SceneFrame` is undirected, and the intent layer then renders
//! exactly what it rendered before this module existed. Anything without a
//! director (Hue, WLED, tests, pre-init frames) is unchanged.

use std::f32::consts::TAU;

use super::fixture_brain::ArcState;
use super::movement::dynamics_lift;
use super::oscillator::{OscillatorFrameSnapshot, OscillatorInterval};
use crate::emerge::contracts::{FixturePatch, StageVisualKind};
use crate::hue::CueSnapshot;

// ── Intervals, as turns of hue ──────────────────────────────────────────────
// Hue is chromatic here, as it is for the pitch anchor: a semitone is a
// twelfth of a turn. A palette built from intervals stays in the harmony.

const SEMITONE: f32 = 1.0 / 12.0;
/// A whole tone: the neighbouring colour.
const TONE: f32 = 2.0 * SEMITONE;
const MINOR_THIRD: f32 = 3.0 * SEMITONE;
const MAJOR_THIRD: f32 = 4.0 * SEMITONE;
/// A fifth up is a fourth down: the same colour, written as the short way
/// round so a glide to it takes the short way too.
const FIFTH: f32 = -5.0 * SEMITONE;
/// The complement.
const TRITONE: f32 = 0.5;
const TRIAD: f32 = 1.0 / 3.0;

// ── The look ────────────────────────────────────────────────────────────────

/// What a scene asks of the colour body and of the beams' levels. Every field
/// is a plain number, so two looks can be crossfaded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    // Palette.
    /// The primary colour, as an offset from the key colour (turns).
    pub hue: f32,
    /// The second colour, as an offset from the primary (turns, signed).
    pub accent: f32,
    /// How much of the second colour is on stage: 0 = every fixture on the
    /// primary, 1 = alternate fixtures fully on the second colour.
    pub split: f32,
    /// Hue gradient from one end of the stage to the other (turns).
    pub spread: f32,
    /// Saturation multiplier on the colour body.
    pub saturation: f32,
    /// How far the colour body is bleached toward white (0..1).
    pub white: f32,
    /// Extra bleach as the build's tension fills (0..1 at a full bank).
    pub tension_white: f32,

    // Levels. Multipliers on what the music already gives each part.
    pub body: f32,
    pub beams: f32,
    pub effects: f32,

    // The chase: how the level is distributed over the stage and the bar.
    // Each depth is how far that pattern pulls a fixture down (0 = off).
    /// Everything together, hit on the beat and falling away.
    pub pulse: f32,
    /// A crest travelling across the stage.
    pub wave: f32,
    /// Rings opening from the centre (or closing on it, `dir` -1).
    pub burst: f32,
    /// Alternate fixtures swapping on and off.
    pub alternate: f32,
    /// Fixtures twinkling independently.
    pub sparkle: f32,
    /// The harmonic field's own bands and ports (see `field_cell`).
    pub field: f32,
    /// How white a twinkle is (0 = the palette colour, 1 = white).
    pub twinkle_white: f32,
    /// How much of the chase the beams join (0 = steady beams).
    pub beam_chase: f32,

    // The chase's shape.
    /// 0 = a broad swell, 1 = a narrow head with dark between.
    pub sharp: f32,
    /// Cycles per bar.
    pub rate: f32,
    /// Crests on stage at once.
    pub waves: f32,
    /// +1 runs low-to-high across the stage (and outward), -1 the other way.
    pub dir: f32,
    /// The chase speeds up as the build's tension fills.
    pub accelerates: bool,
}

impl Look {
    /// One colour, everything at the level the music gives it, nothing moving.
    pub const PLAIN: Look = Look {
        hue: 0.0,
        accent: FIFTH,
        split: 0.0,
        spread: 0.0,
        saturation: 1.0,
        white: 0.0,
        tension_white: 0.0,
        body: 1.0,
        beams: 1.0,
        effects: 1.0,
        pulse: 0.0,
        wave: 0.0,
        burst: 0.0,
        alternate: 0.0,
        sparkle: 0.0,
        field: 0.0,
        twinkle_white: 0.0,
        beam_chase: 0.0,
        sharp: 0.0,
        rate: 1.0,
        waves: 1.0,
        dir: 1.0,
        accelerates: false,
    };

    fn level(&self, part: Part) -> f32 {
        match part {
            Part::Body => self.body,
            Part::Beam => self.beams,
            Part::Effect => self.effects,
        }
    }
}

/// What the beams do with the parts of them that cannot fade: the colour
/// wheel, the gobo wheel, the prism. The director changes these only inside a
/// dip of the beams' dimmer (see [`SceneDirector`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BeamLook {
    /// Open white beams — the festival signature over a saturated stage.
    pub white: bool,
    /// Beam groups spell the triad in colour instead of sharing one gel.
    pub voices: bool,
    /// A gobo rather than the open beam.
    pub gobo: bool,
    pub prism: bool,
}

/// The named looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SceneKind {
    /// Silence: one colour, barely there.
    #[default]
    Ember,
    /// Intro: two neighbouring colours and a slow swell.
    Dawn,
    /// Breakdown: one deep colour, white shafts in the air.
    Cathedral,
    /// Breakdown: a slow gradient drifting across the stage.
    Aurora,
    /// Breakdown: the stage nearly dark, twinkling.
    Starfield,
    /// Groove: two colours a fifth apart on alternate fixtures, on the beat.
    Duotone,
    /// Groove: a wave rolling across the stage once a bar.
    Tide,
    /// Groove: the harmonic field's bands and ports drawn across the stage.
    Lattice,
    /// Groove: one colour pumping, white beams.
    Pulse,
    /// Minor groove: complementary colours swapping sides.
    Checker,
    /// Minor groove: a slow wave running the other way.
    Undertow,
    /// Build: a narrow chase that tightens and bleaches as tension fills.
    Riser,
    /// The last moment before the drop: dark, white sparks.
    Hold,
    /// The drop: white-hot, rings bursting from the centre.
    Supernova,
    /// Peak: complementary colours trading places on every beat.
    Mainstage,
    /// Peak: a fast narrow chase under white beams.
    Voltage,
    /// Peak: three colours across the stage, beams in the triad's colours.
    Prism,
    /// Peak: rings closing on the centre.
    Stampede,
}

impl SceneKind {
    pub fn name(self) -> &'static str {
        match self {
            SceneKind::Ember => "Ember",
            SceneKind::Dawn => "Dawn",
            SceneKind::Cathedral => "Cathedral",
            SceneKind::Aurora => "Aurora",
            SceneKind::Starfield => "Starfield",
            SceneKind::Duotone => "Duotone",
            SceneKind::Tide => "Tide",
            SceneKind::Lattice => "Lattice",
            SceneKind::Pulse => "Pulse",
            SceneKind::Checker => "Checker",
            SceneKind::Undertow => "Undertow",
            SceneKind::Riser => "Riser",
            SceneKind::Hold => "Hold",
            SceneKind::Supernova => "Supernova",
            SceneKind::Mainstage => "Mainstage",
            SceneKind::Voltage => "Voltage",
            SceneKind::Prism => "Prism",
            SceneKind::Stampede => "Stampede",
        }
    }
}

/// One look of the whole rig.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scene {
    pub kind: SceneKind,
    pub look: Look,
    pub beam: BeamLook,
}

impl Default for Scene {
    fn default() -> Self {
        scene(SceneKind::Ember)
    }
}

const COLOUR: BeamLook = BeamLook { white: false, voices: false, gobo: false, prism: false };
const VOICES: BeamLook = BeamLook { white: false, voices: true, gobo: false, prism: false };
const WHITE: BeamLook = BeamLook { white: true, voices: false, gobo: false, prism: false };

/// The library: each named look, written out.
pub fn scene(kind: SceneKind) -> Scene {
    let p = Look::PLAIN;
    let (look, beam) = match kind {
        SceneKind::Ember => (Look { body: 0.7, beams: 0.6, effects: 0.5, ..p }, COLOUR),
        SceneKind::Dawn => (
            Look {
                accent: TONE,
                split: 0.5,
                spread: 0.08,
                wave: 0.35,
                rate: 0.25,
                waves: 0.5,
                body: 0.9,
                beams: 0.8,
                effects: 0.8,
                ..p
            },
            COLOUR,
        ),
        SceneKind::Cathedral => (
            Look { saturation: 1.15, wave: 0.25, rate: 0.125, waves: 0.5, effects: 0.4, ..p },
            WHITE,
        ),
        SceneKind::Aurora => (
            Look {
                accent: TONE,
                spread: 0.25,
                wave: 0.30,
                rate: 0.25,
                waves: 0.75,
                dir: -1.0,
                beams: 0.8,
                effects: 0.7,
                ..p
            },
            BeamLook { gobo: true, ..VOICES },
        ),
        SceneKind::Starfield => (
            Look {
                saturation: 1.1,
                sparkle: 0.85,
                twinkle_white: 0.8,
                rate: 0.5,
                beams: 0.5,
                effects: 0.5,
                ..p
            },
            WHITE,
        ),
        SceneKind::Duotone => (Look { split: 1.0, pulse: 0.30, beam_chase: 0.5, ..p }, VOICES),
        SceneKind::Tide => (
            Look { accent: TONE, spread: 0.12, wave: 0.55, beam_chase: 0.6, ..p },
            COLOUR,
        ),
        SceneKind::Lattice => (
            Look { split: 0.6, field: 0.75, pulse: 0.15, ..p },
            BeamLook { gobo: true, ..VOICES },
        ),
        SceneKind::Pulse => (Look { pulse: 0.5, beam_chase: 0.4, ..p }, WHITE),
        SceneKind::Checker => (
            Look { accent: TRITONE, split: 1.0, alternate: 0.65, beam_chase: 0.7, ..p },
            COLOUR,
        ),
        SceneKind::Undertow => (
            Look {
                accent: -TONE,
                spread: -0.12,
                saturation: 1.1,
                wave: 0.6,
                rate: 0.5,
                waves: 1.5,
                dir: -1.0,
                beam_chase: 0.5,
                ..p
            },
            VOICES,
        ),
        SceneKind::Riser => (
            Look {
                tension_white: 0.6,
                wave: 0.75,
                sharp: 0.7,
                rate: 2.0,
                accelerates: true,
                beam_chase: 1.0,
                effects: 0.6,
                ..p
            },
            WHITE,
        ),
        SceneKind::Hold => (
            Look {
                saturation: 0.6,
                white: 0.2,
                body: 0.35,
                beams: 0.6,
                effects: 0.0,
                sparkle: 0.9,
                twinkle_white: 1.0,
                rate: 2.0,
                ..p
            },
            WHITE,
        ),
        SceneKind::Supernova => (
            Look {
                accent: TRITONE,
                split: 1.0,
                white: 0.2,
                burst: 0.6,
                sharp: 0.3,
                rate: 2.0,
                pulse: 0.25,
                beam_chase: 0.5,
                ..p
            },
            BeamLook { prism: true, ..WHITE },
        ),
        SceneKind::Mainstage => (
            Look {
                accent: TRITONE,
                split: 1.0,
                alternate: 0.6,
                rate: 2.0,
                pulse: 0.2,
                beam_chase: 1.0,
                ..p
            },
            WHITE,
        ),
        SceneKind::Voltage => (
            Look {
                white: 0.12,
                wave: 0.7,
                sharp: 0.8,
                rate: 2.0,
                sparkle: 0.25,
                twinkle_white: 1.0,
                beam_chase: 1.0,
                ..p
            },
            WHITE,
        ),
        SceneKind::Prism => (
            Look { accent: TRIAD, split: 1.0, spread: 0.33, wave: 0.4, beam_chase: 0.6, ..p },
            BeamLook { prism: true, ..VOICES },
        ),
        SceneKind::Stampede => (
            Look {
                split: 0.5,
                burst: 0.7,
                sharp: 0.5,
                rate: 2.0,
                dir: -1.0,
                pulse: 0.2,
                beam_chase: 0.8,
                ..p
            },
            WHITE,
        ),
    };
    Scene { kind, look, beam }
}

/// Where the music is, as far as the look of the rig goes.
///
/// Coarser and slower than the show brain's arc, on purpose. The arc is a
/// frame-by-frame reading: in a full groove it moves between peak, cooldown
/// and groove every few seconds, and its `Drop` means "at peak lately", not
/// the drop. A look that followed it would change every few seconds — the
/// thing scenes exist to stop. The director reads a section from the arc with
/// hysteresis (see `SceneDirector::read_section`), and takes the build and
/// the drop from the cue engine's own tension bank and drop impact.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Section {
    /// Before the music.
    #[default]
    Idle,
    /// A breakdown, an intro, a quiet passage: low on the set's own scale.
    Calm,
    Groove,
    /// Tension is filling toward a drop.
    Build,
    /// The last moments before it.
    Hold,
    /// The drop itself, while its impact lasts.
    Drop,
    /// Full weight, sustained.
    Peak,
}

/// Pick the scene for a section. Pure: the same section, tonality and step
/// always give the same scene.
///
/// `step` advances once per phrase. It walks each section's pool in order, so
/// a section that outlasts a phrase moves on to a different look, and it
/// steps the primary colour through the key's triad — root, fifth, third — so
/// each phrase is a new palette that is still in the harmony. `minor` is the
/// tonality, latched by the director so a field hovering on the boundary
/// cannot flip the choice every frame.
pub fn scene_for(section: Section, minor: bool, step: u32) -> Scene {
    use SceneKind::*;
    let pick = |pool: &[SceneKind]| pool[step as usize % pool.len()];
    let kind = match section {
        Section::Idle => Ember,
        Section::Calm => pick(&[Cathedral, Aurora, Starfield, Dawn]),
        Section::Groove if minor => pick(&[Checker, Undertow, Lattice, Duotone]),
        Section::Groove => pick(&[Duotone, Tide, Lattice, Pulse]),
        Section::Build => Riser,
        Section::Hold => Hold,
        Section::Drop => Supernova,
        Section::Peak => pick(&[Mainstage, Voltage, Prism, Stampede]),
    };
    let mut scene = scene(kind);
    scene.look.hue = match step % 3 {
        0 => 0.0,
        1 => FIFTH,
        _ if minor => MINOR_THIRD,
        _ => MAJOR_THIRD,
    };
    scene
}

// ── Transitions ─────────────────────────────────────────────────────────────

/// How a new look arrives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Transition {
    /// Every fixture together.
    Fade,
    /// The new look crosses the stage from one end (`dir` +1 or -1).
    Sweep { dir: f32 },
    /// The new look opens from the centre outwards.
    Bloom,
    /// At once, behind a flash of white. For the drop.
    Flash,
}

/// How much of the stage a sweep's edge covers: the new look fades in over
/// this fraction of the stage as the edge passes.
const SWEEP_EDGE: f32 = 0.35;

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    smooth((x - lo) / (hi - lo).max(1e-6))
}

impl Transition {
    /// How far in the new look is at stage position `u` (-1..1) when the
    /// transition is `progress` (0..1) of the way through.
    fn weight(self, progress: f32, u: f32) -> f32 {
        let p = progress.clamp(0.0, 1.0);
        let travel = |x: f32| smooth((p * (1.0 + SWEEP_EDGE) - x.clamp(0.0, 1.0)) / SWEEP_EDGE);
        match self {
            Transition::Fade | Transition::Flash => smooth(p),
            Transition::Sweep { dir } => travel(0.5 + 0.5 * u * dir.signum()),
            Transition::Bloom => travel(u.abs()),
        }
    }
}

/// Transition lengths, in beats.
const FADE_FLASH_BEATS: f32 = 0.25;
const FADE_HOLD_BEATS: f32 = 2.0;
const FADE_BUILD_BEATS: f32 = 4.0;
const FADE_RELEASE_BEATS: f32 = 2.0;
const FADE_PEAK_BEATS: f32 = 2.0;
const FADE_GROOVE_BEATS: f32 = 4.0;
const FADE_CALM_BEATS: f32 = 8.0;
const FADE_WAKE_BEATS: f32 = 2.0;

/// The transition from the look of section `from` to the look of section
/// `to`, and its length in beats. `step` alternates the direction of
/// successive sweeps.
fn transition_for(from: Section, to: Section, step: u32) -> (Transition, f32) {
    let side = if step % 2 == 0 { 1.0 } else { -1.0 };
    let build = |s: Section| matches!(s, Section::Build | Section::Hold);
    match to {
        Section::Drop => (Transition::Flash, FADE_FLASH_BEATS),
        Section::Hold => (Transition::Fade, FADE_HOLD_BEATS),
        Section::Build => (Transition::Fade, FADE_BUILD_BEATS),
        // A build that lets go without a drop.
        _ if build(from) => (Transition::Fade, FADE_RELEASE_BEATS),
        // The music starting should not be kept waiting.
        _ if from == Section::Idle => (Transition::Fade, FADE_WAKE_BEATS),
        // Out of the drop, and every third change at the peak, the new look
        // opens from the centre; otherwise it crosses the stage.
        Section::Peak if from == Section::Drop || step % 3 == 2 => (Transition::Bloom, FADE_PEAK_BEATS),
        Section::Peak => (Transition::Sweep { dir: side }, FADE_PEAK_BEATS),
        // Into a breakdown, and between its looks, a slow melt.
        Section::Calm | Section::Idle => (Transition::Fade, FADE_CALM_BEATS),
        // The groove's looks trade places across the stage; the music coming
        // back in opens from the centre.
        Section::Groove if from == Section::Groove => (Transition::Sweep { dir: side }, FADE_GROOVE_BEATS),
        Section::Groove if from == Section::Calm => (Transition::Bloom, FADE_GROOVE_BEATS),
        Section::Groove => (Transition::Fade, FADE_GROOVE_BEATS),
    }
}

// ── Where a fixture stands ──────────────────────────────────────────────────

/// A fixture's place on the stage, as the scene sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StagePlace {
    /// Position across the stage: -1 at one end, +1 at the other.
    pub u: f32,
    /// Index of this fixture's address among the addresses of its kind, in
    /// order across the stage. Alternate looks go by its parity.
    pub rank: usize,
    /// How many addresses of its kind there are.
    pub of: usize,
    /// How much of the stage this fixture's pixel cells cover, in `u`.
    pub span_u: f32,
    /// How many alternating stripes its cells are divided into: 1 on a rig
    /// with many colour fixtures, more when there are only one or two, so a
    /// two-colour look still shows both colours.
    pub stripes: usize,
    /// This fixture has pixel cells, each of which takes its own place in the
    /// chase. Its fixture-level intent then carries no chase of its own.
    pub celled: bool,
}

impl StagePlace {
    /// Centre stage, on its own — a fixture nothing is known about.
    pub const CENTRE: StagePlace =
        StagePlace { u: 0.0, rank: 0, of: 1, span_u: 0.0, stripes: 1, celled: false };

    /// The place of mover `ordinal` of `count` when no layout is known: the
    /// movers spread evenly across the stage in address order.
    pub fn for_mover(ordinal: usize, count: usize) -> Self {
        let u = if count > 1 {
            (ordinal.min(count - 1) as f32 / (count - 1) as f32) * 2.0 - 1.0
        } else {
            0.0
        };
        StagePlace { u, rank: ordinal, of: count.max(1), ..Self::CENTRE }
    }

    /// How far in from the nearest end of the stage this fixture is, counted
    /// in fixtures of its kind from the centre outwards: 0 is the centre (or
    /// the centre pair). On a rig of one or two it is simply the rank. Beam
    /// groups take their colour voice from it, so the triad is laid out
    /// symmetrically about the centre line.
    pub fn ring(&self) -> usize {
        let of = self.of.max(1);
        if of <= 2 {
            return self.rank.min(of - 1);
        }
        let rank = self.rank.min(of - 1);
        let from_end = rank.min(of - 1 - rank);
        (of + 1) / 2 - 1 - from_end
    }

    /// The stage position of pixel cell `cell` of `count`.
    pub fn cell_u(&self, cell: usize, count: usize) -> f32 {
        let frac = (cell as f32 + 0.5) / count.max(1) as f32;
        self.u + (frac - 0.5) * self.span_u
    }

    /// Which of the two colours cell `cell` of `count` takes (its parity).
    pub fn cell_rank(&self, cell: usize, count: usize) -> usize {
        let stripes = self.stripes.max(1);
        let stripe = (cell * stripes) / count.max(1);
        self.rank * stripes + stripe.min(stripes - 1)
    }
}

/// Where every address stands across the stage.
///
/// The chases are spatial — a wave crosses the stage, a sweep brings a new look
/// in from one side — so each fixture needs a position along one axis. That
/// axis is whichever one the rig is longest in, and positions are scaled to
/// -1..1 across it.
///
/// Positions are per DMX address, not per placed fixture: fixtures
/// daisy-chained on one address receive one value, so they are given one
/// place (the middle of where they stand) and the preview shows what the wire
/// carries.
pub struct StageLayout {
    /// `(universe, address, u, rank, is body)`, sorted by universe and address.
    places: Vec<(u16, u16, f32, usize, bool)>,
    /// How many addresses carry the colour body, and how many do not.
    body_units: usize,
    other_units: usize,
}

impl StageLayout {
    pub fn new<'a>(fixtures: impl Iterator<Item = &'a FixturePatch>) -> Self {
        // One entry per address: the mean position of the fixtures on it.
        let mut groups: Vec<(u16, u16, [f32; 3], f32, bool)> = Vec::new();
        for f in fixtures.filter(|f| f.screen.is_none()) {
            let pos = [f.position.x, f.position.y, f.position.z];
            if pos.iter().any(|c| !c.is_finite()) {
                continue;
            }
            let body = is_body(f);
            match groups.iter_mut().find(|g| g.0 == f.universe && g.1 == f.start_address) {
                Some(g) => {
                    for (sum, c) in g.2.iter_mut().zip(pos) {
                        *sum += c;
                    }
                    g.3 += 1.0;
                    g.4 |= body;
                }
                None => groups.push((f.universe, f.start_address, pos, 1.0, body)),
            }
        }
        for g in &mut groups {
            for c in &mut g.2 {
                *c /= g.3;
            }
        }
        // The axis the rig is longest in; x (across the stage) wins a tie.
        let extent = |axis: usize| {
            let lo = groups.iter().map(|g| g.2[axis]).fold(f32::INFINITY, f32::min);
            let hi = groups.iter().map(|g| g.2[axis]).fold(f32::NEG_INFINITY, f32::max);
            if lo.is_finite() && hi.is_finite() { (lo, hi) } else { (0.0, 0.0) }
        };
        let mut axis = 0;
        for candidate in [2, 1] {
            let (lo, hi) = extent(candidate);
            let (best_lo, best_hi) = extent(axis);
            if hi - lo > (best_hi - best_lo) * 1.05 {
                axis = candidate;
            }
        }
        let (lo, hi) = extent(axis);
        let (centre, half) = ((lo + hi) * 0.5, (hi - lo) * 0.5);
        let at = |g: &(u16, u16, [f32; 3], f32, bool)| {
            if half > 1e-3 { ((g.2[axis] - centre) / half).clamp(-1.0, 1.0) } else { 0.0 }
        };

        // Rank within each kind, in order across the stage (address breaks a
        // tie, so heads stacked at one spot still alternate).
        let mut order: Vec<usize> = (0..groups.len()).collect();
        order.sort_by(|&a, &b| {
            at(&groups[a])
                .partial_cmp(&at(&groups[b]))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(groups[a].1.cmp(&groups[b].1))
        });
        let mut ranks = vec![0usize; groups.len()];
        let (mut body_seen, mut other_seen) = (0usize, 0usize);
        for &i in &order {
            let seen = if groups[i].4 { &mut body_seen } else { &mut other_seen };
            ranks[i] = *seen;
            *seen += 1;
        }
        let mut places: Vec<(u16, u16, f32, usize, bool)> =
            groups.iter().enumerate().map(|(i, g)| (g.0, g.1, at(g), ranks[i], g.4)).collect();
        places.sort_unstable_by_key(|p| (p.0, p.1));
        Self { places, body_units: body_seen, other_units: other_seen }
    }

    /// This fixture's place. `cells` is how many pixel cells it has (0 or 1
    /// for a fixture without any).
    pub fn place(&self, fixture: &FixturePatch, cells: usize) -> StagePlace {
        let key = (fixture.universe, fixture.start_address);
        let Ok(i) = self.places.binary_search_by_key(&key, |p| (p.0, p.1)) else {
            return StagePlace::CENTRE;
        };
        let (_, _, u, rank, body) = self.places[i];
        let celled = cells >= 2;
        let units = if body { self.body_units } else { self.other_units }.max(1);
        // Each of `units` celled fixtures covers its share of the stage, and
        // its centre is drawn in by half a share, so the cells of neighbouring
        // fixtures tile the stage from one end to the other.
        let share = 2.0 / units as f32;
        StagePlace {
            u: if celled { u * (1.0 - share * 0.5) } else { u },
            rank,
            of: units,
            span_u: if celled { share } else { 0.0 },
            stripes: if !celled {
                1
            } else if units <= 2 {
                4
            } else if units <= 4 {
                2
            } else {
                1
            },
            celled,
        }
    }
}

/// The colour body: the fixtures that paint the stage rather than throw a beam.
pub(crate) fn is_body(fixture: &FixturePatch) -> bool {
    matches!(fixture.visual_kind, StageVisualKind::Wash | StageVisualKind::LedPanel)
}

/// Which part of the rig a fixture is, for the scene's levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Panels and washes: the colour body.
    Body,
    /// Beams.
    Beam,
    /// Effect projectors (GalaxyJet, moonflower).
    Effect,
}

/// What a look gives one fixture (or one pixel cell) this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shade {
    /// Level multiplier (0..~1).
    pub level: f32,
    /// The fixture's colour before the small harmonic motions (turns).
    pub hue: f32,
    /// Saturation multiplier.
    pub sat: f32,
    /// Bleach toward white (0..1).
    pub white: f32,
}

// ── The per-frame state ─────────────────────────────────────────────────────

/// How many looks can be on stage at once: the one fully in, and up to two
/// fading in over it.
const LEGS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Leg {
    scene: Scene,
    transition: Transition,
    /// 0..1 through the transition (linear; eased where it is read).
    progress: f32,
}

/// What the director decided this frame. Stamped on the `CueSnapshot`; read by
/// the intent layer for every fixture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneFrame {
    /// A director is running. `false` (the default) renders the rig exactly
    /// as it was before scenes existed.
    pub directed: bool,
    legs: [Leg; LEGS],
    count: usize,
    /// The key colour: the harmony's pitch anchor, held and glided (turns).
    pub key_hue: f32,
    /// 0..1 — how far a polarity flip has exchanged the two colours.
    pub swap: f32,
    /// 0..1 — the drop's flash of white, decaying.
    pub flash: f32,
    /// Extra chase cycles the build's tension has added so far (wrapped).
    pub riser: f32,
    /// The beams' wheels, as they are right now.
    pub beam: BeamLook,
    /// The colour the beams are on (turns), stepped only inside a dip.
    pub beam_hue: f32,
    /// The tonality the beams' third voice was chosen in (minor third or major).
    pub beam_minor: bool,
    /// The gobo the beams are on (0 = open).
    pub gobo_slot: u8,
    /// Beam dimmer multiplier: dips to zero while the wheels turn.
    pub beam_dip: f32,
}

impl Default for SceneFrame {
    fn default() -> Self {
        let leg = Leg { scene: scene(SceneKind::Ember), transition: Transition::Fade, progress: 1.0 };
        Self {
            directed: false,
            legs: [leg; LEGS],
            count: 1,
            key_hue: 0.0,
            swap: 0.0,
            flash: 0.0,
            riser: 0.0,
            beam: BeamLook::default(),
            beam_hue: 0.0,
            beam_minor: false,
            gobo_slot: 0,
            beam_dip: 1.0,
        }
    }
}

/// The darkest a field band leaves a cell (see `field_cell`).
const FIELD_CELL_FLOOR: f32 = 0.12;
const FIELD_ACTIVE_LIFT: f32 = 0.2;
/// How much of each half-cycle an alternating chase spends changing over.
const ALTERNATE_EDGE: f32 = 0.18;
/// Twinkle steps per beat at a chase rate of 1.
const TWINKLE_STEPS_PER_BEAT: f64 = 2.0;
/// The share of twinkling fixtures: a fixture is lit above this (0..1).
const TWINKLE_LO: f32 = 0.55;
const TWINKLE_HI: f32 = 0.90;
/// Tension-bank band over which a riser bleaches and accelerates.
const TENSION_LO: f32 = 0.25;
const TENSION_HI: f32 = 0.65;

/// A stable pseudo-random 0..1 for a fixture and a step of time.
fn hash01(seed: u32, step: i64) -> f32 {
    let mut h = (seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (step as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// A crest at phase 0: a broad swell at `sharp` 0, a narrow head at 1.
fn crest(phase: f32, sharp: f32) -> f32 {
    let c = 0.5 + 0.5 * (TAU * phase).cos();
    c.powf(1.0 + 7.0 * sharp.clamp(0.0, 1.0))
}

/// The harmonic field drawn across the stage, 0..1 at stage fraction `x`
/// (0..1): a bright band per symmetry order sweeping on the field's rotation
/// phase, with each quaternion port lighting its quarter of the stage and the
/// live port lifted. The field's own structure as a chase — the look the
/// pixel bars had on their own, now across the whole rig.
fn field_cell(snapshot: &CueSnapshot, x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let bands = snapshot.symmetry.clamp(1, 4) as f32;
    let band_phase = (x * bands - snapshot.phase).rem_euclid(1.0);
    let band = (1.0 - band_phase.min(1.0 - band_phase) * 2.0).clamp(0.0, 1.0);
    let region = ((x * 4.0) as usize).min(3);
    let port = snapshot.qc_ports[region].clamp(0.0, 1.0);
    let active = if region as u8 == snapshot.qc_active_port { FIELD_ACTIVE_LIFT } else { 0.0 };
    ((FIELD_CELL_FLOOR + (1.0 - FIELD_CELL_FLOOR) * band).max(port) + active).clamp(0.0, 1.0)
}

impl SceneFrame {
    /// The scene the rig is in, or heading into.
    pub fn kind(&self) -> SceneKind {
        self.legs[self.count.clamp(1, LEGS) - 1].scene.kind
    }

    /// A transition is under way.
    pub fn in_transition(&self) -> bool {
        self.count > 1
    }

    /// A frame holding `scene` with nothing in transition — for tests and
    /// for rendering a look on its own.
    pub fn holding(scene: Scene) -> Self {
        let leg = Leg { scene, transition: Transition::Fade, progress: 1.0 };
        Self { directed: true, legs: [leg; LEGS], count: 1, beam: scene.beam, ..Self::default() }
    }

    /// Crossfade whatever `of` returns for each look in play at stage position
    /// `u`. With nothing in transition this is `of` of the one look.
    pub fn blend<const N: usize>(&self, u: f32, mut of: impl FnMut(&Scene) -> [f32; N]) -> [f32; N] {
        let count = self.count.clamp(1, LEGS);
        let mut out = of(&self.legs[0].scene);
        for leg in &self.legs[1..count] {
            let w = leg.transition.weight(leg.progress, u);
            if w <= 0.0 {
                continue;
            }
            let next = of(&leg.scene);
            for (o, n) in out.iter_mut().zip(next) {
                *o += (n - *o) * w;
            }
        }
        out
    }

    /// What `scene` gives a fixture of `part` at stage position `u`.
    ///
    /// `rank` picks which of the two colours it takes and which half of an
    /// alternating chase it is in; `seed` makes its twinkle its own.
    #[allow(clippy::too_many_arguments)]
    pub fn shade(
        &self,
        scene: &Scene,
        snapshot: &CueSnapshot,
        osc: &OscillatorFrameSnapshot,
        part: Part,
        u: f32,
        rank: usize,
        seed: u32,
    ) -> Shade {
        let look = &scene.look;
        // The chase rides the same gate as every oscillator: still in silence.
        let live = osc.energy_env();
        let tension = smoothstep(TENSION_LO, TENSION_HI, snapshot.tension_bank);
        let riser = if look.accelerates { self.riser } else { 0.0 };
        let phase = (osc.cycle_phase(OscillatorInterval::Bar, look.rate, 0.0) + riser).fract();
        let x = u * 0.5 + 0.5;
        let dir = if look.dir < 0.0 { -1.0 } else { 1.0 };

        let mut env = 1.0f32;
        let mut pull = |depth: f32, lit: f32| env *= 1.0 - depth.clamp(0.0, 1.0) * live * (1.0 - lit);
        if look.pulse > 0.0 {
            let beat = osc.cycle_phase(OscillatorInterval::Beat, 1.0, 0.0);
            pull(look.pulse, (1.0 - beat) * (1.0 - beat));
        }
        if look.wave > 0.0 {
            pull(look.wave, crest(phase * dir - x * look.waves, look.sharp));
        }
        if look.burst > 0.0 {
            pull(look.burst, crest(phase * dir - u.abs() * look.waves, look.sharp));
        }
        if look.alternate > 0.0 {
            // Lit for one half of the cycle, dark for the other, the two halves
            // of the rig a half-cycle apart. The edges are short ramps rather
            // than cuts: an LED cut reads as a glitch, a ramp as a hit.
            let pos = (phase * 2.0 + (rank % 2) as f32).rem_euclid(2.0);
            let lit = smoothstep(0.0, ALTERNATE_EDGE, pos) * (1.0 - smoothstep(1.0, 1.0 + ALTERNATE_EDGE, pos));
            pull(look.alternate, lit);
        }
        let mut twinkle = 0.0;
        if look.sparkle > 0.0 {
            let steps = osc.beats() * TWINKLE_STEPS_PER_BEAT * look.rate as f64 + riser as f64 * 8.0;
            let step = steps.floor();
            let (a, b) = (hash01(seed, step as i64), hash01(seed, step as i64 + 1));
            twinkle = smoothstep(TWINKLE_LO, TWINKLE_HI, a + (b - a) * smooth((steps - step) as f32));
            pull(look.sparkle, twinkle);
        }
        if look.field > 0.0 {
            pull(look.field, field_cell(snapshot, x));
        }
        // The beams join as much of the chase as the look gives them; effect
        // projectors hold steady.
        let chase = match part {
            Part::Body => env,
            Part::Beam => 1.0 - (1.0 - env) * look.beam_chase.clamp(0.0, 1.0),
            Part::Effect => 1.0,
        };

        // Colour: alternate fixtures take the second colour, and a polarity
        // flip exchanges the two.
        let side = if rank % 2 == 1 { look.split.clamp(0.0, 1.0) } else { 0.0 };
        let swap = self.swap.clamp(0.0, 1.0);
        let second = side + swap - 2.0 * side * swap;
        let hue = self.key_hue + look.hue + look.accent * second + look.spread * u * 0.5;
        let white = look.white
            + look.tension_white * tension
            + look.twinkle_white * twinkle * look.sparkle.clamp(0.0, 1.0) * live;
        Shade {
            level: (look.level(part) * chase).max(0.0),
            hue: hue.rem_euclid(1.0),
            sat: look.saturation.clamp(0.0, 2.0),
            white: white.clamp(0.0, 1.0),
        }
    }

    /// The level multiplier for a fixture of `part`, crossfaded over the looks
    /// in play.
    pub fn level(
        &self,
        snapshot: &CueSnapshot,
        osc: &OscillatorFrameSnapshot,
        part: Part,
        place: &StagePlace,
        seed: u32,
    ) -> f32 {
        self.blend(place.u, |scene| {
            [self.shade(scene, snapshot, osc, part, place.u, place.rank, seed).level]
        })[0]
    }
}

// ── The director ────────────────────────────────────────────────────────────

/// Beats a look holds: a phrase in the groove, half that at the peak, where
/// the music turns over faster. The movement director counts the same way, so
/// the look and the figure change together.
const PHRASE_BEATS: f64 = 32.0;
const PEAK_PHRASE_BEATS: f64 = 16.0;

/// Reading the section. The peak and the calm are each a running vote on the
/// brain's arc, averaged over a few beats, with a gap between the level that
/// enters a section and the level that leaves it.
const PEAK_VOTE_BEATS: f32 = 8.0;
const PEAK_ENTER: f32 = 0.60;
const PEAK_LEAVE: f32 = 0.25;
const CALM_VOTE_BEATS: f32 = 4.0;
const CALM_ENTER: f32 = 0.65;
const CALM_LEAVE: f32 = 0.35;
/// The cue engine's tension bank: a build from here (it rests near zero in a
/// groove and tops out around 0.6-0.7 before a drop), the hold from here.
const BUILD_ENTER: f32 = 0.30;
const BUILD_LEAVE: f32 = 0.20;
const HOLD_ENTER: f32 = 0.55;
const HOLD_LEAVE: f32 = 0.45;
/// The cue engine's drop impact: the drop is its rising edge (the same edge
/// the look rotation used), and the drop's look lasts while it stays up.
const DROP_IMPACT_ON: f32 = 0.60;
const DROP_IMPACT_OFF: f32 = 0.30;
const DROP_MIN_BEATS: f32 = 8.0;
const DROP_MAX_BEATS: f32 = 32.0;
/// Beats a section lasts before the votes may move it on: four bars of
/// groove or peak, two of calm, so the music coming back is not kept waiting.
/// (The build, the hold and the drop come when the music brings them.)
const SECTION_MIN_BEATS: f32 = 16.0;
const CALM_MIN_BEATS: f32 = 8.0;
/// Beats a look stays before a change that can wait may replace it.
const MIN_HOLD_BEATS: f32 = 4.0;
/// A phrase boundary moves the rig on to the next look only if the look it
/// has is at least this much of a phrase old. A look that came in with a
/// change of section a few bars ago stays for the phrase that follows.
const PHRASE_MIN_SHARE: f32 = 0.5;

/// Tonality latch on `major_smooth` (0.5 = ambiguous): minor below, major
/// above, unchanged in between.
const MINOR_BELOW: f32 = 0.42;
const MAJOR_ABOVE: f32 = 0.50;

/// Beats the dominant pitch has to hold before the key colour follows it: a
/// bar, so a passing chord does not move the palette.
const KEY_COMMIT_BEATS: f32 = 4.0;
/// How quickly the key colour glides to a new key (seconds): unhurried in a
/// settled passage, brisk at full weight.
const KEY_GLIDE_SETTLED_S: f32 = 2.5;
const KEY_GLIDE_OPEN_S: f32 = 0.8;

/// A polarity flip exchanges the two colours over about this long (seconds
/// at full weight / settled), and a flip sooner than the hold after the last
/// one is ignored.
const SWAP_TAU_OPEN_S: f32 = 0.4;
const SWAP_TAU_SETTLED_S: f32 = 1.5;
const SWAP_HOLD_OPEN_S: f32 = 2.0;
const SWAP_HOLD_SETTLED_S: f32 = 8.0;

/// The drop's flash: how fast it falls away, and the shortest gap between two.
const FLASH_TAU_S: f32 = 0.45;
const FLASH_MIN_GAP_S: f32 = 4.0;

/// The beams' move-in-black. A colour or gobo wheel cannot fade, and takes a
/// moment to travel, so the dimmer goes down, stays down while the wheels
/// turn, and comes back: `DIP_S` in all, dark from `DIP_DARK_FROM` to
/// `DIP_DARK_TO` of the way through (about a quarter of a second), the wheels
/// sent on their way at `DIP_TURN_AT`, once the dimmer has had time to close.
/// The wheels are then left alone for at least `BEAM_HOLD_BEATS`.
const DIP_S: f32 = 0.60;
const DIP_DARK_FROM: f32 = 0.20;
const DIP_TURN_AT: f32 = 0.30;
const DIP_DARK_TO: f32 = 0.70;
const BEAM_HOLD_BEATS: f32 = 8.0;
/// Gobo slots the rig cycles through (slot 0 is the open beam).
const GOBO_SLOTS: u32 = 7;

fn ease(dt: f32, tau: f32) -> f32 {
    1.0 - (-dt.max(0.0) / tau.max(1e-3)).exp()
}

/// The signed shortest way round the hue circle from `from` to `to` (turns).
fn hue_delta(from: f32, to: f32) -> f32 {
    (to - from + 0.5).rem_euclid(1.0) - 0.5
}

/// What the beams' wheels should be on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BeamTarget {
    look: BeamLook,
    /// Colour, in semitones round the wheel.
    semitone: u8,
    /// The third voice is the minor third.
    minor: bool,
    gobo: u8,
}

/// The beam dimmer `progress` (0..1) of the way through a dip.
fn dip_level(progress: f32) -> f32 {
    if progress >= 1.0 {
        1.0
    } else if progress < DIP_DARK_FROM {
        1.0 - smooth(progress / DIP_DARK_FROM)
    } else if progress < DIP_DARK_TO {
        0.0
    } else {
        smooth((progress - DIP_DARK_TO) / (1.0 - DIP_DARK_TO))
    }
}

/// Chooses the scene, runs the transitions and tracks everything that glides.
/// Hold one for the life of the lighting session and `tick` it once per frame.
#[derive(Clone, Copy, Debug)]
pub struct SceneDirector {
    legs: [Leg; LEGS],
    count: usize,
    /// Seconds each transition takes, alongside `legs`.
    fade_s: [f32; LEGS],
    primed: bool,

    section: Section,
    /// Beats since the section last changed.
    section_beats: f32,
    /// The arc agrees with the section and no vote is on its way across: a
    /// phrase boundary may move the look on. When the music is in the middle
    /// of changing, the look waits for the change instead of getting in
    /// ahead of it.
    secure: bool,
    peak_vote: f32,
    calm_vote: f32,
    drop_armed: bool,
    /// Beats since the drop, while its look is on.
    drop_beats: f32,
    phrase: i64,
    step: u32,
    /// The tonality, latched; and the tonality the current look was chosen
    /// in, which only moves when the look does.
    minor: bool,
    look_minor: bool,
    /// Seconds since the last scene change began.
    since_change_s: f32,

    key_hue: f32,
    key_target: f32,
    key_candidate: u8,
    key_candidate_s: f32,

    swap: f32,
    swapped: bool,
    polarity: u8,
    since_swap_s: f32,

    flash: f32,
    since_flash_s: f32,

    riser: f32,
    last_beat: f64,

    beam: BeamTarget,
    beam_want: BeamTarget,
    /// 0..1 through a dip, or 1 when none is running.
    dip: f32,
    dip_turned: bool,
    since_beam_s: f32,
}

impl Default for SceneDirector {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneDirector {
    pub fn new() -> Self {
        let leg = Leg { scene: scene(SceneKind::Ember), transition: Transition::Fade, progress: 1.0 };
        let beam = BeamTarget { look: leg.scene.beam, semitone: 0, minor: false, gobo: 0 };
        Self {
            legs: [leg; LEGS],
            count: 1,
            fade_s: [1.0; LEGS],
            primed: false,
            section: Section::Idle,
            section_beats: 0.0,
            secure: false,
            peak_vote: 0.0,
            calm_vote: 0.5,
            drop_armed: true,
            drop_beats: 0.0,
            phrase: 0,
            step: 0,
            minor: false,
            look_minor: false,
            since_change_s: 0.0,
            key_hue: 0.0,
            key_target: 0.0,
            key_candidate: 0,
            key_candidate_s: 0.0,
            swap: 0.0,
            swapped: false,
            polarity: 0,
            since_swap_s: 0.0,
            flash: 0.0,
            since_flash_s: FLASH_MIN_GAP_S,
            riser: 0.0,
            last_beat: 0.0,
            beam,
            beam_want: beam,
            dip: 1.0,
            dip_turned: true,
            since_beam_s: 0.0,
        }
    }

    /// The scene the rig is in, or heading into.
    pub fn kind(&self) -> SceneKind {
        self.top().scene.kind
    }

    /// The section the director reads the music as being in.
    pub fn section(&self) -> Section {
        self.section
    }

    fn top(&self) -> &Leg {
        &self.legs[self.count.clamp(1, LEGS) - 1]
    }

    /// Start `scene` fading in over what is on stage. Returns false when two
    /// transitions are already running and this one is not `urgent`.
    fn push(&mut self, scene: Scene, transition: Transition, fade_s: f32, urgent: bool) -> bool {
        if self.count >= LEGS {
            if !urgent {
                return false;
            }
            // Three looks in play and something that cannot wait: finish the
            // oldest transition now.
            self.legs.copy_within(1..LEGS, 0);
            self.fade_s.copy_within(1..LEGS, 0);
            self.count -= 1;
            self.legs[0].progress = 1.0;
        }
        self.legs[self.count] = Leg { scene, transition, progress: 0.0 };
        self.fade_s[self.count] = fade_s.max(1e-3);
        self.count += 1;
        self.since_change_s = 0.0;
        true
    }

    /// Read the section from the brain's arc and the cue engine's build and
    /// drop signals. `beats` is how many beats this frame lasted. Returns the
    /// section and whether this frame is the drop.
    fn read_section(&mut self, snapshot: &CueSnapshot, arc: ArcState, beats: f32) -> (Section, bool) {
        let lift = dynamics_lift(snapshot);
        let peak_arc = matches!(arc, ArcState::Drop | ArcState::SustainPeak);

        // The votes run every frame, whatever section is showing.
        let peak = if peak_arc { 1.0 } else { 0.0 };
        self.peak_vote += (peak - self.peak_vote) * ease(beats, PEAK_VOTE_BEATS);
        let calm = match arc {
            // Silence here is also "nothing, on the scale of what came
            // before": the first seconds after a big fall.
            ArcState::Silence | ArcState::Breakdown => 1.0,
            // The bars after a peak: calm only if the music really fell away.
            ArcState::Cooldown => 1.0 - lift,
            ArcState::Groove | ArcState::Drop | ArcState::SustainPeak => 0.0,
            // No opinion: an intro or a build is whatever it was already.
            ArcState::Intro | ArcState::Buildup | ArcState::PreDropTension => self.calm_vote,
        };
        self.calm_vote += (calm - self.calm_vote) * ease(beats, CALM_VOTE_BEATS);
        self.secure = match self.section {
            Section::Calm => calm >= 0.5 && self.calm_vote >= CALM_ENTER && self.peak_vote <= PEAK_LEAVE,
            Section::Groove => {
                !peak_arc && calm < 0.5 && self.calm_vote <= CALM_LEAVE && self.peak_vote <= PEAK_LEAVE
            }
            Section::Peak => peak_arc && self.peak_vote >= PEAK_ENTER,
            // A build keeps its palette to the drop, and the drop to its end.
            Section::Idle | Section::Build | Section::Hold | Section::Drop => false,
        };

        // The drop: the rising edge of the cue engine's drop impact.
        let impact = snapshot.drop_impact;
        let dropped = self.primed && impact > DROP_IMPACT_ON && self.drop_armed;
        if impact > DROP_IMPACT_ON {
            self.drop_armed = false;
        } else if impact < DROP_IMPACT_OFF {
            self.drop_armed = true;
        }
        if dropped {
            self.drop_beats = 0.0;
            // What follows a drop is a peak until the music says otherwise.
            self.peak_vote = 1.0;
            return (Section::Drop, true);
        }
        if self.section == Section::Drop {
            self.drop_beats += beats;
            let lasting = impact > DROP_IMPACT_OFF && self.drop_beats < DROP_MAX_BEATS;
            if lasting || self.drop_beats < DROP_MIN_BEATS {
                return (Section::Drop, false);
            }
        }

        // The build, from the tension bank (or the brain calling it).
        let bank = snapshot.tension_bank;
        let in_build = matches!(self.section, Section::Build | Section::Hold);
        let building = bank > if in_build { BUILD_LEAVE } else { BUILD_ENTER }
            || matches!(arc, ArcState::Buildup | ArcState::PreDropTension);
        if building {
            let holding = bank > if self.section == Section::Hold { HOLD_LEAVE } else { HOLD_ENTER }
                || arc == ArcState::PreDropTension;
            return (if holding { Section::Hold } else { Section::Build }, false);
        }

        // Otherwise the votes, each with its own way in and way out, and a
        // section that has only just begun is given its time.
        let was = self.section;
        let dwell = match was {
            Section::Groove | Section::Peak => SECTION_MIN_BEATS,
            Section::Calm => CALM_MIN_BEATS,
            _ => 0.0,
        };
        if self.section_beats < dwell {
            return (was, false);
        }
        let calm = self.calm_vote > if was == Section::Calm { CALM_LEAVE } else { CALM_ENTER }
            || (was == Section::Idle && self.calm_vote > 0.5);
        let peak = self.peak_vote > if was == Section::Peak { PEAK_LEAVE } else { PEAK_ENTER }
            // The music slamming in out of a quiet passage is a peak now, not
            // a groove for a bar on the way to one.
            || (was == Section::Calm && !calm && peak_arc);
        let section = if peak {
            if was != Section::Peak {
                // Entered by the short cut above: let it leave like any other.
                self.peak_vote = self.peak_vote.max(PEAK_ENTER);
            }
            Section::Peak
        } else if calm {
            Section::Calm
        } else {
            Section::Groove
        };
        (section, false)
    }

    /// Advance one frame. `beat` and `tempo_bpm` are the resolved show clock,
    /// `dt` the real frame time in seconds.
    pub fn tick(
        &mut self,
        snapshot: &CueSnapshot,
        arc: ArcState,
        beat: f64,
        tempo_bpm: f32,
        dt: f32,
    ) -> SceneFrame {
        let dt = dt.clamp(0.0, 0.1);
        let beat_s = if tempo_bpm > 1.0 { 60.0 / tempo_bpm } else { 0.5 };
        let lift = dynamics_lift(snapshot);

        // ── Transitions in flight.
        for i in 1..self.count {
            self.legs[i].progress = (self.legs[i].progress + dt / self.fade_s[i]).min(1.0);
        }
        // A look that is fully in hides everything beneath it, including a
        // slower transition it overtook.
        if let Some(top) = (1..self.count).rev().find(|&i| self.legs[i].progress >= 1.0) {
            self.legs.copy_within(top..self.count, 0);
            self.fade_s.copy_within(top..self.count, 0);
            self.count -= top;
        }
        self.since_change_s += dt;

        // ── The section.
        let previous = self.section;
        // (On the first frame the votes go straight to where the music is.)
        let frame_beats = if self.primed { dt / beat_s } else { 1.0e3 };
        let (section, dropped) = self.read_section(snapshot, arc, frame_beats);
        self.section = section;
        let section_changed = section != previous;
        self.section_beats = if section_changed { 0.0 } else { self.section_beats + frame_beats.min(1.0) };

        // ── Tonality, latched.
        if snapshot.major_smooth < MINOR_BELOW {
            self.minor = true;
        } else if snapshot.major_smooth > MAJOR_ABOVE {
            self.minor = false;
        }

        // ── The phrase: every boundary moves the rig on to the next look and
        // the next colour of the triad. A new section starts on a fresh one.
        let peaking = matches!(section, Section::Drop | Section::Peak);
        let phrase_beats = if peaking { PEAK_PHRASE_BEATS } else { PHRASE_BEATS };
        let phrase = (beat.max(0.0) / phrase_beats).floor() as i64;
        let look_beats = self.since_change_s / beat_s;
        let boundary =
            phrase != self.phrase && self.secure && look_beats >= PHRASE_MIN_SHARE * phrase_beats as f32;
        if self.primed && (boundary || section_changed) {
            self.step = self.step.wrapping_add(1);
            self.look_minor = self.minor;
        } else if !self.primed {
            self.look_minor = self.minor;
        }
        self.phrase = phrase;

        // ── The scene.
        let want = scene_for(section, self.look_minor, self.step);
        if !self.primed {
            self.legs = [Leg { scene: want, transition: Transition::Fade, progress: 1.0 }; LEGS];
            self.count = 1;
        } else if want != self.top().scene {
            // The build, the hold and the drop arrive when the music does;
            // everything else waits its turn.
            let build = |s: Section| matches!(s, Section::Build | Section::Hold);
            let urgent = dropped || (section_changed && (build(section) || build(previous)));
            let held = self.since_change_s >= MIN_HOLD_BEATS * beat_s;
            if urgent || held {
                let (transition, beats) = transition_for(previous, section, self.step);
                self.push(want, transition, beats * beat_s, urgent);
            }
        }

        // ── The drop's flash, sized by how big the passage really is.
        self.since_flash_s += dt;
        self.flash *= (-dt / FLASH_TAU_S).exp();
        if self.flash < 0.004 {
            self.flash = 0.0;
        }
        if dropped && self.since_flash_s >= FLASH_MIN_GAP_S {
            self.flash = lift;
            self.since_flash_s = 0.0;
        }

        // ── The key colour. The dominant pitch has to hold before the palette
        // follows it; the all-notes blend is already smooth and is followed
        // directly. Either way the colour glides the short way round.
        if snapshot.all_notes_color {
            self.key_target = snapshot.all_notes_hue.rem_euclid(1.0);
        } else {
            let pc = snapshot.dominant_pc % 12;
            if pc != self.key_candidate {
                self.key_candidate = pc;
                self.key_candidate_s = 0.0;
            }
            self.key_candidate_s += dt;
            if !self.primed || self.key_candidate_s >= KEY_COMMIT_BEATS * beat_s {
                self.key_target = pc as f32 * SEMITONE;
            }
        }
        if self.primed {
            let tau = KEY_GLIDE_SETTLED_S + (KEY_GLIDE_OPEN_S - KEY_GLIDE_SETTLED_S) * lift;
            self.key_hue =
                (self.key_hue + hue_delta(self.key_hue, self.key_target) * ease(dt, tau)).rem_euclid(1.0);
        } else {
            self.key_hue = self.key_target;
        }

        // ── Polarity: the harmony changing sides exchanges the two colours.
        self.since_swap_s += dt;
        let flipped = self.primed && snapshot.polarity_quadrant != self.polarity;
        self.polarity = snapshot.polarity_quadrant;
        let hold = SWAP_HOLD_SETTLED_S + (SWAP_HOLD_OPEN_S - SWAP_HOLD_SETTLED_S) * lift;
        if flipped && self.since_swap_s >= hold {
            self.swapped = !self.swapped;
            self.since_swap_s = 0.0;
        }
        let swap_tau = SWAP_TAU_SETTLED_S + (SWAP_TAU_OPEN_S - SWAP_TAU_SETTLED_S) * lift;
        let swap_to = if self.swapped { 1.0 } else { 0.0 };
        self.swap += (swap_to - self.swap) * ease(dt, swap_tau);
        if (self.swap - swap_to).abs() < 0.002 {
            self.swap = swap_to;
        }

        // ── The riser: a build's tension adds chase cycles on top of the
        // clock's, so the chase accelerates without ever jumping.
        let beats = (beat - self.last_beat).clamp(0.0, 1.0) as f32;
        self.last_beat = beat;
        let extra = smoothstep(TENSION_LO, TENSION_HI, snapshot.tension_bank);
        self.riser = (self.riser + beats * 0.25 * 2.0 * extra).rem_euclid(1.0);

        // ── The beams' wheels. They cannot fade, so they turn inside a dip of
        // the dimmer; on the drop they turn under the flash instead.
        self.since_beam_s += dt;
        let look = self.top().scene.look;
        let second = if self.swapped { look.accent } else { 0.0 };
        let colour = (self.key_target + look.hue + second).rem_euclid(1.0);
        let top_beam = self.top().scene.beam;
        // White beams have no colour to keep up with: leave the wheel's
        // colour where it is rather than dip for a gel nobody sees.
        let coloured = !top_beam.white || !self.primed;
        self.beam_want = BeamTarget {
            look: top_beam,
            semitone: if coloured {
                ((colour * 12.0).round() as i64).rem_euclid(12) as u8
            } else {
                self.beam.semitone
            },
            minor: if coloured && top_beam.voices { self.minor } else { self.beam.minor },
            gobo: if top_beam.gobo { 1 + (self.step % GOBO_SLOTS) as u8 } else { 0 },
        };
        if !self.primed || dropped {
            self.beam = self.beam_want;
            self.dip = 1.0;
            self.dip_turned = true;
            self.since_beam_s = 0.0;
        } else if self.dip < 1.0 {
            self.dip = (self.dip + dt / DIP_S).min(1.0);
            // The wheels turn once the beams have gone dark.
            if self.dip >= DIP_TURN_AT && !self.dip_turned {
                self.beam = self.beam_want;
                self.dip_turned = true;
            }
        } else if self.beam_want != self.beam {
            // A change of scene turns the wheels at once; a change of key
            // inside a scene waits out the hold.
            let scene_changed = self.beam_want.look != self.beam.look || self.beam_want.gobo != self.beam.gobo;
            if scene_changed || self.since_beam_s >= BEAM_HOLD_BEATS * beat_s {
                self.dip = 0.0;
                self.dip_turned = false;
                self.since_beam_s = 0.0;
            }
        }

        self.primed = true;
        self.frame()
    }

    /// The current state as a frame, without advancing.
    pub fn frame(&self) -> SceneFrame {
        SceneFrame {
            directed: self.primed,
            legs: self.legs,
            count: self.count,
            key_hue: self.key_hue,
            swap: self.swap,
            flash: self.flash,
            riser: self.riser,
            beam: self.beam.look,
            beam_hue: self.beam.semitone as f32 * SEMITONE,
            beam_minor: self.beam.minor,
            gobo_slot: self.beam.gobo,
            beam_dip: dip_level(self.dip),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emerge::contracts::{ChannelMap, FixtureRole, StageVec3};
    use crate::emerge::intent::fixture_brain::{PhaseState, ShowClockSnapshot};

    const DT: f32 = 1.0 / 60.0;
    const BPM: f32 = 120.0;

    fn playing() -> CueSnapshot {
        CueSnapshot {
            energy: 0.8,
            energy_slow: 0.7,
            trust: 1.0,
            major_smooth: 0.7,
            ..CueSnapshot::default()
        }
    }

    fn osc_at(snapshot: &CueSnapshot, beat: f64) -> OscillatorFrameSnapshot {
        let clock = ShowClockSnapshot { tempo_bpm: BPM, beat, phase: beat.fract() as f32, quantum: 32.0, ..Default::default() };
        OscillatorFrameSnapshot::from_clock(snapshot, PhaseState::from_show_clock(clock))
    }

    /// Run `director` for `secs` from `beat`, returning the last frame and the
    /// beat reached.
    fn run(
        director: &mut SceneDirector,
        snapshot: &CueSnapshot,
        arc: ArcState,
        beat: &mut f64,
        secs: f32,
    ) -> SceneFrame {
        let mut frame = director.frame();
        for _ in 0..(secs / DT) as usize {
            *beat += (BPM / 60.0 * DT) as f64;
            frame = director.tick(snapshot, arc, *beat, BPM, DT);
        }
        frame
    }

    fn fixture_at(id: &str, address: u16, x: f32, kind: StageVisualKind) -> FixturePatch {
        FixturePatch {
            id: id.to_string(),
            name: id.to_string(),
            universe: 0,
            start_address: address,
            profile_id: String::new(),
            mode: String::new(),
            position: StageVec3 { x, y: 2.0, z: 0.0 },
            rotation: StageVec3::zero(),
            scale: StageVec3::one(),
            role: FixtureRole::Accent,
            visual_kind: kind,
            tags: vec![],
            channels: ChannelMap::default(),
            footprint: 0,
            screen: None,
        }
    }

    #[test]
    fn a_frame_without_a_director_is_undirected() {
        assert!(!SceneFrame::default().directed);
        assert!(!SceneDirector::new().frame().directed);
        let mut director = SceneDirector::new();
        assert!(director.tick(&playing(), ArcState::Groove, 1.0, BPM, DT).directed);
    }

    #[test]
    fn every_section_has_a_scene_and_each_phrase_a_new_palette() {
        let sections = [
            Section::Idle,
            Section::Calm,
            Section::Groove,
            Section::Build,
            Section::Hold,
            Section::Drop,
            Section::Peak,
        ];
        for section in sections {
            for minor in [false, true] {
                for step in 0..12 {
                    let now = scene_for(section, minor, step);
                    assert_eq!(now, scene_for(section, minor, step), "scene_for is pure");
                    let next = scene_for(section, minor, step + 1);
                    // The palette moves on every phrase even when a section
                    // has only one look to give.
                    assert_ne!(now.look.hue, next.look.hue, "{section:?} step {step}");
                }
            }
        }
        assert_eq!(scene_for(Section::Drop, false, 0).kind, SceneKind::Supernova);
        assert_eq!(scene_for(Section::Build, false, 0).kind, SceneKind::Riser);
        assert_eq!(scene_for(Section::Hold, true, 5).kind, SceneKind::Hold);
        assert_eq!(scene_for(Section::Groove, false, 0).kind, SceneKind::Duotone);
        assert_eq!(scene_for(Section::Groove, true, 0).kind, SceneKind::Checker);
        assert_eq!(scene_for(Section::Calm, false, 0).kind, SceneKind::Cathedral);
        // Every look in the library is reachable.
        let mut seen = std::collections::HashSet::new();
        for section in sections {
            for minor in [false, true] {
                for step in 0..4 {
                    seen.insert(scene_for(section, minor, step).kind);
                }
            }
        }
        assert_eq!(seen.len(), 18, "{seen:?}");
    }

    #[test]
    fn the_library_only_ever_takes_light_away() {
        let kinds = [
            SceneKind::Ember,
            SceneKind::Dawn,
            SceneKind::Cathedral,
            SceneKind::Aurora,
            SceneKind::Starfield,
            SceneKind::Duotone,
            SceneKind::Tide,
            SceneKind::Lattice,
            SceneKind::Pulse,
            SceneKind::Checker,
            SceneKind::Undertow,
            SceneKind::Riser,
            SceneKind::Hold,
            SceneKind::Supernova,
            SceneKind::Mainstage,
            SceneKind::Voltage,
            SceneKind::Prism,
            SceneKind::Stampede,
        ];
        let snapshot = CueSnapshot { tension_bank: 0.5, ..playing() };
        for kind in kinds {
            let s = scene(kind);
            assert_eq!(s.kind, kind);
            assert!(!kind.name().is_empty());
            let frame = SceneFrame::holding(s);
            for part in [Part::Body, Part::Beam, Part::Effect] {
                for i in 0..400 {
                    let beat = i as f64 * 0.11;
                    let u = -1.0 + 2.0 * (i % 21) as f32 / 20.0;
                    let osc = osc_at(&snapshot, beat);
                    let shade = frame.shade(&s, &snapshot, &osc, part, u, i, i as u32);
                    assert!(
                        (0.0..=1.0001).contains(&shade.level),
                        "{kind:?} {part:?} level {} at beat {beat}",
                        shade.level
                    );
                    assert!((0.0..1.0).contains(&shade.hue) && (0.0..=1.0).contains(&shade.white));
                }
            }
        }
    }

    #[test]
    fn a_look_holds_for_the_phrase_and_then_moves_on() {
        let mut director = SceneDirector::new();
        let snapshot = playing();
        let mut beat = 0.5;
        let first = run(&mut director, &snapshot, ArcState::Groove, &mut beat, 1.0).kind();
        // Most of a phrase later it is still the same look, with nothing in
        // transition.
        let later = run(&mut director, &snapshot, ArcState::Groove, &mut beat, 12.0);
        assert_eq!(later.kind(), first);
        assert!(!later.in_transition());
        assert!(beat < 32.0);
        // Past the phrase boundary it has moved on, and settles again.
        let next = run(&mut director, &snapshot, ArcState::Groove, &mut beat, 4.0);
        assert!(beat > 32.0);
        assert_ne!(next.kind(), first);
        let settled = run(&mut director, &snapshot, ArcState::Groove, &mut beat, 4.0);
        assert!(!settled.in_transition());
        assert_eq!(settled.kind(), next.kind());
    }

    #[test]
    fn an_arc_that_flickers_does_not_change_the_scene() {
        let mut director = SceneDirector::new();
        let snapshot = playing();
        let mut beat = 0.5;
        let held = run(&mut director, &snapshot, ArcState::Groove, &mut beat, 3.0).kind();
        // What a full groove really reads as: every few seconds the brain
        // calls a second or so of peak, then a few bars of cooldown, with an
        // intro frame here and there.
        for i in 0..600 {
            beat += (BPM / 60.0 * DT) as f64;
            let arc = match i % 300 {
                0..=71 => ArcState::Drop,
                72..=200 => ArcState::Cooldown,
                201..=206 => ArcState::Intro,
                _ => ArcState::Groove,
            };
            let frame = director.tick(&snapshot, arc, beat, BPM, DT);
            assert_eq!(director.section(), Section::Groove, "frame {i}");
            assert_eq!(frame.kind(), held, "frame {i}");
        }
    }

    #[test]
    fn a_peak_that_lasts_gets_its_own_look_and_gives_it_back() {
        let mut director = SceneDirector::new();
        let snapshot = playing();
        let mut beat = 0.5;
        run(&mut director, &snapshot, ArcState::Groove, &mut beat, 9.0);
        // A second of peak is not a peak section…
        run(&mut director, &snapshot, ArcState::SustainPeak, &mut beat, 1.0);
        assert_eq!(director.section(), Section::Groove);
        // …a few bars of it is.
        run(&mut director, &snapshot, ArcState::SustainPeak, &mut beat, 4.0);
        assert_eq!(director.section(), Section::Peak);
        let peak = director.kind();
        assert!(matches!(
            peak,
            SceneKind::Mainstage | SceneKind::Voltage | SceneKind::Prism | SceneKind::Stampede
        ));
        // A bar of something else is not the end of the peak…
        run(&mut director, &snapshot, ArcState::Cooldown, &mut beat, 1.5);
        assert_eq!(director.section(), Section::Peak);
        // …but several are.
        run(&mut director, &snapshot, ArcState::Cooldown, &mut beat, 6.0);
        assert_eq!(director.section(), Section::Groove);
    }

    #[test]
    fn the_bars_after_a_peak_are_calm_only_if_the_music_fell_away() {
        let mut director = SceneDirector::new();
        let mut beat = 0.5;
        run(&mut director, &playing(), ArcState::Groove, &mut beat, 3.0);
        run(&mut director, &playing(), ArcState::Cooldown, &mut beat, 6.0);
        assert_eq!(director.section(), Section::Groove, "still at full weight");
        let low = CueSnapshot { dynamics: 0.2, ..playing() };
        run(&mut director, &low, ArcState::Cooldown, &mut beat, 4.0);
        assert_eq!(director.section(), Section::Calm);
        // Silence, on the scale of what came before, is calm too; and the
        // music coming back is the groove again.
        run(&mut director, &low, ArcState::Silence, &mut beat, 2.0);
        assert_eq!(director.section(), Section::Calm);
        run(&mut director, &playing(), ArcState::Groove, &mut beat, 4.0);
        assert_eq!(director.section(), Section::Groove);
    }

    #[test]
    fn the_build_follows_the_tension_bank_and_can_let_go() {
        let mut director = SceneDirector::new();
        let mut snapshot = playing();
        let mut beat = 0.5;
        let groove = run(&mut director, &snapshot, ArcState::Groove, &mut beat, 3.0).kind();
        // The bank fills: the riser, then the hold, whatever the arc says.
        snapshot.tension_bank = 0.4;
        assert_eq!(run(&mut director, &snapshot, ArcState::Groove, &mut beat, 0.5).kind(), SceneKind::Riser);
        snapshot.tension_bank = 0.6;
        assert_eq!(run(&mut director, &snapshot, ArcState::Groove, &mut beat, 0.5).kind(), SceneKind::Hold);
        // Hovering just under the way in does not flap it back.
        snapshot.tension_bank = 0.5;
        assert_eq!(run(&mut director, &snapshot, ArcState::Groove, &mut beat, 0.5).kind(), SceneKind::Hold);
        // It drains with no drop: back to the groove, on a fresh look, no flash.
        snapshot.tension_bank = 0.0;
        let frame = run(&mut director, &snapshot, ArcState::Groove, &mut beat, 2.0);
        assert_eq!(director.section(), Section::Groove);
        assert_ne!(frame.kind(), groove);
        assert_eq!(frame.flash, 0.0);
        assert!(!frame.in_transition());
    }

    #[test]
    fn the_drop_is_never_kept_waiting_and_arrives_under_a_flash() {
        let mut director = SceneDirector::new();
        let mut snapshot = playing();
        let mut beat = 0.5;
        run(&mut director, &snapshot, ArcState::Groove, &mut beat, 3.0);
        snapshot.tension_bank = 0.4;
        run(&mut director, &snapshot, ArcState::Buildup, &mut beat, 1.0);
        // Mid-way into the build's fade, straight to the hold, then the drop:
        // three looks in play and the drop still lands on its first frame.
        snapshot.tension_bank = 0.62;
        run(&mut director, &snapshot, ArcState::PreDropTension, &mut beat, 0.3);
        assert_eq!(director.kind(), SceneKind::Hold);
        snapshot.tension_bank = 0.0;
        snapshot.drop_impact = 0.8;
        beat += (BPM / 60.0 * DT) as f64;
        let frame = director.tick(&snapshot, ArcState::Drop, beat, BPM, DT);
        assert_eq!(frame.kind(), SceneKind::Supernova);
        assert!(frame.flash > 0.9, "flash {}", frame.flash);
        // A quarter of a beat later the drop's look is fully in, and the
        // flash is gone within a couple of seconds.
        let frame = run(&mut director, &snapshot, ArcState::Drop, &mut beat, 0.5);
        assert!(!frame.in_transition());
        let frame = run(&mut director, &snapshot, ArcState::Drop, &mut beat, 2.5);
        assert_eq!(frame.flash, 0.0);
        assert_eq!(frame.kind(), SceneKind::Supernova, "while the impact lasts");
        // The impact lets go: the peak takes over, with no second flash.
        snapshot.drop_impact = 0.0;
        let frame = run(&mut director, &snapshot, ArcState::Drop, &mut beat, 3.0);
        assert_eq!(director.section(), Section::Peak);
        assert_ne!(frame.kind(), SceneKind::Supernova);
        assert_eq!(frame.flash, 0.0);
    }

    #[test]
    fn the_brain_calling_a_peak_is_not_a_drop() {
        // `ArcState::Drop` is held for a second after every peak frame: it is
        // not the drop. Only the cue engine's drop impact flashes the rig.
        let mut director = SceneDirector::new();
        let snapshot = playing();
        let mut beat = 0.5;
        run(&mut director, &snapshot, ArcState::Groove, &mut beat, 3.0);
        for _ in 0..600 {
            beat += (BPM / 60.0 * DT) as f64;
            let frame = director.tick(&snapshot, ArcState::Drop, beat, BPM, DT);
            assert_eq!(frame.flash, 0.0);
            assert_ne!(frame.kind(), SceneKind::Supernova);
        }
    }

    #[test]
    fn a_drop_called_in_a_low_passage_does_not_flash() {
        let mut director = SceneDirector::new();
        let mut snapshot = CueSnapshot { dynamics: 0.2, ..playing() };
        let mut beat = 0.5;
        run(&mut director, &snapshot, ArcState::Breakdown, &mut beat, 3.0);
        snapshot.drop_impact = 0.8;
        beat += (BPM / 60.0 * DT) as f64;
        let frame = director.tick(&snapshot, ArcState::Drop, beat, BPM, DT);
        assert_eq!(frame.kind(), SceneKind::Supernova);
        assert_eq!(frame.flash, 0.0);
    }

    #[test]
    fn the_key_colour_holds_through_a_flicker_and_glides_to_a_new_key() {
        let mut director = SceneDirector::new();
        let mut snapshot = CueSnapshot { dominant_pc: 0, ..playing() };
        let mut beat = 0.5;
        let frame = run(&mut director, &snapshot, ArcState::Groove, &mut beat, 2.0);
        assert!(frame.key_hue.abs() < 1e-4);

        // The dominant pitch flickers to the fifth and back, a beat at a time.
        for i in 0..240 {
            beat += (BPM / 60.0 * DT) as f64;
            snapshot.dominant_pc = if (i / 30) % 2 == 0 { 7 } else { 0 };
            let frame = director.tick(&snapshot, ArcState::Groove, beat, BPM, DT);
            assert!(hue_delta(0.0, frame.key_hue).abs() < 1e-4, "frame {i}: {}", frame.key_hue);
        }

        // It really moves to the fifth: the colour follows, without a jump,
        // and the short way round (downward from 0, not up through 0.5).
        snapshot.dominant_pc = 7;
        let mut last = 0.0f32;
        let mut biggest = 0.0f32;
        for _ in 0..360 {
            beat += (BPM / 60.0 * DT) as f64;
            let frame = director.tick(&snapshot, ArcState::Groove, beat, BPM, DT);
            biggest = biggest.max(hue_delta(last, frame.key_hue).abs());
            assert!(hue_delta(0.0, frame.key_hue) <= 1e-4, "the short way round: {}", frame.key_hue);
            last = frame.key_hue;
        }
        assert!(hue_delta(last, 7.0 * SEMITONE).abs() < 0.01, "arrived: {last}");
        assert!(biggest < 0.02, "no frame moves the key colour more than 7 degrees: {biggest}");
    }

    #[test]
    fn a_polarity_flip_exchanges_the_two_colours_without_a_jump() {
        let mut director = SceneDirector::new();
        let mut snapshot = playing();
        let mut beat = 0.5;
        run(&mut director, &snapshot, ArcState::Groove, &mut beat, 3.0);
        let s = scene_for(Section::Groove, false, 0);
        let osc = osc_at(&snapshot, beat);
        let before = director.frame();
        let primary = before.shade(&s, &snapshot, &osc, Part::Body, 0.0, 0, 0).hue;
        let second = before.shade(&s, &snapshot, &osc, Part::Body, 0.0, 1, 0).hue;
        assert!(hue_delta(primary, second).abs() > 0.3, "two colours: {primary} {second}");

        snapshot.polarity_quadrant = 1;
        let mut last = primary;
        let mut biggest = 0.0f32;
        for i in 0..180 {
            // Half a second in the polarity flips again: too soon, ignored.
            if i == 30 {
                snapshot.polarity_quadrant = 2;
            }
            beat += (BPM / 60.0 * DT) as f64;
            let frame = director.tick(&snapshot, ArcState::Groove, beat, BPM, DT);
            let hue = frame.shade(&s, &snapshot, &osc, Part::Body, 0.0, 0, 0).hue;
            biggest = biggest.max(hue_delta(last, hue).abs());
            last = hue;
        }
        let after = director.frame();
        assert!(hue_delta(last, second).abs() < 0.01, "the first colour is now the second");
        let other = after.shade(&s, &snapshot, &osc, Part::Body, 0.0, 1, 0).hue;
        assert!(hue_delta(other, primary).abs() < 0.01, "and the second the first");
        assert!(biggest < 0.04, "the exchange glides: {biggest}");

        assert_eq!(after.swap, 1.0);

        // Once the exchange has had its time, the next flip trades them back.
        snapshot.polarity_quadrant = 3;
        let frame = run(&mut director, &snapshot, ArcState::Groove, &mut beat, 3.0);
        assert_eq!(frame.swap, 0.0);
    }

    #[test]
    fn a_sweep_crosses_the_stage_and_a_bloom_opens_from_the_centre() {
        let sweep = Transition::Sweep { dir: 1.0 };
        // Early on the near end is changing and the far end has not started.
        assert!(sweep.weight(0.3, -1.0) > 0.9);
        assert_eq!(sweep.weight(0.3, 1.0), 0.0);
        // It gets everywhere, in order, and finishes complete.
        for step in 0..=20 {
            let p = step as f32 / 20.0;
            let across: Vec<f32> = (0..9).map(|i| sweep.weight(p, -1.0 + i as f32 * 0.25)).collect();
            assert!(across.windows(2).all(|w| w[0] >= w[1]), "{across:?}");
        }
        assert!((0..9).all(|i| sweep.weight(1.0, -1.0 + i as f32 * 0.25) == 1.0));
        assert!((0..9).all(|i| sweep.weight(0.0, -1.0 + i as f32 * 0.25) == 0.0));
        // The other way round.
        let back = Transition::Sweep { dir: -1.0 };
        assert!(back.weight(0.3, 1.0) > 0.9 && back.weight(0.3, -1.0) == 0.0);
        // A bloom reaches the centre first and both ends together.
        assert!(Transition::Bloom.weight(0.3, 0.0) > 0.9);
        assert_eq!(Transition::Bloom.weight(0.3, 1.0), Transition::Bloom.weight(0.3, -1.0));
        assert_eq!(Transition::Bloom.weight(0.3, 1.0), 0.0);
    }

    #[test]
    fn blending_crossfades_the_looks_in_play() {
        let mut director = SceneDirector::new();
        let snapshot = playing();
        let mut beat = 0.5;
        run(&mut director, &snapshot, ArcState::Groove, &mut beat, 9.0);
        let old = director.frame().blend(0.0, |s| [s.look.body]);
        // A breakdown, once it has lasted a couple of seconds, melts in over
        // eight beats: half-way through, the level is between the two looks',
        // and it ends on the new one.
        let half = run(&mut director, &snapshot, ArcState::Breakdown, &mut beat, 4.2);
        assert_eq!(director.section(), Section::Calm);
        assert!(half.in_transition());
        let mid = half.blend(0.0, |s| [s.look.effects])[0];
        let (from, to) = (scene_for(Section::Groove, false, 0), scene_for(Section::Calm, false, 1));
        assert!(mid < from.look.effects && mid > to.look.effects, "{mid}");
        let done = run(&mut director, &snapshot, ArcState::Breakdown, &mut beat, 3.0);
        assert!(!done.in_transition());
        assert_eq!(done.blend(0.0, |s| [s.look.effects])[0], to.look.effects);
        assert_eq!(old[0], from.look.body);
    }

    #[test]
    fn the_beams_change_colour_only_inside_a_dip() {
        let mut director = SceneDirector::new();
        let mut snapshot = CueSnapshot { dominant_pc: 0, ..playing() };
        let mut beat = 0.5;
        // Long enough that the wheels' hold has passed.
        let frame = run(&mut director, &snapshot, ArcState::Groove, &mut beat, 6.0);
        assert_eq!(frame.beam_dip, 1.0);
        let colour = frame.beam_hue;

        snapshot.dominant_pc = 5;
        let mut dipped = 1.0f32;
        let mut changed_at = None;
        for i in 0..240 {
            beat += (BPM / 60.0 * DT) as f64;
            let frame = director.tick(&snapshot, ArcState::Groove, beat, BPM, DT);
            dipped = dipped.min(frame.beam_dip);
            if frame.beam_hue != colour && changed_at.is_none() {
                changed_at = Some((i, frame.beam_dip));
            }
        }
        let (_, level) = changed_at.expect("the beams took the new colour");
        assert_eq!(level, 0.0, "the wheel turned in the dark");
        assert_eq!(dipped, 0.0);
        assert_eq!(director.frame().beam_dip, 1.0, "and the beams are back");
    }

    #[test]
    fn a_riser_speeds_up_with_tension_and_never_jumps() {
        let mut director = SceneDirector::new();
        let mut snapshot = playing();
        let mut beat = 0.5;
        run(&mut director, &snapshot, ArcState::Buildup, &mut beat, 3.0);
        let calm = director.frame().riser;
        let frame = run(&mut director, &snapshot, ArcState::Buildup, &mut beat, 1.0);
        assert_eq!(frame.riser, calm, "no tension, no extra speed");

        snapshot.tension_bank = 0.7;
        let mut last = frame.riser;
        for _ in 0..120 {
            beat += (BPM / 60.0 * DT) as f64;
            let frame = director.tick(&snapshot, ArcState::Buildup, beat, BPM, DT);
            let moved = (frame.riser - last).rem_euclid(1.0);
            assert!(moved > 0.0 && moved < 0.05, "{moved}");
            last = frame.riser;
        }
    }

    #[test]
    fn the_layout_spreads_addresses_across_the_stage_and_groups_shared_ones() {
        let rig = vec![
            fixture_at("p0", 1, -6.0, StageVisualKind::LedPanel),
            fixture_at("p1", 25, -2.0, StageVisualKind::LedPanel),
            fixture_at("p2", 49, 2.0, StageVisualKind::LedPanel),
            fixture_at("p3", 73, 6.0, StageVisualKind::LedPanel),
            // Two heads daisy-chained on one address, either side of centre.
            fixture_at("b0", 101, -3.0, StageVisualKind::Beam),
            fixture_at("b1", 101, 3.0, StageVisualKind::Beam),
            fixture_at("b2", 121, 6.0, StageVisualKind::Beam),
        ];
        let layout = StageLayout::new(rig.iter());
        // Without cells a fixture stands where it was placed.
        let flat: Vec<StagePlace> = rig.iter().map(|f| layout.place(f, 0)).collect();
        assert_eq!((flat[0].u, flat[3].u), (-1.0, 1.0));
        assert!((flat[1].u + 1.0 / 3.0).abs() < 1e-4 && (flat[2].u - 1.0 / 3.0).abs() < 1e-4);
        assert_eq!(flat[0].span_u, 0.0);
        // The panels have eight cells each; the beams none.
        let places: Vec<StagePlace> = rig
            .iter()
            .map(|f| layout.place(f, if f.visual_kind == StageVisualKind::LedPanel { 8 } else { 0 }))
            .collect();
        assert_eq!([places[0].rank, places[1].rank, places[2].rank, places[3].rank], [0, 1, 2, 3]);
        // The shared address is one place: the middle of where its heads stand.
        assert_eq!(places[4], places[5]);
        assert_eq!(places[4].u, 0.0);
        // Beams rank among beams.
        assert_eq!((places[4].rank, places[6].rank), (0, 1));
        assert_eq!((places[0].of, places[4].of), (4, 2));
        // Four panels tile the stage with their cells, end to end, two
        // stripes each.
        assert_eq!(places[0].span_u, 0.5);
        assert_eq!(places[0].stripes, 2);
        let cells: Vec<f32> =
            places[..4].iter().flat_map(|p| (0..8).map(|c| p.cell_u(c, 8))).collect();
        assert!((cells[0] + 1.0 - 1.0 / 32.0).abs() < 1e-4, "{}", cells[0]);
        assert!((cells[31] - 1.0 + 1.0 / 32.0).abs() < 1e-4, "{}", cells[31]);
        assert!(cells.windows(2).all(|w| (w[1] - w[0] - 1.0 / 16.0).abs() < 1e-4), "{cells:?}");
        assert_eq!((places[0].cell_rank(0, 8), places[0].cell_rank(7, 8)), (0, 1));
        assert_eq!((places[1].cell_rank(0, 8), places[1].cell_rank(7, 8)), (2, 3));

        // A rig whose positions are all the same is one place, not a panic.
        let stacked = vec![
            fixture_at("a", 1, 0.0, StageVisualKind::LedPanel),
            fixture_at("b", 9, 0.0, StageVisualKind::LedPanel),
        ];
        let layout = StageLayout::new(stacked.iter());
        assert_eq!(layout.place(&stacked[0], 0).u, 0.0);
        assert_eq!(layout.place(&stacked[1], 0).rank, 1);
        // A fixture the layout never saw is centre stage.
        assert_eq!(layout.place(&rig[6], 0), StagePlace::CENTRE);
    }

    #[test]
    fn rings_count_outwards_from_the_centre() {
        let rings = |of: usize| -> Vec<usize> {
            (0..of).map(|rank| StagePlace { rank, of, ..StagePlace::CENTRE }.ring()).collect()
        };
        assert_eq!(rings(1), [0]);
        assert_eq!(rings(2), [0, 1]);
        assert_eq!(rings(3), [1, 0, 1]);
        assert_eq!(rings(4), [1, 0, 0, 1]);
        assert_eq!(rings(5), [2, 1, 0, 1, 2]);
        assert_eq!(StagePlace::for_mover(3, 4).ring(), 1);
        assert_eq!(StagePlace::for_mover(0, 0).ring(), 0);
    }

    #[test]
    fn a_wave_travels_across_the_stage() {
        let snapshot = playing();
        let s = scene(SceneKind::Tide);
        let frame = SceneFrame::holding(s);
        // Where the crest is, a quarter of a bar apart.
        let crest_at = |beat: f64| {
            let osc = osc_at(&snapshot, beat);
            (0..41)
                .map(|i| {
                    let u = -1.0 + i as f32 / 20.0;
                    (u, frame.shade(&s, &snapshot, &osc, Part::Body, u, 0, 0).level)
                })
                .fold((0.0f32, 0.0f32), |best, (u, level)| if level > best.1 { (u, level) } else { best })
                .0
        };
        let (a, b) = (crest_at(16.5), crest_at(17.5));
        assert!(b > a + 0.3, "the crest moved along the stage: {a} -> {b}");
        // And the stage is never uniformly lit while it runs.
        let osc = osc_at(&snapshot, 16.5);
        let levels: Vec<f32> = (0..9)
            .map(|i| frame.shade(&s, &snapshot, &osc, Part::Body, -1.0 + i as f32 * 0.25, 0, 0).level)
            .collect();
        let (lo, hi) = levels.iter().fold((1.0f32, 0.0f32), |(lo, hi), &l| (lo.min(l), hi.max(l)));
        assert!(hi > 0.95 && lo < 0.6, "{levels:?}");
    }

    #[test]
    fn the_chase_goes_still_in_silence() {
        let quiet = CueSnapshot { energy_slow: 0.0, ..CueSnapshot::default() };
        let s = scene(SceneKind::Mainstage);
        let frame = SceneFrame::holding(s);
        for i in 0..40 {
            let osc = osc_at(&quiet, 4.0 + i as f64 * 0.13);
            let shade = frame.shade(&s, &quiet, &osc, Part::Body, 0.3, i, 7);
            assert_eq!(shade.level, s.look.body, "beat step {i}");
        }
    }
}

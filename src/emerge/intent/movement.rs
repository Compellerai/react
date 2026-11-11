//! Mover choreography — the figures a moving head can trace, the director
//! that chooses between them, and the harmonic aim that steers them.
//!
//! Until this module the movers knew one move: a phrase-length pan sine with a
//! bar-length tilt triangle, at every energy, in every section. Here the
//! motion gets a vocabulary (circle, figure-eight, fan, wave, scissor, bounce,
//! ballyhoo, gather) and a reason to change: the [`MovementDirector`] picks a
//! figure from the musical arc and the tonality, re-picks on phrase boundaries,
//! and crossfades so a change never jumps a head.
//!
//! A figure is a shape; a **level** is where in the room it is flown — down on
//! the floor, out over the crowd, up in the air, or split with alternate heads
//! high and low ([`MovePosture`]). The level changes when the music does: the
//! build raises the rig to the sky, the drop throws it out over the room, and
//! every polarity flip and phrase moves it on.
//!
//! The harmonic field steers it three ways:
//!
//! * **Aim** — the harmony's position on the circle of fifths (the Tonnetz
//!   centroid) is a point in a disc; the rig's centre of motion leans toward
//!   it. A chord a fifth away is a small lean, a distant modulation swings the
//!   whole rig across.
//! * **Throw** — every chord change kicks the heads sideways, sharpward one
//!   way and flatward the other, sized by how far the root travelled.
//! * **Gather** — tension (a charging build, a dominant or diminished chord)
//!   pulls the beams in toward one point; the release lets them fly back out.
//!
//! Split of responsibilities: the director is the only stateful part and runs
//! once per frame (in the DMX manager, beside the look rotation); it stamps a
//! [`MovementFrame`] on the snapshot. Everything a fixture then does with that
//! frame is a pure function of the snapshot and the clock — the same contract
//! as the rest of the intent layer, so the wire and the preview cannot drift.
//!
//! A default `MovementFrame` is the legacy sweep with no steering, and the
//! intent layer returns the legacy value untouched in that case. Anything that
//! renders without a director (tests, pre-init frames) is byte-identical to
//! before.

use std::f32::consts::TAU;

use super::fixture_brain::ArcState;
use super::oscillator::{OscillatorFrameSnapshot, OscillatorInterval};
use crate::hue::CueSnapshot;

// ── Figures ─────────────────────────────────────────────────────────────────

/// A shape a moving head traces around its centre.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MoveFigure {
    /// The original look: phrase-length pan sine, bar-length tilt triangle.
    /// Rendered by the legacy path in `intent.rs`, not by [`figure_point`].
    #[default]
    Sweep,
    /// Pan and tilt a quarter-cycle apart. Heads are spread around the circle
    /// so they chase each other; alternate heads run the other way round.
    Circle,
    /// A lemniscate — pan once, tilt twice per cycle.
    FigureEight,
    /// The beams open out sideways from the centre and lift, then close and
    /// settle. Every head breathes together.
    Fan,
    /// Heads hold a fanned-out pan and nod in sequence, so a ripple of tilt
    /// runs along the rig.
    Wave,
    /// Alternate heads pan against each other and cross at the centre.
    Scissor,
    /// A tilt dip on the beat that eases back up; alternate heads dip on the
    /// off-beat.
    Bounce,
    /// Fast, wide, deliberately unruly — each head on its own knot of a
    /// Lissajous curve. For the drop.
    Ballyhoo,
    /// Everything converges on one point, lifted, with a small tremble. For
    /// the build.
    Gather,
}

/// A figure plus how long one cycle takes and how big it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveLook {
    pub figure: MoveFigure,
    /// Bars per cycle. Powers of two, so a figure re-aligns with the phrase.
    pub bars: f32,
    /// Size multiplier on top of the energy / journey / coherence depth.
    pub size: f32,
}

impl MoveLook {
    /// The legacy sweep — the neutral look.
    pub const SWEEP: MoveLook = MoveLook { figure: MoveFigure::Sweep, bars: 8.0, size: 1.0 };

    pub const fn new(figure: MoveFigure, bars: f32) -> Self {
        Self { figure, bars, size: 1.0 }
    }

    pub const fn sized(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    fn is_sweep(&self) -> bool {
        self.figure == MoveFigure::Sweep
    }
}

impl Default for MoveLook {
    fn default() -> Self {
        Self::SWEEP
    }
}

/// Where one head sits within the group of movers — what lets a figure spread
/// the heads out instead of moving them as one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoverSlot {
    /// Cycle offset (0-1): how far round the figure this head starts.
    pub phase: f32,
    /// Position across the rig, -1 (first address) .. +1 (last).
    pub spread: f32,
    /// +1 / -1 on alternate heads, for mirrored and counter-rotating figures.
    pub mirror: f32,
}

impl MoverSlot {
    /// `group_phase` is the symmetry-grouped cycle offset the legacy sweep
    /// already uses, so a figure and the sweep agree on who leads.
    pub fn new(ordinal: usize, count: usize, group_phase: f32) -> Self {
        let spread = if count > 1 {
            (ordinal.min(count - 1) as f32 / (count - 1) as f32) * 2.0 - 1.0
        } else {
            0.0
        };
        Self {
            phase: group_phase,
            spread,
            mirror: if ordinal % 2 == 0 { 1.0 } else { -1.0 },
        }
    }
}

/// One head's place on a figure: pan and tilt offsets from the centre, each
/// within about ±1 (the caller scales them to real depth), plus a steady tilt
/// lift the figure sits on.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FigurePoint {
    pub pan: f32,
    pub tilt: f32,
    pub lift: f32,
}

/// Where `slot`'s head is on `look` right now. Pure, and periodic in the bar
/// clock: every term is a whole number of turns per cycle, so the path closes
/// and the head never jumps when the cycle wraps.
pub fn figure_point(look: MoveLook, osc: &OscillatorFrameSnapshot, slot: MoverSlot) -> FigurePoint {
    let cycles_per_bar = 1.0 / look.bars.max(0.25);
    let p = osc.cycle_phase(OscillatorInterval::Bar, cycles_per_bar, 0.0);
    match look.figure {
        MoveFigure::Sweep => FigurePoint::default(),
        MoveFigure::Circle => {
            let a = TAU * (p + slot.phase);
            FigurePoint { pan: slot.mirror * a.cos() * 0.70, tilt: a.sin() * 0.70, lift: 0.08 }
        }
        MoveFigure::FigureEight => {
            let a = TAU * (p + slot.phase);
            FigurePoint { pan: slot.mirror * a.sin() * 0.85, tilt: (2.0 * a).sin() * 0.55, lift: 0.04 }
        }
        MoveFigure::Fan => {
            // 0 (closed) → 1 (open) → 0 over the cycle.
            let open = 0.5 - 0.5 * (TAU * p).cos();
            FigurePoint {
                pan: slot.spread * (0.30 + 0.70 * open),
                tilt: (open - 0.5) * 0.70,
                lift: 0.10,
            }
        }
        MoveFigure::Wave => FigurePoint {
            pan: slot.spread * 0.60,
            tilt: (TAU * (p - slot.phase)).sin() * 0.85,
            lift: 0.05,
        },
        MoveFigure::Scissor => {
            let a = TAU * p;
            FigurePoint { pan: slot.mirror * a.sin() * 0.90, tilt: (2.0 * a).cos() * 0.25, lift: 0.06 }
        }
        MoveFigure::Bounce => {
            // Dip on every beat for a brisk look, every other beat otherwise;
            // alternate heads take the off-beat so the rig ping-pongs.
            let per_beat = bounce_per_beat(look);
            let offbeat = if slot.mirror < 0.0 { 0.5 } else { 0.0 };
            let b = osc.cycle_phase(OscillatorInterval::Beat, per_beat, offbeat);
            // A ball's arc: at the floor exactly on the beat, at the top
            // between beats. Continuous — the head reverses on the beat
            // rather than being asked to teleport down to it.
            let hit = (1.0 - 2.0 * b) * (1.0 - 2.0 * b);
            FigurePoint {
                pan: slot.spread * 0.55 + slot.mirror * 0.12 * (TAU * p).sin(),
                tilt: 0.35 - 1.10 * hit,
                lift: 0.0,
            }
        }
        MoveFigure::Ballyhoo => {
            // Whole-number harmonics of one phase, so it is unruly but closes.
            // Kept to LOW harmonics: travel is what reads as wild on a real
            // head, and every extra harmonic spends the head's speed on
            // wiggle instead (see the head-speed cap below).
            let a = p + slot.phase * 0.37;
            let pan = 0.85 * (TAU * a).sin() + 0.15 * (TAU * (3.0 * a + 0.31)).sin();
            let tilt = 0.80 * (TAU * (2.0 * a + 0.25 + slot.phase * 0.61)).sin()
                + 0.20 * (TAU * (3.0 * a + 0.13)).sin();
            FigurePoint { pan: slot.mirror * pan, tilt, lift: 0.04 }
        }
        MoveFigure::Gather => {
            let a = TAU * p;
            FigurePoint {
                pan: slot.spread * 0.10 + slot.mirror * 0.04 * (4.0 * a).sin(),
                tilt: 0.05 * (8.0 * a).sin(),
                lift: 0.22,
            }
        }
    }
}

/// Bounce dips per beat: every beat for a brisk look, every other otherwise.
fn bounce_per_beat(look: MoveLook) -> f32 {
    if look.bars <= 2.0 { 1.0 } else { 0.5 }
}

// ── Beam levels ─────────────────────────────────────────────────────────────
// Without these every figure is flown around the head's resting aim, which on
// a rig hung to light the stage is the floor: the beams never leave it. A real
// show spends as much time in the air as on the deck.

/// How far a head's tilt and pan travel across the 0-1 intent range, in
/// degrees, centred on its mounting aim (0.5 is the aim itself).
///
/// A head's reach is the half-sphere around the direction its base was
/// installed pointing: a quarter turn either side of it, in pan and in tilt.
/// Heads that share an address get the same values, and each one turns them
/// into its own half-sphere around its own installed direction.
///
/// The preview uses these to turn a pan/tilt value into a beam direction; the
/// levels below use them to turn an elevation into a tilt value — one number,
/// so the two agree. The DMX writer scales a moving head's pan/tilt down to
/// them (see `scale_motion_to_window` in the DMX mapping), so the real head
/// covers the same half-sphere the preview draws rather than its whole
/// travel.
pub const MOVER_TILT_WINDOW_DEG: f32 = 180.0;
pub const MOVER_PAN_WINDOW_DEG: f32 = 180.0;

/// Top of the band for a hung head: degrees from straight down (90 = the
/// horizon). A head hung to light the stage rarely reaches it — its sky is
/// the top of its own travel.
const SKY_DEG: f32 = 130.0;
/// Top of the band for a head rigged to fire up: straight up.
const ZENITH_DEG: f32 = 180.0;
/// The lowest a head rigged to fire UP is brought — a rake just over head
/// height, never down into the deck in front of it.
const RAKE_DEG: f32 = 100.0;
/// The horizon, in degrees from straight down. A head rigged to fire up is
/// never flown below it — not by its level, and not by the bottom of a figure
/// swinging around that level: a head standing on the deck that dips under the
/// horizon only lights the floor in front of itself.
const HORIZON_DEG: f32 = 90.0;
/// The level a rig flies its figure at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MovePosture {
    /// Where each head was aimed in the stage editor — for a rig hung over
    /// the stage, down onto it, each beam around the spot it was placed on.
    #[default]
    Floor,
    /// Halfway up: out over the audience.
    Crowd,
    /// Up in the air above the room.
    Sky,
    /// Alternate heads in the sky and on the floor — a tall X.
    Split,
}

impl MovePosture {
    /// Level (0 floor .. 1 sky) for the even and the odd heads.
    fn levels(self) -> (f32, f32) {
        match self {
            MovePosture::Floor => (0.0, 0.0),
            MovePosture::Crowd => (0.5, 0.5),
            MovePosture::Sky => (1.0, 1.0),
            MovePosture::Split => (1.0, 0.0),
        }
    }
}

/// The tilt centres `(floor, sky)` a head is flown between, given where it
/// was aimed when it was rigged (`aim_deg`, degrees from straight down) and
/// where it rests (`rest`: that aim plus the harmonic drift).
///
/// The floor is the rest position — the spot the operator placed. The sky is
/// `SKY_DEG`, or the top of the head's travel when that comes first: the
/// levels run the whole way up the half-sphere, so a change from floor to sky
/// is a beam going bottom to top. (A swing flown at the top is slid back
/// under the stop by [`MovementFrame::tilt`], not squeezed.) The levels divide
/// whatever lies between evenly, so every head gets three distinct heights
/// however it is hung.
///
/// A head aimed well above the horizon is already an aerial: its floor comes
/// DOWN to a rake over the room and its sky is straight up.
fn level_band(aim_deg: f32, rest: f32) -> (f32, f32) {
    // The tilt value that points this head at a world elevation.
    let at = |deg: f32| 0.5 + (deg - aim_deg) / MOVER_TILT_WINDOW_DEG;
    let (floor, top) = if aim_deg > RAKE_DEG {
        (rest.min(at(RAKE_DEG)), ZENITH_DEG)
    } else {
        (rest, SKY_DEG)
    };
    let sky = at(aim_deg.max(top)).min(1.0).max(floor);
    (floor, sky)
}

/// How far short of the end of tilt travel a figure turns back.
const STOP_MARGIN: f32 = 0.02;

/// The furthest a figure's tilt offset ever gets from zero (the peak of each
/// tilt expression in [`figure_point`]) — how much room it needs either side
/// of its centre.
fn tilt_reach(figure: MoveFigure) -> f32 {
    match figure {
        MoveFigure::Sweep => 0.0,
        MoveFigure::Circle => 0.70,
        MoveFigure::FigureEight => 0.55,
        MoveFigure::Fan => 0.35,
        MoveFigure::Wave => 0.85,
        MoveFigure::Scissor => 0.25,
        MoveFigure::Bounce => 0.75,
        MoveFigure::Ballyhoo => 1.00,
        MoveFigure::Gather => 0.05,
    }
}

/// Pick the level for a moment in the music. Pure, like [`choose_look`];
/// `step` advances on every phrase and every polarity flip, and no pool
/// repeats back-to-back, so each step is a visible move.
pub fn choose_posture(arc: ArcState, snapshot: &CueSnapshot, step: u32) -> MovePosture {
    use MovePosture::*;
    let pick = |pool: &[MovePosture]| pool[step as usize % pool.len()];
    match arc {
        ArcState::Silence | ArcState::Intro => Floor,
        // The build climbs; the drop throws the beams out over the room.
        ArcState::Buildup | ArcState::PreDropTension => Sky,
        ArcState::Drop => Crowd,
        ArcState::SustainPeak => pick(&[Crowd, Split, Sky]),
        ArcState::Groove => {
            // Minor stays grounded and conflicted; major opens up.
            if snapshot.major_smooth < MINOR_BELOW {
                pick(&[Floor, Split, Crowd])
            } else {
                pick(&[Crowd, Sky, Floor])
            }
        }
        ArcState::Cooldown | ArcState::Breakdown => pick(&[Sky, Crowd]),
    }
}

// ── What a real head can do ─────────────────────────────────────────────────
// A figure is only a figure if the fixture can trace it. A moving head has a
// top speed, and a figure that asks for more does not get faster — the head
// falls behind, cuts every corner, and the shape collapses into a smear that
// no longer lines up with the bar. So each figure's depth is capped at the
// largest size whose FASTEST moment the head can still follow at the current
// tempo: a quick figure is automatically a tight one, a slow figure can be
// wide. (This is the real trade a lighting designer makes by hand.)

/// How fast a head really turns, in degrees per second. A 7R-class beam
/// crosses its 540° pan in roughly 2.5-3 s and tilts at 150-200° a second;
/// these sit at the slow end of that so the slowest head on the rig still
/// traces the figure. Raise them for faster fixtures.
const HEAD_PAN_DEG_PER_S: f32 = 180.0;
const HEAD_TILT_DEG_PER_S: f32 = 150.0;

/// The same, in the units the figures are drawn in: 0-1 pan/tilt ranges per
/// second, where the whole range is the travel window.
const HEAD_PAN_SPEED: f32 = HEAD_PAN_DEG_PER_S / MOVER_PAN_WINDOW_DEG;
const HEAD_TILT_SPEED: f32 = HEAD_TILT_DEG_PER_S / MOVER_TILT_WINDOW_DEG;

/// The steepest a figure's `(pan, tilt)` offsets ever change, per BAR, at unit
/// depth — the analytic peak of each expression in [`figure_point`] (checked
/// against a numeric derivative in the tests, so the two cannot drift).
fn peak_rates(look: MoveLook) -> (f32, f32) {
    let c = 1.0 / look.bars.max(0.25); // cycles per bar
    match look.figure {
        MoveFigure::Sweep => (0.0, 0.0),
        MoveFigure::Circle => (0.70 * TAU * c, 0.70 * TAU * c),
        MoveFigure::FigureEight => (0.85 * TAU * c, 0.55 * 2.0 * TAU * c),
        MoveFigure::Fan => (0.70 * 0.5 * TAU * c, 0.70 * 0.5 * TAU * c),
        MoveFigure::Wave => (0.0, 0.85 * TAU * c),
        MoveFigure::Scissor => (0.90 * TAU * c, 0.25 * 2.0 * TAU * c),
        // The ball's slope is steepest at the floor: 4 per dip, 4 beats a bar.
        MoveFigure::Bounce => (0.12 * TAU * c, 1.10 * 4.0 * 4.0 * bounce_per_beat(look)),
        MoveFigure::Ballyhoo => {
            ((0.85 + 3.0 * 0.15) * TAU * c, (2.0 * 0.80 + 3.0 * 0.20) * TAU * c)
        }
        MoveFigure::Gather => (0.04 * 4.0 * TAU * c, 0.05 * 8.0 * TAU * c),
    }
}

// ── The per-frame movement state ────────────────────────────────────────────

/// What the director decided this frame. Stamped on the `CueSnapshot`; read by
/// the intent layer for every mover.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovementFrame {
    /// The look being moved to (or held).
    pub look: MoveLook,
    /// The look being left.
    pub prev: MoveLook,
    /// 0 = entirely `prev`, 1 = entirely `look`. Already eased.
    pub blend: f32,
    /// Harmonic aim, -1..1 each: the Tonnetz position, slewed.
    pub aim_pan: f32,
    pub aim_tilt: f32,
    /// Chord-change kick, -1..1, decaying. Sign is the side of the circle of
    /// fifths the root moved to.
    pub throw: f32,
    /// 0..1 — how hard tension is pulling the beams together.
    pub gather: f32,
    /// Seconds per bar at the current tempo — what turns a figure's per-bar
    /// rate into a real head speed for the speed cap. 0 = tempo unknown: no
    /// cap (tests, pre-init frames).
    pub bar_s: f32,
    /// Beam level, 0 (floor) .. 1 (sky), already slewed at a pace a head can
    /// follow. See [`MovePosture`].
    pub level: f32,
    /// How far alternate heads sit above (even) and below (odd) `level`.
    pub split: f32,
    /// A director is flying the levels. `false` (no director) leaves every
    /// head's tilt centre exactly where the legacy look puts it.
    pub leveled: bool,
}

impl Default for MovementFrame {
    fn default() -> Self {
        Self {
            look: MoveLook::SWEEP,
            prev: MoveLook::SWEEP,
            blend: 1.0,
            aim_pan: 0.0,
            aim_tilt: 0.0,
            throw: 0.0,
            gather: 0.0,
            bar_s: 0.0,
            level: 0.0,
            split: 0.0,
            leveled: false,
        }
    }
}

// ── Depth knobs (all tunable) ───────────────────────────────────────────────
/// Figure depth at full motion, in pan / tilt range. Matches the legacy sweep
/// amounts so a figure is the same scale as the look it replaces.
const FIGURE_PAN_DEPTH: f32 = 0.34;
const FIGURE_TILT_DEPTH: f32 = 0.28;
/// How far the harmonic aim leans the centre of motion at the rim of the
/// Tonnetz disc.
const AIM_PAN_DEPTH: f32 = 0.14;
const AIM_TILT_DEPTH: f32 = 0.10;
/// How far a full-strength chord throw kicks the pan.
const THROW_PAN_DEPTH: f32 = 0.12;
/// A throw also briefly opens the figure up.
const THROW_SIZE_POP: f32 = 0.30;
/// Tension closes the figure down by up to this fraction…
const GATHER_SQUEEZE: f32 = 0.70;
/// …and lifts the beams as they converge.
const GATHER_LIFT: f32 = 0.10;
/// The drop throws the figure open.
const DROP_SIZE_POP: f32 = 0.35;
/// Intensity-license band over which the figures hand back to the slow scan.
/// Below the low edge the movers are in the legacy SCAN mode untouched — an
/// atonal wall asks for calm, not choreography.
const LICENSE_LO: f32 = 0.25;
const LICENSE_HI: f32 = 0.60;

fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The intent layer's own motion numbers, handed over so the figures inherit
/// exactly the depth, centre and gating the legacy sweep has.
#[derive(Clone, Copy, Debug)]
pub struct MotionContext {
    /// The legacy value for this axis (already license-crossfaded to SCAN).
    pub legacy: f32,
    /// Centre of motion for the chasing (high-license) mode.
    pub center: f32,
    /// Journey × coherence × surprise depth multiplier.
    pub motion: f32,
    pub slot: MoverSlot,
    /// Where the head was aimed when it was rigged, in degrees from straight
    /// down (the stage editor's mounting tilt) — what turns a beam level into
    /// this head's tilt. Only the tilt axis reads it.
    pub mount_tilt_deg: f32,
    /// `center` with the legacy energy lift taken out: where the head rests.
    /// The floor level sits here, and the levels above replace the lift.
    /// Only the tilt axis reads it.
    pub rest: f32,
}

impl MovementFrame {
    /// True when this frame asks for nothing but the legacy sweep — the
    /// intent layer then returns the legacy value bit-for-bit.
    pub fn is_legacy(&self) -> bool {
        self.look.is_sweep()
            && (self.prev.is_sweep() || self.blend >= 1.0)
            && self.aim_pan == 0.0
            && self.aim_tilt == 0.0
            && self.throw == 0.0
            && self.gather == 0.0
            && !self.leveled
    }

    /// Tilt offset (in tilt range) that flies this head at the frame's level.
    fn level_shift(&self, ctx: &MotionContext) -> f32 {
        if !self.leveled {
            return 0.0;
        }
        let aim = if ctx.mount_tilt_deg.is_finite() { ctx.mount_tilt_deg.clamp(0.0, 180.0) } else { 0.0 };
        let level = (self.level + self.split * ctx.slot.mirror).clamp(0.0, 1.0);
        let (floor, sky) = level_band(aim, ctx.rest);
        floor + (sky - floor) * level - ctx.center
    }

    /// The lowest tilt value this head is flown to: the horizon for a head
    /// rigged to fire up, the end of travel for every other.
    fn low_stop(&self, ctx: &MotionContext) -> f32 {
        if !self.leveled || !ctx.mount_tilt_deg.is_finite() {
            return 0.0;
        }
        let aim = ctx.mount_tilt_deg.clamp(0.0, 180.0);
        if aim > RAKE_DEG {
            (0.5 + (HORIZON_DEG - aim) / MOVER_TILT_WINDOW_DEG).max(0.0)
        } else {
            0.0
        }
    }

    /// How much of the figure choreography this moment has earned (0..1).
    fn license_weight(snapshot: &CueSnapshot) -> f32 {
        smoothstep(LICENSE_LO, LICENSE_HI, snapshot.intensity_license.clamp(0.0, 1.0))
    }

    /// Size multiplier shared by both axes: the look's own size, squeezed by
    /// tension, popped by the drop and by a chord throw.
    fn size(&self, look: MoveLook, snapshot: &CueSnapshot) -> f32 {
        look.size
            * (1.0 - GATHER_SQUEEZE * self.gather.clamp(0.0, 1.0))
            * (1.0 + DROP_SIZE_POP * snapshot.drop_impact.clamp(0.0, 1.0))
            * (1.0 + THROW_SIZE_POP * self.throw.abs().min(1.0))
    }

    /// The largest depth at which a figure changing `rate` per bar (at unit
    /// depth) stays within `head_speed` at this tempo.
    fn depth_cap(&self, rate: f32, head_speed: f32) -> f32 {
        if self.bar_s > 0.0 && rate > 1e-6 {
            head_speed * self.bar_s / rate
        } else {
            f32::INFINITY
        }
    }

    /// Blend the two looks' values for one axis. A `Sweep` leg IS the legacy
    /// value, so crossfading to or from the old look is seamless.
    fn blend_axis(&self, legacy: f32, of: impl Fn(MoveLook) -> f32) -> f32 {
        let from = if self.prev.is_sweep() { legacy } else { of(self.prev) };
        let to = if self.look.is_sweep() { legacy } else { of(self.look) };
        from + (to - from) * self.blend.clamp(0.0, 1.0)
    }

    /// Pan (0..1) for one mover.
    pub fn pan(&self, snapshot: &CueSnapshot, osc: &OscillatorFrameSnapshot, ctx: MotionContext) -> f32 {
        if self.is_legacy() {
            return ctx.legacy;
        }
        let env = osc.energy_env();
        let energy_depth = 1.0 + 0.4 * snapshot.energy;
        let figure = self.blend_axis(ctx.legacy, |look| {
            let pt = figure_point(look, osc, ctx.slot);
            let depth = (FIGURE_PAN_DEPTH * energy_depth * ctx.motion * env * self.size(look, snapshot))
                .min(self.depth_cap(peak_rates(look).0, HEAD_PAN_SPEED));
            ctx.center + pt.pan * depth
        });
        // Steering rides on top of whichever look is up — the sweep leans and
        // kicks with the harmony too. Gated like the figures, so it is still
        // in silence.
        let steer = (self.aim_pan * AIM_PAN_DEPTH + self.throw * THROW_PAN_DEPTH) * env;
        let chase = figure + steer;
        (ctx.legacy + (chase - ctx.legacy) * Self::license_weight(snapshot)).clamp(0.0, 1.0)
    }

    /// Tilt (0..1) for one mover.
    pub fn tilt(&self, snapshot: &CueSnapshot, osc: &OscillatorFrameSnapshot, ctx: MotionContext) -> f32 {
        if self.is_legacy() {
            return ctx.legacy;
        }
        let env = osc.energy_env();
        let energy_depth = 1.0 + 0.4 * snapshot.energy;
        // Steering, plus the beam level. The level is a place, not a motion:
        // it holds through a quiet bar (no energy gate) and hands back to the
        // scan only with the license.
        let steer = (self.aim_tilt * AIM_TILT_DEPTH + self.gather.clamp(0.0, 1.0) * GATHER_LIFT) * env
            + self.level_shift(&ctx);
        let low = self.low_stop(&ctx);
        // The biggest a swing can be: half the travel between the stops.
        let half_travel = ((1.0 - low) * 0.5 - STOP_MARGIN).max(0.0);
        // Where a swing reaching `half` either side of `center` is flown: at
        // its level, or as near to it as lets the whole swing through. At the
        // top of travel (or, for a head firing up, the horizon) the swing is
        // slid back until it just touches the stop — it keeps its size and
        // the beam still reaches the end of the half-sphere, instead of
        // flattening against the stop and stalling there every cycle.
        let fly = |center: f32, half: f32| {
            let keep = half.min(half_travel) + STOP_MARGIN;
            let (lo, hi) = (low + keep, 1.0 - keep);
            (center + steer).max(lo).min(hi.max(lo))
        };
        let leg = |look: MoveLook| {
            if look.is_sweep() {
                // The legacy sweep, flown at the level: its own swing around
                // the chase centre, as deep as the oscillator makes it.
                let half = FIGURE_TILT_DEPTH * energy_depth * ctx.motion * env;
                let fit = if half > half_travel { half_travel / half } else { 1.0 };
                return fly(ctx.center, half) + (ctx.legacy - ctx.center) * fit;
            }
            let pt = figure_point(look, osc, ctx.slot);
            // A figure's own lift stood in for a level before there were
            // levels. Under a director the level does the lifting, and the
            // floor level has to stay on the spot the beam was placed on.
            let lift = if self.leveled { 0.0 } else { pt.lift * env };
            let reach = tilt_reach(look.figure).max(1e-3);
            let depth = (FIGURE_TILT_DEPTH * energy_depth * ctx.motion * env * self.size(look, snapshot))
                .min(self.depth_cap(peak_rates(look).1, HEAD_TILT_SPEED))
                .min(half_travel / reach);
            fly(ctx.center + lift, depth * reach) + pt.tilt * depth
        };
        let (from, to) = (leg(self.prev), leg(self.look));
        let chase = from + (to - from) * self.blend.clamp(0.0, 1.0);
        (ctx.legacy + (chase - ctx.legacy) * Self::license_weight(snapshot)).clamp(low, 1.0)
    }
}

// ── The director ────────────────────────────────────────────────────────────

/// Beats per look. A look holds for a phrase in the groove and half that at
/// the peak, where the music turns over faster.
const PHRASE_BEATS: f64 = 32.0;
const PEAK_PHRASE_BEATS: f64 = 16.0;

/// Crossfade lengths (seconds). The drop snaps; a build closes in over a bar
/// or so; the release out of a build is quick; everything else is unhurried.
const FADE_DROP_S: f32 = 0.30;
const FADE_GATHER_S: f32 = 1.20;
const FADE_RELEASE_S: f32 = 0.40;
const FADE_DEFAULT_S: f32 = 2.00;

/// Time constants (seconds) for the steering signals.
const AIM_TAU_S: f32 = 0.50;
const THROW_TAU_S: f32 = 0.50;
const GATHER_ATTACK_TAU_S: f32 = 0.90;
const GATHER_RELEASE_TAU_S: f32 = 0.18;

/// How fast the level may travel, in levels per second. The widest band a
/// head can have is about 0.7 of its tilt range (a rest position drifted low,
/// up to the top of travel), so this keeps a level change inside
/// `HEAD_TILT_SPEED` for every mount.
const LEVEL_MAX_RATE: f32 = 1.15;
/// How eagerly the level closes on its target (seconds): the drop slams as
/// fast as a head can travel, a polarity flip is brisk, a build climbs for
/// its whole length.
const LEVEL_TAU_DROP_S: f32 = 0.15;
const LEVEL_TAU_FLIP_S: f32 = 0.30;
const LEVEL_TAU_BUILD_S: f32 = 1.60;
const LEVEL_TAU_DEFAULT_S: f32 = 0.90;
/// A polarity flip sooner than this after the last level change is ignored,
/// so a flurry of flips can't keep the heads permanently in transit.
const LEVEL_MIN_HOLD_S: f32 = 2.0;

/// A chord-change envelope jump at least this big is a new change (the
/// tracker sets it to 1.0 on commit and decays it ~6% a frame).
const CHANGE_EDGE: f32 = 0.25;

/// Harmonic-rhythm bands: above BUSY a look runs at double speed, below CALM
/// (and only in a quiet section) at half.
const RHYTHM_BUSY: f32 = 0.55;
const RHYTHM_CALM: f32 = 0.12;
const CALM_ENERGY: f32 = 0.45;

/// Tonality bands on `major_smooth` (0.5 = ambiguous). Minor leans the groove
/// toward the crossing, folded figures; major toward the open, round ones.
const MINOR_BELOW: f32 = 0.45;

fn ease(dt: f32, tau: f32) -> f32 {
    1.0 - (-dt.max(0.0) / tau.max(1e-3)).exp()
}

// ── Dynamics: settle in the low passages ────────────────────────────────────
// `snapshot.dynamics` says how big the passage is on the set's own scale (see
// `cue::dynamics`): a breakdown reads ~0.25, a bass-cut blend ~0.5, drums on
// their own ~0.7, everything in 1.0. The rig opens with it.

/// Dynamics at or below which the rig is fully settled...
const DYNAMICS_SETTLED: f32 = 0.25;
/// ...and at or above which it is exactly the rig it was before dynamics
/// existed. In between it opens in proportion.
const DYNAMICS_OPEN: f32 = 0.90;

/// 0 in a low passage, 1 once the music is at full weight. Everything that
/// settles scales on this, so a snapshot that never had dynamics stamped
/// (1.0) is untouched.
pub(crate) fn dynamics_lift(snapshot: &CueSnapshot) -> f32 {
    ((snapshot.dynamics - DYNAMICS_SETTLED) / (DYNAMICS_OPEN - DYNAMICS_SETTLED)).clamp(0.0, 1.0)
}

/// Blend from `settled` (lift 0) to 1.0 (lift 1).
pub(crate) fn settle(settled: f32, lift: f32) -> f32 {
    settled + (1.0 - settled) * lift.clamp(0.0, 1.0)
}

/// Below this dynamics every figure runs at half speed.
const SLOW_DYNAMICS: f32 = 0.45;
/// A polarity flip in a settled passage: the level glides this slowly, and a
/// flip sooner than this after the last change is ignored. (At full weight
/// they are `LEVEL_TAU_FLIP_S` and `LEVEL_MIN_HOLD_S`.)
const LEVEL_TAU_SETTLED_FLIP_S: f32 = 2.4;
const LEVEL_SETTLED_HOLD_S: f32 = 8.0;
/// What is left of a chord-change throw in a settled passage.
const SETTLED_THROW: f32 = 0.35;

/// Pick the look for a moment in the music. Pure: the same arc, harmony and
/// rotation always give the same look. `rotation` steps once per phrase so the
/// pool for an arc is walked in order and never repeats back-to-back.
pub fn choose_look(arc: ArcState, snapshot: &CueSnapshot, rotation: u32) -> MoveLook {
    use MoveFigure::*;
    let pick = |pool: &[MoveLook]| pool[rotation as usize % pool.len()];
    // Harmonic rhythm sets the pace of the groove figures: a progression that
    // turns over quickly runs them at double speed, a parked chord in a quiet
    // section at half.
    let pace = |look: MoveLook| -> MoveLook {
        if look.is_sweep() {
            return look;
        }
        let busy = snapshot.harmonic_rhythm;
        let bars = if busy > RHYTHM_BUSY && snapshot.dynamics >= SLOW_DYNAMICS {
            (look.bars * 0.5).max(1.0)
        } else if (busy < RHYTHM_CALM && snapshot.energy_slow < CALM_ENERGY)
            || snapshot.dynamics < SLOW_DYNAMICS
        {
            look.bars * 2.0
        } else {
            look.bars
        };
        MoveLook { bars, ..look }
    };
    match arc {
        ArcState::Silence | ArcState::Intro => MoveLook::SWEEP,
        ArcState::Buildup | ArcState::PreDropTension => MoveLook::new(Gather, 2.0),
        ArcState::Drop => MoveLook::new(Ballyhoo, 2.0).sized(1.15),
        ArcState::SustainPeak => pick(&[
            MoveLook::new(Scissor, 2.0),
            MoveLook::new(Ballyhoo, 4.0),
            MoveLook::new(Bounce, 2.0),
            MoveLook::new(Circle, 2.0),
        ]),
        ArcState::Groove => {
            if snapshot.major_smooth < MINOR_BELOW {
                pace(pick(&[
                    MoveLook::new(FigureEight, 4.0),
                    MoveLook::new(Wave, 4.0),
                    MoveLook::new(Scissor, 4.0),
                    MoveLook::SWEEP,
                ]))
            } else {
                pace(pick(&[
                    MoveLook::new(Circle, 4.0),
                    MoveLook::new(Fan, 4.0),
                    MoveLook::new(Wave, 4.0),
                    MoveLook::SWEEP,
                ]))
            }
        }
        ArcState::Cooldown | ArcState::Breakdown => pace(pick(&[
            MoveLook::new(Fan, 8.0).sized(0.85),
            MoveLook::new(FigureEight, 8.0).sized(0.85),
            MoveLook::SWEEP,
        ])),
    }
}

/// Chooses the look, crossfades between looks, and tracks the three harmonic
/// steering signals. Hold one for the life of the lighting session and `tick`
/// it once per frame.
#[derive(Clone, Copy, Debug)]
pub struct MovementDirector {
    look: MoveLook,
    prev: MoveLook,
    /// Linear crossfade progress 0..1 (eased on the way out).
    progress: f32,
    fade_s: f32,
    arc: ArcState,
    phrase: i64,
    rotation: u32,
    primed: bool,
    aim_pan: f32,
    aim_tilt: f32,
    throw: f32,
    gather: f32,
    last_change_env: f32,
    /// Side for a change with no direction of its own (same root, tritone):
    /// alternates so those don't always kick the same way.
    flip: bool,
    /// A look change was wanted while a crossfade was still running; retry
    /// each frame until the fade finishes, so the rig is never left on the
    /// wrong arc's look until the next phrase boundary.
    pending: bool,
    /// Seconds per bar at the last tick's tempo (0 until a tempo is known).
    bar_s: f32,
    posture: MovePosture,
    /// Slewed level of the even and the odd heads.
    level_even: f32,
    level_odd: f32,
    level_tau: f32,
    /// Walks the posture pools: +1 per phrase and per accepted polarity flip.
    level_step: u32,
    /// Seconds since the level last changed.
    level_age: f32,
    polarity: u8,
}

impl Default for MovementDirector {
    fn default() -> Self {
        Self::new()
    }
}

impl MovementDirector {
    pub fn new() -> Self {
        Self {
            look: MoveLook::SWEEP,
            prev: MoveLook::SWEEP,
            progress: 1.0,
            fade_s: FADE_DEFAULT_S,
            arc: ArcState::Silence,
            phrase: 0,
            rotation: 0,
            primed: false,
            aim_pan: 0.0,
            aim_tilt: 0.0,
            throw: 0.0,
            gather: 0.0,
            last_change_env: 0.0,
            flip: false,
            pending: false,
            bar_s: 0.0,
            posture: MovePosture::Floor,
            level_even: 0.0,
            level_odd: 0.0,
            level_tau: LEVEL_TAU_DEFAULT_S,
            level_step: 0,
            level_age: 0.0,
            polarity: 0,
        }
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
    ) -> MovementFrame {
        let dt = dt.clamp(0.0, 0.1);
        // Four beats to the bar, as the oscillators count them.
        self.bar_s = if tempo_bpm > 1.0 { 240.0 / tempo_bpm } else { 0.0 };
        self.progress = (self.progress + dt / self.fade_s.max(1e-3)).min(1.0);

        // ── Look selection: only on a phrase boundary or an arc change, so a
        // look holds long enough to read and a chord flicker can't thrash it.
        let phrase_beats = if matches!(arc, ArcState::Drop | ArcState::SustainPeak) {
            PEAK_PHRASE_BEATS
        } else {
            PHRASE_BEATS
        };
        let phrase = (beat.max(0.0) / phrase_beats).floor() as i64;
        let arc_changed = arc != self.arc;
        let phrase_changed = phrase != self.phrase;
        if phrase_changed {
            self.rotation = self.rotation.wrapping_add(1);
        }
        if !self.primed || arc_changed || phrase_changed || self.pending {
            let want = choose_look(arc, snapshot, self.rotation);
            self.pending = false;
            if want != self.look {
                let entering_drop = arc == ArcState::Drop && self.arc != ArcState::Drop;
                let entering_build = want.figure == MoveFigure::Gather;
                let leaving_build = self.look.figure == MoveFigure::Gather;
                // A change mid-crossfade would need a third look to blend
                // from. The drop and the build are worth a small step; any
                // other change waits for the running fade to finish.
                if self.progress >= 1.0 || entering_drop || entering_build || !self.primed {
                    self.prev = self.look;
                    self.look = want;
                    self.progress = if self.primed { 0.0 } else { 1.0 };
                    self.fade_s = if entering_drop {
                        FADE_DROP_S
                    } else if entering_build {
                        FADE_GATHER_S
                    } else if leaving_build {
                        FADE_RELEASE_S
                    } else {
                        FADE_DEFAULT_S
                    };
                } else {
                    self.pending = true;
                }
            }
        }
        // ── Level: where the figure is flown. Re-chosen on the look's own
        // boundaries plus every polarity flip — the harmony changing sides is
        // the moment the rig should change its shape in the room.
        self.level_age += dt;
        let flipped = self.primed && snapshot.polarity_quadrant != self.polarity;
        self.polarity = snapshot.polarity_quadrant;
        // In a settled passage the harmony changing sides is a slow turn of
        // the rig, not a snap: flips are taken less often and glide.
        let lift = dynamics_lift(snapshot);
        let flip_hold = LEVEL_SETTLED_HOLD_S + (LEVEL_MIN_HOLD_S - LEVEL_SETTLED_HOLD_S) * lift;
        let flip_step = flipped && self.level_age >= flip_hold;
        if phrase_changed || flip_step {
            self.level_step = self.level_step.wrapping_add(1);
        }
        if !self.primed || arc_changed || phrase_changed || flip_step {
            let want = choose_posture(arc, snapshot, self.level_step);
            if want != self.posture {
                self.posture = want;
                self.level_age = 0.0;
                self.level_tau = if arc == ArcState::Drop {
                    LEVEL_TAU_DROP_S
                } else if flip_step {
                    LEVEL_TAU_SETTLED_FLIP_S + (LEVEL_TAU_FLIP_S - LEVEL_TAU_SETTLED_FLIP_S) * lift
                } else if matches!(arc, ArcState::Buildup | ArcState::PreDropTension) {
                    LEVEL_TAU_BUILD_S
                } else {
                    LEVEL_TAU_DEFAULT_S
                };
            }
        }
        let (even, odd) = self.posture.levels();
        if self.primed {
            // Ease in to land softly, but never faster than a head can go.
            let reach = LEVEL_MAX_RATE * dt;
            let k = ease(dt, self.level_tau);
            self.level_even += ((even - self.level_even) * k).clamp(-reach, reach);
            self.level_odd += ((odd - self.level_odd) * k).clamp(-reach, reach);
        } else {
            self.level_even = even;
            self.level_odd = odd;
        }

        self.arc = arc;
        self.phrase = phrase;
        self.primed = true;

        // ── Steering. All three ride a loudness × coherence gate: the harmony
        // only steers the rig when there is harmony to hear.
        let audible = smoothstep(0.08, 0.30, snapshot.energy_slow);
        let gate = audible * snapshot.trust.clamp(0.0, 1.0);

        // Aim: lean toward where the harmony sits on the circle of fifths.
        let k = ease(dt, AIM_TAU_S);
        self.aim_pan += (snapshot.tonnetz_x.clamp(-1.0, 1.0) * gate - self.aim_pan) * k;
        self.aim_tilt += (snapshot.tonnetz_y.clamp(-1.0, 1.0) * gate - self.aim_tilt) * k;

        // Throw: kick on every chord change, sized by how far the root went.
        self.throw *= (-dt / THROW_TAU_S).exp();
        if self.throw.abs() < 0.005 {
            self.throw = 0.0;
        }
        if snapshot.chord_change_env - self.last_change_env > CHANGE_EDGE {
            let side = match snapshot.chord_root_dir {
                d if d > 0 => 1.0,
                d if d < 0 => -1.0,
                _ => {
                    self.flip = !self.flip;
                    if self.flip { 1.0 } else { -1.0 }
                }
            };
            let reach = (0.45 + 0.11 * snapshot.chord_root_motion.min(6) as f32).min(1.0);
            // A fast progression would otherwise hammer the heads — back the
            // kick off as the harmonic rhythm climbs.
            let calm = 1.0 - 0.5 * snapshot.harmonic_rhythm.clamp(0.0, 1.0);
            self.throw = side * reach * calm * gate * settle(SETTLED_THROW, lift);
        }
        self.last_change_env = snapshot.chord_change_env;

        // Gather: tension pulls the beams in; the drop lets go at once.
        let released = arc == ArcState::Drop || snapshot.drop_impact > 0.5;
        let want = if released {
            0.0
        } else {
            let build = if matches!(arc, ArcState::Buildup | ArcState::PreDropTension) { 0.8 } else { 0.0 };
            let chord = match snapshot.chord_quality {
                3 => 0.30, // dominant: leaning forward
                4 => 0.40, // diminished: unstable
                _ => 0.0,
            } * gate;
            smoothstep(0.30, 0.62, snapshot.tension_bank).max(build).max(chord)
        };
        let tau = if want > self.gather { GATHER_ATTACK_TAU_S } else { GATHER_RELEASE_TAU_S };
        self.gather += (want - self.gather) * ease(dt, tau);
        if self.gather < 0.002 && want == 0.0 {
            self.gather = 0.0;
        }

        self.frame()
    }

    /// The current state as a frame, without advancing.
    pub fn frame(&self) -> MovementFrame {
        let p = self.progress.clamp(0.0, 1.0);
        MovementFrame {
            look: self.look,
            prev: self.prev,
            blend: p * p * (3.0 - 2.0 * p),
            aim_pan: self.aim_pan,
            aim_tilt: self.aim_tilt,
            throw: self.throw,
            gather: self.gather,
            bar_s: self.bar_s,
            level: (self.level_even + self.level_odd) * 0.5,
            split: (self.level_even - self.level_odd) * 0.5,
            leveled: self.primed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emerge::intent::fixture_brain::{PhaseState, ShowClockSnapshot};

    const DT: f32 = 1.0 / 60.0;

    /// A playing, coherent, fully-licensed snapshot.
    fn playing() -> CueSnapshot {
        CueSnapshot {
            energy: 0.7,
            energy_slow: 0.7,
            trust: 1.0,
            intensity_license: 1.0,
            ..CueSnapshot::default()
        }
    }

    /// Oscillator frame locked to an explicit beat (4/4, 32-beat phrases).
    fn osc_at(snapshot: &CueSnapshot, beat: f64) -> OscillatorFrameSnapshot {
        let clock = ShowClockSnapshot { beat, quantum: 32.0, ..ShowClockSnapshot::default() };
        OscillatorFrameSnapshot::from_clock(snapshot, PhaseState::from_show_clock(clock))
    }

    fn slots(count: usize) -> Vec<MoverSlot> {
        (0..count).map(|i| MoverSlot::new(i, count, i as f32 / count as f32)).collect()
    }

    const FIGURES: [MoveFigure; 8] = [
        MoveFigure::Circle,
        MoveFigure::FigureEight,
        MoveFigure::Fan,
        MoveFigure::Wave,
        MoveFigure::Scissor,
        MoveFigure::Bounce,
        MoveFigure::Ballyhoo,
        MoveFigure::Gather,
    ];

    #[test]
    fn default_frame_is_the_legacy_sweep_untouched() {
        let frame = MovementFrame::default();
        assert!(frame.is_legacy());
        let s = playing();
        let osc = osc_at(&s, 17.3);
        let ctx = MotionContext { legacy: 0.4321, center: 0.5, motion: 1.0, slot: MoverSlot::new(0, 2, 0.0), mount_tilt_deg: 0.0, rest: 0.5 };
        assert_eq!(frame.pan(&s, &osc, ctx), 0.4321, "pan passes through bit-for-bit");
        assert_eq!(frame.tilt(&s, &osc, ctx), 0.4321, "tilt passes through bit-for-bit");
    }

    #[test]
    fn mover_slots_spread_evenly_and_alternate() {
        let four = slots(4);
        for (got, want) in four.iter().map(|s| s.spread).zip([-1.0, -1.0 / 3.0, 1.0 / 3.0, 1.0]) {
            assert!((got - want).abs() < 1e-6, "even spread: {got} vs {want}");
        }
        assert_eq!(four.iter().map(|s| s.mirror).collect::<Vec<_>>(), vec![1.0, -1.0, 1.0, -1.0]);
        // A lone mover sits in the middle; a pair takes the two ends.
        assert_eq!(MoverSlot::new(0, 1, 0.0).spread, 0.0);
        assert_eq!(slots(2).iter().map(|s| s.spread).collect::<Vec<_>>(), vec![-1.0, 1.0]);
    }

    #[test]
    fn every_figure_stays_in_bounds_and_closes_its_loop() {
        let s = playing();
        for figure in FIGURES {
            let look = MoveLook::new(figure, 2.0);
            let cycle_beats = 2.0 * 4.0;
            for slot in slots(4) {
                let mut prev = figure_point(look, &osc_at(&s, 64.0), slot);
                let mut biggest_step = 0.0f32;
                // One full cycle at 60 fps and 128 BPM.
                let frames = (cycle_beats / (128.0 / 60.0 * DT as f64)) as usize;
                for f in 1..=frames {
                    let beat = 64.0 + cycle_beats * f as f64 / frames as f64;
                    let pt = figure_point(look, &osc_at(&s, beat), slot);
                    assert!(pt.pan.abs() <= 1.05 && pt.tilt.abs() <= 1.05, "{figure:?} in bounds: {pt:?}");
                    let step = (pt.pan - prev.pan).abs().max((pt.tilt - prev.tilt).abs());
                    biggest_step = biggest_step.max(step);
                    prev = pt;
                }
                // Bounce is the steepest figure (it reverses on the beat).
                let limit = if figure == MoveFigure::Bounce { 0.20 } else { 0.12 };
                assert!(biggest_step < limit, "{figure:?} moves smoothly: {biggest_step}");
                // Back where it started after exactly one cycle.
                let start = figure_point(look, &osc_at(&s, 64.0), slot);
                let end = figure_point(look, &osc_at(&s, 64.0 + cycle_beats), slot);
                assert!((start.pan - end.pan).abs() < 1e-3, "{figure:?} pan closes");
                assert!((start.tilt - end.tilt).abs() < 1e-3, "{figure:?} tilt closes");
            }
        }
    }

    #[test]
    fn figures_spread_the_heads_apart() {
        let s = playing();
        let osc = osc_at(&s, 70.5);
        for figure in FIGURES {
            let pts: Vec<FigurePoint> =
                slots(4).into_iter().map(|slot| figure_point(MoveLook::new(figure, 4.0), &osc, slot)).collect();
            let distinct = pts.iter().enumerate().any(|(i, a)| {
                pts.iter().skip(i + 1).any(|b| (a.pan - b.pan).abs() > 0.05 || (a.tilt - b.tilt).abs() > 0.05)
            });
            assert!(distinct, "{figure:?} must not move every head as one: {pts:?}");
        }
    }

    #[test]
    fn scissor_heads_cross_and_circle_heads_counter_rotate() {
        let s = playing();
        // Scissor: neighbours are mirror images in pan at every instant.
        for beat in [64.3, 66.0, 69.7] {
            let osc = osc_at(&s, beat);
            let a = figure_point(MoveLook::new(MoveFigure::Scissor, 2.0), &osc, MoverSlot::new(0, 2, 0.0));
            let b = figure_point(MoveLook::new(MoveFigure::Scissor, 2.0), &osc, MoverSlot::new(1, 2, 0.0));
            assert!((a.pan + b.pan).abs() < 1e-5, "scissor pair mirrors");
        }
        // Circle: with the same phase, the mirrored head turns the other way.
        let look = MoveLook::new(MoveFigure::Circle, 2.0);
        let turn = |slot: MoverSlot| {
            let p0 = figure_point(look, &osc_at(&s, 64.0), slot);
            let p1 = figure_point(look, &osc_at(&s, 64.2), slot);
            p0.pan * p1.tilt - p0.tilt * p1.pan // z of the cross product = sense of rotation
        };
        assert!(turn(MoverSlot { phase: 0.0, spread: 0.0, mirror: 1.0 }) > 0.0);
        assert!(turn(MoverSlot { phase: 0.0, spread: 0.0, mirror: -1.0 }) < 0.0);
    }

    #[test]
    fn bounce_dips_on_the_beat_and_recovers() {
        let s = playing();
        let look = MoveLook::new(MoveFigure::Bounce, 2.0);
        let slot = MoverSlot::new(0, 2, 0.0);
        let on = figure_point(look, &osc_at(&s, 64.01), slot).tilt;
        let mid = figure_point(look, &osc_at(&s, 64.5), slot).tilt;
        assert!(on < -0.6, "at the floor on the beat: {on}");
        assert!(mid > 0.3, "back at the top between beats: {mid}");
        // It arrives at the floor and leaves it — no snap across the beat.
        let before = figure_point(look, &osc_at(&s, 64.99), slot).tilt;
        assert!((before - on).abs() < 0.05, "continuous through the beat: {before} vs {on}");
        // The mirrored head takes the off-beat.
        let other = MoverSlot::new(1, 2, 0.5);
        assert!(figure_point(look, &osc_at(&s, 64.51), other).tilt < -0.6);
    }

    #[test]
    fn peak_rates_are_the_real_steepest_slope_of_each_figure() {
        // Numeric derivative of every figure against the analytic table the
        // speed cap trusts: never steeper than the table says (or the cap
        // under-protects), and not far below it (or the cap wastes range).
        let s = playing();
        const STEPS: usize = 4000;
        for figure in FIGURES {
            for bars in [2.0f32, 4.0] {
                let look = MoveLook::new(figure, bars);
                let (pan_rate, tilt_rate) = peak_rates(look);
                let (mut pan_seen, mut tilt_seen) = (0.0f32, 0.0f32);
                for slot in slots(4) {
                    let cycle_beats = bars as f64 * 4.0;
                    let at = |i: usize| {
                        // Offset off the beat grid so no sample sits on a corner.
                        let beat = 64.0 + cycle_beats * (i as f64 + 0.37) / STEPS as f64;
                        figure_point(look, &osc_at(&s, beat), slot)
                    };
                    let d_bars = bars / STEPS as f32;
                    for i in 0..STEPS {
                        let (a, b) = (at(i), at(i + 1));
                        pan_seen = pan_seen.max((b.pan - a.pan).abs() / d_bars);
                        tilt_seen = tilt_seen.max((b.tilt - a.tilt).abs() / d_bars);
                    }
                }
                for (axis, seen, table) in [("pan", pan_seen, pan_rate), ("tilt", tilt_seen, tilt_rate)] {
                    assert!(
                        seen <= table * 1.02 + 1e-3,
                        "{figure:?} {bars} bars {axis}: steeper than the table ({seen} > {table})"
                    );
                    assert!(
                        seen >= table * 0.70 - 1e-3,
                        "{figure:?} {bars} bars {axis}: table far too loose ({seen} vs {table})"
                    );
                }
            }
        }
    }

    #[test]
    fn no_look_asks_a_head_to_outrun_itself_at_any_tempo() {
        // Worst case on every multiplier: full energy, the drop's size pop, a
        // chord throw's size pop, full motion depth, full license.
        let s = CueSnapshot { energy: 1.0, drop_impact: 1.0, ..playing() };
        let arcs = [
            ArcState::Groove,
            ArcState::Buildup,
            ArcState::Drop,
            ArcState::SustainPeak,
            ArcState::Cooldown,
        ];
        let mut capped = 0;
        for tempo in [90.0f32, 128.0, 150.0, 174.0] {
            let bar_s = 240.0 / tempo;
            for arc in arcs {
                for minor in [false, true] {
                    for busy in [0.0f32, 0.9] {
                        let mood = CueSnapshot {
                            major_smooth: if minor { 0.2 } else { 0.8 },
                            harmonic_rhythm: busy,
                            ..s.clone()
                        };
                        for rotation in 0..4 {
                            let look = choose_look(arc, &mood, rotation);
                            if look.is_sweep() {
                                continue;
                            }
                            let frame = MovementFrame { look, prev: look, throw: 1.0, bar_s, ..MovementFrame::default() };
                            let free = MovementFrame { bar_s: 0.0, ..frame };
                            let cycle_beats = look.bars as f64 * 4.0;
                            const STEPS: usize = 2000;
                            let dt = look.bars * bar_s / STEPS as f32;
                            for slot in slots(4) {
                                let ctx = MotionContext { legacy: 0.5, center: 0.5, motion: 1.0, slot, mount_tilt_deg: 0.0, rest: 0.5 };
                                let at = |f: &MovementFrame, i: usize| {
                                    let beat = 64.0 + cycle_beats * (i as f64 + 0.37) / STEPS as f64;
                                    let osc = osc_at(&mood, beat);
                                    (f.pan(&mood, &osc, ctx), f.tilt(&mood, &osc, ctx))
                                };
                                let (mut pan_v, mut tilt_v, mut free_v) = (0.0f32, 0.0f32, 0.0f32);
                                for i in 0..STEPS {
                                    let (a, b) = (at(&frame, i), at(&frame, i + 1));
                                    pan_v = pan_v.max((b.0 - a.0).abs() / dt);
                                    tilt_v = tilt_v.max((b.1 - a.1).abs() / dt);
                                    let (fa, fb) = (at(&free, i), at(&free, i + 1));
                                    free_v = free_v.max((fb.0 - fa.0).abs() / dt);
                                }
                                assert!(
                                    pan_v <= HEAD_PAN_SPEED * 1.03,
                                    "{look:?} at {tempo} BPM pans {pan_v} ranges/s"
                                );
                                assert!(
                                    tilt_v <= HEAD_TILT_SPEED * 1.03,
                                    "{look:?} at {tempo} BPM tilts {tilt_v} ranges/s"
                                );
                                if free_v > HEAD_PAN_SPEED * 1.03 {
                                    capped += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(capped > 0, "the cap is doing real work: uncapped looks do outrun a head");

        // …without flattening the slow looks: a four-bar circle at 128 BPM
        // still swings a third of the pan range or more.
        let look = MoveLook::new(MoveFigure::Circle, 4.0);
        let frame = MovementFrame { look, prev: look, bar_s: 240.0 / 128.0, ..MovementFrame::default() };
        let ctx = MotionContext { legacy: 0.5, center: 0.5, motion: 1.0, slot: MoverSlot::new(0, 4, 0.0), mount_tilt_deg: 0.0, rest: 0.5 };
        let pans: Vec<f32> = (0..64)
            .map(|i| frame.pan(&playing(), &osc_at(&playing(), 64.0 + i as f64 * 0.25), ctx))
            .collect();
        let swing = pans.iter().cloned().fold(0.0, f32::max) - pans.iter().cloned().fold(1.0, f32::min);
        assert!(swing > 0.33, "a slow circle stays wide: {swing}");
    }

    #[test]
    fn the_director_carries_the_tempo_for_the_speed_cap() {
        let mut d = MovementDirector::new();
        let s = playing();
        assert_eq!(d.frame().bar_s, 0.0, "no tempo yet: no cap");
        assert!((d.tick(&s, ArcState::Groove, 1.0, 120.0, DT).bar_s - 2.0).abs() < 1e-6);
        assert_eq!(d.tick(&s, ArcState::Groove, 1.1, 0.0, DT).bar_s, 0.0, "unknown tempo: no cap");
    }

    #[test]
    fn figures_go_still_in_silence() {
        let quiet = CueSnapshot { energy_slow: 0.0, trust: 1.0, intensity_license: 1.0, ..CueSnapshot::default() };
        let frame = MovementFrame {
            look: MoveLook::new(MoveFigure::Ballyhoo, 2.0),
            prev: MoveLook::new(MoveFigure::Ballyhoo, 2.0),
            aim_pan: 0.8,
            throw: 1.0,
            ..MovementFrame::default()
        };
        let ctx = MotionContext { legacy: 0.5, center: 0.5, motion: 1.0, slot: MoverSlot::new(0, 2, 0.0), mount_tilt_deg: 0.0, rest: 0.5 };
        for beat in [64.0, 65.3, 67.9] {
            let osc = osc_at(&quiet, beat);
            assert_eq!(frame.pan(&quiet, &osc, ctx), 0.5, "no music, no motion");
            assert_eq!(frame.tilt(&quiet, &osc, ctx), 0.5);
        }
    }

    #[test]
    fn low_license_hands_the_movers_back_to_the_scan() {
        let frame = MovementFrame {
            look: MoveLook::new(MoveFigure::Circle, 2.0),
            prev: MoveLook::new(MoveFigure::Circle, 2.0),
            ..MovementFrame::default()
        };
        let ctx = MotionContext { legacy: 0.62, center: 0.5, motion: 1.0, slot: MoverSlot::new(0, 2, 0.0), mount_tilt_deg: 0.0, rest: 0.5 };
        let chill = CueSnapshot { intensity_license: 0.1, ..playing() };
        let osc = osc_at(&chill, 65.0);
        assert_eq!(frame.pan(&chill, &osc, ctx), 0.62, "below the license band the scan is untouched");
        let full = playing();
        let osc = osc_at(&full, 65.0);
        assert!((frame.pan(&full, &osc, ctx) - 0.62).abs() > 0.02, "full license runs the figure");
    }

    #[test]
    fn crossfade_from_the_sweep_starts_exactly_on_the_legacy_value() {
        let s = playing();
        let osc = osc_at(&s, 65.0);
        let ctx = MotionContext { legacy: 0.37, center: 0.5, motion: 1.0, slot: MoverSlot::new(1, 4, 0.25), mount_tilt_deg: 0.0, rest: 0.5 };
        let mut frame = MovementFrame {
            look: MoveLook::new(MoveFigure::FigureEight, 4.0),
            prev: MoveLook::SWEEP,
            blend: 0.0,
            ..MovementFrame::default()
        };
        assert!((frame.pan(&s, &osc, ctx) - 0.37).abs() < 1e-6, "blend 0 = the sweep");
        frame.blend = 1.0;
        let full = frame.pan(&s, &osc, ctx);
        frame.blend = 0.5;
        let half = frame.pan(&s, &osc, ctx);
        assert!((half - (0.37 + full) * 0.5).abs() < 1e-5, "blend is linear between the looks");
    }

    #[test]
    fn gather_squeezes_the_figure_and_lifts_the_beams() {
        let s = playing();
        let osc = osc_at(&s, 65.0);
        let slot = MoverSlot::new(0, 4, 0.0);
        let ctx = MotionContext { legacy: 0.5, center: 0.5, motion: 1.0, slot, mount_tilt_deg: 0.0, rest: 0.5 };
        let look = MoveLook::new(MoveFigure::Circle, 2.0);
        let open = MovementFrame { look, prev: look, ..MovementFrame::default() };
        let tight = MovementFrame { gather: 1.0, ..open };
        let reach = |f: &MovementFrame| (f.pan(&s, &osc, ctx) - 0.5).abs();
        assert!(reach(&tight) < reach(&open) * 0.45, "tension closes the figure down");
        // Tilt: same figure point, smaller — but lifted.
        let quiet_point = MovementFrame { look: MoveLook::new(MoveFigure::Gather, 2.0), ..open };
        let lifted = MovementFrame { gather: 1.0, ..quiet_point };
        assert!(lifted.tilt(&s, &osc, ctx) > quiet_point.tilt(&s, &osc, ctx), "and lifts the beams");
    }

    #[test]
    fn harmonic_aim_and_throw_lean_the_rig() {
        let s = playing();
        let osc = osc_at(&s, 65.0);
        let ctx = MotionContext { legacy: 0.5, center: 0.5, motion: 1.0, slot: MoverSlot::new(0, 2, 0.0), mount_tilt_deg: 0.0, rest: 0.5 };
        let base = MovementFrame::default();
        let sharp = MovementFrame { aim_pan: 1.0, ..base };
        let flat = MovementFrame { aim_pan: -1.0, ..base };
        assert!(sharp.pan(&s, &osc, ctx) > 0.5 && flat.pan(&s, &osc, ctx) < 0.5, "aim leans the sweep too");
        let up = MovementFrame { aim_tilt: 1.0, ..base };
        assert!(up.tilt(&s, &osc, ctx) > 0.5);
        let kick = MovementFrame { throw: 1.0, ..base };
        assert!((kick.pan(&s, &osc, ctx) - 0.5 - THROW_PAN_DEPTH).abs() < 1e-4, "a full throw kicks its depth");
    }

    // ── Director ────────────────────────────────────────────────────────────

    /// Tick `seconds` of music at 128 BPM starting from `beat`; returns the
    /// last frame and the beat reached.
    fn run(
        d: &mut MovementDirector,
        s: &CueSnapshot,
        arc: ArcState,
        mut beat: f64,
        seconds: f32,
    ) -> (MovementFrame, f64) {
        let mut frame = d.frame();
        for _ in 0..(seconds / DT) as usize {
            beat += 128.0 / 60.0 * DT as f64;
            frame = d.tick(s, arc, beat, 128.0, DT);
        }
        (frame, beat)
    }

    // ── Beam levels ─────────────────────────────────────────────────────────

    /// The tilt a head rigged at `mount` holds for a level when nothing is
    /// swinging it (no motion depth): the level itself.
    fn tilt_at(level: f32, split: f32, mount: f32, ordinal: usize) -> f32 {
        let s = playing();
        let osc = osc_at(&s, 65.0);
        let frame = MovementFrame { level, split, leveled: true, ..MovementFrame::default() };
        let ctx = MotionContext {
            legacy: 0.5,
            center: 0.5,
            motion: 0.0,
            slot: MoverSlot::new(ordinal, 4, 0.0),
            mount_tilt_deg: mount,
            rest: 0.5,
        };
        frame.tilt(&s, &osc, ctx)
    }

    /// World elevation (degrees from straight down) that tilt value means for
    /// a head rigged at `mount`.
    fn elevation(tilt: f32, mount: f32) -> f32 {
        mount + (tilt - 0.5) * MOVER_TILT_WINDOW_DEG
    }

    #[test]
    fn levels_climb_from_the_placed_aim_to_the_sky() {
        // Down-firing mounts: straight down, a GalaxyJet, the concert 7Rs, a
        // steep front truss. The floor is the spot the operator placed.
        for mount in [0.0f32, 25.0, 42.5, 80.0] {
            let floor = tilt_at(0.0, 0.0, mount, 0);
            let crowd = tilt_at(0.5, 0.0, mount, 0);
            let sky = tilt_at(1.0, 0.0, mount, 0);
            assert_eq!(floor, 0.5, "mount {mount}: the floor is the placed aim, untouched");
            assert!(crowd > floor + 0.08 && sky > crowd + 0.08, "mount {mount}: {floor} < {crowd} < {sky}");
            assert!(sky <= 1.0 - STOP_MARGIN + 1e-6, "mount {mount}: turns back just short of the stop");
        }
        // A head hung to light the stage flies its sky at the very top of its
        // half-sphere: floor to sky is the beam going bottom to top.
        for mount in [0.0f32, 14.7, 25.0, 40.0] {
            let sky = tilt_at(1.0, 0.0, mount, 0);
            assert!((sky - (1.0 - STOP_MARGIN)).abs() < 1e-6, "mount {mount}: sky at {sky}");
            let travel = (sky - tilt_at(0.0, 0.0, mount, 0)) * MOVER_TILT_WINDOW_DEG;
            assert!(travel > 85.0, "mount {mount}: floor to sky is {travel}° of tilt");
        }
        // The concert 7R (aimed 42.5° off straight down) really leaves the
        // floor: the crowd level rakes out under the horizon, and the sky
        // centre is above it — before the figure's own lift and swing.
        let crowd = elevation(tilt_at(0.5, 0.0, 42.5, 0), 42.5);
        assert!((60.0..90.0).contains(&crowd), "crowd level: {crowd}°");
        assert!(elevation(tilt_at(1.0, 0.0, 42.5, 0), 42.5) > 90.0);
        // A head hung nearer the horizon gets all the way to SKY_DEG.
        assert!((elevation(tilt_at(1.0, 0.0, 80.0, 0), 80.0) - SKY_DEG).abs() < 0.5);
    }

    #[test]
    fn heads_rigged_to_fire_up_stay_above_the_horizon() {
        for mount in [95.0f32, 118.0, 150.0, 180.0] {
            for level in [0.0f32, 0.5, 1.0] {
                let e = elevation(tilt_at(level, 0.0, mount, 0), mount);
                assert!(e >= 94.9, "mount {mount} level {level}: never down into the deck ({e}°)");
            }
            let (floor, sky) = (tilt_at(0.0, 0.0, mount, 0), tilt_at(1.0, 0.0, mount, 0));
            assert!(sky >= floor, "mount {mount}: sky is not below floor");
        }
        // An aerial placed at 118° comes down to a rake for the floor level
        // and stands straight up for the sky.
        assert!(tilt_at(0.0, 0.0, 118.0, 0) < 0.5 && tilt_at(1.0, 0.0, 118.0, 0) > 0.5);
        assert!((elevation(tilt_at(0.0, 0.0, 118.0, 0), 118.0) - RAKE_DEG).abs() < 0.5);
        assert!((elevation(tilt_at(1.0, 0.0, 118.0, 0), 118.0) - ZENITH_DEG).abs() < 0.5);
        // Garbage in the patch never becomes a NaN on the wire.
        assert!(tilt_at(1.0, 0.0, f32::NAN, 0).is_finite());
    }

    #[test]
    fn split_sends_alternate_heads_opposite_ways() {
        let even = tilt_at(0.5, 0.5, 42.5, 0);
        let odd = tilt_at(0.5, 0.5, 42.5, 1);
        assert_eq!(even, tilt_at(1.0, 0.0, 42.5, 0), "even heads are in the sky");
        assert_eq!(odd, tilt_at(0.0, 0.0, 42.5, 1), "odd heads are on the floor");
        assert!(even - odd > 0.25, "a tall X: {even} vs {odd}");
    }

    #[test]
    fn a_level_holds_through_a_quiet_bar_but_yields_to_the_scan() {
        // No energy gate: a head parked in the sky does not sag when the kick
        // drops out for a bar…
        let frame = MovementFrame { level: 1.0, leveled: true, ..MovementFrame::default() };
        let ctx = |legacy: f32| MotionContext {
            legacy,
            center: 0.5,
            motion: 1.0,
            slot: MoverSlot::new(0, 2, 0.0),
            mount_tilt_deg: 42.5,
            rest: 0.5,
        };
        let hush = CueSnapshot { energy: 0.0, energy_slow: 0.0, ..playing() };
        assert!(frame.tilt(&hush, &osc_at(&hush, 65.0), ctx(0.5)) > 0.75, "holds its level");
        // …but an unlicensed (atonal, chill) room gets the slow scan back.
        let chill = CueSnapshot { intensity_license: 0.1, ..playing() };
        assert_eq!(frame.tilt(&chill, &osc_at(&chill, 65.0), ctx(0.62)), 0.62);
        // And with no director at all the level fields are inert.
        let inert = MovementFrame { level: 1.0, split: 0.5, ..MovementFrame::default() };
        assert!(inert.is_legacy());
        assert_eq!(inert.tilt(&playing(), &osc_at(&playing(), 65.0), ctx(0.4321)), 0.4321);
    }

    #[test]
    fn no_figure_swings_an_up_firing_head_below_the_horizon() {
        // The level keeps a head rigged to fire up over the horizon; the
        // bottom of a deep figure, a downward harmonic aim and the quiet scan
        // all have to respect it too.
        let figures = [
            MoveFigure::Sweep,
            MoveFigure::Circle,
            MoveFigure::FigureEight,
            MoveFigure::Fan,
            MoveFigure::Wave,
            MoveFigure::Scissor,
            MoveFigure::Bounce,
            MoveFigure::Ballyhoo,
            MoveFigure::Gather,
        ];
        for license in [1.0f32, 0.5, 0.0] {
            let s = CueSnapshot { energy: 1.0, intensity_license: license, ..playing() };
            for mount in [101.0f32, 118.0, 122.7, 150.0, 180.0] {
                for figure in figures {
                    let look = MoveLook::new(figure, 4.0);
                    for (level, split) in [(0.0f32, 0.0f32), (0.5, 0.0), (1.0, 0.0), (0.5, 0.5)] {
                        let frame = MovementFrame {
                            look,
                            prev: look,
                            level,
                            split,
                            aim_tilt: -1.0,
                            leveled: true,
                            ..MovementFrame::default()
                        };
                        for ordinal in 0..2 {
                            for i in 0..64 {
                                let ctx = MotionContext {
                                    // A deep quiet-time scan around the placed aim.
                                    legacy: 0.5 + 0.3 * (i as f32 * 0.4).sin(),
                                    center: 0.5,
                                    motion: 1.3,
                                    slot: MoverSlot::new(ordinal, 4, 0.0),
                                    mount_tilt_deg: mount,
                                    rest: 0.5,
                                };
                                let tilt = frame.tilt(&s, &osc_at(&s, 64.0 + i as f64 * 0.25), ctx);
                                let e = elevation(tilt, mount);
                                assert!(
                                    e >= 89.99,
                                    "{figure:?} on a head placed at {mount}°, level {level}, license {license}: {e}°"
                                );
                            }
                        }
                    }
                }
            }
        }
        // A hung head is not held to it — the floor is where its beams land.
        let s = CueSnapshot { energy: 1.0, ..playing() };
        let look = MoveLook::new(MoveFigure::Ballyhoo, 4.0);
        let frame = MovementFrame { look, prev: look, leveled: true, ..MovementFrame::default() };
        let ctx = MotionContext {
            legacy: 0.5,
            center: 0.5,
            motion: 1.3,
            slot: MoverSlot::new(0, 4, 0.0),
            mount_tilt_deg: 40.0,
            rest: 0.5,
        };
        let lowest = (0..64)
            .map(|i| frame.tilt(&s, &osc_at(&s, 64.0 + i as f64 * 0.25), ctx))
            .fold(1.0, f32::min);
        assert!(lowest < 0.4, "a hung head still swings below its placed spot ({lowest})");
        // And high in the air an up-firing head still has a real figure.
        let sky = MovementFrame { look, prev: look, level: 1.0, leveled: true, ..MovementFrame::default() };
        let up = MotionContext { mount_tilt_deg: 122.7, ..ctx };
        let tilts: Vec<f32> =
            (0..64).map(|i| sky.tilt(&s, &osc_at(&s, 64.0 + i as f64 * 0.25), up)).collect();
        let swing = tilts.iter().cloned().fold(0.0, f32::max) - tilts.iter().cloned().fold(1.0, f32::min);
        assert!(swing > 0.15, "an aerial flown at the sky keeps a wide figure ({swing})");
    }

    #[test]
    fn the_sweep_is_slid_clear_of_a_stop_not_flattened_against_it() {
        // The plain sweep on a head firing up, flown at its floor level: the
        // bottom of the swing would dip under the horizon. It is slid up
        // until it just clears, so the head keeps moving through the whole
        // bar instead of sitting on the horizon for half of it.
        let s = CueSnapshot { energy: 1.0, ..playing() };
        let frame = MovementFrame { leveled: true, ..MovementFrame::default() };
        let mount = 128.4f32;
        let low = 0.5 + (HORIZON_DEG - mount) / MOVER_TILT_WINDOW_DEG;
        let tilts: Vec<f32> = (0..64)
            .map(|i| {
                let osc = osc_at(&s, 64.0 + i as f64 * 0.25);
                // What the intent layer hands over: the sweep's swing around
                // the chase centre, at this energy.
                let swing = FIGURE_TILT_DEPTH * 1.4 * osc.energy_env() * (i as f32 * 0.4).sin();
                let ctx = MotionContext {
                    legacy: 0.5 + swing,
                    center: 0.5,
                    motion: 1.0,
                    slot: MoverSlot::new(0, 3, 0.0),
                    mount_tilt_deg: mount,
                    rest: 0.5,
                };
                frame.tilt(&s, &osc, ctx)
            })
            .collect();
        let parked = tilts.iter().filter(|t| **t <= low + 1e-4 || **t >= 0.999).count();
        assert_eq!(parked, 0, "the sweep sat on a stop for {parked} of 64 samples");
        let swing = tilts.iter().cloned().fold(0.0, f32::max) - tilts.iter().cloned().fold(1.0, f32::min);
        assert!(swing > 0.5, "and it is still a wide sweep ({swing})");
    }

    #[test]
    fn a_figure_keeps_its_shape_at_the_top_of_travel() {
        // A wide circle flown in the sky must not flatten against the stop:
        // across a whole cycle the tilt never sits pinned at the end of
        // travel, and it still moves.
        let s = CueSnapshot { energy: 1.0, ..playing() };
        let look = MoveLook::new(MoveFigure::Circle, 4.0);
        let frame = MovementFrame { look, prev: look, level: 1.0, leveled: true, ..MovementFrame::default() };
        for mount in [0.0f32, 42.5, 118.0] {
            let ctx = MotionContext {
                legacy: 0.62,
                center: 0.62,
                motion: 1.3,
                slot: MoverSlot::new(0, 4, 0.0),
                mount_tilt_deg: mount,
                rest: 0.5,
            };
            let tilts: Vec<f32> =
                (0..128).map(|i| frame.tilt(&s, &osc_at(&s, 64.0 + i as f64 * 0.125), ctx)).collect();
            let pinned = tilts.iter().filter(|t| **t >= 0.999 || **t <= 0.001).count();
            assert_eq!(pinned, 0, "mount {mount}: pinned at the stop for {pinned} of 128 samples");
            let swing = tilts.iter().cloned().fold(0.0, f32::max) - tilts.iter().cloned().fold(1.0, f32::min);
            assert!(swing > 0.1, "mount {mount}: still a figure, not a parked beam ({swing})");
        }
    }

    #[test]
    fn choose_posture_maps_the_arc_to_a_level() {
        let s = CueSnapshot { major_smooth: 0.8, ..playing() };
        assert_eq!(choose_posture(ArcState::Silence, &s, 3), MovePosture::Floor);
        assert_eq!(choose_posture(ArcState::Buildup, &s, 0), MovePosture::Sky, "the build climbs");
        assert_eq!(choose_posture(ArcState::PreDropTension, &s, 7), MovePosture::Sky);
        assert_eq!(choose_posture(ArcState::Drop, &s, 0), MovePosture::Crowd, "the drop throws them out");
        let minor = CueSnapshot { major_smooth: 0.2, ..playing() };
        assert_eq!(choose_posture(ArcState::Groove, &minor, 0), MovePosture::Floor, "minor starts grounded");
        assert_eq!(choose_posture(ArcState::Groove, &s, 0), MovePosture::Crowd, "major starts open");
        // Every step is a visible move, and every pool leaves the floor.
        for mood in [&s, &minor] {
            for arc in [ArcState::SustainPeak, ArcState::Groove, ArcState::Breakdown] {
                let mut seen_up = false;
                for step in 0..6 {
                    let (a, b) = (choose_posture(arc, mood, step), choose_posture(arc, mood, step + 1));
                    assert_ne!(a, b, "{arc:?} never holds a level across a step");
                    seen_up |= a != MovePosture::Floor;
                }
                assert!(seen_up, "{arc:?} gets off the floor");
            }
        }
    }

    #[test]
    fn a_polarity_flip_moves_the_beams_to_a_new_level() {
        let mut d = MovementDirector::new();
        let warm = CueSnapshot { major_smooth: 0.8, polarity_quadrant: 0, ..playing() };
        let cool = CueSnapshot { polarity_quadrant: 2, ..warm.clone() };
        // Settle inside one phrase (beats 64..96) so only the flip can move it.
        let (before, beat) = run(&mut d, &warm, ArcState::Groove, 64.0, 4.0);
        assert!(before.leveled, "a running director flies the levels");
        let (after, beat) = run(&mut d, &cool, ArcState::Groove, beat, 3.0);
        assert!(
            (after.level - before.level).abs() > 0.3 || (after.split - before.split).abs() > 0.3,
            "the flip changed the level: {before:?} -> {after:?}"
        );
        // A flurry of flips inside the hold does not keep the heads in transit:
        // the second flip, a moment after the first, is ignored.
        let mut flurry = MovementDirector::new();
        let (_, b) = run(&mut flurry, &warm, ArcState::Groove, 64.0, 4.0);
        let (_, b) = run(&mut flurry, &cool, ArcState::Groove, b, 0.2);
        let (_, b) = run(&mut flurry, &warm, ArcState::Groove, b, 0.2);
        let (held, _) = run(&mut flurry, &warm, ArcState::Groove, b, 2.5);
        let mut once = MovementDirector::new();
        let (_, b) = run(&mut once, &warm, ArcState::Groove, 64.0, 4.0);
        let (single, _) = run(&mut once, &cool, ArcState::Groove, b, 2.9);
        assert!((held.level - single.level).abs() < 0.02, "one step, not two: {} vs {}", held.level, single.level);
        let _ = beat;
    }

    #[test]
    fn the_build_climbs_and_the_drop_throws_the_beams_out() {
        let mut d = MovementDirector::new();
        let s = playing();
        // Silence parks the rig on the floor.
        let (groove, beat) = run(&mut d, &s, ArcState::Silence, 64.0, 3.0);
        assert!(groove.level < 0.05 && groove.split.abs() < 0.05, "grounded: {groove:?}");
        // Partway into the build the rig is still rising — it climbs with it.
        let (rising, beat) = run(&mut d, &s, ArcState::Buildup, beat, 0.6);
        assert!(rising.level > 0.1 && rising.level < 0.7, "mid-climb: {}", rising.level);
        let (top, beat) = run(&mut d, &s, ArcState::Buildup, beat, 4.0);
        assert!(top.level > 0.9, "in the sky by the end of the build: {}", top.level);
        // The drop comes down to the crowd line fast.
        let (drop, _) = run(&mut d, &s, ArcState::Drop, beat, 1.0);
        assert!((drop.level - 0.5).abs() < 0.05, "out over the room: {}", drop.level);
    }

    #[test]
    fn level_changes_never_outrun_a_head() {
        // Walk the director through the hardest moves — the build, the drop,
        // a flip into a split — and check every head's tilt against the head's
        // top speed, for every kind of mount and the deepest legacy centre
        // (which gives the widest band a head can have).
        let mut d = MovementDirector::new();
        let warm = CueSnapshot { major_smooth: 0.2, polarity_quadrant: 0, ..playing() };
        let cool = CueSnapshot { polarity_quadrant: 2, ..warm.clone() };
        let script: [(&CueSnapshot, ArcState, f32); 6] = [
            (&warm, ArcState::Groove, 3.0),
            (&warm, ArcState::Buildup, 4.0),
            (&warm, ArcState::Drop, 2.0),
            (&warm, ArcState::SustainPeak, 3.0),
            (&cool, ArcState::SustainPeak, 3.0),
            (&warm, ArcState::Groove, 3.0),
        ];
        let home = |f: &MovementFrame, ordinal: usize, mount: f32, center: f32| {
            f.level_shift(&MotionContext {
                legacy: center,
                center,
                motion: 1.0,
                slot: MoverSlot::new(ordinal, 4, 0.0),
                mount_tilt_deg: mount,
                rest: center,
            })
        };
        let mut beat = 64.0;
        let mut prev = d.tick(&warm, ArcState::Groove, beat, 128.0, DT);
        let (mut fastest, mut moved) = (0.0f32, 0.0f32);
        for (s, arc, seconds) in script {
            for _ in 0..(seconds / DT) as usize {
                beat += 128.0 / 60.0 * DT as f64;
                let f = d.tick(s, arc, beat, 128.0, DT);
                for mount in [0.0f32, 42.5, 118.0, 180.0] {
                    for center in [0.30f32, 0.50, 0.62] {
                        for ordinal in 0..2 {
                            let step =
                                (home(&f, ordinal, mount, center) - home(&prev, ordinal, mount, center)).abs();
                            fastest = fastest.max(step / DT);
                            moved += step;
                        }
                    }
                }
                prev = f;
            }
        }
        assert!(fastest <= HEAD_TILT_SPEED, "a level change asks {fastest} ranges/s of a head");
        assert!(moved > 10.0, "…and the heads really travelled: {moved}");
    }

    #[test]
    fn choose_look_maps_the_arc_to_a_vocabulary() {
        let s = playing();
        assert_eq!(choose_look(ArcState::Silence, &s, 0), MoveLook::SWEEP);
        assert_eq!(choose_look(ArcState::Intro, &s, 3), MoveLook::SWEEP);
        assert_eq!(choose_look(ArcState::Buildup, &s, 0).figure, MoveFigure::Gather);
        assert_eq!(choose_look(ArcState::PreDropTension, &s, 5).figure, MoveFigure::Gather);
        assert_eq!(choose_look(ArcState::Drop, &s, 0).figure, MoveFigure::Ballyhoo);
        // The peak and the groove walk a pool: consecutive phrases differ.
        for arc in [ArcState::SustainPeak, ArcState::Groove, ArcState::Breakdown] {
            for r in 0..6 {
                assert_ne!(choose_look(arc, &s, r), choose_look(arc, &s, r + 1), "{arc:?} never repeats back-to-back");
            }
        }
    }

    #[test]
    fn tonality_picks_the_groove_family() {
        let major = CueSnapshot { major_smooth: 0.8, ..playing() };
        let minor = CueSnapshot { major_smooth: 0.2, ..playing() };
        assert_eq!(choose_look(ArcState::Groove, &major, 0).figure, MoveFigure::Circle);
        assert_eq!(choose_look(ArcState::Groove, &minor, 0).figure, MoveFigure::FigureEight);
        assert_eq!(choose_look(ArcState::Groove, &major, 1).figure, MoveFigure::Fan);
        assert_eq!(choose_look(ArcState::Groove, &minor, 2).figure, MoveFigure::Scissor);
    }

    #[test]
    fn harmonic_rhythm_sets_the_pace() {
        let steady = playing();
        let busy = CueSnapshot { harmonic_rhythm: 0.9, ..playing() };
        let parked = CueSnapshot { harmonic_rhythm: 0.0, energy_slow: 0.3, ..playing() };
        let bars = |s: &CueSnapshot| choose_look(ArcState::Groove, s, 0).bars;
        assert_eq!(bars(&steady), 4.0);
        assert_eq!(bars(&busy), 2.0, "a fast progression doubles the speed");
        assert_eq!(bars(&parked), 8.0, "a parked chord in a quiet section halves it");
    }

    #[test]
    fn director_holds_a_look_for_the_phrase_then_rotates() {
        let s = playing();
        let mut d = MovementDirector::new();
        let (first, beat) = run(&mut d, &s, ArcState::Groove, 0.0, 1.0);
        assert_eq!(first.look.figure, MoveFigure::Circle, "first groove look");
        assert_eq!(first.blend, 1.0, "the very first look needs no crossfade");
        // Still the same look just before the 32-beat boundary (~15 s at 128).
        let (held, beat) = run(&mut d, &s, ArcState::Groove, beat, 13.0);
        assert_eq!(held.look, first.look, "holds through the phrase (beat {beat})");
        // Across the boundary it rotates, crossfading from the old look.
        let (next, _) = run(&mut d, &s, ArcState::Groove, beat, 1.5);
        assert_eq!(next.look.figure, MoveFigure::Fan, "next look in the pool");
        assert_eq!(next.prev.figure, MoveFigure::Circle);
        assert!(next.blend > 0.0 && next.blend < 1.0, "mid-crossfade: {}", next.blend);
    }

    #[test]
    fn build_gathers_and_the_drop_snaps_open() {
        let mut d = MovementDirector::new();
        let (_, beat) = run(&mut d, &playing(), ArcState::Groove, 0.0, 3.0);
        let building = CueSnapshot { tension_bank: 0.6, ..playing() };
        let (build, beat) = run(&mut d, &building, ArcState::Buildup, beat, 3.0);
        assert_eq!(build.look.figure, MoveFigure::Gather);
        assert!(build.gather > 0.7, "the build pulls the beams in: {}", build.gather);
        // The drop: ballyhoo within a third of a second, gather released.
        let dropped = CueSnapshot { drop_impact: 0.9, ..playing() };
        let (drop, _) = run(&mut d, &dropped, ArcState::Drop, beat, 0.4);
        assert_eq!(drop.look.figure, MoveFigure::Ballyhoo);
        assert_eq!(drop.blend, 1.0, "the drop snaps");
        assert!(drop.gather < 0.15, "and lets go: {}", drop.gather);
    }

    #[test]
    fn a_change_mid_crossfade_waits_but_the_drop_does_not() {
        let s = playing();
        let mut d = MovementDirector::new();
        let (_, beat) = run(&mut d, &s, ArcState::Groove, 0.0, 1.0);
        // Groove → Breakdown starts a 2 s crossfade…
        let (a, beat) = run(&mut d, &s, ArcState::Breakdown, beat, 0.5);
        assert!(a.blend < 1.0);
        // …and flipping straight back does not start a third look mid-fade.
        let (b, beat) = run(&mut d, &s, ArcState::Groove, beat, 0.2);
        assert_eq!(b.look, a.look, "ordinary changes wait for the fade");
        // Once the fade lands, the deferred change goes through on its own —
        // the rig is not stuck on the breakdown look until the next phrase.
        let (settled, beat) = run(&mut d, &s, ArcState::Groove, beat, 2.0);
        assert_eq!(settled.look.figure, MoveFigure::Circle, "deferred change applied after the fade");
        assert_eq!(settled.prev, a.look);
        // The drop interrupts a running fade.
        let (c, _) = run(&mut d, &s, ArcState::Drop, beat, 0.1);
        assert_eq!(c.look.figure, MoveFigure::Ballyhoo, "the drop does not wait");
    }

    #[test]
    fn chord_changes_throw_the_rig_by_harmonic_distance_and_side() {
        let mut d = MovementDirector::new();
        let (_, beat) = run(&mut d, &playing(), ArcState::Groove, 0.0, 1.0);
        // Up a fifth: a small kick sharpward.
        let fifth = CueSnapshot { chord_change_env: 1.0, chord_root_motion: 1, chord_root_dir: 1, ..playing() };
        let near = d.tick(&fifth, ArcState::Groove, beat, 128.0, DT).throw;
        assert!(near > 0.5 && near < 0.6, "one fifth = a modest throw: {near}");
        // It decays while the envelope decays (no re-trigger).
        let fading = CueSnapshot { chord_change_env: 0.9, ..fifth };
        let (later, beat) = run(&mut d, &fading, ArcState::Groove, beat, 1.0);
        assert!(later.throw < near * 0.2, "the kick relaxes: {}", later.throw);
        // A tritone flatward-less leap: the biggest kick.
        let mut d2 = MovementDirector::new();
        run(&mut d2, &playing(), ArcState::Groove, 0.0, 1.0);
        let far = CueSnapshot { chord_change_env: 1.0, chord_root_motion: 6, chord_root_dir: 0, ..playing() };
        let big = d2.tick(&far, ArcState::Groove, beat, 128.0, DT).throw;
        assert!(big.abs() > 0.95, "the tritone throws furthest: {big}");
        // Flatward moves kick the other way.
        let mut d3 = MovementDirector::new();
        run(&mut d3, &playing(), ArcState::Groove, 0.0, 1.0);
        let fourth = CueSnapshot { chord_root_dir: -1, ..fifth };
        assert!(d3.tick(&fourth, ArcState::Groove, beat, 128.0, DT).throw < -0.5);
    }

    #[test]
    fn aim_glides_to_the_harmonic_position_and_ignores_noise() {
        let mut d = MovementDirector::new();
        let there = CueSnapshot { tonnetz_x: 0.6, tonnetz_y: -0.4, ..playing() };
        let one = d.tick(&there, ArcState::Groove, 1.0, 128.0, DT);
        assert!(one.aim_pan > 0.0 && one.aim_pan < 0.1, "glides, does not jump: {}", one.aim_pan);
        let (settled, _) = run(&mut d, &there, ArcState::Groove, 1.0, 4.0);
        assert!((settled.aim_pan - 0.6).abs() < 0.02 && (settled.aim_tilt + 0.4).abs() < 0.02);
        // An incoherent field (trust 0) or a near-silent one has no aim.
        let noise = CueSnapshot { trust: 0.0, ..there };
        let (lost, _) = run(&mut d, &noise, ArcState::Groove, 1.0, 4.0);
        assert!(lost.aim_pan.abs() < 0.02, "no coherent harmony, no aim: {}", lost.aim_pan);
    }

    #[test]
    fn a_tense_chord_gathers_a_little_without_a_build() {
        let mut d = MovementDirector::new();
        let dominant = CueSnapshot { chord_quality: 3, ..playing() };
        let (f, _) = run(&mut d, &dominant, ArcState::Groove, 0.0, 4.0);
        assert!(f.gather > 0.25 && f.gather < 0.35, "dominant leans in: {}", f.gather);
        let resolved = CueSnapshot { chord_quality: 1, ..playing() };
        let (f, _) = run(&mut d, &resolved, ArcState::Groove, 8.0, 1.0);
        assert_eq!(f.gather, 0.0, "and the resolution lets go");
    }

    // ── Dynamics ────────────────────────────────────────────────────────────

    #[test]
    fn full_dynamics_is_the_rig_as_it_was() {
        assert_eq!(dynamics_lift(&CueSnapshot::default()), 1.0);
        assert_eq!(dynamics_lift(&CueSnapshot { dynamics: 0.25, ..playing() }), 0.0);
        let mid = dynamics_lift(&CueSnapshot { dynamics: 0.55, ..playing() });
        assert!(mid > 0.3 && mid < 0.6, "a bass-cut blend opens the rig part way: {mid}");
        assert_eq!(settle(0.4, 1.0), 1.0);
        assert_eq!(settle(0.4, 0.0), 0.4);
    }

    #[test]
    fn a_low_passage_runs_its_figures_at_half_speed() {
        let full = playing();
        let low = CueSnapshot { dynamics: 0.30, ..playing() };
        for arc in [ArcState::Groove, ArcState::Breakdown] {
            for rotation in 0..4 {
                let (at_full, at_low) = (choose_look(arc, &full, rotation), choose_look(arc, &low, rotation));
                assert_eq!(at_full.figure, at_low.figure, "dynamics changes the pace, not the figure");
                if !at_full.is_sweep() {
                    assert_eq!(at_low.bars, at_full.bars * 2.0, "{arc:?} #{rotation}");
                }
            }
        }
        // A busy progression doubles the pace of a full passage, never of a
        // low one.
        let busy = CueSnapshot { harmonic_rhythm: 0.9, ..playing() };
        let busy_low = CueSnapshot { dynamics: 0.30, ..busy.clone() };
        assert!(
            choose_look(ArcState::Groove, &busy_low, 0).bars > choose_look(ArcState::Groove, &busy, 0).bars
        );
    }

    #[test]
    fn a_polarity_flip_in_a_settled_passage_is_held_off_and_then_glides() {
        let flip_after = |dynamics: f32, wait_s: f32| {
            let mut d = MovementDirector::new();
            let warm = CueSnapshot { major_smooth: 0.8, polarity_quadrant: 0, dynamics, ..playing() };
            let cool = CueSnapshot { polarity_quadrant: 2, ..warm.clone() };
            let (before, beat) = run(&mut d, &warm, ArcState::Groove, 64.0, wait_s);
            let (after, _) = run(&mut d, &cool, ArcState::Groove, beat, 1.0);
            (after.level - before.level).abs().max((after.split - before.split).abs())
        };
        // Four seconds after the last level change a full passage takes the
        // flip; a settled one lets it pass.
        assert!(flip_after(1.0, 4.0) > 0.3);
        assert!(flip_after(0.20, 4.0) < 0.02, "a settled passage ignores a flip this soon");
        // Once it is due, the settled rig still turns — but a second in, it
        // has covered well under half of what the full rig has.
        let (settled, full) = (flip_after(0.20, 9.0), flip_after(1.0, 9.0));
        assert!(settled > 0.05, "the settled rig does take the flip: {settled}");
        assert!(settled < full * 0.5, "…slowly: {settled} vs {full}");
    }
}

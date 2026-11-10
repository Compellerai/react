//! Semantic fixture intent layer for REACT lighting.
//!
//! The renderer and real DMX backend should consume the same intent. DMX bytes
//! are an output encoding, not the lighting model. This module keeps one
//! per-frame, oscillator-driven fixture state between CueSnapshot and DMX.

use super::oscillator::{
    add_bipolar, OscillatorBinding, OscillatorFrameSnapshot,
    OscillatorInterval, OscillatorShape,
};
use crate::emerge::contracts::{FixturePatch, FixtureRole, StageVisualKind};
use crate::hue::CueSnapshot;
use crate::render::HeroAct;
use super::fixture_brain::FixtureFamily;
use crate::emerge::cue::show_brain::{family_for, ShowBrainFrame};

#[derive(Clone, Copy, Debug, Default)]
pub struct FixtureIntentState {
    pub dimmer: f32,
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub white: f32,
    /// UV / blacklight emitter level (0..1). Stays 0 outside UV mode — only
    /// `apply_global_overrides` drives it (UV mode: only UV-capable emitters
    /// lit), so a profile's UV channel idles dark on the wire.
    pub uv: f32,
    pub strobe: f32,
    pub pan: f32,
    pub tilt: f32,
    pub zoom: f32,
    pub focus: f32,
    pub color_wheel: u8,
    pub gobo_slot: u8,
    pub gobo_rotation: f32,
    pub prism: f32,
    pub laser_macro: u8,
}

// ── EALP B1: journey-phase lighting macro ───────────────────────────────────
// The emerge HeroAct journey phase (Birth/Lock/Dive/Afterglow) governs lighting
// INTENSITY + MOVEMENT DEPTH, not just the visuals — the patent-canonical "field
// drives all modalities" (Resonator 63960211; HeroAct previously had zero dmx
// consumers). Two scalars applied as multipliers: an intensity scale (uniform
// over dimmer+colour; lasers skip) and a movement-depth scale (the mover sweep).
// Both time-shaped off `journey_phase_time` so the arc breathes: Birth emerges,
// Lock is full, Dive accents on entry then cools, Afterglow fades to a sustained
// floor (never a hard cut). All constants are tunable knobs.
const JOURNEY_BIRTH_RAMP_S: f32 = 8.0; // Birth emergence ramp (seconds)
const JOURNEY_BIRTH_DIMMER_FLOOR: f32 = 0.45; // sparse at t=0
const JOURNEY_BIRTH_DIMMER_CEIL: f32 = 0.90; // held just under full so Lock releases to 1.0
const JOURNEY_BIRTH_MOTION: f32 = 0.50; // narrow movement during emergence
const JOURNEY_DIVE_COOLDOWN_S: f32 = 5.0; // Dive motion accent cools over this
const JOURNEY_DIVE_MOTION_PUNCH: f32 = 0.40; // extra sweep depth on Dive entry
const JOURNEY_AFTERGLOW_FADE_S: f32 = 16.0; // Afterglow fade window
const JOURNEY_AFTERGLOW_DIMMER_FLOOR: f32 = 0.35; // sustain, never black
const JOURNEY_AFTERGLOW_MOTION_FLOOR: f32 = 0.40; // movement relaxes, not stops

/// Intensity multiplier (~0.35..1.0) for the current journey phase. Applied
/// uniformly over dimmer + colour so there's no colour/dimmer skew; lasers skip
/// it (their dimmer is a binary enable gate, like global brightness).
fn journey_dimmer_scale(snapshot: &CueSnapshot) -> f32 {
    let t = snapshot.journey_phase_time.max(0.0);
    match snapshot.journey_phase {
        HeroAct::Birth => {
            let emerge = (t / JOURNEY_BIRTH_RAMP_S).clamp(0.0, 1.0);
            JOURNEY_BIRTH_DIMMER_FLOOR
                + (JOURNEY_BIRTH_DIMMER_CEIL - JOURNEY_BIRTH_DIMMER_FLOOR) * emerge
        }
        HeroAct::Lock => 1.0,
        HeroAct::Dive => 1.0, // full brightness; the Dive accent rides movement, not the dimmer
        HeroAct::Afterglow => {
            let decay = (1.0 - t / JOURNEY_AFTERGLOW_FADE_S).clamp(0.0, 1.0);
            JOURNEY_AFTERGLOW_DIMMER_FLOOR + (1.0 - JOURNEY_AFTERGLOW_DIMMER_FLOOR) * decay
        }
    }
}

/// Movement-depth multiplier (~0.40..1.40) for the current journey phase — scales
/// the mover sweep so Birth is narrow, Lock full, Dive accents on entry then
/// settles to full, Afterglow relaxes to a floor.
fn journey_motion_scale(snapshot: &CueSnapshot) -> f32 {
    let t = snapshot.journey_phase_time.max(0.0);
    match snapshot.journey_phase {
        HeroAct::Birth => JOURNEY_BIRTH_MOTION,
        // Lock = full-depth synchronized movement (the Part A mirror sweep at full
        // depth). v1 is a flat 1.0; keying depth to symmetry / anchor_theta_deg
        // (plan B1 stretch / B4) is deferred per the engineering filter until a
        // larger multi-fixture rig is in play.
        HeroAct::Lock => 1.0,
        HeroAct::Dive => {
            let cool = (1.0 - t / JOURNEY_DIVE_COOLDOWN_S).clamp(0.0, 1.0);
            1.0 + JOURNEY_DIVE_MOTION_PUNCH * cool
        }
        HeroAct::Afterglow => {
            let decay = (1.0 - t / JOURNEY_AFTERGLOW_FADE_S).clamp(0.0, 1.0);
            JOURNEY_AFTERGLOW_MOTION_FLOOR + (1.0 - JOURNEY_AFTERGLOW_MOTION_FLOOR) * decay
        }
    }
}

// ── EALP B2: harmonic coherence (trust) -> saturation + movement ────────────
// trust = smoothstep(tonal_confidence)·smoothstep(stability) (harmonic_field.rs).
// It gates the TONAL-vs-NOISE axis (energy already gates silence): a confident,
// coherent field stays richly saturated and moves at full depth; an atonal/noisy
// field desaturates toward grey and relaxes its movement — the field layer eases
// while ENERGY still drives brightness (`val`), so a noisy techno drop still rips.
// Both floored so coherence loss never fully greys-out or freezes. Tunable knobs.
const TRUST_SAT_FLOOR: f32 = 0.45; // atonal keeps ~45% of its consonance saturation
const TRUST_MOTION_FLOOR: f32 = 0.55; // atonal relaxes movement to ~55% depth

/// Saturation multiplier (~0.45..1.0) from harmonic coherence. Applied on top of
/// the consonance (balance) saturation — a distinct axis (confidence vs density).
fn trust_saturation(snapshot: &CueSnapshot) -> f32 {
    TRUST_SAT_FLOOR + (1.0 - TRUST_SAT_FLOOR) * snapshot.trust.clamp(0.0, 1.0)
}

/// Movement-depth multiplier (~0.55..1.0) from harmonic coherence — composes with
/// the journey motion scale + the energy depth so an atonal field eases its sweep.
fn trust_motion(snapshot: &CueSnapshot) -> f32 {
    TRUST_MOTION_FLOOR + (1.0 - TRUST_MOTION_FLOOR) * snapshot.trust.clamp(0.0, 1.0)
}

// ── EALP B3: quaternion overlay -> accent intensity + colour punch + tracking ─
// qc_strength (0-1, fold24 overlay lock-in) punches BOTH the mover harmonic-snap
// accent (pan throw) AND the colour toward full saturation on lattice events (the
// plan's "accent/color punch"); qc_active_port (0-3) rotates which lattice port
// the mover pair rests on so the heads track the live harmonic anchor. All no-op
// at 0 (= committed Part A behavior). reg_degree_a was evaluated and PARKED: it is
// energy-ring-quantized (jumps by 24 per ring) so it would flicker hue, and the
// stable pitch-class hue is already the lattice anchor in color_for_fixture.
const QC_ACCENT_BOOST: f32 = 0.5; // up to +50% throw when the lattice is fully locked
const QC_SAT_PUNCH: f32 = 0.4; // lattice lock-in lerps colour up to 40% toward full saturation

// ── LED-panel brightness range (200W RGB body). Wide on purpose: dim in quiet,
// full on a peak, so the bar reads diverse instead of "always bright". The
// style ceiling caps the top per posture.
const LED_PANEL_DIM_FLOOR: f32 = 0.30; // quiet-passage level (was a hard 0.74)
const LED_PANEL_DIM_ENERGY: f32 = 0.60; // energy rides it up to full
const LED_PANEL_DIM_PUMP: f32 = 0.12; // bass-pump punch on top

// ── Segment-bar field brightness (segment_rgbw) — the field's intensity arc +
// spatial structure dictate the bar's brightness. All tunable knobs.
const FIELD_TRUST_FLOOR: f32 = 0.5; // atonal field keeps ~half level (energy still gates via dimmer)
const FIELD_QC_PUNCH: f32 = 0.35; // a quaternion lattice lock-in brightens the bar up to +35%
const FIELD_CELL_FLOOR: f32 = 0.12; // cells away from a field band go nearly dark (more contrast)
const FIELD_ACTIVE_LIFT: f32 = 0.2; // the live quaternion port's quarter of the bar lifts

/// The harmonic lattice port a mover rests on: ordinal picks the mirror pair
/// (0/2), qc_active_port rotates the pair so the heads track the live active port.
/// The two heads stay antipodal (differ by 2) — a mirror pair — for any active port.
/// Sweep-depth floor for the low-license SCAN mode (multiplier on the
/// oscillator depth): wide enough to read as a deliberate slow scan, gentle
/// enough for a chill room. The phrase-length sine at this depth spans
/// roughly a quarter of the pan range per head.
const SCAN_MOTION_FLOOR: f32 = 0.8;
/// SCAN-mode tilt posture: beams all the way UP — horizontal, ~90° from the
/// floor, raking out over the room (the classic chill-beam look; Koob).
/// 1.0 = top of REACT's tilt window; flip with the fixture's TiltReverse if a
/// rig hangs the other way.
const SCAN_TILT: f32 = 1.0;

fn mover_port(mover_ordinal: usize, active_port: u8) -> usize {
    ((mover_ordinal % 2) * 2 + (active_port.min(3) as usize)) % 4
}

/// Accent multiplier (1.0..1.5) from quaternion overlay strength — additive over
/// the Part A polarity throw so a strong lattice (fold24 lock-in) snaps harder.
/// qc_strength = 0 -> 1.0 = unchanged Part A baseline.
fn qc_accent_boost(snapshot: &CueSnapshot) -> f32 {
    1.0 + QC_ACCENT_BOOST * snapshot.qc_strength.clamp(0.0, 1.0)
}

// ── EALP B4: symmetry order -> mover phase-grouping width ────────────────────
// "How many fixtures move as one phrase unit." The movers are split into N phase
// groups, N = symmetry clamped to [2, mover_count]; each group sweeps at its own
// phrase offset so a wave of motion chases across the groups. More symmetry =>
// finer grouping (more, smaller groups) on a larger rig — the value scales with
// rig size. On the 2-mover rig this reduces to EXACTLY 0.0 / 0.5 (the Part A
// counter-phase) for ANY symmetry, so it never regresses the committed slice.
fn mover_group_phase(mover_ordinal: usize, mover_count: usize, symmetry: u8) -> f32 {
    if mover_count <= 1 {
        return 0.0;
    }
    let groups = (symmetry as usize).clamp(2, mover_count);
    let group = (mover_ordinal * groups) / mover_count;
    group as f32 / groups as f32
}

// ── EALP B5: Tonnetz voice-leading -> smooth colour drift + surprise pop ─────
// chroma is embedded on the tonal circles upstream (osc/tonnetz.rs); the snapshot
// carries the smooth fifths hue (tonnetz_hue), its tonal confidence
// (tonnetz_strength), and the voice-leading surprise (tonnetz_delta). Here we
// consume them as a gentle harmonic-motion HUE drift (gated by strength so an
// atonal field doesn't smear colour) and a SURPRISE saturation pop on a harmonic
// jump (gated by trust + loudness so it only fires on a real, audible change — a
// distinct trigger from the B3 qc lattice-lock pop; brightness stays energy-owned).
const TONNETZ_HUE_DEPTH: f32 = 0.10; // max hue drift from the harmonic centroid
const TONNETZ_SURPRISE_SAT: f32 = 0.5; // saturation lerp-toward-full on a jump
// Surprise band tuned to centroid deltas (reviewer-measured): a tritone key jump
// (C->F#) ~0.58 and a root-fixed quality change (C-maj->C-min) ~0.8 clear HI and
// pop; a single added/dropped note stays under LO and is ignored. Retune if it
// over/under-fires on the bench.
const TONNETZ_SURPRISE_LO: f32 = 0.15; // surprise threshold band (on tonnetz_delta)
const TONNETZ_SURPRISE_HI: f32 = 0.50;

/// B5 harmonic-surprise amount (0-1): a voice-leading jump, gated by tonal
/// confidence (trust) AND loudness (energy_slow) so it only pops on a real,
/// audible harmonic change — never on silence or noise. delta self-envelopes.
fn tonnetz_surprise(snapshot: &CueSnapshot) -> f32 {
    smoothstep(TONNETZ_SURPRISE_LO, TONNETZ_SURPRISE_HI, snapshot.tonnetz_delta)
        * snapshot.trust.clamp(0.0, 1.0)
        * snapshot.energy_slow.clamp(0.0, 1.0)
}

// ── EALP B5 all-in: explicit chord quality + neo-Riemannian P/R/L accents ────
// The chord layer (osc/chord.rs) adds what the centroid cannot: an explicit
// QUALITY label -> colour temperature, and a P/R/L move -> a chromatic-mediant
// hue jolt. Distinct axes from the centroid's saturation surprise, so they enrich
// rather than pile onto one channel. All no-op when no chord (quality 0, prl_env 0).
// Motion: a harmonic surprise also briefly widens the mover sweep.
const CHORD_TEMP_DEPTH: f32 = 0.35; // chord-quality temperature nudge (feeds hue+white)
const PRL_HUE_KICK_P: f32 = 0.04; // Parallel (mode-flip) hue jolt
const PRL_HUE_KICK_RL: f32 = 0.08; // Relative / Leading-tone chromatic-mediant jolt
const TONNETZ_MOTION_PUNCH: f32 = 0.5; // sweep-depth boost on a harmonic surprise
const TONNETZ_TILT_DEPTH: f32 = 0.10; // smooth harmonic-position tilt drift

/// Chord-quality colour temperature (-1 cool .. +1 warm): major warm, minor cool,
/// dominant warm+tense, diminished slightly cool/unstable. 0 when no chord. Adds to
/// the existing major/minor temperature — reinforcing it and adding dom/dim character.
fn chord_temperature(snapshot: &CueSnapshot) -> f32 {
    match snapshot.chord_quality {
        1 => 0.5,
        2 => -0.5,
        3 => 0.7,
        4 => -0.3,
        _ => 0.0,
    }
}

/// A P/R/L chromatic-mediant hue jolt (size by move type), carried by the decaying
/// prl_env and gated by loudness so it does not fire in a quiet gap.
fn prl_hue_kick(snapshot: &CueSnapshot) -> f32 {
    let amt = match snapshot.chord_prl_move {
        1 => PRL_HUE_KICK_P,
        2 | 3 => PRL_HUE_KICK_RL,
        _ => 0.0,
    };
    amt * snapshot.chord_prl_env * snapshot.energy_slow.clamp(0.0, 1.0)
}

/// Mover-motion factor (>=1.0) from a Tonnetz harmonic surprise — a harmonic jump
/// briefly widens the sweep. Reuses tonnetz_surprise (already trust+loudness gated).
fn tonnetz_motion_punch(snapshot: &CueSnapshot) -> f32 {
    1.0 + tonnetz_surprise(snapshot) * TONNETZ_MOTION_PUNCH
}

/// `mover_ordinal` ranks this fixture among the Accent (mover) fixtures by DMX
/// address (0,1,2,…) and `mover_count` is how many there are. The heads land on
/// opposite harmonic ports (B3) and their sweep phase is grouped by symmetry (B4)
/// so they converse rather than move in lockstep; on a 2-mover rig that grouping
/// is the Part A 0.0/0.5 counter-phase. Non-movers pass `0`/`0` — these only
/// affect the Accent pan/tilt.
pub fn render_fixture_intent(
    snapshot: &CueSnapshot,
    fixture: &FixturePatch,
    osc: &OscillatorFrameSnapshot,
    gate: Option<&ShowBrainFrame>,
    mover_ordinal: usize,
    mover_count: usize,
) -> FixtureIntentState {
    // COM-499: effect fixtures EMERGE with the music — energy-gated dimmer + a
    // static-band colour + a music-driven gobo spin — instead of sitting always-on.
    // Two flavours (see `EffectKind` / `effect_dimmer`): the Obsession is the refined
    // static moonflower; the GalaxyJet borrows the Obsession's ORIGINAL present look
    // and keeps its own pan/tilt sweep. The 7R movers are neither.
    let effect = effect_kind(fixture);
    let is_effect = effect.is_some();
    let mut intent = FixtureIntentState {
        dimmer: match effect {
            Some(kind) => effect_dimmer(snapshot, osc, kind),
            None => dimmer_for_fixture(snapshot, fixture.role, fixture.visual_kind, osc),
        },
        strobe: strobe_for_fixture(snapshot, fixture.role, fixture.visual_kind, osc),
        pan: pan_for_fixture(snapshot, fixture.role, fixture.visual_kind, osc, mover_ordinal, mover_count),
        tilt: tilt_for_fixture(snapshot, fixture.role, osc, mover_ordinal, mover_count),
        zoom: 0.35 + snapshot.energy * 0.45,
        focus: 0.45 + snapshot.stability * 0.35,
        color_wheel: if is_effect {
            effect_color_wheel(snapshot)
        } else {
            color_wheel_for_fixture(snapshot)
        },
        gobo_slot: gobo_slot_for_fixture(snapshot, osc),
        // COM-499 Stage 2: a static effect spins with the music — energy speed,
        // lattice-commit direction (Ch7 gobo today; Ch8 mirror pending the
        // verified 8-ch profile).
        gobo_rotation: if is_effect {
            effect_rotation(snapshot)
        } else {
            gobo_rotation_for_fixture(snapshot, osc)
        },
        // Prism flourishes on the DROP/release (when the beams slam back in), not
        // on the tense build — tension now retreats the beams, so prism-on-tension
        // would fire while they're meant to be gone.
        prism: if snapshot.trigger_blinder || snapshot.drop_impact > 0.45 { 1.0 } else { 0.0 },
        laser_macro: laser_macro_for_fixture(snapshot, fixture.role, fixture.visual_kind, osc),
        ..Default::default()
    };

    // Stable per-fixture phase (golden-ratio spread of the DMX address) for the
    // dissonance-bloom fan, so each fixture lands on a different point of the arc.
    let addr_key = (fixture.universe as u32) * 1024 + fixture.start_address as u32;
    let fixture_phase = (addr_key as f32 * 0.618_034).fract();
    let (r, g, b, w) = color_for_fixture(
        snapshot, fixture.role, fixture.visual_kind, intent.dimmer, fixture_phase, osc,
    );
    intent.red = r;
    intent.green = g;
    intent.blue = b;
    intent.white = w;

    // EALP B1: journey-phase INTENSITY macro — scale the whole fixture output
    // (dimmer + colour) by the field's journey phase so Birth emerges, Lock is
    // full, Afterglow fades (never to black). Uniform => no colour/dimmer skew.
    // Lasers skip (their dimmer is a binary enable gate, like global brightness);
    // a laser's journey "accent" still rides its pan/tilt movement scale instead.
    let journey_dimmer = journey_dimmer_scale(snapshot);
    if (journey_dimmer - 1.0).abs() > 0.001
        && !matches!(family_for(fixture.role, fixture.visual_kind), FixtureFamily::Laser)
    {
        intent.dimmer = (intent.dimmer * journey_dimmer).clamp(0.0, 1.0);
        intent.red = (intent.red * journey_dimmer).clamp(0.0, 1.0);
        intent.green = (intent.green * journey_dimmer).clamp(0.0, 1.0);
        intent.blue = (intent.blue * journey_dimmer).clamp(0.0, 1.0);
        intent.white = (intent.white * journey_dimmer).clamp(0.0, 1.0);
    }

    // Show-brain gate: intense lanes go dark when the brain rules them
    // ineligible. Wash/mover/gobo/blinder stay unwired until later slices.
    // A manual operator trigger always wins — the gate never overrides a
    // human hitting STROBE.
    if let Some(brain) = gate {
        let family = family_for(fixture.role, fixture.visual_kind);
        let manual_override =
            matches!(family, FixtureFamily::Strobe) && strobe_hard_trigger(snapshot);
        if matches!(family, FixtureFamily::Laser | FixtureFamily::Strobe)
            && !brain.lane(family).eligible
            && !manual_override
        {
            // The gate's contract is "lane goes DARK" — clear every
            // light-emitting field, not just the dimmer-shaped ones.
            // `color_for_fixture` floors val at 0.35 (and forces laser sat to
            // 1.0), and dmx/mapping.rs writes RGB(W) unconditionally, so an
            // RGB-channel laser would keep emitting through a 'gated dark'
            // lane if RGBW survived the gate.
            intent.dimmer = 0.0;
            intent.strobe = 0.0;
            intent.laser_macro = 0;
            intent.red = 0.0;
            intent.green = 0.0;
            intent.blue = 0.0;
            intent.white = 0.0;
            intent.prism = 0.0;
        }
    }
    intent
}

// ── Slice-2 harmonic-field colour shaping (COM-499) ─────────────────────────
// Continuous functions of the harmonic field — no counters — so colour moves
// with tonal texture even on a steady root (the cure for "static magenta").
// All bounded + clamped; they shape sat/hue/white and RAISE a dim floor, and
// never cap a bright moment (the drop bloom stays full per Koob's "RL" steer).
// These are the natural future MIDI-bindable depth knobs.
/// Dissonance (low `balance`) drives saturation toward the per-kind ceiling; a
/// consonant field eases it off. Floored so colours never wash fully to grey.
const SAT_CONSONANCE_DEPTH: f32 = 0.28;
const SAT_FLOOR: f32 = 0.40;
/// Warm(major)/cool(minor) hue bend (±~0.03 of the circle ≈ ±11°) around the
/// lattice anchor, plus how much a major field lifts the white body.
const TEMP_HUE_DEPTH: f32 = 0.03;
const TEMP_WHITE_DEPTH: f32 = 0.40;
/// 24/7 calm-bed dim floor (Wash/Accent) that lifts as a section develops, so
/// quiet passages settle into a dim bed instead of flickering near black.
const BED_FLOOR_BASE: f32 = 0.10;
const BED_FLOOR_EXCITEMENT: f32 = 0.10;

/// Per-kind saturation eased by harmonic consonance (`balance`). Dissonant =
/// toward `ceiling`; consonant = lower, never below `SAT_FLOOR`.
fn consonance_saturation(ceiling: f32, balance: f32) -> f32 {
    // SAT_FLOOR.min(ceiling) guards f32::clamp's min<=max contract if a future
    // visual kind ever passes a ceiling below the floor (panic on the render path).
    (ceiling - balance * SAT_CONSONANCE_DEPTH).clamp(SAT_FLOOR.min(ceiling), ceiling)
}

/// Bipolar warm/cool temperature from the major/minor field. `major_smooth` is
/// 0..1 with 0.5 the live neutral, so this is -1 (minor/cool) .. +1 (major/warm),
/// ~0 on ambiguous harmony.
fn tonal_temperature(major_smooth: f32) -> f32 {
    (major_smooth - 0.5) * 2.0
}

/// 24/7 bed floor that lifts with arc `excitement`.
fn bed_floor(excitement: f32) -> f32 {
    BED_FLOOR_BASE + excitement * BED_FLOOR_EXCITEMENT
}

// ── Slice-2b: dissonance bloom + calm-bed colour breath ─────────────────────
/// As harmonic stability drops, each fixture's hue fans to a different point on a
/// widening arc around the lattice anchor (the rig "shatters" into a chromatic
/// spread), re-converging as harmony settles. Capped to a tasteful arc (~65°),
/// not a full rainbow.
const BLOOM_SPREAD_CAP: f32 = 0.18;
/// Calm-bed colour breath: an ultra-slow, ALWAYS-ON hue drift folded into the
/// wash rotation. wash_rotation's saw is energy-gated OFF in quiet passages, so
/// this un-gated term keeps the wash bed breathing colour instead of freezing on
/// one hue (the review's 24/7 quiet-bed finding). ~1°/sec, well under one
/// rotation/min. MIDI-dialable.
const BED_HUE_DRIFT_RATE: f32 = 0.003;

/// Per-fixture dissonance fan: low stability widens the spread; each fixture (by
/// its stable `fixture_phase` in [0,1)) lands on a different point of the arc.
fn dissonance_fan(stability: f32, fixture_phase: f32) -> f32 {
    let spread = (1.0 - stability).clamp(0.0, 1.0) * BLOOM_SPREAD_CAP;
    (fixture_phase - 0.5) * spread
}

/// Calm-bed hue drift (wash only): a slow continuous offset so the bed breathes
/// colour even when wash_rotation is energy-gated off in a quiet passage.
///
/// `frame_time` is an unbounded monotonic seconds counter, so we wrap the time
/// DOMAIN to one rotation period before scaling. A raw `frame_time * RATE` loses
/// the per-frame increment to f32 rounding within ~1-2 days of 24/7 uptime (the
/// breath freezes); wrapping keeps the multiplied operand < 1 cycle so per-frame
/// resolution stays fine for several days, and it degrades gracefully (coarse,
/// not frozen) rather than stalling. Residual: `frame_time` itself coarsens past
/// ~a week of uptime — the durable fix is a wrapped phase accumulated at the
/// CueEngine source (shared with segment_rgbw / hue::mappings); a restart resets
/// it. Tracked as a follow-on. The 1.0 -> 0 operand wrap is invisible because the
/// hue sum is rem_euclid(1.0) (a full rotation lands back on the same hue).
fn bed_hue_drift(frame_time: f32) -> f32 {
    let period = 1.0 / BED_HUE_DRIFT_RATE; // seconds per full hue rotation (~333s)
    frame_time.rem_euclid(period) * BED_HUE_DRIFT_RATE
}

// ── 7R beam moving-head autopilot ───────────────────────────────────────────
/// Beams (the 7R moving heads) retreat as the build tightens and slam back on the
/// release — anticipation/contrast, the same hold-for-release language as the
/// lasers. Driven by `tension_bank` (the pre-drop buildup ACCUMULATOR, which peaks
/// ~0.6-0.7 before a drop and discharges on it) rather than raw `tension` — so the
/// band below is actually reachable on a real build, and the drop's discharge
/// brings the beams straight back. Fade is smooth (no hard cut). MIDI-dialable.
const BEAM_TENSION_OFF_LOW: f32 = 0.40;
const BEAM_TENSION_OFF_HIGH: f32 = 0.65;

/// Smooth 0->1 Hermite ramp across [lo, hi]; clamps outside. Used to fade rather
/// than hard-cut so threshold crossings don't pop.
fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn dimmer_for_fixture(
    snapshot: &CueSnapshot,
    role: FixtureRole,
    visual_kind: StageVisualKind,
    osc: &OscillatorFrameSnapshot,
) -> f32 {
    let base = match role {
        FixtureRole::Wash => 0.18 + snapshot.energy * 0.62 + snapshot.bass_pump * 0.18,
        FixtureRole::Accent => {
            // Flash accents are licensed: on atonal walls the dominant latch
            // wanders and its envelope re-fires constantly — blinking, not
            // accenting. (The latch's energy gate is honest now; this is the
            // taste layer on top.)
            0.26 + snapshot.high * 0.30
                + snapshot.dominant_env * 0.18 * snapshot.intensity_license.clamp(0.0, 1.0)
        }
        FixtureRole::Strobe => 0.18 + snapshot.drop_impact * 0.22,
    };
    // LED panels (e.g. the 200W RGB) are the rig's colour BODY. Earlier they
    // were pinned to the upper register (0.74<->1.0) and read as "always
    // bright"; Koob wants real brightness VARIATION, so the level now rides
    // energy across a WIDE range — genuinely dim in a quiet passage, full on a
    // peak — with a bass-pump punch. The style brightness ceiling
    // (apply_global_overrides) caps the top per posture, and the Sine Bar +
    // `reactivity` master still shape it. Tunable knobs.
    let base = if matches!(visual_kind, StageVisualKind::LedPanel) {
        LED_PANEL_DIM_FLOOR
            + snapshot.energy * LED_PANEL_DIM_ENERGY
            + snapshot.bass_pump * LED_PANEL_DIM_PUMP
    } else {
        base
    };
    let binding = match visual_kind {
        StageVisualKind::Wash | StageVisualKind::LedPanel => OscillatorBinding::new(
            OscillatorShape::Sine,
            OscillatorInterval::Bar,
            0.18,
        ),
        StageVisualKind::Gobo | StageVisualKind::Beam => OscillatorBinding::new(
            OscillatorShape::Triangle,
            OscillatorInterval::Beat,
            0.14,
        )
        .with_ratio(0.5),
        StageVisualKind::Laser => OscillatorBinding::new(
            OscillatorShape::Saw,
            OscillatorInterval::Bar,
            0.12,
        )
        .with_ratio(0.5),
        StageVisualKind::Strobe => OscillatorBinding::new(
            OscillatorShape::Square,
            OscillatorInterval::Beat,
            0.20,
        )
        .with_pulse_width(0.25),
    };
    let mut value = add_bipolar(base, osc.sample(binding));
    if snapshot.trigger_blinder || snapshot.auto_blinder_active {
        value = value.max(0.88);
    }
    // Slice-2 24/7 bed floor (Wash/Accent only): a floor only RAISES the minimum,
    // so it settles quiet passages without ever capping a bright drop. Applied
    // before the reactivity multiply so it scales with the operator master.
    if matches!(role, FixtureRole::Wash | FixtureRole::Accent) {
        value = value.max(bed_floor(snapshot.excitement));
    }
    // 7R beams retreat as harmonic tension builds and slam back on the release
    // (Koob). Applied AFTER the bed floor so a tense build can pull them dark; the
    // drop releasing tension brings them straight back. Beam family only.
    if matches!(visual_kind, StageVisualKind::Beam) {
        value *= 1.0 - smoothstep(BEAM_TENSION_OFF_LOW, BEAM_TENSION_OFF_HIGH, snapshot.tension_bank);
    }
    value * snapshot.reactivity.max(0.2)
}

/// Which emergence flavour an effect fixture uses. The two flavours diverge ONLY in
/// `effect_dimmer` — the colour wheel (`effect_color_wheel`) and gobo spin
/// (`effect_rotation`) are shared, so both read "musical effect projector".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EffectKind {
    /// The Chauvet Obsession (and any static colour-wheel moonflower): a fixed
    /// housing, no pan/tilt. The REFINED "true moonflower" emergence — an
    /// `energy_slow` presence gate plus a peak-loudness band-pass, so it is present
    /// only through the lower-energy grooves and dark at both the quiet intro AND the
    /// loud climax (the 7R beams carry the peak).
    Moonflower,
    /// The Shehds GalaxyJet 300W beam: a moving head that BORROWS the Obsession's
    /// ORIGINAL (pre-refinement) emergence look. Koob: the Obsession's present,
    /// beam-cone "galaxy" feel — the one it had before it became a true static
    /// moonflower — works perfectly as the GalaxyJet, sitting alongside the 7R beams.
    /// So the GalaxyJet keeps that excitement-gated emergence + harmonic-tension
    /// retreat AND its own pan/tilt sweep (movement is not gated by the effect path).
    /// Matched by IDENTITY: the GalaxyJet is channel-identical to the 7R beam (both
    /// carry colour wheel + gobo + pan/tilt + prism), so no capability rule could tell
    /// them apart without mis-catching the 7R.
    GalaxyBeam,
}

/// The GalaxyJet's ATLABASE profile UUID. In the live patch `profile_id` is the ATLA
/// id (e.g. the Obsession is `bfd9b5ac-…`), so this is the exact, stable identity
/// hook; the `name` contains-"galaxy" check is a belt-and-suspenders fallback for an
/// operator-renamed or re-imported fixture.
const GALAXYJET_PROFILE_ID: &str = "a36a0e1c-a9e9-4978-a62d-06e66c7b1ac8";

/// Allocation-free case-insensitive (ASCII) substring search. `effect_kind`
/// runs per fixture per tick on the DMX render path AND per fixture per UI
/// frame, so the classifier must not heap-allocate — this replaces a
/// `format!(..).to_ascii_lowercase()` (two String allocations per call) with a
/// byte-window scan. An empty needle matches everything (mirrors
/// `str::contains("")`).
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

/// Classify a fixture into an emergence flavour, or `None` if it is not an effect.
///
/// COM-499 fork: identity FIRST (the GalaxyJet has pan/tilt and is channel-identical
/// to the 7R, so the capability predicate below can't catch it and no looser rule
/// could without mis-catching the 7R). The exact, alloc-free profile-id check runs
/// before the case-insensitive "galaxy" name/profile fallback (belt-and-suspenders
/// for an operator-renamed or re-imported fixture). Everything else with a colour
/// wheel and no pan/tilt is the static Obsession moonflower — `visual_kind` is
/// SaaS-authored and the live rig tags the 4 Obsessions `Gobo` (verified against
/// react.db); the 7R beams + lasers (pan/tilt) and the 200W panels (no colour
/// wheel) are all excluded.
fn effect_kind(fixture: &FixturePatch) -> Option<EffectKind> {
    if fixture.profile_id == GALAXYJET_PROFILE_ID
        || contains_ignore_ascii_case(&fixture.name, "galaxy")
        || contains_ignore_ascii_case(&fixture.profile_id, "galaxy")
    {
        return Some(EffectKind::GalaxyBeam);
    }
    let static_colour_effect = matches!(
        fixture.visual_kind,
        StageVisualKind::Beam | StageVisualKind::Gobo
    ) && fixture.channels.color_wheel.is_some()
        && fixture.channels.pan.is_none()
        && fixture.channels.tilt.is_none();
    static_colour_effect.then_some(EffectKind::Moonflower)
}

/// Should this fixture render as a WIDE gobo-effect projector (a cone with a circle
/// pool at the landing, aimed down, its own sweep) rather than a tight 7R beam?
///
/// The GalaxyJet is SaaS-tagged `Beam` (its category is "Moving Heads"), which drops
/// it into the renderer's tight-column beam arm — visually indistinguishable from the
/// 7R movers. But it is an effect projector like the Obsession, so the renderer should
/// give it the Obsession's wide gobo look. Keyed on effect identity (profile), so it
/// survives a SaaS `visual_kind` re-tag. The Obsession itself has no pan/tilt and is
/// drawn by the dedicated moonflower-star path, so only the GalaxyBeam needs this.
pub fn renders_as_galaxy_effect(fixture: &FixturePatch) -> bool {
    matches!(effect_kind(fixture), Some(EffectKind::GalaxyBeam))
}

/// GalaxyBeam ENERGY presence band, on `energy_slow` (the section RMS envelope). Below
/// LOW the galaxies are fully dark (quiet / low-energy grooves), above HIGH they are at
/// full presence — so they emerge with the section instead of sitting on through the
/// grooves. MIDI-dialable: raise the band to keep them off through more of the track;
/// lower it to bring them up in quieter grooves.
const GALAXY_PRESENCE_LOW: f32 = 0.38;
const GALAXY_PRESENCE_HIGH: f32 = 0.68;

/// GalaxyBeam tension retreat band. The galaxies pull to FULLY DARK as tension builds,
/// applied to BOTH the 7R beams' own build signal (`tension_bank`, which reaches
/// ~0.6-0.7 on a build — the signal that visibly darkens the 7R) AND the Atonal-Field
/// "Tension %" meter (`snapshot.tension`), whichever is higher. HOTTER than the 7R's
/// own retreat (0.40-0.65) and with NO bed floor, so the galaxies turn fully OFF where
/// the 7R only dims. MIDI-dialable: lower the band to drop them sooner / harder.
const GALAXY_TENSION_OFF_LOW: f32 = 0.30;
const GALAXY_TENSION_OFF_HIGH: f32 = 0.55;

/// Moonflower (refined Obsession) PRESENCE edges, in SLOW-energy (`energy_slow`, the
/// RMS envelope) units — NOT `excitement`, which pinned the effect "always on".
/// Below LOW it is dark (quiet/intro/breakdown); above HIGH it is at full presence.
const EFFECT_PRESENCE_LOW: f32 = 0.20;
const EFFECT_PRESENCE_HIGH: f32 = 0.55;

/// Moonflower-only: the effect RETREATS at peak loudness (the climax) so, paired with
/// the `energy_slow` presence gate, it is a BAND-PASS on section energy — dark when
/// quiet, lit through the grooves, dark again when loud. Keyed on `energy_slow`, not
/// harmonic tension (Koob's confirmed look is loudness-only). MIDI-dialable.
const EFFECT_LOUD_OFF_LOW: f32 = 0.65;
const EFFECT_LOUD_OFF_HIGH: f32 = 0.85;

/// Dimmer for an effect fixture, dispatched by flavour. Both breathe on the beat and
/// the operator blinder still wins; they differ in how they emerge and retreat.
///
/// * `GalaxyBeam` — on-state managed like the 7R beams (a low ever-present base that
///   lifts with treble + harmony and breathes), but HOTTER on the tolerances: it
///   retreats to fully dark on the VISIBLE Tension meter (`snapshot.tension`) earlier
///   and deeper than the 7R, and the drop punches it back on the release.
/// * `Moonflower` — the refined Obsession: an `energy_slow` presence gate (genuinely
///   emerges + recedes with the section, NO bed floor) plus a peak-loudness band-pass
///   so it is off at the climax.
fn effect_dimmer(snapshot: &CueSnapshot, osc: &OscillatorFrameSnapshot, kind: EffectKind) -> f32 {
    match kind {
        EffectKind::GalaxyBeam => galaxy_dimmer(snapshot, osc),
        EffectKind::Moonflower => moonflower_dimmer(snapshot, osc),
    }
}

/// GalaxyJet dimmer — see `effect_dimmer`. An `energy_slow` presence gate emerges it
/// with the section and floors it to ZERO in quiet / low-energy grooves (NO bed floor —
/// the shared 0.10-0.20 bed floor is what pinned the galaxies always-on). The lit level
/// breathes like the 7R (a 0.26 base lifting with `high` + `dominant_env`, beat breath),
/// and it is HOTTER on tension: it retreats to FULLY DARK on `tension_bank` (the 7R's
/// own build signal, ~0.6-0.7 on a build) OR the visible Tension meter, whichever is
/// higher, earlier than the 7R. The drop punches it back; the operator blinder wins.
fn galaxy_dimmer(snapshot: &CueSnapshot, osc: &OscillatorFrameSnapshot) -> f32 {
    // ENERGY presence: emerge with the section, fully dark in quiet / low-energy
    // grooves (`energy_slow`). NO bed floor — `bed_floor` (0.10 + excitement*0.10 ->
    // ~0.20 at saturated excitement) is what pinned the galaxies always-on, and this
    // presence gate floors them to ZERO instead, so they genuinely come and go.
    let presence = smoothstep(GALAXY_PRESENCE_LOW, GALAXY_PRESENCE_HIGH, snapshot.energy_slow);
    let base = 0.26
        + snapshot.high * 0.30
        + snapshot.dominant_env * 0.18 * snapshot.intensity_license.clamp(0.0, 1.0);
    let breath = OscillatorBinding::new(OscillatorShape::Triangle, OscillatorInterval::Beat, 0.14)
        .with_ratio(0.5);
    let mut value = add_bipolar(base, osc.sample(breath)) * presence;
    // Retreat to FULLY DARK on tension: the 7R beams' own build signal (`tension_bank`,
    // ~0.6-0.7 on a build) OR the visible Atonal-Field meter (`snapshot.tension`),
    // whichever is higher — HOTTER (earlier band) than the 7R's own retreat.
    let retreat = smoothstep(GALAXY_TENSION_OFF_LOW, GALAXY_TENSION_OFF_HIGH, snapshot.tension_bank)
        .max(smoothstep(GALAXY_TENSION_OFF_LOW, GALAXY_TENSION_OFF_HIGH, snapshot.tension));
    value *= 1.0 - retreat;
    // The drop punches them back on the release — gated by presence so a drop in a
    // quiet section stays dark.
    value = value.max(snapshot.drop_impact * presence);
    // The operator blinder still wins, even over a tension retreat.
    if snapshot.trigger_blinder || snapshot.auto_blinder_active {
        value = value.max(0.88);
    }
    value * snapshot.reactivity.max(0.2)
}

/// Obsession (refined moonflower) dimmer — see `effect_dimmer`. An `energy_slow`
/// presence gate (emerges + recedes with the section, NO bed floor — floors to ZERO in
/// quiet) plus a peak-loudness band-pass; the drop slams it back, gated by presence so
/// a drop in silence stays dark.
fn moonflower_dimmer(snapshot: &CueSnapshot, osc: &OscillatorFrameSnapshot) -> f32 {
    let base = 0.30 + snapshot.energy * 0.55 + snapshot.bass_pump * 0.12;
    let breath = OscillatorBinding::new(OscillatorShape::Triangle, OscillatorInterval::Beat, 0.14)
        .with_ratio(0.5);
    let presence = smoothstep(EFFECT_PRESENCE_LOW, EFFECT_PRESENCE_HIGH, snapshot.energy_slow);
    let retreat = smoothstep(EFFECT_LOUD_OFF_LOW, EFFECT_LOUD_OFF_HIGH, snapshot.energy_slow);
    let mut value = add_bipolar(base, osc.sample(breath)) * presence;
    value *= 1.0 - retreat;
    value = value.max(snapshot.drop_impact * presence);
    if snapshot.trigger_blinder || snapshot.auto_blinder_active {
        value = value.max(0.88);
    }
    value * snapshot.reactivity.max(0.2)
}

/// A manual / hard strobe trigger from the operator — the STROBE button, an OSC
/// trigger, or overdrive. It overrides the show-brain gate: hitting STROBE
/// means strobe, regardless of the musical arc.
fn strobe_hard_trigger(snapshot: &CueSnapshot) -> bool {
    snapshot.strobe_active || snapshot.trigger_strobe || snapshot.overdrive_strobe_active
}

fn strobe_for_fixture(
    snapshot: &CueSnapshot,
    role: FixtureRole,
    visual_kind: StageVisualKind,
    osc: &OscillatorFrameSnapshot,
) -> f32 {
    if strobe_hard_trigger(snapshot) {
        return 0.35 + snapshot.tension * 0.65;
    }

    // COM-409A hardcoded demo rig binding: deck strobes pulse from a square
    // carrier so the 3D stage visibly reads as oscillator-driven even without
    // manual cue triggers.
    if matches!(role, FixtureRole::Strobe) && matches!(visual_kind, StageVisualKind::Strobe) {
        let pulse = osc.sample(
            OscillatorBinding::new(
                OscillatorShape::Square,
                OscillatorInterval::Beat,
                1.0,
            )
            .with_ratio(1.0)
            .with_pulse_width(0.22),
        );
        if pulse > 0.0 { 0.60 + snapshot.tension * 0.30 } else { 0.0 }
    } else {
        0.0
    }
}

fn pan_for_fixture(
    snapshot: &CueSnapshot,
    role: FixtureRole,
    visual_kind: StageVisualKind,
    osc: &OscillatorFrameSnapshot,
    mover_ordinal: usize,
    mover_count: usize,
) -> f32 {
    match role {
        FixtureRole::Accent => {
            // Emerge field -> autonomous mover pan (Resonator 63960211). Three
            // field-derived terms, no wall clock, no human input:
            //   1. CENTER placed on a harmonic lattice port. Ports 0 & 2 (θ, θ+π)
            //      are a mirror pair of ANCHORS, but each head applies PORT_SPREAD
            //      identically, so the two CENTERS track in PARALLEL (~0.3 apart),
            //      NOT reflected around 0.5. The crossing/counterpoint comes from the
            //      counter-phased sweep below (term 3), not the center — do NOT
            //      "mirror" this with center = 1.0 - center; that breaks field fidelity.
            //   2. THROW toward the new polarity quadrant's angle on a flip, decaying
            //      via `polarity_env` (the existing accent envelope — no new timer).
            //   3. SWEEP via the energy-gated COM-409A oscillator, counter-phased per
            //      head so the beams cross/converse instead of moving in lockstep.
            // Depth still opens with energy; gated osc => still in silence for free.
            const PORT_SPREAD: f32 = 0.6; // how wide the heads spread off-center
            const THROW: f32 = 0.6;       // polarity-flip throw amount
            // B3: base port rotates with the active lattice port so the mirror pair
            // tracks the live harmonic anchor (active=0 -> ports 0,2 = Part A baseline).
            let port = mover_port(mover_ordinal, snapshot.qc_active_port);
            let base = 0.5 + (snapshot.qc_ports[port] - 0.5) * PORT_SPREAD;
            let q = (snapshot.polarity_quadrant.min(3)) as usize; // new quadrant after a flip
            let throw_to = 0.5 + (snapshot.qc_ports[q] - 0.5) * PORT_SPREAD;
            // B3: quaternion overlay strength punches the harmonic snap harder on a
            // lattice lock-in (additive over the Part A throw; no lattice = baseline).
            let center_chase =
                base + (throw_to - base) * snapshot.polarity_env * THROW * qc_accent_boost(snapshot);
            // SCAN/CHASE crossfade on the intensity license: atonal walls
            // (post-rock, ambient) wander the lattice anchors, so chasing them
            // reads as "jumping all over" exactly when the music asks for a
            // slow scan. License low → the center eases home to FOH (0.5) and
            // the anchor chase + polarity throws fade out; the counter-phased
            // phrase-length sine below is what remains — chill beams scanning
            // in cadence. License high → classic field-chasing behavior.
            let license = snapshot.intensity_license.clamp(0.0, 1.0);
            let center = 0.5 + (center_chase - 0.5) * license;
            let group_phase = mover_group_phase(mover_ordinal, mover_count, snapshot.symmetry);
            let sweep_chase = osc.sample(
                OscillatorBinding::new(
                    OscillatorShape::Sine,
                    OscillatorInterval::Phrase,
                    0.34,
                )
                .with_ratio(1.0)
                .with_phase(group_phase),
            );
            // SCAN sweep: HALF the phrase rate (~30 s per pass at 122) — the
            // lazy side-to-side rake a chill room wants. Sampled separately
            // and crossfaded so a license change never jumps the phase.
            let sweep_scan = osc.sample(
                OscillatorBinding::new(
                    OscillatorShape::Sine,
                    OscillatorInterval::Phrase,
                    0.34,
                )
                .with_ratio(0.5)
                .with_phase(group_phase),
            );
            let sweep = sweep_chase * license + sweep_scan * (1.0 - license);
            // EALP B1/B2: journey phase + harmonic coherence (trust) scale the
            // sweep depth (Birth narrow, Lock full, Dive accent, Afterglow relaxed;
            // atonal field eases) on top of the energy-driven depth.
            // B5 all-in: a harmonic surprise briefly widens the sweep (motion accent).
            // At LOW license the scan keeps a guaranteed gentle depth — the
            // atonal field collapses `trust_motion`, and a frozen head is the
            // wrong kind of calm.
            let motion_full = journey_motion_scale(snapshot) * trust_motion(snapshot)
                * tonnetz_motion_punch(snapshot);
            let motion = motion_full * license + SCAN_MOTION_FLOOR * (1.0 - license);
            add_bipolar(center, sweep * (1.0 + 0.4 * snapshot.energy * license) * motion)
        }
        // A laser-macro fixture has no DMX pan channel, but the virtual stage
        // draws its fan as a horizontal sweep. Drive that sweep from the shared
        // oscillator here so the renderer needs no oscillator of its own.
        FixtureRole::Strobe if matches!(visual_kind, StageVisualKind::Laser) => {
            let sweep = osc.sample(
                OscillatorBinding::new(
                    OscillatorShape::Sine,
                    OscillatorInterval::Bar,
                    0.40,
                )
                .with_ratio(1.0),
            );
            add_bipolar(0.5, sweep)
        }
        FixtureRole::Strobe => 0.5,
        FixtureRole::Wash => 0.5,
    }
}

fn tilt_for_fixture(
    snapshot: &CueSnapshot,
    role: FixtureRole,
    osc: &OscillatorFrameSnapshot,
    mover_ordinal: usize,
    mover_count: usize,
) -> f32 {
    match role {
        FixtureRole::Accent => {
            // COM-409A: mover tilt = triangle sweep, energy as a small lift, and
            // the sweep depth opens up with energy (matching pan) so the head
            // traces a wider arc when the music drives, tighter when it settles.
            // Counter-phased per head (matches pan) so the two heads don't tilt in
            // lockstep, plus an emerge field-phase drift gated by the SAME COM-409A
            // envelope (osc.energy_env) so it goes still in silence like the sweep.
            let sweep = osc.sample(
                OscillatorBinding::new(
                    OscillatorShape::Triangle,
                    OscillatorInterval::Bar,
                    0.28,
                )
                .with_ratio(1.0)
                .with_phase(mover_group_phase(mover_ordinal, mover_count, snapshot.symmetry)),
            );
            // EALP B1/B2: journey phase + harmonic coherence (trust) scale both
            // movement terms (sweep + field drift) so the tilt narrows in Birth,
            // opens in Lock, accents in Dive, relaxes in Afterglow, and eases when
            // the field is atonal — matching the pan.
            // B5 all-in: surprise widens the sweep; + a smooth harmonic-position drift
            // toward the Tonnetz fifths centroid (gated by tonal confidence).
            let motion_full = journey_motion_scale(snapshot) * trust_motion(snapshot)
                * tonnetz_motion_punch(snapshot);
            // SCAN/CHASE crossfade (see pan_for_fixture): license low → the
            // wandering field/Tonnetz drift fades out and the tilt settles
            // near FOH with a gentle guaranteed sweep; high → classic.
            let license = snapshot.intensity_license.clamp(0.0, 1.0);
            // SCAN posture: at low license the tilt eases to SCAN_TILT (beams
            // horizontal over the room) and the sweep shrinks to a faint slow
            // bob — the pan does the scanning; the tilt HOLDS the rake.
            let motion = motion_full * license + 0.15 * (1.0 - license);
            let drift = (snapshot.phase * std::f32::consts::TAU).sin() * 0.12
                + (snapshot.tonnetz_hue * std::f32::consts::TAU).sin()
                    * TONNETZ_TILT_DEPTH
                    * snapshot.tonnetz_strength;
            let field_tilt = drift * osc.energy_env() * motion_full * license;
            let center_chase = 0.50 + snapshot.energy * 0.12 * license + field_tilt;
            let center = center_chase * license + SCAN_TILT * (1.0 - license);
            add_bipolar(
                center,
                sweep * (1.0 + 0.4 * snapshot.energy * license) * motion,
            )
        }
        FixtureRole::Strobe => 0.5,
        FixtureRole::Wash => 0.5,
    }
}

fn color_wheel_for_fixture(snapshot: &CueSnapshot) -> u8 {
    ((snapshot.dominant_pc % 12) * 21).min(255)
}

/// Colour-wheel value for a static effect, kept in the STATIC colour band
/// (0..=139 of 255; the 140..=255 zone is a continuous scroll on the Obsession).
/// The dominant pitch picks a wheel slot so the effect's colour moves with the
/// harmony instead of a fixed gel.
fn effect_color_wheel(snapshot: &CueSnapshot) -> u8 {
    ((snapshot.dominant_pc % 12) * 11).min(139)
}

/// COM-499 Stage 2: rotation command for a static effect's spin (the Obsession
/// Ch7 gobo rotation today; the Ch8 mirror beam-fan once the SKU's 8-channel
/// profile is verified). Emerges with the music — speed rises with
/// energy/excitement, and the harmonic polarity quadrant (the quaternion lattice
/// commit, already on `CueSnapshot`) flips the direction. Convention: 0.5 ≈ stop,
/// below it = one direction (faster toward 0), above = the other (faster toward 1).
///
/// NOTE: the exact CW/CCW/stop DMX bands are fixture-specific and `to_dmx` writes
/// the raw value (no per-profile rotation-range translation), so this is a
/// conservative standard-moonflower mapping — the precise bands + the Ch8 mirror
/// channel land once the 60W GDTF / rig confirms the value ranges (Yakoob's flag).
/// Speed uses energy as a musical proxy; the literal quaternion-field `dw/dt`
/// would need a render-side plumb (a clean follow-up).
fn effect_rotation(snapshot: &CueSnapshot) -> f32 {
    let speed = (0.12 + snapshot.energy * 0.55 + snapshot.excitement * 0.18).clamp(0.0, 0.92);
    if snapshot.polarity_quadrant % 2 == 0 {
        0.5 - speed * 0.5
    } else {
        0.5 + speed * 0.5
    }
}

fn gobo_slot_for_fixture(snapshot: &CueSnapshot, osc: &OscillatorFrameSnapshot) -> u8 {
    // COM-499 Phase A D5: phrase ordinal is clock-locked (real phrase boundaries
    // on a resolved show clock, 122-BPM fallback otherwise) via `osc`.
    let phrase = osc.phrase_count();
    ((snapshot.dominant_pc as u32 + phrase) % 8) as u8
}

fn gobo_rotation_for_fixture(snapshot: &CueSnapshot, osc: &OscillatorFrameSnapshot) -> f32 {
    let base = (snapshot.collatz_phase * 0.5 + snapshot.tension * 0.5).fract();
    let nudge = osc.sample(
        OscillatorBinding::new(
            OscillatorShape::Saw,
            OscillatorInterval::Beat,
            0.12,
        )
        .with_ratio(0.25),
    );
    add_bipolar(base, nudge)
}

fn laser_macro_for_fixture(
    snapshot: &CueSnapshot,
    role: FixtureRole,
    visual_kind: StageVisualKind,
    osc: &OscillatorFrameSnapshot,
) -> u8 {
    if matches!(role, FixtureRole::Strobe) || matches!(visual_kind, StageVisualKind::Laser) {
        // COM-499 Phase A D5: clock-locked phrase ordinal (see gobo_slot).
        let phrase = osc.phrase_count();
        let phrase_macro = 1 + ((snapshot.dominant_pc as u32 + phrase) % 15) as u8;
        // Bump the macro on the BAR, not the beat: a per-beat toggle reverses the
        // projected laser pattern at ~2 Hz (120-140 BPM) — squarely in the
        // photosensitive band for a sustained scan at a public audience. Bar-rate
        // pattern change keeps the projector well below that band.
        let pulse = osc.sample(
            OscillatorBinding::new(
                OscillatorShape::Square,
                OscillatorInterval::Bar,
                0.06,
            )
            .with_ratio(0.5)
            .with_pulse_width(0.35),
        );
        let bump = if pulse > 0.0 { 1 } else { 0 };
        phrase_macro.saturating_add(bump).min(15)
    } else {
        0
    }
}

fn color_for_fixture(
    snapshot: &CueSnapshot,
    role: FixtureRole,
    visual_kind: StageVisualKind,
    dimmer: f32,
    fixture_phase: f32,
    osc: &OscillatorFrameSnapshot,
) -> (f32, f32, f32, f32) {
    // The pitch anchor the rig keys off: the single dominant pitch, or — when
    // the operator un-checks "Dom" — the all-notes chromatic blend (biased
    // toward the dominant, EMA-smoothed; built in osc/thread.rs).
    let lattice_anchor = if snapshot.all_notes_color {
        snapshot.all_notes_hue
    } else {
        snapshot.dominant_pc as f32 / 12.0
    };
    let harmonic_bias = snapshot.balance * 0.06 + snapshot.collatz_phase * 0.04;
    let wash_rotation = if matches!(visual_kind, StageVisualKind::Wash | StageVisualKind::LedPanel) {
        // COM-409A: wash hue = saw/rotation, anchored by lattice pitch. Plus the
        // slice-2b calm-bed drift (un-gated) so the wash bed still breathes colour
        // when the saw is energy-gated off in a quiet passage.
        osc.sample(
            OscillatorBinding::new(
                OscillatorShape::Saw,
                OscillatorInterval::Bar,
                0.28,
            )
            .with_ratio(0.5),
        ) + bed_hue_drift(snapshot.frame_time)
    } else {
        0.0
    };
    // COM-499 autonomous look rotation: shift the rig's hue by a well-distributed
    // (golden-ratio) offset per look_index, so the colour visibly cycles on each
    // polarity / phrase / drop event instead of tracking a (possibly steady) key.
    let look_offset = (snapshot.look_index as f32 * 0.381_966).fract();
    // Slice-2 major/minor temperature: bend hue a few degrees warm in a major
    // field, cool in a minor field (the warmth IS the major-ness); ~0 on
    // ambiguous harmony so the lattice anchor still reads as the key.
    // EALP B5 all-in: chord quality reinforces + extends the major/minor temperature
    // (adds dominant warmth/tension + diminished instability the continuous metric misses).
    let temp = tonal_temperature(snapshot.major_smooth)
        + chord_temperature(snapshot) * CHORD_TEMP_DEPTH;
    // Slice-2b dissonance bloom: a per-fixture hue fan that widens as harmonic
    // stability drops (the rig shatters into a chromatic spread on dissonance and
    // re-converges on the anchor as the harmony settles).
    let fan = dissonance_fan(snapshot.stability, fixture_phase);
    let hue = (lattice_anchor + harmonic_bias + wash_rotation + look_offset
        + temp * TEMP_HUE_DEPTH + fan
        // EALP B5: a gentle harmonic-motion drift toward the Tonnetz fifths
        // centroid, gated by tonal confidence so an atonal field doesn't smear hue.
        + snapshot.tonnetz_hue * TONNETZ_HUE_DEPTH * snapshot.tonnetz_strength
        // EALP B5 all-in: a P/R/L chromatic-mediant hue jolt on a parsimonious chord
        // move (Parallel/Relative/Leading-tone), decaying via prl_env.
        + prl_hue_kick(snapshot))
        .rem_euclid(1.0);
    // Slice-2 consonance saturation: dissonant/dense (low balance) toward the
    // per-kind ceiling, consonant eases off. Strobe (white) + Laser (full-sat)
    // are intentionally fixed.
    let mut sat = match visual_kind {
        StageVisualKind::Strobe => 0.0,
        StageVisualKind::Laser => 1.0,
        StageVisualKind::Gobo | StageVisualKind::Beam => consonance_saturation(0.92, snapshot.balance),
        StageVisualKind::Wash | StageVisualKind::LedPanel => consonance_saturation(0.74, snapshot.balance),
    };
    // EALP B2: harmonic coherence (trust) desaturates a noisy/atonal field toward
    // grey and keeps a confident field richly coloured — a distinct axis from the
    // consonance (balance) saturation above. Strobe (white) + Laser (full-sat) are
    // intentionally fixed, so they skip it. `val` (energy-driven) is untouched, so
    // a noisy drop still rips at full brightness while only the colour relaxes.
    if !matches!(visual_kind, StageVisualKind::Strobe | StageVisualKind::Laser) {
        sat *= trust_saturation(snapshot);
        // EALP B3: a quaternion lattice lock-in (qc_strength) is the "colour punch"
        // companion to the mover accent — lerp saturation toward full so colours pop
        // on lattice events. Lerp (not multiply) so it shows at any base; qc=0 no-op.
        // Distinct axis from trust (coherence floor): trust relaxes, qc punches.
        sat += (1.0 - sat) * snapshot.qc_strength.clamp(0.0, 1.0) * QC_SAT_PUNCH;
        // EALP B5: a voice-leading SURPRISE (harmonic jump) pops saturation toward
        // full — distinct trigger from the B3 lattice-lock pop; brightness untouched.
        sat += (1.0 - sat) * tonnetz_surprise(snapshot) * TONNETZ_SURPRISE_SAT;
        // All-notes colour: a spread chord washes a touch whiter (the blend
        // reads as a mix, not a wrong single hue); a single note stays vivid.
        if snapshot.all_notes_color {
            sat *= 0.7 + 0.3 * snapshot.all_notes_sat;
        }
    }
    let val = (0.35 + snapshot.energy * 0.42 + dimmer * 0.32 + snapshot.drop_impact * 0.12)
        .clamp(0.0, 1.0);
    let (r, g, b) = hsv_to_rgb(hue, sat, val);
    let white = if snapshot.trigger_blinder || snapshot.auto_blinder_active {
        0.8 + snapshot.energy * 0.2
    } else if matches!(role, FixtureRole::Strobe) {
        dimmer.max(snapshot.strobe_env)
    } else {
        // Slice-2 temperature on the white body: major lifts it (fuller, warmer),
        // minor hollows it. TEMP_WHITE_DEPTH < 1 keeps the factor in [0.6, 1.4], so
        // a minor field can never drive the white body negative/hollow.
        (1.0 - sat) * val * 0.35 * (1.0 + temp * TEMP_WHITE_DEPTH)
    };
    (r, g, b, white.clamp(0.0, 1.0))
}

/// Per-segment color for a pixel bar. Each of the `count` cells gets its own
/// RGBW: a hue gradient across the bar (anchored to the show's harmonic key,
/// so the bar reads in tune with the rest of the rig) plus a chase head that
/// sweeps across the cells, its speed riding energy. Overall brightness is
/// gated by the fixture's master `dimmer`, so the bar still goes dark in
/// silence and blackout like every other fixture. Shared by the renderer and
/// the DMX writer so the on-screen bar matches what goes out the wire.
pub fn segment_rgbw(
    snapshot: &CueSnapshot,
    dimmer: f32,
    seg: usize,
    count: usize,
) -> (f32, f32, f32, f32) {
    let frac = if count > 1 {
        seg as f32 / (count - 1) as f32
    } else {
        0.0
    };
    // Same lattice anchor the rest of the rig keys off (see color_for_fixture),
    // so the bar's gradient stays in the show's key — dominant pitch, or the
    // all-notes blend when "Dom" is un-checked.
    let anchor = if snapshot.all_notes_color {
        snapshot.all_notes_hue
    } else {
        snapshot.dominant_pc as f32 / 12.0
    };
    let base_hue = (anchor + snapshot.balance * 0.06).rem_euclid(1.0);
    let drift = snapshot.frame_time * 0.05;
    let hue = (base_hue + frac * 0.34 + drift).rem_euclid(1.0);

    // ── EMeRGE field → per-cell brightness (Resonator 63960211: "the field
    // drives all modalities"). The bar's brightness comes from the field
    // attributes, not raw energy: an OVERALL LEVEL from the field's intensity
    // arc and a PER-CELL STRUCTURE from the field's spatial signals. The
    // master `dimmer` still gates it (energy / silence / global brightness /
    // per-fixture trim / strobe), so quiet still goes dark.

    // OVERALL (how much): the Hero journey arc (Birth emerges → Lock/Dive full
    // → Afterglow fades), lifted by harmonic coherence (`trust`) and punched on
    // a quaternion lattice lock-in (`qc_strength`).
    let arc = journey_dimmer_scale(snapshot);
    let coherence = FIELD_TRUST_FLOOR + (1.0 - FIELD_TRUST_FLOOR) * snapshot.trust.clamp(0.0, 1.0);
    let punch = 1.0 + FIELD_QC_PUNCH * snapshot.qc_strength.clamp(0.0, 1.0);
    let field_level = (arc * coherence * punch).clamp(0.0, 1.0);

    // PER-CELL (where): a bright band — `symmetry` of them — sweeps the bar on
    // the field rotation `phase`; the four quaternion ports each own a quarter
    // of the bar and light it by their value; the active port lifts its
    // quarter. A floor keeps unlit cells dimly present rather than hard black.
    let n_bands = snapshot.symmetry.clamp(1, 4) as f32;
    // Wrapped distance of this cell from the nearest band centre (a band sits
    // where `frac * n_bands ≈ phase`), so the band peaks at the field's
    // rotation position and sweeps as `phase` advances.
    let band_phase = (frac * n_bands - snapshot.phase).rem_euclid(1.0);
    let band_dist = band_phase.min(1.0 - band_phase);
    let band = (1.0 - band_dist * 2.0).clamp(0.0, 1.0);
    let region = ((frac * 4.0) as usize).min(3);
    let port = snapshot.qc_ports[region].clamp(0.0, 1.0);
    let active = if region as u8 == snapshot.qc_active_port { FIELD_ACTIVE_LIFT } else { 0.0 };
    let cell_field =
        ((FIELD_CELL_FLOOR + (1.0 - FIELD_CELL_FLOOR) * band).max(port) + active).clamp(0.0, 1.0);

    let val = (dimmer * field_level * cell_field + snapshot.bass_pump * dimmer * 0.25).clamp(0.0, 1.0);
    let (r, g, b) = hsv_to_rgb(hue, 0.85, val);
    (r, g, b, 0.0)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h = (h.fract() * 6.0).clamp(0.0, 5.999);
    let i = h.floor();
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    match i as u8 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chord_quality_and_prl_drive_temperature_and_hue_gated() {
        // Chord temperature: major warm (+), minor cool (-), none neutral (0).
        assert!(chord_temperature(&CueSnapshot { chord_quality: 1, ..Default::default() }) > 0.0, "major warm");
        assert!(chord_temperature(&CueSnapshot { chord_quality: 2, ..Default::default() }) < 0.0, "minor cool");
        assert!(chord_temperature(&CueSnapshot { chord_quality: 3, ..Default::default() }) > 0.0, "dominant warm/tense");
        assert!(chord_temperature(&CueSnapshot { chord_quality: 4, ..Default::default() }) < 0.0, "diminished cool/unstable");
        assert_eq!(chord_temperature(&CueSnapshot { chord_quality: 0, ..Default::default() }), 0.0, "no chord neutral");
        // P/R/L hue jolt fires only with an active move + envelope + loudness.
        let prl_loud = CueSnapshot { chord_prl_move: 2, chord_prl_env: 1.0, energy_slow: 1.0, ..Default::default() };
        let prl_silent = CueSnapshot { chord_prl_move: 2, chord_prl_env: 1.0, energy_slow: 0.0, ..Default::default() };
        let no_move = CueSnapshot { chord_prl_move: 0, chord_prl_env: 0.0, energy_slow: 1.0, ..Default::default() };
        assert!(prl_hue_kick(&prl_loud) > 0.0, "a loud P/R/L move jolts hue");
        assert_eq!(prl_hue_kick(&prl_silent), 0.0, "no jolt in silence");
        assert_eq!(prl_hue_kick(&no_move), 0.0, "no jolt without a P/R/L move");
        // Motion punch: baseline 1.0, boosts above 1 on a gated harmonic surprise.
        assert_eq!(tonnetz_motion_punch(&CueSnapshot { tonnetz_delta: 0.0, ..Default::default() }), 1.0, "no surprise = baseline");
        let surprise = CueSnapshot { tonnetz_delta: 1.0, trust: 1.0, energy_slow: 1.0, ..Default::default() };
        assert!(tonnetz_motion_punch(&surprise) > 1.0, "a harmonic surprise widens the sweep");
    }

    #[test]
    fn tonnetz_surprise_gates_on_confidence_and_loudness_not_silence() {
        // A harmonic jump only pops when tonal (trust) AND loud (energy_slow).
        let loud_tonal = CueSnapshot { tonnetz_delta: 1.0, trust: 1.0, energy_slow: 1.0, ..Default::default() };
        let silent = CueSnapshot { tonnetz_delta: 1.0, trust: 1.0, energy_slow: 0.0, ..Default::default() };
        let atonal = CueSnapshot { tonnetz_delta: 1.0, trust: 0.0, energy_slow: 1.0, ..Default::default() };
        let no_jump = CueSnapshot { tonnetz_delta: 0.0, trust: 1.0, energy_slow: 1.0, ..Default::default() };
        assert!(tonnetz_surprise(&loud_tonal) > 0.5, "loud + tonal jump pops");
        assert_eq!(tonnetz_surprise(&silent), 0.0, "no pop in silence (energy gate)");
        assert_eq!(tonnetz_surprise(&atonal), 0.0, "no pop on noise (trust gate)");
        assert_eq!(tonnetz_surprise(&no_jump), 0.0, "no pop without a harmonic jump");
        // Smooth hue drift is gated by tonal confidence: identical tonnetz_hue with
        // strength 0 (atonal) vs 1 (tonal) must produce different colour (drift on).
        let wash = FixturePatch::demo_rgbw_wash("w", 0, 1);
        let off = CueSnapshot { tonnetz_hue: 0.9, tonnetz_strength: 0.0, energy: 0.5, ..Default::default() };
        let on = CueSnapshot { tonnetz_hue: 0.9, tonnetz_strength: 1.0, energy: 0.5, ..Default::default() };
        let osc = OscillatorFrameSnapshot::from_cue(&off);
        let c_off = color_for_fixture(&off, wash.role, wash.visual_kind, 1.0, 0.1, &osc);
        let c_on = color_for_fixture(&on, wash.role, wash.visual_kind, 1.0, 0.1, &osc);
        assert!(c_off != c_on, "tonnetz hue drift active when tonal, gated off when atonal");
    }

    #[test]
    fn low_license_movers_scan_foh_instead_of_chasing_anchors() {
        // Atonal walls wander the quaternion lattice — at LOW license the pan
        // center must ease home to FOH and ignore extreme anchors/throws,
        // while the slow counter-phased scan KEEPS moving (a frozen head is
        // the wrong kind of calm). At full license the same field state pulls
        // the center well off 0.5 (classic chase).
        let wild = |license: f32, beat: f64| CueSnapshot {
            energy: 0.6,
            energy_slow: 0.7,
            qc_ports: [0.95, 0.5, 0.05, 0.5], // extreme anchors
            polarity_env: 1.0,                // mid-throw
            polarity_quadrant: 0,
            trust: 0.1,                       // atonal field collapses trust
            intensity_license: license,
            clock_beat: beat,
            frame_time: beat as f32 * 0.5,
            ..Default::default()
        };

        let scan_a = wild(0.05, 0.0);
        let pan_a = pan_for_fixture(
            &scan_a, FixtureRole::Accent, StageVisualKind::Beam,
            &OscillatorFrameSnapshot::from_cue(&scan_a), 0, 2,
        );
        // Quarter-phrase later: the scan has visibly progressed.
        let scan_b = wild(0.05, 8.0);
        let pan_b = pan_for_fixture(
            &scan_b, FixtureRole::Accent, StageVisualKind::Beam,
            &OscillatorFrameSnapshot::from_cue(&scan_b), 0, 2,
        );
        assert!(
            (pan_a - 0.5).abs() < 0.35 && (pan_b - 0.5).abs() < 0.35,
            "scan mode stays around FOH: {pan_a}, {pan_b}"
        );
        assert!(
            (pan_a - pan_b).abs() > 0.02,
            "the slow scan keeps moving ({pan_a} -> {pan_b}), not frozen"
        );

        let chase = wild(1.0, 0.0);
        let pan_chase = pan_for_fixture(
            &chase, FixtureRole::Accent, StageVisualKind::Beam,
            &OscillatorFrameSnapshot::from_cue(&chase), 0, 2,
        );
        assert!(
            (pan_chase - 0.5).abs() > (pan_a - 0.5).abs() + 0.05,
            "full license chases the anchors off-center: chase={pan_chase} vs scan={pan_a}"
        );
    }

    #[test]
    fn mover_group_phase_reduces_to_part_a_and_scales_with_symmetry() {
        // 2-mover rig: ALWAYS the Part A 0.0/0.5 counter-phase, for ANY symmetry —
        // so B4 never regresses the committed slice.
        for sym in [0u8, 12, 24] {
            assert_eq!(mover_group_phase(0, 2, sym), 0.0, "head 0 phase @ sym={sym}");
            assert_eq!(mover_group_phase(1, 2, sym), 0.5, "head 1 phase @ sym={sym}");
        }
        // Single mover / none: phase 0.0 (no group to spread).
        assert_eq!(mover_group_phase(0, 1, 12), 0.0);
        // Big rig: symmetry sets the number of phase groups (value scales w/ rig).
        let distinct = |sym: u8, count: usize| {
            let mut set = std::collections::BTreeSet::new();
            for o in 0..count {
                set.insert((mover_group_phase(o, count, sym) * 1000.0).round() as i32);
            }
            set.len()
        };
        assert_eq!(distinct(12, 20), 12, "symmetry 12 -> 12 phase groups on a 20-mover rig");
        assert_eq!(distinct(24, 20), 20, "symmetry 24 -> 20 groups (capped at count)");
        assert!(distinct(24, 20) > distinct(12, 20), "more symmetry = more groups on a big rig");
        for o in 0..20 {
            assert!((0.0..1.0).contains(&mover_group_phase(o, 20, 24)), "phase in [0,1)");
        }
    }

    #[test]
    fn qc_overlay_boosts_accent_and_rotates_lattice_port() {
        // qc_accent_boost: 1.0 at no lattice (Part A baseline / no-op) up to 1.0+BOOST.
        let none = CueSnapshot { qc_strength: 0.0, ..Default::default() };
        let full = CueSnapshot { qc_strength: 1.0, ..Default::default() };
        assert_eq!(qc_accent_boost(&none), 1.0, "no lattice = Part A baseline accent (no-op)");
        assert!((qc_accent_boost(&full) - (1.0 + QC_ACCENT_BOOST)).abs() < 1e-6, "full lattice punches harder");
        assert!(qc_accent_boost(&full) > qc_accent_boost(&none));
        // mover_port: active=0 reproduces the Part A 0/2 base (no-op); the two heads
        // stay antipodal (mirror pair, differ by 2) for ANY active port, incl. clamp.
        assert_eq!(mover_port(0, 0), 0);
        assert_eq!(mover_port(1, 0), 2);
        for active in 0u8..6 {
            let p0 = mover_port(0, active);
            let p1 = mover_port(1, active);
            assert!(p0 < 4 && p1 < 4, "ports in range for active_port={active}");
            assert_eq!((p1 + 4 - p0) % 4, 2, "heads stay antipodal for active_port={active}");
        }
        // B3 colour punch: a lattice lock-in (qc_strength) raises colour saturation
        // (larger channel spread) vs no lattice — the colour companion to the accent.
        let wash = FixturePatch::demo_rgbw_wash("w", 0, 1);
        let lock = CueSnapshot { qc_strength: 1.0, energy: 0.6, ..Default::default() };
        let unlock = CueSnapshot { qc_strength: 0.0, energy: 0.6, ..Default::default() };
        let osc = OscillatorFrameSnapshot::from_cue(&lock);
        let (rl, gl, bl, _) = color_for_fixture(&lock, wash.role, wash.visual_kind, 1.0, 0.1, &osc);
        let (ru, gu, bu, _) = color_for_fixture(&unlock, wash.role, wash.visual_kind, 1.0, 0.1, &osc);
        let spread = |r: f32, g: f32, b: f32| r.max(g).max(b) - r.min(g).min(b);
        assert!(spread(rl, gl, bl) > spread(ru, gu, bu), "lattice lock-in punches colour saturation");
    }

    #[test]
    fn trust_gates_saturation_and_movement_but_not_brightness() {
        // High coherence => full saturation + movement (no-op 1.0); atonal => floored
        // but never grey/frozen; neither touches brightness (the drop rips on energy).
        let coherent = CueSnapshot { trust: 1.0, ..Default::default() };
        let atonal = CueSnapshot { trust: 0.0, ..Default::default() };
        assert_eq!(trust_saturation(&coherent), 1.0, "coherent = full saturation (B2 no-op)");
        assert_eq!(trust_motion(&coherent), 1.0, "coherent = full movement (B2 no-op)");
        assert!((trust_saturation(&atonal) - TRUST_SAT_FLOOR).abs() < 1e-6, "atonal floored, not grey");
        assert!((trust_motion(&atonal) - TRUST_MOTION_FLOOR).abs() < 1e-6, "atonal relaxes, not frozen");
        assert!(trust_saturation(&atonal) < trust_saturation(&coherent), "atonal is less saturated");
        assert!(trust_motion(&atonal) < trust_motion(&coherent), "atonal moves less");

        // A noisy DROP still rips: at full energy the colour VALUE (brightness) is
        // trust-independent, while the channel spread (saturation) shrinks for atonal.
        let wash = FixturePatch::demo_rgbw_wash("w", 0, 1);
        let drop_atonal = CueSnapshot { trust: 0.0, energy: 1.0, ..Default::default() };
        let drop_tonal = CueSnapshot { trust: 1.0, energy: 1.0, ..Default::default() };
        let osc = OscillatorFrameSnapshot::from_cue(&drop_atonal);
        let (ra, ga, ba, _) = color_for_fixture(&drop_atonal, wash.role, wash.visual_kind, 1.0, 0.1, &osc);
        let (rt, gt, bt, _) = color_for_fixture(&drop_tonal, wash.role, wash.visual_kind, 1.0, 0.1, &osc);
        let bright = |r: f32, g: f32, b: f32| r.max(g).max(b);
        let spread = |r: f32, g: f32, b: f32| bright(r, g, b) - r.min(g).min(b);
        assert!((bright(ra, ga, ba) - bright(rt, gt, bt)).abs() < 1e-3, "brightness is trust-independent (drop rips)");
        assert!(spread(ra, ga, ba) < spread(rt, gt, bt), "atonal field is desaturated (smaller channel spread)");
    }

    #[test]
    fn journey_macro_shapes_intensity_and_motion_across_phases() {
        let snap = |phase: HeroAct, t: f32| CueSnapshot {
            journey_phase: phase,
            journey_phase_time: t,
            ..Default::default()
        };
        // Birth EMERGES: dimmer rises with time-in-phase; movement stays narrow (<1).
        let birth0 = journey_dimmer_scale(&snap(HeroAct::Birth, 0.0));
        let birth_late = journey_dimmer_scale(&snap(HeroAct::Birth, 100.0));
        assert!(birth0 < birth_late, "Birth dimmer emerges: {birth0} < {birth_late}");
        assert!(birth0 >= 0.0 && birth_late <= 1.0, "Birth dimmer in range");
        assert!(journey_motion_scale(&snap(HeroAct::Birth, 0.0)) < 1.0, "Birth movement is narrow");
        // Lock = FULL on both axes.
        assert_eq!(journey_dimmer_scale(&snap(HeroAct::Lock, 5.0)), 1.0);
        assert_eq!(journey_motion_scale(&snap(HeroAct::Lock, 5.0)), 1.0);
        // Dive = a MOTION accent on entry that cools toward 1.0 (dimmer stays full).
        let dive_in = journey_motion_scale(&snap(HeroAct::Dive, 0.0));
        let dive_late = journey_motion_scale(&snap(HeroAct::Dive, 100.0));
        assert!(dive_in > 1.0 && dive_in > dive_late, "Dive accents then cools: {dive_in} -> {dive_late}");
        assert_eq!(journey_dimmer_scale(&snap(HeroAct::Dive, 0.0)), 1.0, "Dive dimmer stays full");
        // Afterglow FADES to a sustained floor on both axes — never to zero.
        let glow_in = journey_dimmer_scale(&snap(HeroAct::Afterglow, 0.0));
        let glow_late = journey_dimmer_scale(&snap(HeroAct::Afterglow, 100.0));
        assert!(glow_in > glow_late, "Afterglow dimmer fades: {glow_in} -> {glow_late}");
        assert!(glow_late >= JOURNEY_AFTERGLOW_DIMMER_FLOOR - 1e-6, "fade floored, never black");
        assert!(
            journey_motion_scale(&snap(HeroAct::Afterglow, 100.0)) >= JOURNEY_AFTERGLOW_MOTION_FLOOR - 1e-6,
            "Afterglow movement relaxes to a floor, not a stop"
        );
    }
    use crate::emerge::contracts::{ChannelMap, FixturePatch, StageVec3};

    #[test]
    fn intent_changes_macro_on_phrase_boundary() {
        let fixture = FixturePatch {
            screen: None,
            footprint: 0,
            id: "laser".to_string(),
            name: "laser".to_string(),
            universe: 0,
            start_address: 1,
            profile_id: "laser".to_string(),
            mode: "macro".to_string(),
            position: StageVec3::zero(),
            rotation: StageVec3::zero(),
            scale: StageVec3::one(),
            role: FixtureRole::Strobe,
            visual_kind: StageVisualKind::Laser,
            tags: vec![],
            channels: ChannelMap::laser_macro(1),
        };
        let first = CueSnapshot { frame_time: 0.0, dominant_pc: 3, ..Default::default() };
        let next = CueSnapshot { frame_time: 32.0, dominant_pc: 3, ..Default::default() };
        let a = render_fixture_intent(&first, &fixture, &OscillatorFrameSnapshot::from_cue(&first), None, 0, 1);
        let b = render_fixture_intent(&next, &fixture, &OscillatorFrameSnapshot::from_cue(&next), None, 0, 1);
        assert_ne!(a.laser_macro, b.laser_macro);
    }

    #[test]
    fn intent_blackout_is_left_to_frame_layer() {
        let fixture = FixturePatch::demo_rgbw_wash("wash", 0, 1);
        let cue = CueSnapshot { trigger_blackout: true, energy: 1.0, ..Default::default() };
        let intent = render_fixture_intent(&cue, &fixture, &OscillatorFrameSnapshot::from_cue(&cue), None, 0, 1);
        assert!(intent.dimmer > 0.0);
    }

    #[test]
    fn intent_renders_through_clock_locked_oscillator() {
        // COM-499 Phase A: production builds the oscillator via `from_clock`
        // (fixture_brain.rs:442, mapping.rs, virtual_stage.rs). The `from_cue`
        // tests in this module no longer cover that path — exercise it directly
        // so the live render path has coverage.
        let fixture = FixturePatch::demo_rgbw_wash("wash", 0, 1);
        let clock = crate::emerge::ShowClockSnapshot {
            beat: 9.0,
            phase: 0.25,
            quantum: 4.0,
            ..Default::default()
        };
        let cue = CueSnapshot { energy: 0.7, energy_slow: 0.7, ..Default::default() };
        let osc =
            OscillatorFrameSnapshot::from_clock(&cue, crate::emerge::PhaseState::from_show_clock(clock));
        let intent = render_fixture_intent(&cue, &fixture, &osc, None, 0, 1);
        // Sane, finite, in-range dimmer off the real clock — no panic / NaN.
        assert!(intent.dimmer.is_finite());
        assert!(
            (0.0..=1.0).contains(&intent.dimmer),
            "dimmer in range: {}",
            intent.dimmer
        );
    }

    fn obsession_patch() -> FixturePatch {
        FixturePatch {
            screen: None,
            footprint: 0,
            id: "obsession".to_string(),
            name: "obsession".to_string(),
            universe: 0,
            start_address: 1,
            profile_id: "obsession".to_string(),
            mode: "Standard".to_string(),
            position: StageVec3::zero(),
            rotation: StageVec3::zero(),
            scale: StageVec3::one(),
            role: FixtureRole::Accent,
            // The live rig tags the Obsessions `Gobo` (SaaS-authored), not `Beam`.
            visual_kind: StageVisualKind::Gobo,
            tags: vec![],
            // Static colour-wheel effect: dimmer + colour + gobo, NO pan/tilt.
            channels: ChannelMap {
                dimmer: Some(3),
                color_wheel: Some(5),
                gobo_slot: Some(6),
                gobo_rotation: Some(7),
                ..Default::default()
            },
        }
    }

    /// A Shehds GalaxyJet 300W beam as the live patch would carry it: a moving head
    /// (pan/tilt present) with the GalaxyJet ATLABASE profile id.
    fn galaxyjet_patch() -> FixturePatch {
        FixturePatch {
            id: "galaxyjet".to_string(),
            name: "GalaxyJet LED 300W Beam 1".to_string(),
            profile_id: GALAXYJET_PROFILE_ID.to_string(),
            visual_kind: StageVisualKind::Beam,
            channels: ChannelMap {
                dimmer: Some(8),
                color_wheel: Some(9),
                gobo_slot: Some(10),
                gobo_rotation: Some(11),
                pan: Some(1),
                tilt: Some(3),
                ..Default::default()
            },
            ..obsession_patch()
        }
    }

    #[test]
    fn effect_kind_classifies_obsession_galaxyjet_and_movers() {
        // The Obsession (static colour-wheel, Gobo-tagged in the live rig) is the
        // refined static Moonflower.
        assert_eq!(effect_kind(&obsession_patch()), Some(EffectKind::Moonflower));
        // A Beam-tagged Obsession (in case SaaS retags it) still resolves Moonflower.
        assert_eq!(
            effect_kind(&FixturePatch { visual_kind: StageVisualKind::Beam, ..obsession_patch() }),
            Some(EffectKind::Moonflower)
        );
        // The GalaxyJet — a moving head with pan/tilt — is the GalaxyBeam flavour,
        // matched by its ATLABASE profile id even though it carries pan/tilt.
        assert_eq!(effect_kind(&galaxyjet_patch()), Some(EffectKind::GalaxyBeam));
        // Name fallback: an operator-renamed GalaxyJet on some other profile id still
        // resolves GalaxyBeam.
        assert_eq!(
            effect_kind(&FixturePatch {
                profile_id: "some-other-id".to_string(),
                name: "Front Galaxy 3".to_string(),
                ..galaxyjet_patch()
            }),
            Some(EffectKind::GalaxyBeam)
        );
        // A 7R moving head (Beam + pan/tilt, NOT a GalaxyJet) is NOT an effect — it
        // keeps its own mover behaviour. Channel-identical to the GalaxyJet, so only
        // the identity check separates them.
        let mover = FixturePatch {
            name: "7R Beam Move Head Light 1".to_string(),
            profile_id: "01cc2583-e793-4efc-8b49-448ac7d4ab21".to_string(),
            visual_kind: StageVisualKind::Beam,
            channels: ChannelMap {
                dimmer: Some(3),
                color_wheel: Some(93),
                pan: Some(102),
                tilt: Some(104),
                ..Default::default()
            },
            ..obsession_patch()
        };
        assert_eq!(effect_kind(&mover), None);
    }

    #[test]
    fn effect_dimmer_moonflower_dark_in_quiet_present_in_groove() {
        // Quiet intro/breakdown: fully dark (no bed floor) — the "stop always-on" fix.
        let quiet = CueSnapshot { excitement: 0.0, energy: 0.0, reactivity: 1.0, ..Default::default() };
        assert_eq!(
            effect_dimmer(&quiet, &OscillatorFrameSnapshot::from_cue(&quiet), EffectKind::Moonflower),
            0.0
        );
        // Mid-energy groove: present (energy_slow in the on-window, below the loud edge).
        let hot = CueSnapshot { excitement: 0.8, energy: 0.6, energy_slow: 0.6, reactivity: 1.0, ..Default::default() };
        assert!(effect_dimmer(&hot, &OscillatorFrameSnapshot::from_cue(&hot), EffectKind::Moonflower) > 0.2);
        // A quiet-but-TENSE riser stays LIT — the Moonflower keys on loudness alone,
        // not harmonic tension (the 7R beams own the tension retreat).
        let tense_quiet = CueSnapshot { tension_bank: 0.95, energy: 0.5, energy_slow: 0.55, reactivity: 1.0, ..Default::default() };
        assert!(effect_dimmer(&tense_quiet, &OscillatorFrameSnapshot::from_cue(&tense_quiet), EffectKind::Moonflower) > 0.2);
    }

    #[test]
    fn effect_dimmer_moonflower_retreats_at_peak_loudness() {
        // Koob: the Obsession turns OFF when the section gets LOUD — a band-pass on
        // section energy. Present in the lower-energy groove, dark at the climax.
        let groove = CueSnapshot { energy: 0.55, energy_slow: 0.55, reactivity: 1.0, ..Default::default() };
        let loud = CueSnapshot { energy: 0.95, energy_slow: 0.95, reactivity: 1.0, ..Default::default() };
        let d_groove = effect_dimmer(&groove, &OscillatorFrameSnapshot::from_cue(&groove), EffectKind::Moonflower);
        let d_loud = effect_dimmer(&loud, &OscillatorFrameSnapshot::from_cue(&loud), EffectKind::Moonflower);
        assert!(d_groove > 0.2, "present in the lower-energy groove");
        assert!(d_loud < 0.05, "retreats to dark at peak loudness");
        // A DROP at peak loudness still slams back — the release punch beats the retreat.
        let loud_drop = CueSnapshot { energy: 0.95, energy_slow: 0.95, drop_impact: 0.9, reactivity: 1.0, ..Default::default() };
        assert!(
            effect_dimmer(&loud_drop, &OscillatorFrameSnapshot::from_cue(&loud_drop), EffectKind::Moonflower) > 0.5,
            "drop slam beats the loud retreat"
        );
    }

    #[test]
    fn galaxy_dimmer_emerges_on_energy_and_cuts_on_tension() {
        // Energetic section, low tension => present + breathing (energy_slow above the
        // presence band, no bed floor pinning it).
        let lit = CueSnapshot { excitement: 0.5, energy: 0.7, energy_slow: 0.75, high: 0.6, tension: 0.1, tension_bank: 0.1, reactivity: 1.0, ..Default::default() };
        let d_lit = effect_dimmer(&lit, &OscillatorFrameSnapshot::from_cue(&lit), EffectKind::GalaxyBeam);
        assert!(d_lit > 0.2, "present when the section energy is up, low tension");
        // Low-energy groove => OFF (the energy gate floors it to zero, no bed floor).
        let groove = CueSnapshot { excitement: 0.5, energy: 0.2, energy_slow: 0.1, high: 0.6, tension: 0.0, tension_bank: 0.0, reactivity: 1.0, ..Default::default() };
        assert!(effect_dimmer(&groove, &OscillatorFrameSnapshot::from_cue(&groove), EffectKind::GalaxyBeam) < 0.05, "fully dark in a low-energy groove");
        // A tension BUILD darkens them even when the section is energetic — via the 7R's
        // own signal (tension_bank), fully dark (hotter than the 7R, no floor).
        let build = CueSnapshot { excitement: 0.5, energy: 0.7, energy_slow: 0.75, high: 0.6, tension_bank: 0.8, tension: 0.0, reactivity: 1.0, ..Default::default() };
        assert!(effect_dimmer(&build, &OscillatorFrameSnapshot::from_cue(&build), EffectKind::GalaxyBeam) < 0.05, "fully dark on a build (tension_bank)");
        // The visible Tension meter (snapshot.tension) also cuts them, whichever is higher.
        let tense = CueSnapshot { excitement: 0.5, energy: 0.7, energy_slow: 0.75, high: 0.6, tension: 0.85, tension_bank: 0.0, reactivity: 1.0, ..Default::default() };
        assert!(effect_dimmer(&tense, &OscillatorFrameSnapshot::from_cue(&tense), EffectKind::GalaxyBeam) < 0.05, "fully dark at high visible tension");
        // The drop punches them back on the release even mid-tension (presence is up).
        let drop = CueSnapshot { excitement: 0.5, energy: 0.7, energy_slow: 0.75, high: 0.6, tension: 0.85, tension_bank: 0.8, drop_impact: 0.9, reactivity: 1.0, ..Default::default() };
        assert!(
            effect_dimmer(&drop, &OscillatorFrameSnapshot::from_cue(&drop), EffectKind::GalaxyBeam) > 0.5,
            "drop punches through the tension retreat"
        );
    }

    #[test]
    fn galaxybeam_and_moonflower_diverge() {
        // The fork keys the two flavours off different axes. At a LOUD, energetic,
        // LOW-tension moment the GalaxyJet is present (it manages its on-state like the
        // 7R — treble/harmony base, dark only on tension) while the refined Obsession
        // has receded (its energy_slow band-pass is dark at peak loudness).
        let loud = CueSnapshot {
            excitement: 0.95, energy: 0.95, energy_slow: 0.95,
            high: 0.7, dominant_env: 0.5, tension: 0.1, reactivity: 1.0,
            ..Default::default()
        };
        let osc = OscillatorFrameSnapshot::from_cue(&loud);
        let galaxy = effect_dimmer(&loud, &osc, EffectKind::GalaxyBeam);
        let moon = effect_dimmer(&loud, &osc, EffectKind::Moonflower);
        assert!(galaxy > 0.2, "GalaxyJet present at a loud, low-tension moment");
        assert!(moon < 0.05, "the refined Obsession has receded at peak loudness");
        assert!(galaxy > moon, "the two flavours diverge");
    }

    #[test]
    fn effect_color_wheel_stays_in_static_band() {
        for pc in 0u8..12 {
            let cue = CueSnapshot { dominant_pc: pc, ..Default::default() };
            assert!(effect_color_wheel(&cue) <= 139, "static colour band only");
        }
    }

    #[test]
    fn effect_rotation_speeds_with_energy_and_flips_on_lattice_commit() {
        // Faster (further from the 0.5 stop point) as energy rises.
        let calm = CueSnapshot { energy: 0.0, excitement: 0.0, polarity_quadrant: 0, ..Default::default() };
        let hot = CueSnapshot { energy: 0.9, excitement: 0.8, polarity_quadrant: 0, ..Default::default() };
        assert!(
            (effect_rotation(&calm) - 0.5).abs() < (effect_rotation(&hot) - 0.5).abs(),
            "rotation speeds up with energy"
        );
        // Opposite polarity quadrants (lattice commits) rotate opposite directions.
        let q0 = CueSnapshot { energy: 0.6, polarity_quadrant: 0, ..Default::default() };
        let q1 = CueSnapshot { energy: 0.6, polarity_quadrant: 1, ..Default::default() };
        assert!(effect_rotation(&q0) < 0.5 && effect_rotation(&q1) > 0.5, "lattice commit flips direction");
        // Bounded.
        let v = effect_rotation(&CueSnapshot { energy: 1.0, excitement: 1.0, polarity_quadrant: 1, ..Default::default() });
        assert!((0.0..=1.0).contains(&v));
    }

    #[test]
    fn manual_strobe_trigger_survives_the_show_brain_gate() {
        use crate::emerge::cue::show_brain::ShowBrain;
        use crate::emerge::ClockInputs;
        use crate::render::LightingMetrics;

        // Silence → the show-brain rules the strobe lane ineligible (gate shut).
        let mut brain = ShowBrain::new();
        let frame = brain.tick(
            &CueSnapshot::default(),
            &LightingMetrics::default(),
            &ClockInputs::default(),
        );
        assert!(!frame.lane(FixtureFamily::Strobe).eligible);

        let fixture = FixturePatch {
            screen: None,
            footprint: 0,
            id: "strobe".to_string(),
            name: "strobe".to_string(),
            universe: 0,
            start_address: 1,
            profile_id: "strobe".to_string(),
            mode: "strobe".to_string(),
            position: StageVec3::zero(),
            rotation: StageVec3::zero(),
            scale: StageVec3::one(),
            role: FixtureRole::Strobe,
            visual_kind: StageVisualKind::Strobe,
            tags: vec![],
            channels: ChannelMap::laser_macro(1),
        };
        // Operator hits STROBE while the lane is ineligible — must still fire.
        let cue = CueSnapshot { trigger_strobe: true, ..Default::default() };
        let intent = render_fixture_intent(
            &cue,
            &fixture,
            &OscillatorFrameSnapshot::from_cue(&cue),
            Some(&frame),
            0,
            1,
        );
        assert!(intent.strobe > 0.0, "manual strobe must survive the show-brain gate");
    }

    #[test]
    fn show_brain_gate_darkens_every_emitting_field_of_a_gated_laser() {
        use crate::emerge::cue::show_brain::ShowBrain;
        use crate::emerge::ClockInputs;
        use crate::render::LightingMetrics;

        // Silence → the show-brain rules the laser lane ineligible (gate shut).
        let mut brain = ShowBrain::new();
        let frame = brain.tick(
            &CueSnapshot::default(),
            &LightingMetrics::default(),
            &ClockInputs::default(),
        );
        assert!(!frame.lane(FixtureFamily::Laser).eligible);

        let fixture = FixturePatch {
            screen: None,
            footprint: 0,
            id: "laser".to_string(),
            name: "laser".to_string(),
            universe: 0,
            start_address: 1,
            profile_id: "laser".to_string(),
            mode: "macro".to_string(),
            position: StageVec3::zero(),
            rotation: StageVec3::zero(),
            scale: StageVec3::one(),
            role: FixtureRole::Strobe,
            visual_kind: StageVisualKind::Laser,
            tags: vec![],
            channels: ChannelMap::laser_macro(1),
        };
        // A hot musical moment so every emitting field would be non-zero
        // without the gate: color_for_fixture floors val at 0.35 and forces
        // laser sat to 1.0 (RGB lit even at dimmer 0), drop_impact arms the
        // prism, and the Strobe-role white rides the dimmer. dmx/mapping.rs
        // writes RGB unconditionally — "gated dark" must mean EVERY emitting
        // field at zero or an RGB-channel laser keeps painting the room.
        let cue = CueSnapshot {
            energy: 0.9,
            energy_slow: 0.9,
            drop_impact: 0.6,
            reactivity: 1.0,
            ..Default::default()
        };
        let intent = render_fixture_intent(
            &cue,
            &fixture,
            &OscillatorFrameSnapshot::from_cue(&cue),
            Some(&frame),
            0,
            1,
        );
        assert_eq!(intent.dimmer, 0.0, "gated laser dimmer dark");
        assert_eq!(intent.strobe, 0.0, "gated laser strobe dark");
        assert_eq!(intent.laser_macro, 0, "gated laser macro off");
        assert_eq!(intent.red, 0.0, "gated laser red dark");
        assert_eq!(intent.green, 0.0, "gated laser green dark");
        assert_eq!(intent.blue, 0.0, "gated laser blue dark");
        assert_eq!(intent.white, 0.0, "gated laser white dark");
        assert_eq!(intent.prism, 0.0, "gated laser prism off");
    }

    #[test]
    fn smoothstep_ramps_and_clamps() {
        assert_eq!(smoothstep(0.2, 0.8, 0.1), 0.0, "clamps below lo");
        assert_eq!(smoothstep(0.2, 0.8, 0.9), 1.0, "clamps above hi");
        let mid = smoothstep(0.2, 0.8, 0.5);
        assert!(mid > 0.4 && mid < 0.6, "smooth in the middle: {mid}");
    }

    #[test]
    fn beam_tension_gate_fades_full_to_off() {
        // Beams full at low tension, dark at high tension, monotonically fading
        // across the band — the retreat-on-build / slam-back-on-release behavior.
        let gate = |t: f32| 1.0 - smoothstep(BEAM_TENSION_OFF_LOW, BEAM_TENSION_OFF_HIGH, t);
        assert!((gate(0.0) - 1.0).abs() < 1e-6, "full at no tension");
        assert!(gate(0.95) < 1e-6, "dark at max tension");
        assert!(gate(BEAM_TENSION_OFF_LOW) > gate(BEAM_TENSION_OFF_HIGH), "fades as tension builds");
    }

    #[test]
    fn consonance_saturation_eases_with_balance() {
        let ceil = 0.74;
        let dissonant = consonance_saturation(ceil, 0.0);
        let consonant = consonance_saturation(ceil, 1.0);
        assert!(dissonant > consonant, "a dissonant field saturates more than a consonant one");
        assert!(consonant >= SAT_FLOOR, "saturation never drops below the floor");
        assert!(dissonant <= ceil, "saturation never exceeds the per-kind ceiling");
    }

    #[test]
    fn tonal_temperature_is_bipolar_around_neutral() {
        assert!(tonal_temperature(1.0) > 0.0, "a major field is warm");
        assert!(tonal_temperature(0.0) < 0.0, "a minor field is cool");
        assert!(tonal_temperature(0.5).abs() < 1e-6, "0.5 is the neutral / ambiguous mood");
    }

    #[test]
    fn bed_floor_lifts_with_excitement() {
        assert!((bed_floor(0.0) - BED_FLOOR_BASE).abs() < 1e-6, "calm bed sits at the base floor");
        assert!(bed_floor(1.0) > bed_floor(0.0), "arc development lifts the bed floor");
    }

    #[test]
    fn dissonance_fan_widens_as_stability_drops() {
        // High stability => fixtures cluster on the anchor (fan ~ 0). Low stability
        // => fixtures fan out, and two fixtures (different phase) land on different
        // hues, within the tasteful cap.
        assert!(dissonance_fan(1.0, 0.1).abs() < 1e-6, "stable field clusters on the anchor");
        assert!(dissonance_fan(1.0, 0.9).abs() < 1e-6, "stable field clusters on the anchor");
        let wild_a = dissonance_fan(0.0, 0.1);
        let wild_b = dissonance_fan(0.0, 0.9);
        assert!(wild_a != wild_b, "a dissonant field fans fixtures to different hues");
        assert!(wild_a.abs() <= BLOOM_SPREAD_CAP, "fan stays within the tasteful cap");
        assert!(wild_b.abs() <= BLOOM_SPREAD_CAP, "fan stays within the tasteful cap");
    }

    #[test]
    fn bed_hue_drift_advances_including_multiday_runtime() {
        // Small-t: the calm bed drifts colour over time.
        assert!(bed_hue_drift(0.0).abs() < 1e-6, "no drift at engine start");
        assert!(bed_hue_drift(120.0) > bed_hue_drift(60.0), "the calm bed slowly drifts colour over time");
        // 24/7 regression guard: at multi-day frame_time a raw frame_time*RATE
        // loses the per-frame increment to f32 rounding (the breath freezes). The
        // time-domain wrap must keep it advancing across a second of frames.
        let t0 = 172_800.0_f32; // ~day 2 of continuous uptime
        let dt = 1.0 / 60.0;
        let start = bed_hue_drift(t0);
        let mut prev = start;
        let mut advanced = 0;
        for i in 1..=60 {
            let d = bed_hue_drift(t0 + dt * i as f32);
            if d != prev {
                advanced += 1;
            }
            prev = d;
        }
        assert!(advanced >= 30, "bed drift must keep advancing per-frame at multi-day runtime, advanced {advanced}/60");
        let net = (prev - start).rem_euclid(1.0);
        assert!(net > 1e-4, "non-negligible per-second advance at multi-day runtime: {net}");
    }

    #[test]
    fn major_field_lifts_the_white_body_vs_minor() {
        // The temperature term lifts the white channel in a major field and
        // hollows it in a minor field, on a non-strobe / non-blinder wash.
        let major = CueSnapshot { major_smooth: 1.0, energy: 0.6, balance: 0.5, ..Default::default() };
        let minor = CueSnapshot { major_smooth: 0.0, energy: 0.6, balance: 0.5, ..Default::default() };
        // The oscillator only feeds wash hue rotation, not the white channel.
        let (_, _, _, white_major) = color_for_fixture(
            &major, FixtureRole::Wash, StageVisualKind::Wash, 0.5, 0.5,
            &OscillatorFrameSnapshot::from_cue(&major),
        );
        let (_, _, _, white_minor) = color_for_fixture(
            &minor, FixtureRole::Wash, StageVisualKind::Wash, 0.5, 0.5,
            &OscillatorFrameSnapshot::from_cue(&minor),
        );
        assert!(white_major > white_minor, "major lifts the white body relative to minor");
    }

    #[test]
    fn contains_ignore_ascii_case_scans_without_allocating() {
        // Mixed case, both sides.
        assert!(contains_ignore_ascii_case("GalaxyJet LED 300W Beam 1", "galaxy"));
        assert!(contains_ignore_ascii_case("FRONT GALAXY 3", "galaxy"));
        assert!(contains_ignore_ascii_case("front galaxy 3", "GALAXY"));
        // Miss.
        assert!(!contains_ignore_ascii_case("7R Beam Move Head Light 1", "galaxy"));
        // Empty needle matches everything (mirrors `str::contains("")`).
        assert!(contains_ignore_ascii_case("anything", ""));
        assert!(contains_ignore_ascii_case("", ""));
        // Empty / too-short haystack never matches a non-empty needle.
        assert!(!contains_ignore_ascii_case("", "galaxy"));
        assert!(!contains_ignore_ascii_case("gal", "galaxy"));
    }

    #[test]
    fn effect_kind_matches_galaxy_in_profile_id_case_insensitively() {
        // The fallback scans name AND profile_id separately (the old code
        // concatenated them into a lowercased String per call — two heap
        // allocations per fixture per frame on the render path).
        assert_eq!(
            effect_kind(&FixturePatch {
                profile_id: "custom-GALAXY-clone".to_string(),
                name: "Spot 1".to_string(),
                ..galaxyjet_patch()
            }),
            Some(EffectKind::GalaxyBeam)
        );
    }

    #[test]
    fn segment_brightness_band_follows_the_field_rotation_phase() {
        // The bar's brightness comes from the EMeRGE field: a bright band sits
        // where the field rotation `phase` points. With symmetry=1 and
        // phase=0.4 on a 6-cell bar, the band peaks at cell 2 (frac 0.4).
        let cue = CueSnapshot {
            phase: 0.4,
            symmetry: 1,
            journey_phase: HeroAct::Lock, // full arc level so the band reads
            trust: 1.0,
            ..Default::default()
        };
        let count = 6;
        let brightness: Vec<f32> = (0..count)
            .map(|seg| {
                let (r, g, b, _) = segment_rgbw(&cue, 1.0, seg, count);
                r.max(g).max(b)
            })
            .collect();
        let peak = brightness
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        assert_eq!(peak, 2, "field phase 0.4 lights cell 2 of 6 brightest: {brightness:?}");

        // The band sweeps with the phase: advancing it moves the peak.
        let cue2 = CueSnapshot { phase: 0.8, ..cue };
        let b2: Vec<f32> = (0..count)
            .map(|seg| {
                let (r, g, b, _) = segment_rgbw(&cue2, 1.0, seg, count);
                r.max(g).max(b)
            })
            .collect();
        let peak2 = b2.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).map(|(i, _)| i).unwrap();
        assert!(peak2 > peak, "advancing the field phase sweeps the band: {b2:?}");
    }

    #[test]
    fn segment_brightness_arc_sets_the_overall_level() {
        // The field intensity arc (Hero journey) sets the bar's overall level:
        // an Afterglow tail is dimmer than a Lock peak at the same cell/energy.
        let cell = |journey: HeroAct, t: f32| -> f32 {
            let cue = CueSnapshot {
                phase: 0.0,
                symmetry: 1,
                journey_phase: journey,
                journey_phase_time: t,
                trust: 1.0,
                ..Default::default()
            };
            let (r, g, b, _) = segment_rgbw(&cue, 1.0, 0, 6);
            r.max(g).max(b)
        };
        let lock = cell(HeroAct::Lock, 0.0);
        let afterglow = cell(HeroAct::Afterglow, 14.0); // late in the fade
        assert!(lock > afterglow, "Lock ({lock}) brighter than a faded Afterglow ({afterglow})");
    }

    #[test]
    fn segment_rgbw_still_gates_on_master_dimmer() {
        // The bar stays dark in blackout/silence regardless of the chase head.
        let cue = CueSnapshot { chase_head: 0.3, energy: 0.8, ..Default::default() };
        for seg in 0..8 {
            let (r, g, b, w) = segment_rgbw(&cue, 0.0, seg, 8);
            assert_eq!((r, g, b, w), (0.0, 0.0, 0.0, 0.0), "cell {seg} dark at dimmer 0");
        }
    }
}

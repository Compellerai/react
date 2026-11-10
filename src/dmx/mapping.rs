use std::collections::HashMap;

use crate::emerge::{FixtureFamily, PhaseState};
use crate::emerge::{render_fixture_intent, segment_rgbw, FixtureIntentState};
use crate::emerge::{apply_cue_bindings, ActiveCue, CueApplyReport};
use crate::emerge::OscillatorFrameSnapshot;
use crate::emerge::{family_for, ShowBrainFrame};
use crate::emerge::contracts::{
    segments_for_profile, ChannelMap, DmxFrame, DmxPatch, FixtureOverride, FixtureOverrides,
    FixturePatch, FixtureProfile, FixtureRole, SegmentPattern, StageVec3, StageVisualKind,
};
use crate::hue::CueSnapshot;

/// Resolved hardware profiles keyed by fixture id — lets the writer expand a
/// pixel bar's repeated color cells into per-segment DMX. Pass an empty map
/// for single-color rigs (or in tests).
pub type ProfileMap = HashMap<String, FixtureProfile>;

/// Floor the per-cell brightness when an operator Segment FX is engaged, so a
/// picked pattern stays visible even where the show would dim the panel. Scaled by
/// the per-fixture intensity and gated by mute/solo, so those controls still win.
const SEG_FX_FLOOR: f32 = 0.6;

pub fn render_dmx_frame(
    snapshot: &CueSnapshot,
    patch: &DmxPatch,
    profiles: &ProfileMap,
    gate: Option<&ShowBrainFrame>,
) -> DmxFrame {
    let mut reports = HashMap::new();
    // No operator overrides on this path (test/no-cue helper) — pass an empty set.
    let overrides = FixtureOverrides::default();
    render_dmx_frame_at(snapshot, patch, profiles, gate, &[], &mut reports, &overrides, 0, 0)
}

/// COM-499: render with active SaaS lighting cues overlaid on each fixture's
/// intent (intent-level composition, Appendix A choice A). `reports` collects
/// per-cue capability misses for the executor's `/report`.
#[allow(clippy::too_many_arguments)]
pub fn render_dmx_frame_with_cues(
    snapshot: &CueSnapshot,
    patch: &DmxPatch,
    profiles: &ProfileMap,
    gate: Option<&ShowBrainFrame>,
    cues: &[ActiveCue],
    reports: &mut HashMap<String, CueApplyReport>,
    overrides: &FixtureOverrides,
) -> DmxFrame {
    render_dmx_frame_at(snapshot, patch, profiles, gate, cues, reports, overrides, 0, 0)
}

#[allow(clippy::too_many_arguments)]
pub fn render_dmx_frame_at(
    snapshot: &CueSnapshot,
    patch: &DmxPatch,
    profiles: &ProfileMap,
    gate: Option<&ShowBrainFrame>,
    cues: &[ActiveCue],
    reports: &mut HashMap<String, CueApplyReport>,
    overrides: &FixtureOverrides,
    sequence: u64,
    timestamp_ms: u64,
) -> DmxFrame {
    let mut frame = DmxFrame::new(patch.universe, sequence, timestamp_ms, is_blackout(snapshot));

    if frame.blackout {
        return frame;
    }

    // Solo is rig-wide: once any fixture is soloed, every non-soloed fixture is
    // silenced. Computed once over the live patch so a stale solo entry for a
    // removed fixture can't silence everything.
    let any_solo = overrides.any_solo(patch);

    // COM-499 Phase A: lock the autopilot oscillators to the real show clock
    // (Ableton Link / Pro DJ / audio-derived) when the show brain has resolved
    // one, so every breath/sweep rides the track's beat/bar/phrase grid instead
    // of the 122-BPM wall clock. Falls back to `from_cue` (122 placeholder) only
    // for non-live paths (test fixtures, pre-init).
    let osc = match gate {
        Some(b) => {
            OscillatorFrameSnapshot::from_clock(snapshot, PhaseState::from_show_clock(b.clock))
        }
        None => OscillatorFrameSnapshot::from_cue(snapshot),
    };

    // Slice 2b: cue oscillators + step_chase phase-lock to the real show
    // clock (Ableton Link / Pro DJ / audio-derived) when the show brain has
    // resolved one. Fall back to deriving beats from `frame_time` at a
    // 122 BPM placeholder when no brain frame is present (test fixtures,
    // pre-init frames). The fallback only matters for non-live code paths
    // — production always carries a `ShowBrainFrame` here.
    //
    // `beat` stays f64 end-to-end: the cue evaluators reduce phase with
    // f64 `.fract()` before any f32 narrowing, so a multi-day uptime's huge
    // beat count (where an f32 ULP exceeds the per-frame advance) cannot
    // quantize fast strobes/chases.
    let beats = gate
        .map(|b| b.clock.beat)
        .unwrap_or_else(|| snapshot.frame_time as f64 * 122.0 / 60.0);

    // Emerge field -> autonomous movers: rank the Accent (mover) fixtures by DMX
    // address so the two GigBAR heads land on opposite harmonic ports (ordinal 0
    // -> port 0, ordinal 1 -> port 2) and counter-phase their sweep. Computed once
    // over this universe before the fixture loop.
    let mut mover_addrs: Vec<u16> = patch
        .fixtures_for_universe(patch.universe)
        .filter(|f| matches!(f.role, FixtureRole::Accent))
        .map(|f| f.start_address)
        .collect();
    mover_addrs.sort_unstable();

    for fixture in patch.fixtures_for_universe(patch.universe) {
        // A Screen is a placeable video surface, not a light — it never emits
        // DMX and never participates in solo/mute (mirrors the renderer skip in
        // virtual_stage.rs). Without this, soloing a screen would blacken the
        // whole real rig.
        if fixture.screen.is_some() {
            continue;
        }

        // Multi-section bars (the Chauvet GigBAR Move: 2 pars + 2 derbies +
        // laser + strobe + 2 moving heads) are ONE stage fixture / ONE combined
        // profile, but physically many sections that a single flat ChannelMap
        // can't all address. Expand the fixture into per-section render units —
        // each driven by the SAME intent engine + cue bindings + overrides — then
        // skip the flat single-fixture path below. `expand_sections` returns
        // `None` for ordinary single-section fixtures, which fall through
        // unchanged (no regression). Section overrides resolve by the PARENT
        // fixture id, so mute/solo/intensity on the bar still apply rig-wide.
        if let Some(sections) = expand_sections(fixture, profiles.get(&fixture.id)) {
            // Audio-reactive BAR GATE. A GigBAR is 8 emitters; left on the rig's
            // 24/7 `bed_floor` (which lingers ~3s via the slow `excitement` decay)
            // the whole bar reads as a wall of light that won't taper. Gate the
            // bar's intensity by the energy envelope so it punches with the music
            // and collapses in quiet — dynamic + fast-tapering, scoped to the bar
            // (the rest of the rig keeps its ambient bed). `reactivity` still trims
            // on top (it's baked into the per-section intent). Tunable via
            // BAR_GATE_LO/HI; a live MIDI-bindable knob is the follow-up.
            // A manual operator override (BLINDER/STROBE/UV) bypasses the quiet
            // collapse — the operator pressed the button, so it must fire with
            // or without music, not get gated to dark in a silent break.
            let bar_gain = if snapshot.manual_light_override() {
                1.0
            } else {
                bar_energy_gate(snapshot.energy)
            };
            // GigBAR full-hardware pass (35ch): capture the strobe pods' FINAL
            // rendered intent and the laser's lit state as the sections render,
            // then shape the manual-verified control bytes once after the loop.
            // Only the 35ch is self-managed (its hardware pass owns the strobe
            // pods); every other sectioned bar — the 17ch GigBAR especially —
            // has NO hardware pass and must take the software strobe like any
            // flat fixture, or a manual STROBE/buildup leaves it inert.
            let self_managed = profiles.get(&fixture.id).map_or(false, is_gigbar_35);
            let mut gig_live = GigbarLive::default();
            for sec in &sections {
                let mut intent = render_fixture_intent(
                    snapshot,
                    &sec.fixture,
                    &osc,
                    gate,
                    sec.mover_ordinal,
                    sec.mover_count,
                );
                // No per-section profile (sections aren't pixel bars), so
                // step_chase bindings simply report a capability miss.
                let overlay =
                    apply_cue_bindings(&mut intent, &sec.fixture, None, 0, cues, beats, reports);
                apply_global_overrides(&mut intent, &sec.fixture, snapshot, !self_managed);
                apply_per_fixture_overrides(&mut intent, &sec.fixture, overrides, any_solo);
                // Pars have no dimmer channel of their own (their dim/strobe slot
                // is a static combo), so fold the wash dimmer into RGB to keep the
                // music's brightness breath.
                if sec.fold_dimmer_into_rgb {
                    intent.red *= intent.dimmer;
                    intent.green *= intent.dimmer;
                    intent.blue *= intent.dimmer;
                    intent.white *= intent.dimmer;
                }
                // Collapse the bar's emitting intensity in quiet. Only intensity
                // fields scale — pan/tilt/colour-wheel/gobo/laser-macro are positions
                // or indices, not levels, so they're left alone (the laser/strobe
                // lanes are already show-brain gated on top).
                intent.dimmer *= bar_gain;
                intent.red *= bar_gain;
                intent.green *= bar_gain;
                intent.blue *= bar_gain;
                intent.white *= bar_gain;
                intent.strobe *= bar_gain;
                // Stamp each section channel's profile default FIRST (this is what
                // lights the pars: their dim/strobe channel carries a static full
                // value), then overlay the live intent on the mapped slots.
                for &(abs, val) in &sec.default_stamps {
                    frame.set_channel(Some(abs), val);
                }
                apply_fixture_intent(&mut frame, &sec.fixture, intent, overlay.gobo_exact);
                // Each kind appears at most once per bar (35ch: one strobe-pod
                // section, one laser section), so a plain overwrite is exact.
                match sec.fixture.visual_kind {
                    StageVisualKind::Strobe => gig_live.strobe_intent = Some(intent),
                    StageVisualKind::Laser => gig_live.laser_lit = intent.laser_macro > 0,
                    _ => {}
                }
            }
            // GigBAR Move 35ch hardware pass: the manual-verified strobe-engine /
            // par-punch / laser-texture / UV bytes the generic per-family intent
            // path cannot express. Mute/solo must hold here too — the pass reads
            // SNAPSHOT state, so without the silence flag a muted bar could still
            // strobe or UV-glow.
            if self_managed {
                let ov = overrides.get(&fixture.id);
                let silenced = if any_solo { !ov.solo } else { ov.mute };
                apply_gigbar_hardware_pass(
                    &mut frame,
                    fixture.start_address,
                    snapshot,
                    bar_gain,
                    &gig_live,
                    silenced,
                    ov.intensity,
                );
            }
            continue;
        }

        let mover_ordinal = mover_addrs
            .iter()
            .position(|&a| a == fixture.start_address)
            .unwrap_or(0);
        let mut intent = render_fixture_intent(snapshot, fixture, &osc, gate, mover_ordinal, mover_addrs.len());
        // Profile (when resolved) is needed by step_chase bindings whose
        // `values_from` path pulls steps off the fixture's gobo/prism/etc
        // detail; passing `None` makes step_chase fall through to a
        // capability-miss report on that binding. The segment list is resolved
        // ONCE here and its cell count handed to `apply_cue_bindings`, which
        // previously re-derived it per segment binding per frame.
        let profile = profiles.get(&fixture.id);
        let segments = profile
            .map(|p| segments_for_profile(p, fixture.start_address))
            .unwrap_or_default();
        let overlay =
            apply_cue_bindings(&mut intent, fixture, profile, segments.len(), cues, beats, reports);
        // Manual global triggers + Global Brightness act last so they win
        // over cue bindings and per-fixture lane logic. Blinder forces a
        // white wash on every non-laser family and zeros lasers; Global
        // Brightness scales every emitting channel uniformly.
        apply_global_overrides(&mut intent, fixture, snapshot, true);
        // Per-fixture operator override runs LAST so a mute/solo wins over a
        // blinder white-flash and the intensity trim stacks under the master.
        apply_per_fixture_overrides(&mut intent, fixture, overrides, any_solo);
        if segments.len() >= 2 {
            apply_segmented_fixture(&mut frame, fixture, intent, snapshot, &segments, overlay.segments.as_deref(), beats, overrides.get(&fixture.id), any_solo);
        } else {
            apply_fixture_intent(&mut frame, fixture, intent, overlay.gobo_exact);
        }
    }

    frame
}

fn is_blackout(snapshot: &CueSnapshot) -> bool {
    // Manual BLACKOUT always wins (operator intent, top of the trigger
    // priority order). The AUTONOMOUS blackout (`blackout_active` — the
    // CueEngine forces the stage dark after ~2s of raw silence) must YIELD
    // to a held manual override: BLINDER/STROBE/UV are top-level operator
    // overrides that fire with or without music, and the silence blackout
    // otherwise zeroes the whole frame before the override is even
    // considered — the "triggers don't fire without audio" symptom.
    snapshot.trigger_blackout
        || (snapshot.blackout_active && !snapshot.manual_light_override())
}

/// The global white override a fixture renders THIS frame, in FRONT of its
/// autopilot/cue look. Computed once per fixture and shared by the flat
/// (`apply_global_overrides`) and segmented (`apply_segmented_fixture`) render
/// paths, so the two can never disagree about when the rig strobes. The
/// decision is capability-agnostic — there are deliberately NO per-brand or
/// per-visual-kind cases here; the strobe "applies the same way to all
/// strobe-capable lights". The one physical distinction (a laser has no white)
/// is handled by each caller when it renders the result.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WhiteOverride {
    /// No override — render the normal look.
    None,
    /// Solid full white: the manual BLINDER hold, or a self-managed bar's coil.
    Solid,
    /// Strobe white on the shared wall-clock square wave (`snapshot.strobe_on`):
    /// the manual STROBE button, or the autonomous buildup (BlindersHold) / Pump
    /// (OverdriveStrobe).
    Strobe,
}

/// Decide a fixture's global white override. `software_strobe` is false only
/// for self-managed bars (the GigBAR drives its own strobe pods in its hardware
/// pass) — they take the solid coil here and shape the real strobe themselves.
fn white_override(snapshot: &CueSnapshot, software_strobe: bool) -> WhiteOverride {
    if snapshot.trigger_blinder {
        WhiteOverride::Solid
    } else if software_strobe
        && (snapshot.trigger_strobe
            || snapshot.auto_blinder_active
            || snapshot.overdrive_strobe_active)
    {
        WhiteOverride::Strobe
    } else if snapshot.auto_blinder_active {
        WhiteOverride::Solid
    } else {
        WhiteOverride::None
    }
}

/// Apply the final overrides every fixture's intent passes through before
/// it lands on the wire:
///   * Blinder — non-laser families (Wash/Strobe/Beam/Gobo/LedPanel) blast
///     full white; lasers zero out so the room reads as a clean operator
///     flash, never a "white wash with laser blasts" sandwich. Triggered by
///     the manual button (`trigger_blinder`) or the auto-blinder hold phase.
///   * UV mode (`trigger_uv`) — "blackout, except blacklight": every
///     emitting channel goes dark (the same set a mute silences) on EVERY
///     fixture, then fixtures with a `ChannelMap.uv` slot drive it at the
///     operator's `uv_level`. Movement/position channels (pan/tilt/gobo/
///     color-wheel) keep their values — the lights are dark, no need to
///     slam heads. Precedence: blackout kills UV too (handled by the frame
///     early-return); a blinder hold wins while held (UV resumes after).
///   * Global Brightness — uniform multiplier on every emitting channel for
///     non-laser fixtures so the operator slider acts as a true master
///     across OSC, DMX, and the virtual-stage preview. **Lasers are
///     skipped** because slice 2d routes the BLACKOUT (LASER ON/OFF) gate
///     channel through the Dimmer slot — multiplying it would silently
///     toggle the laser off below the ~50% threshold.
///
/// `pub(crate)` so the virtual-stage backend can apply the same overrides
/// when computing its preview intents, keeping the preview honest with the
/// wire (no "operator slider only affects DMX, not what I see").
pub(crate) fn apply_global_overrides(
    intent: &mut FixtureIntentState,
    fixture: &FixturePatch,
    snapshot: &CueSnapshot,
    // The manual STROBE button software-flashes this fixture white. Passed
    // `false` for self-managed bars (the GigBAR drives its own strobe pods in
    // the hardware pass), `true` for ordinary flat/segment fixtures.
    software_strobe: bool,
) {
    let family = family_for(fixture.role, fixture.visual_kind);
    let is_laser = matches!(family, FixtureFamily::Laser);

    // One global-white decision, shared with the segment path (see
    // `white_override`). The strobe applies the SAME WAY to every fixture —
    // no per-brand/per-visual-kind cases; only the laser (no white) differs in
    // HOW the result lands.
    match white_override(snapshot, software_strobe) {
        WhiteOverride::Solid => {
            if is_laser {
                intent.dimmer = 0.0;
                intent.laser_macro = 0;
                intent.strobe = 0.0;
                intent.red = 0.0;
                intent.green = 0.0;
                intent.blue = 0.0;
                intent.white = 0.0;
            } else {
                intent.dimmer = 1.0;
                intent.red = 1.0;
                intent.green = 1.0;
                intent.blue = 1.0;
                intent.white = 1.0;
            }
            // White owns the moment — no UV under a blinder blast (mirrors the
            // GigBAR ch21/ch22 hardware exclusion).
            intent.uv = 0.0;
        }
        WhiteOverride::Strobe => {
            // White pulse on the shared wall-clock phase; peaks at gb×ceiling
            // via the master multiply below, and flashes even in silence (the
            // phase free-runs off the DMX clock). The software pulse OWNS the
            // strobe (shutter zeroed) so a hardware-shutter fixture flashes in
            // unison instead of double-strobing. Lasers (no white) only gate
            // dark on the off phase.
            if is_laser {
                if !snapshot.strobe_on {
                    intent.dimmer = 0.0;
                    intent.laser_macro = 0;
                }
            } else {
                let v = if snapshot.strobe_on { 1.0 } else { 0.0 };
                intent.dimmer = v;
                intent.red = v;
                intent.green = v;
                intent.blue = v;
                intent.white = v;
                intent.strobe = 0.0;
            }
            intent.uv = 0.0;
        }
        WhiteOverride::None => {
            if snapshot.trigger_uv && !snapshot.trigger_blackout {
                // UV MODE: zero every emitting channel (the blackout set), then
                // light the UV channel where one exists. Auto strobe/programs
                // are suppressed with the rest of the emitters.
                intent.dimmer = 0.0;
                intent.red = 0.0;
                intent.green = 0.0;
                intent.blue = 0.0;
                intent.white = 0.0;
                intent.strobe = 0.0;
                intent.laser_macro = 0;
                intent.prism = 0.0;
                if fixture.channels.uv.is_some() {
                    // Scaled by Global Brightness below and the per-fixture trim
                    // in apply_per_fixture_overrides — uv = uv_level × GB × trim.
                    intent.uv = snapshot.uv_level.clamp(0.0, 1.0);
                }
            }
        }
    }
    // Global brightness is a wash/par/beam/gobo/strobe master; lasers carry
    // a binary enable on `dimmer`, so scaling them would just hide the
    // laser at low slider positions.
    if !is_laser {
        // The operator master AND the style brightness ceiling (Banger 1.0 /
        // Energetic 0.8 / Chill 0.5) both cap every emitter, so a calm style
        // reads dimmer and a banger gets the full top end. A manual override
        // (blinder/strobe/UV) bypasses the autonomous ceiling — it beats
        // whatever the posture is doing — but still respects the operator master.
        let ceiling = if snapshot.manual_light_override() {
            1.0
        } else {
            snapshot.style_brightness_ceiling
        };
        let gb = (snapshot.global_brightness * ceiling).clamp(0.0, 1.0);
        if gb < 0.999 {
            intent.dimmer *= gb;
            intent.red *= gb;
            intent.green *= gb;
            intent.blue *= gb;
            intent.white *= gb;
            intent.uv *= gb;
        }
    }
}

/// Apply the operator's per-fixture live mix (intensity trim + solo + mute) — the
/// final stage every intent passes through, AFTER [`apply_global_overrides`], so a
/// mute wins over a blinder flash and a per-fixture trim composes under the master.
///
/// Silencing rule: while ANY fixture is soloed (`any_solo`), only soloed fixtures
/// emit; otherwise a `mute`d fixture is silenced. Silence uses the canonical
/// laser-off idiom (`dimmer = 0` **and** `laser_macro = 0`) so a muted laser truly
/// goes dark. The intensity trim **skips lasers** for the same reason
/// [`apply_global_overrides`] does — their `dimmer` slot is a binary enable gate,
/// not a level, so scaling it would just toggle the laser off.
///
/// `pub(crate)` so the virtual-stage backend applies the identical override when
/// it computes preview intents, keeping the on-screen rig honest with the wire.
pub(crate) fn apply_per_fixture_overrides(
    intent: &mut FixtureIntentState,
    fixture: &FixturePatch,
    overrides: &FixtureOverrides,
    any_solo: bool,
) {
    let ov = overrides.get(&fixture.id);
    let silenced = if any_solo { !ov.solo } else { ov.mute };
    if silenced {
        intent.dimmer = 0.0;
        intent.red = 0.0;
        intent.green = 0.0;
        intent.blue = 0.0;
        intent.white = 0.0;
        intent.uv = 0.0;
        intent.strobe = 0.0;
        intent.laser_macro = 0;
        return;
    }
    let is_laser = matches!(family_for(fixture.role, fixture.visual_kind), FixtureFamily::Laser);
    if !is_laser && ov.intensity < 0.999 {
        let trim = ov.intensity.clamp(0.0, 1.0);
        intent.dimmer *= trim;
        intent.red *= trim;
        intent.green *= trim;
        intent.blue *= trim;
        intent.white *= trim;
        intent.uv *= trim;
    }
}

/// Write one fixture's intent to the wire. `gobo_exact` is the profile-step
/// chase's exact 0..1 gobo value from [`apply_cue_bindings`]: when present the
/// gobo channel emits `round(v·255)` instead of the `gobo_slot ×32` spread —
/// real wheels (chauvet obsession: 14 steps, 10-DMX-wide ranges) don't fit 8
/// quantized slots, and the ×32 grid can land a chase one wheel position off.
fn apply_fixture_intent(
    frame: &mut DmxFrame,
    fixture: &crate::emerge::contracts::FixturePatch,
    intent: FixtureIntentState,
    gobo_exact: Option<f32>,
) {
    frame.set_channel(fixture.channels.dimmer, to_dmx(intent.dimmer));
    frame.set_channel(fixture.channels.red, to_dmx(intent.red));
    frame.set_channel(fixture.channels.green, to_dmx(intent.green));
    frame.set_channel(fixture.channels.blue, to_dmx(intent.blue));
    frame.set_channel(fixture.channels.white, to_dmx(intent.white));
    frame.set_channel(fixture.channels.uv, to_dmx(intent.uv));
    frame.set_channel(fixture.channels.strobe, to_dmx(intent.strobe));
    frame.set_channel(fixture.channels.pan, to_dmx(intent.pan));
    frame.set_channel(fixture.channels.tilt, to_dmx(intent.tilt));
    frame.set_channel(fixture.channels.zoom, to_dmx(intent.zoom));
    frame.set_channel(fixture.channels.focus, to_dmx(intent.focus));
    frame.set_channel(fixture.channels.color_wheel, intent.color_wheel);
    let gobo_byte = match gobo_exact {
        Some(v) => to_dmx(v),
        None => intent.gobo_slot.saturating_mul(32),
    };
    frame.set_channel(fixture.channels.gobo_slot, gobo_byte);
    frame.set_channel(fixture.channels.gobo_rotation, to_dmx(intent.gobo_rotation));
    frame.set_channel(fixture.channels.prism, to_dmx(intent.prism));
    frame.set_channel(fixture.channels.laser_macro, intent.laser_macro.saturating_mul(16));
}

/// Pixel-bar fixture: each color cell gets its own RGBW from `segment_rgbw`,
/// while motion / shutter still come from the shared intent. The master dimmer
/// is held full because the per-segment color already carries the master
/// dimmer (segment_rgbw gates on it) — the fixture must not dim a second time.
fn apply_segmented_fixture(
    frame: &mut DmxFrame,
    fixture: &crate::emerge::contracts::FixturePatch,
    intent: FixtureIntentState,
    snapshot: &CueSnapshot,
    segments: &[crate::emerge::contracts::SegmentChannels],
    cue_segments: Option<&[[f32; 3]]>,
    beats: f64,
    ov: FixtureOverride,
    any_solo: bool,
) {
    use crate::emerge::cue::lighting_cue::{auto_segment_kind, eval_segment, operator_segment_kind, SegmentKind};

    // A launched segment cue OWNS the cells (operator Global Brightness + Blinder
    // still apply on top). With no segment cue, fall back to the show-brain
    // per-cell effect so the bar's default look is unchanged (no regression).
    // Master × style ceiling caps the Cue + strobe-white cells; a manual
    // override (blinder/strobe/UV) bypasses the autonomous ceiling (it beats the
    // posture) but still respects the operator master.
    let ceiling = if snapshot.manual_light_override() {
        1.0
    } else {
        snapshot.style_brightness_ceiling
    };
    let gb = (snapshot.global_brightness * ceiling).clamp(0.0, 1.0);
    // Same global-white decision as the flat path (see `white_override`). A
    // segmented fixture is never the self-managed GigBAR (that's a section
    // fixture), so it always allows the software strobe.
    let white = white_override(snapshot, true);
    // UV mode darkens the cells too (a pixel bar is not a UV emitter); any
    // white override (blinder/strobe) wins while held, same precedence as the
    // flat path. The master dimmer must drop with the cells — it's normally
    // pinned at 255 because the per-segment colour carries the dimming, which
    // would leave the bar lit if a cell wrote anything.
    let uv_mode = snapshot.trigger_uv && white == WhiteOverride::None;
    let count = segments.len();

    frame.set_channel(fixture.channels.dimmer, if uv_mode { 0 } else { 255 });
    frame.set_channel(fixture.channels.strobe, to_dmx(intent.strobe));
    // The odd segmented-fixture-with-a-UV-channel still lights it in UV mode
    // (intent.uv is only ever non-zero there — see apply_global_overrides).
    frame.set_channel(fixture.channels.uv, to_dmx(intent.uv));
    frame.set_channel(fixture.channels.pan, to_dmx(intent.pan));
    frame.set_channel(fixture.channels.tilt, to_dmx(intent.tilt));
    frame.set_channel(fixture.channels.zoom, to_dmx(intent.zoom));
    frame.set_channel(fixture.channels.focus, to_dmx(intent.focus));

    // Pick the per-cell colour source ONCE per fixture per frame; cells are then
    // evaluated against it. The previous shape re-decided (and rebuilt the
    // `SegmentKind`, allocating its palette strings) per CELL — ~24 allocations
    // per bar per frame on the ~30Hz render path.
    enum CellSrc<'a> {
        /// Solid full white (manual BLINDER hold).
        Blinder,
        /// Strobe: every cell pulses full white on the shared wall-clock phase
        /// (a pixel bar has no hardware shutter, so it's rendered in software
        /// across the RGB cells). Manual STROBE or the autonomous buildup/Pump.
        StrobeWhite,
        /// A launched segment cue owns the cells (scaled by Global Brightness).
        Cue(&'a [[f32; 3]]),
        /// Operator Segment FX picker / tension autopilot: one `SegmentKind`
        /// evaluated per cell, scaled by `dim`.
        Kind { kind: SegmentKind, dim: f32 },
        /// Show-brain gradient/chase fallback (~1 in 5 looks).
        Brain,
    }
    // The shared global-white override wins over the cue/autopilot look, the
    // same as the flat path — no separate blinder/strobe logic here (that's
    // how the two used to drift). Decided once per fixture, not per cell.
    let src = match white {
        WhiteOverride::Solid => CellSrc::Blinder,
        WhiteOverride::Strobe => CellSrc::StrobeWhite,
        WhiteOverride::None => {
        match cue_segments {
        Some(cells) => CellSrc::Cue(cells),
        None if ov.segment_fx != SegmentPattern::Auto => {
            // Operator-driven segment look (panel's Segment FX picker): lock the
            // cells to the chosen pattern instead of the tension autopilot.
            // Brightness-hold floor: the pattern stays visible against the show's
            // dimming (lifts up to SEG_FX_FLOOR), but mute/solo still silence it
            // (gated) and the per-fixture intensity still scales it.
            let silenced = if any_solo { !ov.solo } else { ov.mute };
            let dim = if silenced {
                0.0
            } else {
                intent.dimmer.max(SEG_FX_FLOOR * ov.intensity)
            };
            CellSrc::Kind {
                kind: operator_segment_kind(ov.segment_fx, ov.segment_speed, snapshot.look_index),
                dim,
            }
        }
        None => {
            // COM-499 autonomous segment look: ~1 in 5 looks keep the
            // show-brain gradient/chase; the rest run the slice-3 tension
            // engine — the segment effect escalates with tension_bank and
            // releases on the drop. Both dim with the fixture (intent.dimmer).
            if snapshot.look_index % 5 == 0 {
                CellSrc::Brain
            } else {
                CellSrc::Kind {
                    kind: auto_segment_kind(
                        snapshot.look_index,
                        snapshot.tension_bank,
                        snapshot.drop_impact,
                        snapshot.auto_chase_cycle,
                    ),
                    dim: intent.dimmer,
                }
            }
        }
        }
        }
    };

    for (i, seg) in segments.iter().enumerate() {
        let (r, g, b, w) = if uv_mode {
            // UV mode: every cell dark — only UV channels emit rig-wide.
            (0.0, 0.0, 0.0, 0.0)
        } else {
            match &src {
                CellSrc::Blinder => (1.0, 1.0, 1.0, 1.0),
                CellSrc::StrobeWhite => {
                    // Shutterless pixel bar: flash every cell white on the
                    // shared wall-clock ON phase, peaking at Global Brightness.
                    if snapshot.strobe_on {
                        (gb, gb, gb, gb)
                    } else {
                        (0.0, 0.0, 0.0, 0.0)
                    }
                }
                CellSrc::Cue(cells) => {
                    let c = cells.get(i).copied().unwrap_or([0.0, 0.0, 0.0]);
                    (c[0] * gb, c[1] * gb, c[2] * gb, 0.0)
                }
                CellSrc::Kind { kind, dim } => {
                    let c = eval_segment(kind, i, count, beats);
                    (c[0] * dim, c[1] * dim, c[2] * dim, 0.0)
                }
                CellSrc::Brain => segment_rgbw(snapshot, intent.dimmer, i, count),
            }
        };
        frame.set_channel(seg.red, to_dmx(r));
        frame.set_channel(seg.green, to_dmx(g));
        frame.set_channel(seg.blue, to_dmx(b));
        frame.set_channel(seg.white, to_dmx(w));
    }
}

fn to_dmx(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

// ───────────────────────────────────────────────────────────────────────────
// Multi-section fixture expansion (GigBAR Move and future all-in-one bars).
//
// One stage fixture / one combined NAS profile, rendered as N physical sections.
// The section MAP lives here in code (keyed off the resolved profile's
// manufacturer+model) so the GOLDEN profile JSON stays untouched and the layout
// is versioned + unit-tested. Each section reuses the normal per-fixture intent
// path via a synthetic FixturePatch; section channel addresses are absolute
// (`base + offset - 1`), and each channel's static default is read from the
// profile so combo channels (e.g. the par dim/strobe slot at a static 127) light
// the section without bespoke values here.
// ───────────────────────────────────────────────────────────────────────────

/// One rendered section of a multi-section bar.
pub(crate) struct SectionRender {
    /// Synthetic fixture for this section. `id` is the PARENT fixture id so
    /// operator overrides (mute/solo/intensity) resolve rig-wide; `position` is
    /// offset along the bar for the operator preview (DMX ignores position).
    pub(crate) fixture: FixturePatch,
    /// `(absolute DMX channel, value)` defaults stamped before the live intent —
    /// pulled from the profile's per-channel `default_value`.
    default_stamps: Vec<(u16, u8)>,
    /// Pars carry no dimmer channel; fold the wash dimmer into RGB instead.
    pub(crate) fold_dimmer_into_rgb: bool,
    /// Mover head index + total head count, for counter-phase pan/tilt.
    pub(crate) mover_ordinal: usize,
    pub(crate) mover_count: usize,
}

/// Declarative template for one section: where it sits in the fixture's channel
/// block, what family drives it, and which intent slots map to which channel
/// offsets (1-based, matching the profile's `channel_offset`).
struct SectionSpec {
    name: &'static str,
    role: FixtureRole,
    visual_kind: StageVisualKind,
    first_offset: u16,
    len: u16,
    /// ChannelMap whose Some(_) values are OFFSETS within the fixture (1-based),
    /// converted to absolute addresses at expansion time.
    channels: ChannelMap,
    fold_dimmer_into_rgb: bool,
    /// `Some((ordinal, count))` if this section is a moving head.
    mover: Option<(usize, usize)>,
}

/// Energy below this leaves the bar dark; above [`BAR_GATE_HI`] it's full; smooth
/// between. Tuned so a quiet/ambient passage collapses the dense bar while a solid
/// groove drives it fully. These are the live-tuning knobs for "too hot / too cold".
const BAR_GATE_LO: f32 = 0.08;
const BAR_GATE_HI: f32 = 0.55;

/// Width (stage units) the bar's sections are spread across for the operator
/// preview. Preview-only geometry — DMX ignores fixture position.
const GIGBAR_PREVIEW_SPREAD: f32 = 1.4;

/// Smooth audio-energy gate for the whole GigBAR: a Hermite ramp from dark→full
/// across `[BAR_GATE_LO, BAR_GATE_HI]`. Energy releases far faster than the
/// `excitement`-driven `bed_floor` (~3s), so gating on it both tames the bar's
/// brightness and makes it taper with the music instead of lingering.
pub(crate) fn bar_energy_gate(energy: f32) -> f32 {
    let t = ((energy - BAR_GATE_LO) / (BAR_GATE_HI - BAR_GATE_LO).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Expand a multi-section fixture into its per-section render units, or `None`
/// for ordinary single-section fixtures (which take the flat path unchanged).
pub(crate) fn expand_sections(
    fixture: &FixturePatch,
    profile: Option<&FixtureProfile>,
) -> Option<Vec<SectionRender>> {
    let profile = profile?;
    let specs = gigbar_sections(profile)?;
    let base = fixture.start_address;
    let n = specs.len();

    let sections = specs
        .into_iter()
        .enumerate()
        .map(|(idx, spec)| {
            let mut channels = absolutize_offsets(&spec.channels, base);
            // UV channels are deliberately absent from the static SectionSpec
            // maps (the 35ch hardware pass owns the GigBAR's UV bytes), so
            // resolve them from the PROFILE instead: the first Uv-attributed
            // channel inside this section's block. This is what makes the
            // sections UV-mode-addressable (and the preview honest) without
            // touching the spec tables — on 35ch the hardware pass overwrites
            // the same bytes after the section loop, identically.
            channels.uv = profile
                .channels
                .iter()
                .find(|c| {
                    c.attribute == crate::emerge::contracts::DmxAttribute::Uv
                        && (spec.first_offset..spec.first_offset.saturating_add(spec.len))
                            .contains(&c.channel_offset)
                })
                .map(|c| base.saturating_add(c.channel_offset).saturating_sub(1));
            // Lay the sections out along the bar's length for the operator preview,
            // rotated by the bar's pan so the row follows where the bar faces. World
            // up is +Z and pan (`rotation.y`) rotates the horizontal X-Y plane around
            // it (same convention as the renderer's `yaw_offset`): a local-X offset
            // `d` maps to world (d·cosθ, d·sinθ, 0). Preview-only — DMX ignores
            // position and render_fixture_intent keys off start_address, so the wire
            // is untouched.
            let d = if n > 1 {
                (idx as f32 / (n as f32 - 1.0) - 0.5) * GIGBAR_PREVIEW_SPREAD
            } else {
                0.0
            };
            let (sin_pan, cos_pan) = fixture.rotation.y.to_radians().sin_cos();
            let position = StageVec3 {
                x: fixture.position.x + d * cos_pan,
                y: fixture.position.y + d * sin_pan,
                z: fixture.position.z,
            };
            let section_fixture = FixturePatch {
                screen: None,
                id: fixture.id.clone(),
                name: format!("{} — {}", fixture.name, spec.name),
                universe: fixture.universe,
                start_address: base.saturating_add(spec.first_offset).saturating_sub(1),
                profile_id: fixture.profile_id.clone(),
                mode: fixture.mode.clone(),
                position,
                rotation: fixture.rotation,
                scale: fixture.scale,
                role: spec.role,
                visual_kind: spec.visual_kind,
                tags: vec![],
                channels,
                // Synthetic render-time section — never passes through the
                // address allocator, so no footprint reservation needed.
                footprint: 0,
            };
            // Static defaults for every channel in this section's block.
            let mut default_stamps = Vec::with_capacity(spec.len as usize);
            for off in spec.first_offset..spec.first_offset.saturating_add(spec.len) {
                if let Some(cd) = profile.channels.iter().find(|c| c.channel_offset == off) {
                    let abs = base.saturating_add(off).saturating_sub(1);
                    default_stamps.push((abs, cd.default_value));
                }
            }
            let (mover_ordinal, mover_count) = spec.mover.unwrap_or((0, 1));
            SectionRender {
                fixture: section_fixture,
                default_stamps,
                fold_dimmer_into_rgb: spec.fold_dimmer_into_rgb,
                mover_ordinal,
                mover_count,
            }
        })
        .collect();
    Some(sections)
}

/// Is this resolved profile the Chauvet GigBAR Move (any personality)?
fn is_gigbar(profile: &FixtureProfile) -> bool {
    profile.manufacturer.eq_ignore_ascii_case("Chauvet DJ")
        && profile.model.to_ascii_lowercase().contains("gigbar move")
}

/// The GigBAR Move patched in the 35-channel personality — the only mode with
/// per-section UV / strobe-engine / par-combo channels, so the only mode the
/// [`apply_gigbar_hardware_pass`] shaping applies to (17ch is untouched).
/// `pub(crate)` so the virtual-stage preview resolves the SAME self-managed
/// predicate when deciding software-strobe, keeping preview and wire in step.
pub(crate) fn is_gigbar_35(profile: &FixtureProfile) -> bool {
    is_gigbar(profile) && profile.channel_count == 35
}

/// Section template for the Chauvet GigBAR Move, chosen by the patched DMX mode
/// (35- or 17-channel). Returns `None` for any other fixture.
fn gigbar_sections(profile: &FixtureProfile) -> Option<Vec<SectionSpec>> {
    if !is_gigbar(profile) {
        return None;
    }
    match profile.channel_count {
        35 => Some(gigbar_sections_35()),
        17 => Some(gigbar_sections_17()),
        _ => None,
    }
}

/// 35-channel personality: full per-section control.
/// Par1 1-5, Par2 6-10, Derby1 11-13, Derby2 14-16, Laser 17-19, Strobe 20-23,
/// MH1 24-29, MH2 30-35 (= 35).
fn gigbar_sections_35() -> Vec<SectionSpec> {
    vec![
        SectionSpec {
            name: "Par 1",
            role: FixtureRole::Wash,
            visual_kind: StageVisualKind::Wash,
            first_offset: 1,
            len: 5,
            channels: ChannelMap { red: Some(1), green: Some(2), blue: Some(3), ..Default::default() },
            fold_dimmer_into_rgb: true,
            mover: None,
        },
        SectionSpec {
            name: "Par 2",
            role: FixtureRole::Wash,
            visual_kind: StageVisualKind::Wash,
            first_offset: 6,
            len: 5,
            channels: ChannelMap { red: Some(6), green: Some(7), blue: Some(8), ..Default::default() },
            fold_dimmer_into_rgb: true,
            mover: None,
        },
        SectionSpec {
            name: "Derby 1",
            role: FixtureRole::Accent,
            visual_kind: StageVisualKind::Gobo,
            first_offset: 11,
            len: 3,
            channels: ChannelMap { color_wheel: Some(11), strobe: Some(12), gobo_rotation: Some(13), ..Default::default() },
            fold_dimmer_into_rgb: false,
            mover: None,
        },
        SectionSpec {
            name: "Derby 2",
            role: FixtureRole::Accent,
            visual_kind: StageVisualKind::Gobo,
            first_offset: 14,
            len: 3,
            channels: ChannelMap { color_wheel: Some(14), strobe: Some(15), gobo_rotation: Some(16), ..Default::default() },
            fold_dimmer_into_rgb: false,
            mover: None,
        },
        SectionSpec {
            name: "Laser",
            role: FixtureRole::Accent,
            visual_kind: StageVisualKind::Laser,
            first_offset: 17,
            len: 3,
            channels: ChannelMap { laser_macro: Some(17), strobe: Some(18), gobo_rotation: Some(19), ..Default::default() },
            fold_dimmer_into_rgb: false,
            mover: None,
        },
        SectionSpec {
            name: "Strobe",
            role: FixtureRole::Strobe,
            visual_kind: StageVisualKind::Strobe,
            first_offset: 20,
            len: 4,
            // White-LED dimmer (ch21) carries the strobe section's level; the
            // auto-strobe program channel (ch20) stays at its static default.
            channels: ChannelMap { dimmer: Some(21), ..Default::default() },
            fold_dimmer_into_rgb: false,
            mover: None,
        },
        SectionSpec {
            name: "Moving Head 1",
            role: FixtureRole::Accent,
            visual_kind: StageVisualKind::Beam,
            first_offset: 24,
            len: 6,
            channels: ChannelMap {
                pan: Some(24), tilt: Some(25), dimmer: Some(26), strobe: Some(27),
                color_wheel: Some(28), gobo_slot: Some(29), ..Default::default()
            },
            fold_dimmer_into_rgb: false,
            mover: Some((0, 2)),
        },
        SectionSpec {
            name: "Moving Head 2",
            role: FixtureRole::Accent,
            visual_kind: StageVisualKind::Beam,
            first_offset: 30,
            len: 6,
            channels: ChannelMap {
                pan: Some(30), tilt: Some(31), dimmer: Some(32), strobe: Some(33),
                color_wheel: Some(34), gobo_slot: Some(35), ..Default::default()
            },
            fold_dimmer_into_rgb: false,
            mover: Some((1, 2)),
        },
    ]
}

/// 17-channel personality: pars+derby share one color block, one control drives
/// both heads. Pars+Derby 1-6, Laser 7-9, Strobe 10-11, Heads 12-17 (= 17).
fn gigbar_sections_17() -> Vec<SectionSpec> {
    vec![
        SectionSpec {
            name: "Pars + Derby",
            role: FixtureRole::Wash,
            visual_kind: StageVisualKind::Wash,
            first_offset: 1,
            len: 6,
            channels: ChannelMap { red: Some(1), green: Some(2), blue: Some(3), ..Default::default() },
            fold_dimmer_into_rgb: true,
            mover: None,
        },
        SectionSpec {
            name: "Laser",
            role: FixtureRole::Accent,
            visual_kind: StageVisualKind::Laser,
            first_offset: 7,
            len: 3,
            // Red-laser channel carries the macro; green (ch8) holds its default.
            channels: ChannelMap { laser_macro: Some(7), gobo_rotation: Some(9), ..Default::default() },
            fold_dimmer_into_rgb: false,
            mover: None,
        },
        SectionSpec {
            name: "Strobe",
            role: FixtureRole::Strobe,
            visual_kind: StageVisualKind::Strobe,
            first_offset: 10,
            len: 2,
            channels: ChannelMap { dimmer: Some(10), ..Default::default() },
            fold_dimmer_into_rgb: false,
            mover: None,
        },
        SectionSpec {
            name: "Moving Heads",
            role: FixtureRole::Accent,
            visual_kind: StageVisualKind::Beam,
            first_offset: 12,
            len: 6,
            channels: ChannelMap {
                pan: Some(12), tilt: Some(13), dimmer: Some(14), strobe: Some(15),
                color_wheel: Some(16), gobo_slot: Some(17), ..Default::default()
            },
            fold_dimmer_into_rgb: false,
            mover: Some((0, 1)),
        },
    ]
}

/// Convert a ChannelMap whose slots hold 1-based OFFSETS into one holding
/// absolute DMX addresses anchored at `base` (`abs = base + offset - 1`).
fn absolutize_offsets(map: &ChannelMap, base: u16) -> ChannelMap {
    let a = |o: Option<u16>| o.map(|off| base.saturating_add(off).saturating_sub(1));
    ChannelMap {
        dimmer: a(map.dimmer),
        red: a(map.red),
        green: a(map.green),
        blue: a(map.blue),
        white: a(map.white),
        uv: a(map.uv),
        strobe: a(map.strobe),
        color_wheel: a(map.color_wheel),
        pan: a(map.pan),
        tilt: a(map.tilt),
        zoom: a(map.zoom),
        focus: a(map.focus),
        gobo_slot: a(map.gobo_slot),
        gobo_rotation: a(map.gobo_rotation),
        prism: a(map.prism),
        laser_macro: a(map.laser_macro),
    }
}

// ───────────────────────────────────────────────────────────────────────────
// GigBAR Move 35ch HARDWARE pass (GigBAR_MOVE_UM_Rev4_ML5, "35 Channel
// (Strobe = Effect)" chart).
//
// The generic per-family intent path drives levels (RGB, dimmers, pan/tilt);
// this pass drives the bytes that are MODE SELECTORS on the real unit — the
// strobe-pod program engine, the par dim/strobe combo, the laser colour-wheel
// strobe textures, and the UV layer with its hardware mutual-exclusion rule.
// REACT is the brain: the unit's sound-active bands (ch20 210-255, ch5/10
// 240-249) are NEVER written — we always run the engine ourselves.
// ───────────────────────────────────────────────────────────────────────────

// 1-based channel offsets within the fixture's 35ch block.
const GIG35_PAR1_UV: u16 = 4; // Par 1 UV 0-100%
const GIG35_PAR1_RATE: u16 = 5; // Par 1 dim/strobe combo (see bands below)
const GIG35_PAR2_UV: u16 = 9; // Par 2 UV 0-100%
const GIG35_PAR2_RATE: u16 = 10; // Par 2 dim/strobe combo
const GIG35_DERBY1_COLOR: u16 = 11; // Derby 1 colour selector (0 = off)
const GIG35_DERBY2_COLOR: u16 = 14; // Derby 2 colour selector (0 = off)
const GIG35_LASER_CONTROL: u16 = 17; // Laser colour/strobe-texture wheel
const GIG35_STROBE_CONTROL: u16 = 20; // Strobe program engine (0-9 = parked)
// ch21 (white strobe dimmer, manual + engine-independent) is NOT listed here:
// it is the strobe section's mapped `dimmer` channel, written from the section
// intent by `apply_fixture_intent` — this pass only reads it for exclusion.
const GIG35_STROBE_UV: u16 = 22; // UV strobe dimmer 0-255 — NEVER together with ch21
const GIG35_STROBE_SPEED: u16 = 23; // Strobe speed slow→fast for the ch20 programs

/// ch20 190-209 = all four pods strobing — the hard-drop look (band center).
const GIG35_STROBE_ALL_PODS: u8 = 199;
/// ch20 auto-program band geometry: programs 1-9 occupy 20-value bands from 10
/// (10-29 prog 1 … 170-189 prog 9); we write each band's CENTER, 20 + 20·k.
const GIG35_PROGRAM_CENTER: u8 = 20;
const GIG35_PROGRAM_BAND: u8 = 20;
/// ch17 240-255 = red + green ALTERNATE strobing — the drop laser texture.
const GIG35_LASER_ALTERNATE: u8 = 247;
/// ch5/10 250-255 = full-power RGB on — the pars join a white blast.
const GIG35_PAR_FULL_PUNCH: u8 = 252;
/// ch5/10 128-239 = par strobe slow→fast. Base + a 111-wide energy ride keeps
/// the byte capped at 239, UNDER the 240-249 strobe-to-sound band.
const GIG35_PAR_STROBE_BASE: u8 = 128;
const GIG35_PAR_STROBE_SPAN: f32 = 111.0;
/// `drop_impact` at/above this = the big moment: all pods strobe (ch20=199) and
/// the laser flips to alternate red/green strobing (ch17=247).
const GIG35_DROP_IMPACT_HARD: f32 = 0.6;
/// ch23 speed floor — even a low-energy strobe event runs at 25% engine speed.
const GIG35_SPEED_FLOOR: f32 = 0.25;

/// Per-frame live state the hardware pass needs from the section loop: the
/// strobe pods' FINAL rendered intent (cue bindings, blinder, mute/solo, the
/// show-brain lane gate and the bar energy gate all baked in) and whether the
/// laser lane is lit (`laser_macro > 0` — the gate/mute/blinder all zero it).
#[derive(Default)]
struct GigbarLive {
    strobe_intent: Option<FixtureIntentState>,
    laser_lit: bool,
}

/// GigBAR Move 35ch hardware shaping — runs once per frame after the section
/// loop, writing absolute channels anchored at the fixture's `base` address.
///
/// What it encodes (manual-verified, GigBAR_MOVE_UM_Rev4_ML5):
///   * STROBE PODS — ch20 actually runs the strobe engine. A blinder hold keeps
///     ch20=0: the ch21 manual white dimmer is independent of the program
///     engine (hardware-verified — white blasts work with ch20=0), so the hold
///     stays a constant blast, no program flicker. A strobe-lane event picks
///     "all four pods" (199) on a hard drop, else rotates auto programs 1-9 by
///     `look_index`; ch23 speed rides energy; ch21 keeps the pods' brightness
///     from the section intent so the program has something to flash. Idle
///     parks ch20 at 0 — written explicitly so no profile default can ever
///     leave the engine in the 210-255 sound-active band.
///   * PAR PUNCH — ch5/ch10 leave the 127 RGB pass-through only for an event:
///     a rig strobe moment strobes the pars (128-239, speed riding energy,
///     capped under the 240-249 sound band); a blinder hold punches full-power
///     RGB (252) so the pars join the white blast.
///   * LASER TEXTURE — a hard drop while the laser lane is lit flips ch17 to
///     red+green alternate strobing (247); otherwise the existing wheel byte
///     (`laser_macro × 16`) stands. A gated/muted/blinder-zeroed laser stays
///     dark — `laser_lit` is false whenever the lane was ruled dark.
///   * UV ACCENT — drive = `uv_level × (0.3·abs-tension + 0.7·tension_dyn)`, building
///     with the music's tension into a drop. As an EMITTING level it rides the
///     same silence discipline as the rest of the bar: the bar energy gate,
///     the global-brightness master (which carries the app SilenceGate), and
///     the operator's per-fixture trim. It zeroes whenever white owns the
///     moment (blinder hold / strobe programs). Par UV lands on ch4/ch9; pod
///     UV on ch22 ONLY while ch21 is 0 this frame (manual: ch21 & ch22 cannot
///     be used at the same time — white wins).
///   * UV MODE (`trigger_uv`) — the manual "blackout, except blacklight"
///     trigger. The section loop already darkened every emitter (UV mode in
///     `apply_global_overrides` zeroes RGB/dimmer/strobe/laser), so here:
///     ch4/ch9/ch22 = `uv_level × GB × trim` (NO tension ramp, NO bar energy
///     gate — a manual trigger must light the room even in silence), ch5/ch10
///     stay at the 127 RGB pass-through, ch20 parks, and the derby colour
///     selectors (ch11/ch14 — emitting selectors with no dimmer of their own)
///     are forced 0. ch21 is already 0 from the darkened section intent, so
///     the ch21/ch22 exclusion holds trivially. A blinder hold beats UV mode
///     (white blast; UV resumes on release); blackout never reaches this pass
///     (the frame early-returns all-zero).
///
/// Gating discipline: ch21/ch22/ch4/ch9 are emitting LEVELS and ride the bar
/// energy gate (ch21 via the section intent, the UV bytes explicitly here —
/// except in UV mode, where the operator's trigger IS the gate);
/// ch20/ch23 and ch5/ch10 are CONTROL channels — event-selected, never
/// energy-scaled. `silenced` (mute, or un-soloed while a solo is live)
/// suppresses every event byte so a muted bar cannot strobe, RGB-punch, or
/// UV-glow off snapshot state alone.
fn apply_gigbar_hardware_pass(
    frame: &mut DmxFrame,
    base: u16,
    snapshot: &CueSnapshot,
    bar_gain: f32,
    live: &GigbarLive,
    silenced: bool,
    intensity_trim: f32,
) {
    let abs = |off: u16| Some(base.saturating_add(off).saturating_sub(1));

    let blinder = snapshot.trigger_blinder || snapshot.auto_blinder_active;
    // UV MODE: blinder beats it while held; mute/solo silence wins over both.
    let uv_mode = snapshot.trigger_uv && !blinder && !silenced;
    let strobe_intent = live.strobe_intent.unwrap_or_default();
    // The strobe lane fired this frame: the section's rendered strobe level IS
    // the event signal — genre/cooldown gating (show brain), mute/solo, and the
    // bar energy gate have all already zeroed it when the lane is dark (UV mode
    // zeroes it too — no programs under blacklight). The blinder owns its
    // moment: constant white, never a program.
    let strobe_firing = !blinder && !silenced && !uv_mode && strobe_intent.strobe > 0.0;

    // ── Strobe pods: ch20 program engine + ch23 engine speed ──
    if strobe_firing {
        let program = if snapshot.drop_impact >= GIG35_DROP_IMPACT_HARD {
            GIG35_STROBE_ALL_PODS
        } else {
            GIG35_PROGRAM_CENTER + (snapshot.look_index % 9) as u8 * GIG35_PROGRAM_BAND
        };
        frame.set_channel(abs(GIG35_STROBE_CONTROL), program);
        let speed =
            (GIG35_SPEED_FLOOR + (1.0 - GIG35_SPEED_FLOOR) * snapshot.energy).clamp(0.0, 1.0);
        frame.set_channel(abs(GIG35_STROBE_SPEED), to_dmx(speed));
    } else {
        // Blinder hold + idle both park the engine. 0-9 blacks out the PROGRAM
        // engine only — the ch21 manual dimmer (already written from the
        // section intent by apply_fixture_intent) blasts independently.
        frame.set_channel(abs(GIG35_STROBE_CONTROL), 0);
    }

    // ── Pars: ch5/ch10 dim/strobe combo ──
    // Mirrors intent.rs `strobe_hard_trigger`: the operator STROBE button, the
    // cue engine's genre-gated strobe state, or the overdrive macro phase.
    let rig_strobe = snapshot.overdrive_strobe_active
        || snapshot.strobe_active
        || snapshot.trigger_strobe;
    let par_rate = if silenced {
        None
    } else if blinder {
        Some(GIG35_PAR_FULL_PUNCH)
    } else if uv_mode {
        // UV mode: pin the explicit 127 RGB pass-through (the pars' RGB is
        // already zeroed, so they emit UV only) — never a strobe band.
        Some(127)
    } else if rig_strobe {
        let speed01 = snapshot.energy.clamp(0.0, 1.0);
        Some(GIG35_PAR_STROBE_BASE + (speed01 * GIG35_PAR_STROBE_SPAN).round() as u8)
    } else {
        None // idle: the profile's stamped 127 pass-through — exactly today's bytes
    };
    if let Some(rate) = par_rate {
        frame.set_channel(abs(GIG35_PAR1_RATE), rate);
        frame.set_channel(abs(GIG35_PAR2_RATE), rate);
    }

    // ── Laser: alternate-strobe texture on the hard drop ──
    if live.laser_lit && !silenced && snapshot.drop_impact >= GIG35_DROP_IMPACT_HARD {
        frame.set_channel(abs(GIG35_LASER_CONTROL), GIG35_LASER_ALTERNATE);
    }

    // ── UV mode: force the derby colour selectors dark ──
    // The derbies have no dimmer; their colour selector (ch11/ch14) IS the
    // emitter, and as a position-style byte it survives the section loop's
    // emitter-zeroing. "Only UV-capable emitters lit" must darken them here.
    if uv_mode {
        frame.set_channel(abs(GIG35_DERBY1_COLOR), 0);
        frame.set_channel(abs(GIG35_DERBY2_COLOR), 0);
    }

    // ── UV accent layer ──
    // Mostly ADAPTIVE tension (dynamics above the track's own baseline) with
    // a small absolute floor: wall-of-sound material rails the absolute
    // tension at ~1.0 for whole songs, which pegged the UV instead of letting
    // it breathe; pure dynamics alone would go fully dark on a long sustained
    // plateau where a dim violet bed reads right.
    let tension_abs = snapshot.tension_bank.max(snapshot.tension).clamp(0.0, 1.0);
    let tension_ramp = (0.3 * tension_abs + 0.7 * snapshot.tension_dyn.clamp(0.0, 1.0))
        .clamp(0.0, 1.0);
    let white_owns = blinder || strobe_firing;
    let mut uv = if uv_mode {
        // Manual UV MODE: the operator owns the level — no tension ramp, no
        // bar energy gate (the trigger must light the room even in silence).
        snapshot.uv_level.clamp(0.0, 1.0)
    } else {
        snapshot.uv_level.clamp(0.0, 1.0) * tension_ramp * bar_gain
    };
    uv *= snapshot.global_brightness.clamp(0.0, 1.0) * intensity_trim.clamp(0.0, 1.0);
    if white_owns || silenced {
        uv = 0.0;
    }
    let uv_byte = to_dmx(uv);
    frame.set_channel(abs(GIG35_PAR1_UV), uv_byte);
    frame.set_channel(abs(GIG35_PAR2_UV), uv_byte);
    // Manual: ch21 & ch22 CANNOT be used at the same time — white wins. Match
    // the exact ch21 byte apply_fixture_intent wrote from the section intent.
    let white_byte = to_dmx(strobe_intent.dimmer);
    frame.set_channel(abs(GIG35_STROBE_UV), if white_byte == 0 { uv_byte } else { 0 });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emerge::contracts::{
        ChannelMap, FixtureOverride, FixtureOverrides, FixturePatch, FixtureRole, StageVisualKind,
        StageVec3,
    };
    use crate::hue::CueSnapshot;

    #[test]
    fn blackout_hard_zeros_all_channels() {
        let patch = DmxPatch::demo_rgbw_wash(0, 1);
        let snapshot = CueSnapshot {
            energy: 1.0,
            trigger_blackout: true,
            ..Default::default()
        };
        let frame = render_dmx_frame(&snapshot, &patch, &ProfileMap::new(), None);
        assert!(frame.blackout);
        assert!(frame.data.iter().all(|v| *v == 0));
    }

    #[test]
    fn energy_drives_dimmer_and_rgb_channels() {
        let patch = DmxPatch::demo_rgbw_wash(2, 10);
        let snapshot = CueSnapshot {
            energy: 0.8,
            reactivity: 1.0,
            dominant_pc: 4,
            ..Default::default()
        };
        let frame = render_dmx_frame(&snapshot, &patch, &ProfileMap::new(), None);
        assert_eq!(frame.universe, 2);
        assert!(!frame.blackout);
        assert!(frame.data[9] > 0); // dimmer at channel 10
        assert!(frame.data[10] > 0 || frame.data[11] > 0 || frame.data[12] > 0);
    }

    #[test]
    fn cue_overlay_drives_dmx_channel() {
        use crate::emerge::{ActiveCue, CueListResponse};
        // RGBW wash: dimmer at start_address 1 -> data[0].
        let patch = DmxPatch::demo_rgbw_wash(0, 1);
        let json = r#"{"cues":[{"launch_id":"L","status":"ACTIVE","variables":{},
            "launch_contract":{"bindings":[
                {"id":"d","target":{"channel_name":"Dimming","type":"DIMMER"},
                 "source":{"kind":"constant","value":0.5},
                 "resolve":{"mode":"override","priority":50}}]}}]}"#;
        let list: CueListResponse = serde_json::from_str(json).unwrap();
        let cues: Vec<ActiveCue> = list.cues.iter().map(ActiveCue::from_dto).collect();
        let mut reports = std::collections::HashMap::new();
        let snapshot = CueSnapshot { energy: 1.0, reactivity: 1.0, ..Default::default() };
        let frame = render_dmx_frame_with_cues(
            &snapshot,
            &patch,
            &ProfileMap::new(),
            None,
            &cues,
            &mut reports,
            &FixtureOverrides::default(),
        );
        // constant override DIMMER=0.5 -> round(0.5*255)=128 on the dimmer channel.
        assert_eq!(frame.data[0], 128, "cue override should drive the dimmer byte");
    }

    #[test]
    fn profile_gobo_step_chase_emits_exact_wheel_byte() {
        // Fix #2 end-to-end: a values_from gobo chase over byte-range profile
        // steps must land the gobo channel INSIDE the step's exact DMX range,
        // not on the nearest ×32 slot byte. Step (40, 49) -> midpoint 44.5 ->
        // normalized 44.5/255 -> emitted round(0.17451*255) = 45. The legacy
        // 8-slot path would have written round(0.17451*7)=1 -> 1×32 = 32,
        // which is OUTSIDE the 40..49 wheel range (one position off).
        use crate::emerge::contracts::{
            ChannelDefinition, ChannelDetail, ChannelSlot, DmxAttribute, FixtureCapability,
            FixtureKind, FixtureProfile,
        };
        use crate::emerge::{ActiveCue, CueListResponse};
        let profile = FixtureProfile {
            id: "obsession-like".into(),
            manufacturer: "Test".into(),
            model: "Gobo Wheel".into(),
            mode: "8".into(),
            kind: FixtureKind::Beam,
            housing: crate::emerge::FixtureHousing::Generic,
            channel_count: 1,
            channels: vec![ChannelDefinition {
                attribute: DmxAttribute::GoboSlot,
                channel_offset: 1,
                default_value: 0,
                blackout_value: 0,
                detail: Some(ChannelDetail::Gobo {
                    steps: vec![ChannelSlot {
                        name: "gobo 5".into(),
                        value: 0,
                        low_value: Some(40.0),
                        high_value: Some(49.0),
                        enabled: true,
                        rgb: None,
                    }],
                }),
                source_attribute: Some(serde_json::json!({"name": "GOBO WHEEL"})),
            }],
            capabilities: vec![FixtureCapability::Gobo],
        };
        let fixture = FixturePatch {
            footprint: 0,
            screen: None,
            id: "mh".into(),
            name: "mh".into(),
            universe: 0,
            start_address: 1,
            profile_id: "obsession-like".into(),
            mode: "8".into(),
            position: StageVec3::zero(),
            rotation: StageVec3::zero(),
            scale: StageVec3::one(),
            role: FixtureRole::Accent,
            visual_kind: StageVisualKind::Beam,
            tags: vec![],
            channels: ChannelMap {
                dimmer: None, red: None, green: None, blue: None, white: None,
                uv: None, strobe: None, color_wheel: None, pan: None, tilt: None,
                zoom: None, focus: None, gobo_slot: Some(1),
                gobo_rotation: None, prism: None, laser_macro: None,
            },
        };
        let patch = DmxPatch { universe: 0, trusses: vec![], venue: None, fixtures: vec![fixture] };
        let mut profiles = ProfileMap::new();
        profiles.insert("mh".into(), profile);
        let json = r#"{"cues":[{"launch_id":"G","status":"ACTIVE","variables":{},
            "launch_contract":{"bindings":[
                {"id":"chase","target":{"channel_name":"GOBO WHEEL","type":"GOBO"},
                 "source":{"kind":"step_chase","values_from":"source_attribute.goboSetting.steps","period_beats":1},
                 "resolve":{"mode":"override","priority":50}}]}}]}"#;
        let list: CueListResponse = serde_json::from_str(json).unwrap();
        let cues: Vec<ActiveCue> = list.cues.iter().map(ActiveCue::from_dto).collect();
        let mut reports = std::collections::HashMap::new();
        let snapshot = CueSnapshot { energy: 1.0, reactivity: 1.0, ..Default::default() };
        let frame = render_dmx_frame_with_cues(
            &snapshot,
            &patch,
            &profiles,
            None,
            &cues,
            &mut reports,
            &FixtureOverrides::default(),
        );
        let byte = frame.data[0];
        assert!(
            (40..=49).contains(&byte),
            "gobo byte must land inside the step's wheel range 40..49, got {}",
            byte
        );
        assert_ne!(byte, 32, "must not collapse onto the legacy ×32 slot grid");
    }

    #[test]
    fn global_brightness_scales_non_laser_dimmer() {
        // Wash fixture at full intent gets scaled by global_brightness;
        // applies to dimmer + RGB + white uniformly.
        let mut intent = FixtureIntentState {
            dimmer: 1.0, red: 0.8, green: 0.4, blue: 0.6, white: 0.2,
            ..Default::default()
        };
        let fixture = FixturePatch {
            footprint: 0,
            screen: None,
            id: "w".into(), name: "wash".into(), universe: 0, start_address: 1,
            profile_id: "p".into(), mode: "m".into(),
            position: StageVec3::zero(), rotation: StageVec3::zero(), scale: StageVec3::one(),
            role: FixtureRole::Wash, visual_kind: StageVisualKind::Wash, tags: vec![],
            channels: ChannelMap { dimmer: Some(1), red: Some(2), green: Some(3), blue: Some(4), white: Some(5),
                uv: None, strobe: None, color_wheel: None, pan: None, tilt: None, zoom: None, focus: None,
                gobo_slot: None, gobo_rotation: None, prism: None, laser_macro: None },
        };
        let snapshot = CueSnapshot { global_brightness: 0.5, ..Default::default() };
        apply_global_overrides(&mut intent, &fixture, &snapshot, true);
        assert!((intent.dimmer - 0.5).abs() < 1e-4);
        assert!((intent.red - 0.4).abs() < 1e-4);
        assert!((intent.green - 0.2).abs() < 1e-4);
        assert!((intent.blue - 0.3).abs() < 1e-4);
        assert!((intent.white - 0.1).abs() < 1e-4);
    }

    #[test]
    fn global_brightness_skips_lasers() {
        // Lasers use the Dimmer slot as a binary LASER ON/OFF gate (slice
        // 2d), so the master brightness must NOT scale them — otherwise a
        // slider <50% would silently turn the laser off.
        let mut intent = FixtureIntentState { dimmer: 1.0, ..Default::default() };
        let fixture = FixturePatch {
            footprint: 0,
            screen: None,
            id: "lz".into(), name: "laser".into(), universe: 0, start_address: 1,
            profile_id: "p".into(), mode: "m".into(),
            position: StageVec3::zero(), rotation: StageVec3::zero(), scale: StageVec3::one(),
            role: FixtureRole::Accent, visual_kind: StageVisualKind::Laser, tags: vec![],
            channels: ChannelMap { dimmer: Some(1), red: None, green: None, blue: None, white: None,
                uv: None, strobe: None, color_wheel: None, pan: None, tilt: None, zoom: None, focus: None,
                gobo_slot: None, gobo_rotation: None, prism: None, laser_macro: None },
        };
        let snapshot = CueSnapshot { global_brightness: 0.2, ..Default::default() };
        apply_global_overrides(&mut intent, &fixture, &snapshot, true);
        assert!((intent.dimmer - 1.0).abs() < 1e-4, "laser dimmer unchanged at 0.2 brightness, got {}", intent.dimmer);
    }

    // ---- per-fixture live overrides (intensity / solo / mute) ----------------

    fn wash_fixture(id: &str) -> FixturePatch {
        FixturePatch {
            footprint: 0,
            screen: None,
            id: id.into(), name: id.into(), universe: 0, start_address: 1,
            profile_id: "p".into(), mode: "m".into(),
            position: StageVec3::zero(), rotation: StageVec3::zero(), scale: StageVec3::one(),
            role: FixtureRole::Wash, visual_kind: StageVisualKind::Wash, tags: vec![],
            channels: ChannelMap { dimmer: Some(1), red: Some(2), green: Some(3), blue: Some(4), white: Some(5),
                uv: None, strobe: None, color_wheel: None, pan: None, tilt: None, zoom: None, focus: None,
                gobo_slot: None, gobo_rotation: None, prism: None, laser_macro: None },
        }
    }

    fn make_laser(id: &str) -> FixturePatch {
        let mut fx = wash_fixture(id);
        fx.role = FixtureRole::Accent;
        fx.visual_kind = StageVisualKind::Laser;
        fx
    }

    #[test]
    fn per_fixture_intensity_scales_non_laser() {
        let mut intent = FixtureIntentState { dimmer: 1.0, red: 0.8, green: 0.4, blue: 0.6, white: 0.2, ..Default::default() };
        let fixture = wash_fixture("w");
        let mut overrides = FixtureOverrides::default();
        overrides.set("w", FixtureOverride { intensity: 0.5, mute: false, solo: false, ..Default::default() });
        apply_per_fixture_overrides(&mut intent, &fixture, &overrides, false);
        assert!((intent.dimmer - 0.5).abs() < 1e-4, "dimmer halved, got {}", intent.dimmer);
        assert!((intent.red - 0.4).abs() < 1e-4);
        assert!((intent.white - 0.1).abs() < 1e-4);
    }

    #[test]
    fn per_fixture_intensity_skips_lasers() {
        // A laser's dimmer is a binary enable gate, so the intensity trim must
        // leave it alone (mirrors global_brightness_skips_lasers).
        let mut intent = FixtureIntentState { dimmer: 1.0, ..Default::default() };
        let fixture = make_laser("lz");
        let mut overrides = FixtureOverrides::default();
        overrides.set("lz", FixtureOverride { intensity: 0.2, mute: false, solo: false, ..Default::default() });
        apply_per_fixture_overrides(&mut intent, &fixture, &overrides, false);
        assert!((intent.dimmer - 1.0).abs() < 1e-4, "laser dimmer untouched by intensity trim, got {}", intent.dimmer);
    }

    #[test]
    fn per_fixture_mute_zeros_everything_including_laser_macro() {
        let mut intent = FixtureIntentState {
            dimmer: 0.9, red: 0.5, green: 0.5, blue: 0.5, white: 0.5, strobe: 0.7, laser_macro: 200,
            ..Default::default()
        };
        let fixture = make_laser("lz");
        let mut overrides = FixtureOverrides::default();
        overrides.set("lz", FixtureOverride { intensity: 1.0, mute: true, solo: false, ..Default::default() });
        apply_per_fixture_overrides(&mut intent, &fixture, &overrides, false);
        assert_eq!(intent.dimmer, 0.0);
        assert_eq!(intent.strobe, 0.0);
        assert_eq!(intent.red, 0.0);
        assert_eq!(intent.laser_macro, 0, "a muted laser must zero its macro so it truly goes dark");
    }

    #[test]
    fn per_fixture_solo_silences_others_and_passes_soloed_at_its_trim() {
        let mut soloed = FixtureIntentState { dimmer: 1.0, ..Default::default() };
        let mut other = FixtureIntentState { dimmer: 1.0, ..Default::default() };
        let fx_solo = wash_fixture("a");
        let fx_other = wash_fixture("b");
        let mut overrides = FixtureOverrides::default();
        overrides.set("a", FixtureOverride { intensity: 0.5, mute: false, solo: true, ..Default::default() });
        // any_solo = true (a fixture is soloed).
        apply_per_fixture_overrides(&mut soloed, &fx_solo, &overrides, true);
        apply_per_fixture_overrides(&mut other, &fx_other, &overrides, true);
        assert!((soloed.dimmer - 0.5).abs() < 1e-4, "soloed fixture plays at its intensity trim, got {}", soloed.dimmer);
        assert_eq!(other.dimmer, 0.0, "a non-soloed fixture is silenced while any solo is active");
    }

    #[test]
    fn any_solo_ignores_stale_ids() {
        // A solo entry for a fixture not in the patch must NOT silence the rig.
        let patch = DmxPatch::demo_rgbw_wash(0, 1);
        let mut overrides = FixtureOverrides::default();
        overrides.set("ghost-not-in-patch", FixtureOverride { solo: true, ..Default::default() });
        assert!(!overrides.any_solo(&patch), "a solo on a missing fixture must not poison any_solo");
    }

    // ---- UV mode (trigger_uv): "blackout, except blacklight" -----------------

    /// Two-wash patch: "plain" (no UV channel, ch1-5) and "uv" (ch6-10 with a
    /// UV emitter on ch11). Shared by the UV-mode tests below.
    fn uv_test_patch() -> DmxPatch {
        let plain = wash_fixture("plain");
        let mut uv = wash_fixture("uv");
        uv.start_address = 6;
        uv.channels = ChannelMap {
            dimmer: Some(6), red: Some(7), green: Some(8), blue: Some(9), white: Some(10),
            uv: Some(11), ..Default::default()
        };
        DmxPatch { universe: 0, trusses: vec![], venue: None, fixtures: vec![plain, uv] }
    }

    #[test]
    fn uv_mode_darkens_non_uv_and_lights_uv_channel_at_level_times_gb() {
        let patch = uv_test_patch();
        let snapshot = CueSnapshot {
            energy: 1.0,
            reactivity: 1.0,
            trigger_uv: true,
            uv_level: 0.8,
            global_brightness: 0.5,
            ..Default::default()
        };
        let frame = render_dmx_frame(&snapshot, &patch, &ProfileMap::new(), None);
        assert!(!frame.blackout, "UV mode is not a frame blackout");
        // Non-UV fixture: fully dark (ch1-5).
        assert!(
            frame.data[0..5].iter().all(|v| *v == 0),
            "non-UV fixture fully dark, got {:?}",
            &frame.data[0..5]
        );
        // UV-capable fixture: emitters dark (ch6-10), UV lit at uv_level × GB.
        assert!(
            frame.data[5..10].iter().all(|v| *v == 0),
            "UV fixture's dimmer/RGBW dark, got {:?}",
            &frame.data[5..10]
        );
        assert_eq!(
            frame.data[10],
            (0.8f32 * 0.5 * 255.0).round() as u8,
            "uv channel = uv_level × global_brightness (ch11)"
        );
    }

    #[test]
    fn blackout_overrides_uv_mode() {
        let patch = uv_test_patch();
        let snapshot = CueSnapshot {
            energy: 1.0,
            trigger_uv: true,
            trigger_blackout: true,
            uv_level: 1.0,
            ..Default::default()
        };
        let frame = render_dmx_frame(&snapshot, &patch, &ProfileMap::new(), None);
        assert!(frame.blackout);
        assert!(frame.data.iter().all(|v| *v == 0), "blackout kills UV too");
    }

    #[test]
    fn blinder_hold_beats_uv_mode() {
        let patch = uv_test_patch();
        let snapshot = CueSnapshot {
            energy: 1.0,
            reactivity: 1.0,
            trigger_uv: true,
            trigger_blinder: true,
            uv_level: 1.0,
            ..Default::default()
        };
        let frame = render_dmx_frame(&snapshot, &patch, &ProfileMap::new(), None);
        // White blast wins while held; UV resumes after release (stateless
        // per-frame precedence, nothing to un-stick).
        assert_eq!(frame.data[0], 255, "plain wash blasts white (ch1 dimmer)");
        assert_eq!(frame.data[5], 255, "UV wash blasts white (ch6 dimmer)");
        assert_eq!(frame.data[10], 0, "UV channel dark under the blinder — white owns the moment (ch11)");
    }

    #[test]
    fn pixel_bar_blinder_and_strobe_drive_white_without_a_cue() {
        use crate::emerge::contracts::SegmentChannels;
        // An 8-cell bar (seg1 R/G/B = 1/2/3 … seg8 = 22/23/24), like the
        // SHEHDS 200W in 24-channel mode. The fixture's own ChannelMap lives
        // far away (start 100) so it can't collide with the segment cells.
        let segments: Vec<SegmentChannels> = (0..8u16)
            .map(|i| SegmentChannels {
                red: Some(1 + i * 3),
                green: Some(2 + i * 3),
                blue: Some(3 + i * 3),
                white: None,
            })
            .collect();
        let fixture = FixturePatch::demo_led_panel("bar", 0, 100);

        let render = |snap: &CueSnapshot| {
            let mut frame = DmxFrame::new(0, 0, 0, false);
            apply_segmented_fixture(
                &mut frame,
                &fixture,
                FixtureIntentState::default(),
                snap,
                &segments,
                None, // NO segment cue running — the bug was white only engaged WITH a cue
                0.0,
                FixtureOverride::default(),
                false,
            );
            frame
        };

        // BLINDER held, no cue: every cell full white.
        let blind = render(&CueSnapshot { trigger_blinder: true, ..Default::default() });
        for i in 0..8u16 {
            let base = (i * 3) as usize;
            assert_eq!(blind.data[base], 255, "cell {i} R white under blinder");
            assert_eq!(blind.data[base + 1], 255, "cell {i} G white under blinder");
            assert_eq!(blind.data[base + 2], 255, "cell {i} B white under blinder");
        }

        // STROBE held, ON phase (strobe_on = wall-clock square wave from the
        // DMX manager): cells flash white, peaking at Global Brightness.
        let on = render(&CueSnapshot {
            trigger_strobe: true,
            strobe_on: true,
            global_brightness: 1.0,
            ..Default::default()
        });
        assert_eq!(on.data[0], 255, "strobe ON: cell0 white at full gb");
        assert_eq!(on.data[23], 255, "strobe ON: cell8 white");
        // Global Brightness caps the flash (peaks at the master).
        let dimmed = render(&CueSnapshot {
            trigger_strobe: true,
            strobe_on: true,
            global_brightness: 0.5,
            ..Default::default()
        });
        assert_eq!(dimmed.data[0], 128, "strobe ON peaks at global brightness (0.5 → 128)");
        // OFF phase: dark.
        let off = render(&CueSnapshot {
            trigger_strobe: true,
            strobe_on: false,
            global_brightness: 1.0,
            ..Default::default()
        });
        assert_eq!(off.data[0], 0, "strobe OFF: cell0 dark");
        assert_eq!(off.data[23], 0, "strobe OFF: cell8 dark");

        // BLINDER beats STROBE when both held (solid white wins, off phase too).
        let both = render(&CueSnapshot {
            trigger_blinder: true,
            trigger_strobe: true,
            strobe_on: false,
            ..Default::default()
        });
        assert_eq!(both.data[0], 255, "blinder beats strobe: cell0 stays white");
    }

    #[test]
    fn manual_strobe_white_flashes_flat_fixtures_peaking_at_global_brightness() {
        let fixture = FixturePatch::demo_led_panel("p", 0, 1); // non-laser wash
        let strobe = |on: bool, gb: f32, sw: bool, mut intent: FixtureIntentState| {
            apply_global_overrides(
                &mut intent,
                &fixture,
                &CueSnapshot {
                    trigger_strobe: true,
                    strobe_on: on,
                    global_brightness: gb,
                    ..Default::default()
                },
                sw,
            );
            intent
        };

        // ON phase at full master → white.
        let i = strobe(true, 1.0, true, FixtureIntentState { red: 0.2, green: 0.4, blue: 0.6, ..Default::default() });
        assert_eq!((i.red, i.green, i.blue), (1.0, 1.0, 1.0), "strobe ON white-flashes a flat fixture");

        // ON phase peaks at Global Brightness (white × gb).
        let i = strobe(true, 0.5, true, FixtureIntentState::default());
        assert!((i.red - 0.5).abs() < 1e-6, "strobe peaks at global brightness, got {}", i.red);

        // OFF phase → dark.
        let i = strobe(false, 1.0, true, FixtureIntentState { red: 0.9, ..Default::default() });
        assert_eq!(i.red, 0.0, "strobe OFF darkens the fixture");

        // Self-managed bar (GigBAR passes software_strobe=false) → untouched,
        // keeps its native strobe pods.
        let i = strobe(false, 1.0, false, FixtureIntentState { red: 0.7, ..Default::default() });
        assert_eq!(i.red, 0.7, "self-managed bar is not software-white-flashed");
    }

    #[test]
    fn strobe_applies_uniformly_to_all_strobe_capable_fixtures() {
        // The unified rule: the autonomous buildup / Pump strobes EVERY
        // strobe-capable (non-laser) fixture the same way — no per-brand or
        // per-visual-kind cases. A plain wash strobes during a build just like
        // the bar; only the manual BLINDER stays a solid hold; the GigBAR
        // (software_strobe=false) opts out for its native pods.
        let wash = FixturePatch::demo_led_panel("w", 0, 1);
        let verdict = |snap: CueSnapshot, sw: bool| {
            let mut i = FixtureIntentState { red: 0.4, green: 0.4, blue: 0.4, ..Default::default() };
            apply_global_overrides(&mut i, &wash, &snap, sw);
            i
        };
        let base = CueSnapshot { global_brightness: 1.0, style_brightness_ceiling: 1.0, ..Default::default() };

        // Auto buildup (BlindersHold) strobes the wash white on the ON phase…
        let on = verdict(CueSnapshot { auto_blinder_active: true, strobe_on: true, ..base.clone() }, true);
        assert_eq!((on.red, on.green, on.blue), (1.0, 1.0, 1.0), "buildup strobes a plain wash white");
        // …and dark on the OFF phase (not a solid coil).
        let off = verdict(CueSnapshot { auto_blinder_active: true, strobe_on: false, ..base.clone() }, true);
        assert_eq!((off.red, off.green, off.blue), (0.0, 0.0, 0.0), "buildup OFF phase darkens the wash");
        // The Pump strobes too.
        let pump = verdict(CueSnapshot { overdrive_strobe_active: true, strobe_on: false, ..base.clone() }, true);
        assert_eq!(pump.red, 0.0, "pump OFF phase darkens the wash");
        // Manual BLINDER is still a solid hold.
        let blind = verdict(CueSnapshot { trigger_blinder: true, strobe_on: false, ..base.clone() }, true);
        assert_eq!(blind.red, 1.0, "manual blinder stays solid white");
        // GigBAR (software_strobe=false): buildup leaves it for the hardware pass
        // (solid coil here, overwritten downstream), not the software strobe.
        let gig = verdict(CueSnapshot { auto_blinder_active: true, strobe_on: false, ..base }, false);
        assert_eq!(gig.red, 1.0, "self-managed bar takes the solid coil, not the software strobe");
    }

    #[test]
    fn style_brightness_ceiling_caps_emitters() {
        // The effective style's ceiling (Chill 0.5) caps every emitter on top
        // of the operator master — a calm style reads dimmer.
        let fixture = FixturePatch::demo_led_panel("p", 0, 1);
        let mut intent = FixtureIntentState { dimmer: 1.0, red: 1.0, green: 0.6, ..Default::default() };
        apply_global_overrides(
            &mut intent,
            &fixture,
            &CueSnapshot { global_brightness: 1.0, style_brightness_ceiling: 0.5, ..Default::default() },
            true,
        );
        assert!((intent.dimmer - 0.5).abs() < 1e-6, "Chill ceiling halves the dimmer, got {}", intent.dimmer);
        assert!((intent.red - 0.5).abs() < 1e-6, "ceiling caps red, got {}", intent.red);
        assert!((intent.green - 0.3).abs() < 1e-6, "ceiling scales green, got {}", intent.green);

        // A MANUAL override (blinder/strobe/UV) bypasses the autonomous ceiling
        // — it beats the posture — but still respects the operator master.
        let mut blind = FixtureIntentState::default();
        apply_global_overrides(
            &mut blind,
            &fixture,
            &CueSnapshot { global_brightness: 1.0, style_brightness_ceiling: 0.5, trigger_blinder: true, ..Default::default() },
            true,
        );
        assert_eq!(blind.red, 1.0, "manual blinder bypasses the Chill ceiling (full white)");
        // …but the operator master still caps it.
        let mut blind_dim = FixtureIntentState::default();
        apply_global_overrides(
            &mut blind_dim,
            &fixture,
            &CueSnapshot { global_brightness: 0.6, style_brightness_ceiling: 0.5, trigger_blinder: true, ..Default::default() },
            true,
        );
        assert!((blind_dim.red - 0.6).abs() < 1e-6, "manual override still respects the master (0.6), got {}", blind_dim.red);
    }

    #[test]
    fn auto_buildup_strobes_the_bar_not_solid() {
        use crate::emerge::contracts::SegmentChannels;
        let segments: Vec<SegmentChannels> = (0..4u16)
            .map(|i| SegmentChannels { red: Some(1 + i * 3), green: Some(2 + i * 3), blue: Some(3 + i * 3), white: None })
            .collect();
        let fixture = FixturePatch::demo_led_panel("bar", 0, 100);
        let render = |snap: &CueSnapshot| {
            let mut frame = DmxFrame::new(0, 0, 0, false);
            apply_segmented_fixture(
                &mut frame, &fixture, FixtureIntentState::default(), snap, &segments,
                None, 0.0, FixtureOverride::default(), false,
            );
            frame
        };
        let base = CueSnapshot { global_brightness: 1.0, style_brightness_ceiling: 1.0, ..Default::default() };

        // AUTO buildup (BlindersHold) STROBES the bar: white on the ON phase…
        let on = render(&CueSnapshot { auto_blinder_active: true, strobe_on: true, ..base.clone() });
        assert_eq!(on.data[0], 255, "auto buildup ON phase: cell white");
        // …dark on the OFF phase (a flat coil would stay white).
        let off = render(&CueSnapshot { auto_blinder_active: true, strobe_on: false, ..base.clone() });
        assert_eq!(off.data[0], 0, "auto buildup OFF phase: cell dark (strobing, not solid)");

        // The Pump (OverdriveStrobe) strobes too.
        let pump_off = render(&CueSnapshot { overdrive_strobe_active: true, strobe_on: false, ..base.clone() });
        assert_eq!(pump_off.data[0], 0, "pump OFF phase: cell dark");

        // MANUAL blinder is still a SOLID hold (white on both phases).
        let manual = render(&CueSnapshot { trigger_blinder: true, strobe_on: false, ..base });
        assert_eq!(manual.data[0], 255, "manual blinder stays solid white");
    }

    #[test]
    fn uv_mode_respects_per_fixture_trim_and_mute() {
        let patch = uv_test_patch();
        let snapshot = CueSnapshot {
            energy: 1.0,
            reactivity: 1.0,
            trigger_uv: true,
            uv_level: 0.8,
            ..Default::default()
        };
        // Intensity trim composes into the UV drive (uv_level × GB × trim).
        let mut overrides = FixtureOverrides::default();
        overrides.set("uv", FixtureOverride { intensity: 0.5, ..Default::default() });
        let mut reports = HashMap::new();
        let frame = render_dmx_frame_with_cues(
            &snapshot, &patch, &ProfileMap::new(), None, &[], &mut reports, &overrides,
        );
        assert_eq!(frame.data[10], (0.8f32 * 0.5 * 255.0).round() as u8, "trim scales the UV byte");
        // Mute silences UV mode like every other emitter.
        overrides.set("uv", FixtureOverride { mute: true, ..Default::default() });
        let frame = render_dmx_frame_with_cues(
            &snapshot, &patch, &ProfileMap::new(), None, &[], &mut reports, &overrides,
        );
        assert_eq!(frame.data[10], 0, "a muted fixture must not UV-glow");
    }

    #[test]
    fn out_of_range_channels_are_ignored() {
        let patch = DmxPatch {
            universe: 0,
            trusses: vec![],
            venue: None,
            fixtures: vec![FixturePatch {
                footprint: 0,
                screen: None,
                id: "bad".to_string(),
                name: "bad patch".to_string(),
                universe: 0,
                start_address: 0,
                profile_id: "bad-profile".to_string(),
                mode: "bad".to_string(),
                position: StageVec3::zero(),
                rotation: StageVec3::zero(),
                scale: StageVec3::one(),
                role: FixtureRole::Wash,
                visual_kind: StageVisualKind::Wash,
                tags: vec![],
                channels: ChannelMap { dimmer: Some(0), red: Some(513), green: None, blue: None, white: None, uv: None, strobe: None, color_wheel: None, pan: None, tilt: None, zoom: None, focus: None, gobo_slot: None, gobo_rotation: None, prism: None, laser_macro: None },
            }],
        };
        let frame = render_dmx_frame(&CueSnapshot::default(), &patch, &ProfileMap::new(), None);
        assert!(frame.data.iter().all(|v| *v == 0));
    }
}


#[cfg(test)]
mod contract_mapping_tests {
    use super::*;
    use crate::emerge::contracts::{ChannelMap, DmxPatch, FixturePatch, FixtureRole, StageVisualKind, StageVec3};

    #[test]
    fn multi_universe_render_only_uses_selected_universe() {
        let patch = DmxPatch {
            universe: 1,
            trusses: vec![],
            venue: None,
            fixtures: vec![
                FixturePatch {
                    footprint: 0,
                    screen: None,
                    id: "u1".to_string(),
                    name: "Universe 1 wash".to_string(),
                    universe: 1,
                    start_address: 1,
                    profile_id: "rgbw".to_string(),
                    mode: "RGBW".to_string(),
                    position: StageVec3::zero(),
                    rotation: StageVec3::zero(),
                    scale: StageVec3::one(),
                    role: FixtureRole::Wash,
                    visual_kind: StageVisualKind::Wash,
                    tags: vec!["front".to_string()],
                    channels: ChannelMap::rgbw_dimmer(1),
                },
                FixturePatch {
                    footprint: 0,
                    screen: None,
                    id: "u2".to_string(),
                    name: "Universe 2 wash".to_string(),
                    universe: 2,
                    start_address: 100,
                    profile_id: "rgbw".to_string(),
                    mode: "RGBW".to_string(),
                    position: StageVec3::zero(),
                    rotation: StageVec3::zero(),
                    scale: StageVec3::one(),
                    role: FixtureRole::Wash,
                    visual_kind: StageVisualKind::Wash,
                    tags: vec!["rear".to_string()],
                    channels: ChannelMap::rgbw_dimmer(100),
                },
            ],
        };
        let snapshot = CueSnapshot { energy: 1.0, reactivity: 1.0, ..Default::default() };
        let frame = render_dmx_frame(&snapshot, &patch, &ProfileMap::new(), None);
        assert_eq!(frame.universe, 1);
        assert!(frame.data[0] > 0);
        assert_eq!(frame.data[99], 0);
    }
}

#[cfg(test)]
mod gigbar_section_tests {
    use super::*;
    use crate::emerge::contracts::{
        ChannelDefinition, ChannelMap, DmxAttribute, DmxPatch, FixtureHousing, FixtureKind,
        FixturePatch, FixtureProfile, FixtureRole, StageVec3, StageVisualKind,
    };
    use crate::hue::CueSnapshot;

    /// A synthetic GigBAR Move profile: only the fields the expander reads
    /// (manufacturer/model/channel_count + per-channel offset/default).
    fn gigbar_profile(channel_count: u16) -> FixtureProfile {
        let channels = (1..=channel_count)
            .map(|off| ChannelDefinition {
                attribute: DmxAttribute::Dimmer, // not read by the expander
                channel_offset: off,
                // The par dim/strobe combo channels carry a static full value.
                default_value: if off == 5 || off == 10 { 127 } else { 0 },
                blackout_value: 0,
                detail: None,
                source_attribute: None,
            })
            .collect();
        FixtureProfile {
            id: "gigbar".into(),
            manufacturer: "Chauvet DJ".into(),
            model: "GigBAR Move".into(),
            mode: "test".into(),
            kind: FixtureKind::Unknown,
            housing: FixtureHousing::Generic,
            channel_count,
            channels,
            capabilities: vec![],
        }
    }

    fn gigbar_patch(base: u16) -> (DmxPatch, ProfileMap, FixtureProfile) {
        let fixture = FixturePatch {
            footprint: 0,
            screen: None,
            id: "bar".into(),
            name: "GigBAR".into(),
            universe: 0,
            start_address: base,
            profile_id: "gigbar".into(),
            mode: "35".into(),
            position: StageVec3::zero(),
            rotation: StageVec3::zero(),
            scale: StageVec3::one(),
            // SaaS-supplied role/kind are intentionally IGNORED by the expander.
            role: FixtureRole::Accent,
            visual_kind: StageVisualKind::Beam,
            tags: vec![],
            channels: ChannelMap::default(),
        };
        let patch = DmxPatch { universe: 0, trusses: vec![], venue: None, fixtures: vec![fixture] };
        (patch, ProfileMap::new(), gigbar_profile(35))
    }

    #[test]
    fn expand_returns_eight_sections_for_35ch() {
        let profile = gigbar_profile(35);
        let fixture = gigbar_patch(1).0.fixtures.remove(0);
        let sections = expand_sections(&fixture, Some(&profile)).expect("gigbar expands");
        assert_eq!(sections.len(), 8, "35ch GigBAR has 8 sections");
        // Two moving heads counter-phase (ordinals 0 and 1, count 2).
        let movers: Vec<_> = sections.iter().filter(|s| s.mover_count == 2).collect();
        assert_eq!(movers.len(), 2);
        assert_eq!(movers[0].mover_ordinal, 0);
        assert_eq!(movers[1].mover_ordinal, 1);
        // Every section keeps the PARENT id so overrides resolve rig-wide.
        assert!(sections.iter().all(|s| s.fixture.id == "bar"));
    }

    #[test]
    fn expand_returns_four_sections_for_17ch() {
        let profile = gigbar_profile(17);
        let mut fixture = gigbar_patch(1).0.fixtures.remove(0);
        fixture.mode = "17".into();
        let sections = expand_sections(&fixture, Some(&profile)).expect("gigbar expands");
        assert_eq!(sections.len(), 4, "17ch GigBAR has 4 sections");
    }

    #[test]
    fn non_gigbar_profile_does_not_expand() {
        let mut profile = gigbar_profile(35);
        profile.manufacturer = "SHEHDS".into();
        profile.model = "LED Beam".into();
        let fixture = gigbar_patch(1).0.fixtures.remove(0);
        assert!(expand_sections(&fixture, Some(&profile)).is_none());
        // And no profile at all -> no expansion (flat path).
        assert!(expand_sections(&fixture, None).is_none());
    }

    #[test]
    fn manual_override_beats_autonomous_silence_blackout() {
        // THE "triggers don't fire without audio" root cause: after ~2s of
        // raw silence the CueEngine latches `blackout_active` and the frame
        // blacked out BEFORE the manual override was considered. A held
        // BLINDER must punch through the autonomous silence blackout...
        let patch = DmxPatch::demo_rgbw_wash(0, 1);
        let snap = CueSnapshot {
            blackout_active: true,
            trigger_blinder: true,
            global_brightness: 1.0,
            ..Default::default()
        };
        let frame = render_dmx_frame(&snap, &patch, &ProfileMap::new(), None);
        assert!(
            frame.data.iter().any(|&b| b == 255),
            "held BLINDER must light the rig through the silence blackout"
        );

        // ...same for STROBE (ON phase) and UV...
        let snap = CueSnapshot {
            blackout_active: true,
            trigger_strobe: true,
            strobe_on: true,
            global_brightness: 1.0,
            ..Default::default()
        };
        let frame = render_dmx_frame(&snap, &patch, &ProfileMap::new(), None);
        assert!(
            frame.data.iter().any(|&b| b == 255),
            "held STROBE (ON phase) must flash through the silence blackout"
        );

        // ...but the MANUAL blackout still beats every other trigger
        // (operator priority order: Blackout > Blinder > Strobe > UV).
        let snap = CueSnapshot {
            trigger_blackout: true,
            trigger_blinder: true,
            global_brightness: 1.0,
            ..Default::default()
        };
        let frame = render_dmx_frame(&snap, &patch, &ProfileMap::new(), None);
        assert!(
            frame.data.iter().all(|&b| b == 0),
            "manual BLACKOUT must win over a held BLINDER"
        );
    }

    #[test]
    fn gigbar_17_software_strobes_on_manual_strobe_even_in_silence() {
        // Pins the section call site passing `!self_managed` (the 17ch has no
        // hardware pass, so it MUST take the software strobe) AND the bar-gate
        // manual-override bypass (energy 0.0 = silence; the flash must not be
        // energy-collapsed). These exact lines were lost from the working
        // tree once before — this test makes a silent revert impossible.
        let (mut patch, mut profiles, _) = gigbar_patch(1);
        patch.fixtures[0].mode = "17".into();
        profiles.insert("bar".into(), gigbar_profile(17));
        let snap = |on: bool| CueSnapshot {
            trigger_strobe: true,
            strobe_on: on,
            energy: 0.0,
            global_brightness: 1.0,
            ..Default::default()
        };

        let frame_on = render_dmx_frame(&snap(true), &patch, &profiles, None);
        assert_eq!(frame_on.data[0], 255, "Pars RGB white-flash on the ON phase (ch1)");
        assert_eq!(frame_on.data[9], 255, "Strobe pod dimmer flashes on the ON phase (ch10)");
        assert_eq!(frame_on.data[13], 255, "Moving-head dimmer flashes on the ON phase (ch14)");

        let frame_off = render_dmx_frame(&snap(false), &patch, &profiles, None);
        assert_eq!(frame_off.data[0], 0, "Pars dark on the OFF phase (ch1)");
        assert_eq!(frame_off.data[9], 0, "Strobe pod dark on the OFF phase (ch10)");
        assert_eq!(frame_off.data[13], 0, "Moving-head dark on the OFF phase (ch14)");
    }

    #[test]
    fn absolutize_anchors_offsets_at_base() {
        let offsets = ChannelMap { red: Some(1), pan: Some(24), gobo_slot: Some(35), ..Default::default() };
        let abs = absolutize_offsets(&offsets, 10);
        assert_eq!(abs.red, Some(10)); // base 10, offset 1 -> abs 10
        assert_eq!(abs.pan, Some(33)); // 10 + 24 - 1
        assert_eq!(abs.gobo_slot, Some(44)); // 10 + 35 - 1
    }

    #[test]
    fn render_lights_all_gigbar_sections_over_one_fixture() {
        let (patch, mut profiles, profile) = gigbar_patch(1);
        profiles.insert("bar".into(), profile);
        // gate=None lets the Laser/Strobe lanes self-emit (passive bench look).
        let snapshot = CueSnapshot { energy: 1.0, reactivity: 1.0, ..Default::default() };
        let frame = render_dmx_frame(&snapshot, &patch, &profiles, None);

        // Par dim/strobe static defaults are stamped (proves both par sections render).
        assert_eq!(frame.data[4], 127, "Par 1 ch5 static default stamped");
        assert_eq!(frame.data[9], 127, "Par 2 ch10 static default stamped");
        // Laser macro lands on ch17 (gate=None -> laser fires).
        assert!(frame.data[16] > 0, "Laser ch17 fires, got {}", frame.data[16]);
        // At least one moving-head dimmer (ch26 / ch32) is lit.
        assert!(
            frame.data[25] > 0 || frame.data[31] > 0,
            "a moving-head dimmer should be lit (ch26={}, ch32={})",
            frame.data[25],
            frame.data[31]
        );
    }

    #[test]
    fn bar_energy_gate_is_dark_in_quiet_and_full_when_loud() {
        assert_eq!(bar_energy_gate(0.0), 0.0, "silence -> bar dark");
        assert_eq!(bar_energy_gate(BAR_GATE_LO - 0.01), 0.0, "below the floor -> dark");
        assert_eq!(bar_energy_gate(1.0), 1.0, "loud -> bar full");
        let groove = bar_energy_gate(0.30);
        assert!(groove > 0.0 && groove < 1.0, "mid energy is partway up, got {groove}");
        assert!(bar_energy_gate(0.45) > groove, "gate rises monotonically with energy");
    }

    #[test]
    fn gigbar_collapses_in_quiet_passages() {
        let (patch, mut profiles, profile) = gigbar_patch(1);
        profiles.insert("bar".into(), profile);
        // A near-silent passage: energy below the gate floor.
        let quiet = CueSnapshot { energy: 0.02, reactivity: 1.0, ..Default::default() };
        let frame = render_dmx_frame(&quiet, &patch, &profiles, None);
        // The moving-head dimmers (ch26 / ch32) are gated to black — no 24/7 wall.
        assert_eq!(frame.data[25], 0, "MH1 dimmer dark in quiet (ch26)");
        assert_eq!(frame.data[31], 0, "MH2 dimmer dark in quiet (ch32)");
        // The par RGB collapses too (the par dim/strobe static can stay; with RGB at
        // 0 the pars emit nothing).
        assert_eq!(frame.data[0], 0, "Par 1 red dark in quiet (ch1)");
    }

    #[test]
    fn global_brightness_master_dims_the_gigbar() {
        // Proves the global-brightness master reaches every bar section — and with
        // it the silence master (DmxManager folds the SilenceGate into global
        // brightness, so silence == global_brightness -> 0 here).
        let (patch, mut profiles, profile) = gigbar_patch(1);
        profiles.insert("bar".into(), profile);

        // Loud track but GB at 0 -> the bar is dark.
        let dark = CueSnapshot { energy: 1.0, reactivity: 1.0, global_brightness: 0.0, ..Default::default() };
        let frame = render_dmx_frame(&dark, &patch, &profiles, None);
        assert_eq!(frame.data[25], 0, "MH1 dimmer dark at GB=0 (ch26)");
        assert_eq!(frame.data[31], 0, "MH2 dimmer dark at GB=0 (ch32)");
        assert_eq!(frame.data[0], 0, "Par 1 red dark at GB=0 (ch1)");

        // Same loud track at full GB -> the bar lights.
        let lit = CueSnapshot { energy: 1.0, reactivity: 1.0, global_brightness: 1.0, ..Default::default() };
        let frame2 = render_dmx_frame(&lit, &patch, &profiles, None);
        assert!(frame2.data[25] > 0, "MH1 dimmer lit at GB=1 (ch26)");
    }

    #[test]
    fn section_spread_follows_bar_pan() {
        let profile = gigbar_profile(35);
        let mut fixture = gigbar_patch(1).0.fixtures.remove(0);
        // Pan the bar 90° -> the section row runs along world Y, not X.
        fixture.rotation = StageVec3 { x: 0.0, y: 90.0, z: 0.0 };
        let sections = expand_sections(&fixture, Some(&profile)).expect("expands");
        let span = |vals: Vec<f32>| {
            let max = vals.iter().cloned().fold(f32::MIN, f32::max);
            let min = vals.iter().cloned().fold(f32::MAX, f32::min);
            max - min
        };
        let xs = span(sections.iter().map(|s| s.fixture.position.x).collect());
        let ys = span(sections.iter().map(|s| s.fixture.position.y).collect());
        assert!(ys > 1.0, "panned 90° spreads along Y, got span {ys}");
        assert!(xs < 0.05, "panned 90° barely varies X, got span {xs}");
    }

    #[test]
    fn base_address_offsets_the_whole_block() {
        // Same bar patched at DMX 101 -> par statics land at ch105 / ch110.
        let (mut patch, mut profiles, profile) = gigbar_patch(101);
        patch.fixtures[0].start_address = 101;
        profiles.insert("bar".into(), profile);
        let snapshot = CueSnapshot { energy: 1.0, reactivity: 1.0, ..Default::default() };
        let frame = render_dmx_frame(&snapshot, &patch, &profiles, None);
        assert_eq!(frame.data[104], 127, "Par 1 ch5 at base 101 -> data[104]");
        assert_eq!(frame.data[109], 127, "Par 2 ch10 at base 101 -> data[109]");
    }

    // ---- 35ch hardware pass (manual-verified control bytes) -----------------
    //
    // Channel↔index legend for base address 1: ch4=data[3], ch5=data[4],
    // ch9=data[8], ch10=data[9], ch17=data[16], ch20=data[19], ch21=data[20],
    // ch22=data[21], ch23=data[22]. All snapshots keep `energy_slow` at its 0.0
    // default so the COM-409A oscillators sample 0 (energy_env gate) — every
    // intent term below is deterministic.

    /// A ShowBrainFrame ruling every gated lane (Laser/Strobe) ineligible — the
    /// production posture between events. Lane order mirrors show_brain's
    /// `FAMILY_ORDER` (Wash, Mover, Gobo, Laser, Strobe, Blinder), which
    /// `ShowBrainFrame::lane` indexes by.
    fn brain_all_lanes_dark() -> ShowBrainFrame {
        use crate::emerge::cue::show_brain::ClockSource;
        use crate::emerge::{ArcState, CueLaneIntent, CuePriority, ShowClockSnapshot};
        let lane = |family| CueLaneIntent::inactive(family, CuePriority::Bed, "test gate");
        ShowBrainFrame {
            arc_state: ArcState::Groove,
            lanes: [
                lane(FixtureFamily::Wash),
                lane(FixtureFamily::Mover),
                lane(FixtureFamily::Gobo),
                lane(FixtureFamily::Laser),
                lane(FixtureFamily::Strobe),
                lane(FixtureFamily::Blinder),
            ],
            clock: ShowClockSnapshot::default(),
            clock_source: ClockSource::Derived,
        }
    }

    #[test]
    fn gigbar_blinder_hold_is_constant_white_not_a_program() {
        let (patch, mut profiles, profile) = gigbar_patch(1);
        profiles.insert("bar".into(), profile);
        // UV pressure deliberately maxed — white still owns the moment.
        let snapshot = CueSnapshot {
            energy: 1.0,
            reactivity: 1.0,
            auto_blinder_active: true,
            tension_bank: 1.0,
            uv_level: 1.0,
            ..Default::default()
        };
        let frame = render_dmx_frame(&snapshot, &patch, &profiles, None);
        assert_eq!(frame.data[19], 0, "blinder: ch20 program engine stays parked");
        assert!(frame.data[20] > 0, "blinder: ch21 white dimmer blasts");
        assert_eq!(frame.data[4], 252, "blinder: par 1 joins at full-power RGB (ch5)");
        assert_eq!(frame.data[9], 252, "blinder: par 2 joins at full-power RGB (ch10)");
        assert_eq!(frame.data[21], 0, "blinder: pod UV off — white owns the moment (ch22)");
        assert_eq!(frame.data[3], 0, "blinder: par UV off (ch4)");
    }

    #[test]
    fn gigbar_manual_trigger_fires_in_dead_silence() {
        // A MANUAL operator trigger must fire with NO music — the bar-energy
        // gate (which collapses the bar in quiet) is bypassed while a trigger
        // is held, so a held BLINDER blasts white even at energy 0.
        let (patch, mut profiles, profile) = gigbar_patch(1);
        profiles.insert("bar".into(), profile);
        let snapshot = CueSnapshot {
            energy: 0.0, // dead silence — the bar gate would otherwise zero it
            trigger_blinder: true,
            ..Default::default()
        };
        let frame = render_dmx_frame(&snapshot, &patch, &profiles, None);
        assert!(frame.data[4] > 0, "manual blinder lights par 1 in silence (ch5)");
        assert!(frame.data[9] > 0, "manual blinder lights par 2 in silence (ch10)");
        assert!(frame.data[20] > 0, "manual blinder white dimmer blasts in silence (ch21)");
    }

    #[test]
    fn gigbar_hard_drop_strobe_event_runs_all_pods() {
        let (patch, mut profiles, profile) = gigbar_patch(1);
        profiles.insert("bar".into(), profile);
        let snapshot = CueSnapshot {
            energy: 1.0,
            reactivity: 1.0,
            trigger_strobe: true,
            drop_impact: 0.8,
            ..Default::default()
        };
        let frame = render_dmx_frame(&snapshot, &patch, &profiles, None);
        let ch20 = frame.data[19];
        assert!(
            (190..=209).contains(&ch20),
            "drop_impact 0.8 -> all four pods strobing, got {ch20}"
        );
        assert!(frame.data[22] > 0, "ch23 engine speed rides energy");
        assert!(frame.data[20] > 0, "ch21 keeps the pods' brightness while the program flashes");
    }

    #[test]
    fn gigbar_strobe_event_rotates_auto_programs_by_look() {
        let (patch, mut profiles, profile) = gigbar_patch(1);
        profiles.insert("bar".into(), profile);
        // Low drop impact -> the 1-9 auto programs, band center by look_index
        // (band k+1 spans [10+20k, 29+20k]; we write its center 20+20k).
        for (look, expected) in [(0u32, 20u8), (1, 40), (4, 100), (8, 180), (9, 20)] {
            let snapshot = CueSnapshot {
                energy: 1.0,
                reactivity: 1.0,
                trigger_strobe: true,
                look_index: look,
                ..Default::default()
            };
            let frame = render_dmx_frame(&snapshot, &patch, &profiles, None);
            let ch20 = frame.data[19];
            assert_eq!(ch20, expected, "look {look} lands its program band center");
            assert!((10..=189).contains(&ch20), "auto programs stay inside bands 1-9");
        }
    }

    #[test]
    fn gigbar_par_rate_idles_at_passthrough_and_strobes_on_rig_moments() {
        let (patch, mut profiles, profile) = gigbar_patch(1);
        profiles.insert("bar".into(), profile);

        // Idle: byte-identical to today's static pass-through, engine parked.
        let idle = CueSnapshot { energy: 1.0, reactivity: 1.0, ..Default::default() };
        let frame = render_dmx_frame(&idle, &patch, &profiles, None);
        assert_eq!(frame.data[4], 127, "idle: par 1 rate stays at pass-through (ch5)");
        assert_eq!(frame.data[9], 127, "idle: par 2 rate stays at pass-through (ch10)");
        assert_eq!(frame.data[19], 0, "idle: ch20 engine parked");

        // A rig strobe moment swaps the pass-through for a par strobe whose
        // speed rides energy — 128 slow … 239 fast, never the 240-249 band.
        let half = CueSnapshot {
            energy: 0.5,
            reactivity: 1.0,
            overdrive_strobe_active: true,
            ..Default::default()
        };
        let frame = render_dmx_frame(&half, &patch, &profiles, None);
        let rate = frame.data[4];
        assert!((128..=239).contains(&rate), "par strobe band, got {rate}");
        assert_eq!(frame.data[9], rate, "both pars share the rate");
        let full = CueSnapshot {
            energy: 1.0,
            reactivity: 1.0,
            overdrive_strobe_active: true,
            ..Default::default()
        };
        let frame2 = render_dmx_frame(&full, &patch, &profiles, None);
        assert!(frame2.data[4] > rate, "more energy -> faster par strobe");
        assert_eq!(frame2.data[4], 239, "full energy caps UNDER the sound band");
    }

    #[test]
    fn gigbar_laser_alternates_on_hard_drop_only_when_lit() {
        let (patch, mut profiles, profile) = gigbar_patch(1);
        profiles.insert("bar".into(), profile);

        // Lit laser (gate=None lets the lane self-emit) + hard drop -> the
        // ch17 wheel flips to red+green alternate strobing.
        let drop = CueSnapshot { energy: 1.0, reactivity: 1.0, drop_impact: 0.7, ..Default::default() };
        let frame = render_dmx_frame(&drop, &patch, &profiles, None);
        assert_eq!(frame.data[16], 247, "lit laser flips to alternate strobing on the drop");

        // Same drop with the laser lane gated dark -> never light a gated laser.
        let brain = brain_all_lanes_dark();
        let frame = render_dmx_frame(&drop, &patch, &profiles, Some(&brain));
        assert_eq!(frame.data[16], 0, "gated laser stays dark through the drop");

        // Lit laser below the drop threshold -> the existing wheel byte stands.
        let groove = CueSnapshot { energy: 1.0, reactivity: 1.0, drop_impact: 0.3, ..Default::default() };
        let frame = render_dmx_frame(&groove, &patch, &profiles, None);
        assert_ne!(frame.data[16], 247, "no alternate override below the drop threshold");
        assert!(frame.data[16] > 0, "existing laser macro byte stands");
    }

    #[test]
    fn gigbar_uv_breathes_with_tension_and_defers_to_white() {
        let (patch, mut profiles, profile) = gigbar_patch(1);
        profiles.insert("bar".into(), profile);
        let tense = |uv_level: f32| CueSnapshot {
            energy: 1.0,
            reactivity: 1.0,
            tension_bank: 0.9,
            uv_level,
            ..Default::default()
        };

        // High-tension groove, strobe lane gated dark by the show brain -> the
        // pods' white dimmer is 0, so BOTH par UV and pod UV breathe.
        let brain = brain_all_lanes_dark();
        let frame = render_dmx_frame(&tense(0.5), &patch, &profiles, Some(&brain));
        assert_eq!(frame.data[20], 0, "gated strobe lane -> ch21 white dark");
        assert!(frame.data[3] > 0, "par 1 UV breathes with tension (ch4)");
        assert!(frame.data[8] > 0, "par 2 UV breathes with tension (ch9)");
        assert!(frame.data[21] > 0, "pod UV allowed while ch21 is 0 (ch22)");

        // Same music with the lane self-emitting (gate=None) -> ch21 white is
        // lit, so the pod UV must drop out (manual: ch21 & ch22 never together)
        // while the par UV stays.
        let frame = render_dmx_frame(&tense(0.5), &patch, &profiles, None);
        assert!(frame.data[20] > 0, "white dimmer lit (ch21)");
        assert_eq!(frame.data[21], 0, "pod UV defers to white (ch22)");
        assert!(frame.data[3] > 0, "par UV unaffected by the pod exclusion (ch4)");

        // Operator UV at zero -> the whole layer is off.
        let frame = render_dmx_frame(&tense(0.0), &patch, &profiles, Some(&brain));
        assert_eq!(frame.data[3], 0, "uv_level 0 -> par UV off (ch4)");
        assert_eq!(frame.data[8], 0, "uv_level 0 -> par UV off (ch9)");
        assert_eq!(frame.data[21], 0, "uv_level 0 -> pod UV off (ch22)");
    }

    #[test]
    fn gigbar_hardware_pass_respects_mute() {
        // The pass reads SNAPSHOT state, so a muted bar must not strobe,
        // RGB-punch, UV-glow, or texture the laser off that state alone.
        let (patch, mut profiles, profile) = gigbar_patch(1);
        profiles.insert("bar".into(), profile);
        let mut overrides = FixtureOverrides::default();
        overrides.set(
            "bar",
            FixtureOverride { intensity: 1.0, mute: true, solo: false, ..Default::default() },
        );
        let snapshot = CueSnapshot {
            energy: 1.0,
            reactivity: 1.0,
            trigger_strobe: true,
            drop_impact: 0.9,
            tension_bank: 1.0,
            uv_level: 1.0,
            ..Default::default()
        };
        let mut reports = HashMap::new();
        let frame = render_dmx_frame_with_cues(
            &snapshot,
            &patch,
            &profiles,
            None,
            &[],
            &mut reports,
            &overrides,
        );
        assert_eq!(frame.data[19], 0, "muted: no strobe program (ch20)");
        assert_eq!(frame.data[4], 127, "muted: par rate stays at pass-through (ch5)");
        assert_eq!(frame.data[9], 127, "muted: par rate stays at pass-through (ch10)");
        assert_eq!(frame.data[3], 0, "muted: no par UV (ch4)");
        assert_eq!(frame.data[21], 0, "muted: no pod UV (ch22)");
        assert_ne!(frame.data[16], 247, "muted: no laser alternate override (ch17)");
    }

    #[test]
    fn gigbar_uv_mode_is_blacklight_only() {
        // UV mode: ch4/9/22 lit at uv_level × GB (no tension ramp, no bar
        // energy gate), ch20 parked, ch21 dark (exclusion holds trivially),
        // ch5/10 pinned at the RGB pass-through, everything else dark.
        let (patch, mut profiles, profile) = gigbar_patch(1);
        profiles.insert("bar".into(), profile);
        let snapshot = CueSnapshot {
            energy: 1.0,
            reactivity: 1.0,
            trigger_uv: true,
            uv_level: 0.8,
            ..Default::default()
        };
        let frame = render_dmx_frame(&snapshot, &patch, &profiles, None);
        let uv_byte = (0.8f32 * 255.0).round() as u8;
        assert_eq!(frame.data[3], uv_byte, "par 1 UV lit (ch4)");
        assert_eq!(frame.data[8], uv_byte, "par 2 UV lit (ch9)");
        assert_eq!(frame.data[21], uv_byte, "pod UV lit (ch22)");
        assert_eq!(frame.data[20], 0, "ch21 white dimmer dark");
        assert_eq!(frame.data[19], 0, "ch20 strobe engine parked");
        assert_eq!(frame.data[4], 127, "par 1 rate at RGB pass-through (ch5)");
        assert_eq!(frame.data[9], 127, "par 2 rate at RGB pass-through (ch10)");
        // Everything else dark.
        assert_eq!(frame.data[0], 0, "par 1 red dark (ch1)");
        assert_eq!(frame.data[10], 0, "derby 1 colour forced off (ch11)");
        assert_eq!(frame.data[13], 0, "derby 2 colour forced off (ch14)");
        assert_eq!(frame.data[16], 0, "laser dark (ch17)");
        assert_eq!(frame.data[25], 0, "MH1 dimmer dark (ch26)");
        assert_eq!(frame.data[31], 0, "MH2 dimmer dark (ch32)");

        // ...and the manual trigger survives silence — no bar energy gate on
        // the UV mode drive (the rest of the bar stays collapsed).
        let quiet = CueSnapshot {
            energy: 0.0,
            trigger_uv: true,
            uv_level: 0.8,
            ..Default::default()
        };
        let frame = render_dmx_frame(&quiet, &patch, &profiles, None);
        assert_eq!(frame.data[3], uv_byte, "UV mode lights even in silence (ch4)");
        assert_eq!(frame.data[21], uv_byte, "pod UV lights even in silence (ch22)");
    }

    #[test]
    fn gigbar_uv_mode_suppresses_strobe_events() {
        // A rig strobe moment during UV mode must NOT run the program engine
        // or the par strobe bands — UV mode owns the look until released.
        let (patch, mut profiles, profile) = gigbar_patch(1);
        profiles.insert("bar".into(), profile);
        let snapshot = CueSnapshot {
            energy: 1.0,
            reactivity: 1.0,
            trigger_uv: true,
            trigger_strobe: true,
            drop_impact: 0.9,
            uv_level: 0.8,
            ..Default::default()
        };
        let frame = render_dmx_frame(&snapshot, &patch, &profiles, None);
        assert_eq!(frame.data[19], 0, "no strobe program during UV mode (ch20)");
        assert_eq!(frame.data[4], 127, "par rate stays at pass-through (ch5)");
        assert_ne!(frame.data[16], 247, "no laser alternate texture during UV mode (ch17)");
        assert!(frame.data[3] > 0, "par UV stays lit through the event (ch4)");
        assert!(frame.data[21] > 0, "pod UV stays lit through the event (ch22)");
    }

    #[test]
    fn gigbar_blinder_beats_uv_mode() {
        // Blinder hold > UV mode: white blast wins while held (UV resumes on
        // release — stateless per-frame precedence).
        let (patch, mut profiles, profile) = gigbar_patch(1);
        profiles.insert("bar".into(), profile);
        let snapshot = CueSnapshot {
            energy: 1.0,
            reactivity: 1.0,
            trigger_uv: true,
            auto_blinder_active: true,
            uv_level: 1.0,
            ..Default::default()
        };
        let frame = render_dmx_frame(&snapshot, &patch, &profiles, None);
        assert!(frame.data[20] > 0, "blinder: ch21 white dimmer blasts");
        assert_eq!(frame.data[4], 252, "blinder: par 1 full-power RGB (ch5)");
        assert_eq!(frame.data[9], 252, "blinder: par 2 full-power RGB (ch10)");
        assert_eq!(frame.data[3], 0, "blinder: par UV off (ch4)");
        assert_eq!(frame.data[21], 0, "blinder: pod UV off (ch22)");
    }

    #[test]
    fn gigbar_never_writes_sound_active_bands() {
        // Sweep snapshot states (energy × drop × look × trigger combos) and
        // prove the unit's sound-active bands are NEVER selected: ch20 210-255
        // and ch5/ch10 240-249 — REACT is the brain, the unit never free-runs.
        // Also holds the ch21/ch22 hardware mutual exclusion across the sweep.
        let (patch, mut profiles, profile) = gigbar_patch(1);
        profiles.insert("bar".into(), profile);
        for energy in [0.0_f32, 0.05, 0.3, 0.6, 1.0] {
            for drop_impact in [0.0_f32, 0.5, 0.59, 0.6, 0.95] {
                for look_index in 0..12u32 {
                    for (blinder, strobe, overdrive) in [
                        (false, false, false),
                        (false, true, false),
                        (false, false, true),
                        (true, false, false),
                        (true, true, true),
                    ] {
                        let snapshot = CueSnapshot {
                            energy,
                            reactivity: 1.0,
                            drop_impact,
                            look_index,
                            auto_blinder_active: blinder,
                            trigger_strobe: strobe,
                            overdrive_strobe_active: overdrive,
                            tension_bank: drop_impact,
                            uv_level: 1.0,
                            ..Default::default()
                        };
                        let frame = render_dmx_frame(&snapshot, &patch, &profiles, None);
                        let ch20 = frame.data[19];
                        assert!(
                            !(210..=255).contains(&ch20),
                            "ch20 sound band hit: {ch20} (e={energy} d={drop_impact} l={look_index})"
                        );
                        for rate in [frame.data[4], frame.data[9]] {
                            assert!(
                                !(240..=249).contains(&rate),
                                "par rate sound band hit: {rate} (e={energy} d={drop_impact})"
                            );
                        }
                        assert!(
                            frame.data[20] == 0 || frame.data[21] == 0,
                            "ch21/ch22 mutual exclusion violated (white={}, uv={})",
                            frame.data[20],
                            frame.data[21]
                        );
                    }
                }
            }
        }
    }
}

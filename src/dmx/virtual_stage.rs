//! Virtual stage backend: retains per-frame fixture intent for the operator preview.
//!
//! The preview and the physical DMX mapping consume the same `FixtureIntentState`
//! — one lighting model, one oscillator. This backend recomputes that intent from
//! the lighting cue each frame and holds it for the egui renderer to draw.

use crate::emerge::PhaseState;
use crate::emerge::{
    render_fixture_intent, render_fixture_intent_aimed, render_fixture_intent_placed,
    FixtureIntentState, StageLayout,
};
use crate::emerge::contracts::segments_for_profile;
use super::mapping::{
    apply_global_overrides, apply_per_fixture_overrides, bar_energy_gate, expand_sections,
    is_gigbar_35, refine_with_profile, MoverRanking, ProfileMap,
};
use crate::emerge::OscillatorFrameSnapshot;
use crate::emerge::ShowBrainFrame;
use crate::emerge::contracts::{DmxPatch, FixtureOverrides, FixturePatch, FixtureRole};
use crate::hue::CueSnapshot;

/// UV mode preview: a fixture whose patch carries a UV channel glows a dim
/// violet (the blacklight look) instead of going fully dark — `intent.uv` is
/// only ever non-zero in UV mode (see `apply_global_overrides`), so this is a
/// no-op the rest of the time. RGB ~ (0.45, 0.0, 0.8) × uv keeps the preview
/// honest with the wire: only UV-capable emitters lit, everything else dark.
fn apply_uv_preview(intent: &mut FixtureIntentState) {
    if intent.uv > 0.0 {
        intent.red = 0.45 * intent.uv;
        intent.green = 0.0;
        intent.blue = 0.8 * intent.uv;
        intent.white = 0.0;
        intent.dimmer = intent.uv;
    }
}

#[derive(Clone, Debug)]
pub struct VirtualStageBackend {
    enabled: bool,
    patch: DmxPatch,
    /// Fixtures to DRAW — usually 1:1 with `patch.fixtures`, but a multi-section
    /// bar (GigBAR) expands into one glyph per section, each positioned along the
    /// bar with its own `visual_kind`. These are draw-only synthetic fixtures.
    glyph_fixtures: Vec<FixturePatch>,
    /// Intent per glyph, index-aligned with `glyph_fixtures`.
    glyph_intents: Vec<FixtureIntentState>,
    /// For each glyph, the index of the EDITABLE `patch.fixtures` it belongs to —
    /// so a click on any GigBAR section selects the one bar, and the selection
    /// ring lights every section.
    glyph_parents: Vec<usize>,
}

impl VirtualStageBackend {
    pub fn new(enabled: bool, patch: DmxPatch) -> Self {
        Self {
            enabled,
            patch,
            glyph_fixtures: Vec::new(),
            glyph_intents: Vec::new(),
            glyph_parents: Vec::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Runtime enable/disable. Disabling clears retained state so a later
    /// re-enable does not surface stale fixtures.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.glyph_fixtures.clear();
            self.glyph_intents.clear();
            self.glyph_parents.clear();
        }
    }

    /// Sync the patch the preview renders from. Called each frame with the LIVE
    /// active patch so operator drags (which mutate it) show immediately — without
    /// this the preview keeps rendering the snapshot taken at stage-load, and a
    /// moved fixture only appears after an app restart.
    pub fn set_patch(&mut self, patch: DmxPatch) {
        self.patch = patch;
    }

    /// Intent per drawn glyph, index-aligned with [`Self::glyph_fixtures`].
    pub fn latest_intents(&self) -> &[FixtureIntentState] {
        &self.glyph_intents
    }

    /// The fixtures to draw (a bar expands into one glyph per section).
    pub fn glyph_fixtures(&self) -> &[FixturePatch] {
        &self.glyph_fixtures
    }

    /// Editable-fixture index each glyph belongs to (for click/selection).
    pub fn glyph_parents(&self) -> &[usize] {
        &self.glyph_parents
    }

    /// Recompute every fixture's intent from the lighting cue. Blackout is
    /// applied here — `render_fixture_intent` is deliberately blackout-free, so
    /// this backend is the preview's "frame layer" (mirrors `render_dmx_frame`).
    /// Global operator overrides (blinder, global brightness — which carries the
    /// silence master) are applied at the same place the DMX pipeline applies
    /// them so the on-screen rig matches the wire (slider effect, silence settle).
    /// A multi-section bar expands into per-section glyphs exactly as the wire
    /// expands it into per-section channels (`profiles` resolves the bar).
    pub fn update(
        &mut self,
        snapshot: &CueSnapshot,
        gate: Option<&ShowBrainFrame>,
        overrides: &FixtureOverrides,
        profiles: &ProfileMap,
    ) {
        if !self.enabled {
            return;
        }
        // Mirror the wire's blackout precedence (mapping::is_blackout): manual
        // BLACKOUT always wins, but the CueEngine's autonomous silence
        // blackout YIELDS to a held manual override — BLINDER/STROBE/UV must
        // fire in silence on the preview exactly as they do on the wire.
        let blackout = snapshot.trigger_blackout
            || (snapshot.blackout_active && !snapshot.manual_light_override());
        // Rig-wide solo, computed over live fixtures (mirrors the DMX path).
        let any_solo = overrides.any_solo(&self.patch);
        // COM-499 Phase A: tempo-lock the preview oscillators to the real show
        // clock when resolved (keeps the on-screen rig honest with the wire),
        // falling back to the 122-BPM `from_cue` for non-live frames.
        let osc = match gate {
            Some(b) => {
                OscillatorFrameSnapshot::from_clock(snapshot, PhaseState::from_show_clock(b.clock))
            }
            None => OscillatorFrameSnapshot::from_cue(snapshot),
        };
        // Emerge field -> autonomous movers (preview parity with the DMX path):
        // the SAME ranking the wire uses, so the heads take the same ports,
        // sweep groups and figure slots on-screen as they do on the rig, and
        // heads sharing an address level from the one aim the wire uses.
        let movers = MoverRanking::new(self.patch.fixtures.iter());
        // And the same stage layout, so a scene's chases and sweeps cross the
        // on-screen rig where they cross the real one.
        let layout = snapshot.scene.directed.then(|| StageLayout::new(self.patch.fixtures.iter()));
        // Same bar gate the wire applies (collapse the dense bar in quiet).
        // A held manual override (BLINDER/STROBE/UV) bypasses it exactly as the
        // wire does (mapping.rs render path) — the operator pressed the button,
        // so the preview bar must fire with or without music instead of
        // collapsing dark in a silent break.
        let bar_gain = if snapshot.manual_light_override() {
            1.0
        } else {
            bar_energy_gate(snapshot.energy)
        };

        self.glyph_fixtures.clear();
        self.glyph_intents.clear();
        self.glyph_parents.clear();

        for (i, fixture) in self.patch.fixtures.iter().enumerate() {
            if let Some(sections) = expand_sections(fixture, profiles.get(&fixture.id)) {
                // Software-strobe parity with the wire: only the self-managed
                // 35ch GigBAR (whose hardware pass owns its strobe pods) opts
                // out; the 17ch and any other sectioned bar flash on the shared
                // wall-clock `snapshot.strobe_on` exactly like the wire does.
                let self_managed = profiles.get(&fixture.id).map_or(false, is_gigbar_35);
                // Collapse the per-section labels into ONE bar label near the
                // center, so the rig reads as a single named fixture (not 8).
                let label_idx = sections.len() / 2;
                for (sidx, sec) in sections.iter().enumerate() {
                    let intent = if blackout {
                        FixtureIntentState::default()
                    } else {
                        let mut it = render_fixture_intent(
                            snapshot, &sec.fixture, &osc, gate, sec.mover_ordinal, sec.mover_count,
                        );
                        apply_global_overrides(&mut it, &sec.fixture, snapshot, !self_managed);
                        apply_per_fixture_overrides(&mut it, &sec.fixture, overrides, any_solo);
                        if sec.fold_dimmer_into_rgb {
                            it.red *= it.dimmer;
                            it.green *= it.dimmer;
                            it.blue *= it.dimmer;
                            it.white *= it.dimmer;
                        }
                        it.dimmer *= bar_gain;
                        it.red *= bar_gain;
                        it.green *= bar_gain;
                        it.blue *= bar_gain;
                        it.white *= bar_gain;
                        it.strobe *= bar_gain;
                        // After the gates: UV is a manual trigger and must not
                        // collapse with the bar energy (mirrors the wire).
                        apply_uv_preview(&mut it);
                        it
                    };
                    // Draw glyph: blank id so draw_fixture dispatches by the
                    // section's visual_kind (a real id would resolve the combined
                    // bar profile and draw every section as one housing). Only the
                    // center section carries the bar's name → one label, not 8.
                    let mut glyph = sec.fixture.clone();
                    glyph.id = String::new();
                    glyph.name = if sidx == label_idx { fixture.name.clone() } else { String::new() };
                    self.glyph_fixtures.push(glyph);
                    self.glyph_intents.push(intent);
                    self.glyph_parents.push(i);
                }
                continue;
            }

            let intent = if blackout {
                FixtureIntentState::default()
            } else {
                let (mover_ordinal, level_aim) = movers.place(fixture);
                let profile = profiles.get(&fixture.id);
                let mut it = match &layout {
                    Some(layout) => {
                        // A pixel fixture's cells are drawn off the wire; its
                        // own intent must be the one the wire computed.
                        let cells = profile
                            .map_or(0, |p| segments_for_profile(p, fixture.start_address).len());
                        render_fixture_intent_placed(
                            snapshot,
                            fixture,
                            &osc,
                            gate,
                            mover_ordinal,
                            movers.count(),
                            level_aim,
                            &layout.place(fixture, cells),
                        )
                    }
                    None => render_fixture_intent_aimed(
                        snapshot, fixture, &osc, gate, mover_ordinal, movers.count(), level_aim,
                    ),
                };
                // Same wheel resolution as the wire, so the preview shows the
                // gel the rig is actually on.
                refine_with_profile(&mut it, fixture, profile);
                apply_global_overrides(&mut it, fixture, snapshot, true);
                apply_per_fixture_overrides(&mut it, fixture, overrides, any_solo);
                apply_uv_preview(&mut it);
                it
            };
            self.glyph_fixtures.push(fixture.clone());
            self.glyph_intents.push(intent);
            self.glyph_parents.push(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_backend_has_no_side_effects() {
        let patch = DmxPatch::demo_virtual_stage(0, 1);
        let mut backend = VirtualStageBackend::new(false, patch);
        backend.update(&CueSnapshot { energy: 1.0, ..Default::default() }, None, &FixtureOverrides::default(), &ProfileMap::new());
        assert!(backend.latest_intents().is_empty());
    }

    #[test]
    fn update_produces_one_intent_per_patched_fixture() {
        let patch = DmxPatch::demo_virtual_stage(0, 1);
        let mut backend = VirtualStageBackend::new(true, patch.clone());
        backend.update(&CueSnapshot::default(), None, &FixtureOverrides::default(), &ProfileMap::new());
        assert_eq!(backend.latest_intents().len(), patch.fixtures.len());
    }

    #[test]
    fn update_produces_visible_demo_intents() {
        let patch = DmxPatch::demo_virtual_stage(0, 1);
        let mut backend = VirtualStageBackend::new(true, patch.clone());
        // frame_time > 0: the mover tilt LFO is a triangle whose phase-0 value
        // is its trough, so t=0 is not representative of a running show.
        backend.update(&CueSnapshot {
            energy: 1.0,
            energy_slow: 1.0,
            reactivity: 1.0,
            tension: 0.9,
            dominant_pc: 5,
            strobe_active: true,
            frame_time: 1.0,
            ..Default::default()
        }, None, &FixtureOverrides::default(), &ProfileMap::new());
        let by_id = |id: &str| {
            patch
                .fixtures
                .iter()
                .zip(backend.latest_intents())
                .find(|(fx, _)| fx.id == id)
                .map(|(_, intent)| *intent)
                .unwrap()
        };
        let strobe = by_id("shehds-strobe-1");
        let beam = by_id("shehds-7r-beam-1");
        let laser = by_id("scorpion-dual-rgb-1");
        assert!(strobe.strobe > 0.0);
        assert!(beam.pan >= 0.0);
        assert!(laser.laser_macro > 0);
    }

    #[test]
    fn update_blackout_zeros_every_intent() {
        let patch = DmxPatch::demo_virtual_stage(0, 1);
        let mut backend = VirtualStageBackend::new(true, patch);
        backend.update(&CueSnapshot { energy: 1.0, trigger_blackout: true, ..Default::default() }, None, &FixtureOverrides::default(), &ProfileMap::new());
        assert!(backend
            .latest_intents()
            .iter()
            .all(|intent| intent.dimmer == 0.0 && intent.strobe == 0.0));
    }

    /// Minimal GigBAR fixture + profile for the preview tests — only the
    /// fields the expander and override paths read.
    fn gigbar_preview_setup(channel_count: u16) -> (VirtualStageBackend, ProfileMap) {
        use crate::emerge::contracts::{
            ChannelDefinition, ChannelMap, DmxAttribute, FixtureHousing, FixtureKind,
            FixtureProfile, StageVec3, StageVisualKind,
        };
        let channels = (1..=channel_count)
            .map(|off| ChannelDefinition {
                attribute: DmxAttribute::Dimmer,
                channel_offset: off,
                default_value: if off == 5 || off == 10 { 127 } else { 0 },
                blackout_value: 0,
                detail: None,
                source_attribute: None,
            })
            .collect();
        let profile = FixtureProfile {
            id: "gigbar".into(), manufacturer: "Chauvet DJ".into(), model: "GigBAR Move".into(),
            mode: format!("{}", channel_count), kind: FixtureKind::Unknown, housing: FixtureHousing::Generic,
            channel_count, channels, capabilities: vec![],
        };
        let fixture = FixturePatch {
            footprint: 0,
            screen: None, id: "bar".into(), name: "GigBAR".into(), universe: 0, start_address: 1,
            profile_id: "gigbar".into(), mode: format!("{}", channel_count),
            position: StageVec3::zero(), rotation: StageVec3::zero(), scale: StageVec3::one(),
            role: FixtureRole::Accent, visual_kind: StageVisualKind::Beam, tags: vec![],
            channels: ChannelMap::default(),
        };
        let patch = DmxPatch { universe: 0, trusses: vec![], venue: None, fixtures: vec![fixture] };
        let mut profiles = ProfileMap::new();
        profiles.insert("bar".into(), profile);
        (VirtualStageBackend::new(true, patch), profiles)
    }

    #[test]
    fn gigbar_expands_into_eight_preview_glyphs() {
        use crate::emerge::contracts::StageVisualKind;
        let (mut backend, profiles) = gigbar_preview_setup(35);
        backend.update(
            &CueSnapshot { energy: 1.0, reactivity: 1.0, frame_time: 1.0, ..Default::default() },
            None,
            &FixtureOverrides::default(),
            &profiles,
        );

        // One bar -> 8 section glyphs, all parented to the single editable fixture.
        assert_eq!(backend.glyph_fixtures().len(), 8);
        assert_eq!(backend.latest_intents().len(), 8);
        assert!(backend.glyph_parents().iter().all(|&p| p == 0));
        // Sections are spread along X and carry distinct kinds; ids blanked so
        // draw_fixture dispatches by visual_kind, not the combined-bar profile.
        let xs: Vec<f32> = backend.glyph_fixtures().iter().map(|f| f.position.x).collect();
        assert!(xs.windows(2).any(|w| (w[0] - w[1]).abs() > 0.01), "sections spread out");
        assert!(
            backend.glyph_fixtures().iter().any(|f| f.visual_kind == StageVisualKind::Laser),
            "a laser section glyph exists"
        );
        assert!(backend.glyph_fixtures().iter().all(|f| f.id.is_empty()));
        // Labels collapse to ONE bar label, not eight section names.
        assert_eq!(
            backend.glyph_fixtures().iter().filter(|f| !f.name.is_empty()).count(),
            1,
            "exactly one section glyph carries the bar label"
        );
    }

    #[test]
    fn preview_manual_override_beats_silence_blackout() {
        use crate::emerge::contracts::StageVisualKind;
        // Mirror of the wire rule: the CueEngine's autonomous silence
        // blackout must not zero the preview glyphs while a manual override
        // is held — but a manual BLACKOUT still wins.
        let (mut backend, profiles) = gigbar_preview_setup(35);
        backend.update(
            &CueSnapshot { blackout_active: true, trigger_blinder: true, ..Default::default() },
            None,
            &FixtureOverrides::default(),
            &profiles,
        );
        let lit = backend
            .glyph_fixtures()
            .iter()
            .zip(backend.latest_intents())
            .filter(|(f, _)| f.visual_kind != StageVisualKind::Laser)
            .all(|(_, i)| i.dimmer > 0.99);
        assert!(lit, "held BLINDER must light preview glyphs through the silence blackout");

        backend.update(
            &CueSnapshot { trigger_blackout: true, trigger_blinder: true, ..Default::default() },
            None,
            &FixtureOverrides::default(),
            &profiles,
        );
        assert!(
            backend.latest_intents().iter().all(|i| i.dimmer == 0.0),
            "manual BLACKOUT must still win over a held BLINDER in the preview"
        );
    }

    #[test]
    fn preview_bar_gate_bypassed_under_manual_override() {
        use crate::emerge::contracts::StageVisualKind;
        // A held BLINDER in SILENCE (energy 0) must light the preview bar —
        // the bar-energy gate is bypassed under a manual override exactly as
        // the wire does. This bypass was lost from the working tree once
        // before; this test makes a silent revert impossible.
        let (mut backend, profiles) = gigbar_preview_setup(35);
        backend.update(
            &CueSnapshot { energy: 0.0, trigger_blinder: true, ..Default::default() },
            None,
            &FixtureOverrides::default(),
            &profiles,
        );
        let lit = backend
            .glyph_fixtures()
            .iter()
            .zip(backend.latest_intents())
            .filter(|(f, _)| f.visual_kind != StageVisualKind::Laser)
            .all(|(_, i)| i.dimmer > 0.99 && i.white > 0.99);
        assert!(lit, "held BLINDER must white-flash every non-laser section glyph in silence");
    }

    #[test]
    fn preview_17ch_software_strobes_but_35ch_self_manages() {
        use crate::emerge::contracts::StageVisualKind;
        // Preview parity with the wire's `!self_managed` decision: the 17ch
        // (no hardware pass) software-strobes its section glyphs on the
        // shared `strobe_on` wave; the 35ch leaves them to its hardware pass.
        let strobe_snap = |on: bool| CueSnapshot {
            energy: 0.0,
            trigger_strobe: true,
            strobe_on: on,
            ..Default::default()
        };
        let pars_dimmer = |backend: &VirtualStageBackend| {
            backend
                .glyph_fixtures()
                .iter()
                .zip(backend.latest_intents())
                .find(|(f, _)| f.visual_kind == StageVisualKind::Wash)
                .map(|(_, i)| i.dimmer)
                .expect("a wash (pars) section glyph exists")
        };

        let (mut b17, p17) = gigbar_preview_setup(17);
        b17.update(&strobe_snap(true), None, &FixtureOverrides::default(), &p17);
        assert!(pars_dimmer(&b17) > 0.99, "17ch pars glyph flashes on the ON phase");
        b17.update(&strobe_snap(false), None, &FixtureOverrides::default(), &p17);
        assert_eq!(pars_dimmer(&b17), 0.0, "17ch pars glyph dark on the OFF phase");

        let (mut b35, p35) = gigbar_preview_setup(35);
        b35.update(&strobe_snap(true), None, &FixtureOverrides::default(), &p35);
        assert!(
            pars_dimmer(&b35) < 0.9,
            "35ch pars glyph is NOT software-white-flashed (hardware pass owns its pods)"
        );
    }

    #[test]
    fn uv_mode_preview_glows_violet_on_uv_fixtures_and_darkens_the_rest() {
        // UV mode honesty: a fixture WITH a UV channel glows a dim violet at
        // the operator's uv_level; everything else goes dark — matching what
        // the wire does (only UV-capable emitters lit).
        let mut patch = DmxPatch::demo_rgbw_wash(0, 1);
        let mut uvw = FixturePatch::demo_rgbw_wash("uv-wash", 0, 6);
        uvw.channels.uv = Some(11);
        patch.fixtures.push(uvw);
        let mut backend = VirtualStageBackend::new(true, patch);
        backend.update(
            &CueSnapshot {
                energy: 1.0,
                reactivity: 1.0,
                trigger_uv: true,
                uv_level: 0.8,
                frame_time: 1.0,
                ..Default::default()
            },
            None,
            &FixtureOverrides::default(),
            &ProfileMap::new(),
        );
        let plain = backend.latest_intents()[0];
        let uv = backend.latest_intents()[1];
        assert_eq!(plain.dimmer, 0.0, "non-UV fixture dark in UV mode");
        assert_eq!(plain.red + plain.green + plain.blue + plain.white, 0.0);
        // UV fixture: violet glow scaled by uv_level (blue-heavy, no green).
        assert!((uv.dimmer - 0.8).abs() < 1e-4, "glyph brightness = uv drive, got {}", uv.dimmer);
        assert!((uv.red - 0.45 * 0.8).abs() < 1e-4, "violet red component, got {}", uv.red);
        assert_eq!(uv.green, 0.0);
        assert!((uv.blue - 0.8 * 0.8).abs() < 1e-4, "violet blue component, got {}", uv.blue);
        assert_eq!(uv.white, 0.0);

        // Blackout still wins over UV mode in the preview too.
        backend.update(
            &CueSnapshot { trigger_uv: true, trigger_blackout: true, uv_level: 0.8, ..Default::default() },
            None,
            &FixtureOverrides::default(),
            &ProfileMap::new(),
        );
        assert!(backend.latest_intents().iter().all(|i| i.dimmer == 0.0 && i.uv == 0.0));
    }

    #[test]
    fn set_patch_moves_glyphs_without_restart() {
        // Regression: an operator drag mutates the active patch; set_patch + update
        // must reflect the new position on the next frame (the bug was the preview
        // rendering a stale stage-load snapshot until an app restart).
        let snap = || CueSnapshot { energy: 1.0, reactivity: 1.0, frame_time: 1.0, ..Default::default() };
        let mut patch = DmxPatch::demo_rgbw_wash(0, 1);
        let mut backend = VirtualStageBackend::new(true, patch.clone());
        backend.update(&snap(), None, &FixtureOverrides::default(), &ProfileMap::new());
        let x0 = backend.glyph_fixtures()[0].position.x;

        // Move the fixture (as a drag would) and re-sync the preview.
        patch.fixtures[0].position.x = x0 + 5.0;
        backend.set_patch(patch);
        backend.update(&snap(), None, &FixtureOverrides::default(), &ProfileMap::new());
        let x1 = backend.glyph_fixtures()[0].position.x;
        assert!((x1 - (x0 + 5.0)).abs() < 1e-3, "glyph follows the moved fixture (x0={x0}, x1={x1})");
    }
}

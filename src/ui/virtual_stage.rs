//! Virtual Stage — a perspective venue preview for the REACT lighting rig.
//!
//! Renders a 3D-projected stage: a receding floor, a large upstage LED wall, the
//! rig trusses (overhead, side towers, downstage floor truss), and the DMX
//! fixtures hung on them. Each fixture is drawn as its real-world type — wash
//! flood, beam, gobo projection, laser fan, strobe flash — driven by live
//! `FixtureIntentState`.
//!
//! ## Visual approach
//!
//! Pure egui `Painter`; no GL, no Bevy. We borrow *techniques* from volumetric
//! renderers, not the engine: beams and light pools are gradient `egui::Mesh`
//! shapes (per-vertex colour, bright core fading to transparent), which read as
//! volumetric scatter and soft glow without a shader.
//!
//! The one thing this can't fake is true additive HDR bloom — egui composites
//! alpha-over, so crossing beams layer rather than sum. A real bloom pass would
//! need a GL post-process step; that is deliberately out of scope here and would
//! be a separate, isolated change. For an operator preview, gradient meshes carry
//! the look; the real rig provides the actual photons.
//!
//! Operator-only surface: it does not touch the streamed / NDI / recorded output.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::sync::Arc;

use egui::{Color32, Mesh, Pos2, Rect, Shape, Stroke, Vec2};
use parking_lot::Mutex;

use crate::emerge::{
    renders_as_galaxy_effect, segments_for_profile, ChannelMap, DmxAttribute, DmxFrame, DmxPatch,
    FixtureHousing, FixtureIntentState, FixtureKind, FixtureOverride, FixtureOverrides,
    FixturePatch, FixtureProfile, SegmentChannels, SegmentPattern, StageVec3, StageViewPreset,
    StageViewPresetKind, StageVisualKind, Truss, TrussAxis, TrussKind, VenueKind,
    MOVER_PAN_WINDOW_DEG, MOVER_TILT_WINDOW_DEG,
};
use crate::render::RenderPipeline;

// ---------------------------------------------------------------------------
// minimal 3D math
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct V3 {
    x: f32,
    y: f32,
    z: f32,
}

impl V3 {
    fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    fn of(s: StageVec3) -> Self {
        Self { x: s.x, y: s.y, z: s.z }
    }
    fn sub(self, o: V3) -> V3 {
        V3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    fn add(self, o: V3) -> V3 {
        V3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
    fn lerp(self, o: V3, t: f32) -> V3 {
        V3::new(
            self.x + (o.x - self.x) * t,
            self.y + (o.y - self.y) * t,
            self.z + (o.z - self.z) * t,
        )
    }
    fn scale(self, k: f32) -> V3 {
        V3::new(self.x * k, self.y * k, self.z * k)
    }
    fn dot(self, o: V3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    fn cross(self, o: V3) -> V3 {
        V3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    fn norm(self) -> V3 {
        let l = self.dot(self).sqrt();
        if l > 1e-6 {
            self.scale(1.0 / l)
        } else {
            self
        }
    }
}

// ---------------------------------------------------------------------------
// camera
// ---------------------------------------------------------------------------

/// A pinhole camera built from a `StageViewPreset`. World axes: +X stage-right,
/// +Y upstage, +Z up.
struct StageCamera {
    eye: V3,
    right: V3,
    up: V3,
    fwd: V3,
    focal: f32,
}

impl StageCamera {
    /// Orbit camera around the preset's look_at target.
    /// - `zoom` scales focal length (1.0 = preset 55° FOV, >1 zoom in, <1 out).
    /// - `yaw_deg` rotates the eye around the vertical (world +Z) axis. 0 =
    ///   the preset's FOH-front angle; +90 swings to stage-right; ±180 to
    ///   behind the wall; -90 to stage-left.
    /// - `pitch_deg` lifts (+) or drops (-) the eye out of the preset's
    ///   elevation. +60 lands a near-overhead bird's eye.
    ///
    /// True orbit: radius (distance to look_at) is preserved, look_at stays
    /// fixed, so no slider re-shuffles the relative fixture layout.
    fn from_preset(p: &StageViewPreset, zoom: f32, yaw_deg: f32, pitch_deg: f32) -> Self {
        let target = V3::of(p.look_at);
        let preset_offset = V3::of(p.camera_position).sub(target);
        let radius = preset_offset.dot(preset_offset).sqrt().max(0.5);

        // Decompose the preset offset into spherical coords around target so
        // (yaw=0, pitch=0) reproduces the preset exactly. base_azimuth is the
        // angle of the eye→target ray in the X-Y plane; base_elevation is its
        // elevation above the horizontal plane.
        let base_azimuth = preset_offset.x.atan2(preset_offset.y);
        let base_elevation = (preset_offset.z / radius).clamp(-1.0, 1.0).asin();

        // Clamp pitch so the camera cannot flip past vertical (would degenerate
        // the right vector when fwd lands on world-up).
        let final_azimuth = base_azimuth + yaw_deg.to_radians();
        let final_elevation = (base_elevation + pitch_deg.to_radians())
            .clamp(-80f32.to_radians(), 80f32.to_radians());

        let cos_el = final_elevation.cos();
        let new_offset = V3::new(
            radius * cos_el * final_azimuth.sin(),
            radius * cos_el * final_azimuth.cos(),
            radius * final_elevation.sin(),
        );
        let eye = target.add(new_offset);

        let fwd = target.sub(eye).norm();
        let right = fwd.cross(V3::new(0.0, 0.0, 1.0)).norm();
        let up = right.cross(fwd).norm();

        let focal_base = 1.0 / (55.0_f32.to_radians() * 0.5).tan();
        let focal = focal_base * zoom.clamp(0.4, 2.5);
        Self { eye, right, up, fwd, focal }
    }

    /// Project a world point into `rect`. Returns the screen position and the
    /// view-space depth (distance along the look direction). `None` when the
    /// point is at or behind the camera plane.
    fn project(&self, w: V3, rect: Rect) -> Option<(Pos2, f32)> {
        let d = w.sub(self.eye);
        let cz = d.dot(self.fwd);
        if cz <= 0.06 {
            return None;
        }
        let aspect = (rect.width() / rect.height().max(1.0)).max(0.1);
        let ndc_x = (d.dot(self.right) / cz) * self.focal / aspect;
        let ndc_y = (d.dot(self.up) / cz) * self.focal;
        Some((
            Pos2::new(
                rect.center().x + ndc_x * rect.width() * 0.5,
                rect.center().y - ndc_y * rect.height() * 0.5,
            ),
            cz,
        ))
    }

    fn screen(&self, w: V3, rect: Rect) -> Option<Pos2> {
        self.project(w, rect).map(|(p, _)| p)
    }

    /// World point under screen position `sp` at view-space depth `depth` —
    /// the inverse of `project` for a known depth. Dragging a fixture at its
    /// own current depth slides it across the screen-facing plane through it,
    /// so the head tracks the cursor in any view.
    fn ray_at_depth(&self, sp: Pos2, rect: Rect, depth: f32) -> V3 {
        let aspect = (rect.width() / rect.height().max(1.0)).max(0.1);
        let ndc_x = (sp.x - rect.center().x) / (rect.width() * 0.5);
        let ndc_y = -(sp.y - rect.center().y) / (rect.height() * 0.5);
        let dir = self
            .fwd
            .add(self.right.scale(ndc_x * aspect / self.focal))
            .add(self.up.scale(ndc_y / self.focal));
        self.eye.add(dir.scale(depth))
    }
}

// ---------------------------------------------------------------------------
// gradient-mesh primitives — the "borrowed from volumetric renderers" toolkit
// ---------------------------------------------------------------------------

/// A soft radial blob: opaque `core` at the centre fading to transparent at the
/// rim. The building block for glow halos, light pools, and strobe flashes.
fn soft_disc(center: Pos2, radius: f32, core: Color32) -> Shape {
    let mut mesh = Mesh::default();
    let rim = Color32::from_rgba_unmultiplied(core.r(), core.g(), core.b(), 0);
    mesh.colored_vertex(center, core);
    const SEG: usize = 28;
    for i in 0..=SEG {
        let a = i as f32 / SEG as f32 * TAU;
        mesh.colored_vertex(center + Vec2::angled(a) * radius, rim);
    }
    for i in 1..=SEG {
        mesh.add_triangle(0, i as u32, i as u32 + 1);
    }
    Shape::mesh(mesh)
}

/// A quad with a per-corner colour — used for gradient light beams and the
/// depth-faded floor. Corners are wound as a loop.
fn grad_quad(corners: [Pos2; 4], colors: [Color32; 4]) -> Shape {
    let mut mesh = Mesh::default();
    for i in 0..4 {
        mesh.colored_vertex(corners[i], colors[i]);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    Shape::mesh(mesh)
}

/// A tapered light beam: a gradient quad from a narrow, bright source to a wide,
/// faded end. `src`/`dst` are screen-space; widths are screen pixels.
fn beam(
    src: Pos2,
    dst: Pos2,
    w_src: f32,
    w_dst: f32,
    color: Color32,
    a_src: f32,
    a_dst: f32,
) -> Option<Shape> {
    let dir = dst - src;
    let len = dir.length();
    if len < 0.5 {
        return None;
    }
    let n = Vec2::new(-dir.y, dir.x) / len;
    let cs = tint(color, a_src);
    let cd = tint(color, a_dst);
    Some(grad_quad(
        [src + n * w_src, dst + n * w_dst, dst - n * w_dst, src - n * w_src],
        [cs, cd, cd, cs],
    ))
}

fn tint(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

fn u8f(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0) as u8
}

fn quad(cam: &StageCamera, rect: Rect, w: [V3; 4]) -> Option<[Pos2; 4]> {
    Some([
        cam.screen(w[0], rect)?,
        cam.screen(w[1], rect)?,
        cam.screen(w[2], rect)?,
        cam.screen(w[3], rect)?,
    ])
}

fn centroid(q: &[Pos2; 4]) -> Pos2 {
    Pos2::new(
        (q[0].x + q[1].x + q[2].x + q[3].x) * 0.25,
        (q[0].y + q[1].y + q[2].y + q[3].y) * 0.25,
    )
}

fn lerp_pos(a: Pos2, b: Pos2, t: f32) -> Pos2 {
    Pos2::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

// ---------------------------------------------------------------------------
// view state
// ---------------------------------------------------------------------------

/// An in-progress fixture drag: the grabbed patch, the view-space depth it was
/// grabbed at (locked, so it slides in a stable plane), and the cursor→head
/// offset so the fixture does not jump to the cursor on grab.
#[derive(Clone, Copy)]
struct FixtureDrag {
    idx: usize,
    depth: f32,
    offset: [f32; 3],
}

/// An in-progress truss drag — same scheme as `FixtureDrag`: the grab depth is
/// locked so the bar slides in a stable plane, and the cursor→center offset
/// keeps it from jumping to the cursor on grab.
#[derive(Clone, Copy)]
struct TrussDrag {
    idx: usize,
    depth: f32,
    offset: [f32; 3],
}

/// An in-progress aim drag: the operator is dragging a fixture's beam endpoint to
/// re-point it. Only the grab depth is locked — the aim (tilt + pan) is recomputed
/// from the cursor each frame and written to `rotation.x` / `rotation.y`.
#[derive(Clone, Copy)]
struct AimDrag {
    idx: usize,
    depth: f32,
}

/// Operator-facing virtual stage. Holds the per-fixture control-panel selection
/// and any in-progress drag; fixture data is passed in each frame from the DMX
/// virtual-stage backend.
#[derive(Default)]
pub struct VirtualStageView {
    /// Patch index of the fixture whose control panel is open, if any.
    selected_fixture: Option<usize>,
    /// Last frame's control-panel screen rect — used to reject stage clicks
    /// that land on the panel rather than a fixture behind it.
    last_panel_rect: Option<Rect>,
    /// The fixture currently being mouse-dragged across the stage, if any.
    dragging: Option<FixtureDrag>,
    /// Smoke build-up level (0 = clear, 1 = full) — eased toward the Fog
    /// trigger so the haze forms from the floor up.
    fog_phase: f32,
    /// Previous frame's time, for the smoke-easing dt.
    last_time: f64,
    /// Patch index of the truss whose control panel is open, if any. Mutually
    /// exclusive with `selected_fixture` — selecting one clears the other.
    selected_truss: Option<usize>,
    /// The truss currently being mouse-dragged across the stage, if any.
    truss_dragging: Option<TrussDrag>,
    /// The fixture whose beam endpoint is being dragged to re-aim it, if any.
    aim_dragging: Option<AimDrag>,
    /// The operator-selected local video bound to a Screen fixture, as
    /// `(fixture_id, file_path)`. Session-scoped for v1 (persistence by fixture
    /// id is a fast-follow). Drives [`screen_decoder`].
    screen_file: Option<(String, String)>,
    /// Async decoder for [`screen_file`] — opened lazily, looped, decoded on its
    /// own thread (so the screen never stalls the render loop). `None` until a
    /// screen video is bound or after a decode failure.
    screen_decoder: Option<crate::video::VideoDecoder>,
    /// Latest decoded screen frame as an egui texture, drawn onto the Screen
    /// fixture's posed quad. `None` until the first frame lands (placeholder shows
    /// until then).
    screen_tex: Option<egui::TextureHandle>,
    /// The path the decoder has already attempted to open (success or failure).
    /// Gates `open()` to at most once per binding so a missing/bad file doesn't
    /// re-probe (a blocking call) on every frame. Cleared on rebind / stage-close
    /// so a fresh binding (or reopen) attempts once.
    screen_open_done: Option<String>,
}

impl VirtualStageView {
    /// Tear down the screen video decoder + texture when the virtual stage is
    /// toggled off. The decoder runs on its own thread, so without this it would
    /// keep decoding in the background while the stage is closed — wasted CPU on
    /// the 24/7 demos. The `screen_file` binding is kept so the video resumes when
    /// the operator reopens the stage. Idempotent.
    pub fn release_video(&mut self) {
        self.screen_decoder = None;
        self.screen_tex = None;
        self.screen_open_done = None;
    }

    /// Draw the virtual stage as a full-window operator overlay. Returns
    /// `false` when the operator dismissed it (Exit / Esc) so the caller can
    /// clear the Lighting toggle. `patches` is mutable: the per-fixture
    /// control panel edits position and mounting aim in place. `overrides` holds
    /// the operator's ephemeral per-fixture live mix (intensity/solo/mute) and is
    /// edited in place by the control panel's LIVE CONTROL section.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        view: StageViewPresetKind,
        zoom: f32,
        yaw_deg: f32,
        pitch_deg: f32,
        pipeline: &Arc<Mutex<Option<RenderPipeline>>>,
        patch: &mut DmxPatch,
        overrides: &mut FixtureOverrides,
        // Draw-list: the glyphs to render (a GigBAR expands into one per section),
        // their intents, and the editable-fixture index each glyph belongs to.
        // `patch` stays the editable/selectable source; these are draw-only.
        glyph_fixtures: &[FixturePatch],
        states: &[FixtureIntentState],
        glyph_parents: &[usize],
        profiles: &std::collections::HashMap<String, FixtureProfile>,
        library_profiles: &[FixtureProfile],
        has_stage: bool,
        dmx_frame: Option<&DmxFrame>,
        fog: bool,
        show_labels: bool,
        venue: &mut VenueKind,
        // `false` for a shared admin template the current login can't edit: the
        // operator may inspect (click a fixture to open its panel) but not move /
        // aim / add fixtures — those edits would be ephemeral (never flushed) and
        // the server 403s them. The local Wedding demo passes `true`.
        editable: bool,
    ) -> bool {
        let screen = ctx.screen_rect();
        let time = ctx.input(|i| i.time);

        // Smoke build-up — the Fog trigger eases fog_phase up; release
        // dissipates it. Time-based so the build rate is frame-rate stable.
        let dt = (time - self.last_time).clamp(0.0, 0.1) as f32;
        self.last_time = time;
        let fog_target = if fog { 1.0 } else { 0.0 };
        let fog_rate = if fog { dt / 3.5 } else { dt / 6.0 };
        self.fog_phase = if self.fog_phase < fog_target {
            (self.fog_phase + fog_rate).min(fog_target)
        } else {
            (self.fog_phase - fog_rate).max(fog_target)
        };
        let fog_phase = self.fog_phase;

        let presets = StageViewPreset::demo_presets();
        let preset = presets
            .iter()
            .find(|p| p.kind == view)
            .cloned()
            .unwrap_or_else(|| presets[0].clone());
        let cam = StageCamera::from_preset(&preset, zoom, yaw_deg, pitch_deg);
        let stage = Rect::from_min_size(
            screen.min + Vec2::new(26.0, 50.0),
            Vec2::new(screen.width() - 52.0, screen.height() - 78.0),
        );

        // Ring the panel-selected fixture and any fixture being dragged.
        let selected = self
            .selected_fixture
            .or(self.dragging.map(|d| d.idx))
            .filter(|&i| i < patch.fixtures.len());
        // The fixture whose attributes panel is open — stays grabbable even
        // under the panel, so it can be dragged with its window open.
        let panel_owner = self.selected_fixture.filter(|&i| i < patch.fixtures.len());
        let prev_panel_rect = self.last_panel_rect;

        let mut exit_clicked = false;
        let mut esc_pressed = false;
        let mut clicked_fixture: Option<usize> = None;
        let mut dragging = self.dragging.filter(|d| d.idx < patch.fixtures.len());
        let mut truss_dragging = self.truss_dragging.filter(|d| d.idx < patch.trusses.len());
        let mut aim_dragging = self.aim_dragging.filter(|d| d.idx < patch.fixtures.len());
        let mut clicked_truss: Option<usize> = None;
        // The truss whose panel is open (also highlighted on the stage).
        let selected_truss = self.selected_truss.filter(|&i| i < patch.trusses.len());

        // ---- Screen video: operator-bound local file -> egui texture --------
        // Drop the binding if its fixture is gone / no longer a screen.
        if let Some((fid, _)) = &self.screen_file {
            if !patch.fixtures.iter().any(|f| &f.id == fid && f.screen.is_some()) {
                self.screen_file = None;
                self.screen_decoder = None;
                self.screen_open_done = None;
            }
        }
        // Open + loop the decoder for the bound file and pull the latest frame
        // into the screen texture. The decoder decodes on its OWN thread, so this
        // never blocks the render loop — the placeholder shows until the first
        // frame lands (anti-jank for the 24/7 demos).
        if let Some((_, path)) = self.screen_file.clone() {
            if crate::video::VideoDecoder::is_available() {
                // Open at most ONCE per binding: only when no decoder is live AND
                // we haven't already attempted this exact path. A failed open
                // marks the path done so it won't re-probe (blocking) every frame;
                // a rebind / stage-close clears the marker to allow a fresh open.
                let open_now = self.screen_decoder.is_none()
                    && self.screen_open_done.as_deref() != Some(path.as_str());
                if open_now {
                    let mut dec = crate::video::VideoDecoder::new();
                    // Force the CPU frame path — the screen reads frames via
                    // get_frame() (the zero-copy IOSurface path fills a different
                    // ring that get_frame never sees, so it'd never play).
                    dec.set_zero_copy(false);
                    let opened = dec.open(&path).is_ok();
                    self.screen_open_done = Some(path.clone());
                    if opened {
                        dec.set_looping(true);
                        dec.play();
                        self.screen_decoder = Some(dec);
                    } else {
                        self.screen_decoder = None;
                    }
                }
                if let Some(dec) = self.screen_decoder.as_mut() {
                    // Guard the exact RGBA size — `from_rgba_unmultiplied` PANICS on
                    // a length mismatch, which would take down the whole app (fatal
                    // for the 24/7 demos). Skip a malformed frame instead.
                    if let Some((bytes, w, h)) = dec.get_frame().filter(|(b, w, h)| {
                        *w > 0 && *h > 0 && (*w as usize) * (*h as usize) * 4 == b.len()
                    }) {
                        // The decoder delivers BGRA (AVFoundation 32BGRA); swap to
                        // RGB for the egui texture. Frames are opaque, so alpha is
                        // dropped. The size guard above makes chunks_exact exact.
                        let pixels: Vec<Color32> = bytes
                            .chunks_exact(4)
                            .map(|px| Color32::from_rgb(px[2], px[1], px[0]))
                            .collect();
                        let img = egui::ColorImage {
                            size: [w as usize, h as usize],
                            pixels,
                        };
                        match self.screen_tex.as_mut() {
                            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                            None => {
                                self.screen_tex = Some(ctx.load_texture(
                                    "vstage_screen",
                                    img,
                                    egui::TextureOptions::LINEAR,
                                ))
                            }
                        }
                    }
                }
            }
        }
        // Owned (fixture_id, texture) for the draw path; selected-screen id for
        // the "Set video" button. Both resolved before the egui closure.
        let screen_video: Option<(String, egui::TextureId)> = self
            .screen_file
            .as_ref()
            .zip(self.screen_tex.as_ref())
            .map(|((fid, _), tex)| (fid.clone(), tex.id()));
        let selected_screen_id: Option<String> = self
            .selected_fixture
            .and_then(|i| patch.fixtures.get(i))
            .filter(|f| f.screen.is_some())
            .map(|f| f.id.clone());
        let mut set_screen_file: Option<(String, String)> = None;

        egui::Area::new(egui::Id::new("virtual_stage_view"))
            .fixed_pos(screen.min)
            // Middle keeps the stage above the main visual but below the
            // controls overlay (Foreground) and the menu toggle (Tooltip), so
            // the operator menu always stays reachable on top of the stage.
            .order(egui::Order::Middle)
            .interactable(true)
            .show(ctx, |ui| {
                let painter = ui.painter_at(screen);

                // backdrop — faint top haze fading down
                painter.add(grad_quad(
                    [
                        screen.left_top(),
                        screen.right_top(),
                        screen.right_bottom(),
                        screen.left_bottom(),
                    ],
                    [
                        Color32::from_rgb(11, 13, 19),
                        Color32::from_rgb(11, 13, 19),
                        Color32::from_rgb(5, 6, 9),
                        Color32::from_rgb(5, 6, 9),
                    ],
                ));

                // header / preset switcher
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(18.0);
                    ui.label(
                        egui::RichText::new("VIRTUAL STAGE")
                            .color(Color32::from_rgb(180, 225, 255))
                            .strong()
                            .size(15.0),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(&preset.label)
                            .color(Color32::from_rgb(150, 180, 210)),
                    );
                    ui.add_space(10.0);
                    // Read-only venue badge — the scene is chosen in the Lighting
                    // menu's Stage dropdown (a wedding is just a stage).
                    ui.label(
                        egui::RichText::new(venue.label())
                            .small()
                            .color(Color32::from_rgb(120, 142, 168)),
                    );
                    ui.add_space(10.0);
                    // Read-only badge for a shared admin template a non-admin picked —
                    // the rig renders but can't be re-arranged here (clone it from the
                    // Stage dropdown to get an editable "My …" copy).
                    if has_stage && !editable {
                        ui.label(
                            egui::RichText::new("\u{1F512} admin-managed \u{00B7} read-only")
                                .small()
                                .color(Color32::from_rgb(190, 150, 110)),
                        );
                        ui.add_space(10.0);
                    }
                    if has_stage && editable {
                        ui.menu_button("+ Fixture", |ui| {
                            ui.label(egui::RichText::new("add to this stage").small().color(Color32::from_rgb(120, 142, 168)));
                            ui.separator();
                            if library_profiles.is_empty() {
                                ui.label(
                                    egui::RichText::new("no NAS hardware profiles found")
                                        .small()
                                        .color(Color32::from_rgb(180, 140, 100)),
                                );
                            } else {
                                for profile in library_profiles {
                                    let count = patch.count_profile(&profile.id);
                                    let label = format!(
                                        "{} {} \u{00B7} {} ({})",
                                        profile.manufacturer, profile.model, profile.mode, count
                                    );
                                    if ui.button(label).clicked() {
                                        let idx = patch.add_fixture_from_profile(profile);
                                        clicked_fixture = Some(idx);
                                        ui.close_menu();
                                    }
                                }
                            }
                        });
                        ui.label(
                            egui::RichText::new(format!("{} fixtures", patch.fixtures.len()))
                                .small()
                                .color(Color32::from_rgb(120, 142, 168)),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new("select a stage in the Lighting menu")
                                .small()
                                .italics()
                                .color(Color32::from_rgb(150, 170, 190)),
                        );
                    }
                    // Drop a placeable video screen — the wedding-room headline
                    // feature. Works with or without a SaaS stage; the operator
                    // drags it into place and (video pass) points it at a file.
                    if matches!(*venue, VenueKind::WeddingRoom) && ui.button("+ Screen").clicked() {
                        let idx = patch.add_screen_fixture();
                        clicked_fixture = Some(idx);
                    }
                    // Bind a local video file to the selected screen (the DJ
                    // points REACT at any file). Native picker, runs on click.
                    if let Some(sid) = &selected_screen_id {
                        if ui.button("Set video\u{2026}").clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("Video", &["mp4", "mov", "m4v", "mkv", "webm", "avi"])
                                .pick_file()
                            {
                                set_screen_file = Some((sid.clone(), path.display().to_string()));
                            }
                        }
                    }
                    if ui.button("Exit").clicked() {
                        exit_clicked = true;
                    }
                });

                esc_pressed = ui.input(|i| i.key_pressed(egui::Key::Escape));

                match *venue {
                    VenueKind::ConcertStage => {
                        draw_floor(&painter, &cam, stage);
                        draw_back_wall(&painter, &cam, stage, pipeline);
                        // Rig scaffold only when a stage is loaded — an empty
                        // venue reads as "pick a stage", not a bare rig.
                        if has_stage {
                            draw_trusses(&painter, &cam, stage, &patch.trusses, selected_truss);
                        }
                    }
                    VenueKind::WeddingRoom => {
                        // A closed reception room with a centered dance floor.
                        // No concert truss/LED wall — the operator drops uplights
                        // around the room via +Fixture and drag-to-place.
                        draw_wedding_room(&painter, &cam, stage);
                    }
                }
                draw_smoke(&painter, &cam, stage, fog_phase, time);
                let hits =
                    draw_fixtures(&painter, &cam, stage, glyph_fixtures, states, glyph_parents, profiles, dmx_frame, time, selected, show_labels, screen_video.as_ref().map(|(f, id)| (f.as_str(), *id)), pipeline);
                // Aim handle for the open fixture — the draggable beam endpoint.
                draw_aim_handle(&painter, &cam, stage, &patch.fixtures, panel_owner, aim_dragging.is_some());
                // Screen-space truss segments for click / drag hit-testing.
                // Only the concert rig draws trusses, so wedding mode has none to hit.
                let truss_hits: Vec<(usize, Pos2, Pos2)> = if has_stage
                    && matches!(*venue, VenueKind::ConcertStage)
                {
                    patch
                        .trusses
                        .iter()
                        .enumerate()
                        .filter_map(|(i, t)| {
                            let (a, b) = t.endpoints();
                            match (
                                cam.screen(V3::of(a), stage),
                                cam.screen(V3::of(b), stage),
                            ) {
                                (Some(sa), Some(sb)) => Some((i, sa, sb)),
                                _ => None,
                            }
                        })
                        .collect()
                } else {
                    Vec::new()
                };

                // Additive GL bloom over the finished stage — laser beams and
                // fixture hotspots flare into a soft glow. egui composites
                // alpha-over and can't sum crossing beams; this post pass does.
                {
                    let pipeline = pipeline.clone();
                    let cb = egui::PaintCallback {
                        rect: stage,
                        callback: std::sync::Arc::new(eframe::egui_glow::CallbackFn::new(
                            move |info, _painter| {
                                let vp = info.viewport_in_pixels();
                                if let Some(pipe) = pipeline.lock().as_mut() {
                                    pipe.draw_vstage_bloom([
                                        vp.left_px as i32,
                                        vp.from_bottom_px as i32,
                                        vp.width_px as i32,
                                        vp.height_px as i32,
                                    ]);
                                }
                            },
                        )),
                    };
                    painter.add(Shape::Callback(cb));
                }

                let hint_text = if has_stage {
                    "operator preview — drag a fixture to place it · click to re-aim · toggle under Lighting"
                } else {
                    "no stage selected — pick one in the Lighting menu to load fixtures"
                };
                painter.text(
                    Pos2::new(screen.center().x, screen.max.y - 16.0),
                    egui::Align2::CENTER_CENTER,
                    hint_text,
                    egui::FontId::proportional(11.0),
                    Color32::from_rgb(90, 105, 125),
                );

                // Click selects a fixture (opens its panel); drag moves it
                // anywhere on the stage. A press on the open control panel (a
                // Foreground area) is rejected via last frame's rect. A read-only
                // stage senses click-only — selection (inspect) still works, but no
                // drag-move / re-aim can start (those edits wouldn't persist anyway).
                let pick = ui.interact(
                    stage,
                    ui.id().with("vstage_pick"),
                    if editable {
                        egui::Sense::click_and_drag()
                    } else {
                        egui::Sense::click()
                    },
                );
                // Nearest fixture head under a screen point, within its
                // (padded) pick radius. A press over the open panel is ignored
                // — except on the fixture that panel belongs to, which stays
                // grabbable so it can be moved with its window open.
                let hit_at = |pos: Pos2| -> Option<usize> {
                    let mut best: Option<(f32, usize)> = None;
                    for (idx, body, radius) in &hits {
                        let d = (*body - pos).length();
                        // Pad the pick radius so small, distant heads stay
                        // grabbable — the visual size can be only a few px.
                        // Floor at 20 px (~touch target) so the operator can
                        // hit a fixture without surgical-precision aiming.
                        if d <= radius.max(20.0) && best.map_or(true, |(bd, _)| d < bd) {
                            best = Some((d, *idx));
                        }
                    }
                    let hit = best.map(|(_, idx)| idx);
                    if hit != panel_owner
                        && prev_panel_rect.map_or(false, |r| r.contains(pos))
                    {
                        return None;
                    }
                    hit
                };

                // Nearest truss bar under a screen point — point-to-projected-
                // segment distance within a small pixel band. Fixtures take
                // priority, so this is consulted only when no fixture is hit.
                let truss_hit_at = |pos: Pos2| -> Option<usize> {
                    if prev_panel_rect.map_or(false, |r| r.contains(pos)) {
                        return None;
                    }
                    let mut best: Option<(f32, usize)> = None;
                    for (idx, a, b) in &truss_hits {
                        let d = dist_point_segment(pos, *a, *b);
                        if d <= 9.0 && best.map_or(true, |(bd, _)| d < bd) {
                            best = Some((d, *idx));
                        }
                    }
                    best.map(|(_, idx)| idx)
                };

                if pick.drag_started() {
                    // Grab the aim handle first (the open fixture's beam end), then a
                    // fixture body, then an unlocked truss. Each locks its grab depth
                    // so it tracks the cursor in a stable plane instead of jumping.
                    if let Some(pos) = pick.interact_pointer_pos() {
                        if let Some(idx) = aim_handle_hit(panel_owner, &patch.fixtures, &cam, stage, pos) {
                            let h = aim_handle_world(&patch.fixtures[idx]);
                            aim_dragging = Some(AimDrag { idx, depth: h.sub(cam.eye).dot(cam.fwd) });
                        } else if let Some(idx) = hit_at(pos) {
                            let p = V3::of(patch.fixtures[idx].position);
                            let depth = p.sub(cam.eye).dot(cam.fwd);
                            let g = cam.ray_at_depth(pos, stage, depth);
                            dragging = Some(FixtureDrag {
                                idx,
                                depth,
                                offset: [p.x - g.x, p.y - g.y, p.z - g.z],
                            });
                        } else if let Some(idx) = truss_hit_at(pos) {
                            if !patch.trusses[idx].locked {
                                let c = V3::of(patch.trusses[idx].center);
                                let depth = c.sub(cam.eye).dot(cam.fwd);
                                let g = cam.ray_at_depth(pos, stage, depth);
                                truss_dragging = Some(TrussDrag {
                                    idx,
                                    depth,
                                    offset: [c.x - g.x, c.y - g.y, c.z - g.z],
                                });
                            }
                        }
                    }
                }
                if pick.dragged() {
                    if let (Some(ad), Some(pos)) = (aim_dragging, pick.interact_pointer_pos()) {
                        let head = V3::of(patch.fixtures[ad.idx].position);
                        if let Some(t) = ray_to_floor(&cam, stage, pos) {
                            // Dropped on the floor: the beam lands exactly there.
                            // Direction (tilt/pan) AND length (throw) come from the
                            // head→spot vector; throw persists in scale.y via sync.
                            let dir = t.sub(head);
                            let (tilt, pan) = aim_to_rotation(dir);
                            let throw = (dir.x * dir.x + dir.y * dir.y + dir.z * dir.z)
                                .sqrt()
                                .clamp(THROW_MIN, THROW_MAX);
                            patch.fixtures[ad.idx].rotation.x = tilt;
                            patch.fixtures[ad.idx].rotation.y = pan;
                            patch.fixtures[ad.idx].scale.y = throw;
                        } else {
                            // Aiming up off the floor: free-swing the direction at a
                            // locked depth, keeping whatever length was set.
                            let target = cam.ray_at_depth(pos, stage, ad.depth);
                            let (tilt, pan) = aim_to_rotation(target.sub(head));
                            patch.fixtures[ad.idx].rotation.x = tilt;
                            patch.fixtures[ad.idx].rotation.y = pan;
                        }
                    } else if let (Some(d), Some(pos)) = (dragging, pick.interact_pointer_pos()) {
                        let np = cam.ray_at_depth(pos, stage, d.depth);
                        patch.fixtures[d.idx].position.x = np.x + d.offset[0];
                        patch.fixtures[d.idx].position.y = np.y + d.offset[1];
                        patch.fixtures[d.idx].position.z = np.z + d.offset[2];
                    } else if let (Some(td), Some(pos)) =
                        (truss_dragging, pick.interact_pointer_pos())
                    {
                        let np = cam.ray_at_depth(pos, stage, td.depth);
                        patch.trusses[td.idx].center.x = np.x + td.offset[0];
                        patch.trusses[td.idx].center.y = np.y + td.offset[1];
                        patch.trusses[td.idx].center.z = np.z + td.offset[2];
                    }
                }
                if pick.drag_stopped() {
                    dragging = None;
                    truss_dragging = None;
                    aim_dragging = None;
                }
                if pick.clicked() {
                    if let Some(pos) = pick.interact_pointer_pos() {
                        if let Some(idx) = hit_at(pos) {
                            clicked_fixture = Some(idx);
                        } else if let Some(idx) = truss_hit_at(pos) {
                            clicked_truss = Some(idx);
                        }
                    }
                }
                // Grab affordance over a fixture or an unlocked truss.
                let hovering_grab = pick.hover_pos().map_or(false, |p| {
                    aim_handle_hit(panel_owner, &patch.fixtures, &cam, stage, p).is_some()
                        || hit_at(p).is_some()
                        || truss_hit_at(p).map_or(false, |i| !patch.trusses[i].locked)
                });
                if dragging.is_some() || truss_dragging.is_some() || aim_dragging.is_some() {
                    ui.ctx()
                        .output_mut(|o| o.cursor_icon = egui::CursorIcon::Grabbing);
                } else if hovering_grab {
                    ui.ctx()
                        .output_mut(|o| o.cursor_icon = egui::CursorIcon::Grab);
                }
            });

        self.dragging = dragging;
        self.truss_dragging = truss_dragging;
        self.aim_dragging = aim_dragging;
        // A freshly-picked screen video rebinds the file and drops the old
        // decoder + frame + open-marker so it reopens on the new path next frame.
        if let Some(binding) = set_screen_file {
            self.screen_file = Some(binding);
            self.screen_decoder = None;
            self.screen_tex = None;
            self.screen_open_done = None;
        }
        // Selection is mutually exclusive — clicking a fixture closes any open
        // truss panel and vice versa.
        if let Some(idx) = clicked_fixture {
            self.selected_fixture = Some(idx);
            self.selected_truss = None;
        } else if let Some(idx) = clicked_truss {
            self.selected_truss = Some(idx);
            self.selected_fixture = None;
        }

        // Per-fixture control panel — a separate Foreground area, drawn after
        // the stage closure releases `patches` so it can borrow the selected
        // fixture mutably.
        let mut keep_open = !exit_clicked;
        // Anchor a panel to whichever side of its target has room, never over
        // it, so the target stays clear and grabbable with the panel open.
        let panel_anchor = |body: Pos2| -> Pos2 {
            let panel_w = 256.0;
            let right_x = body.x + 22.0;
            let left_x = body.x - 22.0 - panel_w;
            let anchor_x = if right_x + panel_w <= screen.right() - 10.0 {
                right_x
            } else if left_x >= screen.left() + 10.0 {
                left_x
            } else {
                right_x
                    .min(screen.right() - panel_w - 10.0)
                    .max(screen.left() + 10.0)
            };
            Pos2::new(
                anchor_x,
                (body.y - 26.0)
                    .max(screen.top() + 44.0)
                    .min(screen.bottom() - 300.0),
            )
        };
        // Fixture and truss panels are mutually exclusive (selection enforces
        // it), so at most one is open at a time and owns `last_panel_rect`.
        if let Some(sel) = self.selected_fixture.filter(|&i| i < patch.fixtures.len()) {
            let fxp = cam
                .project(V3::of(patch.fixtures[sel].position), stage)
                .map(|(body, _)| body)
                .unwrap_or_else(|| stage.center());
            // Capture the id before the panel runs — a delete below removes the
            // fixture at `sel`, invalidating the index for any later id lookup.
            let id = patch.fixtures[sel].id.clone();
            let is_screen = patch.fixtures[sel].screen.is_some();
            let bound_here = self.screen_file.as_ref().map_or(false, |(f, _)| f == &id);
            let (keep_panel, delete) = if is_screen {
                // Screens get the video panel, never the lighting controls.
                let (keep_panel, delete, clear_video, panel_rect) =
                    screen_control_panel(ctx, &patch.fixtures[sel], bound_here, panel_anchor(fxp));
                self.last_panel_rect = Some(panel_rect);
                if clear_video && bound_here {
                    self.screen_file = None;
                    self.screen_decoder = None;
                    self.screen_tex = None;
                    self.screen_open_done = None;
                }
                (keep_panel, delete)
            } else {
                let profile = profiles.get(id.as_str()).cloned();
                let any_solo = overrides.any_solo(patch);
                let mut ov = overrides.get(&id);
                let (keep_panel, delete, panel_rect) = fixture_control_panel(
                    ctx,
                    &mut patch.fixtures[sel],
                    &mut ov,
                    any_solo,
                    profile.as_ref(),
                    panel_anchor(fxp),
                );
                overrides.set(&id, ov);
                self.last_panel_rect = Some(panel_rect);
                (keep_panel, delete)
            };
            if delete {
                patch.fixtures.remove(sel);
                overrides.remove(&id);
                // Drop any video bound to the removed screen.
                if bound_here {
                    self.screen_file = None;
                    self.screen_decoder = None;
                    self.screen_tex = None;
                    self.screen_open_done = None;
                }
                self.selected_fixture = None;
                self.dragging = None;
                self.aim_dragging = None;
                self.last_panel_rect = None;
            } else if !keep_panel {
                self.selected_fixture = None;
                self.last_panel_rect = None;
            }
        } else if let Some(sel) = self.selected_truss.filter(|&i| i < patch.trusses.len()) {
            let (a, b) = patch.trusses[sel].endpoints();
            let mid = StageVec3 {
                x: (a.x + b.x) * 0.5,
                y: (a.y + b.y) * 0.5,
                z: (a.z + b.z) * 0.5,
            };
            let tp = cam
                .project(V3::of(mid), stage)
                .map(|(body, _)| body)
                .unwrap_or_else(|| stage.center());
            let (keep_panel, panel_rect) =
                truss_control_panel(ctx, &mut patch.trusses[sel], panel_anchor(tp));
            self.last_panel_rect = Some(panel_rect);
            if !keep_panel {
                self.selected_truss = None;
                self.last_panel_rect = None;
            }
        } else {
            self.selected_fixture = None;
            self.selected_truss = None;
            self.last_panel_rect = None;
        }

        // Esc closes the control panel first, then the stage itself.
        if esc_pressed {
            if self.selected_fixture.is_some() || self.selected_truss.is_some() {
                self.selected_fixture = None;
                self.selected_truss = None;
                self.last_panel_rect = None;
            } else {
                keep_open = false;
            }
        }

        keep_open
    }
}

// ---------------------------------------------------------------------------
// per-fixture control panel
// ---------------------------------------------------------------------------

/// Human label for a fixture's visual kind — shown in the control panel.
fn kind_label(kind: StageVisualKind) -> &'static str {
    match kind {
        StageVisualKind::Wash => "Wash",
        StageVisualKind::Beam => "Beam",
        StageVisualKind::Laser => "Laser",
        StageVisualKind::Gobo => "Gobo",
        StageVisualKind::Strobe => "Strobe",
        StageVisualKind::LedPanel => "LED panel",
    }
}

/// The three mounting-aim presets the laser fan uses to pick a deliberate
/// centre vector. Kept as a tilt-only classifier here — pan is composed in
/// after the fact via `yaw_offset`.
#[derive(Clone, Copy, PartialEq)]
enum AimPreset {
    Down,
    Horizontal,
    Up,
}

/// Classify a `rotation.x` mounting tilt (degrees) into its aim preset.
fn aim_preset(tilt_deg: f32) -> AimPreset {
    if tilt_deg < 45.0 {
        AimPreset::Down
    } else if tilt_deg < 135.0 {
        AimPreset::Horizontal
    } else {
        AimPreset::Up
    }
}

/// Five cardinal directions a fixture can be aimed when mounted. Operator-
/// facing: the control panel shows these on a + compass pad with FOH at the
/// centre, Top up top, Bottom down bottom, Left/Right on the middle row.
/// Persisted via the existing rotation.x / rotation.y fields — no new SaaS
/// column. Convention is audience-POV: Left aims toward world -X, Right +X,
/// Top at the sky, Bottom at the floor, FOH straight at the audience.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MountDirection {
    Top,
    Bottom,
    Left,
    Right,
    Foh,
}

impl MountDirection {
    fn label(self) -> &'static str {
        match self {
            MountDirection::Top => "Top",
            MountDirection::Bottom => "Bottom",
            MountDirection::Left => "Left",
            MountDirection::Right => "Right",
            MountDirection::Foh => "FOH",
        }
    }

    /// Canonical (tilt, pan) degrees this direction snaps to. Tilt uses the
    /// existing convention (0 = down at the floor, 90 = horizontal toward
    /// audience, 180 = straight up). Pan is right-handed around world Z.
    fn to_rotation(self) -> (f32, f32) {
        match self {
            MountDirection::Bottom => (0.0, 0.0),
            MountDirection::Top => (180.0, 0.0),
            MountDirection::Foh => (90.0, 0.0),
            MountDirection::Right => (90.0, 90.0),
            MountDirection::Left => (90.0, -90.0),
        }
    }

    /// Classify a stored (tilt, pan) back into the nearest cardinal so the
    /// pad can highlight the current selection. Returns `None` for off-
    /// cardinal values — the pad shows no highlight when the fixture has
    /// been fine-tuned past the snap points.
    fn from_rotation(tilt_deg: f32, pan_deg: f32) -> Option<Self> {
        let near = |d: f32, target: f32| (d - target).abs() < 5.0;
        let pan = ((pan_deg % 360.0) + 540.0) % 360.0 - 180.0; // wrap to (-180, 180]
        if near(tilt_deg, 0.0) && near(pan, 0.0) {
            Some(MountDirection::Bottom)
        } else if near(tilt_deg, 180.0) && near(pan, 0.0) {
            Some(MountDirection::Top)
        } else if near(tilt_deg, 90.0) {
            if near(pan, 0.0) {
                Some(MountDirection::Foh)
            } else if near(pan, 90.0) {
                Some(MountDirection::Right)
            } else if near(pan, -90.0) {
                Some(MountDirection::Left)
            } else {
                None
            }
        } else {
            None
        }
    }
}

/// Control panel for a Screen fixture — a placeable video surface, NOT a light.
/// Shows screen actions (bound-video state, Clear video, Remove) instead of the
/// Intensity/Solo/Mute/aim controls of [`fixture_control_panel`] (which don't
/// apply and, for Solo, would dangerously blacken the real rig). Binding a file
/// stays in the header "Set video…" button. Returns
/// `(keep_open, delete, clear_video, panel_rect)`.
fn screen_control_panel(
    ctx: &egui::Context,
    fx: &FixturePatch,
    has_video: bool,
    anchor: Pos2,
) -> (bool, bool, bool, Rect) {
    let mut keep_open = true;
    let mut delete = false;
    let mut clear_video = false;
    let area = egui::Area::new(egui::Id::new(("vstage_screen_panel", fx.id.as_str())))
        .order(egui::Order::Foreground)
        .default_pos(anchor)
        .movable(true)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(Color32::from_rgb(16, 19, 27))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(72, 98, 132)))
                .inner_margin(egui::Margin::same(12.0))
                .show(ui, |ui| {
                    ui.set_width(232.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(&fx.name)
                                .strong()
                                .size(14.0)
                                .color(Color32::from_rgb(192, 226, 255)),
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui.button("\u{2715}").clicked() {
                                    keep_open = false;
                                }
                            },
                        );
                    });
                    ui.label(
                        egui::RichText::new("Video screen")
                            .small()
                            .color(Color32::from_rgb(120, 142, 168)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(if has_video {
                            "Video bound \u{00B7} re-bind with \u{201C}Set video\u{2026}\u{201D} in the header"
                        } else {
                            "No video \u{00B7} bind one with \u{201C}Set video\u{2026}\u{201D} in the header"
                        })
                        .small()
                        .color(Color32::from_rgb(150, 170, 190)),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(has_video, egui::Button::new("Clear video"))
                            .clicked()
                        {
                            clear_video = true;
                        }
                        if ui.button("Remove").clicked() {
                            delete = true;
                        }
                    });
                });
        });
    (keep_open, delete, clear_video, area.response.rect)
}

/// Per-fixture control panel: re-aim (down / horizontal / up) and move the
/// fixture in stage space. Edits `fx` in place — preview-local, with no DMX
/// or intent side effects (see `DmxManager::fixture_patch_mut`). Returns
/// `(keep_open, panel_rect)`; `keep_open` is false once the operator closes it.
fn fixture_control_panel(
    ctx: &egui::Context,
    fx: &mut FixturePatch,
    ov: &mut FixtureOverride,
    any_solo: bool,
    profile: Option<&FixtureProfile>,
    anchor: Pos2,
) -> (bool, bool, Rect) {
    let mut keep_open = true;
    let mut delete = false;
    let area = egui::Area::new(egui::Id::new(("vstage_fixture_panel", fx.id.as_str())))
        .order(egui::Order::Foreground)
        .default_pos(anchor)
        .movable(true)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(Color32::from_rgb(16, 19, 27))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(72, 98, 132)))
                .inner_margin(egui::Margin::same(12.0))
                .show(ui, |ui| {
                    ui.set_width(232.0);

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(&fx.name)
                                .strong()
                                .size(14.0)
                                .color(Color32::from_rgb(192, 226, 255)),
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui.button("✕").clicked() {
                                    keep_open = false;
                                }
                            },
                        );
                    });
                    ui.label(
                        egui::RichText::new(kind_label(fx.visual_kind))
                            .small()
                            .color(Color32::from_rgb(120, 142, 168)),
                    );

                    // LIVE CONTROL — the operator's ephemeral per-fixture mix,
                    // sitting at the top because it is what you reach for mid-show.
                    // Intensity trims the fixture (a further attenuation under the
                    // global master), Solo isolates it rig-wide, Mute silences it.
                    // These are in-memory only (reset on launch, like Blackout) and
                    // apply to BOTH the preview and the real DMX — what you dim here
                    // is what the rig does.
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("LIVE CONTROL")
                            .small()
                            .strong()
                            .color(Color32::from_rgb(138, 162, 188)),
                    );
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Intensity")
                                .small()
                                .color(Color32::from_rgb(170, 192, 220)),
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(format!("{:.0}%", ov.intensity * 100.0))
                                        .small()
                                        .color(Color32::from_rgb(170, 192, 220)),
                                );
                            },
                        );
                    });
                    ui.add(egui::Slider::new(&mut ov.intensity, 0.0..=1.0).show_value(false));
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(ov.solo, "SOLO")
                            .on_hover_text(
                                "Isolate this fixture — every other fixture goes dark while any Solo is active",
                            )
                            .clicked()
                        {
                            ov.solo = !ov.solo;
                        }
                        if ui
                            .selectable_label(ov.mute, "MUTE")
                            .on_hover_text("Silence this fixture (a Solo elsewhere overrides it)")
                            .clicked()
                        {
                            ov.mute = !ov.mute;
                        }
                    });
                    // Honest cue when this fixture is dark only because another is soloed.
                    if any_solo && !ov.solo {
                        ui.label(
                            egui::RichText::new("muted by solo")
                                .small()
                                .italics()
                                .color(Color32::from_rgb(170, 132, 132)),
                        );
                    }

                    // SEGMENT FX — only for segmented panels/bars (the 200W RGB
                    // panel's 8 cells). Auto = the music-tension autopilot; the rest
                    // lock the cells to a chosen pattern the operator drives. A cue
                    // with a segment binding still overrides this.
                    let is_segmented = profile.map_or(false, |p| {
                        matches!(p.housing, FixtureHousing::Panel | FixtureHousing::PixelBar)
                    });
                    if is_segmented {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("SEGMENT FX")
                                .small()
                                .strong()
                                .color(Color32::from_rgb(138, 162, 188)),
                        );
                        let label = |p: SegmentPattern| match p {
                            SegmentPattern::Auto => "Auto (music)",
                            SegmentPattern::Chase => "Chase \u{2192}",
                            SegmentPattern::ChaseReverse => "Chase \u{2190}",
                            SegmentPattern::Bounce => "Bounce",
                            SegmentPattern::CenterOut => "Center-out",
                            SegmentPattern::Rainbow => "Rainbow",
                            SegmentPattern::Wipe => "Wipe",
                            SegmentPattern::Strobe => "Sparkle strobe",
                            SegmentPattern::Bank => "Bank (alternate)",
                            SegmentPattern::Solid => "Solid",
                        };
                        egui::ComboBox::from_id_salt(("seg_fx", fx.id.as_str()))
                            .selected_text(label(ov.segment_fx))
                            .show_ui(ui, |ui| {
                                for p in [
                                    SegmentPattern::Auto,
                                    SegmentPattern::Chase,
                                    SegmentPattern::ChaseReverse,
                                    SegmentPattern::Bounce,
                                    SegmentPattern::CenterOut,
                                    SegmentPattern::Rainbow,
                                    SegmentPattern::Wipe,
                                    SegmentPattern::Strobe,
                                    SegmentPattern::Bank,
                                    SegmentPattern::Solid,
                                ] {
                                    ui.selectable_value(&mut ov.segment_fx, p, label(p));
                                }
                            });
                        if ov.segment_fx != SegmentPattern::Auto {
                            ui.add(
                                egui::Slider::new(&mut ov.segment_speed, 0.25..=4.0)
                                    .text("beats/cycle"),
                            );
                        }
                    }

                    // Mount direction — a + compass pad with FOH at the centre,
                    // Top up top, Bottom at the bottom, Left/Right on the middle
                    // row. Each button snaps rotation.x + rotation.y to a
                    // canonical (tilt, pan) pair. Shown for every kind, including
                    // strobe — operators still want to record the physical mount
                    // even when the visual is omnidirectional.
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("AIM")
                            .small()
                            .strong()
                            .color(Color32::from_rgb(138, 162, 188)),
                    );
                    let current = MountDirection::from_rotation(fx.rotation.x, fx.rotation.y);
                    let mut pick = |ui: &mut egui::Ui, dir: MountDirection| {
                        let on = current == Some(dir);
                        if ui.selectable_label(on, dir.label()).clicked() {
                            let (tilt, pan) = dir.to_rotation();
                            fx.rotation.x = tilt;
                            fx.rotation.y = pan;
                        }
                    };
                    let centered = |ui: &mut egui::Ui, body: &mut dyn FnMut(&mut egui::Ui)| {
                        ui.with_layout(
                            egui::Layout::top_down(egui::Align::Center),
                            |ui| ui.horizontal(|ui| body(ui)),
                        );
                    };
                    centered(ui, &mut |ui| pick(ui, MountDirection::Top));
                    centered(ui, &mut |ui| {
                        pick(ui, MountDirection::Left);
                        pick(ui, MountDirection::Foh);
                        pick(ui, MountDirection::Right);
                    });
                    centered(ui, &mut |ui| pick(ui, MountDirection::Bottom));
                    // The presets are coarse jumps; the beam-end handle on the stage
                    // fine-tunes to any direction. POSITION sliders removed — placement
                    // is drag-only and the dead read-outs added nothing.
                    let aim_hint = if fixture_is_aimable(fx) {
                        "drag the beam end on the stage to aim · drag the fixture to move it"
                    } else {
                        "drag the fixture on the stage to move it"
                    };
                    ui.label(
                        egui::RichText::new(aim_hint)
                            .small()
                            .color(Color32::from_rgb(120, 142, 168)),
                    );

                    // Real ATLABASE attributes for this fixture, resolved from
                    // the NAS hardware-profile library. Honest about absence —
                    // a missing profile reads as such, not faked from the
                    // fixture's built-in defaults.
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("ATTRIBUTES")
                            .small()
                            .strong()
                            .color(Color32::from_rgb(138, 162, 188)),
                    );
                    match profile {
                        Some(profile) => {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} {} — {} ({} ch)",
                                    profile.manufacturer,
                                    profile.model,
                                    profile.mode,
                                    profile.channel_count,
                                ))
                                .small()
                                .color(Color32::from_rgb(170, 192, 220)),
                            );
                            egui::ScrollArea::vertical()
                                .max_height(140.0)
                                .id_salt(("vstage_attr_scroll", fx.id.as_str()))
                                .show(ui, |ui| {
                                    for ch in &profile.channels {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Ch {}: {}",
                                                ch.channel_offset + 1,
                                                attribute_label(&ch.attribute),
                                            ))
                                            .small()
                                            .color(Color32::from_rgb(150, 172, 198)),
                                        );
                                    }
                                });
                        }
                        None => {
                            ui.label(
                                egui::RichText::new(if fx.profile_id.is_empty() {
                                    "No profile_id — set one in SaaS admin"
                                } else {
                                    "Profile not in NAS library"
                                })
                                .small()
                                .italics()
                                .color(Color32::from_rgb(170, 132, 132)),
                            );
                        }
                    }

                    ui.add_space(10.0);
                    ui.separator();
                    // Remove this fixture from the stage. Dropping it from the
                    // patch is all that's needed — the debounced stage sync
                    // diffs it out and DELETEs the SaaS row on the next flush.
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("Remove fixture")
                                    .color(Color32::from_rgb(255, 200, 200)),
                            )
                            .fill(Color32::from_rgb(74, 30, 34)),
                        )
                        .on_hover_text("Delete this fixture from the stage")
                        .clicked()
                    {
                        delete = true;
                    }
                });
        });
    (keep_open, delete, area.response.rect)
}

/// Human label for a truss's slot — shown in its control panel title.
fn truss_kind_label(kind: TrussKind) -> &'static str {
    match kind {
        TrussKind::Overhead => "Overhead truss",
        TrussKind::Floor => "Floor truss",
        TrussKind::LeftTower => "Left tower",
        TrussKind::RightTower => "Right tower",
        TrussKind::Generic => "Truss",
    }
}

/// Per-truss control panel: lock, drag-to-move hint, and sliders for size +
/// position. Mirrors `fixture_control_panel`; edits write straight into the
/// patch, so the debounced stage sync persists them. Returns (keep_open, rect).
fn truss_control_panel(ctx: &egui::Context, t: &mut Truss, anchor: Pos2) -> (bool, Rect) {
    let mut keep_open = true;
    let area = egui::Area::new(egui::Id::new(("vstage_truss_panel", t.id.as_str())))
        .order(egui::Order::Foreground)
        .default_pos(anchor)
        .movable(true)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(Color32::from_rgb(16, 19, 27))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(72, 98, 132)))
                .inner_margin(egui::Margin::same(12.0))
                .show(ui, |ui| {
                    ui.set_width(232.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(truss_kind_label(t.kind))
                                .strong()
                                .size(14.0)
                                .color(Color32::from_rgb(192, 226, 255)),
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui.button("✕").clicked() {
                                    keep_open = false;
                                }
                            },
                        );
                    });

                    ui.add_space(6.0);
                    ui.checkbox(&mut t.locked, "Locked (no drag)");

                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("ORIENTATION")
                            .small()
                            .strong()
                            .color(Color32::from_rgb(138, 162, 188)),
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(t.axis == TrussAxis::X, "Horizontal")
                            .clicked()
                        {
                            t.axis = TrussAxis::X;
                        }
                        if ui
                            .selectable_label(t.axis == TrussAxis::Z, "Vertical")
                            .clicked()
                        {
                            t.axis = TrussAxis::Z;
                        }
                    });

                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("SIZE")
                            .small()
                            .strong()
                            .color(Color32::from_rgb(138, 162, 188)),
                    );
                    ui.add(egui::Slider::new(&mut t.length, 1.0..=28.0).text("Length"));
                    ui.add(egui::Slider::new(&mut t.width, 0.2..=1.5).text("Width"));
                    let mut bays = t.sections as f64;
                    if ui
                        .add(egui::Slider::new(&mut bays, 2.0..=32.0).step_by(1.0).text("Bays"))
                        .changed()
                    {
                        t.sections = bays.round().clamp(2.0, 32.0) as u32;
                    }

                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("POSITION")
                            .small()
                            .strong()
                            .color(Color32::from_rgb(138, 162, 188)),
                    );
                    ui.label(
                        egui::RichText::new("drag the truss on the stage to move it")
                            .small()
                            .color(Color32::from_rgb(120, 142, 168)),
                    );
                    ui.add(egui::Slider::new(&mut t.center.x, -16.0..=16.0).text("L / R"));
                    ui.add(egui::Slider::new(&mut t.center.y, 0.0..=10.0).text("Depth"));
                    ui.add(egui::Slider::new(&mut t.center.z, 0.0..=16.0).text("Height"));
                });
        });
    (keep_open, area.response.rect)
}

/// Compact human label for a profile channel's attribute — fits the panel's
/// per-channel rows without wrapping.
fn attribute_label(attr: &DmxAttribute) -> String {
    match attr {
        DmxAttribute::Dimmer => "Dimmer".into(),
        DmxAttribute::Red => "Red".into(),
        DmxAttribute::Green => "Green".into(),
        DmxAttribute::Blue => "Blue".into(),
        DmxAttribute::White => "White".into(),
        DmxAttribute::Uv => "UV".into(),
        DmxAttribute::ColorWheel => "Color wheel".into(),
        DmxAttribute::Pan => "Pan".into(),
        DmxAttribute::Tilt => "Tilt".into(),
        DmxAttribute::Zoom => "Zoom".into(),
        DmxAttribute::Focus => "Focus".into(),
        DmxAttribute::Strobe => "Strobe".into(),
        DmxAttribute::GoboSlot => "Gobo".into(),
        DmxAttribute::GoboRotation => "Gobo rotation".into(),
        DmxAttribute::Prism => "Prism".into(),
        DmxAttribute::LaserMacro => "Laser macro".into(),
        DmxAttribute::Custom(name) => name.clone(),
    }
}

/// Pitch a fixture's beam target offset by its mounting tilt `rotation.x`
/// (degrees: 0 = the default downward aim, 90 = horizontal toward the
/// audience, 180 = straight up). Rotates the offset around stage-X. A tilt of
/// 0 is the identity, so a fixture at its default aim renders exactly as it
/// did before this control existed.
fn pitch_offset(off: V3, tilt_deg: f32) -> V3 {
    if tilt_deg.abs() < 0.05 {
        return off;
    }
    let t = tilt_deg.to_radians();
    let (s, c) = (t.sin(), t.cos());
    V3::new(off.x, off.y * c + off.z * s, off.z * c - off.y * s)
}

/// Pan rotation around world Z (vertical). Composes with `pitch_offset` so a
/// fixture's mounting aim reads as `yaw_offset(pitch_offset(off, tilt), pan)`:
/// tilt down from vertical, then pan around the hanging axis.
fn yaw_offset(off: V3, pan_deg: f32) -> V3 {
    if pan_deg.abs() < 0.05 {
        return off;
    }
    let t = pan_deg.to_radians();
    let (s, c) = (t.sin(), t.cos());
    V3::new(off.x * c - off.y * s, off.x * s + off.y * c, off.z)
}

/// Stage distance the aim handle floats out along a fixture's mounting aim.
const AIM_HANDLE_THROW: f32 = 5.0;
/// Screen-pixel radius for grabbing the aim handle.
const AIM_HANDLE_GRAB_PX: f32 = 14.0;

/// Min/max operator-set beam throw (stage units). The min stays clear of the
/// `scale.y == 1.0` default so that value still reads as "auto" (the renderer's
/// own per-kind landing).
const THROW_MIN: f32 = 1.5;
const THROW_MAX: f32 = 40.0;

/// Operator-set beam length, stored in the otherwise-unused `scale.y`. `None`
/// means the fixture is still on auto — the renderer uses its own per-kind landing
/// (so an untouched fixture renders exactly as before this feature).
fn fixture_throw(fx: &FixturePatch) -> Option<f32> {
    (fx.scale.y > 1.01).then(|| fx.scale.y.clamp(THROW_MIN, THROW_MAX))
}

/// Fixtures that throw a directional beam, so a draggable aim handle makes sense.
/// Strobes and LED panels are omnidirectional — no handle.
fn fixture_is_aimable(fx: &FixturePatch) -> bool {
    matches!(
        fx.visual_kind,
        StageVisualKind::Wash | StageVisualKind::Beam | StageVisualKind::Gobo | StageVisualKind::Laser
    )
}

/// Mounting-aim unit direction for `(tilt, pan)` degrees: tilt 0 = straight down,
/// 90 = horizontal toward the audience, 180 = up; pan rotates around the hanging
/// vertical axis. This is exactly what `yaw_offset(pitch_offset(·))` applies, so
/// [`aim_to_rotation`] is its clean inverse.
fn aim_dir(tilt_deg: f32, pan_deg: f32) -> V3 {
    let (st, ct) = tilt_deg.to_radians().sin_cos();
    let (sp, cp) = pan_deg.to_radians().sin_cos();
    V3::new(st * sp, -st * cp, -ct)
}

/// World position of a fixture's draggable aim handle — out along its mounting aim,
/// clamped to the floor so a steep-down aim keeps the handle on-screen.
fn aim_handle_world(fx: &FixturePatch) -> V3 {
    let p = V3::of(fx.position);
    let d = aim_dir(fx.rotation.x, fx.rotation.y);
    // The handle sits at the real endpoint: the operator-set throw if placed,
    // else a default reach so the handle is reachable on an un-placed fixture.
    let reach = fixture_throw(fx).unwrap_or(AIM_HANDLE_THROW);
    V3::new(p.x + d.x * reach, p.y + d.y * reach, (p.z + d.z * reach).max(0.0))
}

/// Recover `(tilt_deg, pan_deg)` from a desired aim direction — the inverse of
/// [`aim_dir`]. tilt in [0, 180]; pan in (-180, 180].
fn aim_to_rotation(dir: V3) -> (f32, f32) {
    let len = (dir.x * dir.x + dir.y * dir.y + dir.z * dir.z).sqrt().max(1e-6);
    let (dx, dy, dz) = (dir.x / len, dir.y / len, dir.z / len);
    let tilt = (-dz).clamp(-1.0, 1.0).acos().to_degrees();
    let pan = dx.atan2(-dy).to_degrees();
    (tilt, pan)
}

/// World point where the cursor ray meets the stage floor (z = 0), or `None` when
/// the ray points at/above the horizon (the operator is aiming into open air).
/// Mirrors the ray construction in `StageCamera::ray_at_depth`.
fn ray_to_floor(cam: &StageCamera, rect: Rect, sp: Pos2) -> Option<V3> {
    let aspect = (rect.width() / rect.height().max(1.0)).max(0.1);
    let ndc_x = (sp.x - rect.center().x) / (rect.width() * 0.5);
    let ndc_y = -(sp.y - rect.center().y) / (rect.height() * 0.5);
    let dir = cam
        .fwd
        .add(cam.right.scale(ndc_x * aspect / cam.focal))
        .add(cam.up.scale(ndc_y / cam.focal));
    if dir.z >= -1e-4 {
        return None; // not pointing down at the floor
    }
    let t = -cam.eye.z / dir.z;
    (t > 0.0).then(|| V3::new(cam.eye.x + dir.x * t, cam.eye.y + dir.y * t, 0.0))
}

/// A beam's endpoint. With a throw set: the operator-placed point
/// (`pos + aim · throw`) PLUS the live sweep delta (`off − off_neutral`) so a placed
/// mover still dances *around* its spot — static kinds pass `off_neutral == off`
/// (zero delta) and land exactly on it. Without a throw: the kind's own auto landing.
/// All floor-clamped at `min_z`.
fn aim_target(
    fx: &FixturePatch,
    pos: V3,
    off: V3,
    off_neutral: V3,
    tilt_deg: f32,
    pan_deg: f32,
    min_z: f32,
) -> V3 {
    match fixture_throw(fx) {
        Some(throw) => {
            let d = aim_dir(tilt_deg, pan_deg);
            let sweep = off.sub(off_neutral); // only the live pan/tilt wiggle
            V3::new(
                pos.x + d.x * throw + sweep.x,
                pos.y + d.y * throw + sweep.y,
                (pos.z + d.z * throw + sweep.z).max(min_z),
            )
        }
        None => V3::new(pos.x + off.x, pos.y + off.y, (pos.z + off.z).max(min_z)),
    }
}

/// Beam length for a mover with no operator-set throw.
const MOVER_AUTO_THROW: f32 = 10.0;

/// Where a moving head's beam ends right now, and how squarely it lands
/// (1 = on the floor, 0 = out in open air).
///
/// The live pan/tilt TURN the beam about the head, by real angles around its
/// mounting aim: 0.5 is the aim itself and the ends of the range are half the
/// travel window either side. So a head at rest still points exactly where it
/// was placed, and a head sent up visibly points up — the old landing-point
/// wiggle moved the far end of the beam about a metre whatever the tilt did.
///
/// The beam stops where it meets the floor. Aimed above that, it runs out to
/// a fixed reach in the air; the landing fades out as the floor recedes, so
/// there is no pop where a rising beam lets go of the deck.
fn mover_beam_end(fx: &FixturePatch, pos: V3, pan: f32, tilt: f32, min_z: f32) -> (V3, f32) {
    let tilt_deg = fx.rotation.x + (tilt - 0.5) * MOVER_TILT_WINDOW_DEG;
    let pan_deg = fx.rotation.y + (pan - 0.5) * MOVER_PAN_WINDOW_DEG;
    let d = aim_dir(tilt_deg, pan_deg);
    let air_reach = (fixture_throw(fx).unwrap_or(MOVER_AUTO_THROW) * 1.6).clamp(12.0, 45.0);
    let floor_reach = if d.z < -1e-3 { (pos.z - min_z).max(0.0) / -d.z } else { f32::INFINITY };
    let reach = floor_reach.min(air_reach);
    let landed = ((air_reach - floor_reach) / (air_reach * 0.2)).clamp(0.0, 1.0);
    (V3::new(pos.x + d.x * reach, pos.y + d.y * reach, pos.z + d.z * reach), landed)
}

/// Hit-test the aim handle of `owner` (the panel-selected fixture) at screen `pos`.
/// Returns the fixture index when within grab range and the fixture is aimable.
fn aim_handle_hit(
    owner: Option<usize>,
    fixtures: &[FixturePatch],
    cam: &StageCamera,
    rect: Rect,
    pos: Pos2,
) -> Option<usize> {
    let idx = owner?;
    let fx = fixtures.get(idx)?;
    if !fixture_is_aimable(fx) {
        return None;
    }
    let h = cam.screen(aim_handle_world(fx), rect)?;
    ((h - pos).length() <= AIM_HANDLE_GRAB_PX).then_some(idx)
}

/// Draw the aim handle for the selected, aimable fixture: a ring at the beam's aim
/// point with a thin guide line back to the head, brighter while it is dragged.
fn draw_aim_handle(
    p: &egui::Painter,
    cam: &StageCamera,
    rect: Rect,
    fixtures: &[FixturePatch],
    owner: Option<usize>,
    dragging: bool,
) {
    let Some(idx) = owner else { return };
    let Some(fx) = fixtures.get(idx) else { return };
    if !fixture_is_aimable(fx) {
        return;
    }
    let (Some(head), Some(h)) = (
        cam.screen(V3::of(fx.position), rect),
        cam.screen(aim_handle_world(fx), rect),
    ) else {
        return;
    };
    let col = if dragging {
        Color32::from_rgb(150, 230, 255)
    } else {
        Color32::from_rgb(120, 200, 240)
    };
    p.line_segment([head, h], Stroke::new(1.0_f32, tint(col, 0.5)));
    p.circle_stroke(h, 8.0, Stroke::new(2.0_f32, col));
    p.circle_filled(h, 3.0, col);
}

/// A bright ring marking the fixture whose control panel is open.
fn draw_selection_ring(p: &egui::Painter, body: Pos2, s: f32) {
    let hi = Color32::from_rgb(120, 220, 255);
    p.circle_stroke(body, s * 1.18, Stroke::new(2.0_f32, hi));
    p.circle_stroke(body, s * 1.18 + 3.5, Stroke::new(1.0_f32, tint(hi, 0.35)));
}

// ---------------------------------------------------------------------------
// stage geometry
// ---------------------------------------------------------------------------

fn draw_floor(p: &egui::Painter, cam: &StageCamera, rect: Rect) {
    let (x0, x1, y0, y1) = (-15.0, 15.0, -3.0, 9.0);

    // depth-faded floor fill — lighter downstage (near), darker upstage (far)
    if let Some(corners) = quad(
        cam,
        rect,
        [
            V3::new(x0, y0, 0.0),
            V3::new(x1, y0, 0.0),
            V3::new(x1, y1, 0.0),
            V3::new(x0, y1, 0.0),
        ],
    ) {
        let near = Color32::from_rgb(22, 26, 35);
        let far = Color32::from_rgb(10, 12, 17);
        p.add(grad_quad(corners, [near, near, far, far]));
    }

    // perspective grid
    let grid = Stroke::new(1.0_f32, Color32::from_rgb(30, 38, 52));
    let mut y = y0;
    while y <= y1 + 0.001 {
        if let (Some(a), Some(b)) = (
            cam.screen(V3::new(x0, y, 0.0), rect),
            cam.screen(V3::new(x1, y, 0.0), rect),
        ) {
            p.line_segment([a, b], grid);
        }
        y += 2.0;
    }
    let mut x = x0;
    while x <= x1 + 0.001 {
        if let (Some(a), Some(b)) = (
            cam.screen(V3::new(x, y0, 0.0), rect),
            cam.screen(V3::new(x, y1, 0.0), rect),
        ) {
            p.line_segment([a, b], grid);
        }
        x += 2.0;
    }
}

/// The wedding-room venue: a rectangular reception room (warm floor + three
/// walls, the camera-side "fourth wall" left open) with a centered dance floor.
/// Replaces the concert floor/LED-wall/truss scaffold. Painter has no depth
/// buffer, so draw order is far→near: floor, walls, then the dance floor on top.
fn draw_wedding_room(p: &egui::Painter, cam: &StageCamera, rect: Rect) {
    let (x0, x1, y0, y1) = (-12.0, 12.0, -9.0, 11.0);
    let wall_h = 7.0;

    // warm-neutral floor — lighter near (downstage) fading darker toward the back
    if let Some(corners) = quad(
        cam,
        rect,
        [
            V3::new(x0, y0, 0.0),
            V3::new(x1, y0, 0.0),
            V3::new(x1, y1, 0.0),
            V3::new(x0, y1, 0.0),
        ],
    ) {
        let near = Color32::from_rgb(34, 27, 24);
        let far = Color32::from_rgb(18, 14, 14);
        p.add(grad_quad(corners, [near, near, far, far]));
    }

    // subtle warm perspective grid on the room floor
    let grid = Stroke::new(1.0_f32, Color32::from_rgb(48, 38, 33));
    let mut y = y0;
    while y <= y1 + 0.001 {
        if let (Some(a), Some(b)) = (
            cam.screen(V3::new(x0, y, 0.0), rect),
            cam.screen(V3::new(x1, y, 0.0), rect),
        ) {
            p.line_segment([a, b], grid);
        }
        y += 2.0;
    }
    let mut x = x0;
    while x <= x1 + 0.001 {
        if let (Some(a), Some(b)) = (
            cam.screen(V3::new(x, y0, 0.0), rect),
            cam.screen(V3::new(x, y1, 0.0), rect),
        ) {
            p.line_segment([a, b], grid);
        }
        x += 2.0;
    }

    // walls rise from the floor edges; the front (camera-side, y = y0) is left
    // open so we look into the room. Back first, then sides (far→near).
    draw_wall(p, cam, rect, V3::new(x0, y1, 0.0), V3::new(x1, y1, 0.0), wall_h);
    draw_wall(p, cam, rect, V3::new(x0, y0, 0.0), V3::new(x0, y1, 0.0), wall_h);
    draw_wall(p, cam, rect, V3::new(x1, y0, 0.0), V3::new(x1, y1, 0.0), wall_h);

    // centered dance floor — the "middle of the room" focal panel
    draw_dance_floor(p, cam, rect, -5.0, 5.0, -1.0, 7.0);
}

/// One opaque room wall: a vertical quad from the floor edge `a`→`b` up to
/// `height`, gradient-lit warm/blush at the base (as if uplit) fading dark at the
/// top, with a thin warm ceiling-line rim.
fn draw_wall(p: &egui::Painter, cam: &StageCamera, rect: Rect, a: V3, b: V3, height: f32) {
    let top_a = V3::new(a.x, a.y, height);
    let top_b = V3::new(b.x, b.y, height);
    if let Some(corners) = quad(cam, rect, [a, b, top_b, top_a]) {
        let base = Color32::from_rgb(60, 34, 44); // warm blush uplight at the floor
        let top = Color32::from_rgb(14, 11, 16); // fades to dark near the ceiling
        p.add(grad_quad(corners, [base, base, top, top]));
        if let (Some(ta), Some(tb)) = (cam.screen(top_a, rect), cam.screen(top_b, rect)) {
            p.line_segment([ta, tb], Stroke::new(1.0_f32, Color32::from_rgb(70, 52, 48)));
        }
    }
}

/// The centered dance floor: a warm-wood panel with a soft spotlit center glow,
/// a finer parquet grid, and a bright inset border. Static in slice 1 (a future
/// pass can make it react to the music / fixture wash). World rectangle
/// `x0..x1 × y0..y1` on the room floor.
fn draw_dance_floor(p: &egui::Painter, cam: &StageCamera, rect: Rect, x0: f32, x1: f32, y0: f32, y1: f32) {
    let z = 0.0;
    let Some(scr) = quad(
        cam,
        rect,
        [
            V3::new(x0, y0, z),
            V3::new(x1, y0, z),
            V3::new(x1, y1, z),
            V3::new(x0, y1, z),
        ],
    ) else {
        return;
    };

    // warm-wood fill, a touch brighter at the near edge
    let near = Color32::from_rgb(78, 54, 32);
    let far = Color32::from_rgb(54, 37, 23);
    p.add(grad_quad(scr, [near, near, far, far]));

    // soft warm glow pooled at the center — reads as a spotlit dance floor
    if let Some(c) = cam.screen(V3::new((x0 + x1) * 0.5, (y0 + y1) * 0.5, z), rect) {
        let r = (scr[0] - scr[2]).length() * 0.32;
        p.add(soft_disc(c, r, Color32::from_rgba_unmultiplied(150, 110, 60, 70)));
    }

    // finer parquet grid inside the panel
    let parquet = Stroke::new(1.0_f32, Color32::from_rgb(96, 66, 38));
    let step = 1.25;
    let mut gx = x0;
    while gx <= x1 + 0.001 {
        if let (Some(a), Some(b)) = (
            cam.screen(V3::new(gx, y0, z), rect),
            cam.screen(V3::new(gx, y1, z), rect),
        ) {
            p.line_segment([a, b], parquet);
        }
        gx += step;
    }
    let mut gy = y0;
    while gy <= y1 + 0.001 {
        if let (Some(a), Some(b)) = (
            cam.screen(V3::new(x0, gy, z), rect),
            cam.screen(V3::new(x1, gy, z), rect),
        ) {
            p.line_segment([a, b], parquet);
        }
        gy += step;
    }

    // bright inset border framing the floor
    p.add(Shape::convex_polygon(
        scr.to_vec(),
        Color32::TRANSPARENT,
        Stroke::new(2.0_f32, Color32::from_rgb(150, 110, 60)),
    ));
}

fn draw_back_wall(
    p: &egui::Painter,
    cam: &StageCamera,
    rect: Rect,
    pipeline: &Arc<Mutex<Option<RenderPipeline>>>,
) {
    let y = 8.0; // upstage plane — the LED wall

    let bezel = quad(
        cam,
        rect,
        [
            V3::new(-12.5, y, 13.5),
            V3::new(12.5, y, 13.5),
            V3::new(12.5, y, 0.8),
            V3::new(-12.5, y, 0.8),
        ],
    );
    let scr = quad(
        cam,
        rect,
        [
            V3::new(-12.0, y, 13.0),
            V3::new(12.0, y, 13.0),
            V3::new(12.0, y, 1.2),
            V3::new(-12.0, y, 1.2),
        ],
    );

    if let Some(b) = bezel {
        p.add(Shape::convex_polygon(
            b.to_vec(),
            Color32::from_rgb(20, 22, 29),
            Stroke::new(1.5_f32, Color32::from_rgb(74, 100, 128)),
        ));
    }
    let Some(s) = scr else { return };

    let live = pipeline
        .lock()
        .as_ref()
        .map_or(false, |pipe| pipe.vstage_backwall_ready());

    if live {
        // Live REACT frame — a GL paint callback draws the captured texture
        // onto the back-wall quad. Inserted here (after the floor, before the
        // fixtures) so fixtures composite in front of the wall.
        let pipeline = pipeline.clone();
        let cb = egui::PaintCallback {
            rect,
            callback: std::sync::Arc::new(eframe::egui_glow::CallbackFn::new(
                move |info, _painter| {
                    let vp = info.viewport_in_pixels();
                    let ppp = info.pixels_per_point;
                    let w = (vp.width_px as f32).max(1.0);
                    let h = (vp.height_px as f32).max(1.0);
                    let to_ndc = |q: Pos2| -> [f32; 2] {
                        [
                            (q.x * ppp - vp.left_px as f32) / w * 2.0 - 1.0,
                            1.0 - (q.y * ppp - vp.top_px as f32) / h * 2.0,
                        ]
                    };
                    let corners = [to_ndc(s[0]), to_ndc(s[1]), to_ndc(s[2]), to_ndc(s[3])];
                    let viewport = [
                        vp.left_px as i32,
                        vp.from_bottom_px as i32,
                        vp.width_px as i32,
                        vp.height_px as i32,
                    ];
                    if let Some(pipe) = pipeline.lock().as_ref() {
                        pipe.draw_vstage_backwall(corners, viewport);
                    }
                },
            )),
        };
        p.add(Shape::Callback(cb));
        // thin inner frame on top of the live feed
        p.add(Shape::convex_polygon(
            s.to_vec(),
            Color32::TRANSPARENT,
            Stroke::new(1.0_f32, Color32::from_rgb(52, 70, 92)),
        ));
    } else {
        // Placeholder until the first live frame is captured.
        p.add(grad_quad(
            s,
            [
                Color32::from_rgb(15, 18, 26),
                Color32::from_rgb(15, 18, 26),
                Color32::from_rgb(8, 10, 15),
                Color32::from_rgb(8, 10, 15),
            ],
        ));
        p.add(Shape::convex_polygon(
            s.to_vec(),
            Color32::TRANSPARENT,
            Stroke::new(1.0_f32, Color32::from_rgb(52, 70, 92)),
        ));
        for i in 1..8 {
            let t = i as f32 / 8.0;
            let a = lerp_pos(s[0], s[3], t);
            let b = lerp_pos(s[1], s[2], t);
            p.line_segment(
                [a, b],
                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(70, 100, 140, 22)),
            );
        }
        let c = centroid(&s);
        p.add(soft_disc(
            c,
            (s[1].x - s[0].x).abs().max(120.0) * 0.4,
            Color32::from_rgba_unmultiplied(40, 70, 110, 36),
        ));
        p.text(
            c,
            egui::Align2::CENTER_CENTER,
            "LIVE OUTPUT",
            egui::FontId::proportional(16.0),
            Color32::from_rgb(96, 132, 170),
        );
        p.text(
            c + Vec2::new(0.0, 22.0),
            egui::Align2::CENTER_CENTER,
            "REACT view — live feed warming up",
            egui::FontId::proportional(10.0),
            Color32::from_rgb(64, 84, 108),
        );
    }
}

// ---------------------------------------------------------------------------
// rig trusses — the structure the fixtures hang on
// ---------------------------------------------------------------------------

/// Draw one truss as a flat lattice run from `a` to `b`: two chord rails set
/// `width` apart with rung cross-ties and zig-zag bracing between them. Pure
/// scenery — no DMX, no intent. The lattice face spreads along whichever world
/// axis lies most across the run, so an upright tower spreads in X and a flown
/// horizontal truss spreads in Z; either reads as a truss from front-of-house.
fn draw_truss(
    p: &egui::Painter,
    cam: &StageCamera,
    rect: Rect,
    a: V3,
    b: V3,
    width: f32,
    sections: usize,
) {
    let run = b.sub(a);
    let len = run.dot(run).sqrt();
    if len < 1e-3 {
        return;
    }
    let dir = run.scale(1.0 / len);
    let spread = if dir.z.abs() > dir.x.abs() {
        V3::new(1.0, 0.0, 0.0)
    } else {
        V3::new(0.0, 0.0, 1.0)
    };
    let half = spread.scale(width * 0.5);
    let (a1, a2) = (a.sub(half), a.add(half));
    let (b1, b2) = (b.sub(half), b.add(half));

    let chord = Stroke::new(2.0_f32, Color32::from_rgb(82, 92, 110));
    let web = Stroke::new(1.0_f32, Color32::from_rgb(47, 54, 68));

    // chord rails
    if let (Some(s1), Some(s2)) = (cam.screen(a1, rect), cam.screen(b1, rect)) {
        p.line_segment([s1, s2], chord);
    }
    if let (Some(s1), Some(s2)) = (cam.screen(a2, rect), cam.screen(b2, rect)) {
        p.line_segment([s1, s2], chord);
    }
    // rung cross-ties + alternating zig-zag bracing
    let segs = sections.max(2);
    let mut prev: Option<(Pos2, Pos2)> = None;
    for i in 0..=segs {
        let t = i as f32 / segs as f32;
        let (r1, r2) = match (
            cam.screen(a1.lerp(b1, t), rect),
            cam.screen(a2.lerp(b2, t), rect),
        ) {
            (Some(r1), Some(r2)) => (r1, r2),
            _ => {
                prev = None;
                continue;
            }
        };
        p.line_segment([r1, r2], web);
        if let Some((pr1, pr2)) = prev {
            let diag = if i % 2 == 0 { [pr1, r2] } else { [pr2, r1] };
            p.line_segment(diag, web);
        }
        prev = Some((r1, r2));
    }
}

/// Distance in screen pixels from point `p` to segment `a`–`b`. Used to
/// hit-test trusses, which project to thin lines rather than round bodies.
fn dist_point_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.x * ab.x + ab.y * ab.y;
    if len2 <= 1e-6 {
        return (p - a).length();
    }
    let t = (((p - a).x * ab.x + (p - a).y * ab.y) / len2).clamp(0.0, 1.0);
    let proj = Pos2::new(a.x + ab.x * t, a.y + ab.y * t);
    (p - proj).length()
}

/// Draw the rig: each operator-placed truss as a flat lattice between its
/// endpoints. The selected truss gets a cyan centerline; a locked one carries
/// a padlock so its frozen state reads at a glance. Pure scenery — no DMX.
fn draw_trusses(
    p: &egui::Painter,
    cam: &StageCamera,
    rect: Rect,
    trusses: &[Truss],
    selected: Option<usize>,
) {
    for (i, t) in trusses.iter().enumerate() {
        let (a, b) = t.endpoints();
        let (wa, wb) = (V3::of(a), V3::of(b));
        draw_truss(p, cam, rect, wa, wb, t.width, t.sections.max(2) as usize);

        let (sa, sb) = match (cam.screen(wa, rect), cam.screen(wb, rect)) {
            (Some(sa), Some(sb)) => (sa, sb),
            _ => continue,
        };
        if selected == Some(i) {
            p.line_segment([sa, sb], Stroke::new(2.0_f32, Color32::from_rgb(120, 220, 255)));
        }
        if t.locked {
            let mid = lerp_pos(sa, sb, 0.5);
            p.text(
                mid + Vec2::new(0.0, -11.0),
                egui::Align2::CENTER_CENTER,
                "🔒",
                egui::FontId::proportional(12.0),
                Color32::from_rgb(150, 170, 190),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// stage smoke
// ---------------------------------------------------------------------------

/// Stage smoke drifting up from the floor. `phase` (0..1) is how far the haze
/// has built — puffs are drawn only below a rising ceiling at `phase`, so the
/// smoke forms low and climbs as the Fog trigger ramps it up. Pure scenery.
fn draw_smoke(p: &egui::Painter, cam: &StageCamera, rect: Rect, phase: f32, time: f64) {
    if phase <= 0.002 {
        return;
    }
    // Stable per-puff pseudo-random in 0..1.
    let hash = |n: f32| -> f32 {
        let s = (n * 12.9898).sin() * 43758.5453;
        s - s.floor()
    };
    let t = time as f32;
    const PUFFS: usize = 36;
    let top_z = 7.0_f32; // climb height at full phase

    let mut puffs: Vec<(f32, Pos2, f32, Color32)> = Vec::with_capacity(PUFFS);
    for i in 0..PUFFS {
        let fi = i as f32;
        let bx = -11.0 + hash(fi) * 22.0;
        let by = -1.0 + hash(fi + 41.0) * 7.0;
        let speed = 0.045 + hash(fi + 83.0) * 0.075;
        let off = hash(fi + 127.0);
        // Continuous climb 0..1, wrapping — the puff rises then respawns low.
        let nh = (t * speed + off).fract();
        // Soft rising ceiling at `phase`.
        let ceil = ((phase - nh) / 0.22).clamp(0.0, 1.0);
        if ceil <= 0.0 {
            continue;
        }
        // Fades in off the floor, thins toward the top — hides the wrap.
        let env = (nh * 4.0).min(1.0) * (1.0 - nh).powf(0.55);
        let drift = (t * 0.33 + off * TAU).sin() * (0.35 + nh * 1.1);
        let Some((center, depth)) =
            cam.project(V3::new(bx + drift, by, nh * top_z), rect)
        else {
            continue;
        };
        let churn = 1.0 + (t * 0.7 + fi).sin() * 0.12;
        let rw = (0.9 + nh * 2.8) * churn;
        let screen_r = rw / depth * cam.focal * rect.height() * 0.5;
        // Cap each puff well below the previous 64/255 so the fog reads as
        // atmosphere instead of a screen-filling fog that hides fixtures
        // behind it. The cumulative haze of 36 puffs at this ceiling still
        // softens beams without blocking the rig.
        let alpha = (phase * ceil * env * 28.0).clamp(0.0, 32.0) as u8;
        if alpha == 0 {
            continue;
        }
        puffs.push((
            depth,
            center,
            screen_r,
            Color32::from_rgba_unmultiplied(168, 178, 196, alpha),
        ));
    }
    // Far -> near so nearer puffs layer on top.
    puffs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, center, r, col) in puffs {
        p.add(soft_disc(center, r, col));
    }
}

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

fn draw_fixtures(
    p: &egui::Painter,
    cam: &StageCamera,
    rect: Rect,
    patches: &[FixturePatch],
    states: &[FixtureIntentState],
    // Editable-fixture index per glyph: many glyphs (a bar's sections) can share
    // one parent so a click selects the whole bar and the selection ring lights
    // every section. For an ordinary rig this is just `i`.
    parents: &[usize],
    profiles: &std::collections::HashMap<String, FixtureProfile>,
    dmx_frame: Option<&DmxFrame>,
    time: f64,
    selected: Option<usize>,
    show_labels: bool,
    screen_video: Option<(&str, egui::TextureId)>,
    pipeline: &Arc<Mutex<Option<RenderPipeline>>>,
) -> Vec<(usize, Pos2, f32)> {
    // depth-sort far -> near so nearer fixtures composite on top
    let mut order: Vec<(f32, usize)> = patches
        .iter()
        .enumerate()
        .filter_map(|(i, fx)| cam.project(V3::of(fx.position), rect).map(|(_, d)| (d, i)))
        .collect();
    order.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    // Hit targets for click-to-select: (EDITABLE fixture index, screen body, radius).
    let mut hits = Vec::with_capacity(order.len());
    for (_, i) in order {
        let fx = &patches[i];
        // states is index-aligned with the glyphs (same order)
        let st = states.get(i);
        // A section glyph carries a blank id -> profile None -> draw_fixture
        // dispatches by visual_kind (a real id would resolve the combined bar
        // profile and draw every section as one housing).
        let profile = profiles.get(&fx.id);
        let parent = parents.get(i).copied().unwrap_or(i);
        if let Some((body, s)) = draw_fixture(
            p, cam, rect, fx, st, profile, dmx_frame, time, selected == Some(parent), show_labels,
            screen_video, pipeline,
        ) {
            hits.push((parent, body, s));
        }
    }
    hits
}

fn draw_fixture(
    p: &egui::Painter,
    cam: &StageCamera,
    rect: Rect,
    fx: &FixturePatch,
    st: Option<&FixtureIntentState>,
    profile: Option<&FixtureProfile>,
    dmx_frame: Option<&DmxFrame>,
    time: f64,
    selected: bool,
    show_labels: bool,
    screen_video: Option<(&str, egui::TextureId)>,
    pipeline: &Arc<Mutex<Option<RenderPipeline>>>,
) -> Option<(Pos2, f32)> {
    let (body, depth) = cam.project(V3::of(fx.position), rect)?;
    let s = (160.0 / depth).clamp(3.0, 14.0); // screen-space size unit — realistic fixture scale

    // A Screen fixture is a placeable VIDEO SURFACE, not a light — render it as a
    // posed quad and skip the lighting dispatch. Slice 1 draws a placeholder
    // panel; the operator-selected video lands on this same quad in the video
    // pass. Intercept before the housing/visual_kind dispatch (a screen carries
    // visual_kind=LedPanel only to satisfy the SaaS strict-enum guard).
    if fx.screen.is_some() {
        return draw_screen_fixture(
            p, cam, rect, fx, body, s, selected, show_labels, screen_video, pipeline,
        );
    }

    // Housing-driven dispatch: the profile's `housing` field is the renderer's
    // source of truth for fixture shape, set once at NAS-library load time by
    // scanning all modes. The fixture LOOKS the same regardless of which
    // mode (3/9/24-channel, etc.) SaaS picked at patch time — only what's
    // INSIDE the housing changes via the ChannelMap. See
    // `dmx::profiles::infer_housing` for the inference rules and
    // `FixtureHousing` in contracts.rs for the variant taxonomy.
    if let Some(profile) = profile {
        // Per-cell colors come from segments_for_profile when the active
        // mode is multi-cell; otherwise we synthesize a single virtual
        // segment from the patch's main RGB(W) channels so the housing
        // still renders a coherent color in 3-channel and 9-channel modes.
        let mut segments = segments_for_profile(profile, fx.start_address);
        if segments.is_empty() {
            if let Some(single) = fallback_single_segment(&fx.channels) {
                segments.push(single);
            }
        }
        match profile.housing {
            FixtureHousing::Panel => {
                return draw_segment_panel(
                    p, cam, rect, fx, st, &segments, dmx_frame, time, selected, show_labels,
                );
            }
            FixtureHousing::PixelBar => {
                // Motorized vs static comes from the ACTIVE patched channels
                // — the housing is the same shape, the bar just tilts when
                // a Tilt channel is wired on this mode.
                return draw_segment_bar(p, cam, rect, fx, &segments, dmx_frame, selected, show_labels);
            }
            FixtureHousing::ScanLaser
            | FixtureHousing::MovingHead
            | FixtureHousing::Strobe
            | FixtureHousing::Par
            | FixtureHousing::Generic => {
                // Fall through to the visual_kind dispatch below — these
                // housings still use the per-kind draw branches (Wash /
                // Beam / Gobo / Laser / Strobe).
            }
        }
    }
    let dim = st.map(|x| x.dimmer).unwrap_or(0.0).clamp(0.0, 1.0);
    let strobe_amt = st.map(|x| x.strobe).unwrap_or(0.0).clamp(0.0, 1.0);
    let macro_amt = st.map(|x| x.laser_macro).unwrap_or(0);
    // Zoom channel (0 = tight, 1 = wide) drives cone spread per kind. The
    // intent layer rides it on energy, so beams open up as the track lifts.
    let zoom = st.map(|x| x.zoom).unwrap_or(0.4).clamp(0.0, 1.0);
    // ATLABASE profile is the source of truth for hardware kind — if it says
    // a fixture is a Laser but SaaS authored a different visual_kind, render
    // it as a laser. Channel-map fallback handles the case where the profile
    // didn't resolve (NAS library miss or a typo'd profile_id in SaaS): a
    // fixture with a laser_macro DMX channel IS a laser, regardless of the
    // authored visual_kind.
    let profile_is_laser = matches!(profile.map(|p| p.kind), Some(FixtureKind::Laser));
    let has_laser_macro = fx.channels.laser_macro.is_some();
    let visual_kind = if profile_is_laser || has_laser_macro {
        StageVisualKind::Laser
    } else if renders_as_galaxy_effect(fx) {
        // COM-499: the GalaxyJet is an effect projector like the Obsession, not a
        // tight 7R beam. SaaS tags it `Beam` (category "Moving Heads"), which would
        // drop it into the tight-column beam arm — indistinguishable from the 7R.
        // Render it through the WIDE gobo arm instead (a cone with a circle pool at
        // the landing, aimed down, its own sweep) so it reads like the Obsession did
        // on main. It keeps its pan/tilt sweep. Keyed on effect identity, so a SaaS
        // visual_kind re-tag doesn't change it.
        StageVisualKind::Gobo
    } else {
        fx.visual_kind
    };
    let color = fixture_color(fx, visual_kind, st);
    let pos = V3::of(fx.position);
    // Mounting aim from the fixture control panel: rotation.x is tilt
    // (0 = default downward aim, 90 = horizontal toward audience, 180 = up);
    // rotation.y is pan around the hanging vertical axis. Beam/Gobo/Laser
    // compose pitch then yaw to land on Left / Right / FOH / Top / Bottom
    // cardinal aims.
    let tilt_deg = fx.rotation.x;
    let pan_deg = fx.rotation.y;

    // Off / blackout — no beam, no glow, just a dim head so the rig stays
    // visible. Blackout zeroes every channel, so this is where it lands.
    let lit = dim > 0.03 || strobe_amt > 0.1 || macro_amt > 0;
    if !lit {
        p.circle_filled(body, s * 0.26, tint(color, 0.20));
        p.circle_stroke(body, s * 0.34, Stroke::new(1.0_f32, tint(color, 0.38)));
        if selected {
            draw_selection_ring(p, body, s);
        }
        draw_fixture_label(p, body + Vec2::new(s * 0.7 + 3.0, -s * 0.7 - 3.0), &fx.name, show_labels);
        return Some((body, s));
    }

    // COM-499: a static colour-wheel effect projector (the Chauvet Obsession
    // moonflower) THROWS A BEAM in its mounting-aim direction; the spinning gobo's
    // STAR pattern appears WHERE THAT BEAM LANDS on a surface (floor/wall) — at the
    // beam END, not at the head. Aimed steeply up into open air there is no surface,
    // so the star fades and it reads as just a beam. Mirror `is_effect_fixture`
    // (colour wheel, no pan/tilt) so real moving-head gobos keep their cone.
    if matches!(visual_kind, StageVisualKind::Beam | StageVisualKind::Gobo)
        && fx.channels.color_wheel.is_some()
        && fx.channels.pan.is_none()
        && fx.channels.tilt.is_none()
    {
        // Aim is the mounting orientation only (no pan/tilt channels): tilt_deg
        // 0 = down, 90 = horizontal, 180 = up. Project the landing like the cones.
        let off = yaw_offset(pitch_offset(V3::new(0.0, -1.6, -pos.z), tilt_deg), pan_deg);
        // Effect projector has no live sweep (no pan/tilt channels) → lands exactly.
        let target = aim_target(fx, pos, off, off, tilt_deg, pan_deg, 0.0);
        // Star shows when the throw lands on a surface (down/across); fades toward
        // a bare beam as the aim tilts up into open air ("point at the sky").
        let to_surface = (1.0 - (tilt_deg - 95.0) / 65.0).clamp(0.0, 1.0);
        if let Some(t) = cam.screen(target, rect) {
            // The throw beam, head -> landing (brightness rides dim, no floor).
            if let Some(b) = beam(body, t, s * 0.10, s * 0.22, color, (dim * 0.34).clamp(0.0, 0.45), 0.04) {
                p.add(b);
            }
            // The STAR pattern at the LANDING: a radial burst of thin spikes ending
            // in pattern dots, spun by the gobo rotation. Brightness rides dim (no
            // floor) x how squarely the beam meets a surface.
            const N_SPOKES: usize = 10;
            let rot_phase = st.map(|x| x.gobo_rotation).unwrap_or(0.5);
            let spin = (rot_phase - 0.5) * 2.0; // 0.5 = stop; sign = direction
            let base_ang = (time * spin as f64 * 1.6) as f32;
            let star_r = s * (0.8 + zoom * 1.3) * (0.4 + dim * 1.0);
            let spike_a = (dim * 0.9 * to_surface).clamp(0.0, 0.9);
            p.add(soft_disc(t, star_r * 0.95, tint(color, (dim * 0.4 * to_surface).clamp(0.0, 0.5))));
            for i in 0..N_SPOKES {
                let a = base_ang + i as f32 * (TAU / N_SPOKES as f32);
                let pt = t + Vec2::angled(a) * star_r;
                if let Some(b) = beam(t, pt, s * 0.04, s * 0.09, color, spike_a, 0.0) {
                    p.add(b);
                }
                let spot = s * (0.07 + dim * 0.10);
                p.circle_filled(pt, spot, tint(color, (dim * 0.85 * to_surface).clamp(0.0, 0.95)));
            }
            p.circle_filled(t, s * 0.10, tint(color, (dim * 0.9 * to_surface).clamp(0.0, 0.95)));
        }
        // The fixture head on the truss.
        p.add(soft_disc(body, s * 0.8, tint(color, 0.18 + dim * 0.28)));
        p.circle_filled(body, s * 0.30, tint(color, (0.30 + dim * 0.55).min(1.0)));
        if selected {
            draw_selection_ring(p, body, s);
        }
        draw_fixture_label(p, body + Vec2::new(s * 0.7 + 3.0, -s * 0.7 - 3.0), &fx.name, show_labels);
        return Some((body, s));
    }

    match visual_kind {
        StageVisualKind::Wash => {
            // A wash throws a WIDE soft cone, not a point glow. It has no
            // pan/tilt channels, so the throw follows the mounting aim only
            // (default down). Zoom widens the flood — energy opens it up.
            let off = yaw_offset(
                pitch_offset(
                    V3::new(0.0, 1.0, 1.7_f32.min(pos.z) - pos.z),
                    tilt_deg,
                ),
                pan_deg,
            );
            // Wash has no live sweep (mounting aim only) → lands exactly.
            let target = aim_target(fx, pos, off, off, tilt_deg, pan_deg, 0.0);
            if let Some(t) = cam.screen(target, rect) {
                // wide, soft, short-throw flood — spread opens with zoom
                let end_w = s * (1.9 + zoom * 1.9);
                if let Some(b) = beam(body, t, s * 0.5, end_w, color, 0.12 + dim * 0.24, 0.02) {
                    p.add(b);
                }
                p.add(soft_disc(t, end_w * 1.25, tint(color, 0.06 + dim * 0.16)));
            }
            // body bloom — a wash also flares at the lens itself
            p.add(soft_disc(body, s * 1.6, tint(color, 0.22 + dim * 0.45)));
            p.add(soft_disc(body, s * 0.9, tint(color, 0.35 + dim * 0.55)));
            p.circle_filled(body, s * 0.34, tint(color, 0.6 + dim * 0.4));
        }
        StageVisualKind::Beam => {
            let pan = st.map(|x| x.pan).unwrap_or(0.5);
            let tilt = st.map(|x| x.tilt).unwrap_or(0.5);
            // The head turns about its mounting aim by the live pan/tilt, so
            // it rests on the spot it was dropped on and climbs off the floor
            // when the choreography sends it up.
            let (target, _) = mover_beam_end(fx, pos, pan, tilt, 0.04);
            if let Some(t) = cam.screen(target, rect) {
                // Tight near-parallel column. Zoom only slightly opens it — a
                // beam stays collimated even wide-open, unlike a wash.
                let end_w = s * (0.30 + zoom * 0.35);
                if let Some(b) =
                    beam(body, t, s * 0.16, end_w, color, 0.35 + dim * 0.55, 0.04)
                {
                    p.add(b);
                }
            }
            p.add(soft_disc(body, s * 1.0, tint(color, 0.3 + dim * 0.5)));
            p.circle_filled(body, s * 0.3, tint(color, 0.7 + dim * 0.3));
        }
        StageVisualKind::Gobo => {
            // pan/tilt are already oscillator-driven by the intent layer — the
            // mover sweeps from that one carrier, no renderer-local oscillator.
            // The mounting aim pitches that whole sweep up or down; rotation.y
            // pans it around the hanging axis.
            let pan = st.map(|x| x.pan).unwrap_or(0.5);
            let tilt = st.map(|x| x.tilt).unwrap_or(0.5);
            let rot_phase = st.map(|x| x.gobo_rotation).unwrap_or(0.0);
            // Same head model as the beam arm. `landed` fades the projected
            // pool out as the cone lifts off the floor: a gobo only shows
            // where it meets a surface, in the air it is just the cone.
            let (target, landed) = mover_beam_end(fx, pos, pan, tilt, 0.0);
            if let Some(t) = cam.screen(target, rect) {
                // Mid cone — wider than a beam, tighter than a wash. Zoom
                // scales the projected gobo pool.
                let r = s * (1.15 + zoom * 0.95);
                // COM-499: the gobo pool brightness rides `dim` with LOW floors — the
                // old hardcoded 0.7 ring + 0.65 body core made even a dim galaxy read
                // fully lit ("always on"). Now a low dim genuinely looks faint and a
                // dim of ~0 (gobo arm is gated by the lit check) is gone entirely.
                if let Some(b) =
                    beam(body, t, s * 0.32, r, color, 0.05 + dim * 0.4, 0.02)
                {
                    p.add(b);
                }
                if landed > 0.01 {
                    p.add(soft_disc(t, r * 1.2, tint(color, (0.05 + dim * 0.20) * landed)));
                    p.circle_stroke(t, r, Stroke::new(1.4_f32, tint(color, (0.10 + dim * 0.60) * landed)));
                    // gobo break-up: two crossed slits
                    let slot = st.map(|x| x.gobo_slot).unwrap_or(0) as f32;
                    // gobo_rotation spins the break-up pattern.
                    let ang = slot * 0.5 + rot_phase * TAU;
                    for k in 0..2 {
                        let a = ang + k as f32 * FRAC_PI_2;
                        let d = Vec2::angled(a) * r;
                        p.line_segment([t - d, t + d], Stroke::new(1.2_f32, tint(Color32::BLACK, 0.5 * landed)));
                    }
                }
            }
            p.add(soft_disc(body, s * 0.9, tint(color, 0.06 + dim * 0.6)));
            p.circle_filled(body, s * 0.32, tint(color, 0.12 + dim * 0.78));
        }
        StageVisualKind::Laser => {
            // The macro channel is the device's mode selector: the virtual
            // laser decodes it the same way the real fixture would, so the
            // preview mirrors the rig. Interim decode — the DMX profile
            // system will own real per-fixture macro tables.
            let pattern = laser_pattern(macro_amt);
            let bright = (0.42 + dim * 0.58).clamp(0.20, 1.0);
            // Fan sweep rides the intent layer's pan carrier — the laser has
            // no DMX pan channel, but intent drives one for exactly this.
            let sweep = st.map(|x| (x.pan - 0.5) * 1.1).unwrap_or(0.0);
            draw_laser_fan(
                p, cam, rect, pos, body, s, color, pattern, bright, sweep, tilt_deg, pan_deg,
                fixture_throw(fx),
            );
            // Emitter — a tight bright core the bloom pass flares into a glow.
            p.add(soft_disc(body, s * (0.5 + bright * 0.5), tint(color, 0.28 + bright * 0.5)));
            p.circle_filled(body, s * 0.2, tint(Color32::WHITE, 0.9));
        }
        StageVisualKind::LedPanel => {
            let panel_w = s * fx.scale.x.max(0.6) * 2.2;
            let panel_h = s * fx.scale.z.max(0.8) * 2.4;
            let panel = Rect::from_center_size(body, Vec2::new(panel_w, panel_h));
            p.add(soft_disc(body, panel_w.max(panel_h) * 0.85, tint(color, 0.12 + dim * 0.30)));
            p.rect_filled(panel, 3.0, tint(color, 0.30 + dim * 0.55));
            p.rect_stroke(panel, 3.0, Stroke::new(1.0_f32, tint(Color32::WHITE, 0.42)));
            let cells = 4;
            for i in 1..cells {
                let x = panel.left() + panel.width() * i as f32 / cells as f32;
                p.line_segment([Pos2::new(x, panel.top()), Pos2::new(x, panel.bottom())], Stroke::new(0.7_f32, tint(Color32::BLACK, 0.32)));
                let y = panel.top() + panel.height() * i as f32 / cells as f32;
                p.line_segment([Pos2::new(panel.left(), y), Pos2::new(panel.right(), y)], Stroke::new(0.7_f32, tint(Color32::BLACK, 0.32)));
            }
        }
        StageVisualKind::Strobe => {
            // Omnidirectional flash — a strobe flood has no modelled aim.
            if strobe_amt > 0.1 {
                // STROBE — hard on/off flash, faster as the value rises.
                let rate = 6.0 + strobe_amt * 16.0;
                let on = (time * rate as f64).fract() < 0.5;
                if on {
                    p.add(soft_disc(
                        body,
                        s * 3.2,
                        tint(Color32::from_rgb(235, 240, 255), 0.22),
                    ));
                    p.add(soft_disc(body, s * 1.6, tint(Color32::WHITE, 0.78)));
                    p.circle_filled(body, s * 0.44, Color32::WHITE);
                } else {
                    // dark between flashes — this is what reads as a strobe
                    p.circle_filled(body, s * 0.3, tint(color, 0.22));
                    p.circle_stroke(body, s * 0.4, Stroke::new(1.0_f32, tint(color, 0.4)));
                }
            } else {
                // Lit without strobe — e.g. a blinder boost. Steady white flood.
                p.add(soft_disc(body, s * (1.8 + dim * 1.5), tint(color, 0.14 + dim * 0.4)));
                p.circle_filled(body, s * 0.42, tint(color, 0.55 + dim * 0.45));
                p.circle_stroke(body, s * 0.48, Stroke::new(1.0_f32, Color32::from_rgb(120, 138, 168)));
            }
        }
    }

    if selected {
        draw_selection_ring(p, body, s);
    }
    draw_fixture_label(p, body + Vec2::new(s * 0.7 + 3.0, -s * 0.7 - 3.0), &fx.name, show_labels);
    Some((body, s))
}

/// Synthesize a single-cell `SegmentChannels` from the fixture's main
/// RGB(W) channels. Used when the active mode doesn't expose per-cell
/// segments (e.g. 200W RGB Strobe in 3-channel or 9-channel mode) so the
/// Panel housing still has one big colored zone to render — keeping the
/// housing identical to the multi-cell (24-channel) mode. Returns `None`
/// when the fixture has no RGB channels wired at all.
fn fallback_single_segment(channels: &ChannelMap) -> Option<SegmentChannels> {
    if channels.red.is_none() && channels.green.is_none() && channels.blue.is_none() {
        return None;
    }
    Some(SegmentChannels {
        red: channels.red,
        green: channels.green,
        blue: channels.blue,
        white: channels.white,
    })
}

/// Draw a placeable video screen — a flat surface at the fixture's pose. Slice 1
/// renders a dark placeholder panel with a play glyph (so an unbound screen still
/// reads as a screen); the operator-selected video lands on this same quad in the
/// video pass. The quad spans `±(3·scale.x)` in X and `±(1.8·scale.z)` in Z about
/// the fixture position (a ~16:9 surface at scale 1), facing down the room.
/// Returns the fixture anchor for click/drag hit-testing.
fn draw_screen_fixture(
    p: &egui::Painter,
    cam: &StageCamera,
    rect: Rect,
    fx: &FixturePatch,
    body: Pos2,
    s: f32,
    selected: bool,
    show_labels: bool,
    screen_video: Option<(&str, egui::TextureId)>,
    pipeline: &Arc<Mutex<Option<RenderPipeline>>>,
) -> Option<(Pos2, f32)> {
    let c = V3::of(fx.position);
    let hw = 3.0 * fx.scale.x.max(0.05);
    let hh = 1.8 * fx.scale.z.max(0.05);
    let world = [
        V3::new(c.x - hw, c.y, c.z + hh),
        V3::new(c.x + hw, c.y, c.z + hh),
        V3::new(c.x + hw, c.y, c.z - hh),
        V3::new(c.x - hw, c.y, c.z - hh),
    ];
    let Some(scr) = quad(cam, rect, world) else {
        // Screen is behind the camera — keep the head anchor for hit-testing.
        return Some((body, s));
    };

    // Is this the screen the operator bound a video to (and is a frame ready)?
    let video_tex = screen_video.and_then(|(fid, tex)| (fid == fx.id).then_some(tex));

    let bezel = if selected {
        Color32::from_rgb(150, 200, 255)
    } else {
        Color32::from_rgb(70, 92, 120)
    };

    // The live REACT visual (compel + theme) — the SAME feed the concert LED
    // wall shows — is the screen's primary content, painted on the posed quad
    // via a GL callback. Falls back to a bound local video, then a placeholder.
    let live = pipeline
        .lock()
        .as_ref()
        .map_or(false, |pipe| pipe.vstage_backwall_ready());

    if live {
        let pipeline = pipeline.clone();
        let s4 = scr;
        let cb = egui::PaintCallback {
            rect,
            callback: std::sync::Arc::new(eframe::egui_glow::CallbackFn::new(
                move |info, _painter| {
                    let vp = info.viewport_in_pixels();
                    let ppp = info.pixels_per_point;
                    let w = (vp.width_px as f32).max(1.0);
                    let h = (vp.height_px as f32).max(1.0);
                    let to_ndc = |q: Pos2| -> [f32; 2] {
                        [
                            (q.x * ppp - vp.left_px as f32) / w * 2.0 - 1.0,
                            1.0 - (q.y * ppp - vp.top_px as f32) / h * 2.0,
                        ]
                    };
                    let corners = [to_ndc(s4[0]), to_ndc(s4[1]), to_ndc(s4[2]), to_ndc(s4[3])];
                    let viewport = [
                        vp.left_px as i32,
                        vp.from_bottom_px as i32,
                        vp.width_px as i32,
                        vp.height_px as i32,
                    ];
                    if let Some(pipe) = pipeline.lock().as_ref() {
                        pipe.draw_vstage_backwall(corners, viewport);
                    }
                },
            )),
        };
        p.add(Shape::Callback(cb));
    } else if let Some(tex) = video_tex {
        // Paint the decoded frame onto the posed quad as a textured mesh — UVs
        // top-left → top-right → bottom-right → bottom-left to match `scr`.
        let mut mesh = Mesh::with_texture(tex);
        let uv = [
            Pos2::new(0.0, 0.0),
            Pos2::new(1.0, 0.0),
            Pos2::new(1.0, 1.0),
            Pos2::new(0.0, 1.0),
        ];
        for i in 0..4 {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: scr[i],
                uv: uv[i],
                color: Color32::WHITE,
            });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        p.add(Shape::mesh(mesh));
    } else {
        // Placeholder: dark panel + center play glyph so an unbound screen still
        // reads as a screen.
        let top = Color32::from_rgb(20, 22, 30);
        let bot = Color32::from_rgb(9, 10, 15);
        p.add(grad_quad(scr, [top, top, bot, bot]));
        let center = centroid(&scr);
        let r = ((scr[0] - scr[2]).length() * 0.10).clamp(5.0, 34.0);
        p.add(soft_disc(
            center,
            r * 1.8,
            Color32::from_rgba_unmultiplied(120, 160, 210, 36),
        ));
        let tri = vec![
            center + Vec2::new(-r * 0.42, -r * 0.62),
            center + Vec2::new(r * 0.72, 0.0),
            center + Vec2::new(-r * 0.42, r * 0.62),
        ];
        p.add(Shape::convex_polygon(
            tri,
            Color32::from_rgb(150, 190, 230),
            Stroke::NONE,
        ));
    }

    // Bezel frames the surface in both modes.
    p.add(Shape::convex_polygon(
        scr.to_vec(),
        Color32::TRANSPARENT,
        Stroke::new(2.0_f32, bezel),
    ));

    if selected {
        draw_selection_ring(p, centroid(&scr), s);
    }
    // Label above the top-left corner.
    if let Some(tl) = cam.screen(world[0], rect) {
        draw_fixture_label(p, tl + Vec2::new(0.0, -10.0), &fx.name, show_labels);
    }
    Some((body, s))
}

/// Render a motorized multi-segment pixel-beam bar: a strip of individually-
/// colored emitters along the fixture's (yaw-rotated) long axis, each throwing
/// a tight collimated beam. The whole bar pitches as one rigid unit driven by
/// the Tilt DMX channel — so all 8 beams share the same throw direction but
/// can carry independent per-cell colors. Cell colors are read straight from
/// the DMX frame so the on-screen bar matches the wire exactly.
///
/// Hardware can sweep 360°; the ATLABASE profile's `tiltSetting.width = 0.5`
/// caps practical use at 180°, which matches what we see on stage. Once the
/// parser plumbs `tiltSetting` through to the profile model, the hardcoded
/// 180° below should switch to `width * 360`.
fn draw_segment_bar(
    p: &egui::Painter,
    cam: &StageCamera,
    rect: Rect,
    fx: &FixturePatch,
    segments: &[SegmentChannels],
    dmx_frame: Option<&DmxFrame>,
    selected: bool,
    show_labels: bool,
) -> Option<(Pos2, f32)> {
    let (body, depth) = cam.project(V3::of(fx.position), rect)?;
    let s = (160.0 / depth).clamp(3.0, 14.0);
    let count = segments.len();
    let pos = V3::of(fx.position);

    // The cells lie along the world X axis, yaw-rotated by the mounting pan so
    // an angled bar reads correctly. Length scales with the fixture's X scale.
    let bar_len = (3.0 * fx.scale.x.max(0.4)).clamp(1.5, 9.0);
    let axis = yaw_offset(V3::new(1.0, 0.0, 0.0), fx.rotation.y);

    let read = |addr: Option<u16>| -> u8 {
        addr.and_then(|a| dmx_frame.and_then(|f| f.get_channel(a)))
            .unwrap_or(0)
    };
    let cell_color = |seg: &SegmentChannels| -> Color32 {
        // Warm-white folds into RGB as a warm flood, matching the LED mix.
        let ww = read(seg.white) as f32 * 0.6;
        Color32::from_rgb(
            (read(seg.red) as f32 + ww).min(255.0) as u8,
            (read(seg.green) as f32 + ww * 0.85).min(255.0) as u8,
            (read(seg.blue) as f32 + ww * 0.55).min(255.0) as u8,
        )
    };

    // DMX-driven tilt: center (0.5) = aim straight forward at audience;
    // ends sweep ±90° around the bar's length axis (180° practical range).
    // Falls back to centered if the Tilt channel is unread or unmapped.
    let tilt_norm = fx
        .channels
        .tilt
        .and_then(|a| dmx_frame.and_then(|f| f.get_channel(a)))
        .map(|v| v as f32 / 255.0)
        .unwrap_or(0.5);
    let tilt_sweep_deg = 180.0_f32; // tiltSetting.width = 0.5 -> 180° practical
    let dmx_pitch_deg = (tilt_norm - 0.5) * tilt_sweep_deg;
    // Mounting aim composes with DMX pitch: rig hang sets the home pose, DMX
    // pitches around the (yaw-rotated) bar length axis from there.
    let mounting_tilt = fx.rotation.x;
    let mounting_pan = fx.rotation.y;
    // Long throw so the beam columns visibly reach across the venue, like
    // the real fixture's reference photos — short throws read as wash, not beam.
    let throw_dist = 6.0_f32;
    let home_forward = V3::new(0.0, throw_dist, 0.0);
    let throw_vec = yaw_offset(
        pitch_offset(home_forward, mounting_tilt + dmx_pitch_deg),
        mounting_pan,
    );

    let mut centers: Vec<(Pos2, V3, Color32)> = Vec::with_capacity(count);
    for (i, seg) in segments.iter().enumerate() {
        let frac = if count > 1 {
            i as f32 / (count - 1) as f32 - 0.5
        } else {
            0.0
        };
        let world = V3::new(
            pos.x + axis.x * frac * bar_len,
            pos.y + axis.y * frac * bar_len,
            pos.z + axis.z * frac * bar_len,
        );
        if let Some(c) = cam.screen(world, rect) {
            centers.push((c, world, cell_color(seg)));
        }
    }
    if centers.is_empty() {
        return Some((body, s));
    }

    // Housing bar behind the cells.
    if let (Some(first), Some(last)) = (centers.first(), centers.last()) {
        p.line_segment(
            [first.0, last.0],
            Stroke::new((s * 0.7).max(2.0), Color32::from_rgb(18, 21, 28)),
        );
    }

    let cell_r = if centers.len() >= 2 {
        ((centers[1].0 - centers[0].0).length() * 0.42).clamp(2.0, s * 1.4)
    } else {
        s * 0.6
    };

    // Per-cell beams: each emitter throws a tight collimated column in the
    // shared bar-tilt direction. Drawn before the emitter discs so the
    // emitter sits as the bright source at the beam's root. Beam columns
    // are intentionally narrow (close to the emitter width) so they read as
    // tight searchlight columns like the SHEHDS LED Beam 8x12W references,
    // not as wide wash cones.
    for (c_screen, c_world, color) in &centers {
        let lit = color.r() as u16 + color.g() as u16 + color.b() as u16 > 24;
        if !lit {
            continue;
        }
        let target_world = V3::new(
            c_world.x + throw_vec.x,
            c_world.y + throw_vec.y,
            (c_world.z + throw_vec.z).max(0.02),
        );
        if let Some(t_screen) = cam.screen(target_world, rect) {
            // Wide outer atmospheric haze (low alpha, fades faster than the
            // core) — gives the beam a visible halo through "air".
            if let Some(haze) = beam(
                *c_screen,
                t_screen,
                cell_r * 1.05,
                cell_r * 1.55,
                *color,
                0.18,
                0.01,
            ) {
                p.add(haze);
            }
            // Tight bright core — the searchlight column itself. Stays
            // narrow end-to-end (slight outward fan), fades more gradually
            // than the haze so the beam visibly reaches the far end.
            if let Some(core) = beam(
                *c_screen,
                t_screen,
                cell_r * 0.45,
                cell_r * 0.85,
                *color,
                0.78,
                0.08,
            ) {
                p.add(core);
            }
        }
    }

    for (c, _, color) in &centers {
        let lit = color.r() as u16 + color.g() as u16 + color.b() as u16 > 24;
        if lit {
            // Emitter lens: bright core flares into a halo via soft_disc.
            p.add(soft_disc(*c, cell_r * 2.1, tint(*color, 0.5)));
            p.circle_filled(*c, cell_r, *color);
        } else {
            // dark cell — faint housing dot so the bar stays visible unlit
            p.circle_filled(*c, cell_r * 0.7, Color32::from_rgb(20, 24, 32));
            p.circle_stroke(*c, cell_r * 0.8, Stroke::new(1.0_f32, Color32::from_rgb(50, 60, 78)));
        }
    }

    // Hit radius covers the FULL visual extent — body is at the fixture
    // anchor but the cells spread out along the bar's length, so without
    // padding only a tiny disc at the anchor is grabbable. We extend the
    // pick radius to the farthest cell + the cell's own radius so any
    // emitter the operator can see is clickable + draggable.
    let half_extent = centers
        .iter()
        .map(|(c, _, _)| (*c - body).length())
        .fold(0.0_f32, f32::max);
    let hit_r = (half_extent + cell_r).max(s.max(cell_r));
    if selected {
        draw_selection_ring(p, body, hit_r);
    }
    draw_fixture_label(p, body + Vec2::new(s * 0.7 + 3.0, -s * 0.7 - 3.0), &fx.name, show_labels);
    Some((body, hit_r))
}

/// Render a static segmented panel: a rectangular housing with a dense LED
/// dot matrix on its face, divided into N addressable color cells (e.g.
/// SHEHDS 200W RGB Strobe 8-segment in 24-channel mode). Each cell is
/// subdivided into a sub-grid of individual LED dots so the panel reads as
/// the real high-resolution matrix, not eight solid color rectangles —
/// patterns the cue layer pushes between cells flow through verbatim.
///
/// Three brightness behaviors track the real fixture's huge luminance range:
///   1. Per-dot lens emits at its cell color, alpha proportional to dot
///      luminance (so a dim panel shows the matrix texture visible at low
///      intensity; full power blasts at saturation).
///   2. Mean-color **room flood** halo radiates outward from the panel
///      proportional to total brightness, simulating the panel washing the
///      stage in its color when it's fully lit.
///   3. Master strobe (SHUTTER via intent) overrides every dot to white and
///      pumps an additional bright burst on each flash beat — same rate
///      curve as the omnidirectional StageVisualKind::Strobe branch.
fn draw_segment_panel(
    p: &egui::Painter,
    cam: &StageCamera,
    rect: Rect,
    fx: &FixturePatch,
    st: Option<&FixtureIntentState>,
    segments: &[SegmentChannels],
    dmx_frame: Option<&DmxFrame>,
    time: f64,
    selected: bool,
    show_labels: bool,
) -> Option<(Pos2, f32)> {
    let (body, depth) = cam.project(V3::of(fx.position), rect)?;
    let s = (160.0 / depth).clamp(3.0, 14.0);
    let count = segments.len().max(1);

    let read = |addr: Option<u16>| -> u8 {
        addr.and_then(|a| dmx_frame.and_then(|f| f.get_channel(a)))
            .unwrap_or(0)
    };
    // Master dimmer multiplier — only relevant when the fixture has a real
    // Dimmer channel separate from per-cell color (the 9-channel mode of
    // the 200W RGB Strobe). In 24-channel mode `apply_segmented_fixture`
    // hardcodes the dimmer channel to 255 and per-cell DMX values already
    // carry the intent dimmer, so this evaluates to 1.0 and is a no-op.
    let master_dim = fx
        .channels
        .dimmer
        .and_then(|a| dmx_frame.and_then(|f| f.get_channel(a)))
        .map(|v| v as f32 / 255.0)
        .unwrap_or(1.0);
    let cell_color = |seg: &SegmentChannels| -> Color32 {
        let ww = read(seg.white) as f32 * 0.6;
        let r = (read(seg.red) as f32 + ww).min(255.0) * master_dim;
        let g = (read(seg.green) as f32 + ww * 0.85).min(255.0) * master_dim;
        let b = (read(seg.blue) as f32 + ww * 0.55).min(255.0) * master_dim;
        Color32::from_rgb(r as u8, g as u8, b as u8)
    };

    // Lay segments out in a chunky 2D grid so the housing reads as a panel,
    // not a thin strip. For an even count >= 4 use 2 rows × N/2 cols (e.g.
    // 8-seg -> 2×4, matching real-world wide RGB strobe panels). Lower counts
    // stay in a single row to avoid sliver cells.
    let (cols, rows) = if count >= 4 && count % 2 == 0 {
        (count / 2, 2usize)
    } else {
        (count, 1usize)
    };

    // Aim for a chunky ~2:1 housing aspect — visibly thicker than a beam bar.
    // Width tracks col count and X scale; height tracks row count and Z scale
    // but is floored so a 1-row layout still reads as a rectangle (not a line).
    let cell_unit = s * 0.9;
    let panel_w = (cols as f32 * cell_unit * 1.15 * fx.scale.x.max(0.8)).max(s * 3.0);
    let panel_h =
        (rows as f32 * cell_unit * 1.35 * fx.scale.z.max(0.6)).max(panel_w * 0.32);
    let panel = Rect::from_center_size(body, Vec2::new(panel_w, panel_h));

    // Mean cell color drives the body bloom — the audience reads one glow
    // even when the rig fires per-cell.
    let mut sum_r = 0u32;
    let mut sum_g = 0u32;
    let mut sum_b = 0u32;
    for seg in segments {
        let c = cell_color(seg);
        sum_r += c.r() as u32;
        sum_g += c.g() as u32;
        sum_b += c.b() as u32;
    }
    let n = count as u32;
    let mean = Color32::from_rgb((sum_r / n) as u8, (sum_g / n) as u8, (sum_b / n) as u8);
    let lit_total = sum_r + sum_g + sum_b;

    let strobe_amt = st.map(|x| x.strobe).unwrap_or(0.0).clamp(0.0, 1.0);
    let flash_on = if strobe_amt > 0.1 {
        let rate = 6.0 + strobe_amt * 16.0;
        (time * rate as f64).fract() < 0.5
    } else {
        false
    };

    // Total brightness drives the room flood: a panel ramps from "lens just
    // visible" to "blasting the whole stage in its color" across the DMX
    // range. Compare to the theoretical max (3 channels * 255 * cell count).
    let max_lit = (n * 3 * 255) as f32;
    let energy = (lit_total as f32 / max_lit).clamp(0.0, 1.0);

    // Room flood — large outward halo in the panel's mean color. This is
    // what gives the panel the wash-the-stage character from the references;
    // tiny in dim states, dominant when the panel is fully open.
    if energy > 0.02 || flash_on {
        let flood_color = if flash_on { Color32::WHITE } else { mean };
        let flood_r = panel_w.max(panel_h) * (1.4 + energy * 2.0 + if flash_on { 1.6 } else { 0.0 });
        let flood_a = if flash_on { 0.55 } else { 0.10 + energy * 0.45 };
        p.add(soft_disc(body, flood_r, tint(flood_color, flood_a)));
        // Inner core glow tighter than the flood — reads as the panel face.
        let core_r = panel_w.max(panel_h) * (0.75 + energy * 0.4);
        let core_a = if flash_on { 0.7 } else { 0.18 + energy * 0.5 };
        p.add(soft_disc(body, core_r, tint(flood_color, core_a)));
    }

    // Outer chassis (slightly larger than the lens area) reads as the
    // mounting yoke / housing frame, giving the panel visible depth.
    let chassis = panel.expand2(Vec2::new(panel_w * 0.04, panel_h * 0.12));
    p.rect_filled(chassis, 4.0, Color32::from_rgb(10, 12, 16));
    p.rect_stroke(chassis, 4.0, Stroke::new(1.2_f32, Color32::from_rgb(70, 80, 100)));

    // Lens area: dark backing, then the LED dot matrix.
    p.rect_filled(panel, 3.0, Color32::from_rgb(10, 12, 18));
    p.rect_stroke(panel, 3.0, Stroke::new(1.0_f32, Color32::from_rgb(55, 65, 85)));

    // Each segment is subdivided into sub_w × sub_h LED dots. The dots are
    // sized so the matrix fills the lens with a small bezel gap between
    // dots — that's what gives the panel the "many small LEDs" look from
    // the reference photos. Sub-grid count is tuned for visual density at
    // typical screen sizes; if it overshoots, the dot radius clamps.
    let inset = 1.6_f32;
    let inner_w = (panel.width() - inset * 2.0).max(0.0);
    let inner_h = (panel.height() - inset * 2.0).max(0.0);
    let sub_w: usize = 4;
    let sub_h: usize = 4;
    let cell_w_px = inner_w / cols as f32;
    let cell_h_px = inner_h / rows as f32;
    let dot_pitch_x = cell_w_px / sub_w as f32;
    let dot_pitch_y = cell_h_px / sub_h as f32;
    let dot_r = (dot_pitch_x.min(dot_pitch_y) * 0.40).max(0.6);

    for (seg_i, seg) in segments.iter().enumerate() {
        let col = seg_i % cols;
        let row = seg_i / cols;
        let base_color = cell_color(seg);
        let final_color = if flash_on { Color32::WHITE } else { base_color };
        let cell_lit = (final_color.r() as u16
            + final_color.g() as u16
            + final_color.b() as u16)
            > 24;
        // Cell luminance (0-1) — drives per-dot alpha so a dim cell shows
        // the matrix faintly instead of looking entirely off, and a full
        // cell saturates.
        let cell_lum = ((final_color.r() as u16
            + final_color.g() as u16
            + final_color.b() as u16) as f32
            / (3.0 * 255.0))
            .clamp(0.0, 1.0);

        let cell_x0 = panel.left() + inset + cell_w_px * col as f32;
        let cell_y0 = panel.top() + inset + cell_h_px * row as f32;
        for sy in 0..sub_h {
            for sx in 0..sub_w {
                let cx = cell_x0 + dot_pitch_x * (sx as f32 + 0.5);
                let cy = cell_y0 + dot_pitch_y * (sy as f32 + 0.5);
                let dot = Pos2::new(cx, cy);
                if cell_lit {
                    // Tiny deterministic alpha jitter per dot so the matrix
                    // doesn't look like a perfect computer grid — real LED
                    // panels show small per-LED variation.
                    let h = dot_jitter_hash(sx, sy, seg_i);
                    let jitter = h as f32 / 255.0 * 0.12 - 0.06;
                    let a = (0.45 + cell_lum * 0.55 + jitter).clamp(0.15, 1.0);
                    if cell_lum > 0.6 || flash_on {
                        // Bright: dot halo + saturated core.
                        p.add(soft_disc(dot, dot_r * 1.8, tint(final_color, a * 0.55)));
                    }
                    p.circle_filled(dot, dot_r, tint(final_color, a));
                } else {
                    // Unlit dot: just the dark lens divot so the matrix
                    // texture stays visible.
                    p.circle_filled(dot, dot_r * 0.55, Color32::from_rgb(22, 26, 34));
                }
            }
        }
    }

    // Half-diagonal of the (slightly outset) chassis so the operator can grab
    // anywhere on the visible panel, not just a tiny disc at its center.
    let half_w = chassis.width() * 0.5;
    let half_h = chassis.height() * 0.5;
    let hit_r = (half_w * half_w + half_h * half_h).sqrt();
    if selected {
        draw_selection_ring(p, body, hit_r);
    }
    draw_fixture_label(
        p,
        Pos2::new(chassis.right() + 4.0, chassis.top() - 3.0),
        &fx.name,
        show_labels,
    );
    Some((body, hit_r))
}

/// Per-dot byte (0-255) for the LED-matrix alpha jitter: a fixed scramble of
/// the dot's sub-grid position and its cell index. The multiplier on the cell
/// index is a hash constant that only works by wrapping — a plain `*` runs past
/// `u32::MAX` from the third cell on, which a debug build turns into a panic
/// the moment a multi-cell panel lights up.
fn dot_jitter_hash(sx: usize, sy: usize, seg_i: usize) -> u32 {
    (sx as u32)
        .wrapping_mul(7919)
        .wrapping_add((sy as u32).wrapping_mul(104_729))
        .wrapping_add((seg_i as u32).wrapping_mul(2_654_435_761))
        & 0xff
}

fn draw_fixture_label(p: &egui::Painter, pos: Pos2, label: &str, show: bool) {
    if !show {
        return;
    }
    let font = egui::FontId::proportional(10.0);
    let width = label.len() as f32 * 5.8 + 10.0;
    let rect = Rect::from_min_size(pos + Vec2::new(-4.0, -14.0), Vec2::new(width, 16.0));
    p.rect_filled(rect, 3.0, Color32::from_rgba_unmultiplied(4, 8, 14, 190));
    p.rect_stroke(rect, 3.0, Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(90, 120, 160, 130)));
    p.text(
        pos,
        egui::Align2::LEFT_BOTTOM,
        label,
        font,
        Color32::from_rgb(190, 214, 240),
    );
}

/// A real laser is a single-colour device — green/red/yellow are the common
/// types; the demo rig is green. The macro channel drives the *pattern*, not
/// the colour, so a laser keeps one fixed hue regardless of the cue's RGB
/// intent. Per-fixture colour becomes a profile attribute when the DMX
/// profile system lands; a fixed colour is the right interim.
const LASER_COLOR: Color32 = Color32::from_rgb(72, 255, 102);

/// Open-white a color-capable fixture shows when its color engine is driven
/// to zero — a real head idles near white/amber, not a saturated kind hue.
const OPEN_WHITE: Color32 = Color32::from_rgb(255, 244, 224);

/// The color a fixture actually emits, derived from its real DMX color
/// hardware (the patched `ChannelMap`) driven by the live intent values —
/// not a generic per-kind tint. RGB(W) fixtures additively mix their color
/// channels; color-wheel fixtures decode the wheel slot; fixtures with no
/// color control fall back to a kind tint only as a last resort.
fn fixture_color(
    fx: &FixturePatch,
    kind: StageVisualKind,
    st: Option<&FixtureIntentState>,
) -> Color32 {
    // UV mode: a fixture with a UV channel glows blacklight violet — the UV
    // emitter owns the colour while the RGB engine is forced dark. Needed for
    // UV emitters with NO RGB channels (the GigBAR strobe pod's ch22), which
    // would otherwise fall through to the OPEN_WHITE dimmer branch. intent.uv
    // is only ever non-zero in UV mode, so normal rendering is untouched.
    if fx.channels.uv.is_some() {
        if let Some(s) = st {
            if s.uv > 0.001 {
                return Color32::from_rgb(u8f(0.45 * s.uv), 0, u8f(0.8 * s.uv));
            }
        }
    }
    // A single-colour laser is one fixed hue. But an RGB scan laser — the SHEHDS 3W
    // does Red / Green / Blue + colour mixing and carries red/green/blue channels —
    // takes the harmony RGB the intent already computes (full-spectrum hue, per-fixture
    // dissonance fan, saturated), like any RGB fixture. Only a laser with NO RGB
    // channels falls back to the fixed LASER_COLOR.
    if matches!(kind, StageVisualKind::Laser) {
        let ch = &fx.channels;
        let has_rgb = ch.red.is_some() || ch.green.is_some() || ch.blue.is_some();
        if has_rgb {
            if let Some(s) = st {
                let r = s.red.clamp(0.0, 1.0);
                let g = s.green.clamp(0.0, 1.0);
                let b = s.blue.clamp(0.0, 1.0);
                if r + g + b > 0.02 {
                    return Color32::from_rgb(u8f(r), u8f(g), u8f(b));
                }
            }
        }
        return LASER_COLOR;
    }

    let ch = &fx.channels;
    let has_rgb = ch.red.is_some() || ch.green.is_some() || ch.blue.is_some();
    let has_white = ch.white.is_some();
    let has_wheel = ch.color_wheel.is_some();

    if let Some(s) = st {
        // 1. Additive RGB(W) engine — what an LED fixture's color channels emit.
        //    White folds into all three so a white-channel push reads as a flood
        //    (this is what a blinder / CW LED does).
        if has_rgb {
            let w = if has_white { s.white.clamp(0.0, 1.0) } else { 0.0 };
            let r = (s.red + w).clamp(0.0, 1.0);
            let g = (s.green + w).clamp(0.0, 1.0);
            let b = (s.blue + w).clamp(0.0, 1.0);
            if r + g + b > 0.02 {
                return Color32::from_rgb(u8f(r), u8f(g), u8f(b));
            }
            // Color-capable but driven to zero — idle near white, not a
            // saturated kind hue, so the preview reads true to the hardware.
            return OPEN_WHITE;
        }
        // COM-499: the GalaxyJet is a colour-wheel device whose profile gels are all
        // white, but Koob wants it to read the harmony in FULL colour like the washes —
        // not the warm-clamped wheel decode. Use the harmony RGB the intent already
        // computes (color_for_fixture: full-spectrum hue, look-rotating, saturated).
        // Scoped to the GalaxyBeam; the Obsession is a real colour-wheel device and
        // keeps its wheel decode below.
        if renders_as_galaxy_effect(fx) {
            let r = s.red.clamp(0.0, 1.0);
            let g = s.green.clamp(0.0, 1.0);
            let b = s.blue.clamp(0.0, 1.0);
            if r + g + b > 0.02 {
                return Color32::from_rgb(u8f(r), u8f(g), u8f(b));
            }
            return OPEN_WHITE;
        }
        // 2. Color-wheel fixture (no RGB) — decode the wheel slot to a hue.
        if has_wheel {
            // The profile resolved a real gel: show that gel. Lifted to full
            // brightness — a dark gel still throws a bright beam of its colour,
            // and the beam's level is drawn from the dimmer separately.
            if let Some(rgb) = s.wheel_rgb {
                let peak = rgb[0].max(rgb[1]).max(rgb[2]).max(1e-3);
                return Color32::from_rgb(u8f(rgb[0] / peak), u8f(rgb[1] / peak), u8f(rgb[2] / peak));
            }
            return color_wheel_hue(s.color_wheel);
        }
        // 3. White / dimmer-only fixture — open white when it has a dimmer.
        if has_white || ch.dimmer.is_some() {
            return OPEN_WHITE;
        }
    }

    // No channel info or no intent yet — per-kind tint as a last resort.
    match kind {
        StageVisualKind::Wash => Color32::from_rgb(90, 150, 255),
        StageVisualKind::Beam => Color32::from_rgb(255, 224, 130),
        StageVisualKind::Gobo => Color32::from_rgb(255, 186, 96),
        StageVisualKind::Laser => LASER_COLOR,
        StageVisualKind::Strobe => Color32::from_rgb(236, 240, 255),
        StageVisualKind::LedPanel => Color32::from_rgb(80, 160, 255),
    }
}

/// Interim generic color-wheel decode — even slices of the 0..255 channel
/// range map to a common moving-head wheel order (open, red, amber, yellow,
/// green, blue, cyan, magenta). The DMX profile system will carry real
/// per-fixture wheel-slot tables later; until then this mirrors the most
/// common ordering so a wheel fixture shows plausible color.
fn color_wheel_hue(slot: u8) -> Color32 {
    const WHEEL: [Color32; 8] = [
        Color32::from_rgb(255, 248, 230), // open / white
        Color32::from_rgb(255, 60, 50),   // red
        Color32::from_rgb(255, 150, 40),  // amber
        Color32::from_rgb(255, 225, 70),  // yellow
        Color32::from_rgb(60, 220, 90),   // green
        Color32::from_rgb(50, 120, 255),  // blue
        Color32::from_rgb(40, 210, 230),  // cyan
        Color32::from_rgb(225, 70, 220),  // magenta
    ];
    let idx = ((slot as usize) * WHEEL.len() / 256).min(WHEEL.len() - 1);
    WHEEL[idx]
}

/// Laser macro decode — the virtual device reads the macro channel exactly
/// as the real fixture would. The low half of the range is discrete-beam
/// patterns, the high half is flat (wedge) patterns. This is the interim
/// table; the DMX profile system will carry real per-fixture macro maps.
enum LaserPattern {
    /// Fan of discrete sharp beams. `amount` 0..1 = sparse..dense.
    Beam(f32),
    /// Fan of solid wedges with dark gaps. `amount` 0..1 = few..many.
    Flat(f32),
}

fn laser_pattern(macro_amt: u8) -> LaserPattern {
    // The intent layer feeds macro 1..=15. Split it: low = beam, high = flat.
    if macro_amt <= 7 {
        LaserPattern::Beam((macro_amt.saturating_sub(1) as f32 / 6.0).clamp(0.0, 1.0))
    } else {
        LaserPattern::Flat(((macro_amt - 8) as f32 / 7.0).clamp(0.0, 1.0))
    }
}

/// Draw a laser fan from the emitter. Beam = a spread of discrete sharp
/// beams; flat = a spread of solid wedges with dark gaps between them. Both
/// fan along a centre that points down and downstage, rotated by `sweep`.
#[allow(clippy::too_many_arguments)]
fn draw_laser_fan(
    p: &egui::Painter,
    cam: &StageCamera,
    rect: Rect,
    pos: V3,
    body: Pos2,
    s: f32,
    color: Color32,
    pattern: LaserPattern,
    bright: f32,
    sweep: f32,
    tilt_deg: f32,
    pan_deg: f32,
    throw: Option<f32>,
) {
    // Fan basis: `centre` is the fixture's mounting aim, `open` (stage-X) the
    // axis it fans along. Both unit and orthogonal, so `centre*cos + open*sin`
    // is a true rotation in the fan plane.
    //
    // Each aim preset is its own deliberate direction — not one base rotated
    // in 90° steps (the down aim's forward bias would land "Up" only ~58°
    // above horizontal, indistinguishable from "Horizontal").
    let aim = aim_preset(tilt_deg);
    let centre_base = match aim {
        // Down + forward — the floor fan spreads toward the audience.
        AimPreset::Down => V3::new(0.0, -0.62, -1.0),
        // Forward toward the audience — close to level, lifted just enough
        // off the floor that the fan plane is not edge-on to the camera.
        AimPreset::Horizontal => V3::new(0.0, -1.0, 0.40),
        // Straight up — a vertical column to the sky.
        AimPreset::Up => V3::new(0.0, 0.0, 1.0),
    }
    .norm();
    // Pan rotates centre + open together around the hanging axis, so the fan
    // plane stays orthogonal after rotation (Left / FOH / Right aim presets).
    let centre = yaw_offset(centre_base, pan_deg);
    let open = yaw_offset(V3::new(1.0, 0.0, 0.0), pan_deg);
    // A wide fan opening along stage-X rakes its edge beams down toward
    // horizontal. Fine aimed down — the edges still sweep the floor — but it
    // leaves an up-aimed fan reading as a shallow horizontal spray, because
    // its widest, most visible beams sit barely above level. For the up aim,
    // tighten the fan into a near-vertical column and pull its reach back into
    // frame, so every beam — edges included — visibly climbs.
    let (spread_scale, reach_scale) = match aim {
        AimPreset::Up => (0.5_f32, 0.6_f32),
        _ => (1.0, 1.0),
    };
    let ray = |theta: f32, reach: f32| -> V3 {
        let c = theta.cos();
        let sn = theta.sin();
        V3::new(
            pos.x + (centre.x * c + open.x * sn) * reach,
            pos.y + (centre.y * c + open.y * sn) * reach,
            (pos.z + (centre.z * c + open.z * sn) * reach).max(0.04),
        )
    };

    match pattern {
        LaserPattern::Beam(amt) => {
            let n = (9.0 + amt * 9.0) as usize; // 9..18 beams
            let spread = (46.0 + amt * 68.0).to_radians() * spread_scale; // 46..114 deg
            let a_src = 0.5 + bright * 0.5;
            for k in 0..n {
                let frac = k as f32 / (n - 1).max(1) as f32 - 0.5;
                let end = ray(frac * spread + sweep, throw.unwrap_or(6.2) * reach_scale);
                if let Some(e) = cam.screen(end, rect) {
                    if let Some(bl) = beam(body, e, s * 0.05, s * 0.085, color, a_src, 0.05) {
                        p.add(bl);
                    }
                }
            }
        }
        LaserPattern::Flat(amt) => {
            let n = (5.0 + amt * 4.0) as usize; // 5..9 wedges
            let spread = (62.0 + amt * 74.0).to_radians() * spread_scale; // 62..136 deg
            let slot = spread / n as f32;
            let half = slot * 0.34; // < 0.5 of a slot -> dark gaps between wedges
            let a_src = tint(color, 0.34 + bright * 0.42);
            let a_arc = tint(color, 0.0);
            for k in 0..n {
                let theta = (k as f32 / (n - 1).max(1) as f32 - 0.5) * spread + sweep;
                let ea = ray(theta - half, throw.unwrap_or(5.6) * reach_scale);
                let eb = ray(theta + half, throw.unwrap_or(5.6) * reach_scale);
                if let (Some(pa), Some(pb)) = (cam.screen(ea, rect), cam.screen(eb, rect)) {
                    // Solid wedge: bright at the emitter, fading out at the arc.
                    let mut mesh = Mesh::default();
                    mesh.colored_vertex(body, a_src);
                    mesh.colored_vertex(pa, a_arc);
                    mesh.colored_vertex(pb, a_arc);
                    mesh.add_triangle(0, 1, 2);
                    p.add(Shape::mesh(mesh));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The endpoint-drag derives (tilt, pan) from an aim direction via
    /// `aim_to_rotation`, and the renderer turns (tilt, pan) back into a
    /// direction via `aim_dir`. They must be exact inverses or the beam would
    /// drift away from where the operator dropped the handle.
    #[test]
    fn aim_dir_round_trips_through_rotation() {
        for (tilt, pan) in [
            (0.0_f32, 0.0_f32),
            (45.0, 30.0),
            (90.0, -90.0),
            (135.0, 170.0),
            (60.0, -45.0),
            (180.0, 0.0),
        ] {
            let d = aim_dir(tilt, pan);
            let (t2, p2) = aim_to_rotation(d);
            assert!((t2 - tilt).abs() < 0.01, "tilt {tilt} -> {t2}");
            // Pan is undefined aiming straight down/up (sin(tilt)=0) — skip there.
            if tilt > 0.5 && tilt < 179.5 {
                let dp = ((p2 - pan + 540.0) % 360.0) - 180.0;
                assert!(dp.abs() < 0.01, "pan {pan} -> {p2}");
            }
        }
    }

    /// A mover's beam turns about the head by real angles: at rest it lands
    /// exactly on the spot it was placed, a raised tilt lifts it off the floor
    /// and into the air, and a beam that meets the floor stops there.
    #[test]
    fn a_mover_beam_rests_on_its_spot_and_climbs_with_tilt() {
        let mut patch = DmxPatch::demo_virtual_stage(0, 1);
        let fx = &mut patch.fixtures[0];
        // The concert 7R: 11 m up, aimed 42.5° off straight down, thrown to the floor.
        fx.position = StageVec3 { x: -1.7, y: 0.9, z: 11.3 };
        fx.rotation = StageVec3 { x: 42.5, y: 86.0, z: 0.0 };
        fx.scale.y = 15.4;
        let pos = V3::of(fx.position);

        let (rest, landed) = mover_beam_end(fx, pos, 0.5, 0.5, 0.04);
        let placed = aim_handle_world(fx);
        let off = ((rest.x - placed.x).powi(2) + (rest.y - placed.y).powi(2) + (rest.z - placed.z).powi(2)).sqrt();
        assert!(off < 0.3, "at rest the beam is on its placed spot: {off} m off");
        assert!(landed > 0.9 && rest.z < 0.2, "and that spot is on the floor");

        // Tilt up through the levels: the far end rises above the head.
        let (crowd, _) = mover_beam_end(fx, pos, 0.5, 0.74, 0.04);
        let (sky, in_air) = mover_beam_end(fx, pos, 0.5, 0.95, 0.04);
        assert!(crowd.z > rest.z + 5.0, "the crowd level is well off the floor: z {}", crowd.z);
        assert!(sky.z > pos.z + 5.0, "the sky level points above the head: z {} vs head {}", sky.z, pos.z);
        assert_eq!(in_air, 0.0, "nothing lands when the beam is in the air");

        // Steeper than the placed aim: still stops at the floor, never below it.
        let (down, hit) = mover_beam_end(fx, pos, 0.5, 0.35, 0.04);
        assert!((down.z - 0.04).abs() < 1e-3 && hit == 1.0, "stops on the deck: z {}", down.z);

        // Pan swings the landing sideways across the stage, not a metre.
        let (left, _) = mover_beam_end(fx, pos, 0.2, 0.5, 0.04);
        let (right, _) = mover_beam_end(fx, pos, 0.8, 0.5, 0.04);
        let swing = ((left.x - right.x).powi(2) + (left.y - right.y).powi(2)).sqrt();
        assert!(swing > 5.0, "pan covers real ground: {swing} m");
    }

    /// `scale.y` doubles as the per-fixture throw, with the default 1.0 meaning
    /// "auto". Verify the sentinel + clamp so an untouched fixture stays on auto
    /// and a placed one reports its (clamped) length.
    #[test]
    fn fixture_throw_sentinel_and_clamp() {
        let mut patch = DmxPatch::demo_virtual_stage(0, 1);
        let fx = &mut patch.fixtures[0];
        fx.scale.y = 1.0; // default
        assert_eq!(fixture_throw(fx), None, "default scale.y is auto");
        fx.scale.y = 0.5; // below sentinel
        assert_eq!(fixture_throw(fx), None, "sub-sentinel is auto");
        fx.scale.y = 8.0;
        assert_eq!(fixture_throw(fx), Some(8.0));
        fx.scale.y = 1000.0;
        assert_eq!(fixture_throw(fx), Some(THROW_MAX), "clamped to max");
    }
}

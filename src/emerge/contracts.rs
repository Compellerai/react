//! Backend-neutral interop contracts for REACT Virtual Stage.
//!
//! These structs are intentionally renderer-free. Art-Net, OLA, and the
//! virtual stage should all consume the same contracts: CDJ/proDJ/deck state
//! drives video/performance context and can infer ThemeIntent, Compeller harmonic
//! field algorithms combine that theme intent plus performance state into visual
//! behavior, and `DmxFrame`s remain an optional output/interop path for physical
//! devices when needed. This remains an interop boundary, not the product framing.
//!
//! MVP semantics:
//! - DMX channel definitions are simple one-channel controls.
//! - Fixture geometry is normalized stage-space position/rotation/scale.
//! - Virtual state exposes common controls needed by a useful first stage.
//!
//! Future GDTF-rich semantics:
//! - Multi-channel attributes, physical units, beam geometry, wheels, macros,
//!   calibration, and manufacturer personality files should extend these
//!   contracts without coupling them to renderer UI.

use serde::{Deserialize, Serialize};

pub const DMX_CHANNEL_COUNT: usize = 512;
pub const DMX_MIN_CHANNEL: u16 = 1;
pub const DMX_MAX_CHANNEL: u16 = 512;

pub type DmxPayload = Vec<u8>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmxFrame {
    pub universe: u16,
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub data: DmxPayload,
    pub blackout: bool,
}

impl DmxFrame {
    pub fn new(universe: u16, sequence: u64, timestamp_ms: u64, blackout: bool) -> Self {
        Self {
            universe,
            sequence,
            timestamp_ms,
            data: vec![0u8; DMX_CHANNEL_COUNT],
            blackout,
        }
    }

    pub fn get_channel(&self, channel: u16) -> Option<u8> {
        channel_index(channel).and_then(|idx| self.data.get(idx).copied())
    }

    pub fn set_channel(&mut self, channel: Option<u16>, value: u8) {
        let Some(channel) = channel else { return; };
        let Some(idx) = channel_index(channel) else { return; };
        self.data[idx] = value;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StageVec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl StageVec3 {
    pub const fn zero() -> Self {
        Self { x: 0.0, y: 0.0, z: 0.0 }
    }

    pub const fn one() -> Self {
        Self { x: 1.0, y: 1.0, z: 1.0 }
    }
}

/// Base camera angle for the operator virtual stage. Single FOH-front base
/// today - the operator orbits via Rotate / Tilt sliders rather than picking
/// from a preset list. Kept as an enum so additional bases can be added
/// later without re-plumbing the camera path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageViewPresetKind {
    Foh,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StageViewPreset {
    pub kind: StageViewPresetKind,
    pub id: String,
    pub label: String,
    pub description: String,
    pub camera_position: StageVec3,
    pub look_at: StageVec3,
}

impl StageViewPreset {
    /// FOH baseline angle. Operator orbits this via the Rotate (yaw) and Tilt
    /// (pitch) sliders to reach any other viewpoint - no list of fixed presets
    /// to cycle through.
    pub fn demo_presets() -> Vec<Self> {
        vec![Self {
            kind: StageViewPresetKind::Foh,
            id: "foh".to_string(),
            label: "FOH".to_string(),
            description: "Front-of-house base - orbit via Rotate + Tilt".to_string(),
            camera_position: StageVec3 { x: 0.0, y: -11.5, z: 6.0 },
            look_at: StageVec3 { x: 0.0, y: 6.0, z: 5.8 },
        }]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeckPlaybackState {
    Unknown,
    Stopped,
    Cued,
    Playing,
    Paused,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackCreativeNotes {
    /// First practical metadata source: Rekordbox track grid Comments column.
    pub rekordbox_comments: Option<String>,
    /// Normalized DJ note text consumed by future harmonic-field behavior.
    pub notes_from_dj: Option<String>,
    pub mood_theme_hints: Vec<String>,
    pub key_moments_drops: Vec<String>,
    pub visual_cue_hints: Vec<String>,
    pub avoid_notes: Vec<String>,
    pub prefer_notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CdjPerformanceState {
    pub active_deck_id: Option<String>,
    pub master_deck_id: Option<String>,
    pub deck_playback_state: DeckPlaybackState,
    pub is_master: bool,
    pub is_on_air: bool,
    pub track_id: Option<String>,
    pub track_title: Option<String>,
    pub artist: Option<String>,
    pub bpm: Option<f32>,
    pub beat_phase: f32,
    pub bar_phase: f32,
    pub playback_position_seconds: Option<f32>,
    pub track_notes: TrackCreativeNotes,
}

impl Default for CdjPerformanceState {
    fn default() -> Self {
        Self {
            active_deck_id: Some("demo-cdj-1".to_string()),
            master_deck_id: Some("demo-cdj-1".to_string()),
            deck_playback_state: DeckPlaybackState::Playing,
            is_master: true,
            is_on_air: true,
            track_id: Some("internal-spike-track".to_string()),
            track_title: Some("internal spike track".to_string()),
            artist: Some("Compeller".to_string()),
            bpm: Some(122.0),
            beat_phase: 0.0,
            bar_phase: 0.0,
            playback_position_seconds: Some(0.0),
            track_notes: TrackCreativeNotes {
                rekordbox_comments: Some("cool relaxed blues fades to black like under ocean themed".to_string()),
                notes_from_dj: Some("cool relaxed blues fades to black like under ocean themed".to_string()),
                mood_theme_hints: vec![
                    "cool relaxed blues".to_string(),
                    "under ocean themed".to_string(),
                    "blue palette".to_string(),
                ],
                key_moments_drops: vec!["fade to black transition moments".to_string()],
                visual_cue_hints: vec![
                    "relaxed motion".to_string(),
                    "underwater depth drift".to_string(),
                    "fade-to-black transitions".to_string(),
                ],
                avoid_notes: vec!["avoid harsh strobe energy".to_string()],
                prefer_notes: vec![
                    "prefer blue palette".to_string(),
                    "prefer ocean depth and drift".to_string(),
                ],
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputExpressionKind {
    ReactVideoSurface,
    StagePreviewSurface,
    LightingInterop,
    DmxInterop,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShowTheme {
    pub id: String,
    pub name: String,
    pub palette: Vec<[f32; 3]>,
    pub mood: String,
    pub visual_language: String,
    pub show_identity: String,
}

impl ShowTheme {
    pub fn demo_spike() -> Self {
        Self {
            id: "internal-spike-theme".to_string(),
            name: "Internal spike theme".to_string(),
            palette: vec![[0.1, 1.0, 0.45], [0.15, 0.75, 1.0], [1.0, 0.2, 0.75]],
            mood: "high-energy proof-of-life".to_string(),
            visual_language: "tight laser lines, projected masks, saturated washes".to_string(),
            show_identity: "Koob og-bae Virtual Stage spike".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThemeIntent {
    pub source: CdjPerformanceState,
    pub selected_theme: ShowTheme,
    pub confidence: f32,
    pub output_expressions: Vec<OutputExpressionKind>,
}

impl ThemeIntent {
    pub fn demo_spike() -> Self {
        Self {
            source: CdjPerformanceState::default(),
            selected_theme: ShowTheme::demo_spike(),
            confidence: 1.0,
            output_expressions: vec![
                OutputExpressionKind::ReactVideoSurface,
                OutputExpressionKind::StagePreviewSurface,
                OutputExpressionKind::LightingInterop,
                OutputExpressionKind::DmxInterop,
            ],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThemeState {
    pub intent: ThemeIntent,
    pub active: ShowTheme,
    pub intensity: f32,
}

impl Default for ThemeState {
    fn default() -> Self {
        let intent = ThemeIntent::demo_spike();
        Self { active: intent.selected_theme.clone(), intent, intensity: 1.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FixtureKind {
    Wash,
    Spot,
    Beam,
    Strobe,
    Blinder,
    Laser,
    PixelBar,
    VideoPanel,
    Unknown,
}

/// Physical housing of a fixture — the on-stage SHAPE, distinct from which
/// channels are active in a given mode. The 200W RGB Strobe is a
/// `FixtureHousing::Panel` whether it's patched in 3-channel, 9-channel, or
/// 24-channel mode; only the channels exposed for cues / behavior change
/// across modes.
///
/// Adding a new fixture type? In most cases no Rust change is needed —
/// drop the ATLABASE JSON into `assets/nas-fixture-profiles/`, and
/// `infer_housing` (in `dmx::profiles`) picks the right variant from the
/// profile's category + channel structure across all modes. Add a new
/// variant here ONLY when the fixture has a genuinely new physical shape
/// (and pair it with a `draw_*` function in `ui::virtual_stage`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FixtureHousing {
    /// Single-aperture wash / par. Wide-cone flood from a static head.
    Par,
    /// Rectangular LED matrix housing — 1+ addressable color zones inside a
    /// fixed flat panel. Covers segmented strobes (200W RGB), LED walls, and
    /// 1-cell static color blocks. Behavior across modes changes by which
    /// channels are exposed (per-cell RGB vs whole-panel RGB + Dimmer +
    /// Strobe + macro pattern engine).
    Panel,
    /// Horizontal strip of LED beam emitters. May be motorized (bar tilts
    /// as a rigid unit, e.g. SHEHDS LED Beam 8x12W) or static (a fixed bar
    /// like a wash bar). Per-emitter color comes from per-cell RGB
    /// channels; tilt (if present) sweeps the bar.
    PixelBar,
    /// Pan + tilt yoke with a single aperture — spots, beams, gobo movers
    /// (SHEHDS 7R Beam, Chauvet Obsession). DMX-driven head pose.
    MovingHead,
    /// Scan laser projector — X/Y mirrors plus a pattern/cue bank. No
    /// motorized head, the projection direction is the DMX state.
    ScanLaser,
    /// Classic single-emitter omnidirectional flash strobe. No internal
    /// cells, just a hard burst when fired.
    Strobe,
    /// Fallback for fixtures whose housing the parser can't infer. Renders
    /// with the existing visual_kind dispatch — never the dedicated
    /// housing draw paths.
    Generic,
}

/// Map a NAS-library `FixtureKind` onto the renderer's `(role, visual_kind)`
/// pair. Centralized so the +Fixture palette and any future profile-driven
/// re-bind path stay consistent - and so adding a new kind only requires
/// updating this one match arm.
pub fn role_and_kind_for(kind: FixtureKind) -> (FixtureRole, StageVisualKind) {
    match kind {
        FixtureKind::Wash => (FixtureRole::Wash, StageVisualKind::Wash),
        FixtureKind::Spot => (FixtureRole::Accent, StageVisualKind::Gobo),
        FixtureKind::Beam => (FixtureRole::Accent, StageVisualKind::Beam),
        FixtureKind::Strobe => (FixtureRole::Strobe, StageVisualKind::Strobe),
        FixtureKind::Blinder => (FixtureRole::Wash, StageVisualKind::Wash),
        FixtureKind::Laser => (FixtureRole::Accent, StageVisualKind::Laser),
        FixtureKind::PixelBar => (FixtureRole::Wash, StageVisualKind::LedPanel),
        FixtureKind::VideoPanel => (FixtureRole::Wash, StageVisualKind::LedPanel),
        FixtureKind::Unknown => (FixtureRole::Accent, StageVisualKind::Beam),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FixtureRole {
    Wash,
    Accent,
    Strobe,
}

/// Semantic visual kind for the Virtual Stage renderer.
///
/// Keep this renderer-facing and data-only: fixture patch/state are the surface
/// entity data. CdjPerformanceState can infer/select ThemeIntent; Compeller
/// harmonic field algorithms combine theme intent plus performance context into
/// coherent visual behavior. DMX/device state is optional output/interop that can
/// express the same behavior when needed. stageLX/Bevy is visual surface
/// reference only; no source code was copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageVisualKind {
    Wash,
    Beam,
    Laser,
    Gobo,
    Strobe,
    LedPanel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageFixturePresetKind {
    ShehdsRgbSegmentStrobe,
    ShehdsRgbwBarMover,
    Shehds230w7rBeam,
    ScorpionDualRgbLaser,
    ShehdsRgbScanLaser,
    ChauvetObsessionGobo,
    LedSidePanel,
}

impl StageFixturePresetKind {
    pub fn react_demo_palette() -> &'static [Self] {
        &[
            Self::ShehdsRgbSegmentStrobe,
            Self::ShehdsRgbwBarMover,
            Self::Shehds230w7rBeam,
            Self::ScorpionDualRgbLaser,
            Self::ShehdsRgbScanLaser,
            Self::ChauvetObsessionGobo,
            Self::LedSidePanel,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ShehdsRgbSegmentStrobe => "SHEHDS RGB 8-seg strobe/wash",
            Self::ShehdsRgbwBarMover => "SHEHDS 8x12W RGBW bar mover",
            Self::Shehds230w7rBeam => "SHEHDS 230W 7R beam mover",
            Self::ScorpionDualRgbLaser => "Scorpion Dual RGB laser",
            Self::ShehdsRgbScanLaser => "SHEHDS 1W RGB scan laser",
            Self::ChauvetObsessionGobo => "Chauvet Obsession gobo/flower",
            Self::LedSidePanel => "LED side panel",
        }
    }

    pub fn default_count(self) -> usize {
        match self {
            Self::ShehdsRgbSegmentStrobe => 4,
            Self::ShehdsRgbwBarMover => 2,
            Self::Shehds230w7rBeam => 4,
            Self::ScorpionDualRgbLaser => 1,
            Self::ShehdsRgbScanLaser => 2,
            Self::ChauvetObsessionGobo => 2,
            Self::LedSidePanel => 0,
        }
    }

    pub fn default_position(self, ordinal: usize) -> StageVec3 {
        let n = ordinal as f32;
        match self {
            Self::ShehdsRgbSegmentStrobe => StageVec3 { x: -5.4 + n * 3.6, y: 0.8, z: 0.35 },
            Self::ShehdsRgbwBarMover => StageVec3 { x: if ordinal % 2 == 0 { -5.5 } else { 5.5 }, y: 7.4, z: 11.8 },
            Self::Shehds230w7rBeam => StageVec3 { x: -9.0 + n * 6.0, y: 5.8, z: 8.2 },
            Self::ScorpionDualRgbLaser => StageVec3 { x: 0.0, y: 7.8, z: 6.4 },
            Self::ShehdsRgbScanLaser => StageVec3 { x: if ordinal % 2 == 0 { -11.5 } else { 11.5 }, y: 4.8, z: 4.0 },
            Self::ChauvetObsessionGobo => StageVec3 { x: if ordinal % 2 == 0 { -7.0 } else { 7.0 }, y: 6.5, z: 5.4 },
            Self::LedSidePanel => StageVec3 { x: if ordinal % 2 == 0 { -13.2 } else { 13.2 }, y: 6.8, z: 4.6 },
        }
    }

    pub fn default_tilt(self, ordinal: usize) -> f32 {
        match self {
            Self::ShehdsRgbSegmentStrobe | Self::LedSidePanel => 0.0,
            Self::ShehdsRgbwBarMover | Self::Shehds230w7rBeam | Self::ChauvetObsessionGobo => 90.0,
            Self::ScorpionDualRgbLaser | Self::ShehdsRgbScanLaser => if ordinal % 2 == 0 { 90.0 } else { 180.0 },
        }
    }
}

/// Which venue/environment the virtual stage renders. `ConcertStage` is the
/// original rig preview — receding floor, upstage LED wall, overhead/side/floor
/// trusses. `WeddingRoom` is a rectangular reception room with a centered dance
/// floor where the operator drops lights anywhere (perimeter uplights etc.).
/// A REACT-local operator choice, persisted as a key in the settings KV store
/// (see `ControlPanel::virtual_stage_venue`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum VenueKind {
    #[default]
    ConcertStage,
    WeddingRoom,
}

impl VenueKind {
    /// Every venue, in selector order.
    pub fn all() -> &'static [VenueKind] {
        &[VenueKind::ConcertStage, VenueKind::WeddingRoom]
    }

    /// Operator-facing label for the venue selector.
    pub fn label(self) -> &'static str {
        match self {
            VenueKind::ConcertStage => "Concert Stage",
            VenueKind::WeddingRoom => "Wedding Room",
        }
    }

    /// Stable key for the settings KV store — round-trips via `from_key`. Kept
    /// distinct from the `Debug` name so a future rename can't silently break
    /// persisted operator choices.
    pub fn as_key(self) -> &'static str {
        match self {
            VenueKind::ConcertStage => "concert_stage",
            VenueKind::WeddingRoom => "wedding_room",
        }
    }

    /// Parse a stored key back to a venue; `None` for unknown keys so the caller
    /// falls back to the default.
    pub fn from_key(key: &str) -> Option<VenueKind> {
        match key {
            "concert_stage" => Some(VenueKind::ConcertStage),
            "wedding_room" => Some(VenueKind::WeddingRoom),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FixtureCapability {
    Dimmer,
    Rgb,
    White,
    ColorWheel,
    PanTilt,
    Zoom,
    Focus,
    Strobe,
    Gobo,
    Prism,
    LaserMacro,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DmxAttribute {
    Dimmer,
    Red,
    Green,
    Blue,
    White,
    /// UV / blacklight emitter level. First-class so UV mode (the manual
    /// "blackout, except blacklight" trigger) can address every UV-capable
    /// fixture rig-wide instead of leaving the channel as `Custom`.
    Uv,
    ColorWheel,
    Pan,
    Tilt,
    Zoom,
    Focus,
    Strobe,
    GoboSlot,
    GoboRotation,
    Prism,
    LaserMacro,
    Custom(String),
}

/// A continuous DMX sub-range, stored verbatim as the source profile's
/// 0.0-1.0 fraction of the channel span (ATLABASE `range.lowValue` /
/// `range.highValue`). Kept as `f64` so no precision is dropped on ingest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValueRange {
    pub low: f64,
    pub high: f64,
}

/// One discrete position within a channel - a color-wheel color, a gobo
/// pattern, or a macro entry. `value` is the low DMX value (0-255) at which
/// the slot engages, kept verbatim. `rgb` carries the slot color when the
/// source provides one (0.0-1.0 per component).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelSlot {
    pub name: String,
    pub value: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low_value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high_value: Option<f64>,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rgb: Option<[f64; 3]>,
}

/// Rich, renderer-agnostic capability detail preserved from the source
/// profile. Optional on every channel - a plain channel carries `None`.
///
/// COM-494: full inventory/preservation of rich ATLABASE data. The executor
/// contract (COM-493) only reads `attribute` + channel offsets, so this field
/// is purely additive - it exists so inventory, the inspector, and later
/// normalization never lose color wheels, gobos, motion ranges, dimmer
/// curves, or static/macro semantics. Built-in/macro ranges are kept even
/// when their meaning is opaque.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChannelDetail {
    /// A color wheel: discrete colored slots, optional split-color support.
    ColorWheel {
        allow_split_colors: bool,
        slots: Vec<ChannelSlot>,
    },
    /// A gobo / pattern wheel: discrete pattern steps.
    Gobo { steps: Vec<ChannelSlot> },
    /// Prism / prism-mode selector: discrete prism modes or movement ranges.
    Prism { steps: Vec<ChannelSlot> },
    /// Continuous movement (pan/tilt) with physical width/offset and the
    /// usable DMX range.
    Movement {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<ValueRange>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        smoothing: Option<f64>,
        invert: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<f64>,
    },
    /// A continuous non-movement curve (zoom, focus, dimmer curve).
    Range {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<ValueRange>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        smoothing: Option<f64>,
        invert: bool,
    },
    /// A channel pinned to a fixed value (control-mode selector, static
    /// shutter). Carries the resting DMX value.
    Static { value: u8 },
    /// A set of built-in macro / effect steps whose semantics are opaque but
    /// must not be discarded.
    Macro { steps: Vec<ChannelSlot> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelDefinition {
    pub attribute: DmxAttribute,
    pub channel_offset: u16,
    pub default_value: u8,
    pub blackout_value: u8,
    /// Rich capability detail preserved from the source profile, when any.
    /// Additive to the executor contract - see [`ChannelDetail`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<ChannelDetail>,
    /// Lossless source attribute payload for hardware inventory/API consumers.
    /// REACT's per-frame executor does not read this. It exists so COM-494
    /// can preserve ATLABASE fields that are not normalized yet (for example
    /// opaque macro/toggle/prism settings) while COM-497/498 move to hardware.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_attribute: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FixtureProfile {
    pub id: String,
    pub manufacturer: String,
    pub model: String,
    pub mode: String,
    pub kind: FixtureKind,
    /// Physical housing — see [`FixtureHousing`]. This is the renderer's
    /// source of truth for "what does this fixture look like on stage",
    /// independent of which mode (3/9/24-channel, etc.) the operator
    /// patched. Defaulted to `Generic` so older serialized profiles still
    /// deserialize cleanly; the NAS-library loader fills it in.
    #[serde(default = "FixtureProfile::default_housing")]
    pub housing: FixtureHousing,
    pub channel_count: u16,
    pub channels: Vec<ChannelDefinition>,
    pub capabilities: Vec<FixtureCapability>,
}

impl FixtureProfile {
    fn default_housing() -> FixtureHousing {
        FixtureHousing::Generic
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChannelMap {
    pub dimmer: Option<u16>,
    pub red: Option<u16>,
    pub green: Option<u16>,
    pub blue: Option<u16>,
    pub white: Option<u16>,
    /// UV / blacklight emitter channel. `#[serde(default)]` so patches saved
    /// or synced before the slot existed still deserialize (uv = None).
    #[serde(default)]
    pub uv: Option<u16>,
    pub strobe: Option<u16>,
    pub color_wheel: Option<u16>,
    pub pan: Option<u16>,
    pub tilt: Option<u16>,
    pub zoom: Option<u16>,
    pub focus: Option<u16>,
    pub gobo_slot: Option<u16>,
    pub gobo_rotation: Option<u16>,
    pub prism: Option<u16>,
    pub laser_macro: Option<u16>,
}

impl ChannelMap {
    pub fn max_channel(self) -> Option<u16> {
        [
            self.dimmer, self.red, self.green, self.blue, self.white, self.uv,
            self.strobe, self.color_wheel, self.pan, self.tilt, self.zoom,
            self.focus, self.gobo_slot, self.gobo_rotation, self.prism,
            self.laser_macro,
        ].into_iter().flatten().max()
    }

    pub fn rgbw_dimmer(start_address: u16) -> Self {
        Self {
            dimmer: Some(start_address),
            red: Some(start_address + 1),
            green: Some(start_address + 2),
            blue: Some(start_address + 3),
            white: Some(start_address + 4),
            uv: None,
            strobe: None,
            color_wheel: None,
            pan: None,
            tilt: None,
            zoom: None,
            focus: None,
            gobo_slot: None,
            gobo_rotation: None,
            prism: None,
            laser_macro: None,
        }
    }

    pub fn moving_head_spot(start_address: u16) -> Self {
        Self {
            pan: Some(start_address),
            tilt: Some(start_address + 1),
            dimmer: Some(start_address + 2),
            strobe: Some(start_address + 3),
            color_wheel: Some(start_address + 4),
            gobo_slot: Some(start_address + 5),
            gobo_rotation: Some(start_address + 6),
            prism: Some(start_address + 7),
            zoom: Some(start_address + 8),
            focus: Some(start_address + 9),
            red: Some(start_address + 10),
            green: None,
            blue: Some(start_address + 11),
            white: None,
            uv: None,
            laser_macro: None,
        }
    }

    pub fn laser_macro(start_address: u16) -> Self {
        Self {
            dimmer: Some(start_address),
            laser_macro: Some(start_address + 1),
            strobe: Some(start_address + 2),
            color_wheel: Some(start_address + 3),
            red: None,
            green: None,
            blue: None,
            white: None,
            uv: None,
            pan: None,
            tilt: None,
            zoom: None,
            focus: None,
            gobo_slot: None,
            gobo_rotation: None,
            prism: None,
        }
    }

    pub fn led_strobe(start_address: u16) -> Self {
        Self {
            dimmer: Some(start_address),
            strobe: Some(start_address + 1),
            red: Some(start_address + 2),
            green: Some(start_address + 3),
            blue: Some(start_address + 4),
            white: None,
            uv: None,
            color_wheel: None,
            pan: None,
            tilt: None,
            zoom: None,
            focus: None,
            gobo_slot: None,
            gobo_rotation: None,
            prism: None,
            laser_macro: None,
        }
    }

    /// Build a ChannelMap by walking a fixture profile's ordered channel
    /// list and resolving each `DmxAttribute` to its absolute DMX address.
    /// Attributes that don't map to a `ChannelMap` slot (e.g. `Custom`) are
    /// silently dropped - they're still in the profile, but the renderer
    /// only consumes the well-known slots.
    pub fn from_profile(start_address: u16, profile: &FixtureProfile) -> Self {
        let mut map = Self {
            dimmer: None, red: None, green: None, blue: None, white: None,
            uv: None, strobe: None, color_wheel: None, pan: None, tilt: None,
            zoom: None, focus: None, gobo_slot: None, gobo_rotation: None,
            prism: None, laser_macro: None,
        };
        for channel in &profile.channels {
            // channel_offset is 1-based (first channel = 1), so the absolute
            // address is start_address + offset - 1 (the first channel lands
            // on start_address).
            let addr = start_address.saturating_add(channel.channel_offset.saturating_sub(1));
            match channel.attribute {
                DmxAttribute::Dimmer => map.dimmer = Some(addr),
                DmxAttribute::Red => map.red = Some(addr),
                DmxAttribute::Green => map.green = Some(addr),
                DmxAttribute::Blue => map.blue = Some(addr),
                DmxAttribute::White => map.white = Some(addr),
                DmxAttribute::Uv => map.uv = Some(addr),
                DmxAttribute::ColorWheel => map.color_wheel = Some(addr),
                DmxAttribute::Pan => map.pan = Some(addr),
                DmxAttribute::Tilt => map.tilt = Some(addr),
                DmxAttribute::Zoom => map.zoom = Some(addr),
                DmxAttribute::Focus => map.focus = Some(addr),
                DmxAttribute::Strobe => map.strobe = Some(addr),
                DmxAttribute::GoboSlot => map.gobo_slot = Some(addr),
                DmxAttribute::GoboRotation => map.gobo_rotation = Some(addr),
                DmxAttribute::Prism => map.prism = Some(addr),
                DmxAttribute::LaserMacro => map.laser_macro = Some(addr),
                DmxAttribute::Custom(_) => {}
            }
        }
        map
    }

    pub fn led_panel(start_address: u16) -> Self {
        Self {
            dimmer: Some(start_address),
            red: Some(start_address + 1),
            green: Some(start_address + 2),
            blue: Some(start_address + 3),
            white: Some(start_address + 4),
            uv: None,
            strobe: None,
            color_wheel: None,
            pan: None,
            tilt: None,
            zoom: None,
            focus: None,
            gobo_slot: None,
            gobo_rotation: None,
            prism: None,
            laser_macro: None,
        }
    }
}

/// How a video fills a Screen surface. Wire values are lowercase
/// (`"contain"` / `"cover"`) to match the SaaS `behaviorJson` `screen_fit` key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScreenFit {
    /// Letterbox/pillarbox the whole frame inside the surface (no crop).
    #[default]
    Contain,
    /// Fill the surface, cropping overflow.
    Cover,
}

/// Configuration for a `kind='Screen'` fixture — a plain placeable video surface
/// (NOT the reactive main-stage LED wall). Carries the SaaS-suggested default
/// binding; the operator's REACT-local file selection (kept off the patch, keyed
/// by fixture id) overrides `video_url` at render time. A fixture is a screen iff
/// `FixturePatch::screen` is `Some`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ScreenConfig {
    pub fit: ScreenFit,
    /// SaaS-suggested account media/render id (slice 2 binding).
    pub media_id: Option<i64>,
    /// SaaS-resolved default stream URL (slice 2 binding).
    pub video_url: Option<String>,
}

impl ScreenConfig {
    /// Parse from a SaaS fixture `behaviorJson` object — wire keys `screen_fit`
    /// (`contain`|`cover`), `screen_media_id` (int), `screen_video_url` (string).
    /// Missing/garbage keys fall back to defaults so a bare `kind='Screen'` with
    /// empty behavior still yields a valid screen.
    pub fn from_behavior(behavior: Option<&serde_json::Value>) -> Self {
        let Some(b) = behavior else {
            return Self::default();
        };
        let fit = match b.get("screen_fit").and_then(|v| v.as_str()) {
            Some("cover") => ScreenFit::Cover,
            _ => ScreenFit::Contain,
        };
        Self {
            fit,
            media_id: b.get("screen_media_id").and_then(|v| v.as_i64()),
            video_url: b
                .get("screen_video_url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        }
    }

    /// Serialize back to the SaaS `behaviorJson` wire shape (inverse of
    /// `from_behavior`) so a REACT-authored/edited screen round-trips on PATCH/POST.
    pub fn to_behavior(&self) -> serde_json::Value {
        serde_json::json!({
            "screen_fit": match self.fit {
                ScreenFit::Contain => "contain",
                ScreenFit::Cover => "cover",
            },
            "screen_media_id": self.media_id,
            "screen_video_url": self.video_url,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FixturePatch {
    pub id: String,
    pub name: String,
    pub universe: u16,
    pub start_address: u16,
    pub profile_id: String,
    pub mode: String,
    pub position: StageVec3,
    pub rotation: StageVec3,
    pub scale: StageVec3,
    pub role: FixtureRole,
    pub visual_kind: StageVisualKind,
    pub tags: Vec<String>,
    pub channels: ChannelMap,
    /// Total DMX channel count of the resolved profile/mode — the fixture's
    /// TRUE address footprint, including channels `ChannelMap` has no slot for
    /// (`DmxAttribute::Custom`: STATIC axis-rolling channels, function/speed
    /// selectors, 16-bit fine channels, ...). The address allocator reserves
    /// this whole span so the next fixture can never land on a live-but-unmapped
    /// channel (the Scorpion laser ends with 3 Custom axis channels; the SHEHDS
    /// flat par ends with 2 STATIC function channels). `0` = unknown — older
    /// saves / SaaS rows that predate the field — and falls back to the
    /// ChannelMap-derived span, preserving the pre-footprint behavior.
    #[serde(default)]
    pub footprint: u16,
    /// `Some` iff this fixture is a placeable video screen (`kind='Screen'` on
    /// the SaaS side). Defaults to `None` (a normal light). Never affects DMX.
    #[serde(default)]
    pub screen: Option<ScreenConfig>,
}

/// Operator live-mix override for a single fixture — an in-memory trim the
/// performer rides during a show, applied on top of the cue/autopilot intent.
/// **Ephemeral**: never persisted, never on the synced [`FixturePatch`], reset to
/// default every launch (mirrors the Blackout trigger). `intensity` is a 0..1
/// attenuation (skips lasers, whose dimmer is a binary enable gate); `mute`
/// silences the fixture; `solo` isolates it (see [`FixtureOverrides::any_solo`]).
/// Operator-chosen per-cell pattern for a segmented fixture (the 200W RGB panel's
/// 8 cells / 24-channel mode). `Auto` keeps the music-tension autopilot; the rest
/// lock the cells to a specific look the operator drives. Speed (cycle length in
/// beats) is the sibling `segment_speed` field on [`FixtureOverride`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SegmentPattern {
    #[default]
    Auto,
    Chase,
    ChaseReverse,
    Bounce,
    CenterOut,
    Rainbow,
    Wipe,
    Strobe,
    Bank,
    Solid,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FixtureOverride {
    pub intensity: f32,
    pub mute: bool,
    pub solo: bool,
    /// Operator segment pattern for a segmented panel (default `Auto` = autopilot).
    pub segment_fx: SegmentPattern,
    /// Segment-pattern cycle length in beats (lower = faster). Default 1.0.
    pub segment_speed: f32,
}

impl Default for FixtureOverride {
    fn default() -> Self {
        Self {
            intensity: 1.0,
            mute: false,
            solo: false,
            segment_fx: SegmentPattern::Auto,
            segment_speed: 1.0,
        }
    }
}

/// In-memory map of per-fixture live overrides, keyed by `fixture.id`. Sparse —
/// an entry is dropped once it returns to default so the map never accumulates
/// no-op rows, and a fixture with no entry reads as [`FixtureOverride::default`].
#[derive(Default, Clone, Debug)]
pub struct FixtureOverrides {
    map: std::collections::HashMap<String, FixtureOverride>,
}

impl FixtureOverrides {
    /// Current override for a fixture (default — full, un-muted, un-soloed — if absent).
    pub fn get(&self, id: &str) -> FixtureOverride {
        self.map.get(id).copied().unwrap_or_default()
    }

    /// Write back an override, dropping the entry when it equals default to stay sparse.
    pub fn set(&mut self, id: &str, ov: FixtureOverride) {
        if ov == FixtureOverride::default() {
            self.map.remove(id);
        } else {
            self.map.insert(id.to_string(), ov);
        }
    }

    /// Forget a fixture's override (e.g. when it is removed from the patch).
    pub fn remove(&mut self, id: &str) {
        self.map.remove(id);
    }

    /// Is any *live* fixture soloed? Computed against the patch's current fixtures
    /// so a stale entry for a sync-removed fixture can never silence the rig.
    pub fn any_solo(&self, patch: &DmxPatch) -> bool {
        // Screens are not lights — a stale/UI solo on a screen must never silence
        // the real rig (it's skipped in the DMX render loop anyway).
        patch
            .fixtures
            .iter()
            .any(|f| f.screen.is_none() && self.get(&f.id).solo)
    }

    /// Serialize the *persistent* per-fixture Segment FX choices (pattern + speed)
    /// to JSON, keyed by fixture id. Only non-`Auto` entries are written; the
    /// ephemeral intensity/mute/solo are deliberately not persisted.
    ///
    /// Collected into a `BTreeMap` so the SAME state always serializes to the
    /// SAME string: app.rs detects changes by string-diffing successive calls,
    /// and a fresh `HashMap` per call (randomized seed per instance) reordered
    /// keys with 2+ entries — the debounce re-armed forever and segment FX only
    /// ever persisted on clean exit.
    pub fn segfx_json(&self) -> String {
        let m: std::collections::BTreeMap<&str, (SegmentPattern, f32)> = self
            .map
            .iter()
            .filter(|(_, ov)| ov.segment_fx != SegmentPattern::Auto)
            .map(|(id, ov)| (id.as_str(), (ov.segment_fx, ov.segment_speed)))
            .collect();
        serde_json::to_string(&m).unwrap_or_else(|_| "{}".to_string())
    }

    /// Restore per-fixture Segment FX choices from [`segfx_json`]. Entries for
    /// fixtures not (yet) on the stage are kept — they apply once that fixture
    /// loads, and stay dormant (harmless) if it never does.
    pub fn load_segfx_json(&mut self, json: &str) {
        if let Ok(m) =
            serde_json::from_str::<std::collections::HashMap<String, (SegmentPattern, f32)>>(json)
        {
            for (id, (pattern, speed)) in m {
                let mut ov = self.get(&id);
                ov.segment_fx = pattern;
                ov.segment_speed = speed;
                self.set(&id, ov);
            }
        }
    }
}

/// Absolute DMX addresses for one color cell of a multi-segment fixture
/// (e.g. one of the 8 RGBW cells on a pixel bar). Derived from the resolved
/// profile via [`segments_for_profile`]; not persisted - the profile is the
/// source of truth, so a freshly synced patch re-derives these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentChannels {
    pub red: Option<u16>,
    pub green: Option<u16>,
    pub blue: Option<u16>,
    pub white: Option<u16>,
}

/// Group a resolved profile's color channels into per-cell segments. Each new
/// RED begins a new cell; GREEN/BLUE/WHITE attach to the cell in progress.
/// Returns an empty vec for single-color fixtures (0 or 1 cell) - callers use
/// that to mean "not a pixel bar". Absolute addresses use the 1-based
/// `channel_offset` convention: `start_address + offset - 1`.
pub fn segments_for_profile(profile: &FixtureProfile, start_address: u16) -> Vec<SegmentChannels> {
    let abs = |offset: u16| start_address.saturating_add(offset.saturating_sub(1));
    let mut segments: Vec<SegmentChannels> = Vec::new();
    let mut current: Option<SegmentChannels> = None;
    for ch in &profile.channels {
        let addr = abs(ch.channel_offset);
        match ch.attribute {
            DmxAttribute::Red => {
                if let Some(seg) = current.take() {
                    segments.push(seg);
                }
                current = Some(SegmentChannels {
                    red: Some(addr),
                    green: None,
                    blue: None,
                    white: None,
                });
            }
            DmxAttribute::Green => {
                if let Some(seg) = current.as_mut() {
                    seg.green = Some(addr);
                }
            }
            DmxAttribute::Blue => {
                if let Some(seg) = current.as_mut() {
                    seg.blue = Some(addr);
                }
            }
            DmxAttribute::White => {
                if let Some(seg) = current.as_mut() {
                    seg.white = Some(addr);
                }
            }
            _ => {}
        }
    }
    if let Some(seg) = current.take() {
        segments.push(seg);
    }
    // One cell is just a normal RGB fixture, not a bar.
    if segments.len() < 2 {
        return Vec::new();
    }
    segments
}

/// Which world axis a truss runs along. The lattice face spreads across the
/// run, so an `X` truss reads as a flown horizontal bar and a `Z` truss as an
/// upright tower. `Y` (depth) is reserved for a future upstage/downstage bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrussAxis {
    #[default]
    X,
    Y,
    Z,
}

/// Semantic slot of a truss in the rig. Drives seeding + the panel label; the
/// geometry lives in `center` / `axis` / `length`, so a `Generic` truss the
/// operator adds renders identically to a named one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrussKind {
    Overhead,
    Floor,
    LeftTower,
    RightTower,
    #[default]
    Generic,
}

fn default_truss_width() -> f32 {
    0.5
}
fn default_truss_sections() -> u32 {
    12
}

/// One operator-placed rig truss. Stored per-stage in SaaS (`truss_layout`
/// JSON) and rendered as flat lattice scenery in the virtual stage. The
/// operator drags `center`, sizes `length` / `width` / `sections`, and `locked`
/// freezes it. A real rig is trusses-first, fixtures-hung-second - so this is
/// authored geometry, not derived from where fixtures happen to sit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Truss {
    pub id: String,
    #[serde(default)]
    pub kind: TrussKind,
    pub center: StageVec3,
    #[serde(default)]
    pub axis: TrussAxis,
    pub length: f32,
    #[serde(default = "default_truss_width")]
    pub width: f32,
    #[serde(default = "default_truss_sections")]
    pub sections: u32,
    #[serde(default)]
    pub locked: bool,
}

impl Truss {
    /// Endpoints `(a, b)` in world space - `center +/- (length/2)` along `axis`.
    /// The renderer draws the lattice between them.
    pub fn endpoints(&self) -> (StageVec3, StageVec3) {
        let h = self.length * 0.5;
        let (dx, dy, dz) = match self.axis {
            TrussAxis::X => (h, 0.0, 0.0),
            TrussAxis::Y => (0.0, h, 0.0),
            TrussAxis::Z => (0.0, 0.0, h),
        };
        (
            StageVec3 { x: self.center.x - dx, y: self.center.y - dy, z: self.center.z - dz },
            StageVec3 { x: self.center.x + dx, y: self.center.y + dy, z: self.center.z + dz },
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DmxPatch {
    pub universe: u16,
    pub fixtures: Vec<FixturePatch>,
    #[serde(default)]
    pub trusses: Vec<Truss>,
    /// The venue/environment the SELECTED STAGE authored (`venue_kind` from SaaS),
    /// or `None` when the stage didn't specify one (older SaaS, un-migrated stage,
    /// or the demo rigs). `None` lets the operator's local Venue toggle drive the
    /// preview; `Some` makes the stage authoritative. See the app-loop precedence.
    #[serde(default)]
    pub venue: Option<VenueKind>,
}

impl DmxPatch {
    /// Demo patch: one RGBW dimmer fixture at the given address.
    pub fn demo_rgbw_wash(universe: u16, start_address: u16) -> Self {
        Self {
            universe,
            fixtures: vec![FixturePatch::demo_rgbw_wash("react-demo-rgbw", universe, start_address)],
            trusses: Vec::new(),
            venue: None,
        }
    }

    /// Demo venue rig - an Anyma-style layout: a large upstage LED wall with
    /// fixtures hung on the rig trusses (overhead, four side towers, downstage
    /// floor truss). Every fixture is movable and re-aimable from its control
    /// panel in the virtual stage; the default aims exercise all three aim
    /// presets - down, horizontal and up.
    pub fn demo_virtual_stage(universe: u16, start_address: u16) -> Self {
        let mut patch = Self { universe, fixtures: vec![], trusses: Self::default_trusses(), venue: None };
        let mut next = start_address;
        for preset in StageFixturePresetKind::react_demo_palette() {
            for _ in 0..preset.default_count() {
                let ordinal = patch.count_preset(*preset);
                let fixture = FixturePatch::from_stage_preset(*preset, ordinal, universe, next)
                    .at(preset.default_position(ordinal))
                    .tilted(preset.default_tilt(ordinal));
                next = fixture.channels.max_channel().unwrap_or(next).saturating_add(1);
                patch.fixtures.push(fixture);
            }
        }
        patch
    }

    /// A seeded Wedding Room scene: a centered upstage video screen + four warm
    /// uplights framing the dance floor. Used when the operator first enters an
    /// empty Wedding venue (a REACT-local scene). No trusses — a reception room
    /// has no concert rig. A SaaS-authored wedding stage will later load into the
    /// same scene slot, so this is a sensible default, not a fixed layout.
    pub fn demo_wedding_room(universe: u16, start_address: u16) -> Self {
        let mut patch = Self { universe, fixtures: vec![], trusses: vec![], venue: None };
        let mut next = start_address;
        let mut screen = FixturePatch::demo_screen("screen-1", universe, next);
        screen.name = "Screen 1".to_string();
        next = screen.channels.max_channel().unwrap_or(next).saturating_add(1);
        patch.fixtures.push(screen);
        // Four warm uplights around the dance floor, aimed up the walls.
        for (i, (x, y)) in [(-7.0, 9.5), (7.0, 9.5), (-7.0, 0.5), (7.0, 0.5)].iter().enumerate() {
            let mut up = FixturePatch::demo_rgbw_wash(&format!("uplight-{}", i + 1), universe, next);
            up.name = format!("Uplight {}", i + 1);
            up.position = StageVec3 { x: *x, y: *y, z: 0.3 };
            up.rotation = StageVec3 { x: 178.0, y: 0.0, z: 0.0 }; // aim up the wall
            next = up.channels.max_channel().unwrap_or(next).saturating_add(1);
            patch.fixtures.push(up);
        }
        patch
    }

    /// The corrected baseline rig - deliberate truss geometry, seeded when a
    /// stage has no saved `truss_layout`. The operator then drags / sizes /
    /// locks these and the layout persists per-stage in SaaS. Coords match the
    /// floor/back-wall scale: x +/-12, y = upstage depth (LED wall at y=8), z =
    /// height (floor 0 -> rig ~14). The overhead sits at z=14 (above the screen
    /// top z~13) pulled downstage so it clears the wall; the floor truss is
    /// centered and stepped back; the towers frame the wall from x=+/-13, y=7.
    pub fn default_trusses() -> Vec<Truss> {
        vec![
            Truss {
                id: "overhead".into(),
                kind: TrussKind::Overhead,
                center: StageVec3 { x: 0.0, y: 4.5, z: 14.0 },
                axis: TrussAxis::X,
                length: 22.0,
                width: 0.6,
                sections: 18,
                locked: false,
            },
            Truss {
                id: "floor".into(),
                kind: TrussKind::Floor,
                center: StageVec3 { x: 0.0, y: 2.5, z: 0.3 },
                axis: TrussAxis::X,
                length: 18.0,
                width: 0.5,
                sections: 12,
                locked: false,
            },
            Truss {
                id: "left-tower".into(),
                kind: TrussKind::LeftTower,
                center: StageVec3 { x: -13.0, y: 7.0, z: 4.75 },
                axis: TrussAxis::Z,
                length: 7.5,
                width: 0.5,
                sections: 8,
                locked: false,
            },
            Truss {
                id: "right-tower".into(),
                kind: TrussKind::RightTower,
                center: StageVec3 { x: 13.0, y: 7.0, z: 4.75 },
                axis: TrussAxis::Z,
                length: 7.5,
                width: 0.5,
                sections: 8,
                locked: false,
            },
        ]
    }

    pub fn count_preset(&self, preset: StageFixturePresetKind) -> usize {
        self.fixtures.iter().filter(|f| f.preset_kind() == Some(preset)).count()
    }

    /// Count fixtures already on this stage that resolve to the given NAS
    /// profile id. Drives the "(N)" counter on each +Fixture palette entry.
    pub fn count_profile(&self, profile_id: &str) -> usize {
        self.fixtures.iter().filter(|f| f.profile_id == profile_id).count()
    }

    /// Drop a fresh fixture onto the stage from a NAS profile. The +Fixture
    /// palette path: choose a profile, allocate a unique fixture id (profile
    /// + ordinal suffix collision-checked against existing keys), assign the
    /// next free DMX start address, and append to the patch. The sync
    /// worker picks it up on the next debounced flush and POSTs to SaaS.
    pub fn add_fixture_from_profile(&mut self, profile: &FixtureProfile) -> usize {
        let universe = self.universe;
        let start = self.next_start_address();
        // Ordinal scan: pick the lowest N >= 1 such that "{profile_id}-{N}"
        // is not already a fixture key.
        let mut ordinal = self.count_profile(&profile.id);
        loop {
            let candidate = format!("{}-{}", profile.id, ordinal + 1);
            if !self.fixtures.iter().any(|f| f.id == candidate) {
                break;
            }
            ordinal += 1;
        }
        let fixture = FixturePatch::from_profile(profile, ordinal, universe, start);
        self.fixtures.push(fixture);
        self.fixtures.len() - 1
    }

    /// Drop a placeable video screen onto the stage (the +Screen operator
    /// action). Staggered along X so successive screens don't stack; returns the
    /// new fixture's index. The renderer treats any `screen.is_some()` fixture as
    /// a screen, so this works whether or not a SaaS stage is loaded.
    pub fn add_screen_fixture(&mut self) -> usize {
        let n = self.fixtures.iter().filter(|f| f.screen.is_some()).count();
        let universe = self.universe;
        let start = self.next_start_address();
        let mut screen = FixturePatch::demo_screen(&format!("screen-{}", n + 1), universe, start);
        screen.name = format!("Screen {}", n + 1);
        screen.position.x = -6.0 + (n as f32) * 6.0;
        self.fixtures.push(screen);
        self.fixtures.len() - 1
    }

    /// First DMX address past every patched fixture's full footprint. Uses
    /// [`FixturePatch::last_address`], which honors the profile-derived
    /// `footprint` — `ChannelMap::max_channel` alone under-counts fixtures
    /// whose profile ENDS with `Custom` channels (the map drops them), which
    /// made successive +Fixture adds overlap live channels.
    pub fn next_start_address(&self) -> u16 {
        self.fixtures
            .iter()
            .filter_map(|f| f.last_address())
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1)
    }

    pub fn add_stage_fixture(&mut self, preset: StageFixturePresetKind) -> usize {
        let ordinal = self.count_preset(preset);
        let start = self.next_start_address();
        // Anchor new fixtures to the centroid of any existing same-kind cluster
        // so the operator-derived trusses do not extend out from under them
        // when more lights are added - the new fixture lands on the same bar
        // as its peers and the truss geometry stays put. Only the very first
        // fixture of a preset falls back to the static default_position fan.
        let same_kind: Vec<&FixturePatch> = self
            .fixtures
            .iter()
            .filter(|f| f.preset_kind() == Some(preset))
            .collect();
        let (position, tilt) = if same_kind.is_empty() {
            (preset.default_position(ordinal), preset.default_tilt(ordinal))
        } else {
            let n = same_kind.len() as f32;
            let position = StageVec3 {
                x: same_kind.iter().map(|f| f.position.x).sum::<f32>() / n,
                y: same_kind.iter().map(|f| f.position.y).sum::<f32>() / n,
                z: same_kind.iter().map(|f| f.position.z).sum::<f32>() / n,
            };
            let tilt = same_kind[0].rotation.x;
            (position, tilt)
        };
        let fixture = FixturePatch::from_stage_preset(preset, ordinal, self.universe, start)
            .at(position)
            .tilted(tilt);
        self.fixtures.push(fixture);
        self.fixtures.len() - 1
    }

    pub fn fixtures_for_universe(&self, universe: u16) -> impl Iterator<Item = &FixturePatch> {
        self.fixtures.iter().filter(move |f| f.universe == universe)
    }
}

impl FixturePatch {
    /// The highest DMX address this fixture occupies — the larger of the
    /// profile-footprint span (`start_address + footprint - 1`, when the
    /// footprint is known) and the ChannelMap-derived span. The allocator
    /// keys off this so trailing `Custom` channels (dropped from the map)
    /// still reserve their addresses. `None` only when the footprint is
    /// unknown AND the map is empty.
    pub fn last_address(&self) -> Option<u16> {
        let map_end = self.channels.max_channel();
        let footprint_end = (self.footprint > 0)
            .then(|| self.start_address.saturating_add(self.footprint - 1));
        // `Option` ordering: `None < Some(_)`, so whichever span is known wins.
        map_end.max(footprint_end)
    }

    pub fn preset_kind(&self) -> Option<StageFixturePresetKind> {
        if self.tags.iter().any(|t| t == "shehds-8-segment-strobe") { Some(StageFixturePresetKind::ShehdsRgbSegmentStrobe) }
        else if self.tags.iter().any(|t| t == "shehds-bar-mover") { Some(StageFixturePresetKind::ShehdsRgbwBarMover) }
        else if self.tags.iter().any(|t| t == "shehds-230w-7r") { Some(StageFixturePresetKind::Shehds230w7rBeam) }
        else if self.tags.iter().any(|t| t == "scorpion-dual-rgb") { Some(StageFixturePresetKind::ScorpionDualRgbLaser) }
        else if self.tags.iter().any(|t| t == "shehds-1w-scan-laser") { Some(StageFixturePresetKind::ShehdsRgbScanLaser) }
        else if self.tags.iter().any(|t| t == "chauvet-obsession") { Some(StageFixturePresetKind::ChauvetObsessionGobo) }
        else if self.tags.iter().any(|t| t == "led-side-panel") { Some(StageFixturePresetKind::LedSidePanel) }
        else { None }
    }

    pub fn from_stage_preset(preset: StageFixturePresetKind, ordinal: usize, universe: u16, start_address: u16) -> Self {
        match preset {
            StageFixturePresetKind::ShehdsRgbSegmentStrobe => Self::demo_strobe(&format!("shehds-strobe-{}", ordinal + 1), universe, start_address)
                .named(&format!("SHEHDS strobe {}", ordinal + 1))
                .tagged("shehds-8-segment-strobe"),
            StageFixturePresetKind::ShehdsRgbwBarMover => Self::demo_bar_mover(&format!("shehds-bar-mover-{}", ordinal + 1), universe, start_address)
                .named(&format!("SHEHDS bar mover {}", ordinal + 1)),
            StageFixturePresetKind::Shehds230w7rBeam => Self::demo_beam_mover(&format!("shehds-7r-beam-{}", ordinal + 1), universe, start_address)
                .named(&format!("SHEHDS 7R beam {}", ordinal + 1)),
            StageFixturePresetKind::ScorpionDualRgbLaser => Self::demo_laser_macro(&format!("scorpion-dual-rgb-{}", ordinal + 1), universe, start_address)
                .named(&format!("Scorpion Dual RGB {}", ordinal + 1))
                .tagged("scorpion-dual-rgb"),
            StageFixturePresetKind::ShehdsRgbScanLaser => Self::demo_laser_macro(&format!("shehds-scan-laser-{}", ordinal + 1), universe, start_address)
                .named(&format!("SHEHDS scan laser {}", ordinal + 1))
                .tagged("shehds-1w-scan-laser"),
            StageFixturePresetKind::ChauvetObsessionGobo => Self::demo_gobo_flower(&format!("chauvet-obsession-{}", ordinal + 1), universe, start_address)
                .named(&format!("Chauvet Obsession {}", ordinal + 1)),
            StageFixturePresetKind::LedSidePanel => Self::demo_led_panel(&format!("led-side-panel-{}", ordinal + 1), universe, start_address)
                .named(&format!("LED side panel {}", ordinal + 1)),
        }
    }

    /// Place this fixture at a stage-space position (builder style).
    pub fn at(mut self, position: StageVec3) -> Self {
        self.position = position;
        self
    }

    /// Aim this fixture up by `pitch_deg` of mounting tilt - rotates the beam
    /// up from its default downward aim (~90 = horizontal, ~180 = straight
    /// up); negative tilts it further down; 0 keeps the default aim. Stored in
    /// `rotation.x`; the virtual stage renderer pitches the beam/fan along it.
    pub fn tilted(mut self, pitch_deg: f32) -> Self {
        self.rotation.x = pitch_deg;
        self
    }

    /// Set this fixture's display name (builder style). The virtual stage
    /// shows it in the fixture label and the per-fixture control panel.
    pub fn named(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn tagged(mut self, tag: &str) -> Self {
        if !self.tags.iter().any(|t| t == tag) {
            self.tags.push(tag.to_string());
        }
        self
    }

    pub fn with_visual_kind(mut self, visual_kind: StageVisualKind) -> Self {
        self.visual_kind = visual_kind;
        self
    }

    /// Build a fresh fixture from a NAS ATLABASE profile. Used by the
    /// +Fixture palette: the operator picks a profile, the renderer hands
    /// the result back as a local-only patch entry; the sync worker pushes
    /// it to SaaS on the next flush.
    pub fn from_profile(
        profile: &FixtureProfile,
        ordinal: usize,
        universe: u16,
        start_address: u16,
    ) -> Self {
        let id = format!("{}-{}", profile.id, ordinal + 1);
        let name = if profile.manufacturer.is_empty() {
            format!("{} {}", profile.model, ordinal + 1)
        } else {
            format!("{} {} {}", profile.manufacturer, profile.model, ordinal + 1)
        };
        let channels = ChannelMap::from_profile(start_address, profile);
        let (role, visual_kind) = role_and_kind_for(profile.kind);
        Self {
            id,
            name,
            universe,
            start_address,
            profile_id: profile.id.clone(),
            mode: profile.mode.clone(),
            position: StageVec3 {
                x: 0.0,
                y: 6.0,
                z: 6.0,
            },
            rotation: StageVec3::zero(),
            scale: StageVec3::one(),
            role,
            visual_kind,
            tags: Vec::new(),
            channels,
            // The profile's full channel count, NOT the ChannelMap span — the
            // map drops Custom channels, and several real profiles end with
            // them, so the span under-counts and the next add would overlap.
            footprint: profile.channel_count,
            screen: None,
        }
    }

    pub fn demo_bar_mover(id: &str, universe: u16, start_address: u16) -> Self {
        Self::demo_moving_head(id, universe, start_address)
            .tagged("shehds-bar-mover")
            .with_visual_kind(StageVisualKind::Beam)
    }

    pub fn demo_beam_mover(id: &str, universe: u16, start_address: u16) -> Self {
        Self::demo_moving_head(id, universe, start_address)
            .tagged("shehds-230w-7r")
            .with_visual_kind(StageVisualKind::Beam)
    }

    pub fn demo_gobo_flower(id: &str, universe: u16, start_address: u16) -> Self {
        Self::demo_moving_head(id, universe, start_address)
            .tagged("chauvet-obsession")
            .with_visual_kind(StageVisualKind::Gobo)
    }

    pub fn demo_led_panel(id: &str, universe: u16, start_address: u16) -> Self {
        Self {
            id: id.to_string(),
            name: "REACT demo LED panel".to_string(),
            universe,
            start_address,
            profile_id: "react.demo.led-panel".to_string(),
            mode: "RGBW panel".to_string(),
            position: StageVec3 { x: 12.8, y: 6.8, z: 4.6 },
            rotation: StageVec3::zero(),
            scale: StageVec3 { x: 1.7, y: 1.0, z: 2.6 },
            role: FixtureRole::Wash,
            visual_kind: StageVisualKind::LedPanel,
            tags: vec!["demo".to_string(), "led-panel".to_string(), "led-side-panel".to_string()],
            channels: ChannelMap::led_panel(start_address),
            // Demo maps are hand-built and contiguous — the ChannelMap span IS
            // the footprint, so the unknown sentinel allocates identically.
            footprint: 0,
            screen: None,
        }
    }

    /// Demo LED strobe: RGB + dimmer + strobe, deck-mounted.
    pub fn demo_strobe(id: &str, universe: u16, start_address: u16) -> Self {
        Self {
            id: id.to_string(),
            name: "REACT demo LED strobe".to_string(),
            universe,
            start_address,
            profile_id: "react.demo.led-strobe".to_string(),
            mode: "RGB strobe".to_string(),
            position: StageVec3 { x: 0.0, y: 1.2, z: 0.15 },
            rotation: StageVec3::zero(),
            scale: StageVec3::one(),
            role: FixtureRole::Strobe,
            visual_kind: StageVisualKind::Strobe,
            tags: vec!["demo".to_string(), "strobe".to_string()],
            channels: ChannelMap::led_strobe(start_address),
            footprint: 0,
            screen: None,
        }
    }

    pub fn demo_rgbw_wash(id: &str, universe: u16, start_address: u16) -> Self {
        Self {
            id: id.to_string(),
            name: "REACT demo RGBW wash".to_string(),
            universe,
            start_address,
            profile_id: "react.demo.rgbw-dimmer".to_string(),
            mode: "RGBW dimmer".to_string(),
            position: StageVec3 { x: -2.5, y: 2.0, z: 1.0 },
            rotation: StageVec3::zero(),
            scale: StageVec3::one(),
            role: FixtureRole::Wash,
            visual_kind: StageVisualKind::Wash,
            tags: vec!["demo".to_string(), "wash".to_string()],
            channels: ChannelMap::rgbw_dimmer(start_address),
            footprint: 0,
            screen: None,
        }
    }

    pub fn demo_moving_head(id: &str, universe: u16, start_address: u16) -> Self {
        Self {
            id: id.to_string(),
            name: "REACT demo moving head".to_string(),
            universe,
            start_address,
            profile_id: "open-fixture-library-sample.moving-head-spot-beam.12-channel-spot".to_string(),
            mode: "12-channel spot".to_string(),
            position: StageVec3 { x: 0.0, y: 2.5, z: 2.0 },
            rotation: StageVec3::zero(),
            scale: StageVec3::one(),
            role: FixtureRole::Accent,
            visual_kind: StageVisualKind::Gobo,
            tags: vec!["demo".to_string(), "moving-head".to_string(), "gobo".to_string()],
            channels: ChannelMap::moving_head_spot(start_address),
            footprint: 0,
            screen: None,
        }
    }

    pub fn demo_laser_macro(id: &str, universe: u16, start_address: u16) -> Self {
        Self {
            id: id.to_string(),
            name: "REACT demo laser macro".to_string(),
            universe,
            start_address,
            profile_id: "open-fixture-library-sample.simple-dmx-laser-macro.4-channel-macro".to_string(),
            mode: "4-channel macro".to_string(),
            position: StageVec3 { x: 2.5, y: 2.0, z: 1.0 },
            rotation: StageVec3::zero(),
            scale: StageVec3::one(),
            role: FixtureRole::Strobe,
            visual_kind: StageVisualKind::Laser,
            tags: vec!["demo".to_string(), "laser".to_string()],
            channels: ChannelMap::laser_macro(start_address),
            footprint: 0,
            screen: None,
        }
    }

    /// A placeable video-screen fixture (SaaS `kind='Screen'`): a plain surface
    /// that plays a chosen video, NOT a DMX light. `screen` is `Some` so the
    /// renderer routes it to the screen draw; role/visual_kind stay in-set
    /// (Wash/LedPanel) so a SaaS round-trip never trips the strict-enum guard.
    pub fn demo_screen(id: &str, universe: u16, start_address: u16) -> Self {
        Self {
            id: id.to_string(),
            name: "Screen".to_string(),
            universe,
            start_address,
            profile_id: "react.screen".to_string(),
            mode: "screen".to_string(),
            // Upstage-center, eye height — the operator drags it into place.
            position: StageVec3 { x: 0.0, y: 9.0, z: 4.2 },
            rotation: StageVec3::zero(),
            scale: StageVec3::one(),
            role: FixtureRole::Wash,
            visual_kind: StageVisualKind::LedPanel,
            tags: vec!["screen".to_string()],
            channels: ChannelMap::rgbw_dimmer(start_address),
            footprint: 0,
            screen: Some(ScreenConfig::default()),
        }
    }
}

pub fn channel_index(channel: u16) -> Option<usize> {
    if (DMX_MIN_CHANNEL..=DMX_MAX_CHANNEL).contains(&channel) {
        Some((channel - 1) as usize)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dmx_channel_bounds_are_one_based() {
        assert_eq!(channel_index(1), Some(0));
        assert_eq!(channel_index(512), Some(511));
        assert_eq!(channel_index(0), None);
        assert_eq!(channel_index(513), None);
    }

    #[test]
    fn demo_view_presets_expose_single_foh_base() {
        let presets = StageViewPreset::demo_presets();
        let ids: Vec<&str> = presets.iter().map(|p| p.id.as_str()).collect();

        assert_eq!(ids, vec!["foh"]);
    }

    #[test]
    fn demo_theme_intent_keeps_output_mapping_downstream() {
        let theme_state = ThemeState::default();

        assert_eq!(theme_state.intent.source.bpm, Some(122.0));
        assert_eq!(theme_state.intent.source.bar_phase, 0.0);
        assert_eq!(theme_state.intent.source.playback_position_seconds, Some(0.0));
        assert_eq!(
            theme_state.intent.source.track_notes.rekordbox_comments.as_deref(),
            Some("cool relaxed blues fades to black like under ocean themed")
        );
        assert_eq!(
            theme_state.intent.source.track_notes.notes_from_dj.as_deref(),
            Some("cool relaxed blues fades to black like under ocean themed")
        );
        assert!(theme_state.intent.source.track_notes.mood_theme_hints.iter().any(|v| v == "cool relaxed blues"));
        assert!(theme_state.intent.source.track_notes.key_moments_drops.iter().any(|v| v.contains("fade to black")));
        assert!(theme_state.intent.source.track_notes.visual_cue_hints.iter().any(|v| v.contains("underwater")));
        assert!(theme_state.intent.source.track_notes.avoid_notes.iter().any(|v| v.contains("harsh")));
        assert!(theme_state.intent.source.track_notes.prefer_notes.iter().any(|v| v.contains("blue")));
        assert_eq!(theme_state.active.id, "internal-spike-theme");
        assert!(theme_state.intent.output_expressions.contains(&OutputExpressionKind::ReactVideoSurface));
        assert!(theme_state.intent.output_expressions.contains(&OutputExpressionKind::LightingInterop));
    }

    #[test]
    fn demo_virtual_stage_assigns_semantic_visual_kinds() {
        let mut patch = DmxPatch::demo_virtual_stage(0, 1);
        assert_eq!(patch.count_preset(StageFixturePresetKind::ShehdsRgbSegmentStrobe), 4);
        assert_eq!(patch.count_preset(StageFixturePresetKind::ShehdsRgbwBarMover), 2);
        assert_eq!(patch.count_preset(StageFixturePresetKind::Shehds230w7rBeam), 4);
        assert_eq!(patch.count_preset(StageFixturePresetKind::ScorpionDualRgbLaser), 1);
        assert_eq!(patch.count_preset(StageFixturePresetKind::ShehdsRgbScanLaser), 2);
        assert_eq!(patch.count_preset(StageFixturePresetKind::ChauvetObsessionGobo), 2);

        let strobe = patch.fixtures.iter().find(|f| f.id == "shehds-strobe-1").unwrap();
        let beam = patch.fixtures.iter().find(|f| f.id == "shehds-7r-beam-1").unwrap();
        let laser = patch.fixtures.iter().find(|f| f.id == "scorpion-dual-rgb-1").unwrap();
        let gobo = patch.fixtures.iter().find(|f| f.id == "chauvet-obsession-1").unwrap();

        assert_eq!(strobe.visual_kind, StageVisualKind::Strobe);
        assert_eq!(beam.visual_kind, StageVisualKind::Beam);
        assert_eq!(laser.visual_kind, StageVisualKind::Laser);
        assert_eq!(gobo.visual_kind, StageVisualKind::Gobo);

        let idx = patch.add_stage_fixture(StageFixturePresetKind::LedSidePanel);
        assert_eq!(patch.fixtures[idx].visual_kind, StageVisualKind::LedPanel);
        assert_eq!(patch.count_preset(StageFixturePresetKind::LedSidePanel), 1);
    }

    #[test]
    fn venue_kind_key_round_trips() {
        for &v in VenueKind::all() {
            assert_eq!(VenueKind::from_key(v.as_key()), Some(v), "round-trip {:?}", v);
        }
        assert_eq!(VenueKind::from_key("bogus"), None);
        assert_eq!(VenueKind::default(), VenueKind::ConcertStage);
    }

    #[test]
    fn frame_set_channel_ignores_out_of_range_addresses() {
        let mut frame = DmxFrame::new(0, 9, 1234, false);
        frame.set_channel(Some(1), 255);
        frame.set_channel(Some(0), 128);
        frame.set_channel(Some(513), 128);
        assert_eq!(frame.get_channel(1), Some(255));
        assert_eq!(frame.data.iter().filter(|v| **v != 0).count(), 1);
    }

    // --- UV channel slot (UV mode: only UV-capable emitters lit) -----------

    #[test]
    fn from_profile_maps_uv_into_the_uv_slot() {
        // A SHEHDS-flat-par-shaped profile: RGBW + UV. The UV channel must
        // land in the first-class `uv` slot (it used to fall to Custom and
        // be dropped), anchored at the absolute patch address.
        let ch = |attribute: DmxAttribute, channel_offset: u16| ChannelDefinition {
            attribute,
            channel_offset,
            default_value: 0,
            blackout_value: 0,
            detail: None,
            source_attribute: None,
        };
        let profile = FixtureProfile {
            id: "test.uv-par".to_string(),
            manufacturer: "Test".to_string(),
            model: "UV Par".to_string(),
            mode: "6 Channel".to_string(),
            kind: FixtureKind::Wash,
            housing: FixtureHousing::Par,
            channel_count: 6,
            channels: vec![
                ch(DmxAttribute::Dimmer, 1),
                ch(DmxAttribute::Red, 2),
                ch(DmxAttribute::Green, 3),
                ch(DmxAttribute::Blue, 4),
                ch(DmxAttribute::White, 5),
                ch(DmxAttribute::Uv, 6),
            ],
            capabilities: vec![],
        };
        let map = ChannelMap::from_profile(10, &profile);
        assert_eq!(map.uv, Some(15), "UV at offset 6, base 10 -> abs 15");
        // The UV slot participates in the address span like every other slot.
        assert_eq!(map.max_channel(), Some(15));
    }

    #[test]
    fn channel_map_uv_defaults_to_none_for_old_saves() {
        // A pre-UV persisted ChannelMap (offline cache / SaaS payload) has no
        // `uv` key — it must load as uv: None, not fail.
        let mut v = serde_json::to_value(ChannelMap::rgbw_dimmer(1)).unwrap();
        v.as_object_mut().unwrap().remove("uv");
        let restored: ChannelMap = serde_json::from_value(v).unwrap();
        assert_eq!(restored.uv, None);
        assert_eq!(restored.dimmer, Some(1));
    }

    // --- DMX address allocation honors the full profile footprint ----------

    /// A profile shaped like atla-scorpion-laser.json for allocator tests:
    /// 10 channels where the LAST ChannelMap-mappable attribute (Tilt) sits
    /// at offset 7 and offsets 8-10 are Custom STATIC axis channels the map
    /// drops — exactly the shape that made `next_start_address` under-count.
    fn trailing_custom_profile() -> FixtureProfile {
        let ch = |attribute: DmxAttribute, channel_offset: u16| ChannelDefinition {
            attribute,
            channel_offset,
            default_value: 0,
            blackout_value: 0,
            detail: None,
            source_attribute: None,
        };
        FixtureProfile {
            id: "test.trailing-custom".to_string(),
            manufacturer: "Test".to_string(),
            model: "Trailing Custom Laser".to_string(),
            mode: "10 Channel".to_string(),
            kind: FixtureKind::Laser,
            housing: FixtureHousing::ScanLaser,
            channel_count: 10,
            channels: vec![
                ch(DmxAttribute::Custom("Control mode".to_string()), 1),
                ch(DmxAttribute::GoboSlot, 2),
                ch(DmxAttribute::ColorWheel, 3),
                ch(DmxAttribute::Strobe, 4),
                ch(DmxAttribute::Zoom, 5),
                ch(DmxAttribute::Pan, 6),
                ch(DmxAttribute::Tilt, 7),
                ch(DmxAttribute::Custom("X Axis Rolling".to_string()), 8),
                ch(DmxAttribute::Custom("Y Axis Rolling".to_string()), 9),
                ch(DmxAttribute::Custom("Z Axis Rolling".to_string()), 10),
            ],
            capabilities: vec![],
        }
    }

    #[test]
    fn add_fixture_from_profile_reserves_the_full_profile_footprint() {
        let mut patch = DmxPatch { universe: 0, fixtures: vec![], trusses: vec![], venue: None };
        let profile = trailing_custom_profile();

        let first = patch.add_fixture_from_profile(&profile);
        assert_eq!(patch.fixtures[first].start_address, 1);
        assert_eq!(patch.fixtures[first].footprint, 10);
        // The ChannelMap span ends at Tilt (offset 7) — the Custom channels
        // 8-10 are dropped from the map, which is what under-counted the
        // footprint before the fix.
        assert_eq!(patch.fixtures[first].channels.max_channel(), Some(7));
        assert_eq!(patch.fixtures[first].last_address(), Some(10));

        let second = patch.add_fixture_from_profile(&profile);
        assert_eq!(
            patch.fixtures[second].start_address, 11,
            "second add must start past the full 10-channel footprint \
             (pre-fix it landed on 8, overlapping the live axis channels)"
        );
    }

    #[test]
    fn next_start_address_falls_back_to_channelmap_span_when_footprint_unknown() {
        // Old saves / SaaS rows deserialize with footprint = 0 (serde default)
        // — the allocator must keep the pre-footprint ChannelMap behavior.
        let mut patch = DmxPatch { universe: 0, fixtures: vec![], trusses: vec![], venue: None };
        let wash = FixturePatch::demo_rgbw_wash("wash-1", 0, 1); // map spans 1..=5
        assert_eq!(wash.footprint, 0);
        patch.fixtures.push(wash);
        assert_eq!(patch.next_start_address(), 6);
    }

    #[test]
    fn last_address_takes_the_larger_of_footprint_and_channelmap_span() {
        let mut f = FixturePatch::demo_rgbw_wash("w", 0, 10); // map spans 10..=14
        assert_eq!(f.last_address(), Some(14));
        f.footprint = 3; // footprint shorter than the mapped span — map wins
        assert_eq!(f.last_address(), Some(14));
        f.footprint = 8; // footprint reaches past the mapped span — footprint wins
        assert_eq!(f.last_address(), Some(17));
    }

    #[test]
    fn fixture_patch_footprint_defaults_to_zero_for_old_saves() {
        // A pre-footprint persisted patch (offline cache / SaaS payload) has
        // no `footprint` key — it must load with the unknown sentinel, not fail.
        let mut v = serde_json::to_value(FixturePatch::demo_rgbw_wash("w", 0, 1)).unwrap();
        v.as_object_mut().unwrap().remove("footprint");
        let restored: FixturePatch = serde_json::from_value(v).unwrap();
        assert_eq!(restored.footprint, 0);
    }

    // --- Segment-FX persistence serializes deterministically ---------------

    #[test]
    fn segfx_json_serializes_identically_across_repeated_calls() {
        let mut ovs = FixtureOverrides::default();
        for (id, fx, speed) in [
            ("delta", SegmentPattern::Chase, 1.5f32),
            ("alpha", SegmentPattern::Rainbow, 2.0),
            ("charlie", SegmentPattern::Bounce, 0.5),
        ] {
            let mut ov = ovs.get(id);
            ov.segment_fx = fx;
            ov.segment_speed = speed;
            ovs.set(id, ov);
        }
        // Keys come out in sorted (BTreeMap) order — pinned exactly so the
        // app-loop string-diff debounce sees one stable representation. The
        // old per-call HashMap reordered 2+ entries and re-armed it forever.
        let expected = r#"{"alpha":["Rainbow",2.0],"charlie":["Bounce",0.5],"delta":["Chase",1.5]}"#;
        assert_eq!(ovs.segfx_json(), expected);
        for _ in 0..8 {
            assert_eq!(ovs.segfx_json(), expected, "repeated calls must serialize identically");
        }
        // Round-trips through the restore path.
        let mut restored = FixtureOverrides::default();
        restored.load_segfx_json(&ovs.segfx_json());
        assert_eq!(restored.segfx_json(), expected);
    }
}

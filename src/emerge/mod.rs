//! EMeRGE — the music-shape → lighting-intent engine.
//!
//! The protected IP, lifted out of the transport-named folders (`dmx/`, `osc/`)
//! into one cohesive module. The wire transports (`dmx/`, `osc/`, `hue/`,
//! `wled/`) are dumb sinks that consume the types re-exported here. This is a
//! pure, behaviour-preserving extraction — no logic changes, only module paths.

pub mod cue;
pub mod engine;
pub mod harmony;
pub mod intent;
pub mod contracts;
pub mod profiles;
pub mod style;

// Flat re-exports mirror the surface the transport mod.rs files exposed, so
// callers read `crate::emerge::X` the same way they used to read
// `crate::osc::X` / `crate::dmx::X`.
pub use harmony::{
    centroid_concentration, centroid_hue, chroma_centroid, emphasize_dominant, ChordTracker,
    GenreProfile, KeySignal, KeySignalState, PaletteBias, StrobeZoneMinimum, TonnetzTracker,
    ZoneThresholds,
};
pub use intent::{
    add_bipolar, add_clamped, bipolar_wave, render_fixture_intent, render_fixture_intent_aimed,
    render_fixture_intent_placed, renders_as_galaxy_effect, scene_cell_rgbw,
    segment_rgbw, ArcSignals, ArcState, CueLaneIntent, CuePriority, FixtureBrain,
    FixtureBrainContext, FixtureFamily, FixtureFamilyCapabilities, FixtureFrameContext,
    FixtureGenreProfile, FixtureIntentState, LfoBankConfig, LfoModulation, LfoTarget, LfoWaveform,
    MoveFigure, MoveLook, MovePosture, MovementDirector, MovementFrame, MoverSlot,
    OscillatorBinding, MOVER_PAN_WINDOW_DEG, MOVER_TILT_WINDOW_DEG,
    OscillatorFrameSnapshot, OscillatorInterval, OscillatorShape, PhaseState, ShowClockSnapshot,
    ShowClockSource, TopologyState,
};
pub use intent::{
    scene, scene_for, BeamLook, Look, Scene, SceneDirector, SceneFrame, SceneKind, Section,
    StageLayout, StagePlace, Transition,
};
pub use cue::{
    apply_cue_bindings, family_for, link_clock, prolink_clock, ActiveCue, AutoBlinderPhase,
    CapabilityMiss, ClockInputs, ClockOverride, CueApplyReport, CueDto, CueEngine, CueListResponse,
    EnergyZone, LightingCueWorker, ManualTriggers, Palette, ShowBrain, ShowBrainFrame,
    TriggerButton, TriggerConfig, STOP_FADE_SECS,
};
pub use contracts::{
    segments_for_profile, CdjPerformanceState, ChannelDefinition, ChannelMap, DeckPlaybackState,
    DmxAttribute, DmxFrame, DmxPatch, DmxPayload, FixtureCapability, FixtureHousing, FixtureKind,
    FixtureOverride, FixtureOverrides, FixturePatch, FixtureProfile, FixtureRole,
    OutputExpressionKind, ScreenConfig, ScreenFit, SegmentChannels, SegmentPattern,
    StageFixturePresetKind, StageVec3, StageViewPreset, StageViewPresetKind, StageVisualKind,
    ShowTheme, ThemeIntent, ThemeState, TrackCreativeNotes, Truss, TrussAxis, TrussKind, VenueKind,
    DMX_CHANNEL_COUNT, DMX_MAX_CHANNEL, DMX_MIN_CHANNEL,
};
pub use engine::{EngineTick, LightingEngine, LightingInput, OscUiState, STALE_INPUT_SILENCE};
pub use profiles::{load_ofl_fixture_profile, FixtureCommerceMetadata, FixtureProfileSource};
pub use style::LightingStyle;

//! EMeRGE intent engine — per-fixture lighting intent from the cue snapshot.
//!
//! The fixture brain (per-family arc/topology state), the oscillator/LFO motion
//! carriers, and the renderer that turns a `CueSnapshot` into per-fixture
//! `FixtureIntentState`. Moved verbatim out of `dmx/` — transport-named only by
//! history; this is engine, not wire framing.

mod fixture_brain;
mod intent;
mod lfo;
mod movement;
mod oscillator;

pub use fixture_brain::{
    ArcSignals, ArcState, CueLaneIntent, CuePriority, FixtureBrain, FixtureBrainContext,
    FixtureFamily, FixtureFamilyCapabilities, FixtureFrameContext, FixtureGenreProfile,
    PhaseState, ShowClockSnapshot, ShowClockSource, TopologyState,
};
pub use intent::{
    render_fixture_intent, render_fixture_intent_aimed, renders_as_galaxy_effect, segment_rgbw,
    FixtureIntentState,
};
pub use lfo::{add_clamped, LfoBankConfig, LfoModulation, LfoTarget, LfoWaveform};
pub use movement::{
    choose_look, choose_posture, figure_point, FigurePoint, MotionContext, MoveFigure, MoveLook,
    MovePosture, MovementDirector, MovementFrame, MoverSlot, MOVER_PAN_WINDOW_DEG,
    MOVER_TILT_WINDOW_DEG,
};
pub use oscillator::{
    add_bipolar, bipolar_wave, OscillatorBinding, OscillatorFrameSnapshot, OscillatorInterval,
    OscillatorShape,
};

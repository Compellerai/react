//! EMeRGE cue engine — show-clock resolution, cue decisioning, and cue lifecycle.
//!
//! The OSC-side cue decision engine (audio → zone/palette/strobe), the manual
//! trigger state, the show clock + fixture-family classifier, and the cue
//! launch/apply/fade machinery with its async worker. Moved verbatim out of
//! `dmx/` and `osc/` — engine, not wire framing.
//!
//! Submodules are `pub(crate)` so the wire transports (which legitimately
//! consume cue output) can reach module-path-only helpers like
//! `lighting_cue::eval_segment` / `operator_segment_kind`.

pub(crate) mod cue_engine;
pub(crate) mod dynamics;
pub(crate) mod lighting_cue;
pub(crate) mod lighting_cue_worker;
pub(crate) mod show_brain;
pub(crate) mod triggers;

pub use cue_engine::{AutoBlinderPhase, CueEngine, EnergyZone, Palette};
pub use lighting_cue::{
    apply_cue_bindings, ActiveCue, CapabilityMiss, CueApplyReport, CueDto, CueListResponse,
    STOP_FADE_SECS,
};
pub use lighting_cue_worker::LightingCueWorker;
pub use show_brain::{
    family_for, link_clock, prolink_clock, ClockInputs, ClockOverride, ShowBrain, ShowBrainFrame,
};
pub use triggers::{ManualTriggers, TriggerButton, TriggerConfig};

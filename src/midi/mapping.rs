//! MIDI mapping types and control targets

use super::{MidiEvent, ParsedMidiMessage};
use serde::{Deserialize, Serialize};

/// Unique identifier for a MIDI control
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct MidiKey {
    /// Device name (port identifier)
    pub device_name: String,
    /// MIDI channel (0-15)
    pub channel: u8,
    /// Message type
    pub msg_type: MidiMsgType,
    /// CC number, note number, or 0 for pitch bend
    pub index: u8,
}

/// MIDI message type for mapping
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum MidiMsgType {
    ControlChange,
    NoteOn,
    NoteOff,
    PitchBend,
}

/// Target control that can be mapped to MIDI
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum ControlTarget {
    // Audio controls
    AudioGain,
    KickSensitivity,

    // Visual controls
    ProjectMVisibility,
    VideoVisibility,
    VideoScale,
    /// Compel video luma key (0.0-1.0) - dark pixels become transparent
    VideoLumaKey,

    // Overlay controls
    LogoOpacity,
    LogoScale,
    LogoVisible,

    // Output controls
    ToggleNdi,
    ToggleNdiAudio,
    ToggleOutputWindow,
    ToggleOutputFullscreen,
    ToggleRecording,

    // Actions (triggered on note/button press)
    RandomPreset,
    ToggleProjectM,
    ToggleVideo,
    NextPreset,
    PrevPreset,

    // Video list browsing (encoder rotation + push)
    BrowseVideoNext,
    BrowseVideoPrev,
    LoadVideo,

    // Preset list browsing (encoder rotation + push)
    BrowsePresetNext,
    BrowsePresetPrev,
    LoadPreset,
    /// Single encoder for preset browsing (auto-detects direction from CC value)
    BrowsePresetEncoder,
    /// Single encoder for video browsing (auto-detects direction from CC value)
    BrowseVideoEncoder,

    // Compel (video) list browsing with overlay (encoder rotation + push)
    /// Single encoder for compel browsing (auto-detects direction from CC value)
    BrowseCompelEncoder,
    /// Load the currently browsed compel
    LoadCompel,

    // Camera tracking influence controls (affects accent probability, NOT video opacity)
    /// Camera tracking influence (visibility slider) - parameter is camera index
    CameraVisibility(u32),
    /// Camera enable toggle - parameter is camera index
    CameraEnable(u32),

    // Camera media background controls (visual layers, NOT tracking)
    /// Camera background visibility (alpha compositing) - parameter is camera index
    CameraMediaVisibility(u32),
    /// Camera background enable toggle - parameter is camera index
    CameraMediaEnable(u32),
    /// Camera momentary solo (hold = solo this camera, release = restore previous state)
    CameraSolo(u32),

    // Harmonic Field display toggles
    /// Toggle Chroma key overlay
    ToggleChromaKey,
    /// Toggle Math overlay
    ToggleMathOverlay,
    /// Toggle Performance HUD
    TogglePerfHud,
    /// Toggle Debug HUD
    ToggleDebugHud,
    /// Toggle QS Wheel display
    ToggleQsWheel,

    // Manual lighting triggers (hold-to-activate)
    /// Strobe trigger (hold = active, release = inactive)
    TriggerStrobe,
    /// Blackout trigger (hold = active, release = inactive)
    TriggerBlackout,
    /// Blinder trigger (hold = active, release = inactive)
    TriggerBlinder,
    /// UV mode trigger (hold = active, release = inactive) — only
    /// UV-capable emitters lit, everything else dark
    TriggerUv,
    /// Fog trigger (hold = active, release = inactive + burst)
    TriggerFog,

    /// Force-enter buildup macro (BlindersHold) regardless of audio analysis
    ForceBuildup,

    // Camera layer controls
    /// Camera layer luma key (0.0-1.0) - dark camera areas become transparent
    CameraLumaKey,

    // Lighting controls
    /// Global brightness master dimmer (0.0-1.0), sent as /global/brightness
    GlobalBrightness,

    /// Operator UV accent level (0.0-1.0) scaling the GigBAR UV layer
    /// (par ch4/9 + pod ch22) on the DMX path
    UvLevel,

    /// Toggle "Use Genre for Tempo" — when on, the selected genre's BPM range
    /// guides REACT's autonomous tempo octave (double- vs half-time). External
    /// Link/Pro DJ clocks ignore this entirely.
    GenreTempoGuidance,

    // Lighting style posture (Auto/Chill/Energetic/Banger): four discrete
    // set buttons + a cycle, so a controller can pin a posture on one pad or
    // walk the trigger-row dropdown from a single button. All mutate the
    // same config field the dropdown reads, so the UI follows MIDI.
    StyleAuto,
    StyleChill,
    StyleEnergetic,
    StyleBanger,
    /// Cycle Auto → Chill → Energetic → Banger → Auto
    StyleNext,

    // Mythos engine controls (continuous 0-1)
    MythosInstability,
    MythosFieldStrength,
    MythosDisturbanceSize,
    MythosHarmonicInfluence,
    MythosDecay,
}

impl ControlTarget {
    /// Display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::AudioGain => "Audio Gain",
            Self::KickSensitivity => "Kick Sensitivity",
            Self::ProjectMVisibility => "Theme Visibility",
            Self::VideoVisibility => "Video Visibility",
            Self::VideoScale => "Video Scale",
            Self::VideoLumaKey => "Video Luma Key",
            Self::LogoOpacity => "Logo Opacity",
            Self::LogoScale => "Logo Scale",
            Self::LogoVisible => "Logo Visible",
            Self::ToggleNdi => "Toggle NDI",
            Self::ToggleNdiAudio => "Toggle NDI Audio",
            Self::ToggleOutputWindow => "Toggle Output Window",
            Self::ToggleOutputFullscreen => "Toggle Fullscreen",
            Self::ToggleRecording => "Toggle Recording",
            Self::RandomPreset => "Random Preset",
            Self::ToggleProjectM => "Toggle Themes",
            Self::ToggleVideo => "Toggle Video",
            Self::NextPreset => "Next Preset",
            Self::PrevPreset => "Previous Preset",
            Self::BrowseVideoNext => "Browse Video →",
            Self::BrowseVideoPrev => "Browse Video ←",
            Self::LoadVideo => "Load Video",
            Self::BrowsePresetNext => "Browse Preset →",
            Self::BrowsePresetPrev => "Browse Preset ←",
            Self::LoadPreset => "Load Preset",
            Self::BrowsePresetEncoder => "Browse Themes (Encoder)",
            Self::BrowseVideoEncoder => "Browse Videos (Encoder)",
            Self::BrowseCompelEncoder => "Browse Compels (Encoder)",
            Self::LoadCompel => "Load Compel",
            Self::CameraVisibility(idx) => match idx {
                0 => "Camera 1 Visibility",
                1 => "Camera 2 Visibility",
                2 => "Camera 3 Visibility",
                3 => "Camera 4 Visibility",
                _ => "Camera N Visibility",
            },
            Self::CameraEnable(idx) => match idx {
                0 => "Camera 1 Enable",
                1 => "Camera 2 Enable",
                2 => "Camera 3 Enable",
                3 => "Camera 4 Enable",
                _ => "Camera N Enable",
            },
            Self::CameraMediaVisibility(idx) => match idx {
                0 => "Camera 1 BG Visibility",
                1 => "Camera 2 BG Visibility",
                2 => "Camera 3 BG Visibility",
                3 => "Camera 4 BG Visibility",
                _ => "Camera N BG Visibility",
            },
            Self::CameraMediaEnable(idx) => match idx {
                0 => "Camera 1 BG Enable",
                1 => "Camera 2 BG Enable",
                2 => "Camera 3 BG Enable",
                3 => "Camera 4 BG Enable",
                _ => "Camera N BG Enable",
            },
            Self::CameraSolo(idx) => match idx {
                0 => "Camera 1 Solo",
                1 => "Camera 2 Solo",
                2 => "Camera 3 Solo",
                3 => "Camera 4 Solo",
                _ => "Camera N Solo",
            },
            Self::ToggleChromaKey => "Toggle Chroma",
            Self::ToggleMathOverlay => "Toggle Math",
            Self::TogglePerfHud => "Toggle Perf HUD",
            Self::ToggleDebugHud => "Toggle Debug HUD",
            Self::ToggleQsWheel => "Toggle QS Wheel",
            Self::TriggerStrobe => "Trigger Strobe",
            Self::TriggerBlackout => "Trigger Blackout",
            Self::TriggerBlinder => "Trigger Blinder",
            Self::TriggerUv => "Trigger UV",
            Self::TriggerFog => "Trigger Fog",
            Self::ForceBuildup => "Force Buildup",
            Self::CameraLumaKey => "Camera Luma Key",
            Self::GlobalBrightness => "Global Brightness",
            Self::UvLevel => "UV Level",
            Self::GenreTempoGuidance => "Use Genre for Tempo",
            Self::StyleAuto => "Style: Auto",
            Self::StyleChill => "Style: Chill",
            Self::StyleEnergetic => "Style: Energetic",
            Self::StyleBanger => "Style: Banger",
            Self::StyleNext => "Style: Next →",
            Self::MythosInstability => "Mythos Instability",
            Self::MythosFieldStrength => "Mythos Field Strength",
            Self::MythosDisturbanceSize => "Mythos Disturbance",
            Self::MythosHarmonicInfluence => "Mythos Harmonic",
            Self::MythosDecay => "Mythos Decay",
        }
    }

    /// Whether this target is a continuous control (vs momentary/trigger)
    pub fn is_continuous(&self) -> bool {
        match self {
            Self::AudioGain
            | Self::KickSensitivity
            | Self::ProjectMVisibility
            | Self::VideoVisibility
            | Self::VideoScale
            | Self::VideoLumaKey
            | Self::LogoOpacity
            | Self::LogoScale
            | Self::CameraLumaKey
            | Self::GlobalBrightness
            | Self::UvLevel
            | Self::MythosInstability
            | Self::MythosFieldStrength
            | Self::MythosDisturbanceSize
            | Self::MythosHarmonicInfluence
            | Self::MythosDecay => true,

            Self::LogoVisible
            | Self::ToggleNdi
            | Self::ToggleNdiAudio
            | Self::ToggleOutputWindow
            | Self::ToggleOutputFullscreen
            | Self::ToggleRecording
            | Self::RandomPreset
            | Self::ToggleProjectM
            | Self::ToggleVideo
            | Self::NextPreset
            | Self::PrevPreset
            | Self::BrowseVideoNext
            | Self::BrowseVideoPrev
            | Self::LoadVideo
            | Self::BrowsePresetNext
            | Self::BrowsePresetPrev
            | Self::LoadPreset
            | Self::BrowsePresetEncoder
            | Self::BrowseVideoEncoder
            | Self::BrowseCompelEncoder
            | Self::LoadCompel
            | Self::CameraEnable(_)
            | Self::CameraMediaEnable(_)
            | Self::CameraSolo(_)
            | Self::ToggleChromaKey
            | Self::ToggleMathOverlay
            | Self::TogglePerfHud
            | Self::ToggleDebugHud
            | Self::ToggleQsWheel
            | Self::TriggerStrobe
            | Self::TriggerBlackout
            | Self::TriggerBlinder
            | Self::TriggerUv
            | Self::TriggerFog
            | Self::ForceBuildup
            | Self::GenreTempoGuidance
            | Self::StyleAuto
            | Self::StyleChill
            | Self::StyleEnergetic
            | Self::StyleBanger
            | Self::StyleNext => false,

            Self::CameraVisibility(_)
            | Self::CameraMediaVisibility(_) => true,
        }
    }

    /// All available targets for UI listing
    pub fn all() -> &'static [ControlTarget] {
        &[
            Self::AudioGain,
            Self::KickSensitivity,
            Self::ProjectMVisibility,
            Self::VideoVisibility,
            Self::VideoScale,
            Self::VideoLumaKey,
            Self::LogoOpacity,
            Self::LogoScale,
            Self::LogoVisible,
            Self::ToggleNdi,
            Self::ToggleNdiAudio,
            Self::ToggleOutputWindow,
            Self::ToggleOutputFullscreen,
            Self::ToggleRecording,
            Self::RandomPreset,
            Self::ToggleProjectM,
            Self::ToggleVideo,
            Self::NextPreset,
            Self::PrevPreset,
            Self::BrowseVideoNext,
            Self::BrowseVideoPrev,
            Self::LoadVideo,
            Self::BrowsePresetNext,
            Self::BrowsePresetPrev,
            Self::LoadPreset,
            Self::BrowsePresetEncoder,
            Self::BrowseVideoEncoder,
            Self::BrowseCompelEncoder,
            Self::LoadCompel,
            Self::TriggerStrobe,
            Self::TriggerBlackout,
            Self::TriggerBlinder,
            Self::TriggerUv,
            Self::TriggerFog,
            Self::ForceBuildup,
            Self::CameraLumaKey,
            Self::GlobalBrightness,
            Self::UvLevel,
            Self::GenreTempoGuidance,
            Self::StyleAuto,
            Self::StyleChill,
            Self::StyleEnergetic,
            Self::StyleBanger,
            Self::StyleNext,
            Self::MythosInstability,
            Self::MythosFieldStrength,
            Self::MythosDisturbanceSize,
            Self::MythosHarmonicInfluence,
            Self::MythosDecay,
        ]
    }
}

/// A mapping from MIDI input to a control target
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiMapping {
    /// The MIDI key that triggers this mapping
    pub key: MidiKey,
    /// The control to affect
    pub target: ControlTarget,
    /// Min value for scaling (default 0.0)
    pub min_value: f32,
    /// Max value for scaling (default 1.0)
    pub max_value: f32,
    /// Whether to invert the value
    pub inverted: bool,
}

impl MidiKey {
    /// Create a MidiKey from a MIDI event
    pub fn from_event(event: &MidiEvent) -> Self {
        let (msg_type, index) = match &event.message {
            ParsedMidiMessage::ControlChange { controller, .. } => {
                (MidiMsgType::ControlChange, *controller)
            }
            ParsedMidiMessage::NoteOn { note, .. } => {
                (MidiMsgType::NoteOn, *note)
            }
            ParsedMidiMessage::NoteOff { note } => {
                (MidiMsgType::NoteOff, *note)
            }
            ParsedMidiMessage::PitchBend { .. } => {
                (MidiMsgType::PitchBend, 0)
            }
        };

        Self {
            device_name: event.device_name.clone(),
            channel: event.channel,
            msg_type,
            index,
        }
    }

    /// Human-readable description
    pub fn description(&self) -> String {
        let type_str = match self.msg_type {
            MidiMsgType::ControlChange => format!("CC{}", self.index),
            MidiMsgType::NoteOn => format!("Note {}", self.index),
            MidiMsgType::NoteOff => format!("Note {} Off", self.index),
            MidiMsgType::PitchBend => "Pitch Bend".to_string(),
        };
        format!("{} Ch{} {}", self.device_name, self.channel + 1, type_str)
    }
}

impl MidiMapping {
    /// Create a new mapping with default scaling
    pub fn new(key: MidiKey, target: ControlTarget) -> Self {
        Self {
            key,
            target,
            min_value: 0.0,
            max_value: 1.0,
            inverted: false,
        }
    }

    /// Normalize the MIDI value to 0.0-1.0 range, then scale to min/max
    pub fn normalize_value(&self, event: &MidiEvent) -> f32 {
        let raw = match &event.message {
            ParsedMidiMessage::ControlChange { value, .. } => *value as f32 / 127.0,
            ParsedMidiMessage::NoteOn { velocity, .. } => *velocity as f32 / 127.0,
            ParsedMidiMessage::NoteOff { .. } => 0.0,
            ParsedMidiMessage::PitchBend { value } => *value as f32 / 16383.0,
        };

        let normalized = if self.inverted { 1.0 - raw } else { raw };

        // Scale to min/max range
        self.min_value + normalized * (self.max_value - self.min_value)
    }
}

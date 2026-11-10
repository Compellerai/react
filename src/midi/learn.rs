//! MIDI learn state machine

use super::{ControlTarget, MidiEvent, MidiKey, MidiMapping, ParsedMidiMessage};
use std::time::{Duration, Instant};

/// MIDI learn state machine
#[derive(Debug, Clone)]
pub enum LearnState {
    /// Not learning
    Off,

    /// Armed and waiting for target selection or MIDI input
    Armed {
        target: Option<ControlTarget>,
        armed_at: Instant,
    },

    /// Confirming - saw first MIDI message, waiting for confirmation
    Confirm {
        target: ControlTarget,
        key: MidiKey,
        first_at: Instant,
    },
}

impl Default for LearnState {
    fn default() -> Self {
        Self::Off
    }
}

/// MIDI learn controller
pub struct MidiLearn {
    state: LearnState,
    /// Require confirmation (same key twice within timeout)
    require_confirm: bool,
    /// Confirmation timeout
    confirm_timeout: Duration,
}

impl MidiLearn {
    pub fn new() -> Self {
        Self {
            state: LearnState::Off,
            require_confirm: false,  // Immediate binding - no double-press needed
            confirm_timeout: Duration::from_secs(2),
        }
    }

    /// Get current state
    pub fn state(&self) -> &LearnState {
        &self.state
    }

    /// Check if learn mode is active
    pub fn is_active(&self) -> bool {
        !matches!(self.state, LearnState::Off)
    }

    /// Check if we're armed and have a target
    pub fn is_armed_with_target(&self) -> bool {
        matches!(self.state, LearnState::Armed { target: Some(_), .. })
    }

    /// Arm learn mode (waiting for target + MIDI)
    pub fn arm(&mut self) {
        self.state = LearnState::Armed {
            target: None,
            armed_at: Instant::now(),
        };
    }

    /// Set the target control to learn
    pub fn set_target(&mut self, target: ControlTarget) {
        if let LearnState::Armed { armed_at, .. } = self.state {
            self.state = LearnState::Armed {
                target: Some(target),
                armed_at,
            };
        }
    }

    /// Cancel learn mode
    pub fn cancel(&mut self) {
        self.state = LearnState::Off;
    }

    /// Process a MIDI event during learn mode
    /// Returns Some(mapping) if learning completed successfully
    pub fn process_event(&mut self, event: &MidiEvent) -> Option<MidiMapping> {
        // Filter out messages we don't want to learn
        if !Self::is_learnable_message(&event.message) {
            return None;
        }

        let key = MidiKey::from_event(event);

        match &self.state {
            LearnState::Off => None,

            LearnState::Armed { target: Some(target), .. } => {
                // For LoadPreset (push button), only accept NoteOn - not encoder CCs
                // This prevents accidentally learning encoder turns as the push action
                if matches!(target, ControlTarget::LoadPreset) {
                    if !matches!(event.message, ParsedMidiMessage::NoteOn { .. }) {
                        log::warn!("[MIDI Learn] Invalid input - push the encoder or press a note button");
                        return None;
                    }
                }

                if self.require_confirm {
                    // Move to confirm state
                    self.state = LearnState::Confirm {
                        target: target.clone(),
                        key,
                        first_at: Instant::now(),
                    };
                    None
                } else {
                    // Immediate binding
                    let mapping = MidiMapping::new(key, target.clone());
                    self.state = LearnState::Off;
                    Some(mapping)
                }
            }

            LearnState::Armed { target: None, .. } => {
                // Got MIDI but no target selected yet - ignore
                log::debug!("[MIDI Learn] Got input but no target selected");
                None
            }

            LearnState::Confirm { target, key: confirm_key, first_at } => {
                // Check timeout
                if first_at.elapsed() > self.confirm_timeout {
                    // Timeout - go back to armed with same target
                    self.state = LearnState::Armed {
                        target: Some(target.clone()),
                        armed_at: Instant::now(),
                    };
                    return None;
                }

                // Check if same key
                if key == *confirm_key {
                    // Confirmed! Create mapping
                    let mapping = MidiMapping::new(key, target.clone());
                    self.state = LearnState::Off;
                    Some(mapping)
                } else {
                    // Different key - reset to this new key
                    self.state = LearnState::Confirm {
                        target: target.clone(),
                        key,
                        first_at: Instant::now(),
                    };
                    None
                }
            }
        }
    }

    /// Check if a message type is learnable
    fn is_learnable_message(msg: &ParsedMidiMessage) -> bool {
        match msg {
            ParsedMidiMessage::ControlChange { .. } => true,
            ParsedMidiMessage::NoteOn { velocity, .. } => *velocity > 0,
            ParsedMidiMessage::NoteOff { .. } => false, // Don't learn from note offs
            ParsedMidiMessage::PitchBend { .. } => true,
        }
    }

    /// Get the current target if armed
    pub fn current_target(&self) -> Option<&ControlTarget> {
        match &self.state {
            LearnState::Armed { target, .. } => target.as_ref(),
            LearnState::Confirm { target, .. } => Some(target),
            LearnState::Off => None,
        }
    }

    /// Get confirmation progress (0.0-1.0) if in confirm state
    pub fn confirm_progress(&self) -> Option<f32> {
        if let LearnState::Confirm { first_at, .. } = &self.state {
            let elapsed = first_at.elapsed().as_secs_f32();
            let timeout = self.confirm_timeout.as_secs_f32();
            Some((elapsed / timeout).min(1.0))
        } else {
            None
        }
    }

    /// Set whether confirmation is required
    pub fn set_require_confirm(&mut self, require: bool) {
        self.require_confirm = require;
    }
}

impl Default for MidiLearn {
    fn default() -> Self {
        Self::new()
    }
}

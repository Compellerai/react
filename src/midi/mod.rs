//! MIDI controller support for REACT
//!
//! Provides real-time MIDI input/output with learn mode for mapping
//! controllers to UI parameters.

mod learn;
mod mapping;
mod preset_bindings;

pub use learn::{LearnState, MidiLearn};
pub use mapping::{ControlTarget, MidiKey, MidiMapping, MidiMsgType};
pub use preset_bindings::{
    MidiPresetBinding, MidiNoteKey, PresetBindingManager,
    SharedPresetBindingManager, create_shared_binding_manager
};

use std::sync::Arc;
use std::thread::{self, JoinHandle};
use crossbeam_channel::{bounded, Receiver, Sender};
use midir::{MidiInput, MidiInputConnection, MidiInputPort};
use parking_lot::Mutex;
use wmidi::MidiMessage;

/// MIDI message with device context
#[derive(Debug, Clone)]
pub struct MidiEvent {
    pub device_name: String,
    pub channel: u8,
    pub message: ParsedMidiMessage,
    pub timestamp: u64,
}

/// Parsed MIDI message types we care about
#[derive(Debug, Clone, PartialEq)]
pub enum ParsedMidiMessage {
    ControlChange { controller: u8, value: u8 },
    NoteOn { note: u8, velocity: u8 },
    NoteOff { note: u8 },
    PitchBend { value: u16 },
}

/// MIDI device info
#[derive(Debug, Clone)]
pub struct MidiDeviceInfo {
    pub name: String,
    pub port_index: usize,
    pub connected: bool,
}

/// MIDI controller manager
pub struct MidiController {
    /// Active input connections
    connections: Vec<MidiInputConnection<()>>,

    /// Channel for receiving MIDI events from input threads
    event_rx: Receiver<MidiEvent>,

    /// Sender cloned to each input connection
    event_tx: Sender<MidiEvent>,

    /// Available devices (refreshed on scan)
    available_devices: Vec<MidiDeviceInfo>,

    /// MIDI learn state machine
    learn: MidiLearn,

    /// Active mappings
    mappings: Vec<MidiMapping>,

    /// Pending control values from MIDI (target -> normalized value 0.0-1.0)
    pending_values: Arc<Mutex<Vec<(ControlTarget, f32)>>>,
}

impl MidiController {
    pub fn new() -> Self {
        let (event_tx, event_rx) = bounded(256);

        Self {
            connections: Vec::new(),
            event_rx,
            event_tx,
            available_devices: Vec::new(),
            learn: MidiLearn::new(),
            mappings: Vec::new(),
            pending_values: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Scan for available MIDI input devices
    pub fn scan_devices(&mut self) -> Vec<MidiDeviceInfo> {
        self.available_devices.clear();

        let midi_in = match MidiInput::new("REACT Scanner") {
            Ok(m) => m,
            Err(e) => {
                log::error!("[MIDI] Failed to create input: {}", e);
                return Vec::new();
            }
        };

        let ports = midi_in.ports();
        for (idx, port) in ports.iter().enumerate() {
            let name = midi_in.port_name(port).unwrap_or_else(|_| format!("Port {}", idx));
            let connected = self.connections.iter().any(|_| true); // TODO: track by port

            self.available_devices.push(MidiDeviceInfo {
                name,
                port_index: idx,
                connected,
            });
        }

        log::info!("[MIDI] Found {} devices", self.available_devices.len());
        self.available_devices.clone()
    }

    /// Connect to a MIDI input device by index
    pub fn connect(&mut self, port_index: usize) -> Result<(), String> {
        let midi_in = MidiInput::new("REACT Input")
            .map_err(|e| format!("Failed to create MIDI input: {}", e))?;

        let ports = midi_in.ports();
        let port = ports.get(port_index)
            .ok_or_else(|| format!("Invalid port index: {}", port_index))?;

        let port_name = midi_in.port_name(port)
            .unwrap_or_else(|_| format!("Port {}", port_index));

        let tx = self.event_tx.clone();
        let device_name = port_name.clone();

        let connection = midi_in.connect(
            port,
            "REACT MIDI In",
            move |timestamp, data, _| {
                if let Some(event) = parse_midi_message(&device_name, timestamp, data) {
                    let _ = tx.try_send(event);
                }
            },
            (),
        ).map_err(|e| format!("Failed to connect: {}", e))?;

        self.connections.push(connection);
        log::info!("[MIDI] Connected to: {}", port_name);

        Ok(())
    }

    /// Connect to all available MIDI devices
    pub fn connect_all(&mut self) {
        self.scan_devices();

        for idx in 0..self.available_devices.len() {
            if let Err(e) = self.connect(idx) {
                log::warn!("[MIDI] Failed to connect to device {}: {}", idx, e);
            }
        }
    }

    /// Disconnect all MIDI devices
    pub fn disconnect_all(&mut self) {
        self.connections.clear();
        log::info!("[MIDI] Disconnected all devices");
    }

    /// Process incoming MIDI events and return control values and any new mapping
    /// Returns (control_values, newly_learned_mapping)
    pub fn poll(&mut self) -> (Vec<(ControlTarget, f32)>, Option<MidiMapping>) {
        let mut results = Vec::new();
        let mut new_mapping = None;

        // Process all pending MIDI events (non-blocking)
        while let Ok(event) = self.event_rx.try_recv() {
            log::info!("[MIDI] Event: {:?}", event);  // Changed to info for visibility

            // If in learn mode, handle learning
            if self.learn.is_active() {
                if let Some(mapping) = self.learn.process_event(&event) {
                    // Remove any existing mapping with the same MIDI key (no duplicates)
                    let old_count = self.mappings.len();
                    self.mappings.retain(|m| m.key != mapping.key);
                    if self.mappings.len() < old_count {
                        log::info!("[MIDI] Removed existing mapping for {:?}", mapping.key);
                    }

                    log::info!("[MIDI] Learned mapping: {:?}", mapping);
                    new_mapping = Some(mapping.clone());
                    self.mappings.push(mapping);
                }
                continue;
            }

            // Otherwise, apply existing mappings
            let key = MidiKey::from_event(&event);
            for mapping in &self.mappings {
                if mapping.key == key {
                    let value = mapping.normalize_value(&event);
                    log::info!("[MIDI] Mapped {:?} -> {:?} = {:.3} (raw msg: {:?})",
                        key.msg_type, mapping.target, value, event.message);
                    results.push((mapping.target.clone(), value));
                }
            }
        }

        (results, new_mapping)
    }

    /// Get the current learn state
    pub fn learn_state(&self) -> &LearnState {
        self.learn.state()
    }

    /// Start MIDI learn mode
    pub fn start_learn(&mut self) {
        self.learn.arm();
        log::info!("[MIDI] Learn mode armed");
    }

    /// Set the target control for learning
    pub fn set_learn_target(&mut self, target: ControlTarget) {
        self.learn.set_target(target);
        log::info!("[MIDI] Learn target set: {:?}", self.learn.state());
    }

    /// Cancel learn mode
    pub fn cancel_learn(&mut self) {
        self.learn.cancel();
        log::info!("[MIDI] Learn mode cancelled");
    }

    /// Get all current mappings
    pub fn mappings(&self) -> &[MidiMapping] {
        &self.mappings
    }

    /// Load mappings from database
    pub fn load_mappings(&mut self, mappings: Vec<MidiMapping>) {
        self.mappings = mappings;
        log::info!("[MIDI] Loaded {} mappings", self.mappings.len());
    }

    /// Remove a mapping
    pub fn remove_mapping(&mut self, index: usize) {
        if index < self.mappings.len() {
            self.mappings.remove(index);
        }
    }

    /// Check if any devices are connected
    pub fn is_connected(&self) -> bool {
        !self.connections.is_empty()
    }

    /// Get connected device count
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }
}

/// Parse raw MIDI bytes into our event structure
fn parse_midi_message(device_name: &str, timestamp: u64, data: &[u8]) -> Option<MidiEvent> {
    if data.is_empty() {
        return None;
    }

    // Parse using wmidi
    let msg = MidiMessage::try_from(data).ok()?;

    let (channel, parsed) = match msg {
        MidiMessage::ControlChange(ch, controller, value) => {
            (ch.index(), ParsedMidiMessage::ControlChange {
                controller: controller.into(),
                value: value.into(),
            })
        }
        MidiMessage::NoteOn(ch, note, velocity) => {
            let vel: u8 = velocity.into();
            if vel == 0 {
                // Note On with velocity 0 is equivalent to Note Off
                (ch.index(), ParsedMidiMessage::NoteOff { note: note.into() })
            } else {
                (ch.index(), ParsedMidiMessage::NoteOn {
                    note: note.into(),
                    velocity: vel,
                })
            }
        }
        MidiMessage::NoteOff(ch, note, _) => {
            (ch.index(), ParsedMidiMessage::NoteOff { note: note.into() })
        }
        MidiMessage::PitchBendChange(ch, bend) => {
            (ch.index(), ParsedMidiMessage::PitchBend {
                value: bend.into(),
            })
        }
        // Ignore clock, active sense, sysex, etc.
        _ => return None,
    };

    Some(MidiEvent {
        device_name: device_name.to_string(),
        channel,
        message: parsed,
        timestamp,
    })
}

impl Default for MidiController {
    fn default() -> Self {
        Self::new()
    }
}

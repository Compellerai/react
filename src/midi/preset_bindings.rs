//! MIDI Preset Bindings - Map MIDI notes to specific presets
//!
//! Allows binding MIDI notes to trigger specific presets across all renderer types.
//! This works alongside the regular MIDI mapping system for control targets.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use parking_lot::RwLock;
use std::sync::Arc;

/// A binding from a MIDI note to a specific preset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiPresetBinding {
    /// Device name (port identifier) - empty string means "any device"
    pub device_name: String,
    /// MIDI channel (0-15) - 255 means "any channel"
    pub channel: u8,
    /// Note number (0-127)
    pub note: u8,
    /// Preset ID to load (from registry)
    pub preset_id: String,
    /// Optional: Custom name for this binding
    pub name: Option<String>,
}

impl MidiPresetBinding {
    /// Create a new binding
    pub fn new(device_name: impl Into<String>, channel: u8, note: u8, preset_id: impl Into<String>) -> Self {
        Self {
            device_name: device_name.into(),
            channel,
            note,
            preset_id: preset_id.into(),
            name: None,
        }
    }

    /// Create a binding that matches any device
    pub fn any_device(channel: u8, note: u8, preset_id: impl Into<String>) -> Self {
        Self {
            device_name: String::new(),
            channel,
            note,
            preset_id: preset_id.into(),
            name: None,
        }
    }

    /// Create a binding that matches any device and any channel
    pub fn any_device_any_channel(note: u8, preset_id: impl Into<String>) -> Self {
        Self {
            device_name: String::new(),
            channel: 255,
            note,
            preset_id: preset_id.into(),
            name: None,
        }
    }

    /// With a custom name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Check if this binding matches a MIDI note event
    pub fn matches(&self, device_name: &str, channel: u8, note: u8) -> bool {
        // Check note
        if self.note != note {
            return false;
        }

        // Check channel (255 = any)
        if self.channel != 255 && self.channel != channel {
            return false;
        }

        // Check device (empty = any)
        if !self.device_name.is_empty() && self.device_name != device_name {
            return false;
        }

        true
    }

    /// Get unique key for this binding
    pub fn key(&self) -> MidiNoteKey {
        MidiNoteKey {
            device_name: self.device_name.clone(),
            channel: self.channel,
            note: self.note,
        }
    }

    /// Human-readable description
    pub fn description(&self) -> String {
        let device = if self.device_name.is_empty() {
            "Any".to_string()
        } else {
            self.device_name.clone()
        };

        let channel = if self.channel == 255 {
            "Any".to_string()
        } else {
            format!("{}", self.channel + 1)
        };

        format!("{} Ch{} Note {}", device, channel, self.note)
    }
}

/// Key for looking up bindings
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct MidiNoteKey {
    pub device_name: String,
    pub channel: u8,
    pub note: u8,
}

impl MidiNoteKey {
    pub fn new(device_name: impl Into<String>, channel: u8, note: u8) -> Self {
        Self {
            device_name: device_name.into(),
            channel,
            note,
        }
    }
}

/// Manager for MIDI preset bindings
#[derive(Debug, Default)]
pub struct PresetBindingManager {
    /// All bindings indexed by key
    bindings: HashMap<MidiNoteKey, MidiPresetBinding>,

    /// List order for UI display
    binding_order: Vec<MidiNoteKey>,

    /// Whether bindings have changed since last save
    dirty: bool,
}

impl PresetBindingManager {
    /// Create a new empty manager
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
            binding_order: Vec::new(),
            dirty: false,
        }
    }

    /// Add a binding
    pub fn add(&mut self, binding: MidiPresetBinding) {
        let key = binding.key();
        if !self.bindings.contains_key(&key) {
            self.binding_order.push(key.clone());
        }
        self.bindings.insert(key, binding);
        self.dirty = true;
    }

    /// Remove a binding
    pub fn remove(&mut self, key: &MidiNoteKey) -> Option<MidiPresetBinding> {
        self.binding_order.retain(|k| k != key);
        let removed = self.bindings.remove(key);
        if removed.is_some() {
            self.dirty = true;
        }
        removed
    }

    /// Get a binding by key
    pub fn get(&self, key: &MidiNoteKey) -> Option<&MidiPresetBinding> {
        self.bindings.get(key)
    }

    /// Find binding for a note event (checks wildcards)
    pub fn find_for_note(&self, device_name: &str, channel: u8, note: u8) -> Option<&MidiPresetBinding> {
        // First try exact match
        let exact_key = MidiNoteKey::new(device_name, channel, note);
        if let Some(binding) = self.bindings.get(&exact_key) {
            return Some(binding);
        }

        // Try any channel match
        let any_channel_key = MidiNoteKey::new(device_name, 255, note);
        if let Some(binding) = self.bindings.get(&any_channel_key) {
            return Some(binding);
        }

        // Try any device match
        let any_device_key = MidiNoteKey::new("", channel, note);
        if let Some(binding) = self.bindings.get(&any_device_key) {
            return Some(binding);
        }

        // Try any device any channel match
        let any_key = MidiNoteKey::new("", 255, note);
        if let Some(binding) = self.bindings.get(&any_key) {
            return Some(binding);
        }

        None
    }

    /// Get all bindings in order
    pub fn all(&self) -> Vec<&MidiPresetBinding> {
        self.binding_order
            .iter()
            .filter_map(|k| self.bindings.get(k))
            .collect()
    }

    /// Get all bindings for a specific preset
    pub fn bindings_for_preset(&self, preset_id: &str) -> Vec<&MidiPresetBinding> {
        self.bindings
            .values()
            .filter(|b| b.preset_id == preset_id)
            .collect()
    }

    /// Get number of bindings
    pub fn count(&self) -> usize {
        self.bindings.len()
    }

    /// Check if any binding exists for a note
    pub fn has_binding_for_note(&self, note: u8) -> bool {
        self.bindings.values().any(|b| b.note == note)
    }

    /// Clear all bindings
    pub fn clear(&mut self) {
        self.bindings.clear();
        self.binding_order.clear();
        self.dirty = true;
    }

    /// Check if dirty (needs saving)
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark as clean (after saving)
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// Load from a list of bindings (e.g., from database)
    pub fn load(&mut self, bindings: Vec<MidiPresetBinding>) {
        self.bindings.clear();
        self.binding_order.clear();

        for binding in bindings {
            let key = binding.key();
            self.binding_order.push(key.clone());
            self.bindings.insert(key, binding);
        }

        self.dirty = false;
        log::info!("[MidiPresetBindings] Loaded {} bindings", self.bindings.len());
    }

    /// Export all bindings for saving
    pub fn export(&self) -> Vec<MidiPresetBinding> {
        self.binding_order
            .iter()
            .filter_map(|k| self.bindings.get(k).cloned())
            .collect()
    }
}

/// Thread-safe wrapper for PresetBindingManager
pub type SharedPresetBindingManager = Arc<RwLock<PresetBindingManager>>;

/// Create a new shared binding manager
pub fn create_shared_binding_manager() -> SharedPresetBindingManager {
    Arc::new(RwLock::new(PresetBindingManager::new()))
}

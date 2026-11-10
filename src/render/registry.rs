//! Global Preset Registry - Unified preset management across all renderers
//!
//! Provides a central registry for all theme presets from all renderer types
//! (ProjectM, LED Wall, future Sonic Geometry, etc.)
//!
//! Features:
//! - Unified view of all presets across renderer types
//! - Favorites tracking (persisted to database)
//! - Pinned presets for quick access
//! - Search and filtering
//! - Preset metadata (custom names, ratings, play counts)

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use parking_lot::RwLock;

use super::theme::{ThemePreset, RendererType};

/// A preset entry in the registry with additional metadata
#[derive(Clone, Debug)]
pub struct RegistryPreset {
    /// Core preset info
    pub preset: ThemePreset,
    /// Is this preset a favorite?
    pub is_favorite: bool,
    /// Is this preset pinned for quick access?
    pub is_pinned: bool,
    /// Custom display name (overrides preset.name)
    pub custom_name: Option<String>,
    /// User rating (1-5, 0 = unrated)
    pub rating: u8,
    /// Number of times this preset has been played
    pub play_count: u32,
    /// Is this preset hidden from lists?
    pub hidden: bool,
}

impl RegistryPreset {
    /// Create from a ThemePreset
    pub fn from_preset(preset: ThemePreset) -> Self {
        Self {
            preset,
            is_favorite: false,
            is_pinned: false,
            custom_name: None,
            rating: 0,
            play_count: 0,
            hidden: false,
        }
    }

    /// Get display name (custom name or preset name)
    pub fn display_name(&self) -> &str {
        self.custom_name.as_deref().unwrap_or(&self.preset.name)
    }

    /// Get unique key for this preset
    pub fn key(&self) -> &str {
        &self.preset.id
    }
}

/// Global preset registry
pub struct PresetRegistry {
    /// All presets indexed by ID
    presets: HashMap<String, RegistryPreset>,

    /// Presets grouped by category
    by_category: HashMap<String, Vec<String>>,

    /// Presets grouped by renderer type
    by_renderer: HashMap<RendererType, Vec<String>>,

    /// Favorite preset IDs (for quick lookup)
    favorites: HashSet<String>,

    /// Pinned preset IDs (for quick lookup)
    pinned: HashSet<String>,

    /// Categories in display order
    categories: Vec<String>,
}

impl PresetRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            presets: HashMap::new(),
            by_category: HashMap::new(),
            by_renderer: HashMap::new(),
            favorites: HashSet::new(),
            pinned: HashSet::new(),
            categories: Vec::new(),
        }
    }

    /// Clear all presets (but keep favorites/pinned state)
    pub fn clear(&mut self) {
        self.presets.clear();
        self.by_category.clear();
        self.by_renderer.clear();
        self.categories.clear();
    }

    /// Register a preset
    pub fn register(&mut self, preset: ThemePreset) {
        let id = preset.id.clone();
        let category = preset.category.clone();
        let renderer_type = preset.renderer_type;

        // Check if already favorite/pinned
        let is_favorite = self.favorites.contains(&id);
        let is_pinned = self.pinned.contains(&id);

        let mut entry = RegistryPreset::from_preset(preset);
        entry.is_favorite = is_favorite;
        entry.is_pinned = is_pinned;

        // Add to main index
        self.presets.insert(id.clone(), entry);

        // Add to category index
        self.by_category
            .entry(category.clone())
            .or_default()
            .push(id.clone());

        // Add to renderer index
        if let Some(rt) = renderer_type {
            self.by_renderer
                .entry(rt)
                .or_default()
                .push(id);
        }

        // Track category
        if !self.categories.contains(&category) {
            self.categories.push(category);
        }
    }

    /// Register multiple presets
    pub fn register_all(&mut self, presets: Vec<ThemePreset>) {
        for preset in presets {
            self.register(preset);
        }
    }

    /// Get a preset by ID
    pub fn get(&self, id: &str) -> Option<&RegistryPreset> {
        self.presets.get(id)
    }

    /// Get a mutable preset by ID
    pub fn get_mut(&mut self, id: &str) -> Option<&mut RegistryPreset> {
        self.presets.get_mut(id)
    }

    /// Get all presets
    pub fn all(&self) -> Vec<&RegistryPreset> {
        self.presets.values().filter(|p| !p.hidden).collect()
    }

    /// Get all preset IDs
    pub fn all_ids(&self) -> Vec<&str> {
        self.presets.keys().map(|s| s.as_str()).collect()
    }

    /// Get presets in a category
    pub fn in_category(&self, category: &str) -> Vec<&RegistryPreset> {
        self.by_category
            .get(category)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.presets.get(id))
                    .filter(|p| !p.hidden)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get presets for a renderer type
    pub fn for_renderer(&self, renderer_type: RendererType) -> Vec<&RegistryPreset> {
        self.by_renderer
            .get(&renderer_type)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.presets.get(id))
                    .filter(|p| !p.hidden)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all categories
    pub fn categories(&self) -> &[String] {
        &self.categories
    }

    /// Get categories for a specific renderer
    pub fn categories_for_renderer(&self, renderer_type: RendererType) -> Vec<String> {
        let mut cats: Vec<String> = self.by_renderer
            .get(&renderer_type)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.presets.get(id))
                    .map(|p| p.preset.category.clone())
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default();
        cats.sort();
        cats
    }

    // === Favorites ===

    /// Add a preset to favorites
    pub fn add_favorite(&mut self, id: &str) {
        self.favorites.insert(id.to_string());
        if let Some(preset) = self.presets.get_mut(id) {
            preset.is_favorite = true;
        }
    }

    /// Remove a preset from favorites
    pub fn remove_favorite(&mut self, id: &str) {
        self.favorites.remove(id);
        if let Some(preset) = self.presets.get_mut(id) {
            preset.is_favorite = false;
        }
    }

    /// Toggle favorite status
    pub fn toggle_favorite(&mut self, id: &str) -> bool {
        if self.favorites.contains(id) {
            self.remove_favorite(id);
            false
        } else {
            self.add_favorite(id);
            true
        }
    }

    /// Check if a preset is a favorite
    pub fn is_favorite(&self, id: &str) -> bool {
        self.favorites.contains(id)
    }

    /// Get all favorite presets
    pub fn favorites(&self) -> Vec<&RegistryPreset> {
        self.favorites
            .iter()
            .filter_map(|id| self.presets.get(id))
            .filter(|p| !p.hidden)
            .collect()
    }

    /// Get favorite IDs
    pub fn favorite_ids(&self) -> Vec<&str> {
        self.favorites.iter().map(|s| s.as_str()).collect()
    }

    /// Set favorites from a list (replaces existing)
    pub fn set_favorites(&mut self, ids: Vec<String>) {
        // Clear old favorites
        for id in &self.favorites {
            if let Some(preset) = self.presets.get_mut(id) {
                preset.is_favorite = false;
            }
        }
        self.favorites.clear();

        // Set new favorites
        for id in ids {
            self.add_favorite(&id);
        }
    }

    // === Pinned ===

    /// Add a preset to pinned
    pub fn add_pinned(&mut self, id: &str) {
        self.pinned.insert(id.to_string());
        if let Some(preset) = self.presets.get_mut(id) {
            preset.is_pinned = true;
        }
    }

    /// Remove a preset from pinned
    pub fn remove_pinned(&mut self, id: &str) {
        self.pinned.remove(id);
        if let Some(preset) = self.presets.get_mut(id) {
            preset.is_pinned = false;
        }
    }

    /// Toggle pinned status
    pub fn toggle_pinned(&mut self, id: &str) -> bool {
        if self.pinned.contains(id) {
            self.remove_pinned(id);
            false
        } else {
            self.add_pinned(id);
            true
        }
    }

    /// Check if a preset is pinned
    pub fn is_pinned(&self, id: &str) -> bool {
        self.pinned.contains(id)
    }

    /// Get all pinned presets
    pub fn pinned(&self) -> Vec<&RegistryPreset> {
        self.pinned
            .iter()
            .filter_map(|id| self.presets.get(id))
            .filter(|p| !p.hidden)
            .collect()
    }

    /// Get pinned IDs
    pub fn pinned_ids(&self) -> Vec<&str> {
        self.pinned.iter().map(|s| s.as_str()).collect()
    }

    /// Set pinned from a list (replaces existing)
    pub fn set_pinned(&mut self, ids: Vec<String>) {
        // Clear old pinned
        for id in &self.pinned {
            if let Some(preset) = self.presets.get_mut(id) {
                preset.is_pinned = false;
            }
        }
        self.pinned.clear();

        // Set new pinned
        for id in ids {
            self.add_pinned(&id);
        }
    }

    // === Metadata ===

    /// Set custom name for a preset
    pub fn set_custom_name(&mut self, id: &str, name: Option<String>) {
        if let Some(preset) = self.presets.get_mut(id) {
            preset.custom_name = name;
        }
    }

    /// Set rating for a preset
    pub fn set_rating(&mut self, id: &str, rating: u8) {
        if let Some(preset) = self.presets.get_mut(id) {
            preset.rating = rating.min(5);
        }
    }

    /// Increment play count
    pub fn increment_play_count(&mut self, id: &str) {
        if let Some(preset) = self.presets.get_mut(id) {
            preset.play_count = preset.play_count.saturating_add(1);
        }
    }

    /// Set hidden status
    pub fn set_hidden(&mut self, id: &str, hidden: bool) {
        if let Some(preset) = self.presets.get_mut(id) {
            preset.hidden = hidden;
        }
    }

    // === Search ===

    /// Search presets by name
    pub fn search(&self, query: &str) -> Vec<&RegistryPreset> {
        let query_lower = query.to_lowercase();
        self.presets
            .values()
            .filter(|p| !p.hidden)
            .filter(|p| {
                p.display_name().to_lowercase().contains(&query_lower)
                    || p.preset.category.to_lowercase().contains(&query_lower)
            })
            .collect()
    }

    /// Get presets sorted by play count (most played first)
    pub fn most_played(&self, limit: usize) -> Vec<&RegistryPreset> {
        let mut presets: Vec<_> = self.presets.values()
            .filter(|p| !p.hidden && p.play_count > 0)
            .collect();
        presets.sort_by(|a, b| b.play_count.cmp(&a.play_count));
        presets.truncate(limit);
        presets
    }

    /// Get presets sorted by rating (highest first)
    pub fn top_rated(&self, limit: usize) -> Vec<&RegistryPreset> {
        let mut presets: Vec<_> = self.presets.values()
            .filter(|p| !p.hidden && p.rating > 0)
            .collect();
        presets.sort_by(|a, b| b.rating.cmp(&a.rating));
        presets.truncate(limit);
        presets
    }

    // === Stats ===

    /// Get total number of presets
    pub fn count(&self) -> usize {
        self.presets.len()
    }

    /// Get number of visible presets
    pub fn visible_count(&self) -> usize {
        self.presets.values().filter(|p| !p.hidden).count()
    }

    /// Get number of favorites
    pub fn favorites_count(&self) -> usize {
        self.favorites.len()
    }

    /// Get number of pinned
    pub fn pinned_count(&self) -> usize {
        self.pinned.len()
    }
}

impl Default for PresetRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe wrapper for PresetRegistry
pub type SharedPresetRegistry = Arc<RwLock<PresetRegistry>>;

/// Create a new shared registry
pub fn create_shared_registry() -> SharedPresetRegistry {
    Arc::new(RwLock::new(PresetRegistry::new()))
}

// === Database Integration Helpers ===

impl PresetRegistry {
    /// Load favorites from database (category, name pairs)
    /// Converts to preset IDs and marks them as favorites
    pub fn load_favorites_from_db(&mut self, favorites: &[(String, String)]) {
        let mut favorite_ids = Vec::new();

        for (category, name) in favorites {
            // Find matching preset by category and name
            for (id, preset) in &self.presets {
                if &preset.preset.category == category && &preset.preset.name == name {
                    favorite_ids.push(id.clone());
                    break;
                }
            }
        }

        log::info!("[Registry] Loaded {} favorites from database", favorite_ids.len());
        self.set_favorites(favorite_ids);
    }

    /// Get favorites as (category, name) pairs for database storage
    pub fn favorites_for_db(&self) -> Vec<(String, String)> {
        self.favorites
            .iter()
            .filter_map(|id| self.presets.get(id))
            .map(|p| (p.preset.category.clone(), p.preset.name.clone()))
            .collect()
    }

    /// Load preset metadata from database
    pub fn load_metadata_from_db(&mut self, metadata: &[crate::db::PresetMetadata]) {
        for meta in metadata {
            // Find preset by file path
            if let Some(preset) = self.presets.get_mut(&meta.file_path) {
                preset.custom_name = meta.custom_name.clone();
                preset.rating = meta.rating as u8;
                preset.play_count = meta.play_count as u32;
                preset.hidden = meta.hidden;
            }
        }
        log::info!("[Registry] Loaded metadata for {} presets", metadata.len());
    }

    /// Load pinned presets from "Pinned" category
    /// In the current system, "Pinned" is a physical folder category
    pub fn load_pinned_from_category(&mut self) {
        let pinned_ids: Vec<String> = self.presets
            .iter()
            .filter(|(_, p)| p.preset.category == "Pinned")
            .map(|(id, _)| id.clone())
            .collect();

        log::info!("[Registry] Found {} pinned presets", pinned_ids.len());
        self.set_pinned(pinned_ids);
    }

    /// Get presets for MIDI browse (pinned + favorites)
    pub fn midi_browse_list(&self) -> Vec<&RegistryPreset> {
        let mut list: Vec<&RegistryPreset> = Vec::new();

        // Add pinned first
        for preset in self.pinned() {
            list.push(preset);
        }

        // Add favorites (excluding already-pinned)
        for preset in self.favorites() {
            if !preset.is_pinned {
                list.push(preset);
            }
        }

        list
    }
}

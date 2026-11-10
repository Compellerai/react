//! Theme Renderer Trait - Unified interface for visualization renderers
//!
//! Provides a common interface for different visualization backends:
//! - ProjectM (Milkdrop-style presets)
//! - LED Wall (spectrum analyzer patterns)
//! - Future: Sonic Geometry, etc.

use anyhow::Result;
use eframe::glow;
use std::sync::Arc;

/// Types of theme renderers available
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RendererType {
    /// Milkdrop-style projectM visualizations
    ProjectM,
    /// LED wall spectrum analyzer
    LedWall,
    /// REG Sir Robert Edward Grant - mathematical visualization
    HarmonicGeometry,
    /// Future: 3D sonic geometry
    SonicGeometry,
    /// Mythos: continuous field with Collatz-sequence modulation
    Mythos,
}

impl RendererType {
    /// Human-readable name for UI display
    pub fn display_name(&self) -> &'static str {
        match self {
            RendererType::ProjectM => "projectM",
            RendererType::LedWall => "LED Wall",
            RendererType::HarmonicGeometry => "Geometric Field",
            RendererType::SonicGeometry => "Sonic Geometry",
            RendererType::Mythos => "Mythos Field",
        }
    }

    /// Short identifier for storage/serialization
    pub fn id(&self) -> &'static str {
        match self {
            RendererType::ProjectM => "projectm",
            RendererType::LedWall => "led_wall",
            RendererType::HarmonicGeometry => "harmonic_geometry",
            RendererType::SonicGeometry => "sonic_geometry",
            RendererType::Mythos => "mythos",
        }
    }

    /// Parse from string identifier
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "projectm" => Some(RendererType::ProjectM),
            "led_wall" => Some(RendererType::LedWall),
            "harmonic_geometry" => Some(RendererType::HarmonicGeometry),
            "sonic_geometry" => Some(RendererType::SonicGeometry),
            "mythos" => Some(RendererType::Mythos),
            _ => None,
        }
    }
}

/// Preset/pattern information shared across all renderers
#[derive(Clone, Debug, Default)]
pub struct ThemePreset {
    /// Unique identifier (usually file path or pattern ID)
    pub id: String,
    /// Display name
    pub name: String,
    /// Category (e.g., "Geometric", "Abstract", "LED Patterns")
    pub category: String,
    /// Optional subcategory path
    pub subcategory: Option<String>,
    /// Which renderer this preset belongs to
    pub renderer_type: Option<RendererType>,
}

impl ThemePreset {
    /// Create a new preset
    pub fn new(id: impl Into<String>, name: impl Into<String>, category: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            category: category.into(),
            subcategory: None,
            renderer_type: None,
        }
    }

    /// With subcategory
    pub fn with_subcategory(mut self, subcategory: impl Into<String>) -> Self {
        self.subcategory = Some(subcategory.into());
        self
    }

    /// With renderer type
    pub fn with_renderer(mut self, renderer_type: RendererType) -> Self {
        self.renderer_type = Some(renderer_type);
        self
    }
}

/// Audio analysis data for renderers
/// Contains both raw samples and analyzed frequency bands
#[derive(Clone, Debug)]
pub struct AudioAnalysis {
    /// Raw PCM samples (mono, normalized -1 to 1)
    pub samples: Vec<f32>,
    /// Bass energy (0-1, ~60-250Hz)
    pub bass: f32,
    /// Mid energy (0-1, ~250-4000Hz)
    pub mid: f32,
    /// High energy (0-1, ~4000-16000Hz)
    pub high: f32,
    /// Beat detected this frame
    pub kick: bool,
    /// Raw energy level (for gates/silence detection)
    pub energy: f32,
    /// Perceptual energy (for UI/shaders, remapped to visible 0-1 range)
    pub energy_vis: f32,
    /// 12-bin chroma (pitch class energy): C, C#, D, D#, E, F, F#, G, G#, A, A#, B
    /// SLOW smoothing for stable dominant detection
    pub chroma: [f32; 12],
    /// 12-bin chroma with FAST smoothing for responsive UI display
    /// Use this for shader/HUD rendering
    pub display_chroma: [f32; 12],
    /// Whether chroma passes tonal validity gates (energy + peak separation)
    pub is_tonal: bool,
    /// Dominant pitch class index (0-11) - only valid when is_tonal=true
    pub dominant_pc: usize,
    /// Peak separation ratio (0-1) - how much the dominant stands out
    pub tonal_separation: f32,
    /// Total tonal energy (sum of chroma bins)
    pub tonal_energy: f32,
    /// Temporal stability (0-1) - how stable chroma is frame-to-frame
    pub stability: f32,
    /// Live frequency centroids for 6 spectral bands (Hz)
    /// [Sub, Kick, Low-mid, Mid, High-mid, Air]
    pub band_centroids: [f32; 6],
    /// Raw (pre-normalization) bass energy for silence detection
    pub raw_bass: f32,
    /// Raw (pre-normalization) mid energy for silence detection
    pub raw_mid: f32,
    /// Raw (pre-normalization) high energy for silence detection
    pub raw_high: f32,
    /// Continuous low-band onset strength (analyzer kick-envelope flux) — the
    /// onset-detection function the tempo autocorrelator consumes.
    pub onset_strength: f32,
}

impl Default for AudioAnalysis {
    fn default() -> Self {
        Self {
            samples: Vec::new(),
            bass: 0.0,
            mid: 0.0,
            high: 0.0,
            kick: false,
            energy: 0.0,
            energy_vis: 0.0,
            chroma: [0.0; 12],
            display_chroma: [0.0; 12],
            is_tonal: false,
            dominant_pc: 0,
            tonal_separation: 0.0,
            tonal_energy: 0.0,
            stability: 1.0,
            band_centroids: [35.0, 112.0, 355.0, 1122.0, 3550.0, 11225.0],
            raw_bass: 0.0,
            raw_mid: 0.0,
            raw_high: 0.0,
            onset_strength: 0.0,
        }
    }
}

impl AudioAnalysis {
    /// Create from raw samples with band analysis
    pub fn new(samples: Vec<f32>, bass: f32, mid: f32, high: f32, kick: bool) -> Self {
        let energy = bass * 0.5 + mid * 0.3 + high * 0.2;
        Self {
            samples,
            bass,
            mid,
            high,
            kick,
            energy,
            energy_vis: energy, // Fallback: use raw energy
            chroma: [0.0; 12],
            display_chroma: [0.0; 12],
            is_tonal: false,
            dominant_pc: 0,
            tonal_separation: 0.0,
            tonal_energy: 0.0,
            stability: 1.0,
            band_centroids: [35.0, 112.0, 355.0, 1122.0, 3550.0, 11225.0],
            raw_bass: 0.0,
            raw_mid: 0.0,
            raw_high: 0.0,
            onset_strength: 0.0,
        }
    }

    /// Create with chroma data and tonal analysis from FFT
    pub fn with_chroma(
        samples: Vec<f32>,
        bass: f32,
        mid: f32,
        high: f32,
        kick: bool,
        energy: f32,
        energy_vis: f32,
        chroma: [f32; 12],
        display_chroma: [f32; 12],
        is_tonal: bool,
        dominant_pc: usize,
        tonal_separation: f32,
        tonal_energy: f32,
        stability: f32,
        band_centroids: [f32; 6],
        raw_bass: f32,
        raw_mid: f32,
        raw_high: f32,
        onset_strength: f32,
    ) -> Self {
        Self {
            samples,
            bass,
            mid,
            high,
            kick,
            energy,
            energy_vis,
            chroma,
            display_chroma,
            is_tonal,
            dominant_pc,
            tonal_separation,
            tonal_energy,
            stability,
            band_centroids,
            raw_bass,
            raw_mid,
            raw_high,
            onset_strength,
        }
    }

    /// Create silent analysis
    pub fn silent() -> Self {
        Self::default()
    }
}

/// Unified interface for theme renderers
///
/// All visualization backends implement this trait to provide
/// a consistent interface for the render pipeline.
pub trait ThemeRenderer: Send {
    /// Get the renderer type
    fn renderer_type(&self) -> RendererType;

    /// Initialize or reinitialize the renderer
    /// Called when OpenGL context is available
    fn init(&mut self, gl: Arc<glow::Context>) -> Result<()>;

    /// Check if initialized
    fn is_initialized(&self) -> bool;

    /// Update renderer state with new audio data
    /// dt = frame delta time in seconds
    fn update(&mut self, audio: &AudioAnalysis, dt: f32);

    /// Render a frame to the currently bound FBO
    fn render(&mut self, width: u32, height: u32);

    /// Set the render size
    fn set_size(&mut self, width: u32, height: u32);

    /// Clean up resources
    fn cleanup(&mut self);

    // === Preset management (optional - not all renderers have presets) ===

    /// Scan for available presets/patterns
    /// Returns empty vec if renderer doesn't support presets
    fn scan_presets(&mut self) -> Vec<ThemePreset> {
        Vec::new()
    }

    /// Get available categories
    fn get_categories(&self) -> Vec<String> {
        Vec::new()
    }

    /// Get presets in a category
    fn get_presets_in_category(&self, _category: &str) -> Vec<ThemePreset> {
        Vec::new()
    }

    /// Load a specific preset by ID
    fn load_preset(&mut self, _preset_id: &str) -> Result<()> {
        Ok(())
    }

    /// Load preset by category and name
    fn load_preset_by_name(&mut self, _category: &str, _name: &str) -> Result<()> {
        anyhow::bail!("Presets not supported by this renderer")
    }

    /// Get currently loaded preset
    fn current_preset(&self) -> Option<ThemePreset> {
        None
    }

    /// Set a random preset
    /// Returns the loaded preset info if successful
    fn set_random_preset(&mut self) -> Option<ThemePreset> {
        None
    }

    /// Check if this renderer supports presets
    fn has_presets(&self) -> bool {
        false
    }

    // === Renderer-specific features (optional) ===

    /// Enable/disable transitions (for renderers that support them)
    fn set_transitions_enabled(&mut self, _enabled: bool) {}

    /// Check if transitions are available
    fn has_transitions(&self) -> bool {
        false
    }
}

/// Box type for dynamic dispatch of theme renderers
pub type BoxedThemeRenderer = Box<dyn ThemeRenderer>;

//! Hero's Journey Skin System
//!
//! The Hero system controls ALL compel/video visibility through the 4-act structure:
//! - Birth: Latent (flicker gate)
//! - Lock: Bound to 12-fold harmonic seams
//! - Dive: Full interior revelation
//! - Afterglow: Emissive persistence
//!
//! There is ONE skin configuration optimized for the Hero system.
//! Act transitions drive visibility - not manual preset selection.

// ============================================================================
// SKIN MODE - How the skin is applied (internal)
// ============================================================================

/// How the skin texture/effects are applied to the harmonic field
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SkinMode {
    /// No skin applied - pure harmonic field with default lighting
    #[default]
    None,

    /// Surface skin - Compel video as albedo texture on the field
    Surface,

    /// Emissive skin - Compel as glowing patterns
    Emissive,

    /// Both surface and emissive combined (Hero default)
    SurfaceEmissive,
}

impl SkinMode {
    pub fn as_i32(&self) -> i32 {
        match self {
            SkinMode::None => 0,
            SkinMode::Surface => 1,
            SkinMode::Emissive => 2,
            SkinMode::SurfaceEmissive => 3,
        }
    }

    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => SkinMode::Surface,
            2 => SkinMode::Emissive,
            3 => SkinMode::SurfaceEmissive,
            _ => SkinMode::None,
        }
    }
}

// ============================================================================
// FIELD SKIN - Hero configuration
// ============================================================================

/// Skin configuration for the Hero's Journey system
#[derive(Clone, Debug)]
pub struct FieldSkin {
    /// Compel ID to use as skin texture (None = no texture)
    pub skin_id: Option<String>,

    /// How the skin is applied
    pub mode: SkinMode,

    /// Overall skin strength (0-1)
    pub strength: f32,

    /// Rotate UV so dominant pitch is "up"
    pub lock_to_dominant: bool,

    /// Subtle hue offset by pitch class
    pub hue_follow_pitch: bool,

    /// Surface texture contrast (1.0 = neutral)
    pub surf_contrast: f32,

    /// Surface texture saturation (1.0 = neutral)
    pub surf_saturation: f32,

    /// Emissive intensity multiplier (0-1+)
    pub emissive_strength: f32,

    /// Rim/fresnel flare strength (0-1)
    pub rim_strength: f32,

    /// Bloom amount (0-1)
    pub bloom_amount: f32,

    /// Enable starburst/lens streak effect
    pub starburst_enabled: bool,

    /// Edge outline strength (0-1)
    pub edge_outline: f32,

    /// Specular sparkle intensity (0-1)
    pub specular_sparkle: f32,

    /// Video -> albedo mix strength (0-1)
    pub surface_mix: f32,

    /// Shadow lift - prevents AO from crushing video
    pub shadow_lift: f32,

    /// AO clamp - minimum ambient occlusion value
    pub ao_clamp: f32,

    /// UV mapping mode: 0 = cylindrical, 1 = triplanar
    pub uv_mode: i32,
}

impl Default for FieldSkin {
    fn default() -> Self {
        // Default is now Hero-optimized when video is present
        Self::hero()
    }
}

impl FieldSkin {
    /// Hero's Journey configuration - optimized for act-driven compel visibility
    pub fn hero() -> Self {
        Self {
            skin_id: None,
            mode: SkinMode::SurfaceEmissive,
            strength: 0.9,
            lock_to_dominant: false,  // Let video flow naturally
            hue_follow_pitch: false,  // Don't tint video
            surf_contrast: 1.35,      // Punch up the video
            surf_saturation: 1.25,    // Make colors pop
            emissive_strength: 0.5,   // Glow controlled by act
            rim_strength: 0.25,       // Subtle rim
            bloom_amount: 0.3,        // Moderate bloom
            starburst_enabled: false,
            edge_outline: 0.0,
            specular_sparkle: 0.2,
            surface_mix: 0.9,         // Video is 90% of albedo
            shadow_lift: 0.08,        // Lift shadows
            ao_clamp: 0.35,           // Don't crush darks
            uv_mode: 1,               // Triplanar
        }
    }

    /// No skin - pure harmonic field
    pub fn none() -> Self {
        Self {
            skin_id: None,
            mode: SkinMode::None,
            strength: 0.0,
            lock_to_dominant: false,
            hue_follow_pitch: false,
            surf_contrast: 1.0,
            surf_saturation: 1.0,
            emissive_strength: 0.0,
            rim_strength: 0.15,
            bloom_amount: 0.1,
            starburst_enabled: false,
            edge_outline: 0.0,
            specular_sparkle: 0.0,
            surface_mix: 0.0,
            shadow_lift: 0.0,
            ao_clamp: 0.4,
            uv_mode: 0,
        }
    }

    /// Check if any skin effects are active
    pub fn is_active(&self) -> bool {
        self.mode != SkinMode::None
    }

    /// Check if texture is required
    pub fn needs_texture(&self) -> bool {
        self.skin_id.is_some()
            && (self.mode == SkinMode::Surface || self.mode == SkinMode::SurfaceEmissive)
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skin_mode_roundtrip() {
        for i in 0..4 {
            let mode = SkinMode::from_i32(i);
            assert_eq!(mode.as_i32(), i);
        }
    }

    #[test]
    fn test_hero_skin_active() {
        let skin = FieldSkin::hero();
        assert_eq!(skin.mode, SkinMode::SurfaceEmissive);
        assert!(skin.is_active());
    }

    #[test]
    fn test_none_skin_inactive() {
        let skin = FieldSkin::none();
        assert_eq!(skin.mode, SkinMode::None);
        assert!(!skin.is_active());
    }
}

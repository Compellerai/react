//! Hero Configuration
//!
//! Single baseline configuration for the Hero's Journey visual instrument.
//! There are no presets to choose - the 4-act phase system IS the experience.
//!
//! The journey:
//! - Birth: Field emerges from harmonic interference
//! - Lock: Standing wave stabilizes, geometry clarifies
//! - Dive: Camera enters, interior reveals chopped compels
//! - Afterglow: Resolution, pull back, breathe

use super::harmonic_field::{RenderMode, SymmetryMode};

/// Visual configuration (internal, not user-facing)
#[derive(Clone, Debug)]
pub struct VisualConfig {
    /// Glow intensity (0 = none, 1 = normal, 2 = strong)
    pub glow: f32,

    /// Fog/atmosphere density (0 = none)
    pub fog_density: f32,

    /// Exposure (1.0 = normal)
    pub exposure: f32,

    /// Camera distance from center (baseline, phase system modulates)
    pub camera_distance: f32,

    /// Camera height above center
    pub camera_height: f32,

    /// Camera angle (radians, initial)
    pub camera_angle: f32,
}

impl Default for VisualConfig {
    fn default() -> Self {
        Self {
            glow: 0.5,
            fog_density: 0.0,
            exposure: 1.0,
            camera_distance: 5.0,
            camera_height: 1.5,
            camera_angle: 0.0,
        }
    }
}

/// Hero baseline configuration (internal, not user-facing)
///
/// The phase system (Birth→Lock→Dive→Afterglow) modulates these
/// parameters in real-time based on harmonic state.
#[derive(Clone, Debug)]
pub struct HeroConfig {
    /// 3D realization mode
    pub render_mode: RenderMode,

    /// Symmetry order (12 or 24)
    pub symmetry_mode: SymmetryMode,

    /// Base radius (neutral circle)
    pub base_radius: f32,

    /// Chroma → radius gain
    pub gain: f32,

    /// Extrusion thickness multiplier
    pub thickness_multiplier: f32,

    /// Slow base rotation speed (rad/sec)
    pub rotation_speed: f32,

    /// Chroma smoothing time constant (seconds)
    pub smoothing_tau: f32,

    /// Visual settings
    pub visual: VisualConfig,
}

impl Default for HeroConfig {
    fn default() -> Self {
        Self::hero()
    }
}

impl HeroConfig {
    /// The canonical Hero configuration
    ///
    /// Optimized for the 4-act journey with compel integration.
    /// 12-fold symmetry (the primitive), moderate gain, smooth motion.
    pub fn hero() -> Self {
        Self {
            render_mode: RenderMode::Extrusion,
            symmetry_mode: SymmetryMode::Twelve,
            base_radius: 1.0,
            gain: 0.6,
            thickness_multiplier: 1.0,
            rotation_speed: 0.08,
            smoothing_tau: 0.12,
            visual: VisualConfig {
                glow: 0.5,
                fog_density: 0.0,
                exposure: 1.1,
                camera_distance: 5.0,
                camera_height: 2.0,
                camera_angle: 0.0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hero_config_valid() {
        let config = HeroConfig::hero();
        assert!(config.base_radius > 0.0);
        assert!(config.gain >= 0.0);
        assert!(config.rotation_speed > 0.0);
    }

    #[test]
    fn test_render_mode_values() {
        assert_eq!(RenderMode::Extrusion.as_i32(), 0);
        assert_eq!(RenderMode::Lathe.as_i32(), 1);
        assert_eq!(RenderMode::Both.as_i32(), 2);
    }

    #[test]
    fn test_symmetry_mode_values() {
        assert_eq!(SymmetryMode::Twelve.as_i32(), 12);
        assert_eq!(SymmetryMode::TwentyFour.as_i32(), 24);
    }
}

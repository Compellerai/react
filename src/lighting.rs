//! Shared lighting device types for the unified device registry.
//!
//! Provides common types (FormFactor, DeviceBackend, DeviceCapabilities) used
//! across Hue and WLED integrations, enabling a single "Devices" UI while
//! keeping protocol-specific implementations separate.

/// Physical form factor of a lighting device
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FormFactor {
    Bulb,
    Spot,
    Strip,
    PlayBar,
    Unknown,
}

impl FormFactor {
    pub fn as_str(&self) -> &'static str {
        match self {
            FormFactor::Bulb => "Bulb",
            FormFactor::Spot => "Spot",
            FormFactor::Strip => "Strip",
            FormFactor::PlayBar => "Play Bar",
            FormFactor::Unknown => "Unknown",
        }
    }

    pub fn all() -> &'static [FormFactor] {
        &[FormFactor::Bulb, FormFactor::Spot, FormFactor::Strip, FormFactor::PlayBar, FormFactor::Unknown]
    }
}

impl Default for FormFactor {
    fn default() -> Self {
        FormFactor::Unknown
    }
}

/// Backend protocol for a lighting device
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DeviceBackend {
    Hue,
    Wled,
}

/// Capability description for a lighting device
#[derive(Clone, Debug)]
pub struct DeviceCapabilities {
    pub supports_segments: bool,
    pub channel_count: usize,
}

impl DeviceCapabilities {
    /// Hue lights: single color, one channel
    pub fn hue() -> Self {
        Self {
            supports_segments: false,
            channel_count: 1,
        }
    }

    /// WLED strips: per-LED addressable
    pub fn wled(led_count: usize) -> Self {
        Self {
            supports_segments: true,
            channel_count: led_count,
        }
    }

    /// Human-readable capability note for UI display
    pub fn note(&self) -> &'static str {
        if self.supports_segments {
            "per-LED addressable"
        } else {
            "single-color"
        }
    }
}

/// Detect form factor from a Hue bridge archetype string.
///
/// The Hue v1 API returns archetype strings like "sultanbulb", "huelightstrip",
/// "recessedfloor", "spotbulb", etc. We map these to our FormFactor enum.
pub fn form_factor_from_hue_archetype(archetype: &str) -> FormFactor {
    let lower = archetype.to_lowercase();

    if lower.contains("strip") {
        FormFactor::Strip
    } else if lower.contains("play") || lower.contains("wall_washer") || lower.contains("washer") {
        FormFactor::PlayBar
    } else if lower.contains("spot") || lower.contains("recessed") || lower.contains("ceiling") {
        FormFactor::Spot
    } else if lower.contains("bulb")
        || lower.contains("bloom")
        || lower.contains("candle")
        || lower.contains("flood")
        || lower.contains("pendant")
        || lower.contains("lamp")
        || lower.contains("shade")
        || lower.contains("table")
    {
        FormFactor::Bulb
    } else {
        FormFactor::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_archetype_detection() {
        assert_eq!(form_factor_from_hue_archetype("huelightstrip"), FormFactor::Strip);
        assert_eq!(form_factor_from_hue_archetype("hue_lightstrip"), FormFactor::Strip);
        assert_eq!(form_factor_from_hue_archetype("sultanbulb"), FormFactor::Bulb);
        assert_eq!(form_factor_from_hue_archetype("spotbulb"), FormFactor::Spot);
        assert_eq!(form_factor_from_hue_archetype("recessedfloor"), FormFactor::Spot);
        assert_eq!(form_factor_from_hue_archetype("ceilinground"), FormFactor::Spot);
        assert_eq!(form_factor_from_hue_archetype("huebloom"), FormFactor::Bulb);
        assert_eq!(form_factor_from_hue_archetype("pendantround"), FormFactor::Bulb);
        assert_eq!(form_factor_from_hue_archetype("candlebulb"), FormFactor::Bulb);
        assert_eq!(form_factor_from_hue_archetype("floodlight"), FormFactor::Bulb);
        assert_eq!(form_factor_from_hue_archetype("tablelamp"), FormFactor::Bulb);
        assert_eq!(form_factor_from_hue_archetype("table_shade"), FormFactor::Bulb);
        // Play Bar archetypes
        assert_eq!(form_factor_from_hue_archetype("wall_washer"), FormFactor::PlayBar);
        assert_eq!(form_factor_from_hue_archetype("hue_play"), FormFactor::PlayBar);
        assert_eq!(form_factor_from_hue_archetype("unknown_type"), FormFactor::Unknown);
        assert_eq!(form_factor_from_hue_archetype(""), FormFactor::Unknown);
    }
}

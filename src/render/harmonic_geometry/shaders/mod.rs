//! Shader access layer
//!
//! Serves the GLSL shader source from `source.rs`. Every build profile uses the
//! same readable source.
//!
//! The source constants begin with a newline (`r#"` followed by a line break);
//! `trim_start` puts `#version` first, where GLSL requires it.

mod source;

/// Vertex shader for fullscreen quad
pub fn vertex_shader() -> &'static str {
    source::VERTEX_SHADER.trim_start()
}

/// Harmonic field fragment shader (main renderer)
pub fn harmonic_field_fragment_shader() -> &'static str {
    source::HARMONIC_FIELD_FRAGMENT_SHADER.trim_start()
}

/// Legacy SDF fragment shader (preserved for reference)
#[allow(dead_code)]
pub fn legacy_sdf_fragment_shader() -> &'static str {
    source::LEGACY_SDF_FRAGMENT_SHADER.trim_start()
}

/// Legacy polygon fragment shader (preserved for reference)
#[allow(dead_code)]
pub fn legacy_polygon_fragment_shader() -> &'static str {
    source::LEGACY_POLYGON_FRAGMENT_SHADER.trim_start()
}

/// Upscale fragment shader for compositing low-res interior
pub fn upscale_fragment_shader() -> &'static str {
    source::UPSCALE_FRAGMENT_SHADER.trim_start()
}

/// TAA resolve fragment shader
pub fn taa_resolve_fragment_shader() -> &'static str {
    source::TAA_RESOLVE_FRAGMENT_SHADER.trim_start()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shader_starts_with_its_version_directive() {
        for (name, src) in [
            ("vertex", vertex_shader()),
            ("harmonic_field", harmonic_field_fragment_shader()),
            ("upscale", upscale_fragment_shader()),
            ("taa_resolve", taa_resolve_fragment_shader()),
        ] {
            assert!(src.starts_with("#version"), "{name} shader must begin with #version");
        }
    }
}

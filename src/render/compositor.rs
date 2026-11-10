//! Compositor shader for blending projectM and video layers

use anyhow::{anyhow, Result};
use eframe::glow::{self, HasContext};
use std::sync::Arc;

use super::RenderParams;

/// Vertex shader for fullscreen quad (GL 1.40 compatible for macOS Metal)
const VERTEX_SHADER: &str = r#"
#version 140

in vec2 aPos;
in vec2 aTexCoord;

out vec2 vTexCoord;

void main() {
    gl_Position = vec4(aPos, 0.0, 1.0);
    vTexCoord = aTexCoord;
}
"#;

/// Fragment shader for compositing with high-quality bloom and cinematic effects (GL 1.40 compatible)
const FRAGMENT_SHADER: &str = r#"
#version 140

in vec2 vTexCoord;
out vec4 FragColor;

uniform sampler2D uProjectMTex;
uniform sampler2D uVideoTex;
uniform float uProjectMVisibility;
uniform float uProjectMScale;
uniform float uVideoVisibility;
uniform float uVideoScale;
uniform vec2 uVideoOffset;
uniform int uHasVideo;
uniform float uLumaKey;  // Luma key strength (0=off, 1=full - dark pixels become transparent)
uniform float uBloomIntensity;
uniform float uVignetteIntensity;
uniform float uTime;  // For film grain animation

// Sample with clamped UV
vec4 sampleClamped(sampler2D tex, vec2 uv) {
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return vec4(0.0);
    }
    return texture(tex, uv);
}

// High-quality multi-scale Kawase-style bloom
// Samples at multiple radii with Gaussian-weighted falloff
vec3 sampleBloomHQ(sampler2D tex, vec2 uv, float scale) {
    vec3 bloom = vec3(0.0);
    float totalWeight = 0.0;

    // Soft threshold with knee for smoother bloom
    vec4 center = sampleClamped(tex, uv);
    float centerLum = dot(center.rgb, vec3(0.2126, 0.7152, 0.0722));

    // Multi-scale sampling for smooth, wide bloom
    // Scale 1: Fine detail (small radius)
    float radius1 = 0.002 * scale;
    // Scale 2: Medium glow
    float radius2 = 0.005 * scale;
    // Scale 3: Wide glow
    float radius3 = 0.012 * scale;

    // 13-tap pattern at each scale (center + 12 around)
    vec2 offsets[12];
    offsets[0] = vec2(1.0, 0.0);
    offsets[1] = vec2(0.866, 0.5);
    offsets[2] = vec2(0.5, 0.866);
    offsets[3] = vec2(0.0, 1.0);
    offsets[4] = vec2(-0.5, 0.866);
    offsets[5] = vec2(-0.866, 0.5);
    offsets[6] = vec2(-1.0, 0.0);
    offsets[7] = vec2(-0.866, -0.5);
    offsets[8] = vec2(-0.5, -0.866);
    offsets[9] = vec2(0.0, -1.0);
    offsets[10] = vec2(0.5, -0.866);
    offsets[11] = vec2(0.866, -0.5);

    // Sample at each scale
    for (int s = 0; s < 3; s++) {
        float radius = (s == 0) ? radius1 : ((s == 1) ? radius2 : radius3);
        float scaleWeight = (s == 0) ? 1.0 : ((s == 1) ? 0.6 : 0.3);

        for (int i = 0; i < 12; i++) {
            vec2 offset = offsets[i] * radius;
            vec4 sampleCol = sampleClamped(tex, uv + offset);

            // Soft threshold with knee curve for natural bloom falloff
            float lum = dot(sampleCol.rgb, vec3(0.2126, 0.7152, 0.0722));
            float threshold = 0.4;
            float knee = 0.2;
            float soft = lum - threshold + knee;
            soft = clamp(soft * soft / (4.0 * knee + 0.0001), 0.0, 1.0);
            float bright = max(soft, max(0.0, lum - threshold - knee));

            float weight = scaleWeight;
            bloom += sampleCol.rgb * bright * weight;
            totalWeight += weight;
        }
    }

    return bloom / max(totalWeight, 1.0);
}

// Chromatic aberration - subtle RGB channel separation
vec3 chromaticAberration(sampler2D tex, vec2 uv, float amount) {
    vec2 dir = (uv - 0.5) * amount;
    float r = sampleClamped(tex, uv + dir).r;
    float g = sampleClamped(tex, uv).g;
    float b = sampleClamped(tex, uv - dir).b;
    return vec3(r, g, b);
}

// Film grain - animated noise for cinematic texture
float filmGrain(vec2 uv, float time) {
    // Hash function for pseudo-random noise
    vec2 p = uv * vec2(12.9898, 78.233) + time * 0.1;
    float noise = fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
    // Make grain more organic with second pass
    noise = (noise + fract(sin(dot(p * 2.0, vec2(78.233, 12.9898))) * 23421.631)) * 0.5;
    return noise;
}

// Smooth vignette with configurable falloff
float smoothVignette(vec2 uv, float intensity, float smoothness) {
    vec2 center = uv - 0.5;
    float dist = length(center);
    // Smooth falloff using smoothstep
    float vignette = 1.0 - smoothstep(0.3, 0.7 + smoothness, dist * (1.0 + intensity));
    return vignette;
}

void main() {
    // Sample projectM texture with scale
    vec2 pmUV = (vTexCoord - 0.5) / uProjectMScale + 0.5;
    vec4 pmColor = sampleClamped(uProjectMTex, pmUV);

    // Sample video texture with scale and offset
    vec2 vidUV = (vTexCoord - 0.5) / uVideoScale + 0.5 + uVideoOffset;
    vec4 vidColor = vec4(0.0);
    if (uHasVideo != 0) {
        vidColor = sampleClamped(uVideoTex, vidUV);
    }

    // Composite layers: projectM as base, video on top
    vec4 result = vec4(0.0);

    // projectM/theme is the base layer
    if (uProjectMVisibility > 0.0) {
        result.rgb = pmColor.rgb * uProjectMVisibility;
        // Base layer alpha = visibility (for proper compositing in NDI/external streams)
        result.a = uProjectMVisibility;
    }

    // Video overlays on top with alpha blend
    if (uHasVideo != 0 && uVideoVisibility > 0.0) {
        // Calculate video alpha based on visibility and optional luma key
        float videoAlpha = uVideoVisibility;

        // Luma key: make dark areas transparent so visuals show through
        if (uLumaKey > 0.0) {
            // Calculate luminance using perceptual weights
            float luma = dot(vidColor.rgb, vec3(0.2126, 0.7152, 0.0722));
            // Apply smooth curve - darker pixels become more transparent
            // smoothstep gives nice falloff, threshold at 0.15 keeps some blacks visible
            float lumaAlpha = smoothstep(0.0, 0.3, luma);
            // Blend between full visibility and luma-keyed based on strength
            videoAlpha *= mix(1.0, lumaAlpha, uLumaKey);
        }

        // Alpha blend: video over projectM
        result.rgb = mix(result.rgb, vidColor.rgb, videoAlpha);

        // Output alpha for NDI/external streams:
        // When luma key is active, dark video areas should be transparent in output
        // so external receivers can composite their own background
        if (uLumaKey > 0.0) {
            // Use proper alpha compositing: A_out = A_fg + A_bg * (1 - A_fg)
            // Where video is transparent (dark), let projectM alpha show through
            float baseAlpha = uProjectMVisibility;
            result.a = videoAlpha + baseAlpha * (1.0 - videoAlpha);
        } else {
            // No luma key - just take max of base and video alpha
            result.a = max(result.a, vidColor.a * videoAlpha);
        }
    }

    // Apply high-quality bloom effect
    if (uBloomIntensity > 0.0) {
        vec3 bloom = sampleBloomHQ(uProjectMTex, pmUV, uProjectMScale);
        // Add bloom with color preservation
        result.rgb += bloom * uBloomIntensity * 2.0;

        // Subtle chromatic aberration on bloom (cinematic look)
        float chromaAmount = uBloomIntensity * 0.003;
        if (chromaAmount > 0.0001) {
            vec3 chromaBloom = chromaticAberration(uProjectMTex, pmUV, chromaAmount);
            float chromaLum = dot(chromaBloom, vec3(0.2126, 0.7152, 0.0722));
            float chromaBright = max(0.0, chromaLum - 0.6) * 0.5;
            result.rgb += (chromaBloom - result.rgb) * chromaBright * uBloomIntensity;
        }
    }

    // Apply smooth vignette with natural falloff
    if (uVignetteIntensity > 0.0) {
        float vignette = smoothVignette(vTexCoord, uVignetteIntensity, 0.2);
        result.rgb *= vignette;
    }

    // Subtle film grain for cinematic texture (only when bloom is active)
    if (uBloomIntensity > 0.1) {
        float grain = filmGrain(vTexCoord, uTime);
        // Very subtle grain, more visible in dark areas
        float grainAmount = 0.015 * uBloomIntensity;
        float luminance = dot(result.rgb, vec3(0.2126, 0.7152, 0.0722));
        grainAmount *= (1.0 - luminance * 0.5); // Less grain in bright areas
        result.rgb += (grain - 0.5) * grainAmount;
    }

    // Slight contrast boost for punchier visuals
    result.rgb = (result.rgb - 0.5) * 1.05 + 0.5;

    // Clamp to valid range
    result.rgb = clamp(result.rgb, 0.0, 1.0);

    FragColor = result;
}
"#;

/// Simple texture drawing shader
const SIMPLE_VERTEX_SHADER: &str = r#"
#version 140

in vec2 aPos;
in vec2 aTexCoord;
out vec2 vTexCoord;

void main() {
    gl_Position = vec4(aPos, 0.0, 1.0);
    vTexCoord = aTexCoord;
}
"#;

const SIMPLE_FRAGMENT_SHADER: &str = r#"
#version 140

in vec2 vTexCoord;
out vec4 FragColor;

uniform sampler2D uTexture;
uniform float uAlpha;
uniform float uScale;
uniform vec2 uAspectRatio;  // (x_ratio, y_ratio) - values < 1 mean letterbox/pillarbox
uniform int uSwizzleBGRA;   // If 1, swap R and B channels (BGRA -> RGBA in shader)
uniform int uFlipY;         // If 1, flip Y coordinate (video is top-down, OpenGL is bottom-up)
uniform float uLumaKey;     // Luma key strength (0=off, 1=full - dark pixels become transparent)

void main() {
    // Apply scale first (centered)
    vec2 uv = (vTexCoord - 0.5) / uScale + 0.5;

    // Apply aspect ratio correction
    // Map from screen space (with letterbox/pillarbox) to texture space
    vec2 offset = (1.0 - uAspectRatio) / 2.0;
    vec2 texCoord = (uv - offset) / uAspectRatio;

    // Flip Y if needed (video frames are top-down, OpenGL is bottom-up)
    if (uFlipY != 0) {
        texCoord.y = 1.0 - texCoord.y;
    }

    // Only sample if within texture bounds
    if (texCoord.x < 0.0 || texCoord.x > 1.0 || texCoord.y < 0.0 || texCoord.y > 1.0) {
        FragColor = vec4(0.0);
    } else {
        // Soft edge feathering - optimized with single smoothstep per axis
        float edgeSoftness = 0.15;
        float edgeX = min(texCoord.x, 1.0 - texCoord.x);
        float edgeY = min(texCoord.y, 1.0 - texCoord.y);
        float edgeAlpha = smoothstep(0.0, edgeSoftness, edgeX) * smoothstep(0.0, edgeSoftness, edgeY);

        vec4 color = texture(uTexture, texCoord);

        // BGRA -> RGBA swizzle in shader (free on GPU, saves CPU conversion)
        if (uSwizzleBGRA != 0) {
            color = color.bgra;
        }

        // Luma key: make dark areas transparent so visuals show through
        float lumaAlpha = 1.0;
        if (uLumaKey > 0.0) {
            float luma = dot(color.rgb, vec3(0.2126, 0.7152, 0.0722));
            lumaAlpha = mix(1.0, smoothstep(0.0, 0.3, luma), uLumaKey);
        }

        FragColor = color * uAlpha * edgeAlpha * lumaAlpha;
    }
}
"#;

/// Solid-color fade overlay shader — fullscreen black quad with uniform alpha.
/// Used to fade the theme to black via projectm_visibility (continuous opacity control).
const FADE_VERTEX_SHADER: &str = r#"
#version 140

in vec2 aPos;

void main() {
    gl_Position = vec4(aPos, 0.0, 1.0);
}
"#;

const FADE_FRAGMENT_SHADER: &str = r#"
#version 140

out vec4 FragColor;

uniform float uAlpha;

void main() {
    FragColor = vec4(0.0, 0.0, 0.0, uAlpha);
}
"#;

/// Rectangle texture shader (for GL_TEXTURE_RECTANGLE from CVOpenGLTexture)
/// Uses pixel coordinates instead of normalized 0-1 coordinates
const RECT_VERTEX_SHADER: &str = r#"
#version 140

in vec2 aPos;
in vec2 aTexCoord;
out vec2 vTexCoord;

void main() {
    gl_Position = vec4(aPos, 0.0, 1.0);
    vTexCoord = aTexCoord;
}
"#;

/// Rectangle texture fragment shader
/// sampler2DRect uses pixel coordinates (0 to width, 0 to height)
const RECT_FRAGMENT_SHADER: &str = r#"
#version 140
#extension GL_ARB_texture_rectangle : enable

in vec2 vTexCoord;
out vec4 FragColor;

uniform sampler2DRect uTexture;
uniform float uAlpha;
uniform float uScale;
uniform vec2 uAspectRatio;
uniform vec2 uTexSize;  // Actual texture size in pixels
uniform int uFlipY;
uniform float uLumaKey;  // Luma key strength (0=off, 1=full - dark pixels become transparent)

void main() {
    // Apply scale first (centered) - vTexCoord is still 0-1 normalized
    vec2 uv = (vTexCoord - 0.5) / uScale + 0.5;

    // Apply aspect ratio correction
    vec2 offset = (1.0 - uAspectRatio) / 2.0;
    vec2 normCoord = (uv - offset) / uAspectRatio;

    // Flip Y if needed (video frames are top-down, OpenGL is bottom-up)
    if (uFlipY != 0) {
        normCoord.y = 1.0 - normCoord.y;
    }

    // Only sample if within texture bounds
    if (normCoord.x < 0.0 || normCoord.x > 1.0 || normCoord.y < 0.0 || normCoord.y > 1.0) {
        FragColor = vec4(0.0);
    } else {
        // Soft edge feathering
        float edgeSoftness = 0.15;
        float edgeX = min(normCoord.x, 1.0 - normCoord.x);
        float edgeY = min(normCoord.y, 1.0 - normCoord.y);
        float edgeAlpha = smoothstep(0.0, edgeSoftness, edgeX) * smoothstep(0.0, edgeSoftness, edgeY);

        // Convert normalized coords to pixel coords for sampler2DRect
        vec2 pixelCoord = normCoord * uTexSize;
        vec4 color = texture(uTexture, pixelCoord);

        // Luma key: make dark areas transparent so visuals show through
        float lumaAlpha = 1.0;
        if (uLumaKey > 0.0) {
            float luma = dot(color.rgb, vec3(0.2126, 0.7152, 0.0722));
            lumaAlpha = mix(1.0, smoothstep(0.0, 0.3, luma), uLumaKey);
        }

        FragColor = color * uAlpha * edgeAlpha * lumaAlpha;
    }
}
"#;

/// Overlay vertex shader - draws a positioned quad (not fullscreen)
/// Uses NDC coordinates with position and size uniforms
const OVERLAY_VERTEX_SHADER: &str = r#"
#version 140

in vec2 aPos;       // -1 to 1 for fullscreen quad
in vec2 aTexCoord;  // 0 to 1

out vec2 vTexCoord;

uniform vec2 uPosition;  // Position in NDC (-1 to 1)
uniform vec2 uSize;      // Size in NDC (0 to 2 for full screen)

void main() {
    // Transform from fullscreen quad to positioned overlay
    vec2 pos = aPos * uSize + uPosition;
    gl_Position = vec4(pos, 0.0, 1.0);
    vTexCoord = aTexCoord;
}
"#;

/// Overlay fragment shader - simple alpha-blended texture with glow
const OVERLAY_FRAGMENT_SHADER: &str = r#"
#version 140

in vec2 vTexCoord;
out vec4 FragColor;

uniform sampler2D uTexture;
uniform float uAlpha;
uniform float uGlow;  // Glow intensity (0-1)

void main() {
    // Flip Y to correct for OpenGL coordinate system vs image coordinate system
    vec2 flippedUV = vec2(vTexCoord.x, 1.0 - vTexCoord.y);
    vec4 color = texture(uTexture, flippedUV);

    // Apply alpha
    color.a *= uAlpha;

    // Optional glow effect - brighten and saturate
    if (uGlow > 0.0) {
        vec3 glowColor = color.rgb * (1.0 + uGlow * 0.5);
        color.rgb = mix(color.rgb, glowColor, uGlow);
    }

    FragColor = color;
}
"#;

/// Compositor for blending projectM and video layers with post-processing
pub struct Compositor {
    gl: Arc<glow::Context>,
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,

    // Uniform locations for composite shader
    loc_projectm_tex: Option<glow::UniformLocation>,
    loc_video_tex: Option<glow::UniformLocation>,
    loc_projectm_visibility: Option<glow::UniformLocation>,
    loc_projectm_scale: Option<glow::UniformLocation>,
    loc_video_visibility: Option<glow::UniformLocation>,
    loc_video_scale: Option<glow::UniformLocation>,
    loc_video_offset: Option<glow::UniformLocation>,
    loc_has_video: Option<glow::UniformLocation>,
    loc_luma_key: Option<glow::UniformLocation>,
    // Post-processing uniforms
    loc_bloom_intensity: Option<glow::UniformLocation>,
    loc_vignette_intensity: Option<glow::UniformLocation>,
    loc_time: Option<glow::UniformLocation>,

    // Time tracking for animated effects
    start_time: std::time::Instant,

    // Simple texture drawing shader
    simple_program: glow::Program,
    loc_simple_texture: Option<glow::UniformLocation>,
    loc_simple_alpha: Option<glow::UniformLocation>,
    loc_simple_scale: Option<glow::UniformLocation>,
    loc_simple_aspect: Option<glow::UniformLocation>,
    loc_simple_swizzle: Option<glow::UniformLocation>,
    loc_simple_flip_y: Option<glow::UniformLocation>,
    loc_simple_luma_key: Option<glow::UniformLocation>,

    // Rectangle texture shader (for GL_TEXTURE_RECTANGLE from IOSurface)
    rect_program: Option<glow::Program>,
    loc_rect_texture: Option<glow::UniformLocation>,
    loc_rect_alpha: Option<glow::UniformLocation>,
    loc_rect_scale: Option<glow::UniformLocation>,
    loc_rect_aspect: Option<glow::UniformLocation>,
    loc_rect_tex_size: Option<glow::UniformLocation>,
    loc_rect_flip_y: Option<glow::UniformLocation>,
    loc_rect_luma_key: Option<glow::UniformLocation>,

    // Overlay shader for positioned logo/watermark rendering
    overlay_program: glow::Program,
    loc_overlay_texture: Option<glow::UniformLocation>,
    loc_overlay_alpha: Option<glow::UniformLocation>,
    loc_overlay_position: Option<glow::UniformLocation>,
    loc_overlay_size: Option<glow::UniformLocation>,
    loc_overlay_glow: Option<glow::UniformLocation>,

    // Fade-to-black shader for continuous theme visibility (projectm_visibility slider)
    fade_program: glow::Program,
    loc_fade_alpha: Option<glow::UniformLocation>,
}

impl Compositor {
    /// Create a new compositor
    pub fn new(gl: Arc<glow::Context>) -> Result<Self> {
        log::info!("Creating compositor");

        let program = unsafe {
            // Compile vertex shader
            let vs = gl.create_shader(glow::VERTEX_SHADER)
                .map_err(|e| anyhow!("Failed to create vertex shader: {}", e))?;
            gl.shader_source(vs, VERTEX_SHADER);
            gl.compile_shader(vs);
            if !gl.get_shader_compile_status(vs) {
                let log = gl.get_shader_info_log(vs);
                return Err(anyhow!("Vertex shader compile error: {}", log));
            }

            // Compile fragment shader
            let fs = gl.create_shader(glow::FRAGMENT_SHADER)
                .map_err(|e| anyhow!("Failed to create fragment shader: {}", e))?;
            gl.shader_source(fs, FRAGMENT_SHADER);
            gl.compile_shader(fs);
            if !gl.get_shader_compile_status(fs) {
                let log = gl.get_shader_info_log(fs);
                return Err(anyhow!("Fragment shader compile error: {}", log));
            }

            // Link program
            let program = gl.create_program()
                .map_err(|e| anyhow!("Failed to create program: {}", e))?;
            gl.attach_shader(program, vs);
            gl.attach_shader(program, fs);
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                let log = gl.get_program_info_log(program);
                return Err(anyhow!("Program link error: {}", log));
            }

            // Clean up shader objects
            gl.delete_shader(vs);
            gl.delete_shader(fs);

            program
        };

        // Get uniform locations for composite shader
        let loc_projectm_tex = unsafe { gl.get_uniform_location(program, "uProjectMTex") };
        let loc_video_tex = unsafe { gl.get_uniform_location(program, "uVideoTex") };
        let loc_projectm_visibility = unsafe { gl.get_uniform_location(program, "uProjectMVisibility") };
        let loc_projectm_scale = unsafe { gl.get_uniform_location(program, "uProjectMScale") };
        let loc_video_visibility = unsafe { gl.get_uniform_location(program, "uVideoVisibility") };
        let loc_video_scale = unsafe { gl.get_uniform_location(program, "uVideoScale") };
        let loc_video_offset = unsafe { gl.get_uniform_location(program, "uVideoOffset") };
        let loc_has_video = unsafe { gl.get_uniform_location(program, "uHasVideo") };
        let loc_luma_key = unsafe { gl.get_uniform_location(program, "uLumaKey") };
        // Post-processing uniforms
        let loc_bloom_intensity = unsafe { gl.get_uniform_location(program, "uBloomIntensity") };
        let loc_vignette_intensity = unsafe { gl.get_uniform_location(program, "uVignetteIntensity") };
        let loc_time = unsafe { gl.get_uniform_location(program, "uTime") };

        // Create simple texture drawing shader
        let simple_program = unsafe {
            let vs = gl.create_shader(glow::VERTEX_SHADER)
                .map_err(|e| anyhow!("Failed to create simple vertex shader: {}", e))?;
            gl.shader_source(vs, SIMPLE_VERTEX_SHADER);
            gl.compile_shader(vs);
            if !gl.get_shader_compile_status(vs) {
                let log = gl.get_shader_info_log(vs);
                return Err(anyhow!("Simple vertex shader compile error: {}", log));
            }

            let fs = gl.create_shader(glow::FRAGMENT_SHADER)
                .map_err(|e| anyhow!("Failed to create simple fragment shader: {}", e))?;
            gl.shader_source(fs, SIMPLE_FRAGMENT_SHADER);
            gl.compile_shader(fs);
            if !gl.get_shader_compile_status(fs) {
                let log = gl.get_shader_info_log(fs);
                return Err(anyhow!("Simple fragment shader compile error: {}", log));
            }

            let prog = gl.create_program()
                .map_err(|e| anyhow!("Failed to create simple program: {}", e))?;
            gl.attach_shader(prog, vs);
            gl.attach_shader(prog, fs);
            gl.link_program(prog);
            if !gl.get_program_link_status(prog) {
                let log = gl.get_program_info_log(prog);
                return Err(anyhow!("Simple program link error: {}", log));
            }

            gl.delete_shader(vs);
            gl.delete_shader(fs);
            prog
        };

        let loc_simple_texture = unsafe { gl.get_uniform_location(simple_program, "uTexture") };
        let loc_simple_alpha = unsafe { gl.get_uniform_location(simple_program, "uAlpha") };
        let loc_simple_scale = unsafe { gl.get_uniform_location(simple_program, "uScale") };
        let loc_simple_aspect = unsafe { gl.get_uniform_location(simple_program, "uAspectRatio") };
        let loc_simple_swizzle = unsafe { gl.get_uniform_location(simple_program, "uSwizzleBGRA") };
        let loc_simple_flip_y = unsafe { gl.get_uniform_location(simple_program, "uFlipY") };
        let loc_simple_luma_key = unsafe { gl.get_uniform_location(simple_program, "uLumaKey") };

        // Create rectangle texture shader (for GL_TEXTURE_RECTANGLE from IOSurface)
        // This may fail on systems without GL_ARB_texture_rectangle, so it's optional
        let (rect_program, loc_rect_texture, loc_rect_alpha, loc_rect_scale,
             loc_rect_aspect, loc_rect_tex_size, loc_rect_flip_y, loc_rect_luma_key) = unsafe {
            let vs = gl.create_shader(glow::VERTEX_SHADER).ok();
            let fs = gl.create_shader(glow::FRAGMENT_SHADER).ok();

            if let (Some(vs), Some(fs)) = (vs, fs) {
                gl.shader_source(vs, RECT_VERTEX_SHADER);
                gl.compile_shader(vs);

                gl.shader_source(fs, RECT_FRAGMENT_SHADER);
                gl.compile_shader(fs);

                if gl.get_shader_compile_status(vs) && gl.get_shader_compile_status(fs) {
                    if let Ok(prog) = gl.create_program() {
                        gl.attach_shader(prog, vs);
                        gl.attach_shader(prog, fs);
                        gl.link_program(prog);

                        gl.delete_shader(vs);
                        gl.delete_shader(fs);

                        if gl.get_program_link_status(prog) {
                            log::info!("[Compositor] Rectangle texture shader compiled (IOSurface support)");
                            (
                                Some(prog),
                                gl.get_uniform_location(prog, "uTexture"),
                                gl.get_uniform_location(prog, "uAlpha"),
                                gl.get_uniform_location(prog, "uScale"),
                                gl.get_uniform_location(prog, "uAspectRatio"),
                                gl.get_uniform_location(prog, "uTexSize"),
                                gl.get_uniform_location(prog, "uFlipY"),
                                gl.get_uniform_location(prog, "uLumaKey"),
                            )
                        } else {
                            let log_msg = gl.get_program_info_log(prog);
                            log::warn!("[Compositor] Rect program link failed: {}", log_msg);
                            gl.delete_program(prog);
                            (None, None, None, None, None, None, None, None)
                        }
                    } else {
                        gl.delete_shader(vs);
                        gl.delete_shader(fs);
                        (None, None, None, None, None, None, None, None)
                    }
                } else {
                    let vs_log = gl.get_shader_info_log(vs);
                    let fs_log = gl.get_shader_info_log(fs);
                    log::warn!("[Compositor] Rect shader compile failed: VS={} FS={}", vs_log, fs_log);
                    gl.delete_shader(vs);
                    gl.delete_shader(fs);
                    (None, None, None, None, None, None, None, None)
                }
            } else {
                (None, None, None, None, None, None, None, None)
            }
        };

        // Create overlay shader for positioned logo/watermark rendering
        let overlay_program = unsafe {
            let vs = gl.create_shader(glow::VERTEX_SHADER)
                .map_err(|e| anyhow!("Failed to create overlay vertex shader: {}", e))?;
            gl.shader_source(vs, OVERLAY_VERTEX_SHADER);
            gl.compile_shader(vs);
            if !gl.get_shader_compile_status(vs) {
                let log = gl.get_shader_info_log(vs);
                return Err(anyhow!("Overlay vertex shader compile error: {}", log));
            }

            let fs = gl.create_shader(glow::FRAGMENT_SHADER)
                .map_err(|e| anyhow!("Failed to create overlay fragment shader: {}", e))?;
            gl.shader_source(fs, OVERLAY_FRAGMENT_SHADER);
            gl.compile_shader(fs);
            if !gl.get_shader_compile_status(fs) {
                let log = gl.get_shader_info_log(fs);
                return Err(anyhow!("Overlay fragment shader compile error: {}", log));
            }

            let prog = gl.create_program()
                .map_err(|e| anyhow!("Failed to create overlay program: {}", e))?;
            gl.attach_shader(prog, vs);
            gl.attach_shader(prog, fs);
            gl.link_program(prog);
            if !gl.get_program_link_status(prog) {
                let log = gl.get_program_info_log(prog);
                return Err(anyhow!("Overlay program link error: {}", log));
            }

            gl.delete_shader(vs);
            gl.delete_shader(fs);
            prog
        };

        let loc_overlay_texture = unsafe { gl.get_uniform_location(overlay_program, "uTexture") };
        let loc_overlay_alpha = unsafe { gl.get_uniform_location(overlay_program, "uAlpha") };
        let loc_overlay_position = unsafe { gl.get_uniform_location(overlay_program, "uPosition") };
        let loc_overlay_size = unsafe { gl.get_uniform_location(overlay_program, "uSize") };
        let loc_overlay_glow = unsafe { gl.get_uniform_location(overlay_program, "uGlow") };

        // Create fade-to-black shader (solid-color quad with uniform alpha)
        let fade_program = unsafe {
            let vs = gl.create_shader(glow::VERTEX_SHADER)
                .map_err(|e| anyhow!("Failed to create fade vertex shader: {}", e))?;
            gl.shader_source(vs, FADE_VERTEX_SHADER);
            gl.compile_shader(vs);
            if !gl.get_shader_compile_status(vs) {
                let log = gl.get_shader_info_log(vs);
                return Err(anyhow!("Fade vertex shader compile error: {}", log));
            }

            let fs = gl.create_shader(glow::FRAGMENT_SHADER)
                .map_err(|e| anyhow!("Failed to create fade fragment shader: {}", e))?;
            gl.shader_source(fs, FADE_FRAGMENT_SHADER);
            gl.compile_shader(fs);
            if !gl.get_shader_compile_status(fs) {
                let log = gl.get_shader_info_log(fs);
                return Err(anyhow!("Fade fragment shader compile error: {}", log));
            }

            let prog = gl.create_program()
                .map_err(|e| anyhow!("Failed to create fade program: {}", e))?;
            gl.attach_shader(prog, vs);
            gl.attach_shader(prog, fs);
            gl.link_program(prog);
            if !gl.get_program_link_status(prog) {
                let log = gl.get_program_info_log(prog);
                return Err(anyhow!("Fade program link error: {}", log));
            }

            gl.delete_shader(vs);
            gl.delete_shader(fs);
            prog
        };

        let loc_fade_alpha = unsafe { gl.get_uniform_location(fade_program, "uAlpha") };

        // Create fullscreen quad VAO/VBO
        let (vao, vbo) = unsafe {
            let vao = gl.create_vertex_array()
                .map_err(|e| anyhow!("Failed to create VAO: {}", e))?;
            let vbo = gl.create_buffer()
                .map_err(|e| anyhow!("Failed to create VBO: {}", e))?;

            // Fullscreen quad vertices: position (x, y) + texcoord (u, v)
            #[rustfmt::skip]
            let vertices: [f32; 24] = [
                // Triangle 1
                -1.0, -1.0,  0.0, 0.0,  // bottom-left
                 1.0, -1.0,  1.0, 0.0,  // bottom-right
                 1.0,  1.0,  1.0, 1.0,  // top-right
                // Triangle 2
                -1.0, -1.0,  0.0, 0.0,  // bottom-left
                 1.0,  1.0,  1.0, 1.0,  // top-right
                -1.0,  1.0,  0.0, 1.0,  // top-left
            ];

            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&vertices),
                glow::STATIC_DRAW,
            );

            // Position attribute
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 16, 0);

            // TexCoord attribute
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 16, 8);

            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);

            (vao, vbo)
        };

        Ok(Self {
            gl,
            program,
            vao,
            vbo,
            loc_projectm_tex,
            loc_video_tex,
            loc_projectm_visibility,
            loc_projectm_scale,
            loc_video_visibility,
            loc_video_scale,
            loc_video_offset,
            loc_has_video,
            loc_luma_key,
            loc_bloom_intensity,
            loc_vignette_intensity,
            loc_time,
            start_time: std::time::Instant::now(),
            simple_program,
            loc_simple_texture,
            loc_simple_alpha,
            loc_simple_scale,
            loc_simple_aspect,
            loc_simple_swizzle,
            loc_simple_flip_y,
            loc_simple_luma_key,
            rect_program,
            loc_rect_texture,
            loc_rect_alpha,
            loc_rect_scale,
            loc_rect_aspect,
            loc_rect_tex_size,
            loc_rect_flip_y,
            loc_rect_luma_key,
            overlay_program,
            loc_overlay_texture,
            loc_overlay_alpha,
            loc_overlay_position,
            loc_overlay_size,
            loc_overlay_glow,
            fade_program,
            loc_fade_alpha,
        })
    }

    /// Draw a fullscreen black quad with the given alpha (0.0 = invisible, 1.0 = full black).
    /// Used to fade the theme layer to black for continuous projectm_visibility control:
    /// `alpha = 1.0 - projectm_visibility`. Skip when alpha is effectively zero.
    pub fn draw_fade_to_black(&self, alpha: f32) {
        if alpha <= 0.001 {
            return;
        }
        unsafe {
            self.gl.enable(glow::BLEND);
            self.gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);

            self.gl.use_program(Some(self.fade_program));
            if let Some(loc) = &self.loc_fade_alpha {
                self.gl.uniform_1_f32(Some(loc), alpha.clamp(0.0, 1.0));
            }

            self.gl.bind_vertex_array(Some(self.vao));
            self.gl.draw_arrays(glow::TRIANGLES, 0, 6);
            self.gl.bind_vertex_array(None);
            self.gl.use_program(None);
        }
    }

    /// Composite projectM and video textures to the current framebuffer
    pub fn composite(
        &self,
        projectm_texture: glow::Texture,
        video_texture: Option<glow::Texture>,
        _width: u32,
        _height: u32,
        params: &RenderParams,
    ) {
        unsafe {
            self.gl.use_program(Some(self.program));

            // Bind projectM texture to unit 0
            self.gl.active_texture(glow::TEXTURE0);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(projectm_texture));
            if let Some(loc) = &self.loc_projectm_tex {
                self.gl.uniform_1_i32(Some(loc), 0);
            }

            // Bind video texture to unit 1 (if available)
            let has_video = video_texture.is_some();
            self.gl.active_texture(glow::TEXTURE1);
            if let Some(tex) = video_texture {
                self.gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            }
            if let Some(loc) = &self.loc_video_tex {
                self.gl.uniform_1_i32(Some(loc), 1);
            }

            // Set uniforms
            if let Some(loc) = &self.loc_projectm_visibility {
                self.gl.uniform_1_f32(Some(loc), params.projectm_visibility);
            }
            if let Some(loc) = &self.loc_projectm_scale {
                self.gl.uniform_1_f32(Some(loc), params.projectm_scale);
            }
            if let Some(loc) = &self.loc_video_visibility {
                self.gl.uniform_1_f32(Some(loc), params.video_visibility);
            }
            if let Some(loc) = &self.loc_video_scale {
                self.gl.uniform_1_f32(Some(loc), params.video_scale);
            }
            if let Some(loc) = &self.loc_video_offset {
                self.gl.uniform_2_f32(Some(loc), params.video_x, params.video_y);
            }
            if let Some(loc) = &self.loc_has_video {
                self.gl.uniform_1_i32(Some(loc), if has_video { 1 } else { 0 });
            }
            if let Some(loc) = &self.loc_luma_key {
                self.gl.uniform_1_f32(Some(loc), params.video_luma_key);
            }

            // Post-processing uniforms
            if let Some(loc) = &self.loc_bloom_intensity {
                self.gl.uniform_1_f32(Some(loc), params.bloom_intensity);
            }
            if let Some(loc) = &self.loc_vignette_intensity {
                self.gl.uniform_1_f32(Some(loc), params.vignette_intensity);
            }
            // Time for animated effects (film grain)
            if let Some(loc) = &self.loc_time {
                let time = self.start_time.elapsed().as_secs_f32();
                self.gl.uniform_1_f32(Some(loc), time);
            }

            // Draw fullscreen quad
            self.gl.bind_vertex_array(Some(self.vao));
            self.gl.draw_arrays(glow::TRIANGLES, 0, 6);
            self.gl.bind_vertex_array(None);

            self.gl.use_program(None);
        }
    }

    /// Draw a single texture to the screen with alpha, scale, and aspect ratio correction
    /// swizzle_bgra: if true, swaps R and B channels in shader (for BGRA textures uploaded as RGBA)
    pub fn draw_texture_scaled(
        &self,
        texture: glow::Texture,
        viewport_width: u32,
        viewport_height: u32,
        video_width: u32,
        video_height: u32,
        alpha: f32,
        scale: f32,
    ) {
        self.draw_texture_scaled_ex(texture, viewport_width, viewport_height,
            video_width, video_height, alpha, scale, false, false, 0.0)
    }

    /// Extended version with swizzle and flip control
    /// swizzle_bgra: swap R and B channels (BGRA->RGBA)
    /// flip_y: flip Y coordinate (video is top-down, OpenGL is bottom-up)
    /// luma_key: make dark pixels transparent (0=off, 1=full)
    pub fn draw_texture_scaled_ex(
        &self,
        texture: glow::Texture,
        viewport_width: u32,
        viewport_height: u32,
        video_width: u32,
        video_height: u32,
        alpha: f32,
        scale: f32,
        swizzle_bgra: bool,
        flip_y: bool,
        luma_key: f32,
    ) {
        unsafe {
            self.gl.use_program(Some(self.simple_program));

            // Bind texture to unit 0
            self.gl.active_texture(glow::TEXTURE0);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            if let Some(loc) = &self.loc_simple_texture {
                self.gl.uniform_1_i32(Some(loc), 0);
            }

            // Set alpha
            if let Some(loc) = &self.loc_simple_alpha {
                self.gl.uniform_1_f32(Some(loc), alpha);
            }

            // Set scale
            if let Some(loc) = &self.loc_simple_scale {
                self.gl.uniform_1_f32(Some(loc), scale);
            }

            // Set swizzle flag (for BGRA textures)
            if let Some(loc) = &self.loc_simple_swizzle {
                self.gl.uniform_1_i32(Some(loc), if swizzle_bgra { 1 } else { 0 });
            }

            // Set flip Y flag (video is top-down, OpenGL is bottom-up)
            if let Some(loc) = &self.loc_simple_flip_y {
                self.gl.uniform_1_i32(Some(loc), if flip_y { 1 } else { 0 });
            }

            // Set luma key (dark pixels become transparent)
            if let Some(loc) = &self.loc_simple_luma_key {
                self.gl.uniform_1_f32(Some(loc), luma_key);
            }

            // Calculate aspect ratio correction
            // We want to fit the video while maintaining aspect ratio (contain mode)
            let video_aspect = video_width as f32 / video_height.max(1) as f32;
            let viewport_aspect = viewport_width as f32 / viewport_height.max(1) as f32;

            // Ratio determines how much of the viewport the video occupies in each axis
            let (aspect_x, aspect_y) = if video_aspect > viewport_aspect {
                // Video is wider than viewport - video fills width, letterbox top/bottom
                (1.0, viewport_aspect / video_aspect)
            } else {
                // Video is taller than viewport - video fills height, pillarbox left/right
                (video_aspect / viewport_aspect, 1.0)
            };

            if let Some(loc) = &self.loc_simple_aspect {
                self.gl.uniform_2_f32(Some(loc), aspect_x, aspect_y);
            }

            // Draw fullscreen quad
            self.gl.bind_vertex_array(Some(self.vao));
            self.gl.draw_arrays(glow::TRIANGLES, 0, 6);
            self.gl.bind_vertex_array(None);

            self.gl.use_program(None);
        }
    }

    /// Draw a single texture to the screen with alpha (legacy, no scale/aspect)
    pub fn draw_texture(
        &self,
        texture: glow::Texture,
        width: u32,
        height: u32,
        alpha: f32,
    ) {
        // Call scaled version with defaults
        self.draw_texture_scaled(texture, width, height, width, height, alpha, 1.0);
    }

    /// Draw a rectangle texture (GL_TEXTURE_RECTANGLE from CVOpenGLTexture)
    /// This handles IOSurface-backed textures which use pixel coordinates
    pub fn draw_texture_rect(
        &self,
        texture: glow::Texture,
        texture_target: u32,
        viewport_width: u32,
        viewport_height: u32,
        video_width: u32,
        video_height: u32,
        alpha: f32,
        scale: f32,
        flip_y: bool,
        luma_key: f32,
    ) {
        // If we don't have the rect program or it's a regular texture, fall back
        let rect_prog = match self.rect_program {
            Some(prog) if texture_target == glow::TEXTURE_RECTANGLE => prog,
            _ => {
                // Fall back to regular 2D texture drawing
                self.draw_texture_scaled_ex(
                    texture,
                    viewport_width,
                    viewport_height,
                    video_width,
                    video_height,
                    alpha,
                    scale,
                    false,
                    flip_y,
                    luma_key,
                );
                return;
            }
        };

        unsafe {
            self.gl.use_program(Some(rect_prog));

            // Bind texture to unit 0 with correct target
            self.gl.active_texture(glow::TEXTURE0);
            self.gl.bind_texture(glow::TEXTURE_RECTANGLE, Some(texture));
            if let Some(loc) = &self.loc_rect_texture {
                self.gl.uniform_1_i32(Some(loc), 0);
            }

            // Set alpha
            if let Some(loc) = &self.loc_rect_alpha {
                self.gl.uniform_1_f32(Some(loc), alpha);
            }

            // Set scale
            if let Some(loc) = &self.loc_rect_scale {
                self.gl.uniform_1_f32(Some(loc), scale);
            }

            // Set texture size (needed for pixel coordinate conversion)
            if let Some(loc) = &self.loc_rect_tex_size {
                self.gl.uniform_2_f32(Some(loc), video_width as f32, video_height as f32);
            }

            // Set flip Y flag
            if let Some(loc) = &self.loc_rect_flip_y {
                self.gl.uniform_1_i32(Some(loc), if flip_y { 1 } else { 0 });
            }

            // Set luma key (dark pixels become transparent)
            if let Some(loc) = &self.loc_rect_luma_key {
                self.gl.uniform_1_f32(Some(loc), luma_key);
            }

            // Calculate aspect ratio correction
            let video_aspect = video_width as f32 / video_height.max(1) as f32;
            let viewport_aspect = viewport_width as f32 / viewport_height.max(1) as f32;

            let (aspect_x, aspect_y) = if video_aspect > viewport_aspect {
                (1.0, viewport_aspect / video_aspect)
            } else {
                (video_aspect / viewport_aspect, 1.0)
            };

            if let Some(loc) = &self.loc_rect_aspect {
                self.gl.uniform_2_f32(Some(loc), aspect_x, aspect_y);
            }

            // Draw fullscreen quad
            self.gl.bind_vertex_array(Some(self.vao));
            self.gl.draw_arrays(glow::TRIANGLES, 0, 6);
            self.gl.bind_vertex_array(None);

            self.gl.bind_texture(glow::TEXTURE_RECTANGLE, None);
            self.gl.use_program(None);
        }
    }

    /// Draw a texture in a circular area centered on screen (for QS Wheel camera background)
    /// Uses stencil buffer to create circular mask
    pub fn draw_texture_circular(
        &self,
        texture: glow::Texture,
        viewport_width: u32,
        viewport_height: u32,
        tex_width: u32,
        tex_height: u32,
        alpha: f32,
        flip_y: bool,
    ) {
        unsafe {
            // Enable stencil test for circular mask
            self.gl.enable(glow::STENCIL_TEST);
            self.gl.clear(glow::STENCIL_BUFFER_BIT);

            // First pass: draw circular mask to stencil buffer
            self.gl.stencil_func(glow::ALWAYS, 1, 0xFF);
            self.gl.stencil_op(glow::KEEP, glow::KEEP, glow::REPLACE);
            self.gl.color_mask(false, false, false, false); // Don't write to color buffer

            // Draw a circle using the simple program with a special shader
            // For simplicity, we'll use multiple triangles to approximate a circle
            // The circle will be at center (0,0) with radius ~0.45 (90% of half-screen)
            self.gl.use_program(Some(self.simple_program));

            // Create circle vertices on the fly (fan from center)
            let num_segments = 64;
            // QS Wheel inner content area - the center circle where video shows
            // Full wheel is 0.38 UV radius, inner content is roughly 60% of that
            // 0.38 * 0.6 = 0.23 UV, * 2 = 0.46 NDC
            let radius = 0.46f32;

            // Aspect ratio correction so circle appears round, not oval
            let aspect = viewport_width as f32 / viewport_height.max(1) as f32;

            let mut circle_verts: Vec<f32> = Vec::with_capacity((num_segments + 2) * 2);

            // Center vertex
            circle_verts.push(0.0);
            circle_verts.push(0.0);

            // Circle edge vertices (aspect-corrected)
            for i in 0..=num_segments {
                let angle = (i as f32 / num_segments as f32) * std::f32::consts::TAU;
                // X is divided by aspect to make circle round on widescreen
                circle_verts.push((radius / aspect) * angle.cos());
                circle_verts.push(radius * angle.sin());
            }

            // Create and upload vertex buffer for circle
            let circle_vbo = self.gl.create_buffer().ok();
            if let Some(vbo) = circle_vbo {
                self.gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                self.gl.buffer_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    bytemuck::cast_slice(&circle_verts),
                    glow::STREAM_DRAW,
                );

                // Set up vertex attributes for simple circle (just position, no texcoord)
                self.gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
                self.gl.enable_vertex_attrib_array(0);

                // Draw triangle fan to stencil
                self.gl.draw_arrays(glow::TRIANGLE_FAN, 0, (num_segments + 2) as i32);

                // Clean up circle buffer
                self.gl.delete_buffer(vbo);
            }

            // Second pass: draw texture only where stencil == 1 (inside circle)
            self.gl.stencil_func(glow::EQUAL, 1, 0xFF);
            self.gl.stencil_op(glow::KEEP, glow::KEEP, glow::KEEP);
            self.gl.color_mask(true, true, true, true); // Enable color writing

            // Now draw the camera texture using the simple program
            self.gl.active_texture(glow::TEXTURE0);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));

            if let Some(loc) = &self.loc_simple_texture {
                self.gl.uniform_1_i32(Some(loc), 0);
            }

            if let Some(loc) = &self.loc_simple_alpha {
                self.gl.uniform_1_f32(Some(loc), alpha);
            }

            if let Some(loc) = &self.loc_simple_scale {
                self.gl.uniform_1_f32(Some(loc), 1.0);
            }

            if let Some(loc) = &self.loc_simple_swizzle {
                self.gl.uniform_1_i32(Some(loc), 0);
            }

            if let Some(loc) = &self.loc_simple_flip_y {
                self.gl.uniform_1_i32(Some(loc), if flip_y { 1 } else { 0 });
            }

            if let Some(loc) = &self.loc_simple_luma_key {
                self.gl.uniform_1_f32(Some(loc), 0.0);
            }

            // Calculate aspect ratio correction for cover mode (fill circle)
            let tex_aspect = tex_width as f32 / tex_height.max(1) as f32;
            let viewport_aspect = viewport_width as f32 / viewport_height.max(1) as f32;

            // For cover mode: scale to fill, crop excess
            let (aspect_x, aspect_y) = if tex_aspect > viewport_aspect {
                // Texture is wider - scale to fill height, crop width
                (viewport_aspect / tex_aspect, 1.0)
            } else {
                // Texture is taller - scale to fill width, crop height
                (1.0, tex_aspect / viewport_aspect)
            };

            if let Some(loc) = &self.loc_simple_aspect {
                self.gl.uniform_2_f32(Some(loc), aspect_x, aspect_y);
            }

            // Draw fullscreen quad (clipped by stencil to circle)
            self.gl.bind_vertex_array(Some(self.vao));
            self.gl.draw_arrays(glow::TRIANGLES, 0, 6);
            self.gl.bind_vertex_array(None);

            // Disable stencil test
            self.gl.disable(glow::STENCIL_TEST);
            self.gl.bind_texture(glow::TEXTURE_2D, None);
            self.gl.use_program(None);
        }
    }

    /// Draw an overlay texture at a specific position
    /// position: NDC coordinates (-1 to 1) for the center of the overlay
    /// size: size in NDC (0 to 2, where 2 = full screen)
    /// alpha: opacity (0-1)
    /// glow: glow intensity (0-1)
    pub fn draw_overlay(
        &self,
        texture: glow::Texture,
        position_x: f32,
        position_y: f32,
        size_x: f32,
        size_y: f32,
        alpha: f32,
        glow: f32,
    ) {
        unsafe {
            self.gl.use_program(Some(self.overlay_program));

            // Bind texture to unit 0
            self.gl.active_texture(glow::TEXTURE0);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            if let Some(loc) = &self.loc_overlay_texture {
                self.gl.uniform_1_i32(Some(loc), 0);
            }

            // Set position (center of overlay in NDC)
            if let Some(loc) = &self.loc_overlay_position {
                self.gl.uniform_2_f32(Some(loc), position_x, position_y);
            }

            // Set size (half-size, since quad goes from -1 to 1)
            if let Some(loc) = &self.loc_overlay_size {
                self.gl.uniform_2_f32(Some(loc), size_x / 2.0, size_y / 2.0);
            }

            // Set alpha
            if let Some(loc) = &self.loc_overlay_alpha {
                self.gl.uniform_1_f32(Some(loc), alpha);
            }

            // Set glow
            if let Some(loc) = &self.loc_overlay_glow {
                self.gl.uniform_1_f32(Some(loc), glow);
            }

            // Draw quad
            self.gl.bind_vertex_array(Some(self.vao));
            self.gl.draw_arrays(glow::TRIANGLES, 0, 6);
            self.gl.bind_vertex_array(None);

            self.gl.bind_texture(glow::TEXTURE_2D, None);
            self.gl.use_program(None);
        }
    }

    /// Clean up resources
    pub fn cleanup(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.program);
            gl.delete_program(self.simple_program);
            gl.delete_program(self.overlay_program);
            gl.delete_program(self.fade_program);
            if let Some(rect_prog) = self.rect_program {
                gl.delete_program(rect_prog);
            }
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
        }
    }
}

/// Bytemuck implementation for slice casting
mod bytemuck {
    pub fn cast_slice<T>(data: &[T]) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                data.as_ptr() as *const u8,
                data.len() * std::mem::size_of::<T>(),
            )
        }
    }
}

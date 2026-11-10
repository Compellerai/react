//! Metal-based LED Wall Renderer - Maximum GPU Performance
//!
//! Features:
//! - GPU compute shader for pixel state updates (zero CPU work)
//! - Instanced rendering (1 draw call for 576 pixels)
//! - IOSurface output for zero-copy sharing with OpenGL
//! - Full Metal pipeline for native macOS performance

use std::ffi::c_void;
use metal::*;
use super::LedPattern;

// IOSurface types and functions for zero-copy GPU sharing
pub type IOSurfaceRef = *mut c_void;
type CFDictionaryRef = *mut c_void;
type CFStringRef = *const c_void;
type CFNumberRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFTypeRef = *const c_void;

// CoreFoundation constants
const K_CF_NUMBER_INT_TYPE: i32 = 9;  // kCFNumberIntType

#[link(name = "IOSurface", kind = "framework")]
extern "C" {
    fn IOSurfaceCreate(properties: CFDictionaryRef) -> IOSurfaceRef;
    fn IOSurfaceGetID(surface: IOSurfaceRef) -> u32;

    // Property keys
    static kIOSurfaceWidth: CFStringRef;
    static kIOSurfaceHeight: CFStringRef;
    static kIOSurfaceBytesPerElement: CFStringRef;
    static kIOSurfacePixelFormat: CFStringRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDictionaryCreate(
        allocator: CFAllocatorRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        num_values: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFDictionaryRef;
    fn CFNumberCreate(
        allocator: CFAllocatorRef,
        the_type: i32,
        value_ptr: *const c_void,
    ) -> CFNumberRef;
    fn CFRelease(cf: *mut c_void);

    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
}

// Objective-C runtime for calling Metal methods not exposed by the crate
type Sel = *const c_void;

#[link(name = "objc", kind = "dylib")]
extern "C" {
    fn sel_registerName(name: *const i8) -> Sel;
}

// Custom objc_msgSend declaration for our specific signature
// newTextureWithDescriptor:iosurface:plane: -> returns id (pointer)
#[link(name = "objc", kind = "dylib")]
extern "C" {
    #[link_name = "objc_msgSend"]
    fn objc_msgSend_iosurface(
        receiver: *mut c_void,
        sel: Sel,
        descriptor: *mut c_void,
        iosurface: IOSurfaceRef,
        plane: u64,
    ) -> *mut c_void;
}

// BGRA pixel format constant ('BGRA' as u32)
const K_CV_PIXEL_FORMAT_TYPE_32_BGRA: u32 = 0x42475241; // 'BGRA'

/// Create an IOSurface with the specified dimensions for GPU sharing
fn create_iosurface(width: u32, height: u32) -> Result<IOSurfaceRef, String> {
    unsafe {
        // Create CFNumber values for width, height, bytes per element, and pixel format
        let width_val = width as i32;
        let height_val = height as i32;
        let bytes_per_element: i32 = 4;
        let pixel_format = K_CV_PIXEL_FORMAT_TYPE_32_BGRA as i32;

        let cf_width = CFNumberCreate(
            std::ptr::null(),
            K_CF_NUMBER_INT_TYPE,
            &width_val as *const i32 as *const c_void,
        );
        let cf_height = CFNumberCreate(
            std::ptr::null(),
            K_CF_NUMBER_INT_TYPE,
            &height_val as *const i32 as *const c_void,
        );
        let cf_bytes = CFNumberCreate(
            std::ptr::null(),
            K_CF_NUMBER_INT_TYPE,
            &bytes_per_element as *const i32 as *const c_void,
        );
        let cf_pixel_format = CFNumberCreate(
            std::ptr::null(),
            K_CF_NUMBER_INT_TYPE,
            &pixel_format as *const i32 as *const c_void,
        );

        // Build the properties dictionary
        let keys: [CFTypeRef; 4] = [
            kIOSurfaceWidth as CFTypeRef,
            kIOSurfaceHeight as CFTypeRef,
            kIOSurfaceBytesPerElement as CFTypeRef,
            kIOSurfacePixelFormat as CFTypeRef,
        ];
        let values: [CFTypeRef; 4] = [
            cf_width as CFTypeRef,
            cf_height as CFTypeRef,
            cf_bytes as CFTypeRef,
            cf_pixel_format as CFTypeRef,
        ];

        let properties = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            4,
            &kCFTypeDictionaryKeyCallBacks as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const c_void,
        );

        // Create the IOSurface
        let surface = IOSurfaceCreate(properties);

        // Clean up CoreFoundation objects
        CFRelease(cf_width as *mut c_void);
        CFRelease(cf_height as *mut c_void);
        CFRelease(cf_bytes as *mut c_void);
        CFRelease(cf_pixel_format as *mut c_void);
        CFRelease(properties as *mut c_void);

        if surface.is_null() {
            return Err("IOSurfaceCreate returned null".to_string());
        }

        let surface_id = IOSurfaceGetID(surface);
        log::info!("[MetalLedWall] Created IOSurface {}x{} (ID: {})", width, height, surface_id);

        Ok(surface)
    }
}

/// Target LED pixel size in screen pixels (determines grid density)
const LED_PIXEL_SIZE: u32 = 60;

/// Maximum grid dimensions (for buffer allocation)
const MAX_GRID_COLS: u32 = 64;
const MAX_GRID_ROWS: u32 = 36;
const MAX_PIXELS: u32 = MAX_GRID_COLS * MAX_GRID_ROWS;

/// Number of frequency bands for spectrum
const NUM_BANDS: u32 = 32;

/// GPU-side pixel state (matches Metal shader struct)
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct GpuPixel {
    brightness: f32,
    target: f32,
    hue: f32,
    saturation: f32,
    phase: f32,
    attack: f32,
    release: f32,
    _padding: f32, // Align to 32 bytes
}

/// Uniforms for compute shader
#[repr(C)]
#[derive(Clone, Copy)]
struct ComputeUniforms {
    spectrum: [f32; 32],
    kick_flash: f32,
    ripple_age: f32,
    time: f32,
    energy: f32,
    strobe: f32,
    strobe_hue: f32,
    strobe_phase: f32,
    master_fade: f32,
    bass_smooth: f32,
    mid_smooth: f32,
    high_smooth: f32,
    pattern: u32,        // 0=Spectrum, 1=Bars, 2=Pulse, 3=Wave, 4=VuMeter, 5=Fire, 6=Rain
    grid_cols: u32,
    grid_rows: u32,
    buffer_stride: u32,  // MAX_GRID_COLS for correct buffer indexing
}

/// Uniforms for render shader
#[repr(C)]
#[derive(Clone, Copy)]
struct RenderUniforms {
    pixel_size: [f32; 2],
    gap: [f32; 2],  // [gap_x, gap_y] for aspect-correct gaps
    strobe: f32,
    strobe_hue: f32,
    master_fade: f32,
    _pad: f32,
    grid_cols: u32,
    grid_rows: u32,
    buffer_stride: u32,  // MAX_GRID_COLS for correct buffer indexing
    _pad2: u32,
}

/// Metal Shading Language source for compute and render
const METAL_SHADERS: &str = r#"
#include <metal_stdlib>
using namespace metal;

// GPU pixel state
struct GpuPixel {
    float brightness;
    float target;
    float hue;
    float saturation;
    float phase;
    float attack;
    float release;
    float _padding;
};

// Pattern enum values (must match Rust LedPattern)
constant uint PATTERN_SPECTRUM = 0;
constant uint PATTERN_BARS = 1;
constant uint PATTERN_PULSE = 2;
constant uint PATTERN_WAVE = 3;
constant uint PATTERN_VU_METER = 4;
constant uint PATTERN_FIRE = 5;
constant uint PATTERN_RAIN = 6;

// Compute uniforms
struct ComputeUniforms {
    float spectrum[32];
    float kick_flash;
    float ripple_age;
    float time;
    float energy;
    float strobe;
    float strobe_hue;
    float strobe_phase;
    float master_fade;
    float bass_smooth;
    float mid_smooth;
    float high_smooth;
    uint pattern;
    uint grid_cols;
    uint grid_rows;
    uint buffer_stride;  // For correct pixel buffer indexing
};

// Render uniforms
struct RenderUniforms {
    float2 pixel_size;
    float2 gap;  // [gap_x, gap_y] for aspect-correct gaps
    float strobe;
    float strobe_hue;
    float master_fade;
    float _pad;
    uint grid_cols;
    uint grid_rows;
    uint buffer_stride;  // For correct pixel buffer indexing
    uint _pad2;
};

// Vertex output
struct VertexOut {
    float4 position [[position]];
    float3 color;
    float2 uv;
    float brightness;
    float glow;
    float strobe;
};

// ============== COMPUTE SHADER ==============
// Updates pixel states in parallel on GPU (dynamic grid size)
// Supports multiple visualization patterns

// Simple hash for pseudo-random numbers
float hash(float n) {
    return fract(sin(n) * 43758.5453123);
}

kernel void update_pixels(
    device GpuPixel* pixels [[buffer(0)]],
    constant ComputeUniforms& uniforms [[buffer(1)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint col = gid.x;
    uint row = gid.y;
    uint grid_cols = uniforms.grid_cols;
    uint grid_rows = uniforms.grid_rows;
    uint buffer_stride = uniforms.buffer_stride;

    if (col >= grid_cols || row >= grid_rows) return;

    // Use buffer_stride (MAX_GRID_COLS) for correct indexing into pre-allocated buffer
    uint idx = row * buffer_stride + col;
    device GpuPixel& pixel = pixels[idx];

    // Common calculations
    float col_f = float(col);
    float row_f = float(row);
    float col_norm = col_f / float(grid_cols - 1);
    float row_norm = row_f / float(grid_rows - 1);
    float center_x = float(grid_cols) / 2.0;
    float center_y = float(grid_rows) / 2.0;

    // Map column to spectrum band
    uint band_idx = min(col * 32 / grid_cols, 31u);
    float band_energy = uniforms.spectrum[band_idx];

    float target = 0.0;
    float hue = 0.0;
    float saturation = 0.9;

    // ============== PATTERN SELECTION ==============
    switch (uniforms.pattern) {
        case PATTERN_SPECTRUM: {
            // Spectrum analyzer: columns = frequency bands, rows light up from bottom
            float scaled_energy = band_energy * 0.4;
            float row_threshold = scaled_energy * float(grid_rows);

            if (row_f < row_threshold) {
                target = 1.0;
            } else if (row_f < row_threshold + 1.0) {
                target = row_threshold - row_f + 1.0;
            } else {
                target = 0.0;
            }

            // Add ripple effect on kick
            if (uniforms.ripple_age < 1.0) {
                float dx = col_f - center_x;
                float dy = row_f - center_y;
                float dist = sqrt(dx * dx + dy * dy);
                float ripple_radius = uniforms.ripple_age * 20.0;
                float ripple_width = 3.0;
                float ripple = (1.0 - min(abs(dist - ripple_radius) / ripple_width, 1.0))
                               * (1.0 - uniforms.ripple_age);
                target = min(target + ripple * 0.5, 1.0);
            }

            hue = col_norm * 0.7;
            break;
        }

        case PATTERN_BARS: {
            // Three horizontal bands: bass (bottom), mid (middle), high (top)
            float zone_height = float(grid_rows) / 3.0;
            uint zone = uint(row_f / zone_height);

            float level;
            if (zone == 0) {
                // Bass zone (bottom) - red
                level = uniforms.bass_smooth;
                hue = 0.0;
            } else if (zone == 1) {
                // Mid zone - green
                level = uniforms.mid_smooth;
                hue = 0.33;
            } else {
                // High zone (top) - blue
                level = uniforms.high_smooth;
                hue = 0.66;
            }

            float zone_row = fmod(row_f, zone_height);
            float bar_height = level * zone_height;
            target = (zone_row < bar_height) ? 1.0 : 0.1;
            break;
        }

        case PATTERN_PULSE: {
            // Expanding ring from center on kicks
            float dx = col_f - center_x;
            float dy = row_f - center_y;
            float dist = sqrt(dx * dx + dy * dy);
            float max_dist = sqrt(center_x * center_x + center_y * center_y);
            float norm_dist = dist / max_dist;

            // Expanding ring on kick
            if (uniforms.ripple_age < 1.5) {
                float ring_pos = uniforms.ripple_age;
                float ring_width = 0.2;
                float ring_dist = abs(norm_dist - ring_pos);
                if (ring_dist < ring_width) {
                    target = (1.0 - ring_dist / ring_width) * (1.0 - uniforms.ripple_age / 1.5);
                }
            }

            // Center glow based on energy
            float center_glow = (1.0 - norm_dist) * uniforms.energy * 0.5;
            target = min(target + center_glow, 1.0);

            hue = fract(norm_dist * 0.3 + uniforms.time * 0.1);
            break;
        }

        case PATTERN_WAVE: {
            // Horizontal sine wave modulated by bass
            float wave_freq = 0.3;
            float wave_amp = uniforms.bass_smooth * (float(grid_rows) / 2.0);
            float wave_center = float(grid_rows) / 2.0;
            float wave_y = wave_center + sin(uniforms.time * 3.0 + col_f * wave_freq) * wave_amp;

            float dist_from_wave = abs(row_f - wave_y);
            float wave_width = 2.0 + uniforms.mid_smooth * 3.0;

            if (dist_from_wave < wave_width) {
                target = 1.0 - dist_from_wave / wave_width;
            } else {
                target = 0.0;
            }

            hue = fract(col_norm * 0.5 + uniforms.time * 0.05);
            break;
        }

        case PATTERN_VU_METER: {
            // Left half = bass meter, right half = high meter
            uint half_cols = grid_cols / 2;

            float level;
            if (col < half_cols) {
                level = uniforms.bass_smooth;
                hue = 0.0; // Red for bass
            } else {
                level = uniforms.high_smooth;
                hue = 0.55; // Cyan for highs
            }

            float meter_height = level * float(grid_rows);
            if (row_f < meter_height) {
                target = 1.0;
            } else if (row_f < meter_height + 1.0) {
                target = 0.5;
            } else {
                target = 0.05;
            }
            break;
        }

        case PATTERN_FIRE: {
            // Fire effect - heat rises from bottom
            // Use noise based on position and time for organic look
            float noise_scale = 0.1;
            float noise1 = hash(col_f * 13.0 + uniforms.time * 2.0);
            float noise2 = hash(col_f * 17.0 + row_f * 7.0 + uniforms.time * 1.5);

            // Heat is stronger at bottom and with bass
            float heat_base = (1.0 - row_norm) * (uniforms.bass_smooth * 0.8 + uniforms.energy * 0.5);
            // Add turbulence
            float turbulence = noise1 * 0.3 + noise2 * 0.2;
            // Flames flicker upward
            float flame_y = row_norm + sin(uniforms.time * 5.0 + col_f * 0.5) * 0.1;
            float flame = max(0.0, 1.0 - flame_y * 1.5) * (0.7 + turbulence);

            target = min(heat_base + flame * uniforms.energy, 1.0);

            // Fire colors: black -> red -> orange -> yellow
            hue = target * 0.1; // Red to orange
            saturation = 1.0 - target * 0.3;
            break;
        }

        case PATTERN_RAIN: {
            // Rain drops falling down
            // Each column has a deterministic "random" drop based on column index
            float drop_speed = 1.0 + uniforms.energy * 2.0;

            // Create multiple drops per column using different seeds
            float brightness = 0.02; // Background
            for (int d = 0; d < 3; d++) {
                float seed = col_f * 7.3 + float(d) * 31.7;
                float drop_offset = hash(seed) * float(grid_rows);
                float drop_rate = 0.3 + hash(seed + 100.0) * 0.5;

                float drop_y = fmod(drop_offset + uniforms.time * drop_rate * drop_speed * 10.0, float(grid_rows));
                float dist = row_f - drop_y;
                float trail_len = 3.0 + uniforms.energy * 2.0;

                if (dist >= 0.0 && dist < trail_len) {
                    float drop_brightness = 1.0 - dist / trail_len;
                    brightness = max(brightness, drop_brightness);
                }
            }

            target = brightness;
            hue = 0.55 + col_norm * 0.15; // Cyan to blue
            break;
        }

        default:
            target = 0.0;
            hue = 0.0;
            break;
    }

    // Add kick flash to all patterns
    target = min(target + uniforms.kick_flash * 0.2, 1.0);

    // Apply strobe at high energy (overrides pattern)
    float strobe_blend = saturate((uniforms.energy - 0.9) / 0.1);
    if (strobe_blend > 0.0) {
        target = mix(target, uniforms.energy, strobe_blend);
    }

    pixel.target = target;

    // Apply per-pixel smoothing (attack/release envelope)
    float rate = (target > pixel.brightness) ? pixel.attack : pixel.release;
    pixel.brightness = mix(pixel.brightness, target, rate);

    // Update color
    float energy_shift = uniforms.energy * 0.1;
    float time_shift = sin(uniforms.time * 0.05 + pixel.phase) * 0.05;
    pixel.hue = fract(hue + energy_shift + time_shift);
    pixel.saturation = saturation;
}

// ============== VERTEX SHADER ==============
// Instanced rendering - one vertex shader invocation per pixel

vertex VertexOut vertex_led(
    uint vertex_id [[vertex_id]],
    uint instance_id [[instance_id]],
    constant GpuPixel* pixels [[buffer(0)]],
    constant RenderUniforms& uniforms [[buffer(1)]]
) {
    // Quad vertices (two triangles)
    float2 positions[6] = {
        float2(0, 0), float2(1, 0), float2(1, 1),
        float2(0, 0), float2(1, 1), float2(0, 1)
    };

    float2 local_pos = positions[vertex_id];

    // Calculate grid position from instance ID (dynamic grid size)
    uint grid_cols = uniforms.grid_cols;
    uint grid_rows = uniforms.grid_rows;
    uint buffer_stride = uniforms.buffer_stride;
    uint col = instance_id % grid_cols;
    uint row = instance_id / grid_cols;

    // Skip pixels outside grid (buffer is allocated for max size)
    if (col >= grid_cols || row >= grid_rows) {
        VertexOut out;
        out.position = float4(0, 0, -2, 1); // Behind clip plane
        return out;
    }

    // Pixel position with aspect-corrected gaps
    float gap_x = uniforms.gap.x;
    float gap_y = uniforms.gap.y;
    float pixel_w = uniforms.pixel_size.x;
    float pixel_h = uniforms.pixel_size.y;

    float x = gap_x + float(col) * (pixel_w + gap_x);
    // Flip Y for Metal->OpenGL IOSurface coordinate system difference
    // This makes row 0 appear at visual BOTTOM when displayed via OpenGL
    float y = gap_y + float(grid_rows - 1 - row) * (pixel_h + gap_y);

    // Transform to clip space (-1 to 1)
    float2 pos = local_pos * uniforms.pixel_size + float2(x, y);
    pos = pos * 2.0 - 1.0;

    // Get pixel data using buffer_stride for correct indexing
    uint buffer_idx = row * buffer_stride + col;
    constant GpuPixel& pixel = pixels[buffer_idx];

    // HSV to RGB conversion
    float h = fract(pixel.hue + 1.0) * 6.0;
    float s = pixel.saturation * (1.0 - uniforms.strobe * 0.6);
    float v = 1.0;

    int i = int(floor(h));
    float f = h - float(i);
    float p = v * (1.0 - s);
    float q = v * (1.0 - s * f);
    float t = v * (1.0 - s * (1.0 - f));

    float3 rgb;
    switch (i % 6) {
        case 0: rgb = float3(v, t, p); break;
        case 1: rgb = float3(q, v, p); break;
        case 2: rgb = float3(p, v, t); break;
        case 3: rgb = float3(p, q, v); break;
        case 4: rgb = float3(t, p, v); break;
        default: rgb = float3(v, p, q); break;
    }

    // Strobe color override - when strobe > 0.5, all pixels use unified strobe color
    if (uniforms.strobe > 0.5) {
        // Use strobe_hue (set on each kick) for unified color flash
        float strobe_h = uniforms.strobe_hue * 0.7;  // Same as original
        h = strobe_h * 6.0;
        i = int(floor(h));
        f = h - float(i);
        float p_val = 1.0 * (1.0 - s);
        float q_val = 1.0 * (1.0 - s * f);
        float t_val = 1.0 * (1.0 - s * (1.0 - f));
        switch (i % 6) {
            case 0: rgb = float3(1, t_val, p_val); break;
            case 1: rgb = float3(q_val, 1, p_val); break;
            case 2: rgb = float3(p_val, 1, t_val); break;
            case 3: rgb = float3(p_val, q_val, 1); break;
            case 4: rgb = float3(t_val, p_val, 1); break;
            default: rgb = float3(1, p_val, q_val); break;
        }
    }

    float base_brightness = pixel.brightness + uniforms.strobe * (1.0 - pixel.brightness);
    float brightness = base_brightness * uniforms.master_fade;
    float glow = (base_brightness * base_brightness * 0.8 + uniforms.strobe * 0.5) * uniforms.master_fade;

    VertexOut out;
    // Transform from 0-1 to clip space -1 to 1 (fills full screen)
    float2 clip_pos = pos * 2.0 - 1.0;
    out.position = float4(clip_pos, 0.0, 1.0);
    out.color = rgb;
    out.uv = local_pos;
    out.brightness = brightness;
    out.glow = glow;
    out.strobe = uniforms.strobe;
    return out;
}

// ============== FRAGMENT SHADER ==============
// Realistic LED pixel with glow effect - matches original OpenGL shader

fragment float4 fragment_led(VertexOut in [[stage_in]]) {
    // Distance from center
    float2 center = float2(0.5);
    float2 d = in.uv - center;
    float dist = length(d);

    // Pixel size expands during strobe (fills gaps)
    float pixel_size = mix(0.48, 0.52, in.strobe);
    float corner = mix(0.04, 0.01, in.strobe);

    // Rounded rectangle mask (LED pixel shape)
    float2 size = float2(pixel_size);
    float2 q = abs(d) - size;
    float rect_dist = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - corner;

    // Edge softness decreases during strobe (sharper)
    float edge_soft = mix(0.02, 0.01, in.strobe);
    float pixel_mask = 1.0 - smoothstep(-edge_soft, edge_soft, rect_dist);

    // Hot spot in center (brighter center like real LEDs)
    float hotspot = exp(-dist * dist * 8.0);

    // Strobe boost - everything gets brighter (KEY: original has 1.0 + strobe * 2.0)
    float strobe_boost = 1.0 + in.strobe * 2.0;

    // Outer glow (bloom effect) - stronger during strobe
    float glow_strength = in.glow + in.strobe * 1.5;
    float glow = exp(-dist * dist * 2.0) * glow_strength;

    // During strobe, push color towards white
    float3 strobe_color = mix(in.color, float3(1.0), in.strobe * 0.5);

    // Combine: base color + hotspot boost + glow
    float intensity = in.brightness * (0.7 + hotspot * 0.5) * strobe_boost;
    float3 color = strobe_color * intensity * pixel_mask;

    // Add glow outside the pixel (much stronger during strobe)
    color += strobe_color * glow * in.brightness;

    // Minimum dim state (LEDs are never fully off)
    float dim = 0.02;
    color = max(color, in.color * dim * pixel_mask);

    // Clamp to HDR-ish range during strobe (allow overbright)
    color = min(color, float3(3.0));

    return float4(color, 1.0);
}
"#;

/// Metal-based LED Wall renderer
pub struct MetalLedWall {
    device: Device,
    command_queue: CommandQueue,

    // Compute pipeline for pixel state updates
    compute_pipeline: ComputePipelineState,

    // Render pipeline for drawing
    render_pipeline: RenderPipelineState,

    // GPU buffers
    pixel_buffer: Buffer,
    compute_uniforms_buffer: Buffer,
    render_uniforms_buffer: Buffer,

    // Output texture backed by IOSurface for zero-copy OpenGL sharing
    output_texture: Texture,

    // IOSurface for zero-copy GPU sharing with OpenGL
    iosurface: IOSurfaceRef,

    // State
    spectrum: [f32; NUM_BANDS as usize],
    time: f32,
    kick_flash: f32,
    ripple_age: f32,
    energy: f32,
    strobe: f32,
    strobe_hue: f32,
    strobe_phase: f32,
    master_fade: f32,
    bass_smooth: f32,
    mid_smooth: f32,
    high_smooth: f32,

    // Output dimensions
    width: u32,
    height: u32,

    // Dynamic grid dimensions (calculated from screen size)
    grid_cols: u32,
    grid_rows: u32,

    initialized: bool,

    // Current pattern (Metal only supports Spectrum Analyzer currently)
    current_pattern: LedPattern,
}


impl MetalLedWall {
    /// Create a new Metal-based LED Wall renderer
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        // Get default Metal device
        let device = Device::system_default()
            .ok_or("No Metal device available")?;

        log::info!("[MetalLedWall] Using device: {}", device.name());

        // Create command queue
        let command_queue = device.new_command_queue();

        // Compile shaders
        let library = device
            .new_library_with_source(METAL_SHADERS, &CompileOptions::new())
            .map_err(|e| format!("Failed to compile Metal shaders: {}", e))?;

        // Create compute pipeline
        let compute_function = library
            .get_function("update_pixels", None)
            .map_err(|e| format!("Failed to get compute function: {}", e))?;

        let compute_pipeline = device
            .new_compute_pipeline_state_with_function(&compute_function)
            .map_err(|e| format!("Failed to create compute pipeline: {}", e))?;

        // Create render pipeline
        let vertex_function = library
            .get_function("vertex_led", None)
            .map_err(|e| format!("Failed to get vertex function: {}", e))?;

        let fragment_function = library
            .get_function("fragment_led", None)
            .map_err(|e| format!("Failed to get fragment function: {}", e))?;

        let render_desc = RenderPipelineDescriptor::new();
        render_desc.set_vertex_function(Some(&vertex_function));
        render_desc.set_fragment_function(Some(&fragment_function));

        // Set up color attachment for BGRA output (compatible with OpenGL)
        let color_attachment = render_desc.color_attachments().object_at(0).unwrap();
        color_attachment.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        color_attachment.set_blending_enabled(true);
        color_attachment.set_source_rgb_blend_factor(MTLBlendFactor::SourceAlpha);
        color_attachment.set_destination_rgb_blend_factor(MTLBlendFactor::One);
        color_attachment.set_source_alpha_blend_factor(MTLBlendFactor::One);
        color_attachment.set_destination_alpha_blend_factor(MTLBlendFactor::One);

        let render_pipeline = device
            .new_render_pipeline_state(&render_desc)
            .map_err(|e| format!("Failed to create render pipeline: {}", e))?;

        // Create GPU buffers
        let pixel_buffer = device.new_buffer(
            // Allocate for MAX_PIXELS to allow dynamic grid sizing
            (MAX_PIXELS as usize * std::mem::size_of::<GpuPixel>()) as u64,
            MTLResourceOptions::StorageModeShared,
        );

        let compute_uniforms_buffer = device.new_buffer(
            std::mem::size_of::<ComputeUniforms>() as u64,
            MTLResourceOptions::StorageModeShared,
        );

        let render_uniforms_buffer = device.new_buffer(
            std::mem::size_of::<RenderUniforms>() as u64,
            MTLResourceOptions::StorageModeShared,
        );

        // Calculate grid dimensions from screen size
        let grid_cols = (width / LED_PIXEL_SIZE).max(8).min(MAX_GRID_COLS);
        let grid_rows = (height / LED_PIXEL_SIZE).max(4).min(MAX_GRID_ROWS);

        // Initialize pixel data for max grid (will use subset based on current grid size)
        {
            let pixels = unsafe {
                std::slice::from_raw_parts_mut(
                    pixel_buffer.contents() as *mut GpuPixel,
                    MAX_PIXELS as usize,
                )
            };

            for row in 0..MAX_GRID_ROWS {
                for col in 0..MAX_GRID_COLS {
                    let idx = (row * MAX_GRID_COLS + col) as usize;
                    let col_norm = col as f32 / (MAX_GRID_COLS - 1) as f32;

                    pixels[idx] = GpuPixel {
                        brightness: 0.0,
                        target: 0.0,
                        hue: col_norm * 0.7,
                        saturation: 0.9,
                        phase: ((col * 7 + row * 13) % 100) as f32 / 100.0,
                        attack: 0.4 + ((col + row * 3) % 10) as f32 / 100.0,
                        release: 0.06 + ((col + row * 3) % 10) as f32 / 200.0,
                        _padding: 0.0,
                    };
                }
            }
        }

        // Create IOSurface-backed output texture for zero-copy OpenGL sharing
        let (output_texture, iosurface) = Self::create_iosurface_texture(&device, width, height)?;

        log::info!("[MetalLedWall] Initialized: {}x{}, grid {}x{}, GPU compute + instanced + IOSurface",
            width, height, grid_cols, grid_rows);

        Ok(Self {
            device,
            command_queue,
            compute_pipeline,
            render_pipeline,
            pixel_buffer,
            compute_uniforms_buffer,
            render_uniforms_buffer,
            output_texture,
            iosurface,
            spectrum: [0.0; NUM_BANDS as usize],
            time: 0.0,
            kick_flash: 0.0,
            ripple_age: 1.0,
            energy: 0.0,
            strobe: 0.0,
            strobe_hue: 0.0,
            strobe_phase: 0.0,
            master_fade: 1.0,
            bass_smooth: 0.0,
            mid_smooth: 0.0,
            high_smooth: 0.0,
            width,
            height,
            grid_cols,
            grid_rows,
            initialized: true,
            current_pattern: LedPattern::Spectrum,
        })
    }

    /// Create a Metal texture backed by IOSurface for zero-copy GPU sharing
    fn create_iosurface_texture(device: &Device, width: u32, height: u32) -> Result<(Texture, IOSurfaceRef), String> {
        // Create the IOSurface
        let iosurface = create_iosurface(width, height)?;

        // Create texture descriptor
        let texture_desc = TextureDescriptor::new();
        texture_desc.set_texture_type(MTLTextureType::D2);
        texture_desc.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        texture_desc.set_width(width as u64);
        texture_desc.set_height(height as u64);
        texture_desc.set_usage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        // IOSurface textures need Shared storage for cross-framework sharing
        texture_desc.set_storage_mode(MTLStorageMode::Shared);

        // Create texture from IOSurface using raw Objective-C runtime
        // This avoids objc2 version conflicts with the metal crate
        let texture: Texture = unsafe {
            use metal::foreign_types::ForeignType;

            // Get selector for "newTextureWithDescriptor:iosurface:plane:"
            let sel = sel_registerName(
                b"newTextureWithDescriptor:iosurface:plane:\0".as_ptr() as *const i8
            );

            // Get raw pointers
            let device_ptr = device.as_ptr();
            let desc_ptr = texture_desc.as_ptr();

            // Call the method using objc_msgSend
            let raw_texture: *mut c_void = objc_msgSend_iosurface(
                device_ptr as *mut c_void,
                sel,
                desc_ptr as *mut c_void,
                iosurface,
                0u64,
            );

            if raw_texture.is_null() {
                CFRelease(iosurface as *mut c_void);
                return Err("newTextureWithDescriptor:iosurface:plane: returned null".to_string());
            }

            // Convert raw pointer to Texture
            Texture::from_ptr(raw_texture as *mut _)
        };

        log::info!("[MetalLedWall] Created IOSurface-backed Metal texture {}x{}", width, height);

        Ok((texture, iosurface))
    }

    /// Update state from audio analysis
    pub fn update_state(
        &mut self,
        bass: f32,
        mid: f32,
        high: f32,
        kick: bool,
        dt: f32,
    ) {
        if !self.initialized {
            return;
        }

        self.time += dt;

        // Smooth bass/mid/high for patterns
        self.bass_smooth += (bass - self.bass_smooth) * 0.3;
        self.mid_smooth += (mid - self.mid_smooth) * 0.3;
        self.high_smooth += (high - self.high_smooth) * 0.3;

        // Synthesize spectrum from bass/mid/high
        self.synthesize_spectrum(bass, mid, high);

        // Calculate energy (weighted towards bass for impact, matches original)
        let raw_energy = bass * 0.5 + mid * 0.3 + high * 0.2;

        // Smooth energy with fast attack, medium release (matches original)
        if raw_energy > self.energy {
            self.energy += (raw_energy - self.energy) * 0.6;  // Fast attack
        } else {
            self.energy += (raw_energy - self.energy) * 0.15;  // Medium release
        }

        // Strobe activation (matches original thresholds and smoothing)
        let strobe_threshold = 0.65;  // Original uses 0.65
        let target_strobe = if self.energy > strobe_threshold {
            ((self.energy - strobe_threshold) / (1.0 - strobe_threshold)).powf(0.5)
        } else {
            0.0
        };
        // Fast attack, fast decay for punchy strobe (matches original)
        if target_strobe > self.strobe {
            self.strobe += (target_strobe - self.strobe) * 0.7;  // Fast attack
        } else {
            self.strobe *= 0.75;  // Fast multiplicative decay for punchy feel
        }

        // Strobe flicker phase - speed increases with energy: 15-30+ Hz flicker rate
        let flicker_speed = 15.0 + self.energy * 20.0;  // Use energy like original
        self.strobe_phase += dt * flicker_speed * std::f32::consts::TAU;
        if self.strobe_phase > std::f32::consts::TAU {
            self.strobe_phase -= std::f32::consts::TAU;
        }

        // Handle kick - instant strobe on kick with very high energy only
        if kick && self.energy > 0.8 {
            self.strobe = 1.0;  // Instant max strobe on strong kick
            self.strobe_hue = self.time.fract();  // Random-ish color on each kick
        }

        if kick {
            self.kick_flash = 1.0;
            self.ripple_age = 0.0;
        }

        // Decay kick flash
        self.kick_flash *= 0.85;

        // Age ripple
        self.ripple_age += dt * 2.0;

        // Silence detection - fade to black when no audio (matches original)
        const SILENCE_THRESHOLD: f32 = 0.02;
        let has_audio = raw_energy > SILENCE_THRESHOLD;

        if has_audio {
            // Audio present - fade in quickly
            self.master_fade = (self.master_fade + dt * 3.0).min(1.0);
        } else {
            // No audio - fade out
            self.master_fade = (self.master_fade - dt * 0.5).max(0.0);
        }
    }

    /// Synthesize 32-band spectrum from bass/mid/high
    fn synthesize_spectrum(&mut self, bass: f32, mid: f32, high: f32) {
        for i in 0..NUM_BANDS as usize {
            let pos = i as f32 / (NUM_BANDS - 1) as f32;

            let value = if pos < 0.3 {
                let t = pos / 0.3;
                bass * (1.0 - t * 0.5)
            } else if pos < 0.7 {
                let t = (pos - 0.3) / 0.4;
                bass * 0.5 * (1.0 - t) + mid * t
            } else {
                let t = (pos - 0.7) / 0.3;
                mid * (1.0 - t) + high * t
            };

            // Smooth
            self.spectrum[i] = self.spectrum[i] * 0.7 + value * 0.3;
        }
    }

    /// Render the LED wall using Metal compute + instanced draw
    pub fn render(&mut self) {
        if !self.initialized || self.master_fade < 0.01 {
            return;
        }

        // Update compute uniforms with dynamic grid dimensions
        {
            let uniforms = ComputeUniforms {
                spectrum: self.spectrum,
                kick_flash: self.kick_flash,
                ripple_age: self.ripple_age,
                time: self.time,
                energy: self.energy,
                strobe: self.strobe,
                strobe_hue: self.strobe_hue,
                strobe_phase: self.strobe_phase,
                master_fade: self.master_fade,
                bass_smooth: self.bass_smooth,
                mid_smooth: self.mid_smooth,
                high_smooth: self.high_smooth,
                pattern: self.current_pattern as u32,
                grid_cols: self.grid_cols,
                grid_rows: self.grid_rows,
                buffer_stride: MAX_GRID_COLS,
            };

            unsafe {
                let ptr = self.compute_uniforms_buffer.contents() as *mut ComputeUniforms;
                *ptr = uniforms;
            }
        }

        // Update render uniforms with dynamic grid dimensions
        {
            // Use aspect-corrected gaps so they appear equal visually
            let aspect = self.width as f32 / self.height as f32;
            let base_gap = 0.003;
            let gap_x = base_gap / aspect;  // Narrower gap horizontally on widescreen
            let gap_y = base_gap;
            let pixel_w = (1.0 - gap_x * (self.grid_cols + 1) as f32) / self.grid_cols as f32;
            let pixel_h = (1.0 - gap_y * (self.grid_rows + 1) as f32) / self.grid_rows as f32;

            // Strobe flicker
            let flicker = if self.strobe > 0.3 {
                let wave = self.strobe_phase.sin();
                let flicker_amount = (self.strobe - 0.3) / 0.7;
                if wave > 0.0 { 1.0 } else { 1.0 - flicker_amount * 0.9 }
            } else {
                1.0
            };

            let uniforms = RenderUniforms {
                pixel_size: [pixel_w, pixel_h],
                gap: [gap_x, gap_y],
                strobe: self.strobe * flicker,
                strobe_hue: self.strobe_hue,
                master_fade: self.master_fade,
                _pad: 0.0,
                grid_cols: self.grid_cols,
                grid_rows: self.grid_rows,
                buffer_stride: MAX_GRID_COLS,
                _pad2: 0,
            };

            unsafe {
                let ptr = self.render_uniforms_buffer.contents() as *mut RenderUniforms;
                *ptr = uniforms;
            }
        }

        // Create command buffer
        let command_buffer = self.command_queue.new_command_buffer();

        // === COMPUTE PASS: Update pixel states ===
        {
            let compute_encoder = command_buffer.new_compute_command_encoder();
            compute_encoder.set_compute_pipeline_state(&self.compute_pipeline);
            compute_encoder.set_buffer(0, Some(&self.pixel_buffer), 0);
            compute_encoder.set_buffer(1, Some(&self.compute_uniforms_buffer), 0);

            // Dispatch threads for dynamic grid size
            // Use 16x16 threadgroups (256 threads) - well within Metal limits
            let threads_per_group_x: u32 = 16;
            let threads_per_group_y: u32 = 16;
            let num_groups_x = (self.grid_cols + threads_per_group_x - 1) / threads_per_group_x;
            let num_groups_y = (self.grid_rows + threads_per_group_y - 1) / threads_per_group_y;
            let thread_groups = MTLSize::new(num_groups_x as u64, num_groups_y as u64, 1);
            let threads_per_group = MTLSize::new(threads_per_group_x as u64, threads_per_group_y as u64, 1);
            compute_encoder.dispatch_thread_groups(thread_groups, threads_per_group);
            compute_encoder.end_encoding();
        }

        // === RENDER PASS: Draw all pixels with instancing ===
        {
            let render_pass_desc = RenderPassDescriptor::new();
            let color_attachment = render_pass_desc.color_attachments().object_at(0).unwrap();
            color_attachment.set_texture(Some(&self.output_texture));
            color_attachment.set_load_action(MTLLoadAction::Clear);
            color_attachment.set_clear_color(MTLClearColor::new(0.0, 0.0, 0.0, 1.0));
            color_attachment.set_store_action(MTLStoreAction::Store);

            let render_encoder = command_buffer.new_render_command_encoder(&render_pass_desc);
            render_encoder.set_render_pipeline_state(&self.render_pipeline);
            render_encoder.set_vertex_buffer(0, Some(&self.pixel_buffer), 0);
            render_encoder.set_vertex_buffer(1, Some(&self.render_uniforms_buffer), 0);

            // Draw instances for current grid (shader culls pixels outside bounds)
            let total_pixels = self.grid_cols * self.grid_rows;
            render_encoder.draw_primitives_instanced(
                MTLPrimitiveType::Triangle,
                0,
                6,
                total_pixels as u64,
            );
            render_encoder.end_encoding();
        }

        // Commit and wait
        command_buffer.commit();
        command_buffer.wait_until_completed();
    }

    /// Get the IOSurface for zero-copy sharing with OpenGL
    /// The returned IOSurfaceRef can be used with CGLTexImageIOSurface2D
    pub fn iosurface(&self) -> IOSurfaceRef {
        self.iosurface
    }

    /// Get output dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Resize output texture and recalculate grid dimensions
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }

        // Calculate new grid dimensions based on screen size
        let new_grid_cols = (width / LED_PIXEL_SIZE).max(8).min(MAX_GRID_COLS);
        let new_grid_rows = (height / LED_PIXEL_SIZE).max(4).min(MAX_GRID_ROWS);

        // Release old IOSurface
        if !self.iosurface.is_null() {
            unsafe {
                CFRelease(self.iosurface as *mut c_void);
            }
        }

        // Create new IOSurface-backed texture
        match Self::create_iosurface_texture(&self.device, width, height) {
            Ok((texture, iosurface)) => {
                self.output_texture = texture;
                self.iosurface = iosurface;
                self.width = width;
                self.height = height;
                self.grid_cols = new_grid_cols;
                self.grid_rows = new_grid_rows;
                log::info!("[MetalLedWall] Resized to {}x{}, grid {}x{}",
                    width, height, new_grid_cols, new_grid_rows);
            }
            Err(e) => {
                log::error!("[MetalLedWall] Failed to resize: {}", e);
            }
        }
    }

    /// Get current pattern
    pub fn current_pattern(&self) -> LedPattern {
        self.current_pattern
    }

    /// Set pattern - all patterns are now supported in Metal compute shader
    pub fn set_pattern(&mut self, pattern: LedPattern) {
        if self.current_pattern != pattern {
            self.current_pattern = pattern;
            log::info!("[MetalLedWall] Set pattern to {:?}", pattern);
        }
    }
}

impl Drop for MetalLedWall {
    fn drop(&mut self) {
        // Release IOSurface
        if !self.iosurface.is_null() {
            unsafe {
                CFRelease(self.iosurface as *mut c_void);
            }
        }
        log::info!("[MetalLedWall] Cleaned up (IOSurface released)");
    }
}

// Ensure thread safety for the Metal resources
unsafe impl Send for MetalLedWall {}

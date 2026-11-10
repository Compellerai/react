//! Floating logo particles for landing page
//! Creates a 4D-space effect with depth parallax and rotation

use egui::{Color32, Pos2, Rect, Vec2};

/// A single floating logo particle
#[derive(Clone)]
pub struct FloatingLogo {
    /// Position (0-1 normalized screen coordinates)
    pub x: f32,
    pub y: f32,
    /// Depth (0 = far/small, 1 = close/large)
    pub depth: f32,
    /// Velocity
    pub vx: f32,
    pub vy: f32,
    /// Rotation angle (radians)
    pub rotation: f32,
    /// Rotation speed
    pub rotation_speed: f32,
    /// 4D angle (for extra dimensional wobble)
    pub w_angle: f32,
    /// 4D rotation speed
    pub w_speed: f32,
    /// Base alpha (affected by depth)
    pub alpha: f32,
}

/// Manages floating logo particles
pub struct FloatingLogos {
    logos: Vec<FloatingLogo>,
    time: f32,
}

impl FloatingLogos {
    pub fn new(count: usize) -> Self {
        let mut logos = Vec::with_capacity(count);
        let mut seed = 12345u64;

        // Simple LCG random helper
        let mut rand = || -> f32 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as f32) / (u32::MAX as f32 / 2.0)
        };

        for i in 0..count {
            // Distribute depth evenly with some randomness
            let base_depth = i as f32 / count as f32;
            let depth = (base_depth + rand() * 0.2 - 0.1).clamp(0.05, 0.95);

            // Size/alpha based on depth (closer = bigger and more visible)
            let alpha = 0.15 + depth * 0.5;

            logos.push(FloatingLogo {
                x: rand(),
                y: rand(),
                depth,
                vx: (rand() - 0.5) * 0.0008 * (0.5 + depth), // Faster when closer (parallax)
                vy: (rand() - 0.5) * 0.0006 * (0.5 + depth),
                rotation: rand() * std::f32::consts::TAU,
                rotation_speed: (rand() - 0.5) * 0.02,
                w_angle: rand() * std::f32::consts::TAU,
                w_speed: 0.008 + rand() * 0.015,
                alpha,
            });
        }

        // Sort by depth so far ones render first (painter's algorithm)
        logos.sort_by(|a, b| a.depth.partial_cmp(&b.depth).unwrap());

        Self { logos, time: 0.0 }
    }

    /// Update all particles (call each frame)
    pub fn update(&mut self, dt: f32) {
        self.time += dt;

        for logo in &mut self.logos {
            // Update position
            logo.x += logo.vx;
            logo.y += logo.vy;

            // Wrap around edges with buffer
            if logo.x < -0.15 { logo.x = 1.15; }
            if logo.x > 1.15 { logo.x = -0.15; }
            if logo.y < -0.15 { logo.y = 1.15; }
            if logo.y > 1.15 { logo.y = -0.15; }

            // Update rotation
            logo.rotation += logo.rotation_speed;

            // Update 4D angle
            logo.w_angle += logo.w_speed;
        }
    }

    /// Render all floating logos
    pub fn render(&self, painter: &egui::Painter, screen: Rect, texture: Option<&egui::TextureHandle>) {
        let screen_size = screen.size();

        for logo in &self.logos {
            // 4D projection effect: wobble position and scale based on w_angle
            let w_factor = logo.w_angle.sin();
            let w_factor2 = (logo.w_angle * 1.3).cos();

            // Apply 4D wobble to position
            let wobble_x = w_factor * 0.02 * logo.depth;
            let wobble_y = w_factor2 * 0.015 * logo.depth;

            let screen_x = screen.min.x + (logo.x + wobble_x) * screen_size.x;
            let screen_y = screen.min.y + (logo.y + wobble_y) * screen_size.y;

            // Size based on depth (15-80 pixels) with 4D scale wobble
            let base_size = 15.0 + logo.depth * 65.0;
            let w_scale = 1.0 + w_factor * 0.15; // ±15% scale wobble
            let size = base_size * w_scale;

            // Alpha with depth and 4D pulsing
            let w_alpha = 1.0 + w_factor2 * 0.2;
            let alpha = (logo.alpha * w_alpha * 255.0).clamp(0.0, 255.0) as u8;

            // Color: purple/blue tint, brighter when closer
            let brightness = (150.0 + logo.depth * 80.0) as u8;
            let color = Color32::from_rgba_unmultiplied(
                brightness,
                (brightness as f32 * 0.85) as u8,
                255,
                alpha,
            );

            let center = Pos2::new(screen_x, screen_y);

            if let Some(tex) = texture {
                // Render textured logo with rotation
                // Original logo is 100x63, aspect ratio ~1.587:1 (wider than tall)
                let aspect_ratio = 100.0 / 63.0;
                let half_width = size / 2.0;
                let half_height = half_width / aspect_ratio;

                // Calculate rotated corners
                let cos_r = logo.rotation.cos();
                let sin_r = logo.rotation.sin();

                let corners = [
                    Vec2::new(-half_width, -half_height),
                    Vec2::new(half_width, -half_height),
                    Vec2::new(half_width, half_height),
                    Vec2::new(-half_width, half_height),
                ];

                let rotated: Vec<Pos2> = corners
                    .iter()
                    .map(|c| {
                        Pos2::new(
                            center.x + c.x * cos_r - c.y * sin_r,
                            center.y + c.x * sin_r + c.y * cos_r,
                        )
                    })
                    .collect();

                // Draw as textured quad with tint
                let uv = [
                    Pos2::new(0.0, 0.0),
                    Pos2::new(1.0, 0.0),
                    Pos2::new(1.0, 1.0),
                    Pos2::new(0.0, 1.0),
                ];

                let mesh = egui::Mesh {
                    indices: vec![0, 1, 2, 0, 2, 3],
                    vertices: vec![
                        egui::epaint::Vertex { pos: rotated[0], uv: uv[0], color },
                        egui::epaint::Vertex { pos: rotated[1], uv: uv[1], color },
                        egui::epaint::Vertex { pos: rotated[2], uv: uv[2], color },
                        egui::epaint::Vertex { pos: rotated[3], uv: uv[3], color },
                    ],
                    texture_id: tex.id(),
                };

                painter.add(egui::Shape::mesh(mesh));
            } else {
                // Fallback: render as rotating diamond/rhombus shape
                let half = size / 2.0;
                let cos_r = logo.rotation.cos();
                let sin_r = logo.rotation.sin();

                // Diamond points (rotated)
                let points: Vec<Pos2> = [
                    Vec2::new(0.0, -half),      // Top
                    Vec2::new(half * 0.6, 0.0), // Right
                    Vec2::new(0.0, half),       // Bottom
                    Vec2::new(-half * 0.6, 0.0), // Left
                ]
                .iter()
                .map(|p| {
                    Pos2::new(
                        center.x + p.x * cos_r - p.y * sin_r,
                        center.y + p.x * sin_r + p.y * cos_r,
                    )
                })
                .collect();

                painter.add(egui::Shape::convex_polygon(
                    points,
                    color,
                    egui::Stroke::NONE,
                ));
            }
        }
    }

    /// Get number of logos
    pub fn count(&self) -> usize {
        self.logos.len()
    }
}

impl Default for FloatingLogos {
    fn default() -> Self {
        Self::new(42) // ~42 floating logos
    }
}

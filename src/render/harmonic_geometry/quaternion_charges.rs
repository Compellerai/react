//! QuaternionCharges - REG Quaternion Symmetry Visualization
//!
//! Implements the quaternion charge system for harmonic geometry:
//! - Reduces angle A to w < 90° (REG rule)
//! - Computes 4 quaternion ports: {w, π-w, π+w, τ-w}
//! - Assigns charge polarities based on axis proximity
//! - Verifies invariant relationships (complements, palindrome)
//!
//! Active during fold24 transition (0.10 < t < 0.90) when inside geometry.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

// ============================================================================
// CHARGE ENUM
// ============================================================================

/// Charge polarity enum (REG axes)
/// - +1 at East (0°)
/// - +i at North (90°)
/// - -1 at West (180°)
/// - -i at South (270°)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Charge {
    #[default]
    Plus1,  // near 0° (East)
    PlusI,  // near 90° (North)
    Minus1, // near 180° (West)
    MinusI, // near 270° (South)
}

impl Charge {
    /// Convert to integer for shader uniform
    pub fn as_i32(&self) -> i32 {
        match self {
            Charge::Plus1 => 0,
            Charge::PlusI => 1,
            Charge::Minus1 => 2,
            Charge::MinusI => 3,
        }
    }

    /// Human-readable glyph for HUD
    pub fn glyph(&self) -> &'static str {
        match self {
            Charge::Plus1 => "+1",
            Charge::PlusI => "+i",
            Charge::Minus1 => "-1",
            Charge::MinusI => "-i",
        }
    }
}

// ============================================================================
// INVARIANTS
// ============================================================================

/// Invariant check results for quaternion ports
#[derive(Clone, Debug, Default)]
pub struct Invariants {
    /// p0 + p3 (should = TAU)
    pub pair1_sum: f32,
    /// p1 + p2 (should = TAU)
    pub pair2_sum: f32,
    /// Sum of all 4 ports (should = 2*TAU)
    pub total_sum: f32,
    /// Both complement pairs sum to TAU
    pub complements_ok: bool,
    /// Total sum equals 2*TAU
    pub total_ok: bool,
    /// Each port has a mirror at TAU - port
    pub palindrome_ok: bool,
    /// Each port within epsilon of 15° grid
    pub lattice_ok_24: bool,
}

// ============================================================================
// QUATERNION CHARGES STATE
// ============================================================================

/// QuaternionCharges state - active during fold24 transition
#[derive(Clone, Debug)]
pub struct QuaternionChargesState {
    /// Is QC visualization active?
    pub active: bool,
    /// Reduced angle w in [0, π/2)
    pub w: f32,
    /// 4 quaternion ports (radians, 0..TAU)
    pub ports: [f32; 4],
    /// Charge assignment per port
    pub charges: [Charge; 4],
    /// Current active port index (0-3), nearest to camera with hysteresis
    pub active_port: usize,
    /// Overlay strength from fold24_t (0-1), controls visibility intensity
    pub strength: f32,
    /// Invariant check results
    pub invariants: Invariants,

    // === POLARITY TRACKING (quadrant-based) ===
    /// Current quadrant (0=E/+1, 1=N/+i, 2=W/-1, 3=S/-i)
    pub current_quadrant: u8,
    /// Previous quadrant (before last change)
    pub prev_quadrant: u8,
    /// True for one frame when polarity changes
    pub polarity_changed: bool,
    /// Direction of polarity change: +1=CW, -1=CCW, 0=none
    pub polarity_direction: i8,
    /// Hysteresis timer: time spent in candidate quadrant before committing
    pub quadrant_hold_timer: f32,

    // === POLARITY EFFECT (visual feedback) ===
    /// Effect timer: 1.0 at trigger, counts down to 0 over ~1.5s total
    /// Phase 0.0-1.0: hold (1s), Phase 1.0-1.5: flash+smoke (0.5s)
    pub effect_timer: f32,
    /// Quadrant angle where effect is centered (radians)
    pub effect_angle: f32,
    /// Effect intensity (for flash peak)
    pub effect_intensity: f32,
    /// Degree value (1-360) at the moment of polarity change for label display
    pub current_degree: i32,
}

impl Default for QuaternionChargesState {
    fn default() -> Self {
        Self {
            active: false,
            w: 0.0,
            ports: [0.0; 4],
            charges: [Charge::Plus1; 4],
            active_port: 0,
            strength: 0.0,
            invariants: Invariants::default(),
            // Polarity tracking defaults
            current_quadrant: 0,
            prev_quadrant: 0,
            polarity_changed: false,
            polarity_direction: 0,
            quadrant_hold_timer: 0.0,
            // Polarity effect defaults
            effect_timer: 0.0,
            effect_angle: 0.0,
            effect_intensity: 0.0,
            current_degree: 0,
        }
    }
}

// ============================================================================
// PURE MATH FUNCTIONS
// ============================================================================

/// Reduce angle A to w in [0, π/2) per REG rule
pub fn reduce_to_w(a: f32) -> f32 {
    a.rem_euclid(FRAC_PI_2)
}

/// Compute 4 quaternion ports from reduced angle w
/// Returns: {w, π-w, π+w, τ-w} normalized to [0, TAU)
pub fn quaternion_ports(w: f32) -> [f32; 4] {
    [
        w,                        // p0: w
        (PI - w).rem_euclid(TAU), // p1: π - w
        (PI + w).rem_euclid(TAU), // p2: π + w
        (TAU - w).rem_euclid(TAU), // p3: τ - w
    ]
}

/// Circular angular distance (wrap-aware)
pub fn ang_dist(a: f32, b: f32) -> f32 {
    let diff = (a - b).abs();
    diff.min(TAU - diff)
}

/// Assign charge by nearest axis (wrap-aware)
/// - +1 near 0° (East)
/// - +i near 90° (North)
/// - -1 near 180° (West)
/// - -i near 270° (South)
pub fn charge_for_angle(theta: f32) -> Charge {
    let axes = [0.0, FRAC_PI_2, PI, 3.0 * FRAC_PI_2];
    let charges = [Charge::Plus1, Charge::PlusI, Charge::Minus1, Charge::MinusI];

    let mut best = 0;
    let mut best_dist = f32::MAX;
    for (i, &axis) in axes.iter().enumerate() {
        let d = ang_dist(theta, axis);
        if d < best_dist {
            best_dist = d;
            best = i;
        }
    }
    charges[best]
}

/// Compute invariant checks for quaternion ports
pub fn compute_invariants(ports: &[f32; 4]) -> Invariants {
    const EPS: f32 = 1e-3;

    let pair1_sum = ports[0] + ports[3];
    let pair2_sum = ports[1] + ports[2];
    let total_sum = ports.iter().sum::<f32>();

    // Complement pairs: each should sum to TAU
    let complements_ok = (pair1_sum - TAU).abs() < EPS && (pair2_sum - TAU).abs() < EPS;

    // Total: should equal 2*TAU (720°)
    let total_ok = (total_sum - 2.0 * TAU).abs() < EPS;

    // Palindrome: each port has a mirror at TAU - port
    let palindrome_ok = ports.iter().all(|&p| {
        let mirror = (TAU - p).rem_euclid(TAU);
        ports.iter().any(|&q| ang_dist(mirror, q) < EPS)
    });

    // Lattice 24: each port within EPS of 15° grid
    let step = PI / 12.0; // 15° in radians
    let lattice_ok_24 = ports.iter().all(|&p| {
        let nearest = (p / step).round() * step;
        (p - nearest).abs() < EPS
    });

    Invariants {
        pair1_sum,
        pair2_sum,
        total_sum,
        complements_ok,
        total_ok,
        palindrome_ok,
        lattice_ok_24,
    }
}

// ============================================================================
// POLARITY / QUADRANT FUNCTIONS
// ============================================================================

/// Compute quadrant from dominant pitch class (0-11)
/// Quadrants: 0=E (+1, chromas 0-2), 1=N (+i, chromas 3-5),
///           2=W (-1, chromas 6-8), 3=S (-i, chromas 9-11)
pub fn quadrant_from_pitch_class(pc: usize) -> u8 {
    // Each quadrant spans 3 chromas (90° = π/2)
    // pc 0-2 → E (quadrant 0), pc 3-5 → N (quadrant 1), etc.
    ((pc / 3) as u8).min(3)
}

/// Compute degree A (1-360) from pitch class and energy (REG Figure 8)
/// This is the canonical formula for the harmonic integer.
/// - Spoke from pitch class (2 spokes per pitch class, 24 total)
/// - Ring from energy (0-14, 15 rings)
/// - A = ring * 24 + spoke_offset
pub fn compute_degree_a(pitch_class: usize, energy: f32) -> i32 {
    let dom_pc = pitch_class as i32;
    let s0 = (dom_pc * 2) % 24;
    let num_rings = 15;
    let ring = ((energy * (num_rings as f32 - 1.0)).round() as i32).clamp(0, num_rings - 1);
    let residue = ((s0 + 23) % 24) + 1;
    (residue + ring * 24).clamp(1, 360)
}

/// Compute quadrant from degree A (1-360) - REG-true polarity
/// Maps the degree to which of the 4 charge axes it's nearest to.
/// Wheel layout: wedge 0 at North, wedges increase CW
/// - Wedges 21-23, 0-2 (near North) → quadrant 1 (+i)
/// - Wedges 3-8 (near East) → quadrant 0 (+1)
/// - Wedges 9-14 (near South) → quadrant 3 (-i)
/// - Wedges 15-20 (near West) → quadrant 2 (-1)
pub fn quadrant_from_degree(degree_a: i32) -> u8 {
    // Get wedge (0-23) from degree
    let wedge = ((degree_a - 1) % 24) as i32;

    // Map wedge to nearest axis quadrant
    // Each axis spans 6 wedges (90° = 6 * 15°)
    // Centered on wedges 0 (N), 6 (E), 12 (S), 18 (W)
    match wedge {
        21..=23 | 0..=2 => 1,  // Near North → +i
        3..=8 => 0,            // Near East → +1
        9..=14 => 3,           // Near South → -i
        _ => 2,                // Near West → -1 (15-20)
    }
}

/// Compute direction of quadrant change: +1=CW, -1=CCW
/// CW: E→S→W→N→E (increasing angle direction)
/// CCW: E→N→W→S→E (decreasing angle direction)
pub fn quadrant_direction(from: u8, to: u8) -> i8 {
    if from == to {
        return 0;
    }

    // Compute shortest path direction on the circle
    // Quadrants: 0=E, 1=N, 2=W, 3=S
    // CW order: 0→3→2→1→0 (E→S→W→N→E)
    // CCW order: 0→1→2→3→0 (E→N→W→S→E)

    let diff = (to as i8 - from as i8).rem_euclid(4);

    // diff == 1: moved CCW (E→N)
    // diff == 3: moved CW (E→S, which is -1 mod 4 = 3)
    // diff == 2: opposite, use CCW as default
    if diff == 1 || diff == 2 {
        -1 // CCW
    } else {
        1 // CW
    }
}

/// Hysteresis threshold for switching active port (radians, ~10°)
const HYSTERESIS: f32 = 0.175;

/// Find nearest port to camera yaw angle with hysteresis
/// Returns new active port index (0-3)
/// prev_active_port provides stability - only switch if new port is significantly closer
pub fn compute_active_port_nearest(
    camera_yaw: f32,
    ports: &[f32; 4],
    prev_active_port: usize,
) -> usize {
    let yaw = camera_yaw.rem_euclid(TAU);

    // Find closest port
    let mut nearest = 0;
    let mut nearest_dist = f32::MAX;
    for (i, &port) in ports.iter().enumerate() {
        let d = ang_dist(yaw, port);
        if d < nearest_dist {
            nearest_dist = d;
            nearest = i;
        }
    }

    // Hysteresis: only switch if new port is significantly closer than current
    let prev_dist = ang_dist(yaw, ports[prev_active_port]);
    if nearest != prev_active_port && (prev_dist - nearest_dist) > HYSTERESIS {
        nearest
    } else {
        prev_active_port
    }
}

/// Compute overlay strength from fold24_t using smoothstep
/// Maps fold24_t (0.10..0.90) to strength (0..1) with smooth ramp
pub fn compute_qc_strength(fold24_t: f32) -> f32 {
    // smoothstep: ramps smoothly from 0 at 0.10 to 1 at 0.30, holds, then ramps down
    let ramp_up = smoothstep(0.10, 0.25, fold24_t);
    let ramp_down = 1.0 - smoothstep(0.75, 0.90, fold24_t);
    ramp_up * ramp_down
}

/// Smooth interpolation function (cubic Hermite)
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reduce_to_w_simple() {
        // Value already in range
        assert!((reduce_to_w(0.75) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn test_reduce_to_w_wraps() {
        // Value > π/2 should wrap
        let w = reduce_to_w(2.0);
        assert!(w >= 0.0 && w < FRAC_PI_2);
    }

    #[test]
    fn test_reduce_to_w_negative() {
        // Negative value should wrap to positive
        let w = reduce_to_w(-0.5);
        assert!(w >= 0.0 && w < FRAC_PI_2);
    }

    #[test]
    fn test_ports_43_degrees() {
        // A = 43° => w = 43°, ports = {43°, 137°, 223°, 317°}
        let a = 43.0_f32.to_radians();
        let w = reduce_to_w(a);
        let ports = quaternion_ports(w);

        assert!(
            (ports[0].to_degrees() - 43.0).abs() < 0.1,
            "p0 = {}°",
            ports[0].to_degrees()
        );
        assert!(
            (ports[1].to_degrees() - 137.0).abs() < 0.1,
            "p1 = {}°",
            ports[1].to_degrees()
        );
        assert!(
            (ports[2].to_degrees() - 223.0).abs() < 0.1,
            "p2 = {}°",
            ports[2].to_degrees()
        );
        assert!(
            (ports[3].to_degrees() - 317.0).abs() < 0.1,
            "p3 = {}°",
            ports[3].to_degrees()
        );
    }

    #[test]
    fn test_ports_0_degrees() {
        // A = 0° => w = 0°, ports = {0°, 180°, 180°, 360°(=0°)}
        let w = reduce_to_w(0.0);
        let ports = quaternion_ports(w);

        assert!(ports[0].abs() < 0.01, "p0 = {}°", ports[0].to_degrees());
        assert!(
            (ports[1].to_degrees() - 180.0).abs() < 0.1,
            "p1 = {}°",
            ports[1].to_degrees()
        );
        assert!(
            (ports[2].to_degrees() - 180.0).abs() < 0.1,
            "p2 = {}°",
            ports[2].to_degrees()
        );
        // p3 = TAU - 0 = TAU, which wraps to 0
        assert!(ports[3].abs() < 0.01, "p3 = {}°", ports[3].to_degrees());
    }

    #[test]
    fn test_charge_assignments() {
        // Near 0° => +1
        assert_eq!(charge_for_angle(0.1), Charge::Plus1);
        // Near 90° => +i
        assert_eq!(charge_for_angle(FRAC_PI_2 + 0.1), Charge::PlusI);
        // Near 180° => -1
        assert_eq!(charge_for_angle(PI + 0.1), Charge::Minus1);
        // Near 270° => -i
        assert_eq!(charge_for_angle(3.0 * FRAC_PI_2 + 0.1), Charge::MinusI);
    }

    #[test]
    fn test_invariants_43_degrees() {
        let a = 43.0_f32.to_radians();
        let w = reduce_to_w(a);
        let ports = quaternion_ports(w);
        let inv = compute_invariants(&ports);

        assert!(inv.complements_ok, "Complements should sum to TAU");
        assert!(inv.total_ok, "Total should equal 2*TAU");
        assert!(inv.palindrome_ok, "Palindrome mirrors should exist");
    }

    #[test]
    fn test_invariants_30_degrees() {
        // 30° is on the 15° lattice
        let a = 30.0_f32.to_radians();
        let w = reduce_to_w(a);
        let ports = quaternion_ports(w);
        let inv = compute_invariants(&ports);

        assert!(inv.complements_ok);
        assert!(inv.total_ok);
        assert!(inv.palindrome_ok);
        assert!(inv.lattice_ok_24, "30° should be on 15° lattice");
    }

    #[test]
    fn test_active_port_nearest() {
        // ports at 43°, 137°, 223°, 317°
        let ports = [
            43.0_f32.to_radians(),
            137.0_f32.to_radians(),
            223.0_f32.to_radians(),
            317.0_f32.to_radians(),
        ];

        // Camera at 45° should select port 0 (43°)
        assert_eq!(compute_active_port_nearest(45.0_f32.to_radians(), &ports, 0), 0);

        // Camera at 140° should select port 1 (137°)
        assert_eq!(compute_active_port_nearest(140.0_f32.to_radians(), &ports, 0), 1);

        // Camera at 220° should select port 2 (223°)
        assert_eq!(compute_active_port_nearest(220.0_f32.to_radians(), &ports, 0), 2);

        // Camera at 320° should select port 3 (317°)
        assert_eq!(compute_active_port_nearest(320.0_f32.to_radians(), &ports, 0), 3);
    }

    #[test]
    fn test_active_port_hysteresis() {
        // ports at 43°, 137°, 223°, 317°
        let ports = [
            43.0_f32.to_radians(),
            137.0_f32.to_radians(),
            223.0_f32.to_radians(),
            317.0_f32.to_radians(),
        ];

        // Camera at 90° is equidistant from port 0 (43°) and port 1 (137°)
        // With hysteresis, should stay on current port
        assert_eq!(compute_active_port_nearest(90.0_f32.to_radians(), &ports, 0), 0);
        assert_eq!(compute_active_port_nearest(90.0_f32.to_radians(), &ports, 1), 1);

        // Camera at 135° is clearly closer to port 1, should switch even with hysteresis
        assert_eq!(compute_active_port_nearest(135.0_f32.to_radians(), &ports, 0), 1);
    }

    #[test]
    fn test_qc_strength() {
        // Before window: strength = 0
        assert!(compute_qc_strength(0.05) < 0.01);

        // In ramp-up zone (0.10 to 0.25): increasing
        let s1 = compute_qc_strength(0.15);
        let s2 = compute_qc_strength(0.20);
        assert!(s1 > 0.0 && s1 < s2);

        // Middle of window: strength = 1
        assert!((compute_qc_strength(0.50) - 1.0).abs() < 0.01);

        // In ramp-down zone (0.75 to 0.90): decreasing
        let s3 = compute_qc_strength(0.80);
        let s4 = compute_qc_strength(0.85);
        assert!(s3 > s4 && s4 > 0.0);

        // After window: strength = 0
        assert!(compute_qc_strength(0.95) < 0.01);
    }
}

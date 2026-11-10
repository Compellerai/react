# Torus Knot Iconic Tuning Spec

## Design Philosophy

**Goal**: Maximize perceived "iconic" quality through:
- Richer variation via phrase-aware breathing
- Intentional dynamics via gated spice reveals
- Musically meaningful density changes
- Premium ornament accents locked to symmetry

**Constraints** (non-negotiable):
- Symmetry first: all accents quantized to N-fold anchors (12 or 24)
- Discrete state only: no continuous drift or free morphing
- Stability > responsiveness: hysteresis/cooldowns/holds everywhere
- Names are recognition-only: if we don't recognize, say nothing

---

## 1. Phrase-Aware Density Envelope

### Intent
Same topology can "breathe" across sections without changing topology. Density rises and falls like musical phrasing, not as a static bucket mapping.

### State Additions

```rust
// In TorusKnotState
pub phrase_env: f32,           // 0.0..1.0, envelope value
pub phrase_hold_beats: f32,    // Accumulated beats at current level
pub phrase_last_beat_phase: f32, // Track beat crossings
pub phrase_bias: i8,           // Derived: {-1, 0, +1}
```

### Constants

```rust
const PHRASE_STEP_UP: f32 = 0.15;      // Envelope increment on strong event
const PHRASE_STEP_DOWN: f32 = 0.08;    // Envelope decrement on weak tick
const PHRASE_MIN_HOLD_BEATS: f32 = 1.5; // Min beats before bias can change
const PHRASE_DECAY_RATE: f32 = 0.03;   // Per-beat decay in low-energy sections
```

### Update Rules (Beat-Driven, Not Per-Frame)

```
ON beat_crossing (beat_phase wraps from >0.8 to <0.2):
    phrase_hold_beats += 1.0

    // Determine event strength
    is_bar_boundary = (beat_count % 4 == 0) OR (beat_phase was near 0.0)
    is_strong_onset = spectral_flux > 0.4 AND energy_slope > 0.1
    is_weak_section = section_energy < 0.15 AND stability_smooth < 0.3

    IF is_bar_boundary AND is_strong_onset:
        // Strong event: step envelope UP
        phrase_env = min(phrase_env + PHRASE_STEP_UP, 1.0)
        phrase_hold_beats = 0.0  // Reset hold counter

    ELSE IF is_weak_section:
        // Weak section: step envelope DOWN
        phrase_env = max(phrase_env - PHRASE_STEP_DOWN, 0.0)
        phrase_hold_beats = 0.0

    ELSE:
        // Neutral: slow decay toward 0.5
        IF phrase_hold_beats >= PHRASE_MIN_HOLD_BEATS:
            phrase_env += (0.5 - phrase_env) * PHRASE_DECAY_RATE

// Derive discrete bias (only changes after hold period)
IF phrase_hold_beats >= PHRASE_MIN_HOLD_BEATS:
    new_bias = IF phrase_env < 0.33 THEN -1
               ELSE IF phrase_env > 0.66 THEN +1
               ELSE 0
    IF new_bias != phrase_bias:
        phrase_bias = new_bias
        phrase_hold_beats = 0.0  // Reset after bias change
```

### Integration with Density Pipeline

```rust
// Replace current density_level_target computation:
let bucket = complexity_bucket(pr, qr, spice_level);  // spice_level from Task 2
let biased = (bucket as i8 + phrase_bias).clamp(0, max_level as i8) as u8;
density_level_target = biased;
```

### Behavior Summary

| phrase_env | phrase_bias | Effect |
|------------|-------------|--------|
| 0.0 - 0.32 | -1 | Density pulls DOWN one level (calmer) |
| 0.33 - 0.66 | 0 | Density matches bucket (neutral) |
| 0.67 - 1.0 | +1 | Density pushes UP one level (richer) |

---

## 2. Gated Spice System

### Intent
7-limit and 11-limit boosts become "cinematic reveals" rather than always-on. Exotic intervals feel like intentional moments.

### State Additions

```rust
// In TorusKnotState
pub spice_gate_active: bool,       // Is spice gating currently active
pub spice_level: u8,               // 0, 1, or 2 (discrete steps)
pub spice_confidence_accum: f32,   // Accumulated high-confidence time
pub spice_entropy_stable_time: f32, // Time with low chroma entropy
pub spice_hold_timer: f32,         // Prevents rapid level changes
pub spice_cooldown: f32,           // Longer cooldown when deactivating
```

### Constants

```rust
const SPICE_CONFIDENCE_THRESH: f32 = 0.55;   // Harmonic confidence required
const SPICE_ENTROPY_THRESH: f32 = 0.65;      // Below this = "stable" chroma
const SPICE_GATE_ON_TIME: f32 = 1.5;         // Seconds of stability to activate
const SPICE_GATE_OFF_COOLDOWN: f32 = 3.0;    // Seconds before gate can close
const SPICE_LEVEL_HOLD: f32 = 0.6;           // Seconds between level steps
```

### Chroma Entropy Computation

```rust
fn chroma_entropy(chroma: &[f32; 12]) -> f32 {
    let sum: f32 = chroma.iter().sum();
    if sum < 0.001 { return 1.0; }  // Silent = max entropy

    let mut entropy = 0.0;
    for &c in chroma {
        let p = c / sum;
        if p > 0.001 {
            entropy -= p * p.ln();
        }
    }
    // Normalize to 0..1 (max entropy = ln(12) ≈ 2.48)
    (entropy / 2.48).clamp(0.0, 1.0)
}
```

### Update Rules

```
// Per-frame update
entropy = chroma_entropy(chroma)
is_confident = stability_smooth > SPICE_CONFIDENCE_THRESH
is_stable_chroma = entropy < SPICE_ENTROPY_THRESH

// Accumulate activation conditions
IF is_confident AND is_stable_chroma:
    spice_confidence_accum += dt
    spice_entropy_stable_time += dt
ELSE:
    spice_confidence_accum = max(spice_confidence_accum - dt * 0.5, 0.0)
    spice_entropy_stable_time = max(spice_entropy_stable_time - dt * 0.3, 0.0)

// Gate activation (turn ON)
IF NOT spice_gate_active AND spice_cooldown <= 0:
    IF spice_confidence_accum >= SPICE_GATE_ON_TIME:
        spice_gate_active = true
        spice_level = 0  // Start at level 0, will step up
        spice_hold_timer = SPICE_LEVEL_HOLD

// Gate deactivation (turn OFF) - requires longer cooldown
IF spice_gate_active:
    IF spice_confidence_accum < 0.3 AND spice_entropy_stable_time < 0.5:
        spice_gate_active = false
        spice_cooldown = SPICE_GATE_OFF_COOLDOWN
        // spice_level stays at current value, will decay below

// Spice level stepping (discrete, with holds)
spice_hold_timer = max(spice_hold_timer - dt, 0.0)
IF spice_hold_timer <= 0:
    IF spice_gate_active AND spice_level < 2:
        // Step UP toward 2
        spice_level += 1
        spice_hold_timer = SPICE_LEVEL_HOLD
    ELSE IF NOT spice_gate_active AND spice_level > 0:
        // Step DOWN toward 0 (slower)
        spice_level -= 1
        spice_hold_timer = SPICE_LEVEL_HOLD * 1.5  // Slower decay

// Cooldown tick
spice_cooldown = max(spice_cooldown - dt, 0.0)
```

### Boost Factor Lookup

```rust
fn spice_boost(max_pq: i32, spice_level: u8) -> i64 {
    // Only boost 7-limit and 11-limit intervals
    match max_pq {
        7 => match spice_level {
            0 => 1,
            1 => 2,
            _ => 3,  // Full 7-limit boost at level 2
        },
        11 => match spice_level {
            0 => 1,
            1 => 2,
            _ => 4,  // Full 11-limit boost at level 2
        },
        _ => 1,  // No boost for other primes
    }
}
```

### Modified complexity_bucket

```rust
fn complexity_bucket(p: i32, q: i32, spice_level: u8) -> u8 {
    let c = safe_lcm(p, q);
    let (pr, qr) = reduced_pq(p, q);
    let max_pq = pr.max(qr);

    // Spice boost is now gated by spice_level
    let boost = spice_boost(max_pq, spice_level);
    let effective_c = c * boost;

    // See Task 5 for revised thresholds
    match effective_c {
        0..=8    => 0,
        9..=24   => 1,
        25..=55  => 2,
        56..=110 => 3,
        111..=220 => 4,
        221..=500 => 5,
        _ => 6,
    }
}
```

### Behavior Summary

| spice_level | 7-limit boost | 11-limit boost | Feel |
|-------------|---------------|----------------|------|
| 0 | 1× | 1× | Neutral (exotic intervals blend in) |
| 1 | 2× | 2× | Warming (slight lift) |
| 2 | 3× | 4× | Full reveal (cinematic) |

---

## 3. Ornament Layer

### Intent
Premium visual detail that reacts to audio but NEVER changes topology. Ornaments are symmetry-locked and event-driven.

### Ornament Types (Mutually Exclusive)

| ID | Name | Description |
|----|------|-------------|
| 0 | None | No ornament active |
| 1 | Radial Filigree | Thin 24-fold spokes that pulse on high-band energy |
| 2 | Harmonic Notches | Arc segments at scaffold ring edges, 2-3 patterns |
| 3 | Knot Halo | Faint duplicate knot offset by quantized angle |

### State Additions

```rust
// In TorusKnotState
pub ornament_id: u8,              // 0=none, 1=filigree, 2=notches, 3=halo
pub ornament_intensity: f32,      // Quantized: 0.0, 0.5, 1.0
pub ornament_hold_timer: f32,     // Prevents rapid switching
pub ornament_pattern: u8,         // Sub-pattern within ornament type (0-2)
pub ornament_spoke_mask: u32,     // Bitmask for filigree (which spokes light)
```

### Constants

```rust
const ORNAMENT_MIN_HOLD_BEATS: f32 = 2.0;  // Min beats before ornament can change
const ORNAMENT_INTENSITY_HOLD: f32 = 0.25; // Seconds before intensity can change
```

### Selection Rules (Event-Driven)

```
ON bar_boundary OR strong_onset:
    IF ornament_hold_timer <= 0:
        // Compute selection scores
        high_band_energy = chroma[8..12].sum() / chroma.sum()  // Upper bins
        scaffold_stable = scaffold_blend > 0.7 AND stability_smooth > 0.5
        beat_strong = kick > 0.5 OR spectral_flux > 0.6

        // Priority selection (mutually exclusive)
        IF beat_strong AND section_energy > 0.4:
            new_ornament = 3  // Knot Halo (dramatic)
        ELSE IF scaffold_stable:
            new_ornament = 2  // Harmonic Notches (structural)
        ELSE IF high_band_energy > 0.35:
            new_ornament = 1  // Radial Filigree (shimmer)
        ELSE:
            new_ornament = 0  // None

        IF new_ornament != ornament_id:
            ornament_id = new_ornament
            ornament_hold_timer = ORNAMENT_MIN_HOLD_BEATS * (60.0 / bpm)
            ornament_pattern = (ornament_pattern + 1) % 3  // Cycle patterns

// Hold timer tick
ornament_hold_timer = max(ornament_hold_timer - dt, 0.0)
```

### Intensity Quantization

```
// Continuous input → discrete output
raw_intensity = section_energy * (0.5 + stability_smooth * 0.5)

target_intensity = IF raw_intensity < 0.25 THEN 0.0
                   ELSE IF raw_intensity < 0.55 THEN 0.5
                   ELSE 1.0

// Hysteresis on intensity changes
IF abs(target_intensity - ornament_intensity) > 0.1:
    intensity_change_timer += dt
    IF intensity_change_timer >= ORNAMENT_INTENSITY_HOLD:
        ornament_intensity = target_intensity
        intensity_change_timer = 0.0
ELSE:
    intensity_change_timer = 0.0
```

### Ornament Rendering Parameters

#### 1. Radial Filigree (ID=1)
```
Parameters:
  - spoke_count: 24 (locked to 24-fold)
  - spoke_width: 0.005 (thin)
  - spoke_length: 0.15..0.4 (pulsing with high-band energy)
  - spoke_mask: bitmask, only every k-th spoke lights

Mask computation (quantized):
  k = 1 + ornament_pattern  // k ∈ {1, 2, 3}
  spoke_mask = bits where (i % k == 0) for i in 0..24

Pulse:
  length = 0.15 + high_band_energy * 0.25 * ornament_intensity
```

#### 2. Harmonic Notches (ID=2)
```
Parameters:
  - notch_count: 12 (locked to scaffold)
  - notch_arc: 8° (small arc segment)
  - notch_radius: scaffold_radius + 0.02
  - pattern: 3 discrete patterns

Patterns:
  0: all 12 notches visible
  1: alternating (6 visible)
  2: thirds (4 visible at 0°, 90°, 180°, 270°)

Flip: pattern changes on bar boundary
```

#### 3. Knot Halo (ID=3)
```
Parameters:
  - halo_alpha: 0.15 * ornament_intensity (faint)
  - halo_offset: k * (2π / 24) where k = 1 + ornament_pattern
  - halo_scale: 0.97 (slightly smaller than main knot)

Appears: only when ornament_intensity >= 0.5
```

---

## 4. WI Sector Accent

### Intent
Winding Interval becomes "felt" as sector accent geometry, not a continuous rotation driver. Brief highlights on onset events.

### State Additions

```rust
// In TorusKnotState
pub wi_accent_step: u8,       // Quantized sector index [0..N-1]
pub wi_accent_timer: f32,     // Countdown for accent duration
pub wi_accent_active: bool,   // Is accent currently showing
```

### Constants

```rust
const WI_ACCENT_DURATION: f32 = 0.18;  // 180ms accent flash
const WI_ACCENT_COOLDOWN: f32 = 0.4;   // Min time between accents
```

### Update Rules

```
// Compute quantized sector from WI
N = active_fold  // 12 or 24
wi_eff_step = round(wi_eff / (360.0 / N)) % N

// Trigger on onset (not every frame)
IF spectral_flux > 0.35 AND wi_accent_timer <= -WI_ACCENT_COOLDOWN:
    wi_accent_step = wi_eff_step
    wi_accent_timer = WI_ACCENT_DURATION
    wi_accent_active = true

// Timer tick
IF wi_accent_timer > 0:
    wi_accent_timer -= dt
    IF wi_accent_timer <= 0:
        wi_accent_active = false
ELSE:
    wi_accent_timer -= dt  // Goes negative for cooldown tracking
```

### Accent Rendering

```
When wi_accent_active:
    // Highlight primary sector and its opposite
    primary_sector = wi_accent_step
    opposite_sector = (wi_accent_step + N/2) % N

    // Accent intensity fades over duration
    fade = wi_accent_timer / WI_ACCENT_DURATION
    accent_alpha = 0.4 * fade * ease_out_quad(fade)

    // Render as brief glow at sector boundaries
    sector_angle_primary = primary_sector * (2π / N)
    sector_angle_opposite = opposite_sector * (2π / N)

    // Glow width: ±5° from sector center
    // Color: match knot hue, boost brightness
```

---

## 5. Iconic Tables

### Design Goals
1. Consonances stay low (buckets 0-1) — stable foundation
2. Steps/semitones get shimmer (buckets 3-4) — not maxed constantly
3. 7/11-limit gets clear lift WITH spice (jump 2+ buckets) — cinematic reveal
4. Avoid constant maxing in common music — breathing room

### Revised Bucket Thresholds

```rust
fn complexity_bucket(p: i32, q: i32, spice_level: u8) -> u8 {
    let c = safe_lcm(p, q);
    let (pr, qr) = reduced_pq(p, q);
    let max_pq = pr.max(qr);
    let boost = spice_boost(max_pq, spice_level);
    let eff = c * boost;

    match eff {
        0..=8     => 0,   // Octave (2), Fifth (6)
        9..=24    => 1,   // Fourth (12), Major third (20), Minor sixth (15)
        25..=55   => 2,   // Minor third (30), Augmented (40), 7-limit base
        56..=110  => 3,   // Whole step (72), 7-limit spiced, 11-limit base
        111..=220 => 4,   // Semitone (240→fits!), 7-limit full, Golden (104)
        221..=500 => 5,   // 11-limit spiced, extreme semitone context
        _         => 6,   // Extreme coprime (17:19=323+)
    }
}
```

**Key change**: Semitone (16:15, LCM=240) now lands at bucket 4, not 5. This prevents constant maxing on chromatic passages while still giving shimmer.

### Revised Bead Tables (Premium HIGH)

```rust
// Tier LOW: conservative, 4 levels
const BEADS_LOW: [i32; 4] = [4, 6, 8, 12];
// Increments: +2, +2, +4

// Tier MED: balanced, 6 levels
const BEADS_MED: [i32; 6] = [6, 8, 12, 18, 26, 36];
// Increments: +2, +4, +6, +8, +10 (accelerating)

// Tier HIGH: ambitious, 7 levels (perf doesn't matter)
const BEADS_HIGH: [i32; 7] = [8, 12, 18, 28, 42, 62, 88];
// Increments: +4, +6, +10, +14, +20, +26 (exponential feel)
```

### Golden Test Set

| Ratio | LCM | Spice 0 | Spice 1 | Spice 2 | Bucket (s0/s1/s2) | Beads MED | Musical Role |
|-------|-----|---------|---------|---------|-------------------|-----------|--------------|
| 3:2   | 6   | 6 | 6 | 6 | 0/0/0 | 6 | Perfect fifth |
| 4:3   | 12  | 12 | 12 | 12 | 1/1/1 | 8 | Perfect fourth |
| 5:4   | 20  | 20 | 20 | 20 | 1/1/1 | 8 | Major third |
| 6:5   | 30  | 30 | 30 | 30 | 2/2/2 | 12 | Minor third |
| 9:8   | 72  | 72 | 72 | 72 | 3/3/3 | 18 | Whole step |
| 16:15 | 240 | 240 | 240 | 240 | 4/4/4 | 26 | Semitone |
| 7:4   | 28  | 28 | 56 | 84 | 2/3/3 | 12→18 | Septimal seventh |
| 7:5   | 35  | 35 | 70 | 105 | 2/3/4 | 12→26 | Septimal tritone |
| 8:13  | 104 | 104 | 104 | 104 | 3/3/3 | 18 | Golden ratio |
| 11:8  | 88  | 88 | 176 | 352 | 3/4/5 | 18→36 | Undecimal tritone |
| 17:19 | 323 | 323 | 323 | 323 | 5/5/5 | 36 | Extreme coprime |

### Spice Gate Behavior by Ratio

| Ratio | Without Spice | With Full Spice | Delta | Reveal Quality |
|-------|---------------|-----------------|-------|----------------|
| 7:4   | bucket 2, 12 beads | bucket 3, 18 beads | +1 level | Subtle warmth |
| 7:5   | bucket 2, 12 beads | bucket 4, 26 beads | +2 levels | Clear reveal |
| 11:8  | bucket 3, 18 beads | bucket 5, 36 beads | +2 levels | Dramatic reveal |

### Phrase Bias Examples

With phrase_bias = +1 (climax):
- 3:2 (bucket 0) → level 1 → 8 beads
- 9:8 (bucket 3) → level 4 → 26 beads
- 7:5 spiced (bucket 4) → level 5 → 36 beads

With phrase_bias = -1 (calm):
- 3:2 (bucket 0) → level 0 (clamped) → 6 beads
- 9:8 (bucket 3) → level 2 → 12 beads
- 16:15 (bucket 4) → level 3 → 18 beads

---

## 6. Debug Overlay

### Format

```
T(p:q) | b→L | base×mul→clamp | spice(lvl,×) | phrase(env,bias) | orn(id,int) | wi=step
```

### Fields

| Field | Format | Example |
|-------|--------|---------|
| T(p:q) | `T(p:q)` | `T(7:5)` |
| b→L | `bucket→level` | `4→3` |
| base×mul→clamp | `beads_base×multiplier→clamped` | `18×2→26` |
| spice(lvl,×) | `spice(level,boost)` | `spice(2,3×)` |
| phrase(env,bias) | `phrase(env%,bias)` | `phrase(72%,+1)` |
| orn(id,int) | `ornament(id,intensity)` | `orn(1,0.5)` |
| wi=step | `wi=step_index` | `wi=7` |

### Example Output

```
T(7:5) | 4→3 | 18×1→18 | spice(2,3×) | phrase(72%,+1) | orn(1,0.5) | wi=7
```

### State Addition

```rust
// In TorusKnotState or CameraState
pub debug_overlay_enabled: bool,
```

### Toggle

Via keyboard shortcut or UI toggle. Default: OFF.

### Implementation Note

Format string in Rust:
```rust
fn density_debug_string(&self) -> String {
    format!(
        "T({}:{}) | {}→{} | {}×{}→{} | spice({},{}) | phrase({:.0}%,{:+}) | orn({},{}) | wi={}",
        self.p, self.q,
        complexity_bucket(self.p, self.q, self.spice_level),
        self.density_level,
        beads_for_level(self.density_level, quality_tier),
        self.paint.bead_multiplier(),
        self.bead_count,
        self.spice_level,
        format!("{}×", spice_boost(self.p.max(self.q), self.spice_level)),
        self.phrase_env * 100.0,
        self.phrase_bias,
        self.ornament_id,
        self.ornament_intensity,
        self.wi_accent_step
    )
}
```

---

## State Summary

### New Fields in TorusKnotState

```rust
// Task 1: Phrase envelope
pub phrase_env: f32,
pub phrase_hold_beats: f32,
pub phrase_last_beat_phase: f32,
pub phrase_bias: i8,

// Task 2: Spice gating
pub spice_gate_active: bool,
pub spice_level: u8,
pub spice_confidence_accum: f32,
pub spice_entropy_stable_time: f32,
pub spice_hold_timer: f32,
pub spice_cooldown: f32,

// Task 3: Ornaments
pub ornament_id: u8,
pub ornament_intensity: f32,
pub ornament_hold_timer: f32,
pub ornament_pattern: u8,
pub ornament_spoke_mask: u32,

// Task 4: WI accent
pub wi_accent_step: u8,
pub wi_accent_timer: f32,
pub wi_accent_active: bool,

// Task 6: Debug
pub debug_overlay_enabled: bool,
```

### Modified Functions

| Function | Change |
|----------|--------|
| `complexity_bucket()` | Add `spice_level` parameter, use gated boost |
| `beads_for_level()` | Use revised bead tables |
| `max_density_level()` | No change (tier caps remain) |
| Density hysteresis block | Integrate `phrase_bias` into target |

### New Functions

| Function | Purpose |
|----------|---------|
| `chroma_entropy()` | Compute chroma distribution entropy |
| `spice_boost()` | Lookup boost factor from spice_level |
| `density_debug_string()` | Format debug overlay |

---

## Implementation Order

1. **Task 5 first**: Update bucket thresholds and bead tables (foundation)
2. **Task 2**: Implement spice gating (modifies complexity_bucket signature)
3. **Task 1**: Add phrase envelope (depends on beat tracking)
4. **Task 4**: WI sector accent (independent, can parallel)
5. **Task 3**: Ornament layer (shader changes, last)
6. **Task 6**: Debug overlay (testing aid throughout)

---

## Verification Checklist

- [ ] Build compiles
- [ ] T(3:2) stays bucket 0 regardless of spice/phrase
- [ ] T(7:5) jumps bucket 2→4 when spice gate opens
- [ ] T(11:8) jumps bucket 3→5 when spice gate opens
- [ ] Phrase bias ±1 shifts density without changing bucket
- [ ] Ornaments only change on bar boundaries
- [ ] WI accent fires on onset, lasts ~180ms
- [ ] Debug overlay shows all fields correctly
- [ ] No jitter: all state changes have hold timers

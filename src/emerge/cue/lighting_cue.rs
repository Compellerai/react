//! COM-499 SaaS lighting-cue executor — slice 1 (constant + oscillator).
//!
//! REACT/BAE is the per-frame lighting executor (see
//! `docs/lighting-com-499-react-contract.md`, Appendix A). This module turns the
//! SaaS cue `launch_contract.bindings` into overlays on the existing
//! `FixtureIntentState` (intent-level composition), capability-gated by the
//! fixture's `ChannelMap`.
//!
//! Sources resolved: `constant`, `oscillator`, `step_chase`, `palette`,
//! `color_strobe` (flash a palette colour on a square duty cycle, so an RGB-only
//! fixture can strobe in red/white/any colour with no dedicated strobe channel),
//! and `react_strobe_gate` (minimal engage — operator strobe-slider authority is
//! deferred with the strobe-authority slice). Segmented fixtures (pixel bars, the
//! 24ch RGB strobe) use the per-cell sources `segment_chase` / `segment_rainbow` /
//! `segment_bank` / `segment_wipe` / `segment_solid` / `segment_strobe` (COM-499
//! A.4). Single multi-channel colour (`RGB` / `RGBAWUV`) resolves through the
//! palette path. SaaS never computes frames — this is the REACT side.

use std::borrow::Cow;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::emerge::contracts::{ChannelSlot, FixturePatch, FixtureProfile};
use crate::emerge::FixtureIntentState;
use crate::emerge::{add_bipolar, bipolar_wave, OscillatorShape};

/// Stop/fade-out window (COM-499 rule 8). A stopped cue ramps its contribution to
/// zero over this duration instead of hard-cutting; the worker reports `STOPPED`
/// once the ramp completes (when it is truly no longer applying it). Shared by the
/// worker (report timing) and the renderer (per-frame fade factor) so they can't drift.
pub const STOP_FADE_SECS: f32 = 0.6;

// Slice 2b: cue oscillators + step_chase now phase-lock to the real
// `ShowClockSnapshot.beat` (Ableton Link / Pro DJ Link, audio-derived
// fallback when neither external is live). The previous DEMO_BPM constant
// was a slice-1 placeholder; beats are now plumbed in from the caller
// (`apply_cue_bindings` -> `Source::eval`).

// ----- wire DTOs: GET /api/v1/lighting/cues -----

#[derive(Debug, Clone, Deserialize)]
pub struct CueListResponse {
    #[serde(default)]
    pub cues: Vec<CueDto>,
}

// `Serialize` lets the worker persist a fetched cue verbatim into the local
// store; rehydration deserializes it straight back through `ActiveCue::from_dto`
// (COM-499 offline durability / cache-from-launches).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CueDto {
    pub launch_id: String,
    #[serde(default)]
    pub status: String,
    /// COM-497 starter CueProfile id (consume-surface field).
    #[serde(default)]
    pub cue_profile_id: Option<String>,
    /// Starter effect id when the launch came from the effect catalog.
    #[serde(default)]
    pub effect_id: Option<String>,
    /// Optional stage scope (top-level; `metadata.stage_id` mirrors it).
    #[serde(default)]
    pub stage_id: Option<i64>,
    #[serde(default)]
    pub launch_contract: Option<LaunchContractDto>,
    #[serde(default)]
    pub variables: serde_json::Map<String, serde_json::Value>,
    /// Executor metadata: `strobe_policy` (always `react_slider_authoritative`
    /// in v1) and `runtime_role`. Captured so the consume surface is
    /// contract-complete; `strobe_policy` *enforcement* is deferred (2h).
    #[serde(default)]
    pub metadata: Option<CueMetadataDto>,
    /// Server's echo of our last acknowledgement, if any. Captured so it is not
    /// silently dropped on deserialize (COM-499 consume surface); restart-recovery
    /// use is wired in 2f.
    #[serde(default)]
    pub ack: Option<serde_json::Value>,
    /// Server's echo of our last status report, if any (consume surface).
    #[serde(default)]
    pub report: Option<serde_json::Value>,
}

/// `metadata` block on the cue payload. Only the fields the executor reads are
/// captured; serde ignores the rest (`stage_id`, `last_variable_update_at`, …).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CueMetadataDto {
    #[serde(default)]
    pub strobe_policy: Option<String>,
    #[serde(default)]
    pub runtime_role: Option<String>,
    /// Human label for the operator's local-launch picker (cache-from-launches).
    /// Read only when caching a definition; the executor itself ignores it.
    #[serde(default)]
    pub effect_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LaunchContractDto {
    #[serde(default)]
    pub bindings: Vec<BindingDto>,
    /// COM-499 promote idempotency marker: when REACT promotes an operator-local
    /// cue it stamps this with the local launch_id. SaaS stores + echoes the
    /// launch_contract verbatim, so on a later fetch REACT recognises its own twin
    /// (even after a lost response / crash mid-promote) and never double-POSTs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub react_origin_id: Option<String>,
    /// Variable SCHEMA: `{ <name>: { default, min/max | allowed[] } }` — distinct
    /// from the top-level live values. The server does NOT clamp (mergeVariables
    /// passes live values through), so the executor clamps `${var}` against this
    /// (COM-499 A.9).
    #[serde(default)]
    pub variables: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BindingDto {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub target: serde_json::Value,
    #[serde(default)]
    pub source: serde_json::Value,
    #[serde(default)]
    pub resolve: serde_json::Value,
}

// ----- normalized, evaluable binding model -----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveMode {
    Override,
    Max,
    Add,
}

impl ResolveMode {
    fn parse(s: &str) -> Self {
        match s {
            "max" => Self::Max,
            "add" => Self::Add,
            _ => Self::Override,
        }
    }
}

/// The single-channel intent attribute a binding `target.type` maps onto.
/// Continuous attrs hold an f32 in 0..1 directly; enum-index attrs (GOBO,
/// LASER_MACRO) hold a `u8` in `FixtureIntentState` whose values are
/// normalized to 0..1 here so cue grammar stays uniform across types — the
/// `× 32` / `× 16` byte spread is applied later at DMX emission.
///
/// Slice 2c adds `GoboSlot` + `LaserMacro` so step_chase can drive laser cue
/// banks and gobo wheels; multi-channel color targets (RGBAWUV) still use
/// the dedicated `BindingOp::Color` path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentAttr {
    Dimmer,
    Pan,
    Tilt,
    Strobe,
    Prism,
    GoboSlot,
    GoboRotation,
    LaserMacro,
    Zoom,
    Focus,
}

impl IntentAttr {
    fn from_type(t: &str) -> Option<Self> {
        match t {
            "DIMMER" | "MASTER_DIMMER" => Some(Self::Dimmer),
            // BLACKOUT here is the fixture's master enable (e.g. the
            // scan-laser's LASER ON/OFF gate) — semantically a binary
            // dimmer, plumbed via the same channel/slot. Slice 2d.
            "BLACKOUT" => Some(Self::Dimmer),
            "PAN" => Some(Self::Pan),
            "TILT" => Some(Self::Tilt),
            "SHUTTER" => Some(Self::Strobe),
            "PRISM" => Some(Self::Prism),
            "GOBO" => Some(Self::GoboSlot),
            "ROTATION" => Some(Self::GoboRotation),
            "LASER" | "LASER_MACRO" | "MACRO" => Some(Self::LaserMacro),
            "ZOOM" => Some(Self::Zoom),
            "FOCUS" => Some(Self::Focus),
            _ => None,
        }
    }

    /// Capability gate: is this fixture patched with the channel this attribute
    /// drives? (COM-499 Appendix A.2 / A.11.)
    fn channel_present(self, fixture: &FixturePatch) -> bool {
        let ch = &fixture.channels;
        match self {
            Self::Dimmer => ch.dimmer.is_some(),
            Self::Pan => ch.pan.is_some(),
            Self::Tilt => ch.tilt.is_some(),
            Self::Strobe => ch.strobe.is_some(),
            Self::Prism => ch.prism.is_some(),
            Self::GoboSlot => ch.gobo_slot.is_some(),
            Self::GoboRotation => ch.gobo_rotation.is_some(),
            Self::LaserMacro => ch.laser_macro.is_some(),
            Self::Zoom => ch.zoom.is_some(),
            Self::Focus => ch.focus.is_some(),
        }
    }

    fn get(self, intent: &FixtureIntentState) -> f32 {
        match self {
            Self::Dimmer => intent.dimmer,
            Self::Pan => intent.pan,
            Self::Tilt => intent.tilt,
            Self::Strobe => intent.strobe,
            Self::Prism => intent.prism,
            // Enum-index attrs normalize their u8 max (7 for gobo since ×32
            // covers slots 0..7; 15 for laser_macro since ×16 covers 0..15).
            Self::GoboSlot => intent.gobo_slot as f32 / 7.0,
            Self::GoboRotation => intent.gobo_rotation,
            Self::LaserMacro => intent.laser_macro as f32 / 15.0,
            Self::Zoom => intent.zoom,
            Self::Focus => intent.focus,
        }
    }

    fn set(self, intent: &mut FixtureIntentState, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match self {
            Self::Dimmer => intent.dimmer = v,
            Self::Pan => intent.pan = v,
            Self::Tilt => intent.tilt = v,
            Self::Strobe => intent.strobe = v,
            Self::Prism => intent.prism = v,
            Self::GoboSlot => intent.gobo_slot = (v * 7.0).round().clamp(0.0, 7.0) as u8,
            Self::GoboRotation => intent.gobo_rotation = v,
            Self::LaserMacro => intent.laser_macro = (v * 15.0).round().clamp(0.0, 15.0) as u8,
            Self::Zoom => intent.zoom = v,
            Self::Focus => intent.focus = v,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Source {
    Constant(f32),
    Oscillator {
        shape: OscillatorShape,
        period_beats: f32,
        depth: f32,
        center: f32,
    },
}

impl Source {
    /// Evaluate to a 0..1-ish value (oscillator is bipolar around `center`).
    /// `add` composition clamps; `override`/`max` clamp on write.
    /// `beats` is the monotonic show-clock beat count (Ableton Link / Pro DJ
    /// / audio-derived) — see slice 2b note above.
    ///
    /// `beats` is `f64` and the 0..1 phase is reduced (`.fract()`) in f64
    /// BEFORE narrowing to `f32`: at 122 BPM the beat count passes ~350k after
    /// two days of uptime, where an f32 ULP (~0.03 beats) exceeds the
    /// per-frame beat advance — fast strobes would quantize to a handful of
    /// phase steps. Reducing in f64 keeps sub-frame phase for years.
    fn eval(self, beats: f64) -> f32 {
        match self {
            Source::Constant(v) => v,
            Source::Oscillator {
                shape,
                period_beats,
                depth,
                center,
            } => {
                let beats = beats.max(0.0);
                let period = (period_beats as f64).max(0.0001);
                let phase = (beats / period).fract() as f32;
                center + depth * bipolar_wave(shape, phase, 0.5)
            }
        }
    }
}

fn parse_shape(s: &str) -> OscillatorShape {
    match s {
        "triangle" => OscillatorShape::Triangle,
        "saw" => OscillatorShape::Saw,
        "square" => OscillatorShape::Square,
        _ => OscillatorShape::Sine,
    }
}

#[derive(Clone, Debug)]
enum BindingOp {
    /// Single-channel scalar: a constant or oscillator drives one intent attribute.
    Single { attr: IntentAttr, source: Source },
    /// Multi-channel colour: a palette colour written to red/green/blue.
    Color { rgb: [f32; 3] },
    /// Colour strobe (COM-499): flash a palette colour on/off on a square duty
    /// cycle — the channel-cheap way to strobe an RGB-only fixture (e.g. the 200W
    /// in 3-channel mode) in red/white/any colour with no dedicated strobe channel.
    ColorStrobe {
        rgb: [f32; 3],
        period_beats: f32,
        duty: f32,
    },
    /// Step chase (COM-499 slice 2c): cycle through a list of values on the
    /// beat. Inline `values` is used verbatim; `values_from` is a profile
    /// path (today: `source_attribute.X.steps`) resolved per-fixture at
    /// apply time so each laser/gobo fixture chases its own pattern bank.
    StepChase {
        attr: IntentAttr,
        period_beats: f32,
        inline_values: Option<Vec<f32>>,
        values_from: Option<StepValuesPath>,
    },
    /// Per-cell colour for a segmented fixture (COM-499 A.4): a `SegmentKind`
    /// computes each cell's RGB from the beat clock; the renderer writes them to
    /// the fixture's per-cell channels. Drives the 24ch RGB strobe / pixel bars.
    Segment { kind: SegmentKind },
}

/// Sweep direction for segment effects across the N cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SegDir {
    LeftToRight,
    RightToLeft,
    CenterOut,
    Bounce,
}

impl SegDir {
    fn parse(s: &str) -> Self {
        match s {
            "right_to_left" => Self::RightToLeft,
            "center_out" | "centre_out" => Self::CenterOut,
            "bounce" => Self::Bounce,
            _ => Self::LeftToRight,
        }
    }
}

/// How `segment_bank` partitions the cells into two colour groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BankPattern {
    Alternate,
    Halves,
}

impl BankPattern {
    fn parse(s: &str) -> Self {
        match s {
            "halves" | "split" => Self::Halves,
            _ => Self::Alternate,
        }
    }
}

/// A per-cell effect over a segmented fixture's N cells. Each variant is a pure
/// function of (cell index, cell count, beats) -> RGB, evaluated in `eval_segment`.
///
/// Palette names are `Cow<'static, str>`: the autopilot / operator paths build a
/// `SegmentKind` per fixture per frame on the ~30Hz render path, and every
/// palette they use is a static literal — `Cow::Borrowed` makes those paths
/// allocation-free, while cue-contract parsing (once per launch) still carries
/// operator-authored names via `Cow::Owned`.
///
/// `pub(crate)` so the DMX writer (`dmx/mapping.rs`) can build the kind ONCE per
/// fixture per frame and evaluate cells against it, instead of re-allocating the
/// kind per cell.
#[derive(Clone, Debug)]
pub(crate) enum SegmentKind {
    /// A lit head/comet of `width` cells sweeping the bar in `direction`, head
    /// colour sampled from the (possibly gradient) `palette`, over `background`.
    Chase {
        direction: SegDir,
        width: f32,
        period_beats: f32,
        palette: Cow<'static, str>,
        background: Cow<'static, str>,
        /// Pre-INTEGRATED cycle count overriding the `beats / period_beats`
        /// derivation. Required whenever `period_beats` varies frame-to-frame
        /// (the tension-tightening autopilot): `fract(abs_beats / period)` at
        /// beat ~5000 teleports the head on every period change — the same
        /// class of bug `CueSnapshot::chase_head` fixed on the intent path.
        /// `None` = fixed-period operator/cue-def chases, byte-identical to
        /// the historical derivation.
        head_cycle: Option<f64>,
    },
    /// A hue spread across the cells (`spread` cycles of hue end-to-end) that
    /// scrolls one full cycle every `period_beats`.
    Rainbow { period_beats: f32, spread: f32 },
    /// Two colour groups (`alternate` odd/even, or `halves`); `flip_beats` > 0
    /// swaps the two colours on that clock (0 = static).
    Bank {
        pattern: BankPattern,
        color_a: Cow<'static, str>,
        color_b: Cow<'static, str>,
        flip_beats: f32,
    },
    /// A fill front sweeping left->right every `period_beats`, the filled cells
    /// taking the cycle's colour; `clear` true resets to black behind the front
    /// each cycle (a build/fill), false wipes the new colour over the previous.
    Wipe {
        period_beats: f32,
        palette: Cow<'static, str>,
        clear: bool,
    },
    /// Every cell held one colour; `spread` > 0 spreads the gradient across the
    /// cells (0 = a single flat colour — the 24-channel "wash").
    Solid { palette: Cow<'static, str>, spread: f32 },
    /// All cells flash the palette colour on a `duty` cycle every `period_beats`;
    /// `sparkle` makes each cell flash independently at random instead.
    Strobe {
        period_beats: f32,
        duty: f32,
        palette: Cow<'static, str>,
        sparkle: bool,
    },
    /// Two-source wave interference — the project's signature visual on pixel
    /// bars. Emitters sit at cells 0 and n-1; each cell's brightness follows the
    /// cosine of its path difference, so traveling node/antinode ripples sweep
    /// the bar. `lambda_cells` is the wavelength in cells (smaller = tighter
    /// bands); one full ripple cycle takes `period_beats`.
    Interference {
        lambda_cells: f32,
        period_beats: f32,
        palette: Cow<'static, str>,
    },
}

/// Parsed `values_from` path. Today's only supported root is
/// `source_attribute.<setting>.steps`, where `<setting>` selects which
/// rich-detail channel on the fixture profile to pull steps from
/// (`goboSetting` / `prismSetting` / `colorWheelSetting`).
#[derive(Clone, Debug)]
struct StepValuesPath {
    /// e.g. "goboSetting" / "prismSetting" / "colorWheelSetting"
    setting: String,
}

/// Variable schema from `launch_contract.variables` (COM-499 A.9). The server
/// does not clamp; the executor clamps numeric `${var}` substitutions against
/// this before they reach a binding. `allowed[]` (when present) wins over min/max.
#[derive(Clone, Debug)]
struct VarSpec {
    min: Option<f64>,
    max: Option<f64>,
    allowed: Option<Vec<f64>>,
}

/// Build the `{ name -> VarSpec }` map from the launch-contract variable schema,
/// keeping only entries that actually constrain a value (min/max or allowed).
fn parse_var_specs(
    schema: &serde_json::Map<String, serde_json::Value>,
) -> HashMap<String, VarSpec> {
    let mut out = HashMap::new();
    for (name, v) in schema {
        let Some(obj) = v.as_object() else { continue };
        let min = obj.get("min").and_then(|x| x.as_f64());
        let max = obj.get("max").and_then(|x| x.as_f64());
        let allowed = obj
            .get("allowed")
            .and_then(|x| x.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_f64()).collect::<Vec<f64>>())
            .filter(|a| !a.is_empty());
        if min.is_some() || max.is_some() || allowed.is_some() {
            out.insert(name.clone(), VarSpec { min, max, allowed });
        }
    }
    out
}

/// Snap to the nearest `allowed[]` entry when present, else clamp into
/// `[min, max]`. Unchanged when the spec constrains neither.
fn clamp_to_spec(spec: &VarSpec, raw: f64) -> f64 {
    if let Some(allowed) = spec.allowed.as_ref() {
        return allowed
            .iter()
            .copied()
            .min_by(|a, b| (a - raw).abs().total_cmp(&(b - raw).abs()))
            .unwrap_or(raw);
    }
    match (spec.min, spec.max) {
        // Normalize an inverted (min > max) spec so clamping stays deterministic
        // and lands inside [lo, hi] rather than failing both bounds. `a.min(b) <=
        // a.max(b)` always holds, so `f64::clamp` cannot panic.
        (Some(a), Some(b)) => raw.clamp(a.min(b), a.max(b)),
        (Some(a), None) => raw.max(a),
        (None, Some(b)) => raw.min(b),
        (None, None) => raw,
    }
}

#[derive(Clone, Debug)]
struct CueBinding {
    target_label: String,
    mode: ResolveMode,
    priority: i32,
    op: BindingOp,
}

/// A launched cue normalized into evaluable bindings, with the bindings that
/// could not be interpreted in slice 1 recorded for the status report.
#[derive(Clone, Debug)]
pub struct ActiveCue {
    pub launch_id: String,
    pub status: String,
    /// Consume-surface identity (COM-499) retained off the DTO so the executor
    /// and operator-facing logs can name what is running, not just its launch id.
    pub cue_profile_id: Option<String>,
    pub effect_id: Option<String>,
    pub stage_id: Option<i64>,
    /// `metadata.strobe_policy` — captured for the deferred strobe-authority slice
    /// (2h) to assert against; not enforced here.
    pub strobe_policy: Option<String>,
    pub runtime_role: Option<String>,
    /// Live variable snapshot the executor runs with, AFTER clamping each numeric
    /// to its `launch_contract.variables` spec (COM-499 A.9). Echoed back as
    /// `report.active_variables`, so the operator grid sees the values actually in
    /// effect — not an unclamped request.
    pub variables: serde_json::Map<String, serde_json::Value>,
    /// Out-of-range variables that were clamped/snapped, folded into
    /// `report.capability_misses` (A.9 "note any out-of-range value in the report").
    var_clamp_misses: Vec<CapabilityMiss>,
    /// Per-frame fade-out multiplier (1.0 normally; the renderer ramps it 1→0 on a
    /// retained clone while a stopped cue fades to black — COM-499 rule 8). Scales
    /// every resolved binding value.
    pub fade_factor: f32,
    bindings: Vec<CueBinding>,
    skipped_at_parse: Vec<String>,
}

impl ActiveCue {
    pub fn from_dto(dto: &CueDto) -> Self {
        // Variable schema (min/max/allowed) — distinct from the live values. The
        // server does not clamp, so the executor is the clamp authority (A.9).
        let var_specs = dto
            .launch_contract
            .as_ref()
            .map(|c| parse_var_specs(&c.variables))
            .unwrap_or_default();

        let mut bindings = Vec::new();
        let mut skipped = Vec::new();
        let empty: Vec<BindingDto> = Vec::new();
        let raw = dto
            .launch_contract
            .as_ref()
            .map(|c| &c.bindings)
            .unwrap_or(&empty);
        for b in raw {
            match normalize_binding(b, &dto.variables, &var_specs) {
                Ok(cb) => bindings.push(cb),
                Err(reason) => {
                    let id = if b.id.is_empty() { "<unnamed>" } else { &b.id };
                    skipped.push(format!("{}: {}", id, reason));
                }
            }
        }
        // Pre-sort by priority ONCE at parse time (stable, so authoring order
        // stays the tiebreak). `apply_cue_bindings` runs per fixture per frame
        // (~30Hz) and previously re-sorted a fresh Vec every call; with the
        // bindings pre-sorted the single-cue fast path iterates them directly
        // with zero per-frame allocation.
        bindings.sort_by_key(|b| b.priority);

        // Clamp the live snapshot the same way the bindings were clamped, so
        // report.active_variables reflects the values actually in effect, and
        // note any that were out of range (A.9). Non-numeric / unspecified vars
        // pass through untouched.
        let mut clamped_vars = serde_json::Map::new();
        let mut var_clamp_misses = Vec::new();
        for (name, val) in &dto.variables {
            match (var_specs.get(name), val.as_f64()) {
                (Some(spec), Some(raw_num)) => {
                    let clamped = clamp_to_spec(spec, raw_num);
                    // clamp_to_spec returns `raw` bit-identical when in range, so
                    // any nonzero delta is a genuine clamp/snap (> 0.0 dodges the
                    // f64::EPSILON boundary false-negative and clippy::float_cmp).
                    if (clamped - raw_num).abs() > 0.0 {
                        var_clamp_misses.push(CapabilityMiss {
                            target: format!("variable:{}", name),
                            reason: format!(
                                "value {} out of range, clamped to {}",
                                raw_num, clamped
                            ),
                        });
                    }
                    clamped_vars.insert(name.clone(), serde_json::json!(clamped));
                }
                _ => {
                    clamped_vars.insert(name.clone(), val.clone());
                }
            }
        }

        let (strobe_policy, runtime_role) = dto
            .metadata
            .as_ref()
            .map(|m| (m.strobe_policy.clone(), m.runtime_role.clone()))
            .unwrap_or((None, None));
        ActiveCue {
            launch_id: dto.launch_id.clone(),
            status: dto.status.clone(),
            cue_profile_id: dto.cue_profile_id.clone(),
            effect_id: dto.effect_id.clone(),
            stage_id: dto.stage_id,
            strobe_policy,
            runtime_role,
            variables: clamped_vars,
            var_clamp_misses,
            fade_factor: 1.0,
            bindings,
            skipped_at_parse: skipped,
        }
    }

    /// A cue contributes to frames while launched/active; stop states do not.
    /// A stopped cue's brief fade-out is driven by the renderer from a retained,
    /// render-only clone (forced runnable with a sub-1.0 `fade_factor`), so the
    /// real stopped cue does not need to be re-admitted here.
    pub fn is_runnable(&self) -> bool {
        matches!(self.status.as_str(), "ACTIVE" | "LAUNCH_REQUESTED")
    }

    /// Bindings dropped at parse time (unsupported source.kind or target.type in
    /// slice 1) — surfaced once per cue in `report.skipped_bindings`.
    pub fn parse_skips(&self) -> &[String] {
        &self.skipped_at_parse
    }

    /// COM-499 rules 2 & 4: validate the contract against this rig and reject a
    /// cue we cannot run AT ALL with a `rejected_reason`. "At all" means no
    /// binding parsed into something applicable — `Some(reason)` ⇒ reject.
    /// Partial capability (some bindings parse, others skip) is *accept*-then-skip
    /// per rule 6, so a cue with any runnable binding returns `None`. Evaluated at
    /// ack time, where only parse-level info exists; per-fixture capability misses
    /// surface later in report-back, not as a rejection.
    pub fn rejection_reason(&self) -> Option<String> {
        if !self.bindings.is_empty() {
            return None;
        }
        if self.skipped_at_parse.is_empty() {
            Some("launch_contract has no bindings".to_string())
        } else {
            Some(format!(
                "no runnable bindings for this rig: {}",
                self.skipped_at_parse.join("; ")
            ))
        }
    }

    /// Out-of-range variable clamps (COM-499 A.9) to fold into
    /// `report.capability_misses` so the operator sees what the executor adjusted.
    pub fn var_clamp_misses(&self) -> &[CapabilityMiss] {
        &self.var_clamp_misses
    }
}

fn normalize_binding(
    b: &BindingDto,
    vars: &serde_json::Map<String, serde_json::Value>,
    specs: &HashMap<String, VarSpec>,
) -> Result<CueBinding, String> {
    let ttype = b.target.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let kind = b.source.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let mode = ResolveMode::parse(
        b.resolve.get("mode").and_then(|v| v.as_str()).unwrap_or("override"),
    );
    let priority = b.resolve.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

    // Segmented colour target (RGBW_SEGMENTS / RGB_SEGMENTS via `channel_pattern`):
    // a per-cell effect the renderer maps onto the fixture's cells (COM-499 A.4).
    // The authored `segments` count is advisory — the real cell count comes from
    // the fixture profile at apply time.
    if b.target.get("channel_pattern").map_or(false, |v| v.is_array()) {
        let seg_kind = match kind {
            "segment_chase" => SegmentKind::Chase {
                direction: SegDir::parse(
                    b.source.get("direction").and_then(|v| v.as_str()).unwrap_or("left_to_right"),
                ),
                width: resolve_num(b.source.get("width"), vars, specs).unwrap_or(1.5).max(0.5),
                period_beats: resolve_num(b.source.get("period_beats"), vars, specs)
                    .unwrap_or(1.0)
                    .max(0.0001),
                palette: {
                    let p = resolve_str(b.source.get("palette"), vars);
                    if p.is_empty() { Cow::Borrowed("white") } else { Cow::Owned(p) }
                },
                background: {
                    let bg = resolve_str(b.source.get("background"), vars);
                    if bg.is_empty() { Cow::Borrowed("black") } else { Cow::Owned(bg) }
                },
                head_cycle: None,
            },
            "segment_rainbow" => SegmentKind::Rainbow {
                period_beats: resolve_num(b.source.get("period_beats"), vars, specs).unwrap_or(4.0).max(0.0001),
                spread: resolve_num(b.source.get("spread"), vars, specs).unwrap_or(1.0).max(0.0),
            },
            "segment_bank" => SegmentKind::Bank {
                pattern: BankPattern::parse(
                    b.source.get("pattern").and_then(|v| v.as_str()).unwrap_or("alternate"),
                ),
                color_a: {
                    let c = resolve_str(b.source.get("color_a"), vars);
                    if c.is_empty() { Cow::Borrowed("red") } else { Cow::Owned(c) }
                },
                color_b: {
                    let c = resolve_str(b.source.get("color_b"), vars);
                    if c.is_empty() { Cow::Borrowed("blue") } else { Cow::Owned(c) }
                },
                flip_beats: resolve_num(b.source.get("flip_beats"), vars, specs).unwrap_or(0.0).max(0.0),
            },
            "segment_wipe" => SegmentKind::Wipe {
                period_beats: resolve_num(b.source.get("period_beats"), vars, specs).unwrap_or(4.0).max(0.0001),
                palette: {
                    let p = resolve_str(b.source.get("palette"), vars);
                    if p.is_empty() { Cow::Borrowed("rainbow") } else { Cow::Owned(p) }
                },
                clear: b.source.get("clear").and_then(|v| v.as_bool()).unwrap_or(false),
            },
            "segment_solid" => SegmentKind::Solid {
                palette: {
                    let p = resolve_str(b.source.get("palette"), vars);
                    if p.is_empty() { Cow::Borrowed("white") } else { Cow::Owned(p) }
                },
                spread: resolve_num(b.source.get("spread"), vars, specs).unwrap_or(0.0).max(0.0),
            },
            "segment_strobe" => SegmentKind::Strobe {
                period_beats: resolve_num(b.source.get("period_beats"), vars, specs).unwrap_or(0.5).max(0.0001),
                duty: resolve_num(b.source.get("duty"), vars, specs).unwrap_or(0.5).clamp(0.05, 0.95),
                palette: {
                    let p = resolve_str(b.source.get("palette"), vars);
                    if p.is_empty() { Cow::Borrowed("white") } else { Cow::Owned(p) }
                },
                sparkle: b.source.get("sparkle").and_then(|v| v.as_bool()).unwrap_or(false),
            },
            // Two-source wave interference (the signature ripple): wavelength in
            // cells + ripple cycle length in beats, colour from the palette table.
            "segment_interference" => SegmentKind::Interference {
                lambda_cells: resolve_num(b.source.get("lambda_cells"), vars, specs)
                    .unwrap_or(4.0)
                    .max(0.5),
                period_beats: resolve_num(b.source.get("period_beats"), vars, specs)
                    .unwrap_or(2.0)
                    .max(0.0001),
                palette: {
                    let p = resolve_str(b.source.get("palette"), vars);
                    if p.is_empty() { Cow::Borrowed("white") } else { Cow::Owned(p) }
                },
            },
            other => {
                return Err(format!(
                    "segmented target needs a segment source kind, got '{}' (slice 2)",
                    other
                ))
            }
        };
        return Ok(CueBinding {
            target_label: format!("{} segments", ttype),
            mode,
            priority,
            op: BindingOp::Segment { kind: seg_kind },
        });
    }

    // Multi-channel colour target (RGB / RGBW / RGBAWUV): a static `palette`
    // colour, or a `color_strobe` that flashes that colour on a square duty cycle
    // (channel-cheap strobe for RGB-only fixtures with no dedicated strobe channel).
    if b.target.get("channel_names").map_or(false, |v| v.is_array()) {
        match kind {
            "palette" | "color_strobe" => {}
            other => {
                return Err(format!(
                    "colour target needs a palette or color_strobe source, got '{}' (slice 2)",
                    other
                ))
            }
        }
        let name = resolve_str(b.source.get("value"), vars);
        let rgb = palette_rgb(&name).ok_or_else(|| format!("unknown palette colour '{}'", name))?;
        if kind == "color_strobe" {
            let period_beats = resolve_num(b.source.get("period_beats"), vars, specs)
                .unwrap_or(0.25)
                .max(0.0001);
            let duty = resolve_num(b.source.get("duty"), vars, specs)
                .unwrap_or(0.5)
                .clamp(0.05, 0.95);
            return Ok(CueBinding {
                target_label: format!("{} strobe ({})", ttype, name),
                mode,
                priority,
                op: BindingOp::ColorStrobe { rgb, period_beats, duty },
            });
        }
        return Ok(CueBinding {
            target_label: format!("{} ({})", ttype, name),
            mode,
            priority,
            op: BindingOp::Color { rgb },
        });
    }

    // Single-channel scalar target.
    let channel_name = b.target.get("channel_name").and_then(|v| v.as_str()).unwrap_or("");
    let target_label = if channel_name.is_empty() {
        ttype.to_string()
    } else {
        channel_name.to_string()
    };
    let attr = IntentAttr::from_type(ttype)
        .ok_or_else(|| format!("unsupported target.type '{}' (slice 2)", ttype))?;
    if kind == "step_chase" {
        // step_chase has its own BindingOp because values_from needs
        // per-fixture profile resolution at apply time (each fixture has
        // its own step list under goboSetting / prismSetting / ...).
        let period_beats =
            resolve_num(b.source.get("period_beats"), vars, specs).unwrap_or(1.0).max(0.0001);
        let inline_values = b
            .source
            .get("values")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_f64().map(|n| n as f32)).collect());
        let values_from = b
            .source
            .get("values_from")
            .and_then(|v| v.as_str())
            .and_then(parse_step_values_path);
        if inline_values.is_none() && values_from.is_none() {
            return Err("step_chase needs `values` or `values_from`".to_string());
        }
        return Ok(CueBinding {
            target_label,
            mode,
            priority,
            op: BindingOp::StepChase {
                attr,
                period_beats,
                inline_values,
                values_from,
            },
        });
    }

    let source = match kind {
        "constant" => Source::Constant(
            resolve_num(b.source.get("value"), vars, specs)
                .ok_or_else(|| "constant.value unresolved".to_string())?,
        ),
        "oscillator" => Source::Oscillator {
            shape: parse_shape(b.source.get("wave").and_then(|v| v.as_str()).unwrap_or("sine")),
            period_beats: resolve_num(b.source.get("period_beats"), vars, specs).unwrap_or(4.0),
            depth: resolve_num(b.source.get("depth"), vars, specs).unwrap_or(0.0),
            center: resolve_num(b.source.get("center"), vars, specs).unwrap_or(0.0),
        },
        // react_strobe_gate: the contract (A.10) has this follow the operator's
        // REACT strobe slider. That manual slider does not exist in the executor
        // yet (deferred with the 2h strobe-authority cap), so the minimal,
        // product-acceptable behavior is to ENGAGE the strobe/shutter channel —
        // the cue's `value: "react_strobe_slider"` request resolves to "on" — so
        // the strobe actually fires instead of the binding being skipped. When the
        // operator slider lands (2h), this becomes the slider value at top priority.
        "react_strobe_gate" => Source::Constant(1.0),
        other => return Err(format!("unsupported source.kind '{}' (slice 2)", other)),
    };
    Ok(CueBinding {
        target_label,
        mode,
        priority,
        op: BindingOp::Single { attr, source },
    })
}

/// Parse a `values_from` path. Today only `source_attribute.<setting>.steps`
/// is supported. Returns `None` on unrecognized shape so the binding falls
/// through to a parse-skip with a descriptive message.
fn parse_step_values_path(s: &str) -> Option<StepValuesPath> {
    let rest = s.strip_prefix("source_attribute.")?;
    let setting = rest.strip_suffix(".steps")?;
    if setting.is_empty() {
        return None;
    }
    Some(StepValuesPath {
        setting: setting.to_string(),
    })
}

/// Resolve a `values_from` path against a fixture profile to the raw step
/// slice. Picks the channel matching the target's name (case-insensitive,
/// sourced from `source_attribute.name` since `ChannelDefinition` doesn't
/// hoist the raw name), then pulls steps from its
/// `ChannelDetail::{Gobo,Prism,ColorWheel,Macro}` block. Returns an empty
/// slice when the channel/detail is absent.
///
/// Borrows straight from the profile — this runs per binding per fixture per
/// frame (~30Hz), and the previous `Vec<f32>` + `to_ascii_uppercase` version
/// allocated several strings + a Vec on every call. `eq_ignore_ascii_case`
/// compares in place; callers index enabled steps via [`step_norm_value`].
fn profile_steps<'a>(
    path: &StepValuesPath,
    profile: &'a FixtureProfile,
    target_channel_name: &str,
) -> &'a [ChannelSlot] {
    use crate::emerge::contracts::ChannelDetail;
    let channel = profile.channels.iter().find(|c| {
        c.source_attribute
            .as_ref()
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .map(|n| n.eq_ignore_ascii_case(target_channel_name))
            .unwrap_or(false)
    });
    let Some(channel) = channel else {
        return &[];
    };
    // Setting name in the path is informational — the channel's detail
    // shape is what actually carries the steps. We accept any of the
    // step-bearing details.
    let _ = &path.setting; // currently informational; kept for future routing
    match channel.detail.as_ref() {
        Some(ChannelDetail::Gobo { steps }) => steps,
        Some(ChannelDetail::Prism { steps }) => steps,
        Some(ChannelDetail::Macro { steps }) => steps,
        Some(ChannelDetail::ColorWheel { slots, .. }) => slots,
        _ => &[],
    }
}

/// Normalize one profile step to 0..1. ATLABASE profiles store
/// `low_value`/`high_value` as raw DMX BYTES (the chauvet obsession gobo steps
/// run low 0..130 / high 9..139), so the midpoint must be divided by 255
/// exactly like the `value` fallback — without the division every midpoint
/// exceeded 1.0, `IntentAttr::set` clamped them all to 1.0, and profile-driven
/// chases froze on the last wheel position.
fn step_norm_value(s: &ChannelSlot) -> f32 {
    match (s.low_value, s.high_value) {
        (Some(lo), Some(hi)) => ((((lo + hi) * 0.5) / 255.0).clamp(0.0, 1.0)) as f32,
        _ => s.value as f32 / 255.0,
    }
}

/// Resolve a number that may be a literal or a `${var}` reference into the cue's
/// merged live variables (defaults already merged server-side). A `${var}`
/// substitution is clamped/snapped to its `VarSpec` (COM-499 A.9) before use; a
/// literal is author-controlled and passes through unclamped.
fn resolve_num(
    v: Option<&serde_json::Value>,
    vars: &serde_json::Map<String, serde_json::Value>,
    specs: &HashMap<String, VarSpec>,
) -> Option<f32> {
    match v? {
        serde_json::Value::Number(n) => n.as_f64().map(|x| x as f32),
        serde_json::Value::String(s) => {
            let key = s.trim();
            let key = key
                .strip_prefix("${")
                .and_then(|k| k.strip_suffix('}'))
                .unwrap_or(key);
            let raw = vars.get(key).and_then(|vv| vv.as_f64())?;
            let val = match specs.get(key) {
                Some(spec) => clamp_to_spec(spec, raw),
                None => raw,
            };
            Some(val as f32)
        }
        _ => None,
    }
}

/// Resolve a string that may be a literal or a `${var}` reference into the cue's
/// merged variables — palette colour names come through here.
fn resolve_str(
    v: Option<&serde_json::Value>,
    vars: &serde_json::Map<String, serde_json::Value>,
) -> String {
    match v {
        Some(serde_json::Value::String(s)) => {
            let k = s.trim();
            match k.strip_prefix("${").and_then(|k| k.strip_suffix('}')) {
                Some(name) => vars.get(name).and_then(|vv| vv.as_str()).unwrap_or(name).to_string(),
                None => k.to_string(),
            }
        }
        _ => String::new(),
    }
}

/// Slice-2a starter palette: the named colours the v1 starter effects allow,
/// mapped to RGB. (Amber/UV channels are slice 2f.) Unknown names skip-and-report.
fn palette_rgb(name: &str) -> Option<[f32; 3]> {
    Some(match name {
        "warm_magenta" => [1.0, 0.0, 0.6],
        "deep_blue" => [0.0, 0.15, 1.0],
        "amber_white" => [1.0, 0.6, 0.25],
        "uv_blue" => [0.35, 0.0, 1.0],
        "white" => [1.0, 1.0, 1.0],
        "red" => [1.0, 0.0, 0.0],
        "green" => [0.0, 1.0, 0.0],
        "blue" => [0.0, 0.0, 1.0],
        _ => return None,
    })
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Local HSV->RGB (h/s/v in 0..1) for the `rainbow` gradient palette.
pub(crate) fn hsv_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = (h.rem_euclid(1.0) * 6.0).clamp(0.0, 5.999);
    let i = h.floor();
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    match i as u8 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// Resolve a colour/gradient name at position `t` in 0..1. `rainbow` cycles hue
/// with `t`; two-colour gradients interpolate; a single palette colour (or
/// "black") ignores `t`. Extends the single-colour `palette_rgb` table so segment
/// effects can spread colour across the cells.
fn gradient_rgb(name: &str, t: f32) -> [f32; 3] {
    match name {
        "rainbow" => hsv_rgb(t, 1.0, 1.0),
        "blue_white" => lerp3([0.0, 0.1, 1.0], [1.0, 1.0, 1.0], t),
        "red_white" => lerp3([1.0, 0.0, 0.0], [1.0, 1.0, 1.0], t),
        "warm" => lerp3([1.0, 0.45, 0.0], [1.0, 0.85, 0.55], t),
        "black" | "" => [0.0, 0.0, 0.0],
        single => palette_rgb(single).unwrap_or([1.0, 1.0, 1.0]),
    }
}

/// Evaluate a `SegmentKind` to cell `i`'s RGB (of `n` cells) at `beats`.
///
/// `beats` is the monotonic f64 show-clock beat count; every cycle/phase
/// reduction (`.fract()` / `.floor()` / `.rem_euclid(1.0)`) happens in f64
/// BEFORE narrowing to f32, so multi-day uptimes keep sub-frame phase
/// precision (see [`Source::eval`]). `pub(crate)` so the DMX writer evaluates
/// cells against a once-per-fixture-per-frame kind.
pub(crate) fn eval_segment(kind: &SegmentKind, i: usize, n: usize, beats: f64) -> [f32; 3] {
    let n = n.max(1);
    match kind {
        SegmentKind::Chase {
            direction,
            width,
            period_beats,
            palette,
            background,
            head_cycle,
        } => {
            let cycle = head_cycle
                .unwrap_or_else(|| beats.max(0.0) / (*period_beats as f64).max(0.0001));
            let phase = cycle.fract() as f32;
            let last = (n.saturating_sub(1)).max(1) as f32;
            // Head position; `wrap` = distance loops around the bar (sweep modes).
            let (head, wrap) = match direction {
                SegDir::LeftToRight => (phase * n as f32, true),
                SegDir::RightToLeft => ((1.0 - phase) * n as f32, true),
                SegDir::Bounce => {
                    let t = ((cycle * 0.5).rem_euclid(1.0) * 2.0) as f32;
                    let tri = if t < 1.0 { t } else { 2.0 - t };
                    (tri * last, false)
                }
                SegDir::CenterOut => (phase * (n as f32 / 2.0), false),
            };
            let w = width.max(0.5);
            let lit = if matches!(direction, SegDir::CenterOut) {
                let center = last / 2.0;
                let d = ((i as f32 - center).abs() - head).abs();
                (1.0 - d / w).clamp(0.0, 1.0)
            } else {
                let mut d = (i as f32 - head).abs();
                if wrap {
                    d = d.min(n as f32 - d);
                }
                (1.0 - d / w).clamp(0.0, 1.0)
            };
            // Head colour cycles with the sweep for gradient palettes (a rainbow
            // comet); a single colour stays constant. Background dimly fills the
            // unlit cells (default black = off behind the comet).
            let head_color = gradient_rgb(palette, phase);
            let bg = gradient_rgb(background, i as f32 / last);
            let bg_level = if matches!(background.as_ref(), "black" | "") { 0.0 } else { 0.22 };
            [
                bg[0] * bg_level + head_color[0] * lit,
                bg[1] * bg_level + head_color[1] * lit,
                bg[2] * bg_level + head_color[2] * lit,
            ]
        }
        SegmentKind::Rainbow { period_beats, spread } => {
            // Scroll reduced mod 1 in f64: hue is periodic, and feeding a huge
            // raw beat ratio into the f32 hue would shed all its fraction bits.
            let scroll = (beats.max(0.0) / (*period_beats as f64).max(0.0001)).fract() as f32;
            let last = (n.saturating_sub(1)).max(1) as f32;
            let hue = (i as f32 / last) * spread + scroll;
            hsv_rgb(hue, 1.0, 1.0)
        }
        SegmentKind::Bank {
            pattern,
            color_a,
            color_b,
            flip_beats,
        } => {
            let last = (n.saturating_sub(1)).max(1) as f32;
            let group = match pattern {
                BankPattern::Alternate => i % 2,
                BankPattern::Halves => {
                    if (i as f32) < n as f32 / 2.0 {
                        0
                    } else {
                        1
                    }
                }
            };
            let flip = if *flip_beats > 0.0 {
                ((beats.max(0.0) / *flip_beats as f64).floor() as i64).rem_euclid(2) as usize
            } else {
                0
            };
            let name = if (group + flip) % 2 == 0 { color_a } else { color_b };
            gradient_rgb(name, i as f32 / last)
        }
        SegmentKind::Wipe {
            period_beats,
            palette,
            clear,
        } => {
            let cycle = beats.max(0.0) / (*period_beats as f64).max(0.0001);
            let front = cycle.fract() as f32 * n as f32;
            // The filled colour cycles each pass (golden ratio step) so a gradient
            // palette wipes in a fresh hue each sweep; a single colour stays put.
            let pass = cycle.floor();
            let cur = gradient_rgb(palette, ((pass * 0.381_966).rem_euclid(1.0)) as f32);
            if (i as f32) < front {
                cur
            } else if *clear {
                [0.0, 0.0, 0.0]
            } else {
                gradient_rgb(palette, (((pass - 1.0) * 0.381_966).rem_euclid(1.0)) as f32)
            }
        }
        SegmentKind::Solid { palette, spread } => {
            let last = (n.saturating_sub(1)).max(1) as f32;
            gradient_rgb(palette, (i as f32 / last) * spread)
        }
        SegmentKind::Strobe {
            period_beats,
            duty,
            palette,
            sparkle,
        } => {
            let cycle = beats.max(0.0) / (*period_beats as f64).max(0.0001);
            let last = (n.saturating_sub(1)).max(1) as f32;
            let on = if *sparkle {
                hash01(i as i64, cycle.floor() as i64) < *duty
            } else {
                (cycle.fract() as f32) < *duty
            };
            if on {
                gradient_rgb(palette, i as f32 / last)
            } else {
                [0.0, 0.0, 0.0]
            }
        }
        SegmentKind::Interference {
            lambda_cells,
            period_beats,
            palette,
        } => {
            // Emitters at cells 0 and n-1; path difference d = |i-0| - |i-(n-1)|
            // = 2i - (n-1). Brightness follows the cosine of the path difference
            // (in wavelengths) minus the traveling-phase term, so node/antinode
            // ripples sweep the bar once per `period_beats`. The phase term is
            // reduced mod 1 in f64 (cosine is periodic); d/lambda is bounded by
            // the cell count and safe in f32.
            let last = (n.saturating_sub(1)).max(1) as f32;
            let d = 2.0 * i as f32 - last;
            let travel = (beats.max(0.0) / (*period_beats as f64).max(0.0001)).fract() as f32;
            let b = 0.5 + 0.5 * (std::f32::consts::TAU * (d / lambda_cells.max(0.5) - travel)).cos();
            let c = gradient_rgb(palette, i as f32 / last);
            [c[0] * b, c[1] * b, c[2] * b]
        }
    }
}

/// Deterministic pseudo-random in [0,1) from two integers — the `sparkle` segment
/// effect uses it so each (cell, time-step) flashes independently but stably.
fn hash01(a: i64, b: i64) -> f32 {
    let mut h =
        (a as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (b as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

// ── Slice-3: tension_bank segment-effect engine ─────────────────────────────
// The autonomous segment look EMERGES from the harmonic charge/discharge curve,
// not a counter: a low tension_bank shows a calm slow rainbow; as the bank fills
// toward a drop the look tightens (a fill/wipe, then a fast chase that tightens
// further as the bank keeps filling); a real drop_impact RELEASES to the most
// intense look. So a section that never builds never erupts, and the drop reads
// as a genuine release. These are the future MIDI-bindable knobs.
const AUTO_SEG_DROP_RELEASE: f32 = 0.6;
const AUTO_SEG_TENSION_HIGH: f32 = 0.42;
const AUTO_SEG_TENSION_MID: f32 = 0.3;
/// Realistic pre-drop tension peak (the bank tops out ~0.6-0.7 before a drop
/// spends it down). The tightening chase normalizes its period over [HIGH, PEAK]
/// so a real build actually reaches the fast end, instead of needing an
/// unreachable ~0.9 (review finding: the tightening stage barely engaged).
const AUTO_SEG_TENSION_PEAK: f32 = 0.7;

/// Tightening-chase period in beats for a given `tension_bank` — 1.0 at the
/// HIGH edge, shortening to 0.4 at PEAK. Shared by [`auto_segment_kind`] (the
/// look) and [`advance_auto_chase`] (the integrator) so the period the head
/// advances at can never drift from the period the look claims.
pub(crate) fn auto_chase_period(tension_bank: f32) -> f32 {
    let t = ((tension_bank - AUTO_SEG_TENSION_HIGH)
        / (AUTO_SEG_TENSION_PEAK - AUTO_SEG_TENSION_HIGH))
        .clamp(0.0, 1.0);
    1.0 - 0.6 * t // 1.0 .. 0.4 beats over [HIGH, PEAK]
}

/// Advance the rig-wide integrated chase-cycle accumulator by the beats
/// elapsed since the last frame, at the CURRENT tension-tightened period.
/// `fract(cycle)` is then a head phase that ACCELERATES as the period
/// shortens instead of teleporting (`fract(abs_beats / period)` rescales the
/// whole multi-thousand-beat quotient on every period change). Returns the
/// new `(cycle, last_beat)` pair.
///
/// - Backwards/huge beat deltas (clock rebase, source change) advance at most
///   one beat — the head glides through the seam instead of jumping.
/// - The cycle wraps at 2.0: `eval_segment` consumes `fract(cycle)` and
///   Bounce consumes `rem_euclid(cycle * 0.5, 1.0)`, and both are continuous
///   across a wrap of exactly 2 — while the accumulator stays small enough to
///   keep full f64 fraction precision over multi-day uptimes.
pub(crate) fn advance_auto_chase(
    cycle: f64,
    last_beat: f64,
    beat: f64,
    tension_bank: f32,
) -> (f64, f64) {
    let db = (beat - last_beat).clamp(0.0, 1.0);
    let period = auto_chase_period(tension_bank).max(0.0001) as f64;
    ((cycle + db / period).rem_euclid(2.0), beat)
}

/// Select the autonomous segment effect from where the music sits on the
/// tension charge/discharge curve. `look_index` now only rotates the palette.
/// `chase_cycle` is the rig-wide integrated head cycle (see
/// [`advance_auto_chase`]) the tightening chase sweeps on.
///
/// Palettes are `Cow::Borrowed` statics and the kind is built ONCE per fixture
/// per frame by the DMX writer (then evaluated per cell via [`eval_segment`]) —
/// the previous shape rebuilt it per CELL, allocating the palette string ~24×
/// per bar per frame on the ~30Hz render path.
pub(crate) fn auto_segment_kind(
    look_index: u32,
    tension_bank: f32,
    drop_impact: f32,
    chase_cycle: f64,
) -> SegmentKind {
    const PALETTES: [&str; 6] = ["rainbow", "blue_white", "warm", "red_white", "deep_blue", "white"];
    let palette = Cow::Borrowed(PALETTES[look_index as usize % PALETTES.len()]);
    if drop_impact > AUTO_SEG_DROP_RELEASE {
        // Release: the bank discharges — the rig fires its most intense look. The
        // sparkle desyncs each cell, so it is not a full-field flash (per Koob's
        // "don't gate segment-FX for photosensitivity" call).
        SegmentKind::Strobe {
            period_beats: 0.5,
            duty: 0.5,
            palette,
            sparkle: true,
        }
    } else if tension_bank > AUTO_SEG_TENSION_HIGH {
        // Tightening toward a drop: a fast chase whose period shortens further as
        // the bank keeps filling, so the escalation is smooth, not stepped. The
        // head sweeps on the INTEGRATED cycle — a varying period over absolute
        // beats teleports it (review finding: the build chase skipped
        // pseudo-randomly between cells instead of accelerating).
        SegmentKind::Chase {
            direction: SegDir::LeftToRight,
            width: 1.5,
            period_beats: auto_chase_period(tension_bank),
            palette,
            background: Cow::Borrowed("black"),
            head_cycle: Some(chase_cycle),
        }
    } else if tension_bank > AUTO_SEG_TENSION_MID {
        // Building: two-source wave interference (the signature ripple) whose
        // bands COMPRESS as the build charges — lambda tightens from ~8 cells
        // toward ~3 across [MID, HIGH], so the bar visibly winds up before the
        // tightening chase takes over.
        let t = ((tension_bank - AUTO_SEG_TENSION_MID)
            / (AUTO_SEG_TENSION_HIGH - AUTO_SEG_TENSION_MID))
            .clamp(0.0, 1.0);
        SegmentKind::Interference {
            lambda_cells: 8.0 - 5.0 * t, // 8 .. 3 cells over [MID, HIGH]
            period_beats: 4.0,
            palette,
        }
    } else {
        // Calm: a slow rainbow drift — period/spread tuned so it visibly scrolls
        // at low tension instead of reading as a frozen gradient (review finding).
        SegmentKind::Rainbow {
            period_beats: 12.0,
            spread: 1.5,
        }
    }
}

/// Operator-driven segment look (used when a fixture's `segment_fx` is not
/// `Auto`): map the chosen [`SegmentPattern`] to a `SegmentKind` for the DMX
/// writer to build once per fixture per frame and evaluate per cell via
/// [`eval_segment`]. `speed` is the cycle length in beats (lower = faster);
/// `look_index` rotates the palette, same as the autopilot. All palettes are
/// `Cow::Borrowed` statics — zero allocation on the render path.
pub(crate) fn operator_segment_kind(
    pattern: crate::emerge::contracts::SegmentPattern,
    speed: f32,
    look_index: u32,
) -> SegmentKind {
    use crate::emerge::contracts::SegmentPattern as P;
    const PALETTES: [&str; 6] = ["rainbow", "blue_white", "warm", "red_white", "deep_blue", "white"];
    let palette = Cow::Borrowed(PALETTES[look_index as usize % PALETTES.len()]);
    let period = speed.max(0.1);
    match pattern {
        // Auto is handled by the caller (autopilot path); never reached in
        // practice — kept as flat black for parity with the previous behavior.
        P::Auto => SegmentKind::Solid { palette: Cow::Borrowed("black"), spread: 0.0 },
        P::Chase => SegmentKind::Chase {
            direction: SegDir::LeftToRight,
            width: 1.5,
            period_beats: period,
            palette,
            background: Cow::Borrowed("black"),
            head_cycle: None,
        },
        P::ChaseReverse => SegmentKind::Chase {
            direction: SegDir::RightToLeft,
            width: 1.5,
            period_beats: period,
            palette,
            background: Cow::Borrowed("black"),
            head_cycle: None,
        },
        P::Bounce => SegmentKind::Chase {
            direction: SegDir::Bounce,
            width: 1.5,
            period_beats: period,
            palette,
            background: Cow::Borrowed("black"),
            head_cycle: None,
        },
        P::CenterOut => SegmentKind::Chase {
            direction: SegDir::CenterOut,
            width: 1.5,
            period_beats: period,
            palette,
            background: Cow::Borrowed("black"),
            head_cycle: None,
        },
        P::Rainbow => SegmentKind::Rainbow { period_beats: period * 4.0, spread: 1.5 },
        P::Wipe => SegmentKind::Wipe { period_beats: period * 2.0, palette, clear: true },
        P::Strobe => SegmentKind::Strobe { period_beats: period, duty: 0.5, palette, sparkle: true },
        // Two alternating colour groups flipping on the speed clock (a bank flash).
        P::Bank => SegmentKind::Bank {
            pattern: BankPattern::Alternate,
            color_a: palette,
            color_b: Cow::Borrowed("white"),
            flip_beats: period,
        },
        // Every cell one steady colour from the look palette — no motion.
        P::Solid => SegmentKind::Solid { palette, spread: 0.0 },
    }
}

/// Compose one resolved value onto the current intent value per the resolve mode.
fn compose(mode: ResolveMode, current: f32, value: f32) -> f32 {
    match mode {
        ResolveMode::Override => value.clamp(0.0, 1.0),
        ResolveMode::Max => current.max(value.clamp(0.0, 1.0)),
        ResolveMode::Add => add_bipolar(current, value),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CapabilityMiss {
    pub target: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct CueApplyReport {
    pub applied_bindings: usize,
    pub capability_misses: Vec<CapabilityMiss>,
    /// ISO-8601 UTC wall-clock time of the last frame the executor applied this
    /// cue (COM-499 `report.last_frame_at`). Stamped by the renderer each frame
    /// (`mod.rs::update`); `None` until the first frame renders for the cue.
    pub last_frame_at: Option<String>,
    /// Per-cue apply error (COM-499 `report.error`). `None` on success — the
    /// field is always emitted so the operator grid can distinguish "healthy"
    /// from "no report yet".
    pub error: Option<String>,
}

/// Per-fixture cue outputs that don't fit in `FixtureIntentState`, returned by
/// [`apply_cue_bindings`] for the DMX writer.
#[derive(Debug, Default)]
pub struct CueOverlayOut {
    /// Per-cell RGB for a segmented fixture, built when a Segment binding
    /// applied; the renderer writes them to the fixture's cells.
    pub segments: Option<Vec<[f32; 3]>>,
    /// Exact 0..1 gobo wheel value resolved by a profile-step chase. Real
    /// wheels (chauvet obsession: 14 steps, 10-DMX-wide ranges) don't fit the
    /// `gobo_slot` u8 ×32 emission spread — quantizing to 8 slots can land a
    /// step one wheel position off — so the writer emits `round(v·255)` when
    /// this is present, hitting the profile step's exact byte range.
    pub gobo_exact: Option<f32>,
}

/// Overlay runnable cues onto a fixture's intent (COM-499 Appendix A choice A:
/// intent-level composition). Bindings from ALL active cues are ordered together
/// by priority (ascending, so higher priority composes last and wins for
/// `override` — COM-499 A.8 global HTP/LTP across playbacks, not just within a
/// single cue); each composes per its resolve mode. Targets whose channel is
/// absent on the fixture are capability-gated out and reported. The caller (the
/// executor) reports parse-level skips once per cue via [`ActiveCue::parse_skips`].
///
/// Runs per fixture per frame (~30Hz). Each cue's bindings are pre-sorted by
/// priority at parse time ([`ActiveCue::from_dto`]), so the common single-cue
/// case iterates them directly with zero allocation; only overlapping cues
/// (operator stacking, rare) pay for a merged ordering Vec. `segment_cells` is
/// the fixture's per-cell count, resolved ONCE by the caller (the writer needs
/// the segment list anyway) instead of re-deriving it per binding here.
pub fn apply_cue_bindings(
    intent: &mut FixtureIntentState,
    fixture: &FixturePatch,
    profile: Option<&FixtureProfile>,
    segment_cells: usize,
    cues: &[ActiveCue],
    beats: f64,
    reports: &mut HashMap<String, CueApplyReport>,
) -> CueOverlayOut {
    // A runnable cue always gets a report entry (per-frame heartbeat + the worker
    // reads it), even if it applies nothing on this fixture. The renderer injects
    // fade-out clones as runnable cues with a sub-1.0 fade_factor. `contains_key`
    // first so the launch_id is only cloned on the first frame it appears.
    let mut runnable_count = 0usize;
    let mut single: Option<&ActiveCue> = None;
    for cue in cues.iter().filter(|c| c.is_runnable()) {
        runnable_count += 1;
        single = Some(cue);
        if !reports.contains_key(&cue.launch_id) {
            reports.insert(cue.launch_id.clone(), CueApplyReport::default());
        }
    }

    let mut out = CueOverlayOut::default();
    match runnable_count {
        0 => {}
        1 => {
            // Fast path (the steady state of a live show): one runnable cue,
            // bindings already priority-sorted — no per-frame Vec, no sort.
            let cue = single.expect("counted one runnable cue");
            for b in &cue.bindings {
                apply_one_binding(
                    intent,
                    fixture,
                    profile,
                    segment_cells,
                    beats,
                    &cue.launch_id,
                    cue.fade_factor,
                    b,
                    reports,
                    &mut out,
                );
            }
        }
        _ => {
            // COM-499 A.8: order bindings by priority across ALL active cues,
            // ascending so higher priority composes last and wins for `override`.
            // Inputs are pre-sorted per cue, so the stable sort is near-linear and
            // keeps cue order as the tiebreak (single-cue ordering unchanged).
            // Each binding carries its cue's fade_factor (rule 8) so a stopping
            // cue's contribution scales toward zero.
            let mut ordered: Vec<(&str, f32, &CueBinding)> = Vec::new();
            for cue in cues.iter().filter(|c| c.is_runnable()) {
                for b in &cue.bindings {
                    ordered.push((cue.launch_id.as_str(), cue.fade_factor, b));
                }
            }
            ordered.sort_by_key(|(_, _, b)| b.priority);
            for (launch_id, fade, b) in ordered {
                apply_one_binding(
                    intent,
                    fixture,
                    profile,
                    segment_cells,
                    beats,
                    launch_id,
                    fade,
                    b,
                    reports,
                    &mut out,
                );
            }
        }
    }

    out
}

/// Apply one resolved binding onto the intent — the shared body of the
/// single-cue fast path and the multi-cue merged ordering in
/// [`apply_cue_bindings`].
#[allow(clippy::too_many_arguments)]
fn apply_one_binding(
    intent: &mut FixtureIntentState,
    fixture: &FixturePatch,
    profile: Option<&FixtureProfile>,
    segment_cells: usize,
    beats: f64,
    launch_id: &str,
    fade: f32,
    b: &CueBinding,
    reports: &mut HashMap<String, CueApplyReport>,
    out: &mut CueOverlayOut,
) {
    let Some(report) = reports.get_mut(launch_id) else {
        return;
    };
    match &b.op {
        BindingOp::Single { attr, source } => {
            if !attr.channel_present(fixture) {
                report.capability_misses.push(CapabilityMiss {
                    target: b.target_label.clone(),
                    reason: format!("fixture '{}' has no {:?} channel", fixture.id, attr),
                });
                return;
            }
            attr.set(intent, compose(b.mode, attr.get(intent), source.eval(beats) * fade));
            // A slot-semantic gobo write must clear a stale profile-exact byte
            // from an earlier (lower-priority) binding, or emission would
            // prefer the stale exact value over this composition.
            if matches!(attr, IntentAttr::GoboSlot) {
                out.gobo_exact = None;
            }
            report.applied_bindings += 1;
        }
        BindingOp::Color { rgb } => {
            let ch = &fixture.channels;
            if ch.red.is_none() || ch.green.is_none() || ch.blue.is_none() {
                report.capability_misses.push(CapabilityMiss {
                    target: b.target_label.clone(),
                    reason: format!("fixture '{}' has no RGB channels", fixture.id),
                });
                return;
            }
            intent.red = compose(b.mode, intent.red, rgb[0] * fade);
            intent.green = compose(b.mode, intent.green, rgb[1] * fade);
            intent.blue = compose(b.mode, intent.blue, rgb[2] * fade);
            report.applied_bindings += 1;
        }
        BindingOp::ColorStrobe { rgb, period_beats, duty } => {
            let ch = &fixture.channels;
            if ch.red.is_none() || ch.green.is_none() || ch.blue.is_none() {
                report.capability_misses.push(CapabilityMiss {
                    target: b.target_label.clone(),
                    reason: format!("fixture '{}' has no RGB channels", fixture.id),
                });
                return;
            }
            // Square duty gate on the beat clock: full colour on the ON phase,
            // black on the OFF phase. NOTE: this flashes the RGB slots, NOT the
            // SHUTTER channel — so the deferred A.10 operator strobe-rate cap
            // (designed around SHUTTER strobing) will NOT gate it unless A.10 is
            // explicitly extended to BindingOp::ColorStrobe. Until then the rate
            // is operator-authored (period_beats) and UNCAPPED; cues default to a
            // safe ~2 Hz. TODO(A.10/COM-499): extend strobe-rate capping to cover
            // ColorStrobe RGB flashes (treat it as a strobe-class source).
            // Phase reduced in f64 so the duty gate stays beat-accurate over
            // multi-day uptimes (see `Source::eval`).
            let period = (*period_beats as f64).max(0.0001);
            let phase = (beats.max(0.0) / period).fract() as f32;
            let rgb_out = if phase < *duty { *rgb } else { [0.0, 0.0, 0.0] };
            intent.red = compose(b.mode, intent.red, rgb_out[0] * fade);
            intent.green = compose(b.mode, intent.green, rgb_out[1] * fade);
            intent.blue = compose(b.mode, intent.blue, rgb_out[2] * fade);
            report.applied_bindings += 1;
        }
        BindingOp::StepChase {
            attr,
            period_beats,
            inline_values,
            values_from,
        } => {
            if !attr.channel_present(fixture) {
                report.capability_misses.push(CapabilityMiss {
                    target: b.target_label.clone(),
                    reason: format!("fixture '{}' has no {:?} channel", fixture.id, attr),
                });
                return;
            }
            // Resolve values WITHOUT allocating (this is the per-frame path):
            // inline wins (legacy / hand-authored chases) and is borrowed; else
            // the step list is read in place off the fixture's profile attribute
            // matching the target channel name (enabled steps only).
            enum Steps<'a> {
                Inline(&'a [f32]),
                Profile(&'a [ChannelSlot]),
            }
            let steps = if let Some(v) = inline_values.as_deref() {
                Steps::Inline(v)
            } else if let (Some(path), Some(prof)) = (values_from.as_ref(), profile) {
                Steps::Profile(profile_steps(path, prof, &b.target_label))
            } else {
                Steps::Inline(&[])
            };
            let len = match &steps {
                Steps::Inline(v) => v.len(),
                Steps::Profile(s) => s.iter().filter(|s| s.enabled).count(),
            };
            if len == 0 {
                report.capability_misses.push(CapabilityMiss {
                    target: b.target_label.clone(),
                    reason: format!(
                        "fixture '{}' has no step values for '{}' (profile or values_from unresolved)",
                        fixture.id, b.target_label
                    ),
                });
                return;
            }
            let period = (*period_beats as f64).max(0.0001);
            let idx = ((beats.max(0.0) / period).floor() as usize) % len;
            let (value, from_profile) = match &steps {
                Steps::Inline(v) => (v[idx], false),
                Steps::Profile(s) => (
                    s.iter()
                        .filter(|s| s.enabled)
                        .nth(idx)
                        .map(step_norm_value)
                        .unwrap_or(0.0),
                    true,
                ),
            };
            let composed = compose(b.mode, attr.get(intent), value * fade);
            attr.set(intent, composed);
            // Profile steps name REAL wheel positions with exact byte ranges —
            // carry the composed value past the 8-slot u8 quantization so the
            // writer can emit the exact byte (see `CueOverlayOut::gobo_exact`).
            // Inline values keep historic slot semantics — and must CLEAR a
            // stale exact byte left by an earlier (lower-priority) profile
            // binding, so the last composition wins on the wire.
            if matches!(attr, IntentAttr::GoboSlot) {
                out.gobo_exact = if from_profile { Some(composed) } else { None };
            }
            report.applied_bindings += 1;
        }
        BindingOp::Segment { kind } => {
            // Per-cell colour for a segmented fixture. The cell count is resolved
            // once by the caller (`segments_for_profile(...).len()`); a
            // non-segmented fixture (no per-cell channels) is a capability miss.
            let n = segment_cells;
            if n < 2 {
                report.capability_misses.push(CapabilityMiss {
                    target: b.target_label.clone(),
                    reason: format!(
                        "fixture '{}' is not segmented (no per-cell channels)",
                        fixture.id
                    ),
                });
                return;
            }
            let buf = out
                .segments
                .get_or_insert_with(|| vec![[0.0_f32, 0.0, 0.0]; n]);
            if buf.len() != n {
                buf.resize(n, [0.0, 0.0, 0.0]);
            }
            for (i, cell) in buf.iter_mut().enumerate() {
                let c = eval_segment(kind, i, n, beats);
                cell[0] = compose(b.mode, cell[0], c[0] * fade);
                cell[1] = compose(b.mode, cell[1], c[1] * fade);
                cell[2] = compose(b.mode, cell[2], c[2] * fade);
            }
            report.applied_bindings += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUE_JSON: &str = r#"{"cues":[{
        "launch_id":"L1","status":"ACTIVE",
        "variables":{"intensity":0.8,"pan_swing":0.2},
        "launch_contract":{"bindings":[
            {"id":"dim","target":{"channel_name":"Dimming","type":"DIMMER"},
             "source":{"kind":"constant","value":"${intensity}"},
             "resolve":{"mode":"override","priority":40}},
            {"id":"pan","target":{"channel_name":"Pan","type":"PAN"},
             "source":{"kind":"oscillator","wave":"sine","period_beats":16,"depth":"${pan_swing}"},
             "resolve":{"mode":"add","priority":30}},
            {"id":"gobo","target":{"channel_name":"Gobo","type":"GOBO"},
             "source":{"kind":"step_chase","values_from":"source_attribute.goboSetting.steps","period_beats":4,"clock":"beat"},
             "resolve":{"mode":"override","priority":60}},
            {"id":"chase","target":{"channel_name":"Dimming","type":"DIMMER"},
             "source":{"kind":"step_chase","values":[0.0,1.0],"period_beats":1},
             "resolve":{"mode":"max","priority":20}}
        ]}
    }]}"#;

    fn parse_one() -> ActiveCue {
        let list: CueListResponse = serde_json::from_str(CUE_JSON).unwrap();
        ActiveCue::from_dto(&list.cues[0])
    }

    #[test]
    fn normalizes_all_slice_2c_bindings() {
        let cue = parse_one();
        // Slice 2c: constant DIMMER + oscillator PAN + step_chase GOBO
        // (via values_from) + step_chase DIMMER (inline values) — all four
        // bindings now parse cleanly. No parse-time skips left.
        assert_eq!(cue.bindings.len(), 4);
        assert_eq!(
            cue.parse_skips().len(),
            0,
            "no parse skips expected, got {:?}",
            cue.parse_skips()
        );
    }

    #[test]
    fn consume_surface_fields_are_retained() {
        // Slice 2e: the full COM-499 consume surface must survive deserialize.
        // cue_profile_id / effect_id / stage_id / metadata.{strobe_policy,
        // runtime_role} / ack / report were previously dropped because CueDto
        // omitted the fields — serde silently discarded them.
        let json = r#"{"cues":[{
            "launch_id":"L9","status":"ACTIVE",
            "cue_profile_id":"warm-wash-pulse","effect_id":"flat-par-wash-pulse","stage_id":42,
            "variables":{"intensity":0.82},
            "metadata":{"strobe_policy":"react_slider_authoritative","runtime_role":"intent_metadata_only_no_saas_frame_executor","stage_id":42},
            "ack":{"accepted":true},"report":{"status":"ACTIVE"},
            "launch_contract":{"bindings":[
                {"id":"dim","target":{"channel_name":"Dimming","type":"DIMMER"},
                 "source":{"kind":"constant","value":"${intensity}"},
                 "resolve":{"mode":"override","priority":40}}
            ]}
        }]}"#;
        let list: CueListResponse = serde_json::from_str(json).unwrap();
        let dto = &list.cues[0];
        // DTO captures the consume surface (no longer silently dropped).
        assert_eq!(dto.cue_profile_id.as_deref(), Some("warm-wash-pulse"));
        assert_eq!(dto.effect_id.as_deref(), Some("flat-par-wash-pulse"));
        assert_eq!(dto.stage_id, Some(42));
        assert!(dto.ack.is_some(), "ack captured");
        assert!(dto.report.is_some(), "report captured");
        // ActiveCue retains identity + the live variable snapshot for report-back.
        let cue = ActiveCue::from_dto(dto);
        assert_eq!(cue.cue_profile_id.as_deref(), Some("warm-wash-pulse"));
        assert_eq!(cue.effect_id.as_deref(), Some("flat-par-wash-pulse"));
        assert_eq!(cue.stage_id, Some(42));
        assert_eq!(cue.strobe_policy.as_deref(), Some("react_slider_authoritative"));
        assert_eq!(
            cue.runtime_role.as_deref(),
            Some("intent_metadata_only_no_saas_frame_executor")
        );
        assert_eq!(cue.variables.get("intensity").and_then(|v| v.as_f64()), Some(0.82));
        assert_eq!(cue.bindings.len(), 1);
        assert_eq!(cue.parse_skips().len(), 0);
    }

    #[test]
    fn missing_consume_fields_default_cleanly() {
        // A minimal cue (no identity/metadata/ack/report) must still parse — the
        // new fields are #[serde(default)] so older/stripped payloads don't break.
        let list: CueListResponse = serde_json::from_str(CUE_JSON).unwrap();
        let cue = ActiveCue::from_dto(&list.cues[0]);
        assert!(cue.cue_profile_id.is_none());
        assert!(cue.effect_id.is_none());
        assert!(cue.stage_id.is_none());
        assert!(cue.strobe_policy.is_none());
        assert!(cue.runtime_role.is_none());
    }

    #[test]
    fn rejects_only_when_no_binding_is_runnable() {
        // Slice 2f: a cue whose every binding is unsupported (parses to zero
        // runnable bindings) is rejected with a reason naming the skips; a cue
        // with at least one runnable binding is accepted (None), per rule 6.
        let all_bad = r#"{"cues":[{
            "launch_id":"RJ","status":"LAUNCH_REQUESTED","variables":{},
            "launch_contract":{"bindings":[
                {"id":"x","target":{"channel_name":"Foo","type":"NOPE"},
                 "source":{"kind":"constant","value":1.0},"resolve":{}}
            ]}
        }]}"#;
        let list: CueListResponse = serde_json::from_str(all_bad).unwrap();
        let cue = ActiveCue::from_dto(&list.cues[0]);
        assert_eq!(cue.bindings.len(), 0, "NOPE target.type does not parse");
        let reason = cue.rejection_reason().expect("all-unsupported cue rejects");
        assert!(reason.contains("no runnable bindings"), "got {}", reason);

        // CUE_JSON has runnable bindings -> accept (None) even with a gobo skip.
        let ok: CueListResponse = serde_json::from_str(CUE_JSON).unwrap();
        let ok_cue = ActiveCue::from_dto(&ok.cues[0]);
        assert!(ok_cue.rejection_reason().is_none(), "cue with runnable bindings accepts");
    }

    #[test]
    fn clamps_and_snaps_variables_per_schema() {
        // Slice 2g: launch_contract.variables carries the schema; the server does
        // NOT clamp, so a live value past max is clamped before it reaches a
        // binding, snapped to allowed[], echoed clamped in active_variables, and
        // noted out-of-range. A var with no spec passes through untouched.
        let json = r#"{"cues":[{
            "launch_id":"V1","status":"ACTIVE",
            "variables":{"intensity":5.0,"cycle_beats":7,"hue":0.5},
            "launch_contract":{
                "variables":{
                    "intensity":{"default":0.8,"min":0.0,"max":1.0},
                    "cycle_beats":{"default":8,"allowed":[4,8,16]}
                },
                "bindings":[
                    {"id":"dim","target":{"channel_name":"Dimming","type":"DIMMER"},
                     "source":{"kind":"constant","value":"${intensity}"},
                     "resolve":{"mode":"override","priority":40}}
                ]
            }
        }]}"#;
        let list: CueListResponse = serde_json::from_str(json).unwrap();
        let cue = ActiveCue::from_dto(&list.cues[0]);

        // active_variables echoes CLAMPED values.
        assert_eq!(cue.variables.get("intensity").and_then(|v| v.as_f64()), Some(1.0), "intensity clamped to max");
        assert_eq!(cue.variables.get("cycle_beats").and_then(|v| v.as_f64()), Some(8.0), "cycle_beats snapped to nearest allowed (7 -> 8)");
        assert_eq!(cue.variables.get("hue").and_then(|v| v.as_f64()), Some(0.5), "unspecified var passes through");

        // out-of-range noted for the two adjusted vars, not for hue.
        let notes = cue.var_clamp_misses();
        assert_eq!(notes.len(), 2, "intensity + cycle_beats clamped; hue untouched: {:?}", notes);
        assert!(notes.iter().any(|m| m.target == "variable:intensity"));
        assert!(notes.iter().any(|m| m.target == "variable:cycle_beats"));

        // and the binding itself resolved to the clamped value (1.0), not 5.0.
        let wash = FixturePatch::demo_rgbw_wash("w", 0, 1);
        let mut intent = FixtureIntentState::default();
        let mut reports = HashMap::new();
        apply_cue_bindings(&mut intent, &wash, None, 0, &[cue], 0.0, &mut reports);
        assert!((intent.dimmer - 1.0).abs() < 1e-4, "binding used clamped intensity, got {}", intent.dimmer);
    }

    #[test]
    fn in_range_variables_are_not_noted() {
        // A live value within range is used as-is, with no clamp note.
        let json = r#"{"cues":[{
            "launch_id":"V2","status":"ACTIVE",
            "variables":{"intensity":0.5},
            "launch_contract":{
                "variables":{"intensity":{"default":0.8,"min":0.0,"max":1.0}},
                "bindings":[
                    {"id":"dim","target":{"channel_name":"Dimming","type":"DIMMER"},
                     "source":{"kind":"constant","value":"${intensity}"},
                     "resolve":{"mode":"override","priority":40}}
                ]
            }
        }]}"#;
        let list: CueListResponse = serde_json::from_str(json).unwrap();
        let cue = ActiveCue::from_dto(&list.cues[0]);
        assert_eq!(cue.variables.get("intensity").and_then(|v| v.as_f64()), Some(0.5));
        assert!(cue.var_clamp_misses().is_empty(), "in-range value not noted");
    }

    #[test]
    fn clamp_to_spec_handles_bounds_allowed_and_inverted() {
        // 2g review fix #1: an inverted (min > max) spec must still clamp into a
        // sane [lo, hi] rather than failing both bounds.
        let range = VarSpec { min: Some(0.0), max: Some(1.0), allowed: None };
        assert_eq!(clamp_to_spec(&range, 5.0), 1.0);
        assert_eq!(clamp_to_spec(&range, -2.0), 0.0);
        assert_eq!(clamp_to_spec(&range, 0.4), 0.4);

        let lo_only = VarSpec { min: Some(2.0), max: None, allowed: None };
        assert_eq!(clamp_to_spec(&lo_only, 1.0), 2.0);
        assert_eq!(clamp_to_spec(&lo_only, 9.0), 9.0);

        let inverted = VarSpec { min: Some(10.0), max: Some(5.0), allowed: None };
        assert_eq!(clamp_to_spec(&inverted, 3.0), 5.0, "inverted clamps up into [5,10]");
        assert_eq!(clamp_to_spec(&inverted, 12.0), 10.0, "inverted clamps down into [5,10]");
        assert_eq!(clamp_to_spec(&inverted, 7.0), 7.0, "value already inside [5,10] unchanged");

        let allowed = VarSpec { min: None, max: None, allowed: Some(vec![4.0, 8.0, 16.0]) };
        assert_eq!(clamp_to_spec(&allowed, 7.0), 8.0, "snaps to nearest");
        assert_eq!(clamp_to_spec(&allowed, 8.0), 8.0, "exact allowed value unchanged");
        assert_eq!(clamp_to_spec(&allowed, 100.0), 16.0);
    }

    #[test]
    fn fade_factor_scales_contribution() {
        // 3b (rule 8): the renderer ramps a retained clone's fade_factor 1→0; the
        // resolved binding value is scaled by it so the cue fades to black. Here we
        // exercise the scaling directly (the clone injection lives in DmxManager).
        let json = r#"{"cues":[{
            "launch_id":"F1","status":"ACTIVE","variables":{},
            "launch_contract":{"bindings":[
                {"id":"d","target":{"channel_name":"Dimming","type":"DIMMER"},
                 "source":{"kind":"constant","value":0.8},
                 "resolve":{"mode":"override","priority":40}}
            ]}}]}"#;
        let list: CueListResponse = serde_json::from_str(json).unwrap();
        let mut cue = ActiveCue::from_dto(&list.cues[0]);
        assert!((cue.fade_factor - 1.0).abs() < f32::EPSILON, "default factor is 1.0");
        let wash = FixturePatch::demo_rgbw_wash("w", 0, 1);

        // factor 0.5 -> 0.8 * 0.5 = 0.4
        cue.fade_factor = 0.5;
        let mut intent = FixtureIntentState::default();
        let mut reports = HashMap::new();
        apply_cue_bindings(&mut intent, &wash, None, 0, std::slice::from_ref(&cue), 0.0, &mut reports);
        assert!((intent.dimmer - 0.4).abs() < 1e-4, "fade scales override, got {}", intent.dimmer);

        // factor 0 -> contributes nothing (faded to black)
        cue.fade_factor = 0.0;
        let mut intent2 = FixtureIntentState::default();
        let mut reports2 = HashMap::new();
        apply_cue_bindings(&mut intent2, &wash, None, 0, std::slice::from_ref(&cue), 0.0, &mut reports2);
        assert!(intent2.dimmer.abs() < 1e-4, "fully faded contributes nothing, got {}", intent2.dimmer);
    }

    #[test]
    fn react_strobe_gate_engages_strobe_minimally() {
        // 2h carve-out: react_strobe_gate parses (no longer skipped) and engages
        // the strobe/shutter channel so eight-segment-strobe-burst actually
        // strobes. Operator-slider authority is deferred with 2h.
        use crate::emerge::contracts::{ChannelMap, FixtureRole, StageVec3, StageVisualKind};
        let json = r#"{"cues":[{
            "launch_id":"SG","status":"ACTIVE","variables":{},
            "launch_contract":{"bindings":[
                {"id":"strobe-enable","target":{"channel_name":"Strobe","type":"SHUTTER"},
                 "source":{"kind":"react_strobe_gate","value":"react_strobe_slider"},
                 "resolve":{"mode":"override","priority":100}}
            ]}}]}"#;
        let list: CueListResponse = serde_json::from_str(json).unwrap();
        let cue = ActiveCue::from_dto(&list.cues[0]);
        assert_eq!(cue.parse_skips().len(), 0, "react_strobe_gate parses: {:?}", cue.parse_skips());
        assert_eq!(cue.bindings.len(), 1);

        let fixture = FixturePatch {
            footprint: 0,
            screen: None,
            id: "strobe".into(),
            name: "strobe".into(),
            universe: 0,
            start_address: 1,
            profile_id: "p".into(),
            mode: "1".into(),
            position: StageVec3::zero(),
            rotation: StageVec3::zero(),
            scale: StageVec3::one(),
            role: FixtureRole::Accent,
            visual_kind: StageVisualKind::Laser,
            tags: vec![],
            channels: ChannelMap {
                dimmer: None, red: None, green: None, blue: None, white: None,
                uv: None, strobe: Some(1), color_wheel: None, pan: None, tilt: None,
                zoom: None, focus: None, gobo_slot: None, gobo_rotation: None,
                prism: None, laser_macro: None,
            },
        };
        let mut intent = FixtureIntentState::default();
        let mut reports = HashMap::new();
        apply_cue_bindings(&mut intent, &fixture, None, 0, &[cue], 0.0, &mut reports);
        assert!((intent.strobe - 1.0).abs() < 1e-4, "strobe engaged, got {}", intent.strobe);
        let r = reports.get("SG").expect("report");
        assert_eq!(r.applied_bindings, 1);
        assert!(r.capability_misses.is_empty());
    }

    #[test]
    fn applies_constant_oscillator_and_inline_step_chase() {
        let cue = parse_one();
        // Moving head has dimmer + pan + gobo_slot in its ChannelMap. The
        // gobo step_chase resolves to empty (no profile supplied here) and
        // capability-misses with a profile-related reason.
        let mh = FixturePatch::demo_moving_head("mh", 0, 1);
        let mut intent = FixtureIntentState::default();
        let mut reports = HashMap::new();
        // beats chosen so sine phase = 0.25 (peak): beats=4, period=16.
        let beats = 4.0_f64;
        apply_cue_bindings(&mut intent, &mh, None, 0, &[cue], beats, &mut reports);

        assert!((intent.dimmer - 0.8).abs() < 1e-4, "constant override sets dimmer");
        // pan = add_bipolar(0.0, 0.2 * sin(2pi*0.25)=0.2) = 0.2
        assert!((intent.pan - 0.2).abs() < 1e-3, "oscillator add drives pan, got {}", intent.pan);
        let r = reports.get("L1").expect("report for L1");
        // 3 applied (constant DIMMER + oscillator PAN + inline step_chase
        // DIMMER); 1 miss (step_chase GOBO can't resolve values_from
        // without a fixture profile).
        assert_eq!(r.applied_bindings, 3);
        assert_eq!(r.capability_misses.len(), 1);
        assert!(r.capability_misses[0]
            .reason
            .contains("step values"));
    }

    #[test]
    fn higher_priority_wins_across_cues() {
        // 3a (A.8): two ACTIVE cues both `override` the same DIMMER. The
        // HIGH-priority cue is listed FIRST in the array and the LOW-priority cue
        // LAST — under the old array-order apply the last cue (low) would win;
        // global priority ordering must let the high-priority cue win regardless.
        let high = r#"{
            "launch_id":"HIGH","status":"ACTIVE","variables":{},
            "launch_contract":{"bindings":[
                {"id":"d","target":{"channel_name":"Dimming","type":"DIMMER"},
                 "source":{"kind":"constant","value":0.9},
                 "resolve":{"mode":"override","priority":90}}
            ]}}"#;
        let low = r#"{
            "launch_id":"LOW","status":"ACTIVE","variables":{},
            "launch_contract":{"bindings":[
                {"id":"d","target":{"channel_name":"Dimming","type":"DIMMER"},
                 "source":{"kind":"constant","value":0.2},
                 "resolve":{"mode":"override","priority":10}}
            ]}}"#;
        let high: CueDto = serde_json::from_str(high).unwrap();
        let low: CueDto = serde_json::from_str(low).unwrap();
        // HIGH first in the array, LOW last — array order would make LOW win.
        let cues = [ActiveCue::from_dto(&high), ActiveCue::from_dto(&low)];
        let wash = FixturePatch::demo_rgbw_wash("w", 0, 1);
        let mut intent = FixtureIntentState::default();
        let mut reports = HashMap::new();
        apply_cue_bindings(&mut intent, &wash, None, 0, &cues, 0.0, &mut reports);
        assert!(
            (intent.dimmer - 0.9).abs() < 1e-4,
            "global priority: HIGH (90) composes last and wins, got {}",
            intent.dimmer
        );
        // both cues still get a report entry (per-frame heartbeat).
        assert!(reports.contains_key("HIGH") && reports.contains_key("LOW"));
    }

    #[test]
    fn capability_gate_skips_missing_channel() {
        let cue = parse_one();
        // RGBW wash has a dimmer but no pan + no gobo_slot — both their
        // bindings should capability-miss.
        let wash = FixturePatch::demo_rgbw_wash("w", 0, 1);
        let mut intent = FixtureIntentState::default();
        let mut reports = HashMap::new();
        apply_cue_bindings(&mut intent, &wash, None, 0, &[cue], 0.0, &mut reports);

        assert!((intent.dimmer - 0.8).abs() < 1e-4);
        let r = reports.get("L1").expect("report for L1");
        assert_eq!(r.capability_misses.len(), 2, "pan + gobo gated out");
        assert!(r.capability_misses.iter().any(|m| m.target == "Pan"));
        assert!(r.capability_misses.iter().any(|m| m.target == "Gobo"));
    }

    #[test]
    fn palette_colour_writes_rgb_and_oscillator_pulse() {
        // flat-par-like: palette wash-color (RGBAWUV) + oscillator MASTER_DIMMER.
        let json = r#"{"cues":[{
            "launch_id":"P1","status":"ACTIVE",
            "variables":{"base_color":"warm_magenta","pulse_depth":0.35,"pulse_beats":4},
            "launch_contract":{"bindings":[
                {"id":"wash-color","target":{"channel_names":["Red","Green","Blue","White","Amber","UV"],"type":"RGBAWUV"},
                 "source":{"kind":"palette","value":"${base_color}"},
                 "resolve":{"mode":"override","priority":45}},
                {"id":"wash-pulse","target":{"channel_name":"Dimmer","type":"MASTER_DIMMER"},
                 "source":{"kind":"oscillator","wave":"sine","center":0.65,"depth":"${pulse_depth}","period_beats":"${pulse_beats}"},
                 "resolve":{"mode":"max","priority":40}}
            ]}
        }]}"#;
        let list: CueListResponse = serde_json::from_str(json).unwrap();
        let cue = ActiveCue::from_dto(&list.cues[0]);
        assert_eq!(cue.parse_skips().len(), 0, "palette + oscillator both supported: {:?}", cue.parse_skips());

        let wash = FixturePatch::demo_rgbw_wash("w", 0, 1);
        let mut intent = FixtureIntentState::default();
        let mut reports = HashMap::new();
        apply_cue_bindings(&mut intent, &wash, None, 0, &[cue], 0.0, &mut reports);

        // warm_magenta = [1.0, 0.0, 0.6]
        assert!(
            (intent.red - 1.0).abs() < 1e-4 && intent.green.abs() < 1e-4 && (intent.blue - 0.6).abs() < 1e-4,
            "rgb=({},{},{})", intent.red, intent.green, intent.blue
        );
        // oscillator (max) at frame_time 0: center 0.65 + 0.35*sin(0) = 0.65
        assert!((intent.dimmer - 0.65).abs() < 1e-4, "dimmer={}", intent.dimmer);
        let r = reports.get("P1").expect("report for P1");
        assert!(r.capability_misses.is_empty());
        assert_eq!(r.applied_bindings, 2);
    }

    #[test]
    fn color_strobe_flashes_rgb_on_a_duty_cycle() {
        // 3-channel use case: strobe an RGB-only fixture in a colour with no
        // dedicated strobe channel. period_beats=1, duty=0.5 -> colour ON for the
        // first half of the beat, black (OFF) for the second half.
        let json = r#"{"cues":[{
            "launch_id":"CS1","status":"ACTIVE",
            "variables":{"strobe_color":"red"},
            "launch_contract":{"bindings":[
                {"id":"color-strobe","target":{"channel_names":["Red","Green","Blue"],"type":"RGB"},
                 "source":{"kind":"color_strobe","value":"${strobe_color}","period_beats":1,"duty":0.5},
                 "resolve":{"mode":"override","priority":50}}
            ]}
        }]}"#;
        let list: CueListResponse = serde_json::from_str(json).unwrap();
        let cue = ActiveCue::from_dto(&list.cues[0]);
        assert_eq!(cue.parse_skips().len(), 0, "color_strobe supported: {:?}", cue.parse_skips());
        let wash = FixturePatch::demo_rgbw_wash("w", 0, 1);

        // ON phase (beats 0.0): red full, green/blue 0.
        let mut on = FixtureIntentState::default();
        let mut r1 = HashMap::new();
        apply_cue_bindings(&mut on, &wash, None, 0, &[cue.clone()], 0.0, &mut r1);
        assert!(
            (on.red - 1.0).abs() < 1e-4 && on.green.abs() < 1e-4 && on.blue.abs() < 1e-4,
            "on-phase rgb=({},{},{})", on.red, on.green, on.blue
        );

        // OFF phase (beats 0.6, past duty 0.5): black.
        let mut off = FixtureIntentState::default();
        let mut r2 = HashMap::new();
        apply_cue_bindings(&mut off, &wash, None, 0, &[cue], 0.6, &mut r2);
        assert!(
            off.red.abs() < 1e-4 && off.green.abs() < 1e-4 && off.blue.abs() < 1e-4,
            "off-phase rgb=({},{},{})", off.red, off.green, off.blue
        );
    }

    #[test]
    fn segment_chase_parses_and_evaluates() {
        // Grammar: a segment_chase RGB_SEGMENTS binding normalizes (not skipped).
        let json = r#"{"cues":[{
            "launch_id":"SG1","status":"ACTIVE",
            "variables":{"palette":"rainbow","rate":1},
            "launch_contract":{"bindings":[
                {"id":"seg","target":{"channel_pattern":["Red {n}","Green {n}","Blue {n}"],"segments":8,"type":"RGB_SEGMENTS"},
                 "source":{"kind":"segment_chase","direction":"left_to_right","width":1.0,"period_beats":"${rate}","palette":"${palette}"},
                 "resolve":{"mode":"override","priority":60}}
            ]}
        }]}"#;
        let list: CueListResponse = serde_json::from_str(json).unwrap();
        let cue = ActiveCue::from_dto(&list.cues[0]);
        assert_eq!(cue.parse_skips().len(), 0, "segment_chase supported: {:?}", cue.parse_skips());

        // Math: L->R chase, width 1 over 8 cells. At beat 0 the head sits at cell
        // 0 -> cell 0 fully lit, a far cell (4) dark.
        let chase = SegmentKind::Chase {
            direction: SegDir::LeftToRight,
            width: 1.0,
            period_beats: 1.0,
            palette: Cow::Borrowed("white"),
            background: Cow::Borrowed("black"),
            head_cycle: None,
        };
        let head = eval_segment(&chase, 0, 8, 0.0);
        let far = eval_segment(&chase, 4, 8, 0.0);
        assert!(head[0] > 0.9 && head[1] > 0.9 && head[2] > 0.9, "head cell lit: {:?}", head);
        assert!(far[0] < 0.1 && far[1] < 0.1 && far[2] < 0.1, "far cell dark: {:?}", far);
    }

    #[test]
    fn gradient_palette_resolves_named_and_gradient_colours() {
        assert_eq!(gradient_rgb("red", 0.5), [1.0, 0.0, 0.0]); // single colour ignores t
        let rb0 = gradient_rgb("rainbow", 0.0);
        assert!(rb0[0] > 0.9 && rb0[1] < 0.1 && rb0[2] < 0.1, "rainbow@0 ~ red: {:?}", rb0);
        let bw1 = gradient_rgb("blue_white", 1.0);
        assert!(bw1[0] > 0.9 && bw1[1] > 0.9 && bw1[2] > 0.9, "blue_white@1 ~ white: {:?}", bw1);
        assert_eq!(gradient_rgb("black", 0.5), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn auto_segment_escalates_with_tension_and_releases_on_drop() {
        use std::mem::discriminant;
        // The autonomous segment look emerges from the charge/discharge curve:
        // calm -> building -> tightening -> release, each a distinct effect.
        let calm = auto_segment_kind(0, 0.0, 0.0, 0.0);
        let building = auto_segment_kind(0, 0.4, 0.0, 0.0);
        let tightening = auto_segment_kind(0, 0.7, 0.0, 0.0);
        let release = auto_segment_kind(0, 0.7, 0.9, 0.0);
        assert!(discriminant(&calm) != discriminant(&building), "calm vs building differ");
        assert!(discriminant(&building) != discriminant(&tightening), "building vs tightening differ");
        assert!(discriminant(&tightening) != discriminant(&release), "tightening vs release differ");
        assert!(matches!(calm, SegmentKind::Rainbow { .. }), "calm is a slow rainbow");
        // A drop releases to the strobe look even at high banked tension (the
        // drop overrides the tightening chase).
        assert!(matches!(release, SegmentKind::Strobe { .. }), "a drop releases to the strobe look");
    }

    #[test]
    fn auto_segment_chase_tightens_with_tension() {
        // In the tightening band, more banked tension shortens the chase period.
        let lo = auto_segment_kind(0, 0.55, 0.0, 0.0);
        let hi = auto_segment_kind(0, 0.95, 0.0, 0.0);
        match (lo, hi) {
            (
                SegmentKind::Chase { period_beats: plo, .. },
                SegmentKind::Chase { period_beats: phi, .. },
            ) => assert!(phi < plo, "higher tension tightens the chase: {phi} < {plo}"),
            _ => panic!("both should be a Chase in the tightening band"),
        }
    }

    #[test]
    fn auto_chase_wires_the_integrated_cycle_through() {
        // The glue that fixes the build glitch: the tightening chase must
        // carry the rig-wide integrated cycle, not fall back to the
        // teleporting beats/period derivation. (The integrator and eval are
        // pinned separately — this pins the hand-off between them.)
        match auto_segment_kind(1, 0.6, 0.0, 0.123) {
            SegmentKind::Chase { head_cycle, .. } => {
                assert_eq!(head_cycle, Some(0.123), "tightening chase must ride the integrator")
            }
            other => panic!("tension 0.6 should be the tightening chase, got {other:?}"),
        }
    }

    #[test]
    fn auto_chase_head_does_not_teleport_while_tension_varies() {
        // THE build glitch: the tightening chase derived its head from
        // fract(abs_beats / period) while tension varied the period — at beat
        // ~5000 every period change rescaled the whole quotient and the head
        // skipped pseudo-randomly between cells instead of accelerating.
        // The integrated cycle must move the head smoothly AND faster as the
        // bank fills.
        let mut cycle = 0.0_f64;
        let mut last_beat = 5000.0_f64;
        let mut beat = 5000.0_f64;
        let db = 0.05; // ~30Hz frames at moderate tempo
        let mut prev_phase: Option<f64> = None;
        let mut max_step = 0.0_f64;
        let mut old_jumped = false;
        for frame in 0..200 {
            beat += db;
            // Tension ramps through the tightening band — period 1.0 -> 0.4.
            let tension = 0.45 + 0.25 * (frame as f32 / 200.0);
            (cycle, last_beat) = advance_auto_chase(cycle, last_beat, beat, tension);
            let phase = cycle.fract();
            if let Some(p) = prev_phase {
                // Step measured around the wrap (a 0.98 -> 0.02 sweep is fine).
                let step = (phase - p).rem_euclid(1.0).min((p - phase).rem_euclid(1.0));
                max_step = max_step.max(step);
            }
            prev_phase = Some(phase);
            // The OLD derivation for contrast: fract(beats / period) across
            // one frame's period change.
            let period_now = auto_chase_period(tension) as f64;
            let period_prev =
                auto_chase_period(0.45 + 0.25 * ((frame.max(1) - 1) as f32 / 200.0)) as f64;
            let old = (beat / period_now).fract();
            let old_prev = ((beat - db) / period_prev).fract();
            if ((old - old_prev).rem_euclid(1.0)).min((old_prev - old).rem_euclid(1.0)) > 0.3 {
                old_jumped = true;
            }
        }
        // Integrated head: per-frame motion bounded by db / min_period = 0.125
        // of the bar — a sweep, never a teleport.
        assert!(
            max_step < 0.13,
            "integrated head must sweep continuously, worst step {max_step}"
        );
        // Sanity: the old formulation genuinely teleported under this ramp —
        // the regression this test pins would be invisible otherwise.
        assert!(
            old_jumped,
            "expected the legacy beats/period derivation to teleport under a tension ramp"
        );
    }

    #[test]
    fn advance_auto_chase_accelerates_and_guards_clock_seams() {
        // Higher tension -> shorter period -> more cycles per beat.
        let (calm, _) = advance_auto_chase(0.0, 100.0, 101.0, 0.0); // period 1.0
        let (tight, _) = advance_auto_chase(0.0, 100.0, 101.0, 0.95); // period 0.4
        assert!((calm - 1.0).abs() < 1e-9, "one beat at period 1.0 = one cycle");
        assert!(
            ((tight - 0.5).abs() < 1e-6) || ((tight - 2.5).abs() < 1e-6),
            "one beat at period 0.4 = 2.5 cycles (0.5 after the wrap at 2): {tight}"
        );

        // Clock seams: a backwards rebase advances nothing; a huge forward
        // jump advances at most one beat — the head glides through, no leap.
        let (c, lb) = advance_auto_chase(0.25, 500.0, 12.0, 0.0);
        assert_eq!(c, 0.25, "backwards beat must not advance the head");
        assert_eq!(lb, 12.0, "but the reference beat re-anchors");
        let (c, _) = advance_auto_chase(0.25, 12.0, 9000.0, 0.0);
        assert!((c - 1.25).abs() < 1e-9, "forward jump clamps to one beat of travel");
    }

    #[test]
    fn eval_segment_honors_integrated_head_cycle() {
        // With head_cycle set, the head position comes from the integrator —
        // absolute beats are ignored, so a varying period cannot teleport it.
        let mk = |head_cycle| SegmentKind::Chase {
            direction: SegDir::LeftToRight,
            width: 1.0,
            period_beats: 0.7,
            palette: Cow::Borrowed("white"),
            background: Cow::Borrowed("black"),
            head_cycle,
        };
        let n = 8;
        // cycle 0.0: head at cell 0 regardless of the huge beat count.
        let c = eval_segment(&mk(Some(0.0)), 0, n, 123_456.789);
        assert!(c[0] > 0.9, "head at cell 0 for cycle 0.0: {c:?}");
        // cycle 0.5: head halfway down the bar (cell 4 of 8).
        let c = eval_segment(&mk(Some(0.5)), 4, n, 123_456.789);
        assert!(c[0] > 0.9, "head at cell 4 for cycle 0.5: {c:?}");
        let dark = eval_segment(&mk(Some(0.5)), 0, n, 123_456.789);
        assert!(dark[0] < 0.1, "cell 0 dark for cycle 0.5: {dark:?}");
        // None keeps the historical beats/period derivation bit-for-bit
        // (period widened from f32 exactly as eval does).
        let legacy = eval_segment(&mk(None), 3, n, 10.0);
        let phase = (10.0_f64 / (0.7_f32 as f64).max(0.0001)).fract() as f32;
        let head = phase * n as f32;
        let mut d = (3.0_f32 - head).abs();
        d = d.min(n as f32 - d);
        let lit = (1.0 - d / 1.0).clamp(0.0, 1.0);
        assert!(
            (legacy[0] - lit).abs() < 1e-6,
            "None must keep the legacy derivation: {legacy:?} vs lit={lit}"
        );
    }

    #[test]
    fn chill_caps_segment_tension() {
        // The Chill style caps CueSnapshot.tension_bank at exactly
        // AUTO_SEG_TENSION_HIGH (osc/thread.rs stamp seam): the chase branch
        // is a STRICT greater-than, so a capped bank tops out at the measured
        // Interference ripple and never the <=1-beat tightening chase.
        let cap = crate::emerge::LightingStyle::Chill.segment_tension_cap();
        assert_eq!(
            cap, AUTO_SEG_TENSION_HIGH,
            "style.rs CHILL_SEGMENT_TENSION_CAP must mirror AUTO_SEG_TENSION_HIGH"
        );
        assert!(
            matches!(
                auto_segment_kind(0, cap, 0.0, 0.0),
                SegmentKind::Interference { .. }
            ),
            "a chill-capped bank reads as the Interference ripple"
        );
        assert!(
            matches!(
                auto_segment_kind(0, cap + 0.01, 0.0, 0.0),
                SegmentKind::Chase { .. }
            ),
            "just above the cap the tightening chase engages (Auto behavior)"
        );
        // The chill drop ceiling (0.35 + 0.65*0.15) sits under the sparkle
        // release threshold — the strobe segment is unreachable in Chill.
        let chill_ceiling =
            0.35 + 0.65 * crate::emerge::LightingStyle::Chill.clamp_license(1.0);
        assert!(
            chill_ceiling < AUTO_SEG_DROP_RELEASE,
            "chill drop ceiling {chill_ceiling} must stay under the sparkle threshold"
        );
    }

    #[test]
    fn stopped_cue_does_not_apply() {
        let mut cue = parse_one();
        cue.status = "STOPPED".to_string();
        let mh = FixturePatch::demo_moving_head("mh", 0, 1);
        let mut intent = FixtureIntentState::default();
        let mut reports = HashMap::new();
        apply_cue_bindings(&mut intent, &mh, None, 0, &[cue], 0.0, &mut reports);
        assert_eq!(intent.dimmer, 0.0);
        assert!(reports.is_empty());
    }

    #[test]
    fn step_chase_resolves_values_from_profile_steps() {
        // Slice 2c smoke: a cue with a GOBO step_chase + values_from
        // resolves against the fixture's profile gobo steps, indexed by
        // beats. low/high are DMX BYTES, as in real ATLABASE profiles: steps
        // (0, 102) and (153, 255) — their /255-normalized midpoints (0.2,
        // 0.8) form the chase. period_beats=1, beats=0 -> idx 0 -> value
        // 0.2 -> attr.set(GoboSlot, 0.2) -> u8 round(0.2 * 7) = 1.
        use crate::emerge::contracts::{
            ChannelDefinition, ChannelDetail, ChannelMap, ChannelSlot, DmxAttribute,
            FixtureCapability, FixtureKind, FixtureProfile, FixtureRole, StageVec3,
            StageVisualKind,
        };
        let profile = FixtureProfile {
            id: "demo-laser".into(),
            manufacturer: "Test".into(),
            model: "Demo Laser".into(),
            mode: "8".into(),
            kind: FixtureKind::Laser,
            housing: crate::emerge::FixtureHousing::ScanLaser,
            channel_count: 1,
            channels: vec![ChannelDefinition {
                attribute: DmxAttribute::GoboSlot,
                channel_offset: 1,
                default_value: 0,
                blackout_value: 0,
                detail: Some(ChannelDetail::Gobo {
                    steps: vec![
                        ChannelSlot {
                            name: "A".into(),
                            value: 0,
                            low_value: Some(0.0),
                            high_value: Some(102.0),
                            enabled: true,
                            rgb: None,
                        },
                        ChannelSlot {
                            name: "B".into(),
                            value: 0,
                            low_value: Some(153.0),
                            high_value: Some(255.0),
                            enabled: true,
                            rgb: None,
                        },
                    ],
                }),
                source_attribute: Some(serde_json::json!({"name": "CUE GROUPS"})),
            }],
            capabilities: vec![FixtureCapability::Gobo],
        };
        let fixture = FixturePatch {
            footprint: 0,
            screen: None,
            id: "lz".into(),
            name: "lz".into(),
            universe: 0,
            start_address: 1,
            profile_id: "demo-laser".into(),
            mode: "8".into(),
            position: StageVec3::zero(),
            rotation: StageVec3::zero(),
            scale: StageVec3::one(),
            role: FixtureRole::Accent,
            visual_kind: StageVisualKind::Laser,
            tags: vec![],
            channels: ChannelMap {
                dimmer: None, red: None, green: None, blue: None, white: None,
                uv: None, strobe: None, color_wheel: None, pan: None, tilt: None,
                zoom: None, focus: None, gobo_slot: Some(1),
                gobo_rotation: None, prism: None, laser_macro: None,
            },
        };
        let json = r#"{"cues":[{
            "launch_id":"S1","status":"ACTIVE",
            "variables":{},
            "launch_contract":{"bindings":[
                {"id":"chase","target":{"channel_name":"CUE GROUPS","type":"GOBO"},
                 "source":{"kind":"step_chase","values_from":"source_attribute.goboSetting.steps","period_beats":1},
                 "resolve":{"mode":"override","priority":50}}
            ]}
        }]}"#;
        let list: CueListResponse = serde_json::from_str(json).unwrap();
        let cue = ActiveCue::from_dto(&list.cues[0]);
        assert_eq!(cue.parse_skips().len(), 0);

        let mut intent = FixtureIntentState::default();
        let mut reports = HashMap::new();
        // beats=0 -> step 0 -> value 0.2 -> gobo_slot = (0.2 * 7).round() = 1
        let out0 =
            apply_cue_bindings(&mut intent, &fixture, Some(&profile), 0, &[cue.clone()], 0.0, &mut reports);
        assert_eq!(intent.gobo_slot, 1, "beats=0 -> step 0 (mid 51/255 = 0.2)");
        // Fix #2: the exact normalized step value rides past the 8-slot
        // quantization for the writer to emit as a precise byte.
        assert!(
            (out0.gobo_exact.expect("profile-step gobo chase carries exact value") - 0.2).abs() < 1e-4,
            "gobo_exact = {:?}",
            out0.gobo_exact
        );

        // beats=1 -> step 1 -> value 0.8 -> gobo_slot = (0.8 * 7).round() = 6
        let mut intent = FixtureIntentState::default();
        let mut reports = HashMap::new();
        let out1 = apply_cue_bindings(&mut intent, &fixture, Some(&profile), 0, &[cue], 1.0, &mut reports);
        assert_eq!(intent.gobo_slot, 6, "beats=1 -> step 1 (mid 204/255 = 0.8)");
        assert!((out1.gobo_exact.expect("exact value at step 1") - 0.8).abs() < 1e-4);
        let r = reports.get("S1").expect("report");
        assert!(r.capability_misses.is_empty());
        assert_eq!(r.applied_bindings, 1);
    }

    #[test]
    fn blackout_target_drives_master_enable() {
        // Slice 2d: a BLACKOUT-typed cue binding (e.g. scan-laser-pulse's
        // `laser-on`) writes through the IntentAttr::Dimmer slot since the
        // ATLABASE BLACKOUT channel is the fixture's master enable. A
        // fixture whose profile carries a BLACKOUT channel ends up with
        // ChannelMap.dimmer set, so the constant 1.0 lights the laser on
        // the wire.
        use crate::emerge::contracts::{ChannelMap, FixtureRole, StageVec3, StageVisualKind};
        let fixture = FixturePatch {
            footprint: 0,
            screen: None,
            id: "lz".into(),
            name: "scan".into(),
            universe: 0,
            start_address: 1,
            profile_id: "scan".into(),
            mode: "16".into(),
            position: StageVec3::zero(),
            rotation: StageVec3::zero(),
            scale: StageVec3::one(),
            role: FixtureRole::Accent,
            visual_kind: StageVisualKind::Laser,
            tags: vec![],
            channels: ChannelMap {
                // The scan-laser's LASER ON/OFF address lands in the dimmer
                // slot via attribute_from_atla("BLACKOUT") -> Dimmer.
                dimmer: Some(1), red: None, green: None, blue: None, white: None,
                uv: None, strobe: None, color_wheel: None, pan: None, tilt: None,
                zoom: None, focus: None, gobo_slot: None,
                gobo_rotation: None, prism: None, laser_macro: None,
            },
        };
        let json = r#"{"cues":[{
            "launch_id":"LZ","status":"ACTIVE",
            "variables":{},
            "launch_contract":{"bindings":[
                {"id":"laser-on","target":{"channel_name":"LASER ON/OFF","type":"BLACKOUT"},
                 "source":{"kind":"constant","value":1.0},
                 "resolve":{"mode":"override","priority":90}}
            ]}
        }]}"#;
        let list: CueListResponse = serde_json::from_str(json).unwrap();
        let cue = ActiveCue::from_dto(&list.cues[0]);
        assert_eq!(cue.parse_skips().len(), 0, "BLACKOUT target now parses");
        assert_eq!(cue.bindings.len(), 1);

        let mut intent = FixtureIntentState::default();
        let mut reports = HashMap::new();
        apply_cue_bindings(&mut intent, &fixture, None, 0, &[cue], 0.0, &mut reports);
        assert!(
            (intent.dimmer - 1.0).abs() < 1e-4,
            "BLACKOUT constant 1.0 lifts the master enable, got {}",
            intent.dimmer
        );
        let r = reports.get("LZ").expect("report");
        assert!(r.capability_misses.is_empty());
        assert_eq!(r.applied_bindings, 1);
    }

    #[test]
    fn atla_blackout_channel_maps_to_dimmer_slot() {
        // Slice 2d: a profile channel whose ATLABASE type is "BLACKOUT"
        // (LASER ON/OFF on the SHEHDS 3W scan laser) parses to the Dimmer
        // attribute, so ChannelMap::from_profile populates `.dimmer` with
        // its address — that's what the BLACKOUT cue binding writes to.
        use crate::emerge::contracts::{
            ChannelDefinition, ChannelMap, DmxAttribute, FixtureCapability, FixtureKind,
            FixtureProfile,
        };
        let profile = FixtureProfile {
            id: "scan".into(),
            manufacturer: "Test".into(),
            model: "Scan".into(),
            mode: "16".into(),
            kind: FixtureKind::Laser,
            housing: crate::emerge::FixtureHousing::ScanLaser,
            channel_count: 1,
            channels: vec![ChannelDefinition {
                attribute: DmxAttribute::Dimmer,
                channel_offset: 1,
                default_value: 0,
                blackout_value: 0,
                detail: None,
                source_attribute: Some(serde_json::json!({"name": "LASER ON/OFF", "type": "BLACKOUT"})),
            }],
            capabilities: vec![FixtureCapability::Dimmer],
        };
        let map = ChannelMap::from_profile(7, &profile);
        assert_eq!(map.dimmer, Some(7), "BLACKOUT channel lands in dimmer slot");
    }

    #[test]
    fn byte_scale_profile_step_midpoints_normalize_distinctly() {
        // Regression (fix #1): real ATLABASE profiles store step low/high as DMX
        // BYTES (chauvet obsession gobo: 14 steps, lows 0..130 / highs 9..139).
        // The old midpoint arm skipped the /255 normalization, so every value
        // exceeded 1.0, IntentAttr::set clamped them all to 1.0, and the chase
        // froze. Byte-scale steps must yield DISTINCT per-step values <= 1.0.
        use crate::emerge::contracts::ChannelSlot;
        let steps: Vec<ChannelSlot> = (0..14u16)
            .map(|k| ChannelSlot {
                name: format!("gobo {}", k),
                value: 0,
                low_value: Some(k as f64 * 10.0),
                high_value: Some(k as f64 * 10.0 + 9.0),
                enabled: true,
                rgb: None,
            })
            .collect();
        let values: Vec<f32> = steps.iter().map(step_norm_value).collect();
        for (k, v) in values.iter().enumerate() {
            assert!(
                *v < 1.0,
                "byte-scale midpoint {} must normalize below 1.0, got {}",
                k,
                v
            );
        }
        for w in values.windows(2) {
            assert!(
                w[1] > w[0] + 0.01,
                "per-step values must stay distinct after /255: {:?}",
                values
            );
        }
        // Spot-check the math: step 0 midpoint (0+9)/2 = 4.5 -> 4.5/255.
        assert!((values[0] - 4.5 / 255.0).abs() < 1e-6);
        // And the `value` fallback path is unchanged.
        let plain = ChannelSlot {
            name: "plain".into(),
            value: 128,
            low_value: None,
            high_value: None,
            enabled: true,
            rgb: None,
        };
        assert!((step_norm_value(&plain) - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn beat_phase_keeps_precision_at_multi_day_uptime() {
        // Regression (fix #3): at 2^24 beats (~95 days at 122 BPM, but f32
        // trouble starts ~2 days) an f32 beat count's ULP is 2.0 WHOLE beats —
        // a half-beat strobe would freeze on one phase. The f64 pipeline must
        // still resolve quarter-beat phase differences up there.
        let osc = Source::Oscillator {
            shape: OscillatorShape::Square,
            period_beats: 0.5,
            depth: 0.5,
            center: 0.5,
        };
        let base = 16_777_216.0_f64; // 2^24: f32 cannot represent base + 0.375
        let on = osc.eval(base + 0.125); // phase 0.25 -> square +1 -> 1.0
        let off = osc.eval(base + 0.375); // phase 0.75 -> square -1 -> 0.0
        assert!((on - 1.0).abs() < 1e-4, "on-phase at huge beat count, got {}", on);
        assert!(off.abs() < 1e-4, "off-phase at huge beat count, got {}", off);

        // step_chase indexing holds parity too: consecutive integer beats land
        // on consecutive steps even where f32 collapses them to the same value.
        let json = r#"{"cues":[{
            "launch_id":"UP","status":"ACTIVE","variables":{},
            "launch_contract":{"bindings":[
                {"id":"c","target":{"channel_name":"Dimming","type":"DIMMER"},
                 "source":{"kind":"step_chase","values":[0.1,0.9],"period_beats":1},
                 "resolve":{"mode":"override","priority":50}}
            ]}}]}"#;
        let list: CueListResponse = serde_json::from_str(json).unwrap();
        let cue = ActiveCue::from_dto(&list.cues[0]);
        let wash = FixturePatch::demo_rgbw_wash("w", 0, 1);
        let mut even = FixtureIntentState::default();
        let mut r1 = HashMap::new();
        apply_cue_bindings(&mut even, &wash, None, 0, std::slice::from_ref(&cue), base, &mut r1);
        let mut odd = FixtureIntentState::default();
        let mut r2 = HashMap::new();
        apply_cue_bindings(&mut odd, &wash, None, 0, &[cue], base + 1.0, &mut r2);
        assert!((even.dimmer - 0.1).abs() < 1e-4, "even beat -> step 0, got {}", even.dimmer);
        assert!((odd.dimmer - 0.9).abs() < 1e-4, "odd beat -> step 1, got {}", odd.dimmer);
    }

    #[test]
    fn within_cue_priority_pre_sorted_at_parse() {
        // Bindings are priority-sorted ONCE in from_dto (the per-frame re-sort
        // was dropped from the render path): a high-priority override authored
        // FIRST must still compose last and win over a low-priority override
        // authored second.
        let json = r#"{"cues":[{
            "launch_id":"PS","status":"ACTIVE","variables":{},
            "launch_contract":{"bindings":[
                {"id":"hi","target":{"channel_name":"Dimming","type":"DIMMER"},
                 "source":{"kind":"constant","value":0.9},
                 "resolve":{"mode":"override","priority":90}},
                {"id":"lo","target":{"channel_name":"Dimming","type":"DIMMER"},
                 "source":{"kind":"constant","value":0.2},
                 "resolve":{"mode":"override","priority":10}}
            ]}}]}"#;
        let list: CueListResponse = serde_json::from_str(json).unwrap();
        let cue = ActiveCue::from_dto(&list.cues[0]);
        assert_eq!(cue.bindings[0].priority, 10, "parse-time sort is ascending");
        assert_eq!(cue.bindings[1].priority, 90);
        let wash = FixturePatch::demo_rgbw_wash("w", 0, 1);
        let mut intent = FixtureIntentState::default();
        let mut reports = HashMap::new();
        apply_cue_bindings(&mut intent, &wash, None, 0, &[cue], 0.0, &mut reports);
        assert!(
            (intent.dimmer - 0.9).abs() < 1e-4,
            "priority 90 wins regardless of authored order, got {}",
            intent.dimmer
        );
    }

    #[test]
    fn interference_brightness_bounded_and_travels() {
        // Item 7: two-source interference. Brightness must stay in [0,1] for
        // every cell, and the node/antinode pattern must TRAVEL with beats.
        let kind = SegmentKind::Interference {
            lambda_cells: 4.0,
            period_beats: 2.0,
            palette: Cow::Borrowed("white"),
        };
        let n = 24;
        for i in 0..n {
            for &beats in &[0.0_f64, 0.7, 1.3, 123_456.5] {
                let c = eval_segment(&kind, i, n, beats);
                for ch in c {
                    assert!((0.0..=1.0).contains(&ch), "cell {} at {} out of range: {:?}", i, beats, c);
                }
                // White palette: the brightness rides all three channels equally.
                assert!((c[0] - c[1]).abs() < 1e-5 && (c[1] - c[2]).abs() < 1e-5);
            }
        }
        // Same cell, a quarter period later: the ripple has moved through it.
        let before = eval_segment(&kind, 3, n, 0.0);
        let after = eval_segment(&kind, 3, n, 0.5);
        assert!(
            (before[0] - after[0]).abs() > 0.05,
            "pattern travels with beats: {} -> {}",
            before[0],
            after[0]
        );
    }

    #[test]
    fn segment_interference_binding_parses() {
        // Grammar: segment_interference normalizes off a segmented target with
        // lambda/period/palette (variables resolve like the other segment kinds).
        let json = r#"{"cues":[{
            "launch_id":"IN1","status":"ACTIVE",
            "variables":{"bands":3.0},
            "launch_contract":{"bindings":[
                {"id":"rip","target":{"channel_pattern":["Red {n}","Green {n}","Blue {n}"],"segments":8,"type":"RGB_SEGMENTS"},
                 "source":{"kind":"segment_interference","lambda_cells":"${bands}","period_beats":2,"palette":"deep_blue"},
                 "resolve":{"mode":"override","priority":60}}
            ]}
        }]}"#;
        let list: CueListResponse = serde_json::from_str(json).unwrap();
        let cue = ActiveCue::from_dto(&list.cues[0]);
        assert_eq!(cue.parse_skips().len(), 0, "segment_interference parses: {:?}", cue.parse_skips());
        match &cue.bindings[0].op {
            BindingOp::Segment { kind: SegmentKind::Interference { lambda_cells, period_beats, .. } } => {
                assert!((lambda_cells - 3.0).abs() < 1e-4, "lambda from ${{bands}}");
                assert!((period_beats - 2.0).abs() < 1e-4);
            }
            other => panic!("expected Interference segment, got {:?}", other),
        }
    }

    #[test]
    fn auto_building_interference_compresses_with_tension() {
        // Item 7: the autopilot 'building' band is now the interference ripple,
        // and its wavelength tightens as tension_bank charges toward the drop.
        let lo = auto_segment_kind(0, 0.32, 0.0, 0.0);
        let hi = auto_segment_kind(0, 0.41, 0.0, 0.0);
        match (lo, hi) {
            (
                SegmentKind::Interference { lambda_cells: la, .. },
                SegmentKind::Interference { lambda_cells: lb, .. },
            ) => assert!(lb < la, "bands compress as the build charges: {lb} < {la}"),
            other => panic!("building band should be Interference, got {:?}", other),
        }
    }
}

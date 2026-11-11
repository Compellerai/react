//! LightingEngine — the audio-independent lighting snapshot producer.
//!
//! Owns the CueEngine, the manual triggers, the operator LFO bank, the
//! harmonic trackers and the operator lighting config, and produces the
//! `CueSnapshot` + `OscUiState` that every output (DMX wire, virtual-stage
//! preview, OSC, Hue, WLED, UI) consumes. The engine ticks on its OWN clock
//! (the hosting thread's ~100Hz loop): a fresh audio-frame `LightingInput` is
//! just one input, cached and re-stamped on the ticks between (and without)
//! audio frames, so a held manual trigger (BLINDER / STROBE / UV) reaches the
//! published snapshot within one tick with or without music. OSC is an output
//! adapter consuming this engine's results (see `osc::OscManager`), a peer to
//! ArtNet/OLA/USB/Hue/WLED — not the engine's host.
//!
//! Behavior contract (24/7 streams): for an identical stream of fresh
//! `LightingInput`s, the engine state evolution and the published snapshot are
//! byte-equivalent with the pre-extraction producer (`osc/thread.rs` +
//! `OscManager::update`). CueEngine processing, tracker EMAs, the LFO eval and
//! the key-signal fallback all advance ONLY on fresh inputs, at the same
//! per-input cadence as before; republish ticks rebuild the snapshot from the
//! frozen engine state + the CURRENT trigger/operator state.

use std::time::{Duration, Instant};

use crate::audio::AudioAnalysis;
use crate::emerge::cue::{CueEngine, ManualTriggers, TriggerButton};
use crate::emerge::harmony::{
    centroid_concentration, centroid_hue, chroma_centroid, emphasize_dominant, ChordFrame,
    ChordTracker, KeySignal, KeySignalState, TonnetzFrame, TonnetzTracker,
};
use crate::emerge::intent::{add_clamped, LfoBankConfig, LfoModulation};
use crate::hue::CueSnapshot;
use crate::osc::OscConfig;
use crate::render::{HarmonicLabel, HeroAct, LightingMetrics};

/// COM-499 Phase B2: tempo-sync LFO slots only lock to the show clock once the
/// audio-derived tempo estimate is at least this confident; below it they
/// free-run at their Hz rate (avoids a rate-wobble as the estimate converges).
const LFO_CONFIDENCE_MIN: f32 = 0.5;

/// With no fresh `LightingInput` for this long, the snapshot rebuild starts
/// EASING the audio bands toward silent (see [`STALE_FADE_TAU`]) while STILL
/// publishing trigger / clock / manual / operator state — a stalled feed must
/// never drive the autonomous look on stale loudness, and a held trigger must
/// never need audio. ~15 missed frames at 60fps: far above tick jitter, far
/// below a human "the music stopped". Note the feed rides the app's render
/// loop, so "stalled" includes UI freezes (the scrub Freeze toggle, a heavy
/// main-thread hitch) — the easing below is what keeps a brief hitch
/// imperceptible instead of a dark pop.
pub const STALE_INPUT_SILENCE: Duration = Duration::from_millis(250);

/// Exponential fade time-constant for the stale-feed energy duck (mirrors the
/// DMX manager's silence master, `SILENCE_FADE_OUT_TAU`). A hard zero-step at
/// the staleness boundary would render one fully dark frame after any ≥250ms
/// main-thread hitch (the app reads the snapshot before re-feeding the
/// engine) and would flap the rig warm/dark on a sustained slow main loop;
/// the eased gain turns both into a gentle duck. Fresh input snaps the gain
/// back to 1.0 — live audio is the truth.
const STALE_FADE_TAU: f32 = 0.45;

// ============================================================================
// LIGHTING INPUT (app main thread → engine, every frame)
// ============================================================================

/// Audio analysis + optional rendering data sent from the app each frame.
/// The engine consumes this to drive CueEngine processing; the latest input is
/// cached so republish ticks can re-stamp its fields without fresh audio.
pub struct LightingInput {
    pub analysis: AudioAnalysis,
    /// Pre-extracted key signal from KeyDetector (if HarmonicGeometry active).
    /// None → engine uses its own audio-derived fallback.
    pub key_signal: Option<KeySignal>,
    /// Lighting metrics from HarmonicGeometry (if active).
    /// None → engine derives metrics from AudioAnalysis.
    /// Includes Collatz state when Mythos engine is active.
    pub metrics: Option<LightingMetrics>,
    /// COM-499 Phase B2: the resolved show clock (Link/ProDJ/audio-derived),
    /// resolved once per frame in the app above the DMX render throttle and
    /// stamped onto the published CueSnapshot so ALL providers + the LFO bank
    /// read one authoritative beat. `confidence` is already silence-gated.
    pub show_clock: crate::emerge::ShowClockSnapshot,
    /// Show-brain musical arc (debounced), resolved in the DMX manager each
    /// frame (the brain ticks even when DMX output is disabled). Drives the
    /// CueEngine buildup macro — one musical brain decides what a build is.
    pub arc_state: crate::emerge::ArcState,
    /// Show-brain intensity license 0..1 (genre-free percussiveness ×
    /// spectral fullness). Shapes the CueEngine Pump (bloom vs detonation)
    /// and thins strobe probability — an EDM vocal bridge softens the show.
    pub intensity_license: f32,
    /// Operator "Dom" toggle (the Quaternion-wheel `qs_dominant_only`): true
    /// = fixtures colour off the single dominant pitch (today's behaviour),
    /// false = obey the all-notes chromatic blend. One control governs the
    /// wheel and the rig.
    pub dominant_only: bool,
    /// The posture actually driving the engine (operator pin, or the Auto
    /// resolver's flip). Gates the CueEngine macro/kick-strobe and caps the
    /// snapshot tension bank — one brain decides the posture for ALL sinks.
    pub effective_style: crate::emerge::LightingStyle,
}

impl LightingInput {
    /// The engine's pre-first-input seed: a silent room. Bands zero, no
    /// harmonic engine, zero-confidence clock (the type's `Default` claims
    /// confidence 1.0 — wrong for "nothing heard yet"), neutral Auto posture.
    /// The resulting pre-input snapshot matches what `CueSnapshot::default()`
    /// published under the old producer on every field except `excitement`
    /// (a fresh CueEngine seeds 0.15 so lights respond immediately; with
    /// energy 0 and zone Silent every consumer renders the same look).
    pub fn silent() -> Self {
        Self {
            analysis: AudioAnalysis::default(),
            key_signal: None,
            metrics: None,
            show_clock: crate::emerge::ShowClockSnapshot {
                confidence: 0.0,
                ..Default::default()
            },
            arc_state: crate::emerge::ArcState::Silence,
            intensity_license: 1.0,
            dominant_only: true,
            effective_style: crate::emerge::LightingStyle::Auto,
        }
    }
}

// ============================================================================
// ENGINE UI STATE (engine → main thread, every tick)
// ============================================================================

/// Lightweight snapshot of CueEngine + trigger state for the UI.
/// Republished every engine tick so held/latched button display follows the
/// operator with or without audio.
#[derive(Clone, Debug)]
pub struct OscUiState {
    // CueEngine state
    pub current_zone: crate::emerge::EnergyZone,
    pub current_palette: crate::emerge::Palette,
    pub current_harmonic: HarmonicLabel,
    pub strobe_hold_active: bool,
    pub blackout_active: bool,
    pub zone_activity: [f32; 5],
    pub smoothed_metrics: (f32, f32, f32, f32),
    pub auto_blinder_active: bool,
    pub macro_phase: crate::emerge::AutoBlinderPhase,
    pub current_pattern: i32,
    pub excitement: f32,
    // Trigger state
    pub trigger_strobe_held: bool,
    pub trigger_blackout_held: bool,
    pub trigger_blinder_held: bool,
    pub trigger_fog_held: bool,
    /// Fog latch state — orthogonal to `trigger_fog_held`. UI shows a
    /// toggle button; latch + hold compose via `ManualTriggers::fog_active`.
    pub trigger_fog_latched: bool,
    pub trigger_uv_held: bool,
    /// UV latch state — orthogonal to `trigger_uv_held`. UI shows a toggle
    /// button; latch + hold compose via `ManualTriggers::uv_active`.
    pub trigger_uv_latched: bool,
    pub trigger_strobe_rate: f32,
}

impl Default for OscUiState {
    fn default() -> Self {
        Self {
            current_zone: crate::emerge::EnergyZone::Silent,
            current_palette: crate::emerge::Palette::CoolA,
            current_harmonic: HarmonicLabel::Atonal,
            strobe_hold_active: false,
            blackout_active: false,
            zone_activity: [0.0; 5],
            smoothed_metrics: (0.0, 0.0, 0.5, 0.0),
            auto_blinder_active: false,
            macro_phase: crate::emerge::AutoBlinderPhase::Normal,
            current_pattern: -1,
            excitement: 0.0,
            trigger_strobe_held: false,
            trigger_blackout_held: false,
            trigger_blinder_held: false,
            trigger_fog_held: false,
            trigger_fog_latched: false,
            trigger_uv_held: false,
            trigger_uv_latched: false,
            trigger_strobe_rate: 10.0,
        }
    }
}

// ============================================================================
// ENGINE TICK RESULT
// ============================================================================

/// What one engine tick produced. The hosting thread publishes `snapshot` +
/// `ui_state` to the shared Arcs and hands the message vecs (plus the engine
/// state the continuous tier reads) to the OSC output adapter.
pub struct EngineTick {
    pub snapshot: CueSnapshot,
    pub ui_state: OscUiState,
    /// CueEngine messages from `process()` — fresh-input ticks only, and only
    /// when no manual trigger override is active (engine-freeze-by-design).
    pub cue_messages: Vec<(String, Vec<rosc::OscType>)>,
    /// Held manual-trigger messages — generated EVERY tick while a trigger is
    /// active and OSC output is live, so the OSC wire keeps seeing the held
    /// state (and the strobe square wave keeps pulsing) with no audio.
    pub trigger_messages: Vec<(String, rosc::OscType)>,
    /// This tick's operator-LFO modulation (frozen between fresh inputs).
    pub lfo_mod: LfoModulation,
    /// Whether this tick consumed a fresh `LightingInput`.
    pub fresh: bool,
    /// The key signal resolved on a fresh tick (None on republish ticks) —
    /// the OSC continuous tier reads it.
    pub key_signal: Option<KeySignal>,
}

// ============================================================================
// LIGHTING ENGINE
// ============================================================================

pub struct LightingEngine {
    /// Decision engine for cue triggering (advances on fresh inputs only).
    cue_engine: CueEngine,
    /// Manual lighting triggers (Strobe, Blackout, Blinder, UV, Fog) — set
    /// audio-independently by the command path.
    manual_triggers: ManualTriggers,
    /// Operator lighting configuration (genre, reactivity, masters, targets —
    /// the OSC transport fields are read by the OSC adapter via `config()`).
    config: OscConfig,

    // === Fallback key signal (when HarmonicGeometry not active) ===
    /// Fallback key signal state with latching and hold.
    key_signal_state: KeySignalState,
    /// Smoothed balance derived from audio bands (fallback mode).
    fallback_balance: f32,
    /// Smoothed tension derived from audio characteristics (fallback mode).
    fallback_tension: f32,
    /// Frame counter for periodic diagnostic logging.
    diag_counter: u64,

    // === Operator LFO bank (COM-499 Phase B2/B4) ===
    /// Inert until an operator enables a slot; a default bank yields zero
    /// modulation, so this is 24/7-safe.
    lfo_bank: LfoBankConfig,
    last_lfo_eval: Option<Instant>,
    /// Frozen between fresh inputs: republish ticks re-stamp the last
    /// modulation instead of advancing the bank, so the LFO keeps its
    /// per-input cadence (byte-equivalence) and the rig doesn't pulse on in
    /// silence.
    last_lfo_mod: LfoModulation,

    // === Harmonic trackers (advance on fresh inputs only) ===
    /// EALP B5: holds the slow centroid baseline so the voice-leading
    /// surprise delta works across frames.
    tonnetz_tracker: TonnetzTracker,
    /// EALP B5 all-in: hysteresis-stable quality label + neo-Riemannian
    /// P/R/L move classification.
    chord_tracker: ChordTracker,
    /// All-notes colour: EMA of the chromatic centroid VECTOR (correct
    /// circular smoothing — averaging the angle directly would break at the
    /// wrap). Held across frames so the blended hue glides instead of
    /// jittering as inner voices shift.
    an_ema_x: f32,
    an_ema_y: f32,
    /// Tracker OUTPUTS cached on fresh ticks and re-stamped on republish
    /// ticks — re-running the trackers on the cached chroma would advance
    /// their EMAs off the per-input cadence.
    cached_tonnetz: TonnetzFrame,
    cached_chord: ChordFrame,
    /// (hue, sat, mode-flag) for the all-notes blend.
    cached_all_notes: (f32, f32, bool),

    // === Input cache (the audio decoupling) ===
    /// Latest `LightingInput`, seeded with `LightingInput::silent()` so the
    /// snapshot rebuild never needs a fresh audio frame.
    last_input: LightingInput,
    /// When the last FRESH input arrived. None = never (stale from birth).
    last_input_at: Option<Instant>,
    /// Stale-feed energy duck (1.0 = warm). Eases toward 0 with
    /// [`STALE_FADE_TAU`] once the feed has stalled past
    /// [`STALE_INPUT_SILENCE`]; snaps to 1.0 on fresh input. Multiplied onto
    /// the energy/band/confidence stamps in `build_snapshot` — exactly 1.0
    /// while the feed is live, so the fresh path stays byte-identical.
    stale_gain: f32,

    // === Engine wall-clock strobe square wave ===
    /// Phase in [0,1), integrated as `phase += dt * rate` per tick (dt capped
    /// so a hitch can't jump it) — the PUBLISHED snapshot carries a live
    /// `strobe_on` wave for soft consumers (Hue) with or without audio. The
    /// DMX wire keeps stamping its own wave from its own clock in-frame
    /// (byte-identical), so the two run at the same RATE with free-running
    /// relative phase. Phase (not absolute seconds) because an f32 seconds
    /// accumulator stops advancing in 10ms steps after ~3 days of 24/7
    /// uptime (ulp > dt), which would freeze the wave solid; phase stays
    /// small forever, and a rate change glides instead of jumping.
    strobe_phase: f32,
    last_tick_at: Option<Instant>,
}

impl LightingEngine {
    pub fn new() -> Self {
        Self {
            cue_engine: CueEngine::new(),
            manual_triggers: ManualTriggers::new(),
            config: OscConfig::default(),
            key_signal_state: KeySignalState::new(),
            fallback_balance: 0.5,
            fallback_tension: 0.3,
            diag_counter: 0,
            lfo_bank: LfoBankConfig::default(),
            last_lfo_eval: None,
            last_lfo_mod: LfoModulation::default(),
            tonnetz_tracker: TonnetzTracker::default(),
            chord_tracker: ChordTracker::default(),
            an_ema_x: 0.0,
            an_ema_y: 0.0,
            cached_tonnetz: TonnetzFrame::default(),
            cached_chord: ChordFrame::default(),
            cached_all_notes: (0.0, 1.0, false),
            last_input: LightingInput::silent(),
            last_input_at: None,
            stale_gain: 1.0,
            strobe_phase: 0.0,
            last_tick_at: None,
        }
    }

    // === Accessors (the thread command handler + OSC adapter read these) ===

    pub fn config(&self) -> &OscConfig {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut OscConfig {
        &mut self.config
    }

    pub fn set_config(&mut self, config: OscConfig) {
        self.config = config;
    }

    pub fn cue_engine(&self) -> &CueEngine {
        &self.cue_engine
    }

    pub fn manual_triggers(&self) -> &ManualTriggers {
        &self.manual_triggers
    }

    pub fn manual_triggers_mut(&mut self) -> &mut ManualTriggers {
        &mut self.manual_triggers
    }

    /// The cached input's analysis — the OSC continuous tier reads its
    /// normalizer debug fields on fresh ticks.
    pub fn last_analysis(&self) -> &AudioAnalysis {
        &self.last_input.analysis
    }

    // === Commands (audio-independent; the held state lands in the snapshot
    // on the next tick, ≤10ms at the host's 100Hz) ===

    pub fn trigger_press(&mut self, button: TriggerButton) {
        self.manual_triggers.on_press(button);
    }

    pub fn trigger_release(&mut self, button: TriggerButton) {
        self.manual_triggers.on_release(button);
    }

    pub fn set_strobe_rate(&mut self, hz: f32) {
        self.manual_triggers.set_strobe_rate(hz);
    }

    /// Force-enter BlindersHold macro from external trigger (MIDI / UI).
    /// Returns the OSC messages for the adapter to send immediately.
    pub fn force_buildup(&mut self) -> Vec<(String, Vec<rosc::OscType>)> {
        self.cue_engine.force_buildup()
    }

    /// COM-499 Phase B4: replace the operator LFO bank, carrying the running
    /// phase/output across the swap so a live tweak (waveform/target/depth)
    /// doesn't click. The editor's copy carries no runtime phase; the truth
    /// lives here.
    pub fn set_lfo_bank(&mut self, mut new_bank: LfoBankConfig) {
        for i in 0..new_bank.slots.len() {
            new_bank.slots[i].phase = self.lfo_bank.slots[i].phase;
            new_bank.slots[i].output = self.lfo_bank.slots[i].output;
        }
        self.lfo_bank = new_bank;
    }

    /// Test seam: a long-settled stall — input cache aged past the staleness
    /// window AND the energy duck fully ramped.
    #[cfg(test)]
    pub fn force_stale(&mut self) {
        self.last_input_at = Some(Instant::now() - STALE_INPUT_SILENCE * 2);
        self.stale_gain = 0.0;
    }

    /// Test seam: age the input cache just past the staleness window WITHOUT
    /// touching the duck gain — for asserting the fade is eased, not a step.
    #[cfg(test)]
    pub fn force_stale_age_only(&mut self) {
        self.last_input_at = Some(Instant::now() - STALE_INPUT_SILENCE * 2);
    }

    // === The tick ===

    /// Produce this tick's snapshot + UI state (+ OSC messages for the
    /// adapter). `fresh_input` advances the engine; `None` republishes from
    /// the frozen engine state + CURRENT trigger/operator state. `osc_active`
    /// = the OSC output adapter has a live sender (gates message generation,
    /// which mutates the trigger strobe-pulse state — exactly the old
    /// enabled+sender gate). `use_maestro` = MaestroDMX target profile.
    pub fn tick(
        &mut self,
        fresh_input: Option<LightingInput>,
        osc_active: bool,
        use_maestro: bool,
    ) -> EngineTick {
        // Advance the engine wall-clock strobe square wave by real tick dt —
        // phase integration, never absolute time (see `strobe_phase` doc).
        let now = Instant::now();
        let tick_dt = self
            .last_tick_at
            .map(|t| (now - t).as_secs_f32().min(0.1))
            .unwrap_or(0.0);
        self.last_tick_at = Some(now);
        let strobe_hz = self.manual_triggers.config.strobe_rate_hz.clamp(1.0, 20.0);
        self.strobe_phase = (self.strobe_phase + tick_dt * strobe_hz).rem_euclid(1.0);
        let strobe_on = self.strobe_phase < 0.5;

        let fresh = fresh_input.is_some();
        let mut cue_messages: Vec<(String, Vec<rosc::OscType>)> = Vec::new();
        let mut trigger_messages: Vec<(String, rosc::OscType)> = Vec::new();
        let mut key_signal_out: Option<KeySignal> = None;

        if let Some(li) = fresh_input {
            // COM-499 Phase B2: eval the operator LFO bank ONCE per fresh
            // input from the resolved show clock, so the SAME modulation
            // reaches the OSC sender AND the CueSnapshot fan-out
            // (Hue/WLED/DMX). dt is inter-input time (~60Hz audio cadence);
            // tempo-sync slots re-derive phase from the clock so dt jitter
            // can't drift them, and dt is clamped in the bank for stalls.
            let lfo_dt = self
                .last_lfo_eval
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(0.0);
            self.last_lfo_eval = Some(Instant::now());
            self.last_lfo_mod = self.lfo_bank.eval(
                lfo_dt,
                li.show_clock.beat,
                li.show_clock.confidence >= LFO_CONFIDENCE_MIN,
            );

            // Stamp the show brain's arc + percussiveness + posture before the
            // engine processes: the buildup macro keys on the arc (one musical
            // brain — the CueEngine no longer second-guesses builds with its
            // own energy trend), Pump shapes its detonation by the atmosphere
            // signal, and the style gates read the EFFECTIVE posture (operator
            // pin, or the Auto resolver's flip).
            self.cue_engine.set_arc(li.arc_state);
            self.cue_engine.set_intensity_license(li.intensity_license);
            self.cue_engine.set_effective_style(li.effective_style);

            self.diag_counter += 1;

            // Approximate dt from frame counter (assume ~60fps; fallback state).
            let dt = 1.0 / 60.0;

            // === Always update fallback key signal state from audio ===
            self.key_signal_state.update(&li.analysis, dt);

            // === Build KeySignal from best available source ===
            let has_hg = li.key_signal.is_some();
            let key_signal = li
                .key_signal
                .unwrap_or_else(|| KeySignal::from_fallback(&self.key_signal_state));

            // === Build LightingMetrics (derive from audio when HG not available) ===
            let metrics_for_cue = match li.metrics {
                Some(ref m) => m.clone(),
                None => self.fallback_metrics(&li.analysis),
            };

            // Periodic diagnostic: log CueEngine input metrics (~every 2s at 60fps)
            if self.diag_counter % 120 == 1 {
                log::info!(
                    "[CUE] metrics: hg={} energy={:.2} balance={:.2} tension={:.2} stability={:.2} dom_pc={} kick={} tonal_s={:.2}",
                    has_hg, metrics_for_cue.energy, metrics_for_cue.balance,
                    metrics_for_cue.tension, metrics_for_cue.stability,
                    metrics_for_cue.dominant_pc, metrics_for_cue.kick,
                    self.key_signal_state.is_tonal_smooth,
                );
            }

            // === Manual Triggers (highest priority) vs CueEngine ===
            // While strobe/blackout/blinder is held the CueEngine deliberately
            // does NOT process (engine freeze; UV/fog excluded — see
            // ManualTriggers::is_any_active). Trigger message generation is
            // OSC-output work: gated on the adapter being live, because
            // `generate_messages` also advances the OSC strobe-pulse state.
            if self.manual_triggers.is_any_active() {
                if osc_active {
                    trigger_messages = self.manual_triggers.generate_messages(use_maestro);
                }
            } else {
                // Diagnostic: cue_engine.process() is pure CPU math and should
                // take well under 1ms. If it ever runs long it drags the host
                // thread below 60Hz and stretches the energy smoothing — the
                // audio->light lag.
                let cue_proc_start = Instant::now();
                cue_messages =
                    self.cue_engine
                        .process(&metrics_for_cue, &key_signal, &self.config);
                let cue_proc_ms = cue_proc_start.elapsed().as_secs_f32() * 1000.0;
                if cue_proc_ms > 10.0 {
                    log::warn!(
                        "[OSC-DIAG] cue_engine.process() took {:.1}ms (expect <1ms) — CueEngine is the cue-cadence bottleneck",
                        cue_proc_ms,
                    );
                }
            }

            // === Harmonic trackers (fresh inputs only; outputs cached for
            // republish ticks). Only meaningful where the chroma vector
            // exists, so gated on metrics being present — matching the
            // snapshot stamp gate below. ===
            if let Some(ref m) = li.metrics {
                self.cached_tonnetz = self.tonnetz_tracker.update(&m.chroma);
                self.cached_chord = self.chord_tracker.update(&m.chroma);

                // All-notes colour blend (the "obey all present notes" mode
                // the operator selects by un-checking "Dom"). Energy-weighted
                // chromatic centroid of the 12 chroma bins, biased toward the
                // latched dominant so the colour still RESPECTS the loudest
                // note, then EMA-smoothed for a glide.
                const AN_DOMINANT_EMPHASIS: f32 = 0.8; // dominant's extra pull
                const AN_EMA_ALPHA: f32 = 0.20; // hue glide smoothing
                const AN_TONAL_FULL: f32 = 0.6; // chroma energy → full trust
                let dominant_pc = self.cue_engine.last_dominant_pc();
                let (cx, cy, ctotal) = chroma_centroid(&m.chroma);
                let (dx, dy, dtotal) =
                    emphasize_dominant(cx, cy, ctotal, dominant_pc, AN_DOMINANT_EMPHASIS);
                self.an_ema_x += AN_EMA_ALPHA * (dx - self.an_ema_x);
                self.an_ema_y += AN_EMA_ALPHA * (dy - self.an_ema_y);
                let blended_hue = centroid_hue(self.an_ema_x, self.an_ema_y);
                let concentration =
                    centroid_concentration(self.an_ema_x, self.an_ema_y, dtotal.max(1e-6));
                // Tonal-energy trust: silence/noise (flat, low chroma)
                // crossfades the hue back to the stable latched dominant; a
                // real chord keeps its blend. (Concentration drives
                // SATURATION, not the fallback — a spread chord is still
                // tonal, it just washes whiter.)
                let tonal = (ctotal / AN_TONAL_FULL).clamp(0.0, 1.0);
                let dom_hue = dominant_pc as f32 / 12.0;
                // Shortest-arc hue lerp toward the dominant when un-tonal.
                let d = (blended_hue - dom_hue + 0.5).rem_euclid(1.0) - 0.5;
                let an_hue = (dom_hue + d * tonal).rem_euclid(1.0);
                // Spread chord → whiter (down to 0.45), single note → vivid.
                let an_sat = 0.45 + 0.55 * concentration;
                self.cached_all_notes = (an_hue, an_sat, !li.dominant_only);
            }

            key_signal_out = Some(key_signal);
            self.last_input = li;
            self.last_input_at = Some(now);
        } else if self.manual_triggers.is_any_active() && osc_active {
            // No fresh input — the decoupling fix: the OSC wire keeps seeing
            // the held trigger state (and the strobe pulse keeps alternating)
            // on the engine's own clock instead of waiting for audio.
            trigger_messages = self.manual_triggers.generate_messages(use_maestro);
        }

        let stale = self
            .last_input_at
            .map(|t| t.elapsed() >= STALE_INPUT_SILENCE)
            .unwrap_or(true);
        // Stale-feed energy duck: ease toward dark on a stalled feed, snap
        // back on fresh input (live audio is the truth; the DMX silence
        // master's own fade-in smooths the rig side). Exactly 1.0 while the
        // feed is live, so multiplying it in stays byte-exact.
        if fresh {
            self.stale_gain = 1.0;
        } else if stale {
            self.stale_gain *= (-tick_dt / STALE_FADE_TAU).exp();
        }

        let snapshot = self.build_snapshot(stale, strobe_on);
        let ui_state = self.build_ui_state();

        EngineTick {
            snapshot,
            ui_state,
            cue_messages,
            trigger_messages,
            lfo_mod: self.last_lfo_mod,
            fresh,
            key_signal: key_signal_out,
        }
    }

    /// Rebuild the published snapshot from the current engine state + the
    /// cached input. Pure read — all stateful advancement happens on the
    /// fresh path in `tick`, so calling this on every 100Hz tick cannot
    /// change time constants.
    fn build_snapshot(&self, stale_silent: bool, strobe_on: bool) -> CueSnapshot {
        let li = &self.last_input;
        let triggers = &self.manual_triggers;
        let mut cue_snap = CueSnapshot::from_cue_engine(
            &self.cue_engine,
            &self.config.genre_profile,
            self.config.reactivity,
            self.config.global_brightness,
            self.config.uv_level,
            triggers.strobe_held,
            triggers.blackout_held,
            triggers.blinder_held,
            triggers.uv_active(),
            triggers.fog_active(),
        );

        // Stale-feed energy duck: `g` is exactly 1.0 while the feed is live
        // (fresh path byte-identical: x * 1.0 is exact in f32) and eases
        // toward 0 once the feed has stalled — the engine-smoothed energy
        // fields (frozen at pre-stall values) and the cached bands settle
        // dark instead of riding stale loudness, without the one-frame dark
        // pop a hard zero-step would render after a brief main-thread hitch.
        // Trigger/manual/operator state above is untouched — that's the
        // point of republishing.
        let g = self.stale_gain;
        cue_snap.energy *= g;
        cue_snap.energy_slow *= g;
        cue_snap.energy_slope *= g;
        // Populate normalized audio band levels for MultiBand mapping mode
        cue_snap.bass = li.analysis.bass * g;
        cue_snap.mid = li.analysis.mid * g;
        cue_snap.high = li.analysis.high * g;
        // Transients are booleans — suppressed outright once stale (a frozen
        // kick=true must not replay at 100Hz; one suppressed kick during a
        // boundary hitch is invisible).
        cue_snap.kick = li.analysis.kick && !stale_silent;
        // Baseline onset = kick: covers the no-harmonic-engine path where
        // li.metrics is None (mirrors the old osc/mod.rs LightingMetrics
        // fallback onset:=kick). Upgraded to the spectral-flux onset from
        // LightingMetrics below when the engine is live.
        cue_snap.onset = cue_snap.kick;

        // Populate Collatz + harmonic field state from LightingMetrics
        // (available to Hue, WLED, DMX). Pure copies of the cached input —
        // on republish ticks these are exactly the values the last published
        // snapshot carried.
        if let Some(ref m) = li.metrics {
            cue_snap.collatz_heat = m.collatz_heat;
            cue_snap.collatz_phase = m.collatz_phase;
            cue_snap.collatz_intensity = m.collatz_intensity;
            cue_snap.collatz_breath = m.collatz_breath;
            cue_snap.collatz_act = m.collatz_act;
            if !stale_silent {
                // transient flag -> LFO/cue bursts (spectral-flux onset,
                // distinct from kick); a stalled feed must not replay it.
                cue_snap.onset = m.onset;
            }
            // Emerge field -> autonomous lighting (Resonator 63960211): stamp
            // the field rotation phase + normalized quaternion ports so
            // per-fixture intent reads them off `snapshot` (the same seam as
            // `onset`/collatz). The two GigBAR movers derive their pan/tilt
            // entirely from these.
            cue_snap.phase = m.phase;
            cue_snap.qc_ports = m.qc_ports;
            // EALP B1: journey phase (HeroAct) + time-in-phase -> lighting
            // macro (Birth/Lock/Dive/Afterglow govern intensity + movement).
            cue_snap.journey_phase = m.journey_phase;
            cue_snap.journey_phase_time = m.journey_phase_time;
            // EALP B2: harmonic coherence -> saturation + movement depth.
            cue_snap.trust = m.trust;
            // EALP B3: quaternion overlay -> accent punch + port tracking.
            cue_snap.qc_strength = m.qc_strength;
            cue_snap.qc_active_port = m.qc_active_port;
            // EALP B4: symmetry order -> mover phase-grouping width.
            cue_snap.symmetry = m.symmetry;
            // EALP B5: tonal-circle embedding + chord quality / P/R/L move —
            // tracker OUTPUTS cached on the fresh path (re-running the
            // trackers here would advance their EMAs off-cadence).
            cue_snap.tonnetz_hue = self.cached_tonnetz.hue;
            cue_snap.tonnetz_strength = self.cached_tonnetz.strength;
            cue_snap.tonnetz_delta = self.cached_tonnetz.delta;
            cue_snap.chord_quality = self.cached_chord.quality;
            cue_snap.chord_prl_move = self.cached_chord.prl_move;
            cue_snap.chord_prl_env = self.cached_chord.prl_env;
            // The rest of what the chord tracker hears: root, the any-change
            // accent (with how far and which way the root moved), harmonic
            // rhythm — and where the harmony sits on the circle of fifths.
            // These drive the mover steering + the colour reaction to a
            // progression. The transient is suppressed on a stalled feed like
            // `onset` (a frozen accent must not replay).
            cue_snap.chord_root = self.cached_chord.root;
            cue_snap.chord_change_env =
                if stale_silent { 0.0 } else { self.cached_chord.change_env };
            cue_snap.chord_root_motion = self.cached_chord.root_motion;
            cue_snap.chord_root_dir = self.cached_chord.root_dir;
            cue_snap.harmonic_rhythm = self.cached_chord.rhythm;
            cue_snap.tonnetz_x = self.cached_tonnetz.x;
            cue_snap.tonnetz_y = self.cached_tonnetz.y;
            let (an_hue, an_sat, an_on) = self.cached_all_notes;
            cue_snap.all_notes_hue = an_hue;
            cue_snap.all_notes_sat = an_sat;
            cue_snap.all_notes_color = an_on;
        }

        // COM-499 Phase B2: stamp the resolved show clock so ALL providers
        // (Hue/WLED/OSC) + the LFO bank read one authoritative beat. DMX
        // reads the clock in-frame; this is the soft-consumer fan-out copy.
        // Confidence is silence-gated upstream in app.rs, and zeroed here
        // when the feed itself stalls so soft consumers can't lock to a
        // ghost tempo through a stall.
        cue_snap.clock_beat = li.show_clock.beat;
        // Confidence ducks with the stale gain so soft consumers can't lock
        // to a ghost tempo through a stall (g == 1.0 while the feed is live).
        cue_snap.clock_confidence = li.show_clock.confidence * g;
        cue_snap.intensity_license = li.intensity_license;
        // Operator strobe rate → the DMX manager builds the global strobe
        // square wave at this Hz (UI "Strobe Rate" slider).
        cue_snap.strobe_rate_hz = self.manual_triggers.config.strobe_rate_hz;
        // Master brightness ceiling from the effective lighting style
        // (Banger 1.0 / Energetic 0.8 / Chill 0.5) — caps every emitter.
        cue_snap.style_brightness_ceiling = li.effective_style.brightness_ceiling();
        // Style cap on the autopilot segment tension bank: the bank fills
        // license-independently (tension + energy slope), so Chill's measured
        // pixel pace needs this explicit ceiling. Reads the EFFECTIVE
        // posture, so the Auto resolver's flips pace the pixels too.
        cue_snap.tension_bank = cue_snap
            .tension_bank
            .min(li.effective_style.segment_tension_cap());
        // COM-499 Phase B2: apply the SAME LFO modulation onto the fan-out
        // snapshot so Hue/WLED/DMX get it too (OSC gets it via the adapter).
        // The operator master is the base; the bank rides on top, clamped to
        // each target's range. Frozen between fresh inputs.
        cue_snap.global_brightness =
            add_clamped(cue_snap.global_brightness, self.last_lfo_mod.brightness, 0.0, 1.0);
        cue_snap.reactivity =
            add_clamped(cue_snap.reactivity, self.last_lfo_mod.reactivity, 0.0, 2.0);
        // The engine's own wall-clock strobe square wave: live with or
        // without audio, so soft consumers (Hue) flash a held STROBE at the
        // operator's rate. The DMX manager overwrites this with its OWN
        // clock's wave before rendering (wire bytes unchanged) — same rate,
        // free-running relative phase between Hue and the rig.
        cue_snap.strobe_on = strobe_on;

        cue_snap
    }

    fn build_ui_state(&self) -> OscUiState {
        let cue = &self.cue_engine;
        let trigs = &self.manual_triggers;
        OscUiState {
            current_zone: cue.current_zone(),
            current_palette: cue.current_palette(),
            current_harmonic: cue.current_harmonic(),
            strobe_hold_active: cue.strobe_hold_active(),
            blackout_active: cue.blackout_active(),
            zone_activity: cue.zone_activity(),
            smoothed_metrics: cue.smoothed_metrics(),
            auto_blinder_active: cue.auto_blinder_active(),
            macro_phase: cue.auto_blinder_phase(),
            current_pattern: cue.current_pattern(),
            excitement: cue.excitement(),
            trigger_strobe_held: trigs.strobe_held,
            trigger_blackout_held: trigs.blackout_held,
            trigger_blinder_held: trigs.blinder_held,
            trigger_fog_held: trigs.fog_held,
            trigger_fog_latched: trigs.fog_latched,
            trigger_uv_held: trigs.uv_held,
            trigger_uv_latched: trigs.uv_latched,
            trigger_strobe_rate: trigs.config.strobe_rate_hz,
        }
    }

    /// Derive meaningful metrics from AudioAnalysis when HarmonicGeometry is
    /// not active (moved verbatim from the old `OscManager::update`).
    fn fallback_metrics(&mut self, analysis: &AudioAnalysis) -> LightingMetrics {
        // Balance: spectral brightness ratio — more highs = higher balance
        let band_sum = analysis.bass + analysis.mid + analysis.high + 0.001;
        let raw_balance = (analysis.mid + analysis.high) / band_sum;
        self.fallback_balance += (raw_balance - self.fallback_balance) * 0.04;

        // Tension: combination of instability and tonal complexity
        let instability = 1.0 - analysis.stability;
        let tonal_tension = if analysis.is_tonal {
            analysis.tonal_separation
        } else {
            0.6
        };
        let raw_tension = instability * 0.5 + tonal_tension * 0.5;
        self.fallback_tension += (raw_tension - self.fallback_tension) * 0.06;

        // Use latched dominant_pc from KeySignalState (stable, no jitter)
        let dominant_changed = self.key_signal_state.dominant_changed_pulse;
        let dominant_pc = self.key_signal_state.dominant_pc_latched;

        // Polarity from KeySignalState (slow, hysteresis-gated)
        let polarity_changed = self.key_signal_state.polarity_changed_pulse;
        let polarity_quadrant = if self.key_signal_state.polarity_warm { 0 } else { 2 };

        LightingMetrics {
            energy: analysis.energy_vis,
            stability: analysis.stability,
            balance: self.fallback_balance,
            tension: self.fallback_tension,
            beat_confidence: 0.5,
            onset: analysis.kick,
            kick: analysis.kick,
            onset_strength: analysis.onset_strength,
            dominant_changed,
            dominant_pc,
            polarity_quadrant,
            polarity_prev_quadrant: if self.key_signal_state.polarity_warm { 2 } else { 0 },
            polarity_changed,
            major_likelihood: if self.key_signal_state.polarity_warm { 0.65 } else { 0.35 },
            // Frequency bands for macro state machine
            bass: analysis.bass,
            mid: analysis.mid,
            high: analysis.high,
            // Raw energy for true silence detection
            raw_energy: analysis.raw_bass + analysis.raw_mid + analysis.raw_high,
            raw_bass: analysis.raw_bass,
            raw_high: analysis.raw_high,
            // Collatz not available in fallback (Mythos not active)
            collatz_heat: 0.0,
            collatz_phase: 0.0,
            collatz_intensity: 0.0,
            collatz_breath: 0.0,
            collatz_act: 0,
            // No harmonic engine → neutral field: centered phase + neutral ports.
            // qc_ports = 0.5 centers the A3 pan formula (base = 0.5 + (0.5-0.5)*k)
            // so a beam never slams to pan=0 pre-init / on the no-engine path.
            phase: 0.0,
            qc_ports: [0.5; 4],
            // No engine → journey defaults to Birth at t=0 (emergence start).
            journey_phase: HeroAct::Birth,
            journey_phase_time: 0.0,
            // No engine → full coherence so B2 is a no-op (no desaturation /
            // movement relax) on the fallback path; the live field overwrites.
            trust: 1.0,
            // No lattice → qc neutral: 0 strength (Part A baseline accent) +
            // port 0 (Part A 0/2 base) — B3 is a no-op on the fallback path.
            qc_strength: 0.0,
            qc_active_port: 0,
            // No engine → base symmetry 12 (mover grouping reduces to Part A
            // counter-phase on the 2-mover rig regardless of this value).
            symmetry: 12,
            // No engine → atonal chroma (B5 centroid collapses to 0 strength).
            chroma: [0.0; 12],
        }
    }
}

impl Default for LightingEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emerge::cue::TriggerButton;

    fn fresh_input(energy: f32) -> LightingInput {
        LightingInput {
            analysis: AudioAnalysis {
                bass: energy,
                mid: energy,
                high: energy,
                energy_vis: energy,
                ..Default::default()
            },
            ..LightingInput::silent()
        }
    }

    // === The bug this engine exists to fix: a held manual GLOBAL trigger
    // must reach the published snapshot with NO LightingInput — no audio
    // frame, ever. ===

    #[test]
    fn held_blinder_reaches_snapshot_without_any_lighting_input() {
        let mut e = LightingEngine::new();
        e.trigger_press(TriggerButton::Blinder);
        let t = e.tick(None, false, false);
        assert!(t.snapshot.trigger_blinder, "held BLINDER must land in the published snapshot with no audio input");
        assert!(t.ui_state.trigger_blinder_held, "UI state must republish the held trigger too");
        assert!(!t.fresh);
    }

    #[test]
    fn held_strobe_and_uv_reach_snapshot_without_input() {
        let mut e = LightingEngine::new();
        e.trigger_press(TriggerButton::Strobe);
        e.trigger_press(TriggerButton::Uv);
        let t = e.tick(None, false, false);
        assert!(t.snapshot.trigger_strobe);
        assert!(t.snapshot.trigger_uv);
        assert!(t.snapshot.manual_light_override());
    }

    #[test]
    fn trigger_release_clears_snapshot_without_input() {
        let mut e = LightingEngine::new();
        e.trigger_press(TriggerButton::Blinder);
        assert!(e.tick(None, false, false).snapshot.trigger_blinder);
        e.trigger_release(TriggerButton::Blinder);
        assert!(!e.tick(None, false, false).snapshot.trigger_blinder);
    }

    #[test]
    fn latched_uv_reaches_snapshot_without_input() {
        let mut e = LightingEngine::new();
        e.manual_triggers_mut().toggle_uv_latch();
        let t = e.tick(None, false, false);
        assert!(t.snapshot.trigger_uv, "UV latch must land without audio");
        assert!(t.ui_state.trigger_uv_latched);
    }

    #[test]
    fn strobe_rate_change_reaches_snapshot_without_input() {
        let mut e = LightingEngine::new();
        e.set_strobe_rate(17.0);
        let t = e.tick(None, false, false);
        assert!((t.snapshot.strobe_rate_hz - 17.0).abs() < 1e-6);
    }

    // === Staleness: a stalled feed silences the bands but keeps publishing
    // trigger/operator state. ===

    #[test]
    fn stale_input_silences_bands_but_keeps_trigger() {
        let mut e = LightingEngine::new();
        // Feed one energetic frame, then stall long enough for the duck to
        // settle (force_stale = aged feed + fully ramped gain).
        let t = e.tick(Some(fresh_input(0.9)), false, false);
        assert!(t.fresh);
        assert!(t.snapshot.bass > 0.8, "fresh bands must stamp");
        e.trigger_press(TriggerButton::Blinder);
        e.force_stale();
        let t = e.tick(None, false, false);
        assert_eq!(t.snapshot.bass, 0.0, "settled stale feed must read silent");
        assert_eq!(t.snapshot.energy, 0.0, "frozen smoothed energy must be silenced");
        assert_eq!(t.snapshot.clock_confidence, 0.0, "no ghost tempo through a stall");
        assert!(!t.snapshot.kick, "frozen transients must not replay");
        assert!(t.snapshot.trigger_blinder, "trigger state must survive staleness");
    }

    #[test]
    fn stale_fade_is_eased_not_a_step() {
        let mut e = LightingEngine::new();
        e.tick(Some(fresh_input(0.9)), false, false);
        // Age the feed just past the window WITHOUT settling the gain: the
        // very next tick must duck gently, not slam to black — this is what
        // keeps a ~300ms main-thread hitch imperceptible on the rig.
        e.force_stale_age_only();
        let t = e.tick(None, false, false);
        assert!(
            t.snapshot.bass > 0.7,
            "one tick past the staleness boundary must still be warm (eased fade), got {}",
            t.snapshot.bass
        );
        // And a fresh frame snaps it straight back to full.
        let t = e.tick(Some(fresh_input(0.9)), false, false);
        assert!(t.snapshot.bass > 0.85, "fresh input must snap the duck back to 1.0");
    }

    #[test]
    fn republish_within_window_keeps_cached_bands() {
        let mut e = LightingEngine::new();
        e.tick(Some(fresh_input(0.9)), false, false);
        // Immediately republish (well inside the staleness window): the
        // cached bands persist — identical to what the old producer left
        // in the shared Arc between inputs.
        let t = e.tick(None, false, false);
        assert!(t.snapshot.bass > 0.8);
        assert!(!t.fresh);
    }

    #[test]
    fn never_fed_engine_publishes_silent_snapshot() {
        let mut e = LightingEngine::new();
        let t = e.tick(None, false, false);
        assert_eq!(t.snapshot.bass, 0.0);
        assert_eq!(t.snapshot.clock_confidence, 0.0);
        assert_eq!(t.snapshot.style_brightness_ceiling, 1.0);
        assert_eq!(t.snapshot.intensity_license, 1.0);
    }

    // === OSC wire: held-trigger messages keep flowing on the engine's own
    // clock when the adapter is live. ===

    #[test]
    fn held_trigger_emits_osc_messages_without_input_when_active() {
        let mut e = LightingEngine::new();
        e.trigger_press(TriggerButton::Blinder);
        let t = e.tick(None, true, false);
        assert!(
            t.trigger_messages.iter().any(|(a, v)| a == "/triggers/blinder"
                && matches!(v, rosc::OscType::Bool(true))),
            "held blinder must keep emitting on no-input ticks"
        );
        // And NOT when the OSC output is dark (no sender) — generation also
        // advances the strobe-pulse state, so it stays gated like before.
        let t = e.tick(None, false, false);
        assert!(t.trigger_messages.is_empty());
    }

    // === Engine wall-clock strobe wave is live on the published snapshot ===

    #[test]
    fn published_strobe_wave_alternates_without_audio() {
        let mut e = LightingEngine::new();
        e.set_strobe_rate(20.0); // 50ms period, 25ms half-phase
        e.trigger_press(TriggerButton::Strobe);
        let first = e.tick(None, false, false).snapshot.strobe_on;
        let mut flipped = false;
        // Walk the wall clock past a half-period; the wave must flip.
        for _ in 0..12 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            if e.tick(None, false, false).snapshot.strobe_on != first {
                flipped = true;
                break;
            }
        }
        assert!(flipped, "published strobe_on must alternate on the engine clock in silence");
    }

    // === Fresh-path stamps (the republish path must not regress them) ===

    #[test]
    fn fresh_input_stamps_clock_and_style() {
        let mut e = LightingEngine::new();
        let mut li = fresh_input(0.5);
        li.show_clock.beat = 42.5;
        li.show_clock.confidence = 0.9;
        li.intensity_license = 0.7;
        li.effective_style = crate::emerge::LightingStyle::Chill;
        let t = e.tick(Some(li), false, false);
        assert_eq!(t.snapshot.clock_beat, 42.5);
        assert!((t.snapshot.clock_confidence - 0.9).abs() < 1e-6);
        assert!((t.snapshot.intensity_license - 0.7).abs() < 1e-6);
        assert!(t.snapshot.style_brightness_ceiling < 1.0, "Chill must cap brightness");
        // The republish tick re-stamps the same cached values.
        let t2 = e.tick(None, false, false);
        assert_eq!(t2.snapshot.clock_beat, 42.5);
        assert!((t2.snapshot.clock_confidence - 0.9).abs() < 1e-6);
    }
}

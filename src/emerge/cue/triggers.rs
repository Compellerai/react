//! Manual Lighting Triggers
//!
//! Hold-to-activate buttons for manual lighting control (Strobe, Blackout, Blinder, UV, Fog).
//! These override automatic CueEngine behavior while held.

use std::time::{Duration, Instant}; // Duration used for strobe interval
use serde::{Deserialize, Serialize};

/// Types of manual trigger buttons
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TriggerButton {
    Strobe,
    Blackout,
    Blinder,
    /// UV mode — "blackout, except blacklight": only UV-capable emitters lit.
    Uv,
    Fog,
}

impl TriggerButton {
    pub fn all() -> &'static [TriggerButton] {
        &[
            TriggerButton::Strobe,
            TriggerButton::Blackout,
            TriggerButton::Blinder,
            TriggerButton::Uv,
            TriggerButton::Fog,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            TriggerButton::Strobe => "Strobe",
            TriggerButton::Blackout => "Blackout",
            TriggerButton::Blinder => "Blinder",
            TriggerButton::Uv => "UV",
            TriggerButton::Fog => "Fog",
        }
    }

    /// Icon for the button
    pub fn icon(&self) -> &'static str {
        match self {
            TriggerButton::Strobe => "\u{26A1}",    // Lightning
            TriggerButton::Blackout => "\u{25FC}",  // Black square
            TriggerButton::Blinder => "\u{2600}",   // Sun
            TriggerButton::Uv => "\u{1F7E3}",       // Purple circle (may not render in all fonts)
            TriggerButton::Fog => "\u{1F32B}",      // Fog cloud (may not render in all fonts)
        }
    }

    /// MIDI learn target name
    pub fn midi_target(&self) -> &'static str {
        match self {
            TriggerButton::Strobe => "trigger_strobe",
            TriggerButton::Blackout => "trigger_blackout",
            TriggerButton::Blinder => "trigger_blinder",
            TriggerButton::Uv => "trigger_uv",
            TriggerButton::Fog => "trigger_fog",
        }
    }
}

/// Configuration for manual triggers (saved to database)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TriggerConfig {
    /// Strobe pulse rate (Hz)
    pub strobe_rate_hz: f32,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            strobe_rate_hz: 10.0,
        }
    }
}

/// Manual trigger state machine
///
/// Handles hold-to-activate behavior for lighting triggers.
/// Priority order: Blackout > Blinder > Strobe > UV > Fog
/// All triggers are active while held, inactive on release.
pub struct ManualTriggers {
    /// Strobe button held
    pub strobe_held: bool,
    /// Blackout button held
    pub blackout_held: bool,
    /// Blinder button held
    pub blinder_held: bool,
    /// UV button held — UV mode: only UV-capable emitters lit, all else dark.
    pub uv_held: bool,
    /// UV latch: when true, UV mode stays on regardless of `uv_held` —
    /// mirrors `fog_latched` so the operator can run a blacklight passage
    /// hands-free. Toggled from the UI; orthogonal to the hold path so a
    /// tap-and-hold still works.
    pub uv_latched: bool,
    /// Fog button held
    pub fog_held: bool,
    /// Fog latch: when true, fog stays on regardless of `fog_held` so the
    /// operator can leave fog running hands-free (covers low ceilings on
    /// long ambient sets where holding the button would mash their hand).
    /// Toggled from the UI; orthogonal to the hold path so a tap-and-hold
    /// still works.
    pub fog_latched: bool,

    /// Configuration
    pub config: TriggerConfig,

    /// Time of last strobe pulse (for rate limiting)
    last_strobe_pulse: Instant,
    /// Strobe toggle state (for pulse effect)
    strobe_on: bool,
}

impl Default for ManualTriggers {
    fn default() -> Self {
        Self::new()
    }
}

impl ManualTriggers {
    pub fn new() -> Self {
        Self {
            strobe_held: false,
            blackout_held: false,
            blinder_held: false,
            uv_held: false,
            uv_latched: false,
            fog_held: false,
            fog_latched: false,
            config: TriggerConfig::default(),
            last_strobe_pulse: Instant::now(),
            strobe_on: false,
        }
    }

    /// `true` whenever fog should be emitting — either the button is held or
    /// the latch is engaged. Use this everywhere downstream instead of
    /// reading `fog_held` directly.
    pub fn fog_active(&self) -> bool {
        self.fog_held || self.fog_latched
    }

    /// `true` whenever UV mode is engaged — either the button is held or the
    /// latch is on. Use this everywhere downstream instead of reading
    /// `uv_held` directly (mirrors [`Self::fog_active`]).
    pub fn uv_active(&self) -> bool {
        self.uv_held || self.uv_latched
    }

    /// Flip the latch (UI calls this; the button itself stays hold-only).
    pub fn toggle_fog_latch(&mut self) {
        self.fog_latched = !self.fog_latched;
        log::info!("[Triggers] Fog latch -> {}", self.fog_latched);
    }

    /// Flip the UV latch (UI calls this; the button itself stays hold-only).
    pub fn toggle_uv_latch(&mut self) {
        self.uv_latched = !self.uv_latched;
        log::info!("[Triggers] UV latch -> {}", self.uv_latched);
    }

    /// Check if any manual trigger is currently overriding the automatic
    /// CueEngine output. **Fog is intentionally excluded** — it's purely
    /// scenic haze, never a lighting override. Including it here used to
    /// halt the CueEngine in `OscManager::update`, freezing every light at
    /// its last value while fog was on or latched. **UV is excluded for the
    /// same latch reason**: a latched UV passage can run for minutes, and
    /// halting the engine that long would freeze the energy/excitement
    /// state the rig resumes from. UV-mode darkness is enforced per-frame
    /// in the DMX/Hue mapping layers off `CueSnapshot::trigger_uv`; OSC
    /// consumers get the explicit `/triggers/uv` true/false edge instead.
    pub fn is_any_active(&self) -> bool {
        self.blackout_held || self.blinder_held || self.strobe_held
    }

    /// Handle button press
    pub fn on_press(&mut self, button: TriggerButton) {
        match button {
            TriggerButton::Strobe => {
                self.strobe_held = true;
                self.last_strobe_pulse = Instant::now();
                self.strobe_on = true;
            }
            TriggerButton::Blackout => {
                self.blackout_held = true;
            }
            TriggerButton::Blinder => {
                self.blinder_held = true;
            }
            TriggerButton::Uv => {
                self.uv_held = true;
            }
            TriggerButton::Fog => {
                self.fog_held = true;
            }
        }
        log::info!("[Triggers] {} pressed", button.as_str());
    }

    /// Handle button release
    pub fn on_release(&mut self, button: TriggerButton) {
        match button {
            TriggerButton::Strobe => {
                self.strobe_held = false;
                self.strobe_on = false;
            }
            TriggerButton::Blackout => {
                self.blackout_held = false;
            }
            TriggerButton::Blinder => {
                self.blinder_held = false;
            }
            TriggerButton::Uv => {
                self.uv_held = false;
            }
            TriggerButton::Fog => {
                self.fog_held = false;
            }
        }
        log::info!("[Triggers] {} released", button.as_str());
    }

    /// Check if button is currently held
    pub fn is_held(&self, button: TriggerButton) -> bool {
        match button {
            TriggerButton::Strobe => self.strobe_held,
            TriggerButton::Blackout => self.blackout_held,
            TriggerButton::Blinder => self.blinder_held,
            TriggerButton::Uv => self.uv_held,
            TriggerButton::Fog => self.fog_held,
        }
    }

    /// Generate OSC messages for current state (called on press and continuously while held)
    /// MaestroDMX spec: /triggers/{strobe,blackout,blinder,uv,fog} with boolean values
    pub fn generate_messages(&mut self, _use_maestro: bool) -> Vec<(String, rosc::OscType)> {
        let mut messages = Vec::new();

        // STROBE: Pulse on/off at configured rate while held
        if self.strobe_held {
            let interval = Duration::from_secs_f32(1.0 / self.config.strobe_rate_hz);
            if self.last_strobe_pulse.elapsed() >= interval {
                self.strobe_on = !self.strobe_on;
                self.last_strobe_pulse = Instant::now();
            }
            // Send current strobe state (alternates true/false at strobe_rate_hz)
            messages.push(("/triggers/strobe".to_string(), rosc::OscType::Bool(self.strobe_on)));
        }

        // BLACKOUT: Constant while held
        if self.blackout_held {
            messages.push(("/triggers/blackout".to_string(), rosc::OscType::Bool(true)));
        }

        // BLINDER: Constant while held
        if self.blinder_held {
            messages.push(("/triggers/blinder".to_string(), rosc::OscType::Bool(true)));
        }

        // UV: Constant while held OR latched — uv_active() is the single
        // source of truth, same latch rule as fog below.
        if self.uv_active() {
            messages.push(("/triggers/uv".to_string(), rosc::OscType::Bool(true)));
        }

        // FOG: Constant while held OR latched — fog_active() is the single
        // source of truth (see its doc). Checking `fog_held` alone here meant
        // OSC fog machines never saw the latch while DMX consumers (which
        // read fog_active()) did, splitting the rig.
        if self.fog_active() {
            messages.push(("/triggers/fog".to_string(), rosc::OscType::Bool(true)));
        }

        messages
    }

    /// Generate release messages when a trigger is released
    /// Sends boolean false to deactivate the trigger
    pub fn generate_release_messages(&self, button: TriggerButton, _use_maestro: bool) -> Vec<(String, rosc::OscType)> {
        let mut messages = Vec::new();

        match button {
            TriggerButton::Strobe => {
                messages.push(("/triggers/strobe".to_string(), rosc::OscType::Bool(false)));
            }
            TriggerButton::Blackout => {
                messages.push(("/triggers/blackout".to_string(), rosc::OscType::Bool(false)));
            }
            TriggerButton::Blinder => {
                messages.push(("/triggers/blinder".to_string(), rosc::OscType::Bool(false)));
            }
            TriggerButton::Uv => {
                // Same latch rule as Fog below: while the UV latch is on,
                // uv_active() stays true, so releasing the held button must
                // NOT send uv-off — DMX consumers (which read uv_active())
                // would keep UV mode while OSC consumers dropped it. The
                // latch-off transition emits `/triggers/uv false` from the
                // ToggleUvLatch handler in osc/thread.rs.
                if !self.uv_latched {
                    messages.push(("/triggers/uv".to_string(), rosc::OscType::Bool(false)));
                }
            }
            TriggerButton::Fog => {
                // Releasing the held button while the latch is engaged must
                // NOT send fog-off: the latch keeps fog active (fog_active()),
                // and DMX consumers keep emitting — an OSC `false` here would
                // turn OSC fog machines off while DMX fog stays on. The
                // latch-off transition itself emits `/triggers/fog false`
                // from the ToggleFogLatch handler in osc/thread.rs.
                if !self.fog_latched {
                    messages.push(("/triggers/fog".to_string(), rosc::OscType::Bool(false)));
                }
            }
        }

        messages
    }

    /// Set strobe rate
    pub fn set_strobe_rate(&mut self, hz: f32) {
        self.config.strobe_rate_hz = hz.clamp(1.0, 20.0);
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn fog_msgs(messages: &[(String, rosc::OscType)]) -> Vec<&rosc::OscType> {
        messages
            .iter()
            .filter(|(a, _)| a == "/triggers/fog")
            .map(|(_, v)| v)
            .collect()
    }

    // === Finding: fog latch must reach OSC consumers =========================

    #[test]
    fn latched_fog_emits_fog_true_without_hold() {
        let mut t = ManualTriggers::new();
        t.toggle_fog_latch(); // latch on, button NOT held
        assert!(t.fog_active());

        let messages = t.generate_messages(false);
        let fog = fog_msgs(&messages);
        assert_eq!(fog.len(), 1, "latched fog must emit /triggers/fog");
        assert!(
            matches!(fog[0], rosc::OscType::Bool(true)),
            "latched fog must emit true"
        );
    }

    #[test]
    fn fog_release_suppressed_while_latched() {
        let mut t = ManualTriggers::new();
        t.on_press(TriggerButton::Fog);
        t.toggle_fog_latch(); // latch on while held
        t.on_release(TriggerButton::Fog); // hold ends, latch stays

        // fog_active() is still true via the latch — the release path must
        // NOT send `/triggers/fog false`, or OSC fog machines turn off while
        // DMX fog (driven by fog_active()) stays on.
        assert!(t.fog_active());
        let messages = t.generate_release_messages(TriggerButton::Fog, false);
        assert!(
            fog_msgs(&messages).is_empty(),
            "release-false must be suppressed while the fog latch is on"
        );
    }

    #[test]
    fn fog_release_sends_false_when_not_latched() {
        let mut t = ManualTriggers::new();
        t.on_press(TriggerButton::Fog);
        t.on_release(TriggerButton::Fog);

        // No latch: plain hold-release behavior is unchanged.
        assert!(!t.fog_active());
        let messages = t.generate_release_messages(TriggerButton::Fog, false);
        let fog = fog_msgs(&messages);
        assert_eq!(fog.len(), 1);
        assert!(matches!(fog[0], rosc::OscType::Bool(false)));
    }

    #[test]
    fn held_fog_still_emits_true_with_latch_off() {
        let mut t = ManualTriggers::new();
        t.on_press(TriggerButton::Fog);

        let messages = t.generate_messages(false);
        let fog = fog_msgs(&messages);
        assert_eq!(fog.len(), 1);
        assert!(matches!(fog[0], rosc::OscType::Bool(true)));
    }

    // === UV mode mirrors the fog latch contract ==============================

    fn uv_msgs(messages: &[(String, rosc::OscType)]) -> Vec<&rosc::OscType> {
        messages
            .iter()
            .filter(|(a, _)| a == "/triggers/uv")
            .map(|(_, v)| v)
            .collect()
    }

    #[test]
    fn latched_uv_emits_uv_true_without_hold() {
        let mut t = ManualTriggers::new();
        t.toggle_uv_latch(); // latch on, button NOT held
        assert!(t.uv_active());

        let messages = t.generate_messages(false);
        let uv = uv_msgs(&messages);
        assert_eq!(uv.len(), 1, "latched UV must emit /triggers/uv");
        assert!(
            matches!(uv[0], rosc::OscType::Bool(true)),
            "latched UV must emit true"
        );
    }

    #[test]
    fn uv_release_suppressed_while_latched() {
        let mut t = ManualTriggers::new();
        t.on_press(TriggerButton::Uv);
        t.toggle_uv_latch(); // latch on while held
        t.on_release(TriggerButton::Uv); // hold ends, latch stays

        // uv_active() is still true via the latch — the release path must
        // NOT send `/triggers/uv false`, or OSC consumers would leave UV
        // mode while DMX (driven by uv_active()) stays in it.
        assert!(t.uv_active());
        let messages = t.generate_release_messages(TriggerButton::Uv, false);
        assert!(
            uv_msgs(&messages).is_empty(),
            "release-false must be suppressed while the UV latch is on"
        );
    }

    #[test]
    fn uv_release_sends_false_when_not_latched() {
        let mut t = ManualTriggers::new();
        t.on_press(TriggerButton::Uv);
        t.on_release(TriggerButton::Uv);

        // No latch: plain hold-release behavior is unchanged.
        assert!(!t.uv_active());
        let messages = t.generate_release_messages(TriggerButton::Uv, false);
        let uv = uv_msgs(&messages);
        assert_eq!(uv.len(), 1);
        assert!(matches!(uv[0], rosc::OscType::Bool(false)));
    }

    #[test]
    fn held_uv_still_emits_true_with_latch_off() {
        let mut t = ManualTriggers::new();
        t.on_press(TriggerButton::Uv);

        let messages = t.generate_messages(false);
        let uv = uv_msgs(&messages);
        assert_eq!(uv.len(), 1);
        assert!(matches!(uv[0], rosc::OscType::Bool(true)));
    }

    #[test]
    fn uv_does_not_halt_the_cue_engine() {
        // Like fog, UV is latch-capable and can stay on for minutes — it must
        // never trip is_any_active() (which halts CueEngine processing in
        // OscManager::update). UV darkness is a mapping-layer rule.
        let mut t = ManualTriggers::new();
        t.on_press(TriggerButton::Uv);
        t.toggle_uv_latch();
        assert!(!t.is_any_active(), "UV (held or latched) must not halt the CueEngine");
    }
}

//! Operator lighting style — a performance posture laid over the adaptive
//! engine.
//!
//! The show brain already runs a genre-free intensity dial (the intensity
//! license: percussiveness × spectral fullness). A style is the operator
//! pinning that dial's range and hard-banning vocabulary the license only
//! attenuates:
//!
//! - `Auto` — no overrides; the adaptive engine is the authority.
//! - `Chill` — never lasers, never strobes (lane, Pump flavor, kick strobe,
//!   8Hz blinder hold), no blinder macro; license capped low so movers ride
//!   the slow SCAN posture and pixel color stays measured.
//! - `Energetic` — lasers allowed (exposure caps still apply), strobes only
//!   as drop accents, never the sustained BlindersHold strobe.
//! - `Banger` — license floored high so the full vocabulary is licensed
//!   nearly always; true silence still reads dark because the arc classifier
//!   and energy gates are license-independent.
//!
//! Composition with genre is most-restrictive-wins and symmetric: Chill
//! zeroes a Hardstyle profile's tolerances, and Banger does NOT re-open an
//! ambient genre's `strobe_enabled: false` bans — it only floors the
//! license. Manual operator triggers (STROBE button, `force_buildup()`)
//! bypass style entirely: the operator always wins over the operator's own
//! posture.

use serde::{Deserialize, Serialize};

use crate::emerge::intent::FixtureGenreProfile;

/// License cap for Chill: below the 0.25 atmosphere governor (strobe/blinder
/// lanes stand down) and below the 0.4 Pump strobe-flavor gate; deep enough
/// into the SCAN crossfade that movers glide instead of chase.
const CHILL_LICENSE_CAP: f32 = 0.15;
/// License cap for Energetic: drops still detonate with strobe flavor
/// (>= 0.4) but the ceiling and movement never reach full-vocabulary heat.
const ENERGETIC_LICENSE_CAP: f32 = 0.65;
/// License floor for Banger: above every license gate so the vocabulary is
/// armed whenever the music (arc + energy gates) calls for it.
const BANGER_LICENSE_FLOOR: f32 = 0.85;
/// Energetic mover depth relative to the genre's designed movement.
const ENERGETIC_MOVEMENT_SCALE: f32 = 0.7;
/// Chill blinder ban: under the 0.35 density gate in the blinder lane.
const CHILL_DENSITY_CAP: f32 = 0.30;
/// Brightness ceiling for Energetic — the loud-but-not-peak posture holds
/// ~20% back so a banger drop still reads as a lift.
const ENERGETIC_BRIGHTNESS_CEIL: f32 = 0.80;
/// Brightness ceiling for Chill — calm material sits at half brightness so
/// the rig reads as restrained, with plenty of headroom for the music to lift.
const CHILL_BRIGHTNESS_CEIL: f32 = 0.50;
/// Chill cap on the autopilot segment tension bank. Mirrors
/// `AUTO_SEG_TENSION_HIGH` in `lighting_cue.rs`: at or below it the segment
/// look tops out at the measured Interference ripple and never enters the
/// tightening (<= 1-beat) chase.
const CHILL_SEGMENT_TENSION_CAP: f32 = 0.42;

/// Operator-selected lighting style (trigger-row dropdown, MIDI-bindable).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightingStyle {
    #[default]
    Auto,
    Chill,
    Energetic,
    Banger,
}

impl LightingStyle {
    pub const ALL: [LightingStyle; 4] = [
        LightingStyle::Auto,
        LightingStyle::Chill,
        LightingStyle::Energetic,
        LightingStyle::Banger,
    ];

    /// Clamp the smoothed intensity license into this style's band.
    /// Applied to the EMA state itself so a style switch lands the same
    /// frame (panic-cut / instant-heat semantics); the attack/decay taus
    /// still govern motion within the band.
    pub fn clamp_license(self, license: f32) -> f32 {
        match self {
            LightingStyle::Auto => license,
            LightingStyle::Chill => license.min(CHILL_LICENSE_CAP),
            LightingStyle::Energetic => license.min(ENERGETIC_LICENSE_CAP),
            LightingStyle::Banger => license.max(BANGER_LICENSE_FLOOR),
        }
    }

    /// Master brightness CEILING for this posture (0..1) — caps every
    /// emitter so the rig is visibly dimmer in calm styles and full-bright in
    /// a banger. Energetic/Chill hold back so the loud moments have somewhere
    /// to go; Auto is 1.0 here because the AUTO resolver swaps in a concrete
    /// posture (Chill/Energetic/Banger) whose ceiling is used instead.
    pub fn brightness_ceiling(self) -> f32 {
        match self {
            LightingStyle::Auto => 1.0,
            LightingStyle::Chill => CHILL_BRIGHTNESS_CEIL,
            LightingStyle::Energetic => ENERGETIC_BRIGHTNESS_CEIL,
            LightingStyle::Banger => 1.0,
        }
    }

    /// Whether the buildup macro (BlindersHold → Pump → Recover) may run at
    /// all. Composed with the operator's AUTO master arm: both must allow.
    pub fn allows_macro(self) -> bool {
        !matches!(self, LightingStyle::Chill)
    }

    /// Whether the sustained 8Hz BlindersHold white coil may auto-enter.
    /// Energetic keeps the macro for Pump drop response but never the hold.
    pub fn allows_blinder_hold(self) -> bool {
        matches!(self, LightingStyle::Auto | LightingStyle::Banger)
    }

    /// Whether the zone-gated kick strobe (cue engine `update_strobe`, which
    /// reaches the physical rig via `strobe_active`) may fire. The license
    /// only thins its probability, so the ban must be explicit.
    pub fn allows_kick_strobe(self, arc_is_drop: bool) -> bool {
        match self {
            LightingStyle::Auto | LightingStyle::Banger => true,
            LightingStyle::Chill => false,
            LightingStyle::Energetic => arc_is_drop,
        }
    }

    /// Compose the genre's fixture profile with this style,
    /// most-restrictive-wins. The hard lane gates in the fixture brain
    /// (laser_tolerance < 0.25, strobe_tolerance < 0.30, density < 0.35)
    /// stay style-unaware; Chill simply drives the fields under them.
    pub fn compose_genre(self, genre: FixtureGenreProfile) -> FixtureGenreProfile {
        match self {
            LightingStyle::Auto | LightingStyle::Banger => genre,
            LightingStyle::Chill => FixtureGenreProfile {
                strobe_tolerance: 0.0,
                laser_tolerance: 0.0,
                density: genre.density.min(CHILL_DENSITY_CAP),
                ..genre
            },
            LightingStyle::Energetic => FixtureGenreProfile {
                movement: genre.movement * ENERGETIC_MOVEMENT_SCALE,
                ..genre
            },
        }
    }

    /// Ceiling on the autopilot segment tension bank (pixel-bar look
    /// selection). 1.0 = no cap.
    pub fn segment_tension_cap(self) -> f32 {
        match self {
            LightingStyle::Chill => CHILL_SEGMENT_TENSION_CAP,
            _ => 1.0,
        }
    }

    /// Cycle order for the MIDI StyleNext binding.
    pub fn next(self) -> LightingStyle {
        match self {
            LightingStyle::Auto => LightingStyle::Chill,
            LightingStyle::Chill => LightingStyle::Energetic,
            LightingStyle::Energetic => LightingStyle::Banger,
            LightingStyle::Banger => LightingStyle::Auto,
        }
    }

    /// Stable string for persistence (db settings) and display.
    pub fn as_str(self) -> &'static str {
        match self {
            LightingStyle::Auto => "Auto",
            LightingStyle::Chill => "Chill",
            LightingStyle::Energetic => "Energetic",
            LightingStyle::Banger => "Banger",
        }
    }

    /// Parse a persisted string; unknown/missing values fall back to Auto so
    /// stale or hand-edited settings can never brick the engine posture.
    pub fn from_str(value: &str) -> LightingStyle {
        match value {
            "Chill" => LightingStyle::Chill,
            "Energetic" => LightingStyle::Energetic,
            "Banger" => LightingStyle::Banger,
            _ => LightingStyle::Auto,
        }
    }

    /// License clamp for an AUTO-RESOLVED posture. Differs from
    /// `clamp_license` in exactly one way: resolved Banger does NOT floor the
    /// license. The within-song dial (vocal bridges thinning strobes, drops
    /// re-arming — field-validated) keeps working inside an auto-Banger
    /// section; the resolver's job is the discrete vocabulary, and Banger
    /// bans nothing. An operator-PINNED Banger keeps the floor — that is the
    /// operator explicitly saying "go hard".
    pub fn auto_clamp_license(self, license: f32) -> f32 {
        match self {
            LightingStyle::Banger => license,
            other => other.clamp_license(license),
        }
    }
}

// ── Auto posture resolver ───────────────────────────────────────────────────
//
// When the operator style is Auto, EMeRGE flips between Chill / Energetic /
// Banger itself. CHILL IS THE DEFAULT: the resolver classifies each moment
// by how exceptional it is WITHIN THE SONG — a z-score of the unclamped
// license against its own running mean/σ — never by absolute level (the
// license sits 0.6–0.9 on almost any beat-driven track, so absolute
// thresholds read everything as hot; field lesson 2026-06-11, same
// adaptive-baseline principle as tension_dyn). Roughly +1σ above the
// track's own norm earns Energetic, +2σ earns Banger, and a real drop
// fast-promotes without waiting out a timer. Sustained evidence +
// hysteresis + minimum dwell keep the switch measured. The resolver reads
// the UNCLAMPED license — classifying from the clamped value would lock
// Chill in forever (clamp → low reading → stays Chill).

// ── Tuning ───────────────────────────────────────────────────────────────────
//
// The posture "drive" is a BLEND of two things, both in the same units:
//
//   drive = ABS_BIAS_GAIN·(license − ABS_NEUTRAL)   ← how loud it is (absolute)
//         + z                                        ← how much it's surging
//                                                       (deviation vs the
//                                                        track's own 60s norm)
//
// Pure-absolute pinned everything hot; pure-z over-chilled a consistently
// loud track (its loudness was its norm, so z≈0 → Chill, and the bangers
// never banged). The blend gives both: a loud track runs warm because the
// absolute term lifts the baseline, while quiet sections still chill and
// dynamic surges still drive the exciting moments. ABS_NEUTRAL (the license
// level that contributes zero warmth) and ABS_BIAS_GAIN (how much absolute
// loudness matters) are the primary warm/cool knobs.

/// License level that contributes zero absolute warmth. Below it the drive
/// is pulled negative (Chill); above it, positive (toward Energetic).
const ABS_NEUTRAL: f32 = 0.55;
/// How strongly absolute loudness biases the drive (in drive units per unit
/// license). Higher = louder material runs warmer. Primary warmth knob.
const ABS_BIAS_GAIN: f32 = 5.0;
/// Evidence needed to promote one step up (sustained drive above the bar).
const PROMOTE_EVIDENCE_S: f32 = 4.0;
/// Evidence needed to demote one step down.
const DEMOTE_EVIDENCE_S: f32 = 6.0;
/// Minimum time in a posture before a non-drop switch (anti-flap).
const MIN_DWELL_S: f32 = 6.0;
/// Tau of the running license mean/variance — long enough to characterize
/// the song's own norm, short enough to re-baseline across a track change.
const STATS_TAU_S: f32 = 60.0;
/// Floor on σ so railed-flat material (σ→0) cannot turn noise wiggles into
/// huge z-scores in the surge term.
const SIGMA_FLOOR: f32 = 0.11;
/// drive to enter Energetic (sustained).
const DRIVE_ENERGETIC_ENTER: f32 = 1.2;
/// drive to fall back from Energetic toward Chill (hysteresis).
const DRIVE_ENERGETIC_EXIT: f32 = 0.6;
/// drive to enter Banger (sustained, non-drop): loud AND surging.
const DRIVE_BANGER_ENTER: f32 = 2.4;
/// drive to fall back from Banger toward Energetic (hysteresis).
const DRIVE_BANGER_EXIT: f32 = 1.3;
/// Absolute license floor for ANY Banger entry (drive- or drop-driven):
/// even the loudest moment of an ambient track must not strobe.
const BANGER_ABS_MIN: f32 = 0.55;
/// Drop fast-path: a Drop/SustainPeak arc promotes straight to Banger
/// (bypassing evidence + dwell) when the drop's drive clears this bar AND
/// raw license is at least BANGER_ABS_MIN. A drop staying that hot holds
/// the vocabulary open (no demotion).
const DROP_PROMOTE_DRIVE: f32 = 1.2;

/// State machine resolving the Auto style into a concrete posture.
/// Pure (no clocks, no I/O): feed it dt + the unclamped license + whether
/// the arc reads as a drop/peak, read back the posture.
#[derive(Clone, Copy, Debug)]
pub struct AutoStyleResolver {
    resolved: LightingStyle,
    dwell_s: f32,
    up_evidence_s: f32,
    down_evidence_s: f32,
    /// Running mean / variance of the unclamped license (EMA, STATS_TAU_S)
    /// — the song's own norm that z-scores are measured against.
    license_mean: f32,
    license_var: f32,
    stats_seeded: bool,
}

impl Default for AutoStyleResolver {
    fn default() -> Self {
        Self {
            // Chill is the default posture: most of a song is its own norm;
            // the intense vocabulary is earned by exceptional moments.
            resolved: LightingStyle::Chill,
            dwell_s: 0.0,
            up_evidence_s: 0.0,
            down_evidence_s: 0.0,
            license_mean: 0.5,
            license_var: 0.0,
            stats_seeded: false,
        }
    }
}

impl AutoStyleResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// The currently resolved posture (always Chill/Energetic/Banger).
    pub fn resolved(&self) -> LightingStyle {
        self.resolved
    }

    fn switch_to(&mut self, posture: LightingStyle) {
        self.resolved = posture;
        self.dwell_s = 0.0;
        self.up_evidence_s = 0.0;
        self.down_evidence_s = 0.0;
    }

    /// Advance the resolver one frame. `raw_license` MUST be the unclamped
    /// intensity license; `arc_is_peak` = the debounced arc reads
    /// Drop/SustainPeak this frame.
    pub fn update(&mut self, dt: f32, raw_license: f32, arc_is_peak: bool) -> LightingStyle {
        self.dwell_s += dt;

        // Running stats of the song's own license level. Seeded on the
        // first sample so a session never opens with a phantom deviation.
        if !self.stats_seeded {
            self.license_mean = raw_license;
            self.license_var = 0.0;
            self.stats_seeded = true;
        }
        let alpha = 1.0 - (-dt / STATS_TAU_S).exp();
        let delta = raw_license - self.license_mean;
        self.license_mean += alpha * delta;
        self.license_var += alpha * (delta * delta - self.license_var);
        let sigma = self.license_var.max(0.0).sqrt().max(SIGMA_FLOOR);
        // Surge term: how exceptional is THIS moment vs the track's own norm.
        let z = (raw_license - self.license_mean) / sigma;
        // Posture drive = absolute loudness (per-track-centered) + surge.
        let drive = ABS_BIAS_GAIN * (raw_license - ABS_NEUTRAL) + z;

        // Drop fast-path: a peaked arc that's also loud-and/or-surging enough
        // (drive clears the bar) detonates immediately — skipping evidence +
        // dwell. The absolute term means a genuinely loud drop fires even on
        // a consistent banger; a quiet section's "drop" (low license) does
        // not. The license floor blocks ambient false peaks.
        let drop_detonates =
            arc_is_peak && drive >= DROP_PROMOTE_DRIVE && raw_license >= BANGER_ABS_MIN;
        if drop_detonates && self.resolved != LightingStyle::Banger {
            self.switch_to(LightingStyle::Banger);
            return self.resolved;
        }

        // Sustained-evidence accumulators, reset on contradiction so dips
        // and spikes have to be CONTINUOUS to move the posture (measured,
        // not jittery).
        match self.resolved {
            LightingStyle::Chill => {
                if drive >= DRIVE_ENERGETIC_ENTER {
                    self.up_evidence_s += dt;
                } else {
                    self.up_evidence_s = 0.0;
                }
                // Leaving the default is evidence-gated but not dwell-gated:
                // the song lifting the room without a long lag (a slam goes
                // straight to Banger via the drop fast-path above).
                if self.up_evidence_s >= PROMOTE_EVIDENCE_S {
                    self.switch_to(LightingStyle::Energetic);
                }
            }
            LightingStyle::Energetic => {
                if drive >= DRIVE_BANGER_ENTER && raw_license >= BANGER_ABS_MIN {
                    self.up_evidence_s += dt;
                } else {
                    self.up_evidence_s = 0.0;
                }
                if drive < DRIVE_ENERGETIC_EXIT {
                    self.down_evidence_s += dt;
                } else {
                    self.down_evidence_s = 0.0;
                }
                if self.dwell_s >= MIN_DWELL_S {
                    if self.up_evidence_s >= PROMOTE_EVIDENCE_S {
                        self.switch_to(LightingStyle::Banger);
                    } else if self.down_evidence_s >= DEMOTE_EVIDENCE_S {
                        self.switch_to(LightingStyle::Chill);
                    }
                }
            }
            LightingStyle::Banger => {
                // A sustained drop still hot enough holds the vocabulary open
                // — demotion evidence only accrues once the moment passes.
                let drop_holding = drop_detonates;
                if drive < DRIVE_BANGER_EXIT && !drop_holding {
                    self.down_evidence_s += dt;
                } else {
                    self.down_evidence_s = 0.0;
                }
                if self.dwell_s >= MIN_DWELL_S && self.down_evidence_s >= DEMOTE_EVIDENCE_S {
                    self.switch_to(LightingStyle::Energetic);
                }
            }
            // Resolver state is never Auto; normalize defensively.
            LightingStyle::Auto => self.switch_to(LightingStyle::Chill),
        }

        self.resolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hardstyle_grade() -> FixtureGenreProfile {
        FixtureGenreProfile {
            aggression: 0.9,
            density: 0.85,
            movement: 0.8,
            color_saturation: 0.9,
            strobe_tolerance: 0.85,
            laser_tolerance: 0.8,
        }
    }

    #[test]
    fn style_posture_table() {
        // License bands: exact clamp arithmetic per posture.
        assert_eq!(LightingStyle::Auto.clamp_license(0.93), 0.93);
        assert_eq!(LightingStyle::Chill.clamp_license(0.93), 0.15);
        assert_eq!(LightingStyle::Chill.clamp_license(0.05), 0.05);
        assert_eq!(LightingStyle::Energetic.clamp_license(0.93), 0.65);
        assert_eq!(LightingStyle::Energetic.clamp_license(0.4), 0.4);
        assert_eq!(LightingStyle::Banger.clamp_license(0.1), 0.85);
        assert_eq!(LightingStyle::Banger.clamp_license(0.95), 0.95);

        // Vocabulary matrix.
        assert!(LightingStyle::Auto.allows_macro());
        assert!(!LightingStyle::Chill.allows_macro());
        assert!(LightingStyle::Energetic.allows_macro());
        assert!(LightingStyle::Banger.allows_macro());

        assert!(LightingStyle::Auto.allows_blinder_hold());
        assert!(!LightingStyle::Chill.allows_blinder_hold());
        assert!(!LightingStyle::Energetic.allows_blinder_hold());
        assert!(LightingStyle::Banger.allows_blinder_hold());

        assert!(LightingStyle::Auto.allows_kick_strobe(false));
        assert!(!LightingStyle::Chill.allows_kick_strobe(true));
        assert!(!LightingStyle::Energetic.allows_kick_strobe(false));
        assert!(LightingStyle::Energetic.allows_kick_strobe(true));
        assert!(LightingStyle::Banger.allows_kick_strobe(false));

        // Chill zeroes the hard-gate fields below the fixture-brain
        // thresholds even on an aggressive genre; Energetic only tames
        // movement; Banger does not re-open anything.
        let chill = LightingStyle::Chill.compose_genre(hardstyle_grade());
        assert_eq!(chill.strobe_tolerance, 0.0);
        assert_eq!(chill.laser_tolerance, 0.0);
        assert!(chill.density < 0.35);
        assert_eq!(chill.aggression, 0.9);

        let energetic = LightingStyle::Energetic.compose_genre(hardstyle_grade());
        assert!((energetic.movement - 0.8 * ENERGETIC_MOVEMENT_SCALE).abs() < 1e-6);
        assert_eq!(energetic.strobe_tolerance, 0.85);
        assert_eq!(energetic.laser_tolerance, 0.8);

        assert_eq!(LightingStyle::Banger.compose_genre(hardstyle_grade()), hardstyle_grade());

        // Segment tension cap: only Chill measures the pixel look.
        assert_eq!(LightingStyle::Chill.segment_tension_cap(), 0.42);
        assert_eq!(LightingStyle::Banger.segment_tension_cap(), 1.0);

        // MIDI cycle order and the persistence round-trip with the Auto
        // fallback for unknown strings.
        assert_eq!(LightingStyle::Auto.next(), LightingStyle::Chill);
        assert_eq!(LightingStyle::Chill.next(), LightingStyle::Energetic);
        assert_eq!(LightingStyle::Energetic.next(), LightingStyle::Banger);
        assert_eq!(LightingStyle::Banger.next(), LightingStyle::Auto);
        for style in LightingStyle::ALL {
            assert_eq!(LightingStyle::from_str(style.as_str()), style);
        }
        assert_eq!(LightingStyle::from_str("garbage"), LightingStyle::Auto);
        assert_eq!(LightingStyle::from_str(""), LightingStyle::Auto);
    }

    /// Drive the resolver at 60fps for `secs` with a constant license.
    fn run_resolver(r: &mut AutoStyleResolver, secs: f32, license: f32, peak: bool) {
        let frames = (secs * 60.0) as u32;
        for _ in 0..frames {
            r.update(1.0 / 60.0, license, peak);
        }
    }

    #[test]
    fn auto_resolver_measured_flips() {
        // The posture drive blends absolute loudness with the per-track
        // surge. A track sitting at the neutral license level reads Chill; a
        // sustained lift well above neutral earns Energetic; a settle back
        // returns to Chill.
        let mut r = AutoStyleResolver::new();
        assert_eq!(r.resolved(), LightingStyle::Chill, "chill is the default");

        // Steady at the neutral license level → drive ~ 0 → Chill.
        run_resolver(&mut r, 30.0, ABS_NEUTRAL, false);
        assert_eq!(r.resolved(), LightingStyle::Chill, "neutral-loudness material reads chill");

        // A sustained lift well above neutral earns Energetic…
        run_resolver(&mut r, 8.0, 0.82, false);
        assert_eq!(r.resolved(), LightingStyle::Energetic, "a clear lift earns Energetic");

        // …and once the music settles back to neutral, the room walks back
        // down to Chill over the demote window.
        run_resolver(&mut r, 25.0, ABS_NEUTRAL, false);
        assert_eq!(r.resolved(), LightingStyle::Chill, "the room settles back to chill");
    }

    #[test]
    fn auto_resolver_absolute_level_sets_the_baseline() {
        // The blend fix (field feedback 2026-06-11: pure-z over-chilled
        // consistently-loud tracks — the bangers never banged): absolute
        // loudness lifts the baseline posture even when the level is FLAT.
        // A quiet flat track is Chill; a loud flat track runs warm and its
        // peak sections detonate.
        let mut quiet = AutoStyleResolver::new();
        run_resolver(&mut quiet, 60.0, 0.20, false);
        assert_eq!(quiet.resolved(), LightingStyle::Chill, "a quiet flat track is chill");

        // "Loud" is defined relative to the current ABS_NEUTRAL so the test
        // tracks live tuning: a level a clear margin above neutral must run
        // warm regardless of the exact neutral/gain knobs.
        let loud_level = (ABS_NEUTRAL + 0.35).min(1.0);
        let mut loud = AutoStyleResolver::new();
        run_resolver(&mut loud, 60.0, loud_level, false);
        assert_ne!(
            loud.resolved(),
            LightingStyle::Chill,
            "a loud flat track runs warm (absolute level lifts the baseline)"
        );
        // …and a peak-arc section of that loud track detonates to Banger
        // (the bangers bang) — the property pure-z could not deliver.
        run_resolver(&mut loud, 5.0, (ABS_NEUTRAL + 0.4).min(1.0), true);
        assert_eq!(loud.resolved(), LightingStyle::Banger, "a loud track's drops detonate");
    }

    #[test]
    fn auto_resolver_drop_fast_path() {
        // A DYNAMIC drop — loud relative to a quiet baseline — promotes
        // IMMEDIATELY to Banger (no evidence window, no dwell).
        let mut r = AutoStyleResolver::new();
        run_resolver(&mut r, 20.0, 0.1, false);
        assert_eq!(r.resolved(), LightingStyle::Chill);

        r.update(1.0 / 60.0, 0.7, true); // loud + big surge over the 0.1 norm
        assert_eq!(r.resolved(), LightingStyle::Banger, "a dynamic drop slams to full vocabulary");

        // While the drop stays hot, it HOLDS Banger open (no demotion)…
        run_resolver(&mut r, 8.0, 0.7, true);
        assert_eq!(r.resolved(), LightingStyle::Banger, "a sustained drop holds the vocabulary");
        // …and once it passes (arc drops out, level settles quiet), it walks
        // down off Banger.
        run_resolver(&mut r, 18.0, 0.2, false);
        assert_ne!(r.resolved(), LightingStyle::Banger, "the moment passing releases Banger");

        // A "drop" arc on quiet material (below the license floor) does NOT
        // fast-promote — ambient false peaks stay measured.
        let mut r = AutoStyleResolver::new();
        run_resolver(&mut r, 20.0, 0.1, false);
        r.update(1.0 / 60.0, 0.3, true); // below BANGER_ABS_MIN
        assert_eq!(r.resolved(), LightingStyle::Chill, "quiet material ignores the fast path");
    }

    #[test]
    fn auto_clamp_skips_banger_floor() {
        // Resolved Banger leaves the license free (within-song dial intact);
        // resolved Chill/Energetic clamp exactly like the pinned postures.
        assert_eq!(LightingStyle::Banger.auto_clamp_license(0.3), 0.3);
        assert_eq!(LightingStyle::Banger.clamp_license(0.3), 0.85);
        assert_eq!(LightingStyle::Chill.auto_clamp_license(0.93), 0.15);
        assert_eq!(LightingStyle::Energetic.auto_clamp_license(0.93), 0.65);
    }

    #[test]
    fn auto_style_is_identity() {
        // The Auto guarantee: no clamp, no genre rewrite — the adaptive
        // engine output is byte-identical to a build without styles.
        for license in [0.0, 0.15, 0.4, 0.65, 0.85, 1.0] {
            assert_eq!(LightingStyle::Auto.clamp_license(license), license);
        }
        assert_eq!(LightingStyle::Auto.compose_genre(hardstyle_grade()), hardstyle_grade());
        assert_eq!(LightingStyle::Auto.segment_tension_cap(), 1.0);
    }
}

//! Chromatic all-notes colour centroid.
//!
//! The dominant-note colour path collapses the 12-bin chroma to its single
//! loudest pitch class (argmax + latch in `key_signal.rs`) and maps it
//! `hue = pc/12` around the colour wheel. This module is the "obey ALL the
//! notes" alternative the Hero's-Journey / Quaternion wheel already shows
//! (it lights every present note): the energy-weighted vector mean of the 12
//! pitch classes on the SAME chromatic wheel (`hue = i/12`), so the rig's
//! colour is the blend of what's actually sounding, not just the loudest.
//!
//! Distinct from `tonnetz.rs`, which embeds chroma on the circle of FIFTHS
//! (related keys adjacent) for smooth harmonic-motion drift — a different
//! hue space. This one stays in the chromatic `pc/12` space the fixture
//! colour seams already use, so it is a drop-in substitution for the
//! dominant-note hue.
//!
//! A spread chord's pitch vectors partially cancel → low concentration →
//! the caller desaturates the colour (many notes → whiter); a single strong
//! note → high concentration → vivid. The angle of a weak/atonal centroid is
//! unstable, so the caller crossfades to the (latched, stable) dominant hue
//! when total tonal energy is low.

use std::f32::consts::TAU;

/// Energy-weighted centroid of the 12 chroma bins on the chromatic colour
/// wheel. Returns `(x, y, total)` — the unnormalised vector sum (`x`,`y`)
/// and the summed weight (`total`). The caller EMA-smooths `(x, y)` across
/// frames (correct circular smoothing) and derives hue/concentration from
/// the smoothed vector; `total` gates the atonal fallback.
pub fn chroma_centroid(chroma: &[f32; 12]) -> (f32, f32, f32) {
    let mut x = 0.0f32;
    let mut y = 0.0f32;
    let mut total = 0.0f32;
    for (i, &raw) in chroma.iter().enumerate() {
        let w = raw.max(0.0);
        let angle = (i as f32) / 12.0 * TAU;
        x += w * angle.cos();
        y += w * angle.sin();
        total += w;
    }
    (x, y, total)
}

/// Bias a centroid toward the (latched, stable) dominant pitch class so the
/// blend RESPECTS the dominant note instead of drifting to an in-between hue
/// that ignores the clearly-loudest one. The dominant gets an extra weight
/// proportional to the total chroma energy (`emphasis` = its share, e.g.
/// 0.6 ≈ "the dominant counts for 60% of the present energy again"), which
/// keeps the bias scale-stable and pulls the colour toward the dominant
/// while the other present notes still shift it. Returns the augmented
/// `(x, y, total)`.
pub fn emphasize_dominant(
    x: f32,
    y: f32,
    total: f32,
    dominant_pc: u8,
    emphasis: f32,
) -> (f32, f32, f32) {
    if total <= 1e-6 || emphasis <= 0.0 {
        return (x, y, total);
    }
    let w = emphasis * total;
    let angle = (dominant_pc.min(11) as f32) / 12.0 * TAU;
    (x + w * angle.cos(), y + w * angle.sin(), total + w)
}

/// Derive the chromatic hue (0–1) of a centroid vector.
pub fn centroid_hue(x: f32, y: f32) -> f32 {
    (y.atan2(x) / TAU).rem_euclid(1.0)
}

/// Concentration of a centroid: `|vector| / total`, 0–1. ~1 when one note
/// dominates (vivid), ~0 when energy is spread across many notes (wash to
/// white). Returns 0 when there is no tonal energy.
pub fn centroid_concentration(x: f32, y: f32, total: f32) -> f32 {
    if total > 1e-6 {
        (x.hypot(y) / total).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_note(pc: usize, level: f32) -> [f32; 12] {
        let mut c = [0.0f32; 12];
        c[pc] = level;
        c
    }

    #[test]
    fn single_note_is_its_own_hue_fully_concentrated() {
        // A lone pitch class sits at its chromatic hue with concentration 1.
        for pc in 0..12usize {
            let (x, y, total) = chroma_centroid(&one_note(pc, 0.7));
            let hue = centroid_hue(x, y);
            assert!(
                (hue - pc as f32 / 12.0).abs() < 1e-4,
                "pc {pc} hue {hue}"
            );
            assert!(
                (centroid_concentration(x, y, total) - 1.0).abs() < 1e-4,
                "pc {pc} should be fully concentrated"
            );
        }
    }

    #[test]
    fn spread_chroma_desaturates_and_silence_is_zero() {
        // A flat (all-notes-equal) chroma cancels → ~0 concentration (the
        // caller washes it toward white) and the angle is meaningless.
        let flat = [0.5f32; 12];
        let (x, y, total) = chroma_centroid(&flat);
        assert!(centroid_concentration(x, y, total) < 0.05, "flat chroma must wash out");

        // Silence → zero total → zero concentration (caller falls back).
        let (x, y, total) = chroma_centroid(&[0.0; 12]);
        assert_eq!(centroid_concentration(x, y, total), 0.0);
    }

    #[test]
    fn dominant_emphasis_pulls_the_blend_toward_the_dominant() {
        // C + F# (a tritone) nearly cancel → an ambiguous centroid. Emphasising
        // the dominant (say C, pc 0) pulls the blended hue toward C's hue (0.0)
        // and restores concentration, while F# still shifts it off pure C.
        let mut c = [0.0f32; 12];
        c[0] = 0.5; // C
        c[6] = 0.5; // F#
        let (x, y, total) = chroma_centroid(&c);
        let (ex, ey, et) = emphasize_dominant(x, y, total, 0, 1.0);
        let hue = centroid_hue(ex, ey);
        // C is hue 0.0; emphasis pulls the blend near it (within an eighth-wheel).
        let dist = hue.min(1.0 - hue);
        assert!(dist < 0.125, "emphasis should pull toward C's hue, got {hue}");
        assert!(
            centroid_concentration(ex, ey, et) > centroid_concentration(x, y, total),
            "emphasis restores concentration on an ambiguous chord"
        );
    }

    #[test]
    fn a_triad_lands_between_its_notes() {
        // C + E + G: the blended hue sits between the notes (not on any one),
        // with partial concentration — the "obey all notes" blend.
        let mut c = [0.0f32; 12];
        c[0] = 0.6; // C
        c[4] = 0.6; // E
        c[7] = 0.6; // G
        let (x, y, total) = chroma_centroid(&c);
        let conc = centroid_concentration(x, y, total);
        assert!(conc > 0.0 && conc < 1.0, "a triad is partially concentrated, got {conc}");
    }
}

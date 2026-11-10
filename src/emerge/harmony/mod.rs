//! EMeRGE harmony engine — chroma → key / chord / tonal-centroid signals.
//!
//! Pure harmonic analysis with no transport concern: the fallback key signal,
//! genre tuning profiles, the Tonnetz centroid + voice-leading "surprise"
//! tracker, and lightweight triad / neo-Riemannian chord detection. Moved
//! verbatim out of `osc/` — it was transport-named only by history.

mod chord;
pub mod chroma_color;
pub mod genre;
mod key_signal;
mod tonnetz;

pub use chord::{ChordFrame, ChordTracker};
pub use chroma_color::{centroid_concentration, centroid_hue, chroma_centroid, emphasize_dominant};
pub use genre::{GenreProfile, PaletteBias, StrobeZoneMinimum, ZoneThresholds};
pub use key_signal::{KeySignal, KeySignalState};
pub use tonnetz::{TonnetzFrame, TonnetzTracker};

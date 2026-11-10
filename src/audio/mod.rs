//! Audio capture and analysis
//!
//! Supports capturing from system audio, specific apps (Apple Music, etc.), or microphone.
//!
//! ## Modules
//! - `capture`: System audio capture using ScreenCaptureKit/cpal
//! - `analyzer`: FFT-based frequency analysis with kick detection
//! - `meters`: fundsp-based real-time metering with band splitting
//! - `output`: Audio output for scrub playback

mod capture;
mod gst_capture;
mod analyzer;
mod meters;
mod output;
pub mod audio_bus;
pub mod normalizer;

pub use capture::{AudioCapture, AudioSource, AudioSourceInfo, CaptureBackend, AudioDiagnostics, get_available_sources, get_running_applications, get_device_profile};
pub use analyzer::{AudioAnalyzer, AudioAnalysis};
pub use meters::{AudioMeters, MeterValues};
pub use output::AudioOutput;
pub use audio_bus::{AudioBusProducers, AudioBusConsumers, AudioBusStats, create_audio_bus, CHUNK_FRAMES, QUEUE_FRAMES, CANON_RATE_HZ};
pub use normalizer::{ControlSignalNormalizer, NormalizedSignals};

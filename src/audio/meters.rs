//! fundsp-based Audio Meters with Band Splitting
//!
//! Provides real-time audio metering using fundsp for DSP:
//! - Band splitting (bass/mid/high) using SVF filters
//! - Envelope followers for each band
//! - RMS, peak hold, and decay meter values
//! - Dedicated audio thread for consistent timing
//! - Lock-free channel to UI at ~60Hz
//!
//! Architecture:
//! ```
//! AudioCapture (rtrb) → AudioMeters thread → MeterValues → UI (crossbeam channel)
//! ```

use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError};
use fundsp::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Sample rate for DSP processing
const SAMPLE_RATE: f32 = 48000.0;

/// UI update rate (Hz) - send meter values at this rate
const UI_UPDATE_RATE: f32 = 60.0;

/// Samples per UI update (sample-based timing, NOT wall clock)
const SAMPLES_PER_UI_UPDATE: usize = (SAMPLE_RATE / UI_UPDATE_RATE) as usize; // 800 samples @ 48kHz

/// Band crossover frequencies
const BASS_HIGH_FREQ: f32 = 250.0;   // Bass → Mid crossover
const MID_HIGH_FREQ: f32 = 4000.0;   // Mid → High crossover

/// Filter Q (resonance) - 0.707 = Butterworth (maximally flat)
const FILTER_Q: f32 = 0.707;

/// Envelope follower attack/release times (seconds)
const ENV_ATTACK_S: f32 = 0.003;     // 3ms attack (very fast)
const ENV_RELEASE_S: f32 = 0.080;    // 80ms release

/// Peak hold time (seconds)
const PEAK_HOLD_S: f32 = 1.5;

/// Peak decay rate (per second)
const PEAK_DECAY_RATE: f32 = 0.5;

/// DSP boost factor - amplify meter values before sending to UI
/// Audio envelope values are typically 0.01-0.1 for normal levels,
/// this brings them into a more visible 0.1-1.0 range
const DSP_BOOST: f32 = 8.0;

/// Meter values sent to UI
#[derive(Clone, Copy, Debug, Default)]
pub struct MeterValues {
    /// Bass band level (0-1, envelope-followed)
    pub bass: f32,
    /// Mid band level (0-1, envelope-followed)
    pub mid: f32,
    /// High band level (0-1, envelope-followed)
    pub high: f32,

    /// Overall RMS level (0-1)
    pub rms: f32,

    /// Peak level with hold (0-1)
    pub peak: f32,
    /// Current instantaneous peak (0-1)
    pub peak_instant: f32,

    /// Bass peak with hold
    pub bass_peak: f32,
    /// Mid peak with hold
    pub mid_peak: f32,
    /// High peak with hold
    pub high_peak: f32,

    /// Kick detection confidence (0-1)
    pub kick_confidence: f32,
    /// True if kick detected this frame
    pub kick: bool,
}

/// Peak hold meter with decay
struct PeakMeter {
    peak: f32,
    held_peak: f32,
    hold_samples_remaining: usize,
    hold_samples: usize,
    decay_rate: f32,
}

impl PeakMeter {
    fn new(sample_rate: f32, hold_time_s: f32, decay_rate: f32) -> Self {
        Self {
            peak: 0.0,
            held_peak: 0.0,
            hold_samples_remaining: 0,
            hold_samples: (hold_time_s * sample_rate) as usize,
            decay_rate: decay_rate / sample_rate,
        }
    }

    fn process(&mut self, sample: f32) -> (f32, f32) {
        let abs = sample.abs();

        // Update instantaneous peak
        self.peak = self.peak.max(abs);

        // Update held peak
        if abs >= self.held_peak {
            self.held_peak = abs;
            self.hold_samples_remaining = self.hold_samples;
        } else if self.hold_samples_remaining > 0 {
            self.hold_samples_remaining -= 1;
        } else {
            // Decay
            self.held_peak = (self.held_peak - self.decay_rate).max(0.0);
        }

        (self.peak, self.held_peak)
    }

    fn reset_instant(&mut self) {
        self.peak = 0.0;
    }
}

/// RMS meter with windowed averaging
struct RmsMeter {
    sum_squares: f32,
    sample_count: usize,
}

impl RmsMeter {
    fn new() -> Self {
        Self {
            sum_squares: 0.0,
            sample_count: 0,
        }
    }

    fn process(&mut self, sample: f32) {
        self.sum_squares += sample * sample;
        self.sample_count += 1;
    }

    fn get_rms(&self) -> f32 {
        if self.sample_count == 0 {
            return 0.0;
        }
        (self.sum_squares / self.sample_count as f32).sqrt()
    }

    fn reset(&mut self) {
        self.sum_squares = 0.0;
        self.sample_count = 0;
    }
}

/// Envelope follower using exponential smoothing
struct EnvelopeFollower {
    level: f32,
    attack_coeff: f32,
    release_coeff: f32,
}

impl EnvelopeFollower {
    fn new(sample_rate: f32, attack_s: f32, release_s: f32) -> Self {
        // Convert time constants to filter coefficients
        let attack_coeff = 1.0 - (-1.0 / (attack_s * sample_rate)).exp();
        let release_coeff = 1.0 - (-1.0 / (release_s * sample_rate)).exp();

        Self {
            level: 0.0,
            attack_coeff,
            release_coeff,
        }
    }

    fn process(&mut self, sample: f32) -> f32 {
        let abs = sample.abs();

        if abs > self.level {
            self.level += self.attack_coeff * (abs - self.level);
        } else {
            self.level += self.release_coeff * (abs - self.level);
        }

        self.level
    }
}

/// Kick detector using fast vs slow envelope comparison
struct KickDetector {
    /// Fast envelope (follows transients quickly)
    fast_env: f32,
    /// Slow envelope (baseline level)
    slow_env: f32,
    fast_attack: f32,
    fast_release: f32,
    slow_attack: f32,
    slow_release: f32,
    cooldown: usize,
    cooldown_samples: usize,
}

impl KickDetector {
    fn new(sample_rate: f32) -> Self {
        // Fast envelope: 1ms attack, 30ms release - catches transients
        let fast_attack = 1.0 - (-1.0f32 / (0.001 * sample_rate)).exp();
        let fast_release = 1.0 - (-1.0f32 / (0.030 * sample_rate)).exp();

        // Slow envelope: 50ms attack, 200ms release - baseline level
        let slow_attack = 1.0 - (-1.0f32 / (0.050 * sample_rate)).exp();
        let slow_release = 1.0 - (-1.0f32 / (0.200 * sample_rate)).exp();

        Self {
            fast_env: 0.0,
            slow_env: 0.0,
            fast_attack,
            fast_release,
            slow_attack,
            slow_release,
            cooldown: 0,
            // Minimum time between kicks (~200 BPM max)
            cooldown_samples: (sample_rate * 0.075) as usize,
        }
    }

    fn process(&mut self, bass_sample: f32) -> (f32, bool) {
        let abs = bass_sample.abs();

        // Fast envelope (catches transients)
        if abs > self.fast_env {
            self.fast_env += self.fast_attack * (abs - self.fast_env);
        } else {
            self.fast_env += self.fast_release * (abs - self.fast_env);
        }

        // Slow envelope (baseline)
        if abs > self.slow_env {
            self.slow_env += self.slow_attack * (abs - self.slow_env);
        } else {
            self.slow_env += self.slow_release * (abs - self.slow_env);
        }

        // Confidence based on fast envelope level (less boosted than meters)
        let confidence = (self.fast_env * DSP_BOOST * 0.5).min(1.0);

        // Kick = fast envelope significantly exceeds slow envelope
        // This detects transients regardless of overall level
        let ratio = if self.slow_env > 0.00001 {
            self.fast_env / self.slow_env
        } else if self.fast_env > 0.00001 {
            10.0  // If slow is zero but fast has signal, it's a kick
        } else {
            1.0
        };

        let kick = if self.cooldown == 0 && ratio > 2.0 && self.fast_env > 0.0001 {
            self.cooldown = self.cooldown_samples;
            true
        } else {
            if self.cooldown > 0 {
                self.cooldown -= 1;
            }
            false
        };

        (confidence, kick)
    }
}

/// Audio meters processor using fundsp
pub struct AudioMeters {
    /// Receiver for meter values (poll from UI)
    receiver: Receiver<MeterValues>,
    /// Running flag
    running: Arc<AtomicBool>,
    /// Processing thread handle
    thread: Option<JoinHandle<()>>,
}

impl AudioMeters {
    /// Create new audio meters
    /// Takes ownership of a frame-based rtrb consumer from AudioCapture
    pub fn new(audio_consumer: rtrb::Consumer<[f32; 2]>) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        // Create bounded channel for meter values
        // Small buffer (4 frames) - UI only needs latest value
        let (sender, receiver) = bounded::<MeterValues>(4);

        let thread = thread::spawn(move || {
            Self::meter_loop(audio_consumer, sender, running_clone);
        });

        Self {
            receiver,
            running,
            thread: Some(thread),
        }
    }

    /// Get latest meter values (non-blocking)
    /// Returns the most recent values, discarding any older ones
    pub fn get_meters(&self) -> Option<MeterValues> {
        let mut latest = None;

        // Drain channel to get latest value
        loop {
            match self.receiver.try_recv() {
                Ok(values) => latest = Some(values),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        latest
    }

    /// Main metering loop running on dedicated audio thread
    ///
    /// IMPORTANT: All DSP runs here on sample-based timing.
    /// UI updates are sent every SAMPLES_PER_UI_UPDATE samples (~60Hz).
    /// Wall clock time is NEVER used for meter math - only sample counts.
    fn meter_loop(
        mut consumer: rtrb::Consumer<[f32; 2]>,
        sender: Sender<MeterValues>,
        running: Arc<AtomicBool>,
    ) {
        log::info!("[AudioMeters] Starting meter thread (sample-based timing, {} samples/update)",
            SAMPLES_PER_UI_UPDATE);

        // Create fundsp band-split filters using SVF (State Variable Filter)
        let mut bass_filter = lowpass_hz::<f32>(BASS_HIGH_FREQ, FILTER_Q);
        let mut mid_hp_filter = highpass_hz::<f32>(BASS_HIGH_FREQ, FILTER_Q);
        let mut mid_lp_filter = lowpass_hz::<f32>(MID_HIGH_FREQ, FILTER_Q);
        let mut high_filter = highpass_hz::<f32>(MID_HIGH_FREQ, FILTER_Q);

        // Set sample rate for all filters
        bass_filter.set_sample_rate(SAMPLE_RATE as f64);
        mid_hp_filter.set_sample_rate(SAMPLE_RATE as f64);
        mid_lp_filter.set_sample_rate(SAMPLE_RATE as f64);
        high_filter.set_sample_rate(SAMPLE_RATE as f64);

        bass_filter.reset();
        mid_hp_filter.reset();
        mid_lp_filter.reset();
        high_filter.reset();

        // Envelope followers for each band
        let mut bass_env = EnvelopeFollower::new(SAMPLE_RATE, ENV_ATTACK_S, ENV_RELEASE_S);
        let mut mid_env = EnvelopeFollower::new(SAMPLE_RATE, ENV_ATTACK_S, ENV_RELEASE_S);
        let mut high_env = EnvelopeFollower::new(SAMPLE_RATE, ENV_ATTACK_S, ENV_RELEASE_S);

        // Peak meters with hold
        let mut overall_peak = PeakMeter::new(SAMPLE_RATE, PEAK_HOLD_S, PEAK_DECAY_RATE);
        let mut bass_peak_meter = PeakMeter::new(SAMPLE_RATE, PEAK_HOLD_S, PEAK_DECAY_RATE);
        let mut mid_peak_meter = PeakMeter::new(SAMPLE_RATE, PEAK_HOLD_S, PEAK_DECAY_RATE);
        let mut high_peak_meter = PeakMeter::new(SAMPLE_RATE, PEAK_HOLD_S, PEAK_DECAY_RATE);

        // RMS meter
        let mut rms_meter = RmsMeter::new();

        // Kick detector
        let mut kick_detector = KickDetector::new(SAMPLE_RATE);

        // Current envelope-followed levels (updated every sample)
        let mut current_bass = 0.0f32;
        let mut current_mid = 0.0f32;
        let mut current_high = 0.0f32;
        let mut kick_this_frame = false;
        let mut kick_confidence = 0.0f32;

        // SAMPLE-BASED TIMING: count samples, not wall clock
        let mut samples_since_update = 0usize;
        let mut total_samples = 0u64;
        let mut update_count = 0u64;

        // Small buffer for draining frames from ring buffer
        let mut frame_buf: Vec<[f32; 2]> = Vec::with_capacity(2048);

        while running.load(Ordering::Relaxed) {
            // Drain available frames from ring buffer (non-blocking)
            frame_buf.clear();
            while let Ok(frame) = consumer.pop() {
                frame_buf.push(frame);
                if frame_buf.len() >= 1024 {
                    break; // Process in manageable chunks
                }
            }

            if frame_buf.is_empty() {
                // No audio available - brief sleep to avoid spinning
                // This doesn't affect meter math since we count samples
                thread::sleep(Duration::from_micros(200));
                continue;
            }

            // Process audio frames (stereo → mono)
            // DSP runs on EVERY frame - timing is sample-accurate
            for &frame in &frame_buf {
                // Mix stereo to mono
                let mono = (frame[0] + frame[1]) * 0.5;

                // Band splitting using fundsp SVF filters
                let input_frame = [mono].into();
                let bass_out = bass_filter.tick(&input_frame)[0];
                let mid_hp_out = mid_hp_filter.tick(&input_frame)[0];
                let mid_out = mid_lp_filter.tick(&[mid_hp_out].into())[0];
                let high_out = high_filter.tick(&input_frame)[0];

                // Envelope followers (smooth the rectified signal)
                current_bass = bass_env.process(bass_out);
                current_mid = mid_env.process(mid_out);
                current_high = high_env.process(high_out);

                // Peak meters (track instantaneous and held peaks)
                overall_peak.process(mono);
                bass_peak_meter.process(bass_out);
                mid_peak_meter.process(mid_out);
                high_peak_meter.process(high_out);

                // RMS accumulator
                rms_meter.process(mono);

                // Kick detection
                let (conf, kick) = kick_detector.process(bass_out);
                kick_confidence = conf;
                if kick {
                    kick_this_frame = true;
                }

                // Count samples for timing
                samples_since_update += 1;
                total_samples += 1;
            }

            // SAMPLE-BASED UI UPDATE: every N samples, send meter values
            if samples_since_update >= SAMPLES_PER_UI_UPDATE {
                update_count += 1;

                // Apply DSP boost - audio envelope values are small, need amplification
                // UI sensitivity slider provides additional user control
                let values = MeterValues {
                    bass: current_bass * DSP_BOOST,
                    mid: current_mid * DSP_BOOST,
                    high: current_high * DSP_BOOST,
                    rms: rms_meter.get_rms() * DSP_BOOST,
                    peak: overall_peak.held_peak * DSP_BOOST,
                    peak_instant: overall_peak.peak * DSP_BOOST,
                    bass_peak: bass_peak_meter.held_peak * DSP_BOOST,
                    mid_peak: mid_peak_meter.held_peak * DSP_BOOST,
                    high_peak: high_peak_meter.held_peak * DSP_BOOST,
                    kick_confidence,
                    kick: kick_this_frame,
                };

                // Non-blocking send to UI (drops oldest if channel full)
                let _ = sender.try_send(values);

                // Reset per-frame state
                overall_peak.reset_instant();
                bass_peak_meter.reset_instant();
                mid_peak_meter.reset_instant();
                high_peak_meter.reset_instant();
                rms_meter.reset();
                kick_this_frame = false;
                samples_since_update = 0;

                // Periodic logging
                if update_count % 300 == 1 {
                    log::info!("[AudioMeters] Update {}: bass={:.2} mid={:.2} high={:.2} rms={:.2} ({}k samples)",
                        update_count, values.bass, values.mid, values.high, values.rms, total_samples / 1000);
                }
            }
        }

        log::info!("[AudioMeters] Meter thread stopped after {} updates, {} samples",
            update_count, total_samples);
    }

    /// Stop the meter thread
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for AudioMeters {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_envelope_follower() {
        let mut env = EnvelopeFollower::new(48000.0, 0.005, 0.100);

        // Attack should be fast
        for _ in 0..240 { // 5ms at 48kHz
            env.process(1.0);
        }
        assert!(env.level > 0.5, "Envelope should rise during attack");

        // Release should be slower
        let level_after_attack = env.level;
        for _ in 0..4800 { // 100ms at 48kHz
            env.process(0.0);
        }
        assert!(env.level < level_after_attack * 0.5, "Envelope should decay during release");
    }

    #[test]
    fn test_peak_meter() {
        let mut peak = PeakMeter::new(48000.0, 1.0, 0.7);

        // Process a peak
        peak.process(0.8);
        assert_eq!(peak.peak, 0.8);
        assert_eq!(peak.held_peak, 0.8);

        // Peak should hold
        for _ in 0..24000 { // 0.5s - should still be holding
            peak.process(0.1);
        }
        assert_eq!(peak.held_peak, 0.8, "Peak should hold");

        // After hold time, should decay
        for _ in 0..48000 { // 1s more
            peak.process(0.1);
        }
        assert!(peak.held_peak < 0.8, "Peak should decay after hold");
    }

    #[test]
    fn test_rms_meter() {
        let mut rms = RmsMeter::new();

        // Process constant amplitude signal
        for _ in 0..1000 {
            rms.process(0.5);
        }

        let rms_val = rms.get_rms();
        assert!((rms_val - 0.5).abs() < 0.01, "RMS of constant signal should equal amplitude");

        rms.reset();
        assert_eq!(rms.get_rms(), 0.0, "RMS should be zero after reset");
    }
}

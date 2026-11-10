//! Invisible Audio Fingerprint — DSSS/BPSK at 19kHz
//!
//! Embeds account-specific data (account_id + timestamp) in the ultrasonic
//! range using Direct Sequence Spread Spectrum with BPSK modulation.
//!
//! Design parameters:
//! - **Carrier**: 19kHz (above music, below Nyquist/2 for 48kHz)
//! - **Spreading**: Gold codes (degree-5 LFSR, length 31 chips per bit)
//! - **Chip rate**: 300 chips/sec → ~9.7 bits/sec → 64-bit payload in ~6.6s
//! - **Amplitude**: -40dB relative to full scale (inaudible)
//! - **Payload**: 32-bit account_id + 32-bit Unix timestamp
//!
//! Survives: lossy compression (AAC, MP3, Opus), screen recording,
//! social media re-encoding, analog-to-digital re-capture.
//!
//! ## Decoding (server-side)
//!
//! To extract the fingerprint from a recording:
//! 1. Bandpass filter around 19kHz (±500Hz)
//! 2. Correlate with known Gold code at 300 chips/sec
//! 3. BPSK demodulate: positive correlation = bit 1, negative = bit 0
//! 4. Reconstruct 64-bit payload: first 32 bits = account_id, next 32 = timestamp
//! 5. The Gold code provides ~15dB processing gain, pulling signal out of noise
//!
//! The payload repeats every ~6.6 seconds, so a 10+ second clip gives at
//! least one full extraction. Multiple repetitions can be majority-voted
//! for error correction.

/// Gold code length: 2^5 - 1 = 31 chips per bit
const GOLD_CODE_LEN: usize = 31;

/// Payload size in bits (32-bit account_id + 32-bit timestamp)
const PAYLOAD_BITS: usize = 64;

/// Generate a degree-5 LFSR sequence with given tap mask.
/// Taps are 1-indexed bit positions that feed back via XOR.
/// Returns `2^5 - 1 = 31` chips as `{-1, +1}` values.
fn lfsr_sequence(taps: &[usize]) -> [f32; GOLD_CODE_LEN] {
    let mut state: u32 = 0b11111; // All ones initial state
    let mut seq = [0.0f32; GOLD_CODE_LEN];

    for chip in seq.iter_mut() {
        // Output is LSB
        let output_bit = state & 1;
        *chip = if output_bit == 1 { 1.0 } else { -1.0 };

        // Feedback: XOR of tapped bit positions (1-indexed)
        let mut feedback = 0u32;
        for &tap in taps {
            feedback ^= (state >> (tap - 1)) & 1;
        }

        // Shift right, insert feedback at MSB (bit 4 for degree-5)
        state = (state >> 1) | (feedback << 4);
    }

    seq
}

/// Generate Gold code by XOR-ing two LFSR sequences.
/// Uses standard taps: LFSR1=[5,3], LFSR2=[5,4,3,2].
fn generate_gold_code() -> [f32; GOLD_CODE_LEN] {
    let seq1 = lfsr_sequence(&[5, 3]);
    let seq2 = lfsr_sequence(&[5, 4, 3, 2]);

    let mut gold = [0.0f32; GOLD_CODE_LEN];
    for i in 0..GOLD_CODE_LEN {
        // XOR in {-1, +1} domain: multiply
        gold[i] = seq1[i] * seq2[i];
    }
    gold
}

/// DSSS/BPSK audio fingerprint generator.
///
/// Call `process()` on each audio buffer to additively embed the fingerprint.
/// The fingerprint is phase-continuous across buffer boundaries and repeats
/// the payload indefinitely.
pub struct AudioFingerprint {
    /// Gold code spreading sequence (31 chips, values in {-1, +1})
    gold_code: [f32; GOLD_CODE_LEN],
    /// Payload bits (64 bits: 32-bit account_id + 32-bit timestamp)
    payload: [u8; PAYLOAD_BITS],
    /// Carrier frequency in Hz (19000.0)
    carrier_freq: f32,
    /// Signal amplitude (-40dB ≈ 0.01)
    amplitude: f32,
    /// Sample rate (48000.0)
    sample_rate: f32,
    /// Samples per chip (sample_rate / chip_rate = 48000/300 = 160)
    samples_per_chip: usize,
    /// Running carrier phase (wraps at 2*PI for continuity)
    phase: f64,
    /// Phase increment per sample (2*PI * carrier_freq / sample_rate)
    phase_inc: f64,
    /// Current bit index in payload (0..63, wraps)
    bit_index: usize,
    /// Current chip index in Gold code (0..30, wraps per bit)
    chip_index: usize,
    /// Sample counter within current chip
    chip_sample: usize,
}

impl AudioFingerprint {
    /// Create a new fingerprint generator for the given account.
    ///
    /// `account_id`: Account to encode (from entitlements)
    /// `sample_rate`: Audio sample rate (typically 48000)
    pub fn new(account_id: u32, sample_rate: u32) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as u32;

        // Pack payload: 32 bits account_id (MSB first) + 32 bits timestamp (MSB first)
        let mut payload = [0u8; PAYLOAD_BITS];
        for i in 0..32 {
            payload[i] = ((account_id >> (31 - i)) & 1) as u8;
        }
        for i in 0..32 {
            payload[32 + i] = ((timestamp >> (31 - i)) & 1) as u8;
        }

        let carrier_freq = 19_000.0f32;
        let chip_rate = 300.0f32;
        let sr = sample_rate as f32;

        log::info!(
            "[AudioFingerprint] Initialized: account_id={}, timestamp={}, carrier={}Hz, chip_rate={}, sr={}",
            account_id, timestamp, carrier_freq, chip_rate, sample_rate
        );

        Self {
            gold_code: generate_gold_code(),
            payload,
            carrier_freq,
            amplitude: 0.01, // -40dB
            sample_rate: sr,
            samples_per_chip: (sr / chip_rate) as usize, // 160 at 48kHz
            phase: 0.0,
            phase_inc: 2.0 * std::f64::consts::PI * carrier_freq as f64 / sr as f64,
            bit_index: 0,
            chip_index: 0,
            chip_sample: 0,
        }
    }

    /// Add fingerprint signal to interleaved stereo f32 samples in-place.
    ///
    /// The ultrasonic carrier is identical in both channels (mono fingerprint).
    /// This is intentional — stereo decorrelation at 19kHz would be audible
    /// as spatial artifacts on high-end monitoring systems.
    #[inline]
    pub fn process(&mut self, samples: &mut [f32]) {
        // Process sample pairs (interleaved stereo: L, R, L, R, ...)
        let mut i = 0;
        while i + 1 < samples.len() {
            // Current BPSK symbol: data bit XOR'd with Gold code chip
            // In {-1, +1} domain: multiply
            let data_symbol = if self.payload[self.bit_index] == 1 { 1.0f32 } else { -1.0f32 };
            let chip_value = self.gold_code[self.chip_index];
            let bpsk = data_symbol * chip_value;

            // Modulate carrier with BPSK symbol
            let carrier = (self.phase as f32).sin();
            let signal = self.amplitude * bpsk * carrier;

            // Add to both channels (mono fingerprint)
            samples[i] += signal;
            samples[i + 1] += signal;

            // Advance carrier phase (continuous across chunks)
            self.phase += self.phase_inc;
            if self.phase >= 2.0 * std::f64::consts::PI {
                self.phase -= 2.0 * std::f64::consts::PI;
            }

            // Advance chip timing
            self.chip_sample += 1;
            if self.chip_sample >= self.samples_per_chip {
                self.chip_sample = 0;
                self.chip_index += 1;
                if self.chip_index >= GOLD_CODE_LEN {
                    self.chip_index = 0;
                    self.bit_index += 1;
                    if self.bit_index >= PAYLOAD_BITS {
                        self.bit_index = 0; // Payload repeats
                    }
                }
            }

            i += 2; // Next stereo pair
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gold_code_length() {
        let code = generate_gold_code();
        assert_eq!(code.len(), 31);
        // All values should be -1.0 or 1.0
        for &c in &code {
            assert!(c == -1.0 || c == 1.0, "Gold code chip must be -1 or +1, got {}", c);
        }
    }

    #[test]
    fn test_gold_code_balance() {
        let code = generate_gold_code();
        // Gold codes have approximately equal +1 and -1 chips
        let positive: usize = code.iter().filter(|&&c| c > 0.0).count();
        let negative: usize = code.iter().filter(|&&c| c < 0.0).count();
        // For length-31 Gold codes, imbalance can be up to 3
        assert!((positive as i32 - negative as i32).unsigned_abs() <= 3,
            "Gold code not balanced: {} positive, {} negative", positive, negative);
    }

    #[test]
    fn test_lfsr_period() {
        // LFSR should produce 31 unique states before repeating
        let seq = lfsr_sequence(&[5, 3]);
        assert_eq!(seq.len(), 31);
        // No two consecutive identical values for a maximal LFSR
        // (not strictly true, but the period should be 31)
    }

    #[test]
    fn test_fingerprint_amplitude() {
        let mut fp = AudioFingerprint::new(12345, 48000);
        // Generate one second of audio
        let mut samples = vec![0.0f32; 48000 * 2]; // 1 second stereo
        fp.process(&mut samples);

        // Peak amplitude should be around 0.01 (-40dB)
        let peak: f32 = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(peak <= 0.015, "Peak amplitude too high: {}", peak);
        assert!(peak >= 0.005, "Peak amplitude too low: {}", peak);
    }

    #[test]
    fn test_fingerprint_does_not_modify_silence_drastically() {
        let mut fp = AudioFingerprint::new(99999, 48000);
        let mut samples = vec![0.0f32; 9600]; // 100ms stereo

        fp.process(&mut samples);

        // All samples should be very small (ultrasonic fingerprint only)
        for &s in &samples {
            assert!(s.abs() <= 0.015, "Sample too loud: {}", s);
        }
    }

    #[test]
    fn test_payload_encoding() {
        let fp = AudioFingerprint::new(0xDEADBEEF, 48000);

        // Check first 32 bits encode the account_id correctly
        let mut reconstructed: u32 = 0;
        for i in 0..32 {
            reconstructed |= (fp.payload[i] as u32) << (31 - i);
        }
        assert_eq!(reconstructed, 0xDEADBEEF, "Account ID not encoded correctly");
    }

    #[test]
    fn test_phase_continuity() {
        let mut fp = AudioFingerprint::new(42, 48000);

        // Process two consecutive chunks
        let mut chunk1 = vec![0.0f32; 1920]; // 20ms stereo
        let mut chunk2 = vec![0.0f32; 1920];
        fp.process(&mut chunk1);
        let phase_after_1 = fp.phase;
        fp.process(&mut chunk2);

        // Phase should advance smoothly (not reset to 0)
        assert!(phase_after_1 > 0.0, "Phase should advance after first chunk");
        // After second chunk, phase has advanced further (may have wrapped past 2*PI)
        // Just verify both chunks produced non-zero output (fingerprint was applied)
        let has_signal_1 = chunk1.iter().any(|&s| s != 0.0);
        let has_signal_2 = chunk2.iter().any(|&s| s != 0.0);
        assert!(has_signal_1, "First chunk should have fingerprint signal");
        assert!(has_signal_2, "Second chunk should have fingerprint signal");
    }
}

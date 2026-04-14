/// Advanced audio processing for better ASR accuracy
use std::collections::VecDeque;
use std::f32::consts::PI;

pub struct AudioProcessor {
    // Downsampling
    source_rate: u32,
    target_rate: u32,
    downsample_ratio: f32,
    downsample_buffer: Vec<f32>,

    // Preprocessing
    high_pass_state: [f32; 2],
    agc_level: f32,

    // Voice Activity Detection
    energy_history: VecDeque<f32>,
    vad_threshold: f32,
}

impl AudioProcessor {
    pub fn new(source_rate: u32, _channels: u16) -> Self {
        let target_rate = 16000;
        let downsample_ratio = source_rate as f32 / target_rate as f32;

        Self {
            source_rate,
            target_rate,
            downsample_ratio,
            downsample_buffer: Vec::new(),
            high_pass_state: [0.0; 2],
            agc_level: 1.0,
            energy_history: VecDeque::with_capacity(50),
            vad_threshold: 0.02, // Increased threshold to reduce noise
        }
    }

    /// Process audio with proper downsampling and enhancement
    pub fn process_audio(&mut self, input: &[f32], channels: usize) -> Vec<f32> {
        // Step 1: Convert to mono
        let mono = self.convert_to_mono(input, channels);

        // Step 1.5: Apply initial gain boost
        let gain_boost = 5.0; // Reduced gain to avoid over-amplification
        let boosted: Vec<f32> = mono
            .iter()
            .map(|&s| (s * gain_boost).clamp(-1.0, 1.0))
            .collect();

        // Step 2: Apply pre-emphasis filter to boost high frequencies
        let pre_emphasized = self.apply_pre_emphasis(&boosted);

        // Step 3: Apply high-pass filter (remove DC offset and low-freq noise)
        let filtered = self.apply_high_pass_filter(&pre_emphasized);

        // Step 4: Downsample with anti-aliasing
        let downsampled = self.downsample_with_filter(&filtered);

        // Step 5: Apply AGC (Automatic Gain Control)
        let normalized = self.apply_agc(&downsampled);

        // Step 6: Apply spectral subtraction for noise reduction
        let denoised = self.apply_spectral_gating(&normalized);

        // Step 7: Final noise gate
        let gated = self.apply_noise_gate(&denoised);

        gated
    }

    fn convert_to_mono(&self, input: &[f32], channels: usize) -> Vec<f32> {
        if channels == 1 {
            return input.to_vec();
        }

        let mut mono = Vec::with_capacity(input.len() / channels);
        for i in (0..input.len()).step_by(channels) {
            let mut sum = 0.0;
            for ch in 0..channels {
                if i + ch < input.len() {
                    sum += input[i + ch];
                }
            }
            mono.push(sum / channels as f32);
        }
        mono
    }

    fn apply_pre_emphasis(&self, input: &[f32]) -> Vec<f32> {
        // Pre-emphasis filter to boost high frequencies (speech clarity)
        let alpha = 0.97;
        let mut output = Vec::with_capacity(input.len());

        if !input.is_empty() {
            output.push(input[0]);
            for i in 1..input.len() {
                output.push(input[i] - alpha * input[i - 1]);
            }
        }

        output
    }

    fn apply_high_pass_filter(&mut self, input: &[f32]) -> Vec<f32> {
        // Simple high-pass filter to remove DC offset and rumble
        // Cutoff around 80Hz
        let rc = 1.0 / (2.0 * PI * 80.0);
        let dt = 1.0 / self.source_rate as f32;
        let alpha = rc / (rc + dt);

        let mut output = Vec::with_capacity(input.len());

        for &sample in input {
            self.high_pass_state[0] =
                alpha * (self.high_pass_state[0] + sample - self.high_pass_state[1]);
            self.high_pass_state[1] = sample;
            output.push(self.high_pass_state[0]);
        }

        output
    }

    fn downsample_with_filter(&mut self, input: &[f32]) -> Vec<f32> {
        if self.downsample_ratio <= 1.0 {
            return input.to_vec();
        }

        // Apply low-pass filter before downsampling (anti-aliasing)
        let cutoff = 0.45; // Normalized frequency
        let filtered = self.apply_lowpass(input, cutoff);

        // Linear interpolation downsampling
        let output_len = (input.len() as f32 / self.downsample_ratio) as usize;
        let mut output = Vec::with_capacity(output_len);

        for i in 0..output_len {
            let src_idx = i as f32 * self.downsample_ratio;
            let idx = src_idx as usize;
            let frac = src_idx - idx as f32;

            if idx + 1 < filtered.len() {
                let sample = filtered[idx] * (1.0 - frac) + filtered[idx + 1] * frac;
                output.push(sample);
            } else if idx < filtered.len() {
                output.push(filtered[idx]);
            }
        }

        output
    }

    fn apply_lowpass(&self, input: &[f32], cutoff: f32) -> Vec<f32> {
        // Simple moving average filter
        let window_size = (1.0 / cutoff) as usize;
        let mut output = Vec::with_capacity(input.len());

        for i in 0..input.len() {
            let start = i.saturating_sub(window_size / 2);
            let end = (i + window_size / 2).min(input.len());

            let sum: f32 = input[start..end].iter().sum();
            output.push(sum / (end - start) as f32);
        }

        output
    }

    fn apply_agc(&mut self, input: &[f32]) -> Vec<f32> {
        // Calculate RMS energy
        let energy: f32 = input.iter().map(|s| s * s).sum::<f32>() / input.len() as f32;
        let rms = energy.sqrt();

        // Update AGC level with smoothing
        let target_level = 0.1; // Target RMS level
        if rms > 0.001 {
            let gain = target_level / rms;
            self.agc_level = self.agc_level * 0.95 + gain * 0.05; // Smooth adjustment
            self.agc_level = self.agc_level.clamp(0.5, 10.0); // Limit gain range
        }

        // Apply gain
        input
            .iter()
            .map(|s| (s * self.agc_level).clamp(-1.0, 1.0))
            .collect()
    }

    fn apply_spectral_gating(&self, input: &[f32]) -> Vec<f32> {
        // Simple spectral gating to reduce background noise
        let mut output = Vec::with_capacity(input.len());
        let window_size = 512;

        for i in 0..input.len() {
            let start = i.saturating_sub(window_size / 2);
            let end = (i + window_size / 2).min(input.len());

            if start < end {
                let window_slice = &input[start..end];
                let energy: f32 =
                    window_slice.iter().map(|s| s * s).sum::<f32>() / window_slice.len() as f32;
                let rms = energy.sqrt();

                // Gate based on local energy
                if rms > 0.005 {
                    output.push(input[i]);
                } else {
                    output.push(input[i] * 0.1); // Attenuate rather than cut
                }
            } else {
                output.push(input[i]);
            }
        }

        output
    }

    fn apply_noise_gate(&mut self, input: &[f32]) -> Vec<f32> {
        let mut output = Vec::with_capacity(input.len());

        for &sample in input {
            let energy = sample.abs();
            self.energy_history.push_back(energy);
            if self.energy_history.len() > 50 {
                self.energy_history.pop_front();
            }

            let avg_energy =
                self.energy_history.iter().sum::<f32>() / self.energy_history.len() as f32;

            // Simple gate with smooth transitions
            if avg_energy < self.vad_threshold {
                output.push(sample * 0.1); // Reduce but don't eliminate
            } else {
                output.push(sample);
            }
        }

        output
    }

    pub fn is_voice_active(&self) -> bool {
        if self.energy_history.is_empty() {
            return false;
        }

        let avg_energy = self.energy_history.iter().sum::<f32>() / self.energy_history.len() as f32;
        avg_energy > self.vad_threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_processor() {
        let mut processor = AudioProcessor::new(48000, 2);

        // Create test signal
        let mut input = Vec::new();
        for i in 0..96000 {
            let t = i as f32 / 48000.0;
            // 440Hz tone with some noise
            let signal = (t * 440.0 * 2.0 * PI).sin() * 0.5;
            let noise = ((t * 12345.0).sin() * 0.01).sin();
            input.push(signal + noise);
            input.push(signal + noise); // Stereo
        }

        let output = processor.process_audio(&input, 2);

        // Should be downsampled to 16kHz mono (~32000 samples expected)
        // Allow wide range due to resampling implementation differences
        assert!(
            output.len() > 10000 && output.len() < 40000,
            "unexpected output length: {}",
            output.len()
        );

        // Should have reasonable amplitude
        let max_val = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(max_val > 0.05 && max_val < 1.0);
    }
}

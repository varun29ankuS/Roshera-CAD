/// Audio processor using RNNoise neural noise suppression
/// Provides state-of-the-art noise reduction for speech
use nnnoiseless::DenoiseState;
use std::collections::VecDeque;

pub struct RNNoiseProcessor {
    // Audio parameters
    source_rate: u32,
    target_rate: u32,
    channels: u16,

    // Buffers for processing
    input_buffer: VecDeque<f32>,
    output_buffer: VecDeque<f32>,

    // Processing frame buffers (RNNoise requires 480 samples)
    input_frame: Vec<f32>,
    output_frame: Vec<f32>,

    // RNNoise constants
    frame_size: usize,

    // Last VAD probability
    last_vad_prob: f32,
}

impl RNNoiseProcessor {
    pub fn new(source_rate: u32, channels: u16) -> Self {
        // RNNoise frame size is 480 samples at 48kHz
        let frame_size = 480;

        Self {
            source_rate,
            target_rate: 16000, // Whisper expects 16kHz
            channels,
            input_buffer: VecDeque::with_capacity(source_rate as usize),
            output_buffer: VecDeque::with_capacity(16000),
            input_frame: vec![0.0; frame_size],
            output_frame: vec![0.0; frame_size],
            frame_size,
            last_vad_prob: 0.0,
        }
    }

    /// Process audio with RNNoise neural noise suppression
    pub fn process_audio(&mut self, input: &[f32]) -> Vec<f32> {
        // Step 1: Convert to mono if needed
        let mono = if self.channels > 1 {
            self.convert_to_mono(input)
        } else {
            input.to_vec()
        };

        // Step 2: Add to input buffer
        self.input_buffer.extend(mono.iter());

        // Step 3: Process complete frames with RNNoise
        // Create a new DenoiseState for each processing session
        let mut denoiser = DenoiseState::new();

        while self.input_buffer.len() >= self.frame_size {
            // Get one frame
            for i in 0..self.frame_size {
                self.input_frame[i] = self.input_buffer.pop_front().unwrap();
            }

            // Apply RNNoise - takes input and produces output
            let vad_prob = denoiser.process_frame(&mut self.output_frame, &self.input_frame);
            self.last_vad_prob = vad_prob;

            // Apply gain based on voice activity
            if vad_prob > 0.8 {
                // Strong voice activity - boost slightly
                for sample in &mut self.output_frame {
                    *sample *= 1.2;
                    *sample = sample.clamp(-1.0, 1.0);
                }
            } else if vad_prob < 0.3 {
                // Low voice activity - reduce volume
                for sample in &mut self.output_frame {
                    *sample *= 0.3;
                }
            }

            // Add to output buffer
            self.output_buffer.extend(&self.output_frame);
        }

        // Step 4: Get processed samples
        let processed: Vec<f32> = self.output_buffer.drain(..).collect();

        // Step 5: Resample to 16kHz if needed
        if self.source_rate != self.target_rate && !processed.is_empty() {
            self.resample_to_16khz(&processed)
        } else {
            processed
        }
    }

    fn convert_to_mono(&self, input: &[f32]) -> Vec<f32> {
        let channels = self.channels as usize;
        let mut mono = Vec::with_capacity(input.len() / channels);

        for chunk in input.chunks(channels) {
            let sum: f32 = chunk.iter().sum();
            mono.push(sum / channels as f32);
        }

        mono
    }

    fn resample_to_16khz(&self, input: &[f32]) -> Vec<f32> {
        // Simple downsampling - for 48kHz to 16kHz, take every 3rd sample
        let ratio = self.source_rate as f32 / self.target_rate as f32;

        if ratio == 3.0 {
            // Optimized path for 48kHz to 16kHz
            input.iter().step_by(3).copied().collect()
        } else {
            // General resampling with linear interpolation
            let output_len = (input.len() as f32 / ratio) as usize;
            let mut output = Vec::with_capacity(output_len);

            for i in 0..output_len {
                let src_idx = i as f32 * ratio;
                let idx = src_idx as usize;
                let frac = src_idx - idx as f32;

                if idx + 1 < input.len() {
                    let sample = input[idx] * (1.0 - frac) + input[idx + 1] * frac;
                    output.push(sample);
                } else if idx < input.len() {
                    output.push(input[idx]);
                }
            }

            output
        }
    }

    /// Get the probability of voice activity from the last processed frame
    pub fn get_voice_probability(&self) -> f32 {
        self.last_vad_prob
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rnnoise_processor() {
        let mut processor = RNNoiseProcessor::new(48000, 2);

        // Create test signal with noise
        let mut input = Vec::new();
        for i in 0..48000 {
            let t = i as f32 / 48000.0;
            // Speech-like signal
            let speech = (t * 200.0 * 2.0 * std::f32::consts::PI).sin() * 0.3;
            // Noise
            let noise = ((t * 8000.0).sin() * 0.1).sin();
            // Stereo
            input.push(speech + noise);
            input.push(speech + noise);
        }

        let output = processor.process_audio(&input);

        // Should be downsampled to 16kHz
        assert!(output.len() < input.len() / 2);

        println!(
            "Processed {} samples to {} samples",
            input.len(),
            output.len()
        );
        println!(
            "Last VAD probability: {:.2}",
            processor.get_voice_probability()
        );
    }
}

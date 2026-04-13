/// Simple but effective audio processor with aggressive noise reduction
/// Focused on making Whisper work in real environments
use std::collections::VecDeque;
use std::f32::consts::PI;

pub struct SimpleAudioProcessor {
    // Audio parameters
    source_rate: u32,
    target_rate: u32,

    // Processing state
    noise_floor: f32,
    speech_threshold: f32,

    // Buffers
    history: VecDeque<f32>,
}

impl SimpleAudioProcessor {
    pub fn new(source_rate: u32) -> Self {
        Self {
            source_rate,
            target_rate: 16000,
            noise_floor: 0.01,
            speech_threshold: 0.05,
            history: VecDeque::with_capacity(1000),
        }
    }

    pub fn process_audio(&mut self, input: &[f32], channels: usize) -> Vec<f32> {
        // Convert to mono
        let mono = if channels > 1 {
            let mut m = Vec::with_capacity(input.len() / channels);
            for chunk in input.chunks(channels) {
                let sum: f32 = chunk.iter().sum();
                m.push(sum / channels as f32);
            }
            m
        } else {
            input.to_vec()
        };

        // Update noise floor estimate
        let current_level = mono.iter().map(|s| s.abs()).sum::<f32>() / mono.len() as f32;
        self.noise_floor = self.noise_floor * 0.95 + current_level * 0.05;

        // Aggressive preprocessing
        let mut processed = Vec::with_capacity(mono.len());

        for &sample in &mono {
            // Gate out noise
            let gated = if sample.abs() < self.noise_floor * 2.0 {
                0.0
            } else {
                sample
            };

            // Apply compression and normalization
            let compressed = gated.signum() * gated.abs().powf(0.7);
            let normalized = compressed * 3.0; // Boost signal

            processed.push(normalized.clamp(-1.0, 1.0));
        }

        // Simple but effective noise reduction
        let mut cleaned = Vec::with_capacity(processed.len());
        self.history.clear();

        for &sample in &processed {
            self.history.push_back(sample);
            if self.history.len() > 5 {
                self.history.pop_front();
            }

            // Moving average filter
            let avg: f32 = self.history.iter().sum::<f32>() / self.history.len() as f32;
            cleaned.push(avg);
        }

        // Downsample to 16kHz
        if self.source_rate != self.target_rate {
            self.downsample(&cleaned)
        } else {
            cleaned
        }
    }

    fn downsample(&self, input: &[f32]) -> Vec<f32> {
        let ratio = self.source_rate as f32 / self.target_rate as f32;
        let output_len = (input.len() as f32 / ratio) as usize;
        let mut output = Vec::with_capacity(output_len);

        for i in 0..output_len {
            let src_idx = i as f32 * ratio;
            let idx = src_idx as usize;

            if idx < input.len() {
                output.push(input[idx]);
            }
        }

        output
    }

    pub fn is_speech(&self, samples: &[f32]) -> bool {
        let energy = samples.iter().map(|s| s.abs()).sum::<f32>() / samples.len() as f32;
        energy > self.speech_threshold
    }
}

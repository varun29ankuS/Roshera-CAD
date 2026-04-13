//! Scalar Quantization - Simple int8 quantization for memory reduction
//! 
//! Much simpler than PQ and often works better for high-dimensional vectors

use std::sync::Arc;

/// Scalar Quantizer - quantizes each dimension independently to int8
pub struct ScalarQuantizer {
    /// Minimum value for each dimension
    mins: Vec<f32>,
    /// Scale factor for each dimension (to map to int8 range)
    scales: Vec<f32>,
    /// Dimension
    dim: usize,
}

/// Quantized vector - one byte per dimension
#[derive(Clone, Debug)]
pub struct SQCode {
    /// Quantized values (-128 to 127)
    pub codes: Vec<i8>,
}

impl ScalarQuantizer {
    /// Create a new scalar quantizer
    pub fn new(dim: usize) -> Self {
        Self {
            mins: vec![0.0; dim],
            scales: vec![1.0; dim],
            dim,
        }
    }
    
    /// Train the quantizer on sample vectors
    pub fn train(&mut self, vectors: &[Vec<f32>]) {
        if vectors.is_empty() {
            return;
        }
        
        // Find min/max for each dimension
        let mut mins = vec![f32::MAX; self.dim];
        let mut maxs = vec![f32::MIN; self.dim];
        
        for vector in vectors {
            for (i, &val) in vector.iter().enumerate() {
                mins[i] = mins[i].min(val);
                maxs[i] = maxs[i].max(val);
            }
        }
        
        // Compute scales
        self.mins = mins;
        self.scales = Vec::with_capacity(self.dim);
        
        for i in 0..self.dim {
            let range = maxs[i] - self.mins[i];
            if range > 0.0 {
                // Map to int8 range (-128 to 127)
                self.scales.push(255.0 / range);
            } else {
                self.scales.push(1.0);
            }
        }
    }
    
    /// Encode a vector to int8
    pub fn encode(&self, vector: &[f32]) -> SQCode {
        let mut codes = Vec::with_capacity(self.dim);
        
        for i in 0..self.dim {
            let normalized = (vector[i] - self.mins[i]) * self.scales[i];
            let quantized = (normalized - 128.0).round() as i8;
            codes.push(quantized);
        }
        
        SQCode { codes }
    }
    
    /// Decode back to float (for debugging)
    pub fn decode(&self, code: &SQCode) -> Vec<f32> {
        let mut vector = Vec::with_capacity(self.dim);
        
        for i in 0..self.dim {
            let dequantized = (code.codes[i] as f32 + 128.0) / self.scales[i] + self.mins[i];
            vector.push(dequantized);
        }
        
        vector
    }
    
    /// Compute distance between query and quantized vector
    pub fn distance(&self, query: &[f32], code: &SQCode) -> f32 {
        let mut sum = 0.0f32;
        
        for i in 0..self.dim {
            // Dequantize and compute difference
            let dequantized = (code.codes[i] as f32 + 128.0) / self.scales[i] + self.mins[i];
            let diff = query[i] - dequantized;
            sum += diff * diff;
        }
        
        sum.sqrt()
    }
    
    /// Fast approximate distance using integer arithmetic
    pub fn distance_fast(&self, query: &[f32], code: &SQCode) -> f32 {
        // Pre-quantize query
        let mut query_quantized = Vec::with_capacity(self.dim);
        for i in 0..self.dim {
            let normalized = (query[i] - self.mins[i]) * self.scales[i];
            query_quantized.push((normalized - 128.0).round() as i8);
        }
        
        // Integer distance
        let mut sum = 0i32;
        for i in 0..self.dim {
            let diff = query_quantized[i] as i32 - code.codes[i] as i32;
            sum += diff * diff;
        }
        
        // Approximate scaling back
        (sum as f32).sqrt() / (self.scales.iter().sum::<f32>() / self.dim as f32)
    }
}

/// Optimized SQ for normalized vectors (like OpenAI embeddings)
pub struct SQ1536 {
    quantizer: ScalarQuantizer,
}

impl SQ1536 {
    pub fn new() -> Self {
        Self {
            quantizer: ScalarQuantizer::new(1536),
        }
    }
    
    pub fn train(&mut self, vectors: &[Vec<f32>]) {
        self.quantizer.train(vectors);
    }
    
    pub fn encode(&self, vector: &[f32]) -> SQCode {
        self.quantizer.encode(vector)
    }
    
    pub fn distance(&self, query: &[f32], code: &SQCode) -> f32 {
        self.quantizer.distance(query, code)
    }
    
    /// Memory reduction: 4x (float32 to int8)
    pub fn compression_ratio() -> f32 {
        4.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_scalar_quantization() {
        let mut sq = ScalarQuantizer::new(128);
        
        // Generate test vectors
        let vectors: Vec<Vec<f32>> = (0..100)
            .map(|_| {
                (0..128).map(|_| rand::random::<f32>() * 2.0 - 1.0).collect()
            })
            .collect();
        
        sq.train(&vectors);
        
        // Test encoding and decoding
        let original = &vectors[0];
        let encoded = sq.encode(original);
        let decoded = sq.decode(&encoded);
        
        // Check reconstruction error
        let error: f32 = original.iter()
            .zip(decoded.iter())
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f32>()
            .sqrt();
        
        println!("Reconstruction error: {}", error);
        assert!(error < 1.0); // Should have reasonable reconstruction
        
        // Test distance computation
        let dist = sq.distance(original, &encoded);
        println!("Distance to self after quantization: {}", dist);
        assert!(dist < 1.0); // Much better than PQ!
    }
}
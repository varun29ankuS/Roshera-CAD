//! SIMD-optimized vector operations
//!
//! Note: Due to project's no-unsafe-code policy, only safe portable implementations are used.
//! The performance is still optimized through loop unrolling and compiler optimizations.

/// Portable SIMD operations using safe code
pub struct PortableSimdOps;

impl PortableSimdOps {
    /// Portable dot product using loop unrolling
    pub fn dot_product_portable(a: &[f32], b: &[f32]) -> f32 {
        // Handle empty vectors
        if a.is_empty() || b.is_empty() {
            return 0.0;
        }
        
        assert_eq!(a.len(), b.len(), "Vector dimensions must match");
        let len = a.len();
        let mut sum = 0.0;
        
        // Process 8 elements at a time for better cache usage
        let chunks = len / 8;
        for i in 0..chunks {
            let offset = i * 8;
            // Unroll loop for better performance
            sum += a[offset] * b[offset];
            sum += a[offset + 1] * b[offset + 1];
            sum += a[offset + 2] * b[offset + 2];
            sum += a[offset + 3] * b[offset + 3];
            sum += a[offset + 4] * b[offset + 4];
            sum += a[offset + 5] * b[offset + 5];
            sum += a[offset + 6] * b[offset + 6];
            sum += a[offset + 7] * b[offset + 7];
        }
        
        // Handle remainder
        for i in (chunks * 8)..len {
            sum += a[i] * b[i];
        }
        
        sum
    }

    /// Portable cosine similarity
    pub fn cosine_similarity_portable(a: &[f32], b: &[f32]) -> f32 {
        let dot = Self::dot_product_portable(a, b);
        let norm_a = Self::dot_product_portable(a, a).sqrt();
        let norm_b = Self::dot_product_portable(b, b).sqrt();
        
        if norm_a * norm_b == 0.0 {
            0.0
        } else {
            dot / (norm_a * norm_b)
        }
    }

    /// Portable Euclidean distance
    pub fn euclidean_distance_portable(a: &[f32], b: &[f32]) -> f32 {
        // Handle empty vectors
        if a.is_empty() || b.is_empty() {
            return 0.0;
        }
        
        assert_eq!(a.len(), b.len(), "Vector dimensions must match");
        let len = a.len();
        let mut sum = 0.0;
        
        // Process 4 elements at a time
        let chunks = len / 4;
        for i in 0..chunks {
            let offset = i * 4;
            let diff0 = a[offset] - b[offset];
            let diff1 = a[offset + 1] - b[offset + 1];
            let diff2 = a[offset + 2] - b[offset + 2];
            let diff3 = a[offset + 3] - b[offset + 3];
            
            sum += diff0 * diff0;
            sum += diff1 * diff1;
            sum += diff2 * diff2;
            sum += diff3 * diff3;
        }
        
        // Handle remainder
        for i in (chunks * 4)..len {
            let diff = a[i] - b[i];
            sum += diff * diff;
        }
        
        sum.sqrt()
    }

    /// Batch cosine similarity
    pub fn batch_cosine_similarity_portable(
        query: &[f32],
        vectors: &[Vec<f32>],
    ) -> Vec<f32> {
        vectors
            .iter()
            .map(|v| Self::cosine_similarity_portable(query, v))
            .collect()
    }
}

/// Auto-selecting SIMD implementation
pub struct AutoSimd;

impl AutoSimd {
    /// Automatically select best SIMD implementation
    /// Note: Using safe portable implementation as unsafe code is forbidden
    pub fn dot_product(a: &[f32], b: &[f32]) -> f32 {
        // Always use safe portable implementation
        PortableSimdOps::dot_product_portable(a, b)
    }

    /// Automatically select best cosine similarity implementation
    /// Note: Using safe portable implementation as unsafe code is forbidden
    pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        // Always use safe portable implementation
        PortableSimdOps::cosine_similarity_portable(a, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dot_product() {
        let a = vec![1.0, 2.0, 3.0, 4.0];
        let b = vec![4.0, 3.0, 2.0, 1.0];
        
        let result = PortableSimdOps::dot_product_portable(&a, &b);
        assert_eq!(result, 20.0);
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        
        let result = PortableSimdOps::cosine_similarity_portable(&a, &b);
        assert!((result - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_euclidean_distance() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![3.0, 4.0, 0.0];
        
        let result = PortableSimdOps::euclidean_distance_portable(&a, &b);
        assert!((result - 5.0).abs() < 1e-6);
    }

    #[test]
    fn test_batch_cosine_similarity() {
        let query = vec![1.0, 0.0, 0.0];
        let vectors = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
        ];
        
        let results = PortableSimdOps::batch_cosine_similarity_portable(&query, &vectors);
        assert!((results[0] - 1.0).abs() < 1e-6);
        assert!((results[1] - 0.0).abs() < 1e-6);
        assert!((results[2] - 0.0).abs() < 1e-6);
    }
}
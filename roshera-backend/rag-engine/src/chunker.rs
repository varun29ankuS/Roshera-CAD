/// Intelligent Document Chunker for TurboRAG
/// 
/// Smart chunking that respects code structure and semantic boundaries

use regex::Regex;

/// Chunk of text with metadata
#[derive(Debug, Clone)]
pub struct Chunk {
    pub text: String,
    pub start: usize,
    pub end: usize,
    pub chunk_type: ChunkType,
}

/// Type of chunk
#[derive(Debug, Clone)]
pub enum ChunkType {
    Code,
    Documentation,
    Comment,
    Mixed,
}

/// Intelligent chunker
pub struct IntelligentChunker {
    pub target_chunk_size: usize,
    pub overlap_size: usize,
    pub respect_boundaries: bool,
}

impl IntelligentChunker {
    pub fn new() -> Self {
        Self {
            target_chunk_size: 512,  // Target ~512 tokens per chunk
            overlap_size: 50,        // 50 character overlap
            respect_boundaries: true, // Respect function/class boundaries
        }
    }
    
    /// Chunk text intelligently
    pub fn chunk_text(&self, text: &str) -> Vec<Chunk> {
        // Detect if this is code or prose
        if self.is_code(text) {
            self.chunk_code(text)
        } else {
            self.chunk_prose(text)
        }
    }
    
    /// Check if text is code
    fn is_code(&self, text: &str) -> bool {
        // Simple heuristics for code detection
        let code_indicators = [
            "fn ", "def ", "class ", "struct ", "impl ", "pub ", "private ", "public ",
            "import ", "use ", "include ", "require", "{", "}", "();", "->", "=>",
        ];
        
        let code_count = code_indicators.iter()
            .filter(|&indicator| text.contains(indicator))
            .count();
        
        code_count >= 3
    }
    
    /// Chunk code respecting function/class boundaries
    fn chunk_code(&self, text: &str) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        let lines: Vec<&str> = text.lines().collect();
        
        let mut current_chunk = String::new();
        let mut chunk_start = 0;
        let mut in_function = false;
        let mut brace_depth = 0;
        
        for (i, line) in lines.iter().enumerate() {
            // Track brace depth for function boundaries
            brace_depth += line.chars().filter(|&c| c == '{').count() as i32;
            brace_depth -= line.chars().filter(|&c| c == '}').count() as i32;
            
            // Detect function/method start
            if self.is_function_start(line) {
                // If we have content, save it as a chunk
                if !current_chunk.is_empty() && current_chunk.len() > 100 {
                    chunks.push(Chunk {
                        text: current_chunk.clone(),
                        start: chunk_start,
                        end: chunk_start + current_chunk.len(),
                        chunk_type: ChunkType::Code,
                    });
                    chunk_start += current_chunk.len();
                    current_chunk.clear();
                }
                in_function = true;
            }
            
            current_chunk.push_str(line);
            current_chunk.push('\n');
            
            // Check if we should create a chunk
            let should_chunk = if in_function && brace_depth == 0 {
                // End of function
                in_function = false;
                true
            } else if current_chunk.len() >= self.target_chunk_size {
                // Size limit reached
                true
            } else {
                false
            };
            
            if should_chunk {
                chunks.push(Chunk {
                    text: current_chunk.clone(),
                    start: chunk_start,
                    end: chunk_start + current_chunk.len(),
                    chunk_type: ChunkType::Code,
                });
                
                // Add overlap for context
                if self.overlap_size > 0 && i < lines.len() - 1 {
                    let overlap_lines = lines[i.saturating_sub(2)..=i].join("\n");
                    current_chunk = overlap_lines;
                } else {
                    current_chunk.clear();
                }
                
                chunk_start += current_chunk.len();
            }
        }
        
        // Add remaining content
        if !current_chunk.is_empty() && current_chunk.len() > 50 {
            chunks.push(Chunk {
                text: current_chunk,
                start: chunk_start,
                end: text.len(),
                chunk_type: ChunkType::Code,
            });
        }
        
        chunks
    }
    
    /// Chunk prose text
    fn chunk_prose(&self, text: &str) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        let paragraphs: Vec<&str> = text.split("\n\n").collect();
        
        let mut current_chunk = String::new();
        let mut chunk_start = 0;
        
        for para in paragraphs {
            // If adding this paragraph exceeds target size, create a chunk
            if !current_chunk.is_empty() 
                && current_chunk.len() + para.len() > self.target_chunk_size 
            {
                chunks.push(Chunk {
                    text: current_chunk.clone(),
                    start: chunk_start,
                    end: chunk_start + current_chunk.len(),
                    chunk_type: ChunkType::Documentation,
                });
                
                chunk_start += current_chunk.len();
                current_chunk.clear();
            }
            
            if !current_chunk.is_empty() {
                current_chunk.push_str("\n\n");
            }
            current_chunk.push_str(para);
            
            // If current chunk is large enough, save it
            if current_chunk.len() >= self.target_chunk_size {
                chunks.push(Chunk {
                    text: current_chunk.clone(),
                    start: chunk_start,
                    end: chunk_start + current_chunk.len(),
                    chunk_type: ChunkType::Documentation,
                });
                
                chunk_start += current_chunk.len();
                current_chunk.clear();
            }
        }
        
        // Add remaining content
        if !current_chunk.is_empty() {
            chunks.push(Chunk {
                text: current_chunk,
                start: chunk_start,
                end: text.len(),
                chunk_type: ChunkType::Documentation,
            });
        }
        
        // If no chunks were created, just split by size
        if chunks.is_empty() {
            self.chunk_by_size(text)
        } else {
            chunks
        }
    }
    
    /// Simple size-based chunking as fallback
    fn chunk_by_size(&self, text: &str) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        let mut start = 0;
        
        while start < text.len() {
            let end = (start + self.target_chunk_size).min(text.len());
            
            // Try to find a good break point (sentence or line end)
            let chunk_text = if end < text.len() {
                let mut break_point = end;
                
                // Look for sentence end
                if let Some(pos) = text[start..end].rfind(". ") {
                    break_point = start + pos + 1;
                } else if let Some(pos) = text[start..end].rfind('\n') {
                    break_point = start + pos + 1;
                }
                
                &text[start..break_point]
            } else {
                &text[start..end]
            };
            
            chunks.push(Chunk {
                text: chunk_text.to_string(),
                start,
                end: start + chunk_text.len(),
                chunk_type: ChunkType::Mixed,
            });
            
            // Move start with overlap
            start = if start + chunk_text.len() >= text.len() {
                text.len()
            } else {
                start + chunk_text.len() - self.overlap_size.min(chunk_text.len() / 2)
            };
        }
        
        chunks
    }
    
    /// Check if a line starts a function/method
    fn is_function_start(&self, line: &str) -> bool {
        let patterns = [
            r"^\s*(pub\s+)?fn\s+",        // Rust
            r"^\s*def\s+",                 // Python
            r"^\s*(public|private|protected)?\s*(static\s+)?.*\s+\w+\s*\(", // Java/C++
            r"^\s*function\s+",            // JavaScript
            r"^\s*func\s+",                // Go
        ];
        
        patterns.iter().any(|pattern| {
            Regex::new(pattern)
                .expect("static function-signature regex pattern must compile")
                .is_match(line)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_code_chunking() {
        let chunker = IntelligentChunker::new();
        
        let code = r#"
fn main() {
    println!("Hello, world!");
}

fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn multiply(a: i32, b: i32) -> i32 {
    a * b
}
"#;
        
        let chunks = chunker.chunk_text(code);
        assert!(chunks.len() > 0);
        
        // Each function should ideally be its own chunk
        for chunk in &chunks {
            println!("Chunk: {:?}", chunk.text);
        }
    }
    
    #[test]
    fn test_prose_chunking() {
        let chunker = IntelligentChunker::new();
        
        let prose = "This is the first paragraph. It contains some text that should be chunked appropriately.

This is the second paragraph. It also contains text that needs to be processed.

This is the third paragraph with more content.";
        
        let chunks = chunker.chunk_text(prose);
        assert!(chunks.len() > 0);
    }
}
# Contributing to Roshera CAD

## Welcome Contributors!

Thank you for your interest in contributing to Roshera CAD! This guide will help you understand our development philosophy, code standards, and contribution process.

## 🎯 Development Philosophy

### Fundamental Principle: Context-First Development

**STOP AND UNDERSTAND BEFORE ANY CHANGE**

Before writing or modifying ANY code, you must:

1. **UNDERSTAND THE CONTEXT**
   - Read relevant documentation (CLAUDE.md, README, module docs)
   - Understand system architecture and component interactions
   - Trace through existing code to understand data flow
   - Identify what problem this code solves

2. **UNDERSTAND THE INTENT**
   - Why does this feature/function exist?
   - What business or technical goal does it serve?
   - Who are the users (AI agents, developers, end users)?
   - What is the expected behavior and output?

3. **UNDERSTAND THE IMPACT**
   - What other code depends on this?
   - What could break if this changes?
   - What are the performance implications?
   - What are the edge cases and failure modes?

4. **DOCUMENT YOUR UNDERSTANDING**
   - Before making changes, explain your understanding
   - State the context, intent, and potential impact
   - Ask for clarification if anything is unclear

### Example of Proper Approach

```
"I need to fix the BRepModel import error.

Context: The compilation is failing because BRepModel is being imported from 
'builder' module but it actually exists in 'topology_builder' module. This 
affects 21 files in the operations module.

Intent: BRepModel is the core B-Rep topology container that operations need 
to access. The intent is to fix the import path so operations can properly 
modify topology.

Impact: This change will allow all operation modules to compile and access 
the B-Rep model. No functional changes, just fixing the import path.

Shall I proceed with this fix?"
```

## 🏗️ Architecture Principles

### 1. AI-Native Design
- **Universal Accessibility**: Every module and function must be callable by AI agent, human user, or script
- **Deterministic & Thread-Safe**: All data is Send + Sync with no race conditions
- **Strict Separation of Concerns**: Clear interfaces for AI, UI, and future integrations

### 2. Timeline Over Parametric Tree
- **Timeline-Based History**: Operations are recorded as independent events
- **No Dependency Graphs**: Avoids cascading failures of parametric trees
- **AI-Friendly**: Enables parallel AI exploration without conflicts
- **Simple Merging**: Git-like branch and merge operations

### 3. DashMap Everywhere
- **Concurrent by Default**: Use DashMap instead of HashMap for all collections
- **Lock-Free Performance**: Enables multi-user and multi-AI-agent scenarios
- **Timeline Architecture**: Essential for parallel branch management

### 4. Production-Grade Only
- **No TODOs or Placeholders**: Every function must be fully implemented
- **Comprehensive Error Handling**: All operations return Result<T, E>
- **Mathematical Rigor**: Algorithms must reference academic papers
- **Performance Benchmarked**: Must meet or exceed industry standards

## 💻 Code Standards

### Rust Code Quality

#### Required Documentation Format
```rust
/// Brief description of what this does
/// 
/// # Arguments
/// * `param` - What this parameter controls
/// 
/// # Returns
/// What this returns and when
/// 
/// # Errors
/// When this fails and why
/// 
/// # Example
/// ```
/// let result = module::function(param)?;
/// ```
/// 
/// # Performance
/// O(n) complexity, ~10ms for 1000 elements
#[inline]
pub fn every_public_function<T>(param: T) -> Result<Output, Error> 
where 
    T: Send + Sync + 'static  // Thread-safe by default
{
    // Implementation with comprehensive error handling
}
```

#### Error Handling Standards
```rust
// REQUIRED: All operations return Result<T, E>
pub fn create_sphere(radius: f64) -> Result<GeometryId, GeometryError> {
    if radius <= 0.0 {
        return Err(GeometryError::InvalidRadius { 
            value: radius,
            min: f64::EPSILON 
        });
    }
    // Implementation
}

// FORBIDDEN: Panics or unwrap() in production code
pub fn bad_function(data: Option<Data>) -> Data {
    data.unwrap() // ❌ NEVER DO THIS
}
```

#### Performance Requirements
```rust
// Required for all critical paths
#[inline(always)]
pub fn hot_path_function() -> f64 {
    // Implementation optimized for performance
}

// Required benchmarks for all public functions
#[cfg(test)]
mod benches {
    use super::*;
    use criterion::{black_box, criterion_group, criterion_main, Criterion};
    
    fn bench_function(c: &mut Criterion) {
        c.bench_function("function_name", |b| {
            b.iter(|| function_name(black_box(input)))
        });
    }
}
```

### Testing Standards

#### Required Test Coverage
```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    // Unit tests for all public functions
    #[test]
    fn test_basic_functionality() {
        let result = function(valid_input).unwrap();
        assert_eq!(result.expected_field, expected_value);
    }
    
    // Error case testing
    #[test]
    fn test_error_handling() {
        let error = function(invalid_input).unwrap_err();
        assert!(matches!(error, ExpectedErrorType { .. }));
    }
    
    // Property-based testing for mathematical invariants
    #[proptest]
    fn prop_mathematical_invariant(input: ValidInput) {
        let result = function(input).unwrap();
        prop_assert!(mathematical_property_holds(result));
    }
    
    // Performance tests
    #[test]
    fn test_performance_requirement() {
        let start = Instant::now();
        let _result = function(large_input).unwrap();
        assert!(start.elapsed() < Duration::from_millis(100));
    }
}
```

### Mathematical Implementation Standards

#### Algorithm Documentation
```rust
/// Evaluates a NURBS curve at parameter t using De Boor's algorithm.
/// 
/// Uses De Boor's algorithm for stable evaluation near knot values.
/// Reference: "The NURBS Book" by Piegl & Tiller (1997), Algorithm A2.2
/// 
/// # Performance
/// O(p²) where p is the degree. ~50ns for cubic curves.
/// 
/// # Mathematical Properties
/// - C^(p-m) continuous at knots with multiplicity m
/// - Affine invariant under transformations
/// - Convex hull property maintained
pub fn evaluate_nurbs(
    control_points: &[Point3],
    weights: &[f64],
    knots: &[f64],
    degree: usize,
    t: f64,
) -> Result<Point3, NurbsError> {
    // Implementation with comprehensive validation
}
```

#### Performance Targets
All implementations must meet these benchmarks:
- Vector operations: < 1ns (SIMD optimized)
- Matrix operations: < 10ns (cache-aligned)
- B-Spline evaluation: < 100ns (lookup tables)
- NURBS evaluation: < 200ns (GPU-ready)
- Boolean operations: 50-80% faster than industry leaders

## 🔧 Development Workflow

### Setup Your Environment

1. **Install Dependencies**
```bash
# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup component add clippy rustfmt

# Development tools
cargo install cargo-watch cargo-audit cargo-deny
```

2. **Clone and Build**
```bash
git clone https://github.com/your-org/roshera-cad
cd roshera-cad

# Build all modules
cargo build --workspace

# Run comprehensive tests
cargo test --workspace

# Run benchmarks
cargo bench --workspace
```

3. **Pre-commit Setup**
```bash
# Install pre-commit hooks
pre-commit install

# Run quality checks
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
cargo audit
```

### Making Changes

#### 1. Understand Before Acting
- Read module documentation thoroughly
- Trace through existing code patterns
- Understand architectural decisions
- Identify impact on other modules

#### 2. Create Feature Branch
```bash
git checkout -b feature/your-feature-name
```

#### 3. Implement Changes
- Follow architectural principles
- Maintain DashMap usage patterns
- Include comprehensive error handling
- Add documentation and tests

#### 4. Quality Assurance
```bash
# Format code
cargo fmt --all

# Check for issues
cargo clippy --workspace -- -D warnings

# Run tests
cargo test --workspace

# Run benchmarks
cargo bench --workspace

# Security audit
cargo audit
```

#### 5. Documentation Updates
- Update DEVELOPMENT_LOG.md with changes
- Add/update module documentation
- Include performance impact analysis
- Document any API changes

### Pull Request Process

#### 1. PR Description Template
```markdown
## Context and Intent

**Context**: Describe the current state and problem being solved
**Intent**: Explain the business/technical goal
**Impact**: Detail what changes and potential effects

## Changes Made

- [ ] Core implementation changes
- [ ] Test coverage added
- [ ] Documentation updated
- [ ] Performance benchmarked

## Checklist

- [ ] Follows context-first development principle
- [ ] Uses DashMap for all concurrent data structures
- [ ] Comprehensive error handling with Result<T, E>
- [ ] Documentation includes performance characteristics
- [ ] Tests cover edge cases and error conditions
- [ ] Benchmarks demonstrate performance requirements
- [ ] No TODOs or placeholder implementations
```

#### 2. Review Criteria

**Automatic Rejection Criteria**:
- Missing documentation for public functions
- Panics instead of proper error handling
- Non-deterministic behavior
- Blocking operations in async context
- Use of HashMap instead of DashMap

**Excellence Criteria**:
- Comprehensive documentation with examples
- Mathematical rigor with paper references
- Performance benchmarks included
- Property-based tests for invariants
- Clear architectural reasoning

## 📝 Documentation Standards

### Module Documentation
```rust
//! Brief module description
//!
//! This module provides [core functionality] for [business purpose].
//! 
//! # Architecture
//! Describe how this module fits in the overall system
//! 
//! # Performance
//! Document performance characteristics and benchmarks
//! 
//! # Examples
//! ```
//! use module::*;
//! let result = function()?;
//! ```
```

### API Documentation
- Every public function must have complete documentation
- Include performance characteristics (O notation, typical times)
- Provide working code examples
- Document error conditions and recovery
- Reference academic papers for algorithms

### Architecture Documentation
- Update CLAUDE.md for architectural changes
- Document design decisions and rationale
- Include performance impact analysis
- Explain integration with other modules

## 🧪 Testing Guidelines

### Test Categories Required

1. **Unit Tests**: Every public function
2. **Integration Tests**: Module interactions
3. **Property Tests**: Mathematical invariants
4. **Performance Tests**: Meet benchmark requirements
5. **Error Tests**: All error conditions
6. **Edge Case Tests**: Pathological inputs

### Evil Edge Case Testing

Based on our principle of testing beyond "happy path":

```rust
#[test]
fn test_degenerate_geometry() {
    // Test zero-area faces
    let result = create_face_with_collinear_points();
    assert!(result.is_ok()); // Should handle gracefully
}

#[test]
fn test_concurrent_mutations() {
    // Test thread safety under high concurrency
    let model = Arc::new(BRepModel::new());
    let handles: Vec<_> = (0..100).map(|i| {
        let model = model.clone();
        tokio::spawn(async move {
            model.add_vertex(i as f64, 0.0, 0.0).await
        })
    }).collect();
    
    // All operations should succeed
    for handle in handles {
        assert!(handle.await.unwrap().is_ok());
    }
}
```

## 🎯 AI Integration Guidelines

### AI-Callable Functions
```rust
// Good: AI can discover and use this function
pub fn create_gear(teeth: u32, diameter: f64) -> Result<GeometryId, GeometryError> {
    // Implementation
}

// Better: AI can introspect capabilities
impl Discoverable for GeometryEngine {
    fn schema() -> serde_json::Value {
        // Return JSON Schema for AI discovery
    }
    
    fn examples() -> Vec<Example> {
        // Return usage examples for AI learning
    }
}
```

### Natural Language Interface
```rust
// Commands must be parseable by AI
#[derive(Serialize, Deserialize)]
pub enum AICommand {
    #[serde(rename = "create_gear")]
    CreateGear {
        teeth: u32,
        diameter: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        thickness: Option<f64>,
    },
}
```

## 🔒 Security Guidelines

### Data Privacy
- Local processing by default
- No data leakage to external services
- Comprehensive audit logging
- Encryption for sensitive data

### Code Security
```rust
// Required: Input validation
pub fn process_input(input: &str) -> Result<ProcessedData, ValidationError> {
    if input.len() > MAX_INPUT_SIZE {
        return Err(ValidationError::InputTooLarge);
    }
    
    if !is_valid_format(input) {
        return Err(ValidationError::InvalidFormat);
    }
    
    // Process validated input
}

// Forbidden: Unsafe operations without justification
pub fn dangerous_operation() {
    unsafe {
        // Must have detailed comment explaining why unsafe is necessary
        // and what safety guarantees are maintained
    }
}
```

## 📊 Performance Requirements

### Benchmarking Standards
```rust
// All performance-critical functions need benchmarks
#[cfg(test)]
mod benchmarks {
    use super::*;
    use criterion::*;
    
    fn bench_industry_comparison(c: &mut Criterion) {
        c.bench_function("our_algorithm", |b| {
            b.iter(|| our_algorithm(black_box(&test_data)))
        });
        
        // Must be 50-80% faster than industry benchmarks
        assert!(measured_time < industry_benchmark * 0.5);
    }
}
```

### Memory Efficiency
- Use data-oriented design (SoA over AoS)
- Minimize allocations in hot paths
- Cache-friendly data structures
- SIMD-ready algorithms

### Concurrency Requirements
- All data structures must be Send + Sync
- Use DashMap for concurrent collections
- Lock-free algorithms where possible
- No blocking operations in async contexts

## 🚀 Getting Started

### Good First Issues

Look for issues tagged with:
- `good-first-issue`: Beginner-friendly tasks
- `documentation`: Improve docs and examples
- `testing`: Add test coverage
- `performance`: Optimize algorithms

### Development Areas

1. **Geometry Engine**: Mathematical algorithms and B-Rep operations
2. **AI Integration**: Natural language processing and provider systems
3. **Session Management**: Multi-user collaboration features
4. **Export Systems**: File format support and conversion
5. **Performance**: Optimization and benchmarking

### Learning Resources

- [CLAUDE.md](CLAUDE.md): Architecture and development principles
- [AI_INTEGRATION_OVERVIEW.md](roshera-backend/ai-integration/AI_INTEGRATION_OVERVIEW.md): AI system architecture
- [DEVELOPMENT_LOG.md](DEVELOPMENT_LOG.md): Development history and decisions
- Academic papers referenced in code comments

## 📞 Communication

### Getting Help

- **GitHub Issues**: Technical questions and bug reports
- **Discussions**: Architecture and design discussions
- **Discord**: Real-time development chat (link in issues)

### Code Review Process

1. All changes require review from core maintainers
2. Reviews focus on architecture, performance, and quality
3. Changes must pass all CI checks
4. Documentation and tests are mandatory

### Community Guidelines

- Be respectful and inclusive
- Focus on technical merit
- Provide constructive feedback
- Help newcomers understand our principles

## 🎉 Recognition

Contributors will be recognized in:
- Project README
- Release notes for significant contributions
- Annual contributor reports
- Speaking opportunities at conferences

## 📜 License

By contributing to Roshera CAD, you agree that your contributions will be licensed under the same terms as the project.

---

Thank you for contributing to the future of AI-native CAD! Together we're building something revolutionary. 🚀
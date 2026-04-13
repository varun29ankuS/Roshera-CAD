/// Hardcoded knowledge about Roshera CAD system
/// This is a pragmatic approach that works immediately without any external dependencies

pub struct RosheraKnowledge;

impl RosheraKnowledge {
    /// Get context for any command - this is injected into every LLM prompt
    pub fn get_context() -> &'static str {
        r#"
You are Roshera AI, operating a professional CAD system. Here's what you need to know:

## SYSTEM CAPABILITIES

### Primitives You Can Create:
- Box: create_box(width, height, depth) - for brackets, plates, housings
- Cylinder: create_cylinder(radius, height) - for shafts, pins, holes  
- Sphere: create_sphere(radius) - for ball joints, dome features
- Cone: create_cone(base_radius, height) - for tapered features
- Torus: create_torus(major_radius, minor_radius) - for O-rings, curved pipes

### Workflows (MUST follow in order):
1. Part Maturity: Sketch → Solid → Validate → Optimize → Finalize → Release
2. Requirements-Driven: Requirements → Initial → Iterate → Verify → Finalize
3. Assembly-Centric: Layout → Parts → Assemble → Validate → Finalize
4. Iterative Design: Concept → Prototype → Test → Refine → Finalize

### Engineering Rules:
- Bracket thickness: minimum 10mm for 10kg load
- Safety factor: always 2.5x for aerospace
- Default material: Aluminum 6061-T6
- Mounting holes: M8 standard, 20mm spacing minimum
- Fillet radius: 3mm minimum for stress relief

### Operations Available:
- Boolean: union, intersection, difference
- Transform: translate, rotate, scale
- Modify: fillet, chamfer, shell, offset
- Pattern: linear, circular, mirror
- Analysis: volume, surface_area, center_of_mass

## COMMAND UNDERSTANDING

When user says "create a bracket":
1. Activate Part Maturity workflow
2. Create box with appropriate dimensions
3. Add mounting holes using boolean difference
4. Apply fillets for stress relief
5. Validate thickness for load

When user says "I need to mount a 10kg sensor":
1. Calculate required bracket size (10kg → 10mm thick minimum)
2. Design mounting plate with 4 holes
3. Include cable management features
4. Suggest material and finish

## RESPONSE STYLE

Always respond with:
1. What you're doing (action)
2. Why you're doing it (engineering reason)
3. What comes next (workflow stage)

Example: "Creating a 100x80x10mm bracket. The 10mm thickness ensures it can handle your 10kg load with a 2.5x safety factor. Next, I'll add mounting holes in the Define stage."

## TECHNICAL DETAILS

Coordinate System: Right-handed, Z-up
Units: Millimeters (mm) default
Precision: 0.01mm for general, 0.001mm for precision features
File formats: STL (prototypes), STEP (production), ROS (native)
        "#
    }

    /// Get specific knowledge for different command types
    pub fn get_command_examples() -> Vec<(&'static str, &'static str)> {
        vec![
            ("create a bracket", "Command::CreateBox { width: 100.0, height: 80.0, depth: 10.0 }"),
            ("create a mounting plate", "Command::CreateBox { width: 150.0, height: 150.0, depth: 5.0 }"),
            ("make a shaft", "Command::CreateCylinder { radius: 10.0, height: 100.0 }"),
            ("add a hole", "Command::BooleanDifference { object_a: plate_id, object_b: cylinder_id }"),
            ("round the edges", "Command::Fillet { edges: selection, radius: 3.0 }"),
            ("make it hollow", "Command::Shell { faces: selection, thickness: 2.0 }"),
            ("create 4 holes", "Command::Pattern { feature: hole_id, type: Linear, count: 4, spacing: 20.0 }"),
        ]
    }

    /// Parse natural language to commands
    pub fn parse_intent(text: &str) -> Option<IntentHint> {
        let lower = text.to_lowercase();
        
        // Bracket-related
        if lower.contains("bracket") || lower.contains("mounting") {
            return Some(IntentHint::Bracket {
                load_kg: extract_number(&lower, "kg").unwrap_or(10.0),
                mount_points: if lower.contains("4") { 4 } else { 2 },
            });
        }
        
        // Shaft/cylinder
        if lower.contains("shaft") || lower.contains("pin") || lower.contains("rod") {
            return Some(IntentHint::Shaft {
                diameter: extract_number(&lower, "mm").unwrap_or(20.0),
                length: extract_number(&lower, "long").unwrap_or(100.0),
            });
        }
        
        // Plate
        if lower.contains("plate") || lower.contains("base") {
            return Some(IntentHint::Plate {
                thickness: extract_number(&lower, "thick").unwrap_or(5.0),
            });
        }
        
        // Housing/enclosure
        if lower.contains("housing") || lower.contains("enclosure") || lower.contains("case") {
            return Some(IntentHint::Housing {
                wall_thickness: 2.0,
                ip_rating: if lower.contains("waterproof") { "IP67" } else { "IP20" },
            });
        }
        
        None
    }
}

pub enum IntentHint {
    Bracket { load_kg: f64, mount_points: usize },
    Shaft { diameter: f64, length: f64 },
    Plate { thickness: f64 },
    Housing { wall_thickness: f64, ip_rating: &'static str },
}

fn extract_number(text: &str, before_word: &str) -> Option<f64> {
    // Simple number extraction - finds number before specific word
    // "10kg" -> 10.0, "5mm thick" -> 5.0
    if let Some(idx) = text.find(before_word) {
        let prefix = &text[0.max(idx.saturating_sub(10))..idx];
        let numbers: String = prefix.chars()
            .filter(|c| c.is_numeric() || *c == '.')
            .collect();
        numbers.parse().ok()
    } else {
        None
    }
}

/// Material properties for engineering calculations
pub struct Materials;

impl Materials {
    pub fn aluminum_6061() -> MaterialProps {
        MaterialProps {
            name: "Aluminum 6061-T6",
            density: 2700.0, // kg/m³
            yield_strength: 276.0, // MPa
            elastic_modulus: 68.9, // GPa
            poisson_ratio: 0.33,
        }
    }
    
    pub fn steel_304() -> MaterialProps {
        MaterialProps {
            name: "Stainless Steel 304",
            density: 8000.0,
            yield_strength: 215.0,
            elastic_modulus: 193.0,
            poisson_ratio: 0.29,
        }
    }
}

pub struct MaterialProps {
    pub name: &'static str,
    pub density: f64,
    pub yield_strength: f64,
    pub elastic_modulus: f64,
    pub poisson_ratio: f64,
}

impl MaterialProps {
    /// Calculate minimum thickness for a given load
    pub fn min_thickness_for_load(&self, load_kg: f64, safety_factor: f64) -> f64 {
        // Simplified calculation for demonstration
        // Real calculation would consider geometry, boundary conditions, etc.
        let stress = (load_kg * 9.81) / 1000.0; // Convert to N/mm²
        let allowable_stress = self.yield_strength / safety_factor;
        
        // Minimum thickness (simplified)
        (stress / allowable_stress * 10.0).max(3.0) // Minimum 3mm
    }
}
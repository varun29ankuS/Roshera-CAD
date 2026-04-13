//! Material definitions and presets
//!
//! Provides a comprehensive material system for CAD objects.

use crate::Color;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Complete material definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Material {
    /// Material name
    pub name: String,
    /// Display name
    pub display_name: String,
    /// Material category
    pub category: MaterialCategory,
    /// Physical properties
    pub physical: PhysicalProperties,
    /// Visual properties
    pub visual: VisualProperties,
    /// Thermal properties
    pub thermal: Option<ThermalProperties>,
    /// Mechanical properties
    pub mechanical: Option<MechanicalProperties>,
    /// Custom properties
    pub custom: HashMap<String, f64>,
}

/// Material categories
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MaterialCategory {
    /// Metals
    Metal,
    /// Plastics and polymers
    Plastic,
    /// Ceramics
    Ceramic,
    /// Composites
    Composite,
    /// Glass
    Glass,
    /// Wood
    Wood,
    /// Other materials
    Other,
}

/// Physical material properties
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhysicalProperties {
    /// Density in kg/m³
    pub density: f64,
    /// Melting point in Kelvin
    pub melting_point: Option<f64>,
    /// Boiling point in Kelvin
    pub boiling_point: Option<f64>,
    /// Electrical resistivity in Ω⋅m
    pub electrical_resistivity: Option<f64>,
}

/// Visual material properties
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualProperties {
    /// Base color
    pub base_color: Color,
    /// Metallic factor (0.0 = dielectric, 1.0 = metal)
    pub metallic: f32,
    /// Roughness factor (0.0 = smooth, 1.0 = rough)
    pub roughness: f32,
    /// Index of refraction
    pub ior: f32,
    /// Emission color
    pub emission: [f32; 3],
    /// Emission intensity
    pub emission_intensity: f32,
    /// Transparency
    pub transparency: f32,
    /// Subsurface scattering
    pub subsurface: f32,
    /// Subsurface color
    pub subsurface_color: [f32; 3],
}

/// Thermal properties
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermalProperties {
    /// Thermal conductivity in W/(m⋅K)
    pub conductivity: f64,
    /// Specific heat capacity in J/(kg⋅K)
    pub specific_heat: f64,
    /// Thermal expansion coefficient in 1/K
    pub expansion_coefficient: f64,
}

/// Mechanical properties
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MechanicalProperties {
    /// Young's modulus in GPa
    pub youngs_modulus: f64,
    /// Poisson's ratio
    pub poissons_ratio: f64,
    /// Yield strength in MPa
    pub yield_strength: f64,
    /// Ultimate tensile strength in MPa
    pub tensile_strength: f64,
    /// Hardness (Brinell)
    pub hardness: f64,
}

/// Material library
#[derive(Debug, Clone)]
pub struct MaterialLibrary {
    materials: HashMap<String, Material>,
}

impl Material {
    /// Create steel material
    pub fn steel() -> Self {
        Self {
            name: "steel".to_string(),
            display_name: "Steel (AISI 1045)".to_string(),
            category: MaterialCategory::Metal,
            physical: PhysicalProperties {
                density: 7850.0,
                melting_point: Some(1793.0),
                boiling_point: Some(3273.0),
                electrical_resistivity: Some(1.7e-7),
            },
            visual: VisualProperties {
                base_color: [0.6, 0.6, 0.7, 1.0],
                metallic: 1.0,
                roughness: 0.2,
                ior: 2.5,
                emission: [0.0, 0.0, 0.0],
                emission_intensity: 0.0,
                transparency: 0.0,
                subsurface: 0.0,
                subsurface_color: [0.0, 0.0, 0.0],
            },
            thermal: Some(ThermalProperties {
                conductivity: 49.8,
                specific_heat: 486.0,
                expansion_coefficient: 11.5e-6,
            }),
            mechanical: Some(MechanicalProperties {
                youngs_modulus: 200.0,
                poissons_ratio: 0.29,
                yield_strength: 305.0,
                tensile_strength: 585.0,
                hardness: 170.0,
            }),
            custom: HashMap::new(),
        }
    }

    /// Create aluminum material
    pub fn aluminum() -> Self {
        Self {
            name: "aluminum".to_string(),
            display_name: "Aluminum (6061-T6)".to_string(),
            category: MaterialCategory::Metal,
            physical: PhysicalProperties {
                density: 2700.0,
                melting_point: Some(933.0),
                boiling_point: Some(2743.0),
                electrical_resistivity: Some(2.65e-8),
            },
            visual: VisualProperties {
                base_color: [0.91, 0.92, 0.92, 1.0],
                metallic: 1.0,
                roughness: 0.1,
                ior: 1.5,
                emission: [0.0, 0.0, 0.0],
                emission_intensity: 0.0,
                transparency: 0.0,
                subsurface: 0.0,
                subsurface_color: [0.0, 0.0, 0.0],
            },
            thermal: Some(ThermalProperties {
                conductivity: 167.0,
                specific_heat: 896.0,
                expansion_coefficient: 23.6e-6,
            }),
            mechanical: Some(MechanicalProperties {
                youngs_modulus: 68.9,
                poissons_ratio: 0.33,
                yield_strength: 276.0,
                tensile_strength: 310.0,
                hardness: 95.0,
            }),
            custom: HashMap::new(),
        }
    }

    /// Create ABS plastic material
    pub fn abs_plastic() -> Self {
        Self {
            name: "abs".to_string(),
            display_name: "ABS Plastic".to_string(),
            category: MaterialCategory::Plastic,
            physical: PhysicalProperties {
                density: 1040.0,
                melting_point: Some(378.0),
                boiling_point: None,
                electrical_resistivity: Some(1e16),
            },
            visual: VisualProperties {
                base_color: [0.1, 0.1, 0.1, 1.0],
                metallic: 0.0,
                roughness: 0.5,
                ior: 1.5,
                emission: [0.0, 0.0, 0.0],
                emission_intensity: 0.0,
                transparency: 0.0,
                subsurface: 0.1,
                subsurface_color: [0.1, 0.1, 0.1],
            },
            thermal: Some(ThermalProperties {
                conductivity: 0.17,
                specific_heat: 1470.0,
                expansion_coefficient: 90e-6,
            }),
            mechanical: Some(MechanicalProperties {
                youngs_modulus: 2.3,
                poissons_ratio: 0.35,
                yield_strength: 45.0,
                tensile_strength: 40.0,
                hardness: 100.0,
            }),
            custom: HashMap::new(),
        }
    }

    /// Create glass material
    pub fn glass() -> Self {
        Self {
            name: "glass".to_string(),
            display_name: "Glass (Soda-lime)".to_string(),
            category: MaterialCategory::Glass,
            physical: PhysicalProperties {
                density: 2500.0,
                melting_point: Some(1473.0),
                boiling_point: None,
                electrical_resistivity: Some(1e12),
            },
            visual: VisualProperties {
                base_color: [1.0, 1.0, 1.0, 0.1],
                metallic: 0.0,
                roughness: 0.0,
                ior: 1.52,
                emission: [0.0, 0.0, 0.0],
                emission_intensity: 0.0,
                transparency: 0.9,
                subsurface: 0.0,
                subsurface_color: [0.0, 0.0, 0.0],
            },
            thermal: Some(ThermalProperties {
                conductivity: 1.0,
                specific_heat: 840.0,
                expansion_coefficient: 9e-6,
            }),
            mechanical: Some(MechanicalProperties {
                youngs_modulus: 70.0,
                poissons_ratio: 0.22,
                yield_strength: 0.0, // Brittle material
                tensile_strength: 50.0,
                hardness: 550.0,
            }),
            custom: HashMap::new(),
        }
    }
}

impl MaterialLibrary {
    /// Create new material library with defaults
    pub fn new() -> Self {
        let mut library = Self {
            materials: HashMap::new(),
        };

        // Add default materials
        library.add_material(Material::steel());
        library.add_material(Material::aluminum());
        library.add_material(Material::abs_plastic());
        library.add_material(Material::glass());

        library
    }

    /// Add material to library
    pub fn add_material(&mut self, material: Material) {
        self.materials.insert(material.name.clone(), material);
    }

    /// Get material by name
    pub fn get(&self, name: &str) -> Option<&Material> {
        self.materials.get(name)
    }

    /// Get all materials in category
    pub fn by_category(&self, category: MaterialCategory) -> Vec<&Material> {
        self.materials
            .values()
            .filter(|m| m.category == category)
            .collect()
    }

    /// Search materials by name
    pub fn search(&self, query: &str) -> Vec<&Material> {
        let query_lower = query.to_lowercase();
        self.materials
            .values()
            .filter(|m| {
                m.name.to_lowercase().contains(&query_lower)
                    || m.display_name.to_lowercase().contains(&query_lower)
            })
            .collect()
    }
}

impl Default for MaterialLibrary {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_material_properties() {
        let steel = Material::steel();
        assert_eq!(steel.category, MaterialCategory::Metal);
        assert_eq!(steel.physical.density, 7850.0);
        assert_eq!(steel.visual.metallic, 1.0);
    }

    #[test]
    fn test_material_library() {
        let library = MaterialLibrary::new();

        assert!(library.get("steel").is_some());
        assert!(library.get("aluminum").is_some());

        let metals = library.by_category(MaterialCategory::Metal);
        assert_eq!(metals.len(), 2); // steel and aluminum

        let search_results = library.search("steel");
        assert_eq!(search_results.len(), 1);
    }
}

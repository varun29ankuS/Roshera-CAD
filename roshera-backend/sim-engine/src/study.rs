//! A static study: what holds the part, what loads it, what it is made of,
//! and what it must achieve. The request names faces by their live ids (what
//! the viewport and the agent pick); the report records each one's
//! persistent id, so the run can be audited and replayed after edits.

use serde::{Deserialize, Serialize};

use crate::loads::Spread;

/// One static study on one part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Study {
    pub supports: Vec<Support>,
    pub loads: Vec<Load>,
    pub material: Material,
    pub goal: Goal,
    #[serde(default)]
    pub mesh: MeshControl,
}

/// How a set of faces is held.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Support {
    pub kind: SupportKind,
    pub faces: Vec<u32>,
}

/// The ways a face can be held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportKind {
    /// Held in every direction (welded, bonded).
    Fixed,
    /// A bolt clamping the part to a rigid frame: the named faces (washer
    /// face, hole wall) held in every direction.
    BoltClamp,
    /// A pin in a hole: the wall held radially, free to turn and slide on
    /// the pin.
    Pin,
    /// A roller or slider: held along the face normal only.
    Slider,
}

/// A force on a set of faces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Load {
    pub faces: Vec<u32>,
    /// The total force, N, in model axes.
    pub force_n: [f64; 3],
    #[serde(default = "even")]
    pub spread: Spread,
}

fn even() -> Spread {
    Spread::Even
}

/// What the part is made of. Linear elastic and isotropic. The yield
/// strength and its source are what a safety factor is measured against: a
/// study without them is solved, but its safety factor is refused, never
/// guessed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Material {
    pub name: String,
    pub youngs_modulus_mpa: f64,
    pub poisson_ratio: f64,
    #[serde(default)]
    pub yield_strength_mpa: Option<f64>,
    /// Where the yield value comes from (a datasheet, a standard with its
    /// edition, the customer's own data).
    #[serde(default)]
    pub source: Option<String>,
}

/// What the part must achieve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Goal {
    pub min_safety_factor: f64,
}

/// Mesh size control. `size_mm` is the coarse mesh's largest element; the
/// proving (fine) mesh uses [`FINE_RATIO`] of it. `None`: a twentieth of the
/// part's largest extent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshControl {
    #[serde(default)]
    pub size_mm: Option<f64>,
}

/// The fine mesh's size as a share of the coarse mesh's.
pub const FINE_RATIO: f64 = 0.6;

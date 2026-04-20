//! Export module for geometry serialization
//!
//! Provides functionality to export B-Rep solids to standard file formats.
//!
//! # Supported Formats
//! - **STL** (Binary and ASCII): Triangle mesh format for 3D printing and FEA
//! - **OBJ** (Wavefront): Triangle mesh with normals, optional vertex welding

pub mod obj;
pub mod stl;

pub use obj::{export_obj, write_obj, write_obj_welded, ObjError};
pub use stl::{
    export_stl_ascii, export_stl_binary, read_stl_binary, write_stl_ascii, write_stl_binary,
    StlError,
};

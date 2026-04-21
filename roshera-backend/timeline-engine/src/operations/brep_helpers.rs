//! Helper extensions for BRepModel to simplify operations

use geometry_engine::{
    math::Point3,
    primitives::{
        curve::{CurveId, ParameterRange},
        edge::{Edge, EdgeId, EdgeOrientation},
        face::{Face, FaceId, FaceOrientation},
        r#loop::{Loop, LoopId, LoopType},
        shell::{Shell, ShellId, ShellType},
        solid::{Solid, SolidId},
        surface::SurfaceId,
        topology_builder::BRepModel,
        vertex::VertexId,
    },
};

/// Extension trait for BRepModel to add convenience methods
pub trait BRepModelExt {
    /// Add a vertex at the given position
    fn add_vertex(&mut self, position: Point3) -> VertexId;

    /// Add an edge between two vertices
    fn add_edge(&mut self, start: VertexId, end: VertexId, curve: Option<CurveId>) -> EdgeId;

    /// Add a loop
    fn add_loop(&mut self, loop_type: LoopType) -> LoopId;

    /// Add a face
    fn add_face(&mut self, surface: Option<SurfaceId>) -> FaceId;

    /// Add a shell
    fn add_shell(&mut self, shell_type: ShellType) -> ShellId;

    /// Add a solid
    fn add_solid(&mut self) -> SolidId;

    /// Get mutable access to vertices store
    fn vertices_mut(&mut self) -> &mut geometry_engine::primitives::vertex::VertexStore;

    /// Get mutable access to edges store
    fn edges_mut(&mut self) -> &mut geometry_engine::primitives::edge::EdgeStore;

    /// Get mutable access to loops store
    fn loops_mut(&mut self) -> &mut geometry_engine::primitives::r#loop::LoopStore;

    /// Get mutable access to faces store
    fn faces_mut(&mut self) -> &mut geometry_engine::primitives::face::FaceStore;

    /// Get mutable access to shells store
    fn shells_mut(&mut self) -> &mut geometry_engine::primitives::shell::ShellStore;

    /// Get mutable access to solids store
    fn solids_mut(&mut self) -> &mut geometry_engine::primitives::solid::SolidStore;
}

impl BRepModelExt for BRepModel {
    fn add_vertex(&mut self, position: Point3) -> VertexId {
        self.vertices.add(position.x, position.y, position.z)
    }

    fn add_edge(&mut self, start: VertexId, end: VertexId, curve: Option<CurveId>) -> EdgeId {
        let edge = Edge::new(
            0, // ID will be assigned by store
            start,
            end,
            curve.unwrap_or(0),
            EdgeOrientation::Forward,
            ParameterRange {
                start: 0.0,
                end: 1.0,
            },
        );
        self.edges.add(edge)
    }

    fn add_loop(&mut self, loop_type: LoopType) -> LoopId {
        let loop_ = Loop::new(0, loop_type); // ID will be assigned by store
        self.loops.add(loop_)
    }

    fn add_face(&mut self, surface: Option<SurfaceId>) -> FaceId {
        // For operations that don't need surfaces, we create a simple face
        // In a full implementation, this would properly handle surface creation
        let surface_id = surface.unwrap_or(0);
        let face = Face::new(0, surface_id, 0, FaceOrientation::Forward); // ID and outer_loop will be assigned later
        self.faces.add(face)
    }

    fn add_shell(&mut self, shell_type: ShellType) -> ShellId {
        let shell = Shell::new(0, shell_type); // ID will be assigned by store
        self.shells.add(shell)
    }

    fn add_solid(&mut self) -> SolidId {
        // Create a solid with a default outer shell
        // In real usage, the shell would be set after creation
        let solid = Solid::new(0, 0); // ID will be assigned by store, outer_shell will be set later
        self.solids.add(solid)
    }

    fn vertices_mut(&mut self) -> &mut geometry_engine::primitives::vertex::VertexStore {
        &mut self.vertices
    }

    fn edges_mut(&mut self) -> &mut geometry_engine::primitives::edge::EdgeStore {
        &mut self.edges
    }

    fn loops_mut(&mut self) -> &mut geometry_engine::primitives::r#loop::LoopStore {
        &mut self.loops
    }

    fn faces_mut(&mut self) -> &mut geometry_engine::primitives::face::FaceStore {
        &mut self.faces
    }

    fn shells_mut(&mut self) -> &mut geometry_engine::primitives::shell::ShellStore {
        &mut self.shells
    }

    fn solids_mut(&mut self) -> &mut geometry_engine::primitives::solid::SolidStore {
        &mut self.solids
    }
}

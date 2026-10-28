//! Bodies: a welded triangle mesh with the B-rep's faces, edges and corners on it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::geometry::{Direction, Mm, Point, Radians};
use super::ids::{BodyName, EdgeIndex, EdgeName, FaceIndex, FaceName, FeatureId, MaterialName, MeshVertex, VertexIndex, VertexName};

/// One body: a watertight, welded triangle mesh whose vertices its faces share.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Body {
    /// Unique in the snapshot. A part's bodies keep their names; in an assembly the first
    /// placement of a component's body keeps the body's name and every further placement adds
    /// its occurrence path in brackets (`Body1 [Arm:2]`).
    pub name: BodyName,
    /// The physical material assigned to the body, if any (one of [`super::Snapshot::materials`]).
    pub material: Option<MaterialName>,
    /// Every edge of the mesh is shared by exactly two triangles, once each way (false for a
    /// surface body or a mesh that did not close).
    pub closed: bool,
    /// Vertex positions, world coordinates; [`MeshVertex`] indexes them.
    pub positions: Vec<Point>,
    /// The B-rep faces, in the body's face order ([`FaceIndex`]).
    pub faces: Vec<Face>,
    /// The B-rep edges, in the body's edge order ([`EdgeIndex`]).
    pub edges: Vec<Edge>,
    /// The B-rep corners, in the body's corner order ([`VertexIndex`]).
    pub vertices: Vec<Vertex>,
    /// Key/value data for downstream tools (empty for now).
    pub attributes: BTreeMap<String, String>,
}

/// One B-rep face and its triangles.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Face {
    pub index: FaceIndex,
    pub name: FaceName,
    pub surface: Surface,
    /// Counter-clockwise seen from outside.
    pub triangles: Vec<[MeshVertex; 3]>,
    /// The features that made or shaped the face, in the order its name mentions them.
    pub features: Vec<FeatureId>,
    /// Key/value data for downstream tools (empty for now).
    pub attributes: BTreeMap<String, String>,
}

/// One B-rep edge.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Edge {
    pub index: EdgeIndex,
    pub name: EdgeName,
    /// The faces it bounds: two, or one for a seam within a face.
    pub faces: Vec<FaceIndex>,
    pub curve: Curve,
    /// The corners at its start and end (the same one for a closed edge).
    pub ends: Option<[VertexIndex; 2]>,
    /// The mesh vertices along it from start to end: the triangle sides on the edge, exactly.
    /// Empty when the mesh does not hold the edge.
    pub path: Vec<MeshVertex>,
}

/// One B-rep corner.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Vertex {
    pub index: VertexIndex,
    pub name: VertexName,
    pub point: Point,
    /// The mesh vertex at the corner.
    pub position: Option<MeshVertex>,
    /// The edges that end there.
    pub edges: Vec<EdgeIndex>,
}

/// The surface a face lies on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Surface {
    /// `normal` points out of the material.
    Plane {
        point: Point,
        normal: Direction,
    },
    Cylinder {
        origin: Point,
        axis: Direction,
        radius: Mm,
    },
    /// `axis` points from the apex into the cone.
    Cone {
        apex: Point,
        axis: Direction,
        half_angle: Radians,
    },
    Sphere {
        center: Point,
        radius: Mm,
    },
    /// `major` is the ring radius, negative on a spindle torus's inner sheet; `minor` the tube's.
    Torus {
        center: Point,
        axis: Direction,
        major: Mm,
        minor: Mm,
    },
    /// A B-spline or NURBS patch: `points[i][j]` weighted `weights[i][j]` (all 1 for a B-spline)
    /// over the knots `knots_u` (index i) and `knots_v` (index j); ∂S/∂u × ∂S/∂v points out of
    /// the material. The face is the part of the patch its edges bound.
    Nurbs {
        degree_u: u32,
        degree_v: u32,
        knots_u: Vec<f64>,
        knots_v: Vec<f64>,
        points: Vec<Vec<Point>>,
        weights: Vec<Vec<f64>>,
    },
    /// Any other surface (a spline swept round an axis, an imported mesh), or a kind this
    /// version does not know.
    #[default]
    #[serde(other)]
    Other,
}

/// The curve an edge lies on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Curve {
    Line {
        start: Point,
        end: Point,
    },
    /// A circle or an arc of one, run counter-clockwise about `axis` through `angle` (2π for a
    /// whole circle).
    Circle {
        center: Point,
        axis: Direction,
        radius: Mm,
        angle: Radians,
    },
    /// Any other curve, or a kind this version does not know.
    #[default]
    #[serde(other)]
    Other,
}

//! The snapshot: plain, versioned data (docs/formats/exchange-snapshot.md).
//!
//! Compatibility rules, for writers and readers alike:
//! - Fields are only ever added, never renamed, retyped or removed, and a field added later has a
//!   default (empty, zero or `None`), so a reader built against an older or newer version reads
//!   the data with the fields it knows. Every struct reads missing fields as their defaults and
//!   ignores fields it does not know.
//! - A surface kind a reader does not know reads as [`Surface::Other`], a parameter kind or source
//!   it does not know as `Other`.
//! - [`SCHEMA`] goes up only for a change that breaks these rules (a field that means something
//!   else); a reader refuses a schema above the one it was built for.
//!
//! Lengths are in millimetres (`unit` says so), angles in radians, densities in kg/m³ and
//! material properties in the SI units their names carry (`_pa`, `_w_m_k`, …).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The snapshot schema this crate writes and reads.
pub const SCHEMA: u32 = 1;

/// A design evaluated at a set of parameter values, packaged for analysis.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Snapshot {
    /// [`SCHEMA`] when written by this crate.
    pub schema: u32,
    /// The length unit of every coordinate and length: always `mm`.
    pub unit: String,
    /// The design's name.
    pub design: String,
    /// Every parameter at the values the snapshot was taken with, in the design's order.
    pub parameters: Vec<Parameter>,
    /// The physical materials the bodies use, by name.
    pub materials: Vec<Material>,
    /// The bodies, placed in world coordinates, in the design's order.
    pub bodies: Vec<Body>,
    /// The timeline's modelling features (sketches and construction geometry left out), in
    /// timeline order, suppressed ones included.
    pub features: Vec<Feature>,
    /// The edits the snapshot was taken with ([`crate::Edit`]), in the order given, each with
    /// the feature it became.
    pub edits: Vec<AppliedEdit>,
}

/// One parameter of the design: a user parameter, a sketch dimension or a feature input.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Parameter {
    pub name: String,
    /// The expression as the design stores it (a text parameter's text).
    pub expression: String,
    /// The value in `unit` (none for a text parameter or one whose expression fails).
    pub value: Option<f64>,
    /// `mm`, `deg`, another unit the design uses (`in`, `cm`, `rad`, …), empty when unit-less, or
    /// `text`.
    pub unit: String,
    pub kind: ParameterKind,
    pub source: ParameterSource,
    pub comment: String,
}

/// What a parameter measures.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ParameterKind {
    Length,
    Angle,
    Unitless,
    Text,
    /// A kind this version does not know.
    #[default]
    #[serde(other)]
    Other,
}

/// Where a parameter comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ParameterSource {
    /// Made by the user in the parameters table.
    User,
    /// A sketch dimension.
    Sketch,
    /// A feature's input (an extrude's distance, a fillet's radius…).
    Feature,
    /// A source this version does not know.
    #[default]
    #[serde(other)]
    Other,
}

/// A physical material.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Material {
    pub name: String,
    /// Mass density (kg/m³), the one SolidCraft computes mass with.
    pub density_kg_m3: f64,
    /// Engineering data the design holds for the material (a design material's card), as the
    /// user entered it: candidate values, not certified ones.
    pub properties: Vec<MaterialProperty>,
}

/// One engineering property of a material (docs/formats/exchange-snapshot.md lists the kinds
/// and keys).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct MaterialProperty {
    /// `density`, `elasticity`, `tensile_strength`, `thermal_conductivity`, …
    pub kind: String,
    /// Numbers by name, each in the SI unit its name ends with (`young_modulus_pa`,
    /// `poisson_ratio`, `w_m_k_1`, `temperature_k`, …).
    pub values: BTreeMap<String, f64>,
    /// Words by name (`symmetry`, `axes`, `state`, `source`).
    pub labels: BTreeMap<String, String>,
}

/// One body: a watertight, welded triangle mesh whose vertices its faces share.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Body {
    /// Unique in the snapshot. A part's bodies keep their names; in an assembly the first
    /// placement of a component's body keeps the body's name and every further placement adds
    /// its occurrence path in brackets (`Body1 [Arm:2]`).
    pub name: String,
    /// The physical material assigned to the body, if any (one of [`Snapshot::materials`]).
    pub material: Option<String>,
    /// Every edge of the mesh is shared by exactly two triangles (false for a surface body or a
    /// mesh that did not close).
    pub closed: bool,
    /// Vertex positions (mm, world coordinates).
    pub positions: Vec<[f64; 3]>,
    /// The body's B-rep faces, in the body's own face order.
    pub faces: Vec<Face>,
    /// Key/value data for downstream tools (empty for now).
    pub attributes: BTreeMap<String, String>,
    /// The body's B-rep edges, in the body's own edge order.
    pub edges: Vec<Edge>,
    /// The body's corners: where edges end.
    pub vertices: Vec<Vertex>,
}

/// One B-rep edge of a body.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Edge {
    /// The edge's place in the body's edge order.
    pub index: u32,
    /// The edge's persistent name: the names of the faces it bounds (`F1:-z|F2:side`), numbered
    /// `~k` when several edges share them.
    pub name: String,
    /// The faces it bounds ([`Face::index`]): two, or one for a seam.
    pub faces: Vec<u32>,
    pub curve: Curve,
    /// The edge as a polyline within the snapshot's chord tolerance, start to end.
    pub points: Vec<[f64; 3]>,
    /// The mesh vertices along the edge, in order ([`Body::positions`] indices): the triangle
    /// sides that run along it. Empty when the mesh does not trace it as one chain (a seam).
    pub path: Vec<u32>,
}

/// The exact curve an edge lies on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Curve {
    Line {
        start: [f64; 3],
        end: [f64; 3],
    },
    /// A circle or an arc of one: the edge runs counter-clockwise about `axis` through `angle`
    /// radians (2π for a whole circle).
    Circle {
        center: [f64; 3],
        axis: [f64; 3],
        radius: f64,
        angle: f64,
    },
    /// Any other curve, or a kind this version does not know.
    #[default]
    #[serde(other)]
    Other,
}

/// A corner of a body.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Vertex {
    /// The names of the faces that meet there (`F1:-x|F1:-y|F1:-z`), numbered `~k` when several
    /// corners share them.
    pub name: String,
    pub point: [f64; 3],
    /// The mesh vertex at the corner ([`Body::positions`] index).
    pub position: Option<u32>,
    /// The edges that end there ([`Edge::index`]).
    pub edges: Vec<u32>,
}

/// One modelling feature of the timeline.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Feature {
    /// The feature's id: `F<id>` in persistent names.
    pub id: u64,
    pub name: String,
    /// `extrude`, `revolve`, `box`, `cylinder`, `hole`, `fillet`, `chamfer`, `shell`, `pattern`, …
    #[serde(rename = "type")]
    pub kind: String,
    pub suppressed: bool,
    /// `new_body`, `join`, `cut` or `intersect`, for features that have one.
    pub operation: Option<String>,
    /// The numeric inputs, evaluated, by snake-case label (`distance`, `radius`, …): lengths in
    /// mm, angles in radians, counts and factors as numbers.
    pub values: BTreeMap<String, f64>,
    /// The keys of `values` that are lengths.
    pub lengths: Vec<String>,
    /// Where a primitive or hole sits (a box's corner, a cylinder's base, a hole's position).
    pub placement: Option<[f64; 3]>,
    /// The direction it builds along: an extrude's (the sketch normal, flipped for a negative
    /// extrude), a cylinder's axis, a hole's drilling direction.
    pub direction: Option<[f64; 3]>,
    /// An extrude in both directions.
    pub two_sided: bool,
    /// The sketch it takes its profiles from, with those profiles.
    pub sketch: Option<Sketch>,
    /// A pattern's or mirror's copies: each copy's 4×4 transform, row-major, the identity left out.
    pub transforms: Vec<[[f64; 4]; 4]>,
    /// The bodies it changes (a join's, cut's or fillet's), a combine's target.
    pub targets: Vec<String>,
    /// A combine's tool bodies.
    pub tools: Vec<String>,
    /// The names it gives the bodies it makes.
    pub creates: Vec<String>,
    /// The snapshot's bodies it made or shaped faces of, or names.
    pub bodies: Vec<String>,
}

/// A sketch and the closed profiles a feature uses from it (sketch coordinates, mm).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Sketch {
    pub id: u64,
    pub name: String,
    pub origin: [f64; 3],
    pub x_dir: [f64; 3],
    pub y_dir: [f64; 3],
    pub normal: [f64; 3],
    pub profiles: Vec<Profile>,
}

/// A closed profile: its outer loop and holes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Profile {
    /// Its place in the solved sketch's profile list.
    pub index: u32,
    pub area: f64,
    pub outer: Vec<SketchCurve>,
    pub holes: Vec<Vec<SketchCurve>>,
}

/// A curve of a profile loop. A loop of arcs about one centre that closes is one circle.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SketchCurve {
    Line {
        a: [f64; 2],
        b: [f64; 2],
    },
    /// From angle `start` through `sweep` radians (counter-clockwise when positive).
    Arc {
        center: [f64; 2],
        radius: f64,
        start: f64,
        sweep: f64,
    },
    Circle {
        center: [f64; 2],
        radius: f64,
    },
    /// A spline, conic or other curve, or a kind this version does not know.
    #[default]
    #[serde(other)]
    Other,
}

/// An edit a snapshot was taken with and the timeline feature it became.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct AppliedEdit {
    /// `suppress`, `partition` or `split_face`.
    pub kind: String,
    /// The edit's name (a partition's or face split's), empty for a suppression.
    pub name: String,
    /// The features it is: the one suppressed, or those it added (a partition adds one split per
    /// body it cuts). Faces it made carry these ids in their names.
    pub features: Vec<u64>,
    /// The bodies it cut or split faces of.
    pub bodies: Vec<String>,
}

/// One B-rep face of a body and its triangles.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Face {
    /// The face's place in the body's face order (as in the 3MF face tags).
    pub index: u32,
    /// The face's persistent name (`F1:-z`, `F2:side`, `F3:side:l4`, …): it stays with the face when a
    /// parameter changes.
    pub name: String,
    pub surface: Surface,
    /// Triangles as indices into [`Body::positions`], counter-clockwise seen from outside.
    pub triangles: Vec<[u32; 3]>,
    /// Key/value data for downstream tools (empty for now).
    pub attributes: BTreeMap<String, String>,
    /// The features that made or shaped the face (the `F<id>`s of its name), in name order.
    pub features: Vec<u64>,
}

/// The surface a face lies on. Directions are unit vectors.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Surface {
    /// `normal` points out of the material.
    Plane {
        point: [f64; 3],
        normal: [f64; 3],
    },
    Cylinder {
        origin: [f64; 3],
        axis: [f64; 3],
        radius: f64,
    },
    /// `axis` points from the apex into the cone; `half_angle` in radians.
    Cone {
        apex: [f64; 3],
        axis: [f64; 3],
        half_angle: f64,
    },
    Sphere {
        center: [f64; 3],
        radius: f64,
    },
    /// `major` is the ring radius, negative on a spindle torus's inner sheet; `minor` the tube's.
    Torus {
        center: [f64; 3],
        axis: [f64; 3],
        major: f64,
        minor: f64,
    },
    /// A B-spline or NURBS patch: `points[i][j]` with `weights[i][j]` (all 1 for a B-spline)
    /// over the knots `knots_u` (index i) and `knots_v` (index j), clamped or not;
    /// ∂S/∂u × ∂S/∂v points out of the material. The face is the part of the patch its edges
    /// bound.
    Nurbs {
        degree_u: u32,
        degree_v: u32,
        knots_u: Vec<f64>,
        knots_v: Vec<f64>,
        points: Vec<Vec<[f64; 3]>>,
        weights: Vec<Vec<f64>>,
    },
    /// Any other surface (a surface of revolution of a spline, an imported mesh), or a kind this
    /// version does not know.
    #[default]
    #[serde(other)]
    Other,
}

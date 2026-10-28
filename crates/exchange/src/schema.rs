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
}

/// The analytic surface a face lies on. Directions are unit vectors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    /// Any other surface (a spline, an imported mesh), or a kind this version does not know.
    #[default]
    #[serde(other)]
    Other,
}

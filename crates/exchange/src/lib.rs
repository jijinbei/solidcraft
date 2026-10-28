//! SolidCraft exchange: the one boundary through which analysis tools (electromagnetics,
//! structural, CFD) read SolidCraft designs.
//!
//! Three entry points, and plain data out:
//!
//! ```no_run
//! # fn main() -> Result<(), solidcraft_exchange::Error> {
//! let bytes = std::fs::read("part.sc").unwrap_or_default();
//! let design = solidcraft_exchange::open(&bytes)?;
//! for p in design.parameters() {
//!     println!("{} = {} ({:?} {})", p.name, p.expression, p.value, p.unit);
//! }
//! let overrides = [("width".to_string(), solidcraft_exchange::Override::Value(30.0))];
//! let snapshot = design.snapshot(&overrides, &solidcraft_exchange::Options::default())?;
//! let json = solidcraft_exchange::to_json(&snapshot)?;
//! # let _ = json;
//! # Ok(())
//! # }
//! ```
//!
//! A [`Snapshot`] is serde data with a schema number: the parameters, the physical materials,
//! and per body one watertight, welded triangle mesh (the tagged 3MF export's tessellation,
//! docs/formats/3mf-face-tags.md) whose triangles are grouped by B-rep face, each face with its
//! persistent name and analytic surface. No kernel or document type appears in it, so tools
//! depend on this crate alone. The compatibility rules are in [`schema`] and in
//! docs/formats/exchange-snapshot.md.
//!
//! Everything is bounded: the design's size, the number of overrides, bodies, faces and
//! triangles, the chord tolerance. The same design, overrides and options give the same bytes.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod features;
pub mod schema;
mod topology;

use std::collections::BTreeSet;
use std::sync::Mutex;

use solidcraft_doc::expr::{Kind, unit_info};
use solidcraft_doc::physical_material::{Elasticity, MaterialAxes, PropertyData, ValidatedCard};
use solidcraft_doc::{Document, Model, ModelBody};
use solidcraft_kernel::FaceSurface;

pub use schema::{
    AppliedEdit, Body, Curve, Edge, Face, Feature, Material, MaterialProperty, Parameter, ParameterKind, ParameterSource, Profile, SCHEMA, Sketch,
    SketchCurve, Snapshot, Surface, Vertex,
};

/// Largest design file [`open`] reads (the design reader's own limit).
pub const MAX_DESIGN_BYTES: usize = solidcraft_io::MAX_DESIGN_BYTES;
/// Most parameter overrides one snapshot takes.
pub const MAX_OVERRIDES: usize = 1024;
/// Longest override expression (bytes).
pub const MAX_EXPRESSION_BYTES: usize = 4096;
/// Largest magnitude of an override value.
pub const MAX_VALUE: f64 = 1e12;
/// Most bodies in one snapshot.
pub const MAX_BODIES: usize = 100_000;
/// Most faces in one snapshot (all bodies together).
pub const MAX_FACES: usize = 1_000_000;
/// Most triangles in one snapshot (all bodies together).
pub const MAX_TRIANGLES: usize = 20_000_000;
/// Most edits one snapshot takes.
pub const MAX_EDITS: usize = 1024;
/// Most NURBS control points in one snapshot (all faces together).
pub const MAX_CONTROL_POINTS: usize = 10_000_000;
/// Longest edit name (bytes).
pub const MAX_NAME_BYTES: usize = 256;
/// Finest chord tolerance (mm); the export meshes never go below it either.
pub const MIN_TOLERANCE_MM: f64 = 1e-3;
/// Coarsest absolute chord tolerance (mm).
pub const MAX_TOLERANCE_MM: f64 = 1e4;
/// Range of a relative chord tolerance (a share of each body's size).
pub const RELATIVE_TOLERANCE: (f64, f64) = (1e-6, 0.1);
/// The default relative chord tolerance: the export's standard quality.
pub const DEFAULT_RELATIVE_TOLERANCE: f64 = 2e-4;

/// Why a design could not be opened or a snapshot taken.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The bytes are not a design SolidCraft reads.
    #[error("cannot read the design: {0}")]
    Read(String),
    /// An override names an unknown parameter or gives it a value it cannot take.
    #[error("parameter: {0}")]
    Parameter(String),
    /// Timeline features failed at these parameter values (one `feature: message` per entry).
    #[error("{} timeline feature(s) failed: {}", .0.len(), .0.join("; "))]
    Evaluation(Vec<String>),
    /// An option is out of range, or a body filter names a body the design does not have.
    #[error("{0}")]
    Invalid(String),
    /// The snapshot would be larger than a limit allows.
    #[error("too large: {0}")]
    Limit(String),
    /// The design uses something this crate cannot export yet.
    #[error("not supported yet: {0}")]
    NotSupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A new value for a parameter.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Override {
    /// A number in the parameter's own unit ([`Parameter::unit`]).
    Value(f64),
    /// An expression as the parameters table takes it (`25 mm`, `width / 2`, a quoted text).
    Expression(String),
}

impl From<f64> for Override {
    fn from(v: f64) -> Self {
        Override::Value(v)
    }
}

impl From<&str> for Override {
    fn from(e: &str) -> Self {
        Override::Expression(e.to_string())
    }
}

/// How closely the meshes follow curved faces: the largest distance between a triangle and its
/// face (chord tolerance).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Tolerance {
    /// A share of each body's size (its box diagonal), within [`RELATIVE_TOLERANCE`]; never
    /// finer than [`MIN_TOLERANCE_MM`].
    Relative(f64),
    /// Millimetres, from [`MIN_TOLERANCE_MM`] to [`MAX_TOLERANCE_MM`].
    Absolute(f64),
}

impl Default for Tolerance {
    fn default() -> Self {
        Tolerance::Relative(DEFAULT_RELATIVE_TOLERANCE)
    }
}

/// A change made to the design before it is evaluated, on a copy: the design itself is left as
/// it is. Edits apply in order, after the parameter overrides, as timeline features at the end
/// of the timeline, so faces they make get persistent names that name them
/// ([`Snapshot::edits`] gives their feature ids).
///
/// As JSON: `{"suppress": 3}`, `{"partition": {"name", "point", "normal", "bodies"?}}`,
/// `{"split_face": {"name", "body", "face", "point", "normal"}}` ([`edits_from_json`]).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Edit {
    /// Rebuild without the feature `F<id>` (a hole filled, a fillet back to its sharp edge, a
    /// boss gone). Suppressing a feature that makes a body takes that body away; one a later
    /// feature needs fails that feature ([`Error::Evaluation`]).
    Suppress(u64),
    /// Cut bodies by the plane through `point` square to `normal`: each body the plane crosses
    /// becomes two (only `bodies`, when not empty). The cut faces are named after the split.
    Partition {
        name: String,
        point: [f64; 3],
        normal: [f64; 3],
        #[serde(default)]
        bodies: Vec<String>,
    },
    /// Split face `face` (a persistent name) of `body` where the plane through `point` square to
    /// `normal` meets it; the solid is unchanged, the pieces are named after the face (`#k`).
    SplitFace { name: String, body: String, face: String, point: [f64; 3], normal: [f64; 3] },
}

/// What a snapshot contains.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Options {
    pub tolerance: Tolerance,
    /// Only these bodies (by [`Body::name`]); all when `None`.
    pub bodies: Option<Vec<String>>,
    /// Edits made before evaluating ([`Edit`]).
    pub edits: Vec<Edit>,
}

impl Options {
    /// The options with this chord tolerance.
    pub fn with_tolerance(mut self, tolerance: Tolerance) -> Self {
        self.tolerance = tolerance;
        self
    }

    /// The options with these edits.
    pub fn with_edits(mut self, edits: Vec<Edit>) -> Self {
        self.edits = edits;
        self
    }

    /// The options with only these bodies.
    pub fn with_bodies(mut self, bodies: Vec<String>) -> Self {
        self.bodies = Some(bodies);
        self
    }
}

/// An opened design. Snapshots of it share one evaluation, so a second snapshot recomputes only
/// the features its overrides change.
#[derive(Debug)]
pub struct Design {
    doc: Document,
    model: Mutex<Model>,
}

/// Open a design file's bytes (`.sc`, a legacy `.solidcraft`/`.solvecraft` container, or the
/// plain JSON design).
pub fn open(bytes: &[u8]) -> Result<Design> {
    if bytes.len() > MAX_DESIGN_BYTES {
        return Err(Error::Limit(format!("the design is {} MB, over the {} MB a design file may hold", bytes.len() >> 20, MAX_DESIGN_BYTES >> 20)));
    }
    let doc = solidcraft_io::read_design(bytes).map_err(|e| Error::Read(e.to_string()))?;
    Ok(Design { doc, model: Mutex::new(Model::new()) })
}

/// Edits from their JSON array (as [`Edit`] describes), refused past [`MAX_EDITS`].
pub fn edits_from_json(text: &str) -> Result<Vec<Edit>> {
    let edits: Vec<Edit> = serde_json::from_str(text).map_err(|e| Error::Invalid(format!("edits JSON: {e}")))?;
    if edits.len() > MAX_EDITS {
        return Err(Error::Limit(format!("{} edits, over {MAX_EDITS}", edits.len())));
    }
    Ok(edits)
}

/// The snapshot as JSON: deterministic bytes (fixed field and element order, shortest
/// round-trip numbers).
pub fn to_json(snapshot: &Snapshot) -> Result<String> {
    serde_json::to_string(snapshot).map_err(|e| Error::Invalid(format!("snapshot as JSON: {e}")))
}

/// A snapshot read from JSON. A schema above [`SCHEMA`] is refused; unknown fields are ignored.
pub fn from_json(text: &str) -> Result<Snapshot> {
    let s: Snapshot = serde_json::from_str(text).map_err(|e| Error::Read(format!("snapshot JSON: {e}")))?;
    if s.schema > SCHEMA {
        return Err(Error::NotSupported(format!("snapshot schema {} (this reader knows up to {SCHEMA})", s.schema)));
    }
    Ok(s)
}

impl Design {
    /// The design's name.
    pub fn name(&self) -> &str {
        &self.doc.name
    }

    /// Every parameter (user parameters, sketch dimensions, named feature inputs, text
    /// parameters) at the design's own values.
    pub fn parameters(&self) -> Vec<Parameter> {
        parameters_of(&self.doc)
    }

    /// The design evaluated with `overrides` applied, packaged as a [`Snapshot`]. The design
    /// itself is left as it is. Fails, naming them, when timeline features fail at these values.
    pub fn snapshot(&self, overrides: &[(String, Override)], options: &Options) -> Result<Snapshot> {
        let tolerance = checked_tolerance(options.tolerance)?;
        let mut doc = self.with_overrides(overrides)?;
        let edits = self.apply_edits(&mut doc, &options.edits)?;
        let state = self.evaluate(&doc)?;
        let world = solidcraft_doc::world_state(&doc, &state);
        if let Some(wanted) = &options.bodies {
            if wanted.len() > MAX_BODIES {
                return Err(Error::Limit(format!("more than {MAX_BODIES} bodies asked for")));
            }
            let have: BTreeSet<&str> = world.bodies.iter().map(|b| b.name.as_str()).collect();
            let missing: Vec<&str> = wanted.iter().map(String::as_str).filter(|n| !have.contains(n)).collect();
            if !missing.is_empty() {
                return Err(Error::Invalid(format!("no body named {}", missing.join(", "))));
            }
        }
        let chosen: Vec<&ModelBody> = world.bodies.iter().filter(|b| options.bodies.as_ref().is_none_or(|w| w.contains(&b.name))).collect();
        if chosen.len() > MAX_BODIES {
            return Err(Error::Limit(format!("{} bodies, over {MAX_BODIES}", chosen.len())));
        }
        let mut budget = Budget { faces: MAX_FACES, triangles: MAX_TRIANGLES, control_points: MAX_CONTROL_POINTS };
        let mut bodies = Vec::with_capacity(chosen.len());
        for b in chosen {
            bodies.push(body_of(&doc, b, tolerance, &mut budget)?);
        }
        let used: BTreeSet<&str> = bodies.iter().filter_map(|b| b.material.as_deref()).collect();
        let materials = used.into_iter().map(|m| material_of(&doc, m)).collect();
        let features = features::features_of(&doc, &state, &bodies);
        Ok(Snapshot {
            schema: SCHEMA,
            unit: "mm".into(),
            design: doc.name.clone(),
            parameters: parameters_of(&doc),
            materials,
            bodies,
            features,
            edits,
        })
    }

    /// The design's model at `doc`, refused when a timeline feature fails.
    fn evaluate(&self, doc: &Document) -> Result<std::sync::Arc<solidcraft_doc::ModelState>> {
        let mut model = self.model.lock().unwrap_or_else(|e| e.into_inner());
        model.evaluate(doc);
        let failed: Vec<String> = model.results.iter().filter_map(|r| r.error.as_ref().map(|e| format!("{}: {e}", r.name))).collect();
        if !failed.is_empty() {
            return Err(Error::Evaluation(failed));
        }
        Ok(model.state())
    }

    /// `edits` made to `doc`, in order: suppressions set on their features, partitions and face
    /// splits added as features at the end of the timeline (each from the model as the edits
    /// before it left it).
    fn apply_edits(&self, doc: &mut Document, edits: &[Edit]) -> Result<Vec<AppliedEdit>> {
        if edits.len() > MAX_EDITS {
            return Err(Error::Limit(format!("{} edits, over {MAX_EDITS}", edits.len())));
        }
        let mut applied = Vec::with_capacity(edits.len());
        for edit in edits {
            match edit {
                Edit::Suppress(id) => {
                    let f = doc.feature_mut(*id).ok_or_else(|| Error::Invalid(format!("suppress: the design has no feature F{id}")))?;
                    f.suppressed = true;
                    applied.push(AppliedEdit { kind: "suppress".into(), name: String::new(), features: vec![*id], bodies: Vec::new() });
                }
                Edit::Partition { name, point, normal, bodies } => {
                    let plane = edit_plane("partition", name, *point, *normal)?;
                    let state = self.evaluate(doc)?;
                    if let Some(missing) = bodies.iter().find(|b| state.body(b).is_none()) {
                        return Err(Error::Invalid(format!("partition `{name}`: no body `{missing}`")));
                    }
                    let n = solidcraft_geom::Vec3::new(normal[0], normal[1], normal[2]);
                    let o = solidcraft_geom::Vec3::new(point[0], point[1], point[2]);
                    let cut: Vec<String> = state
                        .bodies
                        .iter()
                        .filter(|b| bodies.is_empty() || bodies.contains(&b.name))
                        .filter(|b| {
                            // The plane crosses the body: its mesh has points on both sides.
                            let eps = (b.body.size() * 1e-6).max(1e-9);
                            let side = |p: &solidcraft_geom::Vec3| (*p - o).dot(n);
                            let m = b.mesh();
                            m.positions.iter().any(|p| side(p) > eps) && m.positions.iter().any(|p| side(p) < -eps)
                        })
                        .map(|b| b.name.clone())
                        .collect();
                    let mut features = Vec::with_capacity(cut.len());
                    for body in &cut {
                        let kind = solidcraft_doc::FeatureKind::Split {
                            body: solidcraft_doc::BodyName(body.clone()),
                            plane: solidcraft_doc::PlaneRef::Custom { plane },
                            tool: None,
                            sketch: None,
                            line: None,
                        };
                        let label = if cut.len() > 1 { format!("{name} ({body})") } else { name.clone() };
                        features.push(doc.add_feature(kind, Some(&label)).map_err(|e| Error::Invalid(format!("partition `{name}`: {e}")))?);
                    }
                    applied.push(AppliedEdit { kind: "partition".into(), name: name.clone(), features, bodies: cut });
                }
                Edit::SplitFace { name, body, face, point, normal } => {
                    let plane = edit_plane("split_face", name, *point, *normal)?;
                    let state = self.evaluate(doc)?;
                    if state.body(body).is_none() {
                        return Err(Error::Invalid(format!("split_face `{name}`: no body `{body}`")));
                    }
                    let at = solidcraft_doc::naming::named_face_point(&state, Some(body), face)
                        .map_err(|e| Error::Invalid(format!("split_face `{name}`: {e}")))?;
                    let kind = solidcraft_doc::FeatureKind::SplitFace {
                        faces: vec![solidcraft_doc::FacePick(at)],
                        body: Some(solidcraft_doc::BodyName(body.clone())),
                        plane: solidcraft_doc::PlaneRef::Custom { plane },
                        tool: None,
                        sketch: None,
                    };
                    let id = doc.add_feature(kind, Some(name)).map_err(|e| Error::Invalid(format!("split_face `{name}`: {e}")))?;
                    applied.push(AppliedEdit { kind: "split_face".into(), name: name.clone(), features: vec![id], bodies: vec![body.clone()] });
                }
            }
        }
        Ok(applied)
    }

    /// A copy of the design with the overrides applied, all or none.
    fn with_overrides(&self, overrides: &[(String, Override)]) -> Result<Document> {
        if overrides.len() > MAX_OVERRIDES {
            return Err(Error::Limit(format!("{} parameter overrides, over {MAX_OVERRIDES}", overrides.len())));
        }
        let mut doc = self.doc.clone();
        if overrides.is_empty() {
            return Ok(doc);
        }
        let params = parameters_of(&doc);
        let mut changes = Vec::with_capacity(overrides.len());
        for (name, value) in overrides {
            let Some(p) = params.iter().find(|p| p.name == *name) else {
                return Err(Error::Parameter(format!("unknown parameter `{name}`")));
            };
            let expression = match value {
                Override::Value(v) => {
                    if p.kind == ParameterKind::Text {
                        return Err(Error::Parameter(format!("`{name}` is a text parameter; give it a quoted text")));
                    }
                    if !v.is_finite() || v.abs() > MAX_VALUE {
                        return Err(Error::Parameter(format!("`{name}`: {v} is not a finite number of at most {MAX_VALUE:e}")));
                    }
                    // `{}` writes the shortest text that reads back as the same number.
                    if p.unit.is_empty() { format!("{v}") } else { format!("{v} {}", p.unit) }
                }
                Override::Expression(e) => {
                    if e.len() > MAX_EXPRESSION_BYTES || e.trim().is_empty() || e.chars().any(char::is_control) {
                        return Err(Error::Parameter(format!(
                            "`{name}`: the expression is empty, has control characters or is over {MAX_EXPRESSION_BYTES} bytes"
                        )));
                    }
                    e.clone()
                }
            };
            changes.push((name.clone(), expression));
        }
        doc.change_params_atomic(&changes).map_err(|e| Error::Parameter(e.to_string()))?;
        Ok(doc)
    }
}

/// The chord tolerance, refused when out of range.
fn checked_tolerance(t: Tolerance) -> Result<Tolerance> {
    let ok = match t {
        Tolerance::Relative(r) => r.is_finite() && (RELATIVE_TOLERANCE.0..=RELATIVE_TOLERANCE.1).contains(&r),
        Tolerance::Absolute(a) => a.is_finite() && (MIN_TOLERANCE_MM..=MAX_TOLERANCE_MM).contains(&a),
    };
    if ok {
        Ok(t)
    } else {
        Err(Error::Invalid(format!(
            "chord tolerance {t:?} is out of range (relative {:e} to {}, absolute {MIN_TOLERANCE_MM} to {MAX_TOLERANCE_MM} mm)",
            RELATIVE_TOLERANCE.0, RELATIVE_TOLERANCE.1
        )))
    }
}

/// What is left of the snapshot's face, triangle and control point limits.
struct Budget {
    faces: usize,
    triangles: usize,
    control_points: usize,
}

/// The plane of a partition or face split, refused when its point or normal is not finite, the
/// normal has no direction, or the name is empty or too long.
fn edit_plane(kind: &str, name: &str, point: [f64; 3], normal: [f64; 3]) -> Result<solidcraft_geom::Plane> {
    if name.trim().is_empty() || name.len() > MAX_NAME_BYTES || name.chars().any(char::is_control) {
        return Err(Error::Invalid(format!("{kind}: a name of 1 to {MAX_NAME_BYTES} bytes without control characters is needed")));
    }
    if point.iter().chain(&normal).any(|x| !x.is_finite() || x.abs() > MAX_VALUE) {
        return Err(Error::Invalid(format!("{kind} `{name}`: the point and normal must be finite")));
    }
    let o = solidcraft_geom::Vec3::new(point[0], point[1], point[2]);
    let n = solidcraft_geom::Vec3::new(normal[0], normal[1], normal[2]);
    n.normalized()
        .and_then(|n| solidcraft_geom::Plane::from_normal(o, n))
        .ok_or_else(|| Error::Invalid(format!("{kind} `{name}`: the normal has no direction")))
}

fn parameters_of(doc: &Document) -> Vec<Parameter> {
    doc.param_rows()
        .into_iter()
        .map(|r| {
            let kind = if r.text.is_some() || solidcraft_doc::text_params::is_text_unit(&r.unit) {
                ParameterKind::Text
            } else {
                match unit_info(&r.unit).map(|(k, _)| k) {
                    Some(Kind::Length) => ParameterKind::Length,
                    Some(Kind::Angle) => ParameterKind::Angle,
                    Some(Kind::Unitless) => ParameterKind::Unitless,
                    None => ParameterKind::Other,
                }
            };
            let source = match r.source {
                "user" => ParameterSource::User,
                "sketch" => ParameterSource::Sketch,
                "feature" => ParameterSource::Feature,
                _ => ParameterSource::Other,
            };
            Parameter {
                name: r.name,
                expression: r.expression,
                value: r.value.filter(|v| v.is_finite()),
                unit: r.unit,
                kind,
                source,
                comment: r.comment,
            }
        })
        .collect()
}

/// The material assigned to a body (by its definition's name, so every placement of a
/// component's body has it), as the library or the design spells it.
fn assigned_material(doc: &Document, b: &ModelBody) -> Option<String> {
    let assigned = doc.materials.get(b.definition_name()).or_else(|| doc.materials.get(&b.name))?;
    doc.material_name(assigned)
}

fn body_of(doc: &Document, b: &ModelBody, tolerance: Tolerance, budget: &mut Budget) -> Result<Body> {
    let tol = match tolerance {
        Tolerance::Relative(r) => (b.body.size() * r).max(MIN_TOLERANCE_MM),
        Tolerance::Absolute(a) => a,
    };
    let (mesh, polylines, edge_faces) =
        solidcraft_io::tagged_mesh_with_edges(&b.body, tol).map_err(|e| Error::Invalid(format!("body {}: {e}", b.name)))?;
    let face_count = mesh.faces as usize;
    if face_count > budget.faces || mesh.triangles.len() > budget.triangles {
        return Err(Error::Limit(format!(
            "body {} brings the snapshot over {MAX_FACES} faces or {MAX_TRIANGLES} triangles (a coarser tolerance or a body filter helps)",
            b.name
        )));
    }
    budget.faces -= face_count;
    budget.triangles -= mesh.triangles.len();
    let positions: Vec<[f64; 3]> = mesh.positions.iter().map(|p| v3(*p)).collect();
    if positions.iter().flatten().any(|x| !x.is_finite()) {
        return Err(Error::Invalid(format!("body {} has a vertex that is not a finite point", b.name)));
    }
    let names = solidcraft_doc::naming::face_names(b);
    let mut by_face: Vec<Vec<[u32; 3]>> = vec![Vec::new(); face_count];
    for (t, f) in mesh.triangles.iter().zip(&mesh.tri_faces) {
        if let Some(list) = by_face.get_mut(*f as usize) {
            list.push(*t);
        }
    }
    let closed = is_closed(&mesh.triangles);
    let mut faces = Vec::with_capacity(face_count);
    for (i, triangles) in by_face.into_iter().enumerate() {
        let surface = match solidcraft_kernel::face_surface(&b.body, i) {
            Some(s) => surface_of(s, &positions, &triangles),
            None => match solidcraft_kernel::face_nurbs(&b.body, i) {
                Some(n) => {
                    let count = n.points.iter().map(Vec::len).sum::<usize>();
                    if count > budget.control_points {
                        return Err(Error::Limit(format!("body {} brings the snapshot over {MAX_CONTROL_POINTS} NURBS control points", b.name)));
                    }
                    budget.control_points -= count;
                    Surface::Nurbs {
                        degree_u: u32::try_from(n.degree_u).unwrap_or(u32::MAX),
                        degree_v: u32::try_from(n.degree_v).unwrap_or(u32::MAX),
                        knots_u: n.knots_u,
                        knots_v: n.knots_v,
                        points: n.points.iter().map(|row| row.iter().map(|p| v3(*p)).collect()).collect(),
                        weights: n.weights,
                    }
                }
                None => Surface::Other,
            },
        };
        let name = names.get(i).cloned().unwrap_or_default();
        faces.push(Face {
            index: u32::try_from(i).unwrap_or(u32::MAX),
            features: features::feature_ids(&name),
            name,
            surface,
            triangles,
            attributes: Default::default(),
        });
    }
    let close = (b.body.size() * 1e-6).max(1e-7);
    let index = topology::PointIndex::new(&positions, close);
    let edge_names = solidcraft_doc::naming::edge_names(b);
    let edges = topology::edges_of(&b.body, &polylines, &edge_faces, &edge_names, &positions, &mesh.triangles, &mesh.tri_faces, &index, tol);
    let face_names: Vec<String> = faces.iter().map(|f| f.name.clone()).collect();
    let vertices = topology::vertices_of(&edges, &face_names, &index, close);
    Ok(Body { name: b.name.clone(), material: assigned_material(doc, b), closed, positions, faces, attributes: Default::default(), edges, vertices })
}

/// Every edge used once in each direction: a closed, consistently oriented mesh.
fn is_closed(triangles: &[[u32; 3]]) -> bool {
    if triangles.is_empty() {
        return false;
    }
    let mut edges: Vec<(u32, u32)> = triangles.iter().flat_map(|[a, b, c]| [(*a, *b), (*b, *c), (*c, *a)]).collect();
    edges.sort_unstable();
    if edges.windows(2).any(|w| w[0] == w[1]) {
        return false;
    }
    edges.iter().all(|(a, b)| edges.binary_search(&(*b, *a)).is_ok())
}

/// A point or direction as snapshot data; `+ 0.0` writes -0 as 0.
fn v3(v: solidcraft_geom::Vec3) -> [f64; 3] {
    [v.x + 0.0, v.y + 0.0, v.z + 0.0]
}

/// The kernel's surface as snapshot data. A plane's normal is turned to agree with its
/// triangles, which face out of the material.
fn surface_of(s: FaceSurface, positions: &[[f64; 3]], triangles: &[[u32; 3]]) -> Surface {
    match s {
        FaceSurface::Plane { normal, point } => {
            let n = normal.vec();
            let mut sum = [0.0; 3];
            for t in triangles {
                let [Some(a), Some(b), Some(c)] = t.map(|i| positions.get(i as usize)) else { continue };
                let (u, v) = (sub(*b, *a), sub(*c, *a));
                let x = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
                for k in 0..3 {
                    sum[k] += x[k];
                }
            }
            let flip = sum[0] * n.x + sum[1] * n.y + sum[2] * n.z < 0.0;
            let normal = if flip { v3(-n) } else { v3(n) };
            Surface::Plane { point: v3(point), normal }
        }
        FaceSurface::Cylinder { origin, axis, radius } => Surface::Cylinder { origin: v3(origin), axis: v3(axis.vec()), radius },
        FaceSurface::Cone { apex, axis, half_angle } => Surface::Cone { apex: v3(apex), axis: v3(axis.vec()), half_angle },
        FaceSurface::Sphere { center, radius } => Surface::Sphere { center: v3(center), radius },
        FaceSurface::Torus { center, axis, major, minor } => Surface::Torus { center: v3(center), axis: v3(axis.vec()), major, minor },
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// A material by name: its density and, for a design material with a card, its engineering data.
fn material_of(doc: &Document, name: &str) -> Material {
    // g/cm³ to kg/m³.
    let density_kg_m3 = doc.material_density(name).map_or(0.0, |d| d * 1000.0);
    let properties = doc.user_material(name).and_then(|m| m.physical_card.as_ref()).map(card_properties).unwrap_or_default();
    Material { name: name.to_string(), density_kg_m3, properties }
}

fn axes_label(a: MaterialAxes) -> &'static str {
    match a {
        MaterialAxes::Material123 => "material123",
        MaterialAxes::WoodLrt => "wood_lrt",
    }
}

/// `name_1`, `name_2`, `name_3`: a value per material axis.
fn triple(values: &mut std::collections::BTreeMap<String, f64>, name: &str, v: &[f64; 3]) {
    for (k, x) in v.iter().enumerate() {
        values.insert(format!("{name}_{}", k + 1), *x);
    }
}

fn card_properties(card: &ValidatedCard) -> Vec<MaterialProperty> {
    card.card()
        .properties
        .iter()
        .map(|p| {
            let mut out = MaterialProperty { kind: p.data.kind().to_string(), ..Default::default() };
            let (values, labels) = (&mut out.values, &mut out.labels);
            match &p.data {
                PropertyData::Density { kg_m3 } => {
                    values.insert("kg_m3".into(), *kg_m3);
                }
                PropertyData::Elasticity { model: Elasticity::Isotropic { young_modulus_pa, poisson_ratio } } => {
                    labels.insert("symmetry".into(), "isotropic".into());
                    values.insert("young_modulus_pa".into(), *young_modulus_pa);
                    values.insert("poisson_ratio".into(), *poisson_ratio);
                }
                PropertyData::Elasticity { model: Elasticity::Orthotropic { axes, young_moduli_pa, shear_moduli_pa, poisson_ratios } } => {
                    labels.insert("symmetry".into(), "orthotropic".into());
                    labels.insert("axes".into(), axes_label(*axes).into());
                    triple(values, "young_modulus_pa", young_moduli_pa);
                    triple(values, "shear_modulus_pa", shear_moduli_pa);
                    triple(values, "poisson_ratio", poisson_ratios);
                }
                PropertyData::TensileStrength { pa }
                | PropertyData::CompressiveStrength { pa }
                | PropertyData::YieldStrength { pa }
                | PropertyData::ShearStrength { pa } => {
                    values.insert("pa".into(), *pa);
                }
                PropertyData::ThermalConductivity { w_m_k, axes } => {
                    labels.insert("axes".into(), axes_label(*axes).into());
                    triple(values, "w_m_k", w_m_k);
                }
                PropertyData::ThermalExpansion { per_k, axes } => {
                    labels.insert("axes".into(), axes_label(*axes).into());
                    triple(values, "per_k", per_k);
                }
                PropertyData::SpecificHeat { j_kg_k } => {
                    values.insert("j_kg_k".into(), *j_kg_k);
                }
            }
            let c = &p.conditions;
            for (k, v) in [
                ("temperature_k", c.temperature_k),
                ("moisture_dry_basis_kg_kg", c.moisture_dry_basis_kg_kg),
                ("strain_rate_per_s", c.strain_rate_per_s),
            ] {
                if let Some(v) = v {
                    out.values.insert(k.into(), v);
                }
            }
            if let Some(state) = &c.state {
                out.labels.insert("state".into(), state.clone());
            }
            out.labels.insert("source".into(), p.source_id.clone());
            out
        })
        .collect()
}

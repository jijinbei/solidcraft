//! The timeline as snapshot data: each modelling feature with its inputs evaluated, the sketch
//! profiles it uses, its copies' transforms and the bodies it built.
//!
//! Which bodies a feature built comes from two places: the persistent face names (each names the
//! features that made the face: `F8:side:l1`, `F3:inner:F1:+y`), and the bodies the feature
//! names (targets, tools, the bodies it creates).

use std::collections::{BTreeMap, BTreeSet};

use solidcraft_doc::expr::Kind;
use solidcraft_doc::{Document, FeatureKind, ModelState, Operation, SolvedSketch, plain};
use solidcraft_geom::{Seg2, Vec3};

use crate::schema::{Body, Feature, Profile, Sketch, SketchCurve};

/// A vector shorter than this has no direction.
const MIN_NORM: f64 = 1e-12;
/// A unit vector's component this small is zero.
const ZERO_COMPONENT: f64 = 1e-12;
/// Arcs whose centres and radii agree within this (mm) are of one circle.
const SAME_ARC: f64 = 1e-9;
/// Arcs that turn a full circle within this (radians) close it.
const FULL_TURN: f64 = 1e-6;
/// Most features listed (a design's timeline is capped far above any real part).
pub const MAX_FEATURES: usize = 100_000;

/// The ids of the features a persistent face name mentions (`F3:inner:F1:+y` → 3, 1), in text
/// order: by the name grammar, or the `F<n>` tokens of a name it does not read.
pub(crate) fn feature_ids(name: &str) -> Vec<u64> {
    use solidcraft_doc::persistent_name::{Base, FaceName};
    match FaceName::parse(name).ok().filter(|f| !matches!(f.base, Base::Other(_))) {
        Some(f) => f.features().into_iter().map(|id| id.0).collect(),
        None => feature_tokens(name),
    }
}

/// The `F<n>` tokens of `name` not inside a word.
fn feature_tokens(name: &str) -> Vec<u64> {
    let bytes = name.as_bytes();
    (0..bytes.len())
        .filter_map(|i| {
            let starts = bytes.get(i) == Some(&b'F') && (i == 0 || !bytes.get(i - 1).is_some_and(|b| b.is_ascii_alphanumeric()));
            if !starts {
                return None;
            }
            let digits: &str = name.get(i + 1..)?.split(|c: char| !c.is_ascii_digit()).next()?;
            if digits.is_empty() { None } else { digits.parse().ok() }
        })
        .collect()
}

/// `Taper angle` → `taper_angle`, `Distance (0 to 1)` → `distance_0_to_1`.
fn snake(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

fn operation_name(op: Operation) -> &'static str {
    match op {
        Operation::NewBody => "new_body",
        Operation::Join => "join",
        Operation::Cut => "cut",
        Operation::Intersect => "intersect",
    }
}

fn xyz(v: Vec3) -> [f64; 3] {
    [v.x + 0.0, v.y + 0.0, v.z + 0.0]
}

/// `v` scaled to unit length, components within 1e-12 of zero made zero.
fn unit(v: [f64; 3]) -> Option<[f64; 3]> {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (n.is_finite() && n > MIN_NORM).then(|| v.map(|c| if (c / n).abs() < ZERO_COMPONENT { 0.0 } else { c / n }))
}

fn curves(segs: &[Seg2]) -> Vec<SketchCurve> {
    // A loop of arcs about one centre and radius that turns a full circle is a circle.
    let arcs: Vec<(solidcraft_geom::Vec2, f64, f64)> =
        segs.iter().filter_map(|s| if let Seg2::Arc { center, radius, sweep, .. } = s { Some((*center, *radius, *sweep)) } else { None }).collect();
    if let Some(&(c0, r0, _)) = arcs.first()
        && arcs.len() == segs.len()
    {
        let same = arcs.iter().all(|(c, r, _)| (c.x - c0.x).abs() < SAME_ARC && (c.y - c0.y).abs() < SAME_ARC && (r - r0).abs() < SAME_ARC);
        let turn: f64 = arcs.iter().map(|a| a.2).sum();
        if same && (turn.abs() - std::f64::consts::TAU).abs() < FULL_TURN {
            return vec![SketchCurve::Circle { center: [c0.x, c0.y], radius: r0 }];
        }
    }
    segs.iter()
        .map(|s| match s {
            Seg2::Line { a, b } => SketchCurve::Line { a: [a.x, a.y], b: [b.x, b.y] },
            Seg2::Arc { center, radius, start, sweep } => {
                SketchCurve::Arc { center: [center.x, center.y], radius: *radius, start: *start, sweep: *sweep }
            }
            _ => SketchCurve::Other,
        })
        .collect()
}

fn sketch_of(ss: &SolvedSketch, sel: Option<&solidcraft_doc::ProfileSel>) -> Sketch {
    let indices: Vec<usize> = match sel {
        Some(sel) => solidcraft_doc::profile_selection_indices(ss, sel).unwrap_or_else(|_| (0..ss.profiles.len()).collect()),
        None => (0..ss.profiles.len()).collect(),
    };
    let profiles = indices
        .into_iter()
        .filter_map(|i| {
            let p = ss.profiles.get(i)?;
            Some(Profile {
                index: u32::try_from(i).ok()?,
                area: p.area,
                outer: curves(&p.region.outer.segs),
                holes: p.region.holes.iter().map(|h| curves(&h.segs)).collect(),
            })
        })
        .collect();
    Sketch {
        id: ss.feature,
        name: ss.name.clone(),
        origin: xyz(ss.plane.origin),
        x_dir: xyz(ss.plane.x),
        y_dir: xyz(ss.plane.y),
        normal: xyz(ss.plane.normal()),
        profiles,
    }
}

/// Whether a feature only builds reference geometry (a sketch, a construction plane, axis or
/// point): no faces of a body.
fn reference_only(kind: &FeatureKind) -> bool {
    matches!(
        kind,
        FeatureKind::Sketch { .. }
            | FeatureKind::ConstructionPlane { .. }
            | FeatureKind::ConstructionAxis { .. }
            | FeatureKind::ConstructionPoint { .. }
    )
}

/// The timeline's modelling features, described against the snapshot's `bodies`.
pub(crate) fn features_of(doc: &Document, state: &ModelState, bodies: &[Body]) -> Vec<Feature> {
    // The bodies each feature made faces of.
    let mut made: BTreeMap<u64, BTreeSet<&str>> = BTreeMap::new();
    for b in bodies {
        for f in &b.faces {
            for id in &f.features {
                made.entry(*id).or_default().insert(b.name.as_str());
            }
        }
    }
    let (vals, _) = doc.param_values();
    let eval = |e: &str, k: Kind| Document::eval_in(&vals, e, k).ok().filter(|v| v.is_finite());
    let point = |p: &[solidcraft_doc::Expr; 3]| -> Option<[f64; 3]> {
        Some([eval(&p[0], Kind::Length)?, eval(&p[1], Kind::Length)?, eval(&p[2], Kind::Length)?])
    };
    let mut out = Vec::new();
    for f in doc.features.iter().filter(|f| !reference_only(&f.kind)).take(MAX_FEATURES) {
        let mut values = BTreeMap::new();
        let mut lengths = Vec::new();
        for (label, e, k) in f.kind.inputs() {
            if let Some(v) = eval(&e, k) {
                let mut key = snake(label);
                let mut n = 2;
                while values.contains_key(&key) {
                    key = format!("{}_{n}", snake(label));
                    n += 1;
                }
                if k == Kind::Length {
                    lengths.push(key.clone());
                }
                values.insert(key, v);
            }
        }
        let kind = serde_json::to_value(&f.kind).ok().and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string)).unwrap_or_default();
        let mut node = Feature {
            id: f.id,
            name: f.name.clone(),
            kind,
            suppressed: f.suppressed,
            values,
            lengths,
            creates: f.body_names.clone(),
            ..Feature::default()
        };
        let sketch = |id: solidcraft_doc::SketchId| state.sketch(*id);
        match &f.kind {
            FeatureKind::Extrude { sketch: id, profiles, extent, operation, targets, .. } => {
                node.operation = Some(operation_name(*operation).into());
                node.targets = plain(targets);
                if let Some(ss) = sketch(*id) {
                    let n = xyz(ss.plane.normal());
                    let negative = matches!(extent.direction, solidcraft_doc::Direction::Negative);
                    node.direction = unit(if negative { n.map(|c| -c) } else { n });
                    node.two_sided = matches!(extent.direction, solidcraft_doc::Direction::Symmetric) || extent.distance2.is_some();
                    node.sketch = Some(sketch_of(ss, Some(profiles)));
                }
            }
            FeatureKind::Revolve { sketch: id, profiles, operation, targets, .. }
            | FeatureKind::Sweep { sketch: id, profiles, operation, targets, .. } => {
                node.operation = Some(operation_name(*operation).into());
                node.targets = plain(targets);
                node.sketch = sketch(*id).map(|ss| sketch_of(ss, Some(profiles)));
            }
            FeatureKind::Loft { operation, targets, .. } => {
                node.operation = Some(operation_name(*operation).into());
                node.targets = plain(targets);
            }
            FeatureKind::Box { corner, operation, .. } => {
                node.operation = Some(operation_name(*operation).into());
                node.placement = point(corner);
                node.direction = Some([0.0, 0.0, 1.0]);
            }
            FeatureKind::Cylinder { base, axis, operation, .. } => {
                node.operation = Some(operation_name(*operation).into());
                node.placement = point(base);
                node.direction = unit(xyz(**axis));
            }
            FeatureKind::Sphere { center, operation, .. } | FeatureKind::Torus { center, operation, .. } => {
                node.operation = Some(operation_name(*operation).into());
                node.placement = point(center);
            }
            FeatureKind::Hole { position, direction, .. } => {
                node.operation = Some("cut".into());
                node.placement = Some(xyz(**position));
                node.direction = unit(xyz(**direction));
            }
            FeatureKind::Combine { target, tools, operation, .. } => {
                node.operation = Some(operation_name(*operation).into());
                node.targets = vec![target.to_string()];
                node.tools = plain(tools);
            }
            FeatureKind::Shell { body, .. } | FeatureKind::Fillet { body, .. } | FeatureKind::Chamfer { body, .. } => {
                node.targets = body.iter().map(ToString::to_string).collect();
            }
            FeatureKind::Split { body, .. } => node.targets = vec![body.to_string()],
            FeatureKind::Pattern { pattern, bodies: copied, .. } => {
                node.targets = plain(copied);
                if let Ok(m) = solidcraft_doc::pattern_matrices(doc, state, pattern) {
                    // SolidCraft's matrices are column-major (`m[3]` the translation).
                    let rows = |m: [[f64; 4]; 4]| std::array::from_fn(|i| std::array::from_fn(|j| m[j][i]));
                    node.transforms = m.into_iter().filter(|m| *m != solidcraft_doc::IDENTITY).map(rows).collect();
                }
            }
            FeatureKind::Mirror { plane, bodies: copied, .. } => {
                node.targets = plain(copied);
                if let Ok(p) = doc.resolve_plane_in(&vals, plane, 0, Some(state))
                    && let Some(n) = unit(xyz(p.normal()))
                {
                    // x' = x − 2 ((x − o)·n) n
                    let o = xyz(p.origin);
                    let d = 2.0 * (o[0] * n[0] + o[1] * n[1] + o[2] * n[2]);
                    let mut m = solidcraft_doc::IDENTITY;
                    for (i, row) in m.iter_mut().take(3).enumerate() {
                        for (j, c) in row.iter_mut().take(3).enumerate() {
                            *c -= 2.0 * n[i] * n[j];
                        }
                        row[3] = d * n[i];
                    }
                    node.transforms = vec![m];
                }
            }
            _ => {}
        }
        // The bodies it made faces of, and the snapshot's bodies it names.
        let mut on: BTreeSet<&str> = made.get(&f.id).cloned().unwrap_or_default();
        for name in node.targets.iter().chain(&node.tools).chain(&node.creates) {
            if let Some(b) = bodies.iter().find(|b| b.name == *name) {
                on.insert(b.name.as_str());
            }
        }
        node.bodies = bodies.iter().map(|b| b.name.as_str()).filter(|n| on.contains(n)).map(str::to_string).collect();
        out.push(node);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_ids_come_from_every_name_part() {
        assert_eq!(feature_ids("F3:inner:F2:blend:F1:+y|F1:-x"), vec![3, 2, 1, 1]);
        assert_eq!(feature_ids("F44:side:l12"), vec![44]);
        assert_eq!(feature_ids("OFF7"), Vec::<u64>::new());
        assert_eq!(feature_ids("F"), Vec::<u64>::new());
    }

    #[test]
    fn labels_become_snake_case_keys() {
        assert_eq!(snake("Taper angle"), "taper_angle");
        assert_eq!(snake("Distance (0 to 1)"), "distance_0_to_1");
    }
}

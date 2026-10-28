//! The snapshot contract, continued: edges and corners, the feature timeline, edits (suppress,
//! partition, face split) and free-form faces. Expected values come from the geometry each
//! design describes, not from SolidCraft's own output.

use std::collections::BTreeSet;

use solidcraft_doc::{Dir, Document, FeatureKind, Operation, point_expr};
use solidcraft_exchange::{Curve, Edit, Error, Options, Snapshot, Surface, Tolerance, edits_from_json, open, to_json};
use solidcraft_geom::Vec3;

/// A 20 × 20 × 10 mm box with a radius-3 hole through it along Z, at (10, 10).
fn holed_doc() -> Document {
    let mut doc = Document::new("plate");
    doc.add_feature(
        FeatureKind::Box {
            corner: point_expr(Vec3::ZERO),
            length: "20".into(),
            width: "20".into(),
            height: "10".into(),
            operation: Operation::NewBody,
        },
        None,
    )
    .unwrap();
    doc.add_feature(
        FeatureKind::Cylinder {
            base: point_expr(Vec3::new(10.0, 10.0, -1.0)),
            axis: Dir(Vec3::Z),
            radius: "3".into(),
            height: "12".into(),
            operation: Operation::Cut,
            references: Vec::new(),
        },
        None,
    )
    .unwrap();
    doc
}

fn bytes(doc: &Document) -> Vec<u8> {
    doc.to_json().into_bytes()
}

fn snap_with(doc: &Document, edits: Vec<Edit>) -> Result<Snapshot, Error> {
    open(&bytes(doc))?.snapshot(&[], &Options::default().with_tolerance(Tolerance::Absolute(0.01)).with_edits(edits))
}

fn volume(b: &solidcraft_exchange::Body) -> f64 {
    let p = &b.positions;
    b.faces
        .iter()
        .flat_map(|f| f.triangles.iter())
        .map(|[a, b, c]| {
            let (a, b, c) = (p[*a as usize], p[*b as usize], p[*c as usize]);
            a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0])
        })
        .sum::<f64>()
        / 6.0
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The holed plate's edges: 12 box edges, 2 × 2 rim arcs of the hole (its wall is two halves)
/// and the 2 seams between the halves. Each is named after its faces, its curve is exact, and its
/// path is a chain of triangle sides running along it.
#[test]
fn edges_are_named_exact_and_traced_by_the_mesh() {
    let s = snap_with(&holed_doc(), Vec::new()).unwrap();
    let b = &s.bodies[0];
    assert_eq!(b.edges.len(), 18, "{:?}", b.edges.iter().map(|e| &e.name).collect::<Vec<_>>());
    let names: BTreeSet<&str> = b.edges.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names.len(), b.edges.len(), "edge names are unique");
    // Every triangle side of the mesh, without direction.
    let sides: BTreeSet<(u32, u32)> = b
        .faces
        .iter()
        .flat_map(|f| f.triangles.iter())
        .flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])])
        .map(|(a, c)| (a.min(c), a.max(c)))
        .collect();
    let (mut lines, mut circles) = (0, 0);
    for (i, e) in b.edges.iter().enumerate() {
        assert_eq!(e.index as usize, i);
        assert_eq!(e.faces.len(), 2, "{}", e.name);
        // Named after the two faces it bounds.
        let base = e.name.split('~').next().unwrap();
        let mut theirs: Vec<&str> = e.faces.iter().map(|f| b.faces[*f as usize].name.as_str()).collect();
        theirs.sort();
        assert_eq!(base, theirs.join("|"), "{}", e.name);
        // A chain of triangle sides from one end of the polyline to the other.
        assert!(e.path.len() >= 2, "{} has no path", e.name);
        assert!(e.path.windows(2).all(|w| sides.contains(&(w[0].min(w[1]), w[0].max(w[1])))), "{}", e.name);
        let (first, last) = (b.positions[e.path[0] as usize], b.positions[*e.path.last().unwrap() as usize]);
        // Welding keeps one copy of points within 10⁻⁶ of the body's size.
        assert!(dist(first, e.points[0]) < WELD && dist(last, *e.points.last().unwrap()) < WELD, "{}", e.name);
        match e.curve {
            Curve::Line { start, end } => {
                lines += 1;
                assert!(dist(start, e.points[0]) < 1e-9 && dist(end, *e.points.last().unwrap()) < 1e-9, "{}", e.name);
            }
            Curve::Circle { center, axis, radius, angle } => {
                circles += 1;
                assert!((radius - 3.0).abs() < 1e-9 && (center[0] - 10.0).abs() < 1e-9 && (center[1] - 10.0).abs() < 1e-9, "{}", e.name);
                assert!((axis[2].abs() - 1.0).abs() < 1e-9 && (angle - PI_).abs() < 1e-9, "{}: a half circle", e.name);
                // The path's vertices are on the circle.
                for v in &e.path {
                    let p = b.positions[*v as usize];
                    assert!(((p[0] - 10.0).hypot(p[1] - 10.0) - 3.0).abs() < 1e-6, "{}", e.name);
                }
            }
            _ => panic!("{}: {:?}", e.name, e.curve),
        }
    }
    assert_eq!((lines, circles), (14, 4));
}

const PI_: f64 = std::f64::consts::PI;
/// How far welding may move a point of the 30 mm plate (10⁻⁶ of its size, with margin).
const WELD: f64 = 1e-4;

/// The corners: the box's 8 and the ends of the hole's 2 seams. A box corner is where three faces
/// meet; each corner sits on a mesh vertex; names are unique.
#[test]
fn corners_are_where_edges_end() {
    let s = snap_with(&holed_doc(), Vec::new()).unwrap();
    let b = &s.bodies[0];
    assert_eq!(b.vertices.len(), 12, "{:?}", b.vertices);
    let names: BTreeSet<&str> = b.vertices.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names.len(), 12);
    for v in &b.vertices {
        let at = v.position.unwrap_or_else(|| panic!("{} has no mesh vertex", v.name));
        assert!(dist(b.positions[at as usize], v.point) < WELD, "{}", v.name);
        for e in &v.edges {
            let e = &b.edges[*e as usize];
            assert!(dist(e.points[0], v.point) < 1e-6 || dist(*e.points.last().unwrap(), v.point) < 1e-6, "{} {}", v.name, e.name);
        }
    }
    let corner = b.vertices.iter().find(|v| dist(v.point, [0.0, 0.0, 0.0]) < 1e-9).unwrap();
    assert_eq!((corner.name.split('|').count(), corner.edges.len()), (3, 3), "{}", corner.name);
}

/// The timeline: a box that makes the body and a cylinder that cuts it, with their inputs, and
/// the faces each one made.
#[test]
fn features_list_their_inputs_and_the_faces_they_made() {
    let s = snap_with(&holed_doc(), Vec::new()).unwrap();
    let kinds: Vec<(&str, Option<&str>)> = s.features.iter().map(|f| (f.kind.as_str(), f.operation.as_deref())).collect();
    assert_eq!(kinds, [("box", Some("new_body")), ("cylinder", Some("cut"))]);
    let (block, hole) = (&s.features[0], &s.features[1]);
    assert_eq!((block.values["length"], block.values["width"], block.values["height"]), (20.0, 20.0, 10.0));
    assert_eq!(block.placement, Some([0.0, 0.0, 0.0]));
    assert_eq!((hole.values["radius"], hole.direction, hole.placement), (3.0, Some([0.0, 0.0, 1.0]), Some([10.0, 10.0, -1.0])));
    assert!(hole.lengths.contains(&"radius".to_string()));
    let body = &s.bodies[0];
    for f in [block, hole] {
        assert_eq!(f.bodies, vec![body.name.clone()]);
        assert!(!f.suppressed);
    }
    let made = |id: u64| body.faces.iter().filter(|f| f.features.first() == Some(&id)).count();
    assert_eq!((made(block.id), made(hole.id)), (6, 2));
}

/// Suppressing the hole fills it: six planes, the full volume; the design itself keeps it.
#[test]
fn suppressing_a_feature_rebuilds_without_it() {
    let doc = holed_doc();
    let design = open(&bytes(&doc)).unwrap();
    let hole = snap_with(&doc, Vec::new()).unwrap().features[1].id;
    let s = design.snapshot(&[], &Options::default().with_edits(vec![Edit::Suppress(hole)])).unwrap();
    let b = &s.bodies[0];
    assert_eq!(b.faces.len(), 6);
    assert!((volume(b) - 4000.0).abs() < 1e-6, "{}", volume(b));
    assert!(b.faces.iter().all(|f| matches!(f.surface, Surface::Plane { .. })));
    assert!(s.features[1].suppressed && s.features[1].bodies.is_empty());
    assert_eq!(s.edits[0].kind, "suppress");
    assert_eq!(s.edits[0].features, vec![hole]);
    // The same design without the edit still has its hole.
    assert_eq!(design.snapshot(&[], &Options::default()).unwrap().bodies[0].faces.len(), 8);
    // A feature the design lacks is refused.
    assert!(matches!(design.snapshot(&[], &Options::default().with_edits(vec![Edit::Suppress(999)])), Err(Error::Invalid(_))));
}

/// A partition at x = 5 cuts the plate in two (the hole stays whole in the second), each with a
/// cut face named after the partition's feature; a face split at x = 15 splits the top in two
/// pieces named after it and leaves the solid as it was.
#[test]
fn partitions_and_face_splits_are_named_after_their_features() {
    let doc = holed_doc();
    let cut = Edit::Partition { name: "Mid".into(), point: [5.0, 0.0, 0.0], normal: [1.0, 0.0, 0.0], bodies: Vec::new() };
    let s = snap_with(&doc, vec![cut]).unwrap();
    assert_eq!(s.bodies.len(), 2, "{:?}", s.bodies.iter().map(|b| &b.name).collect::<Vec<_>>());
    let edit = &s.edits[0];
    assert_eq!((edit.kind.as_str(), edit.name.as_str(), edit.bodies.len(), edit.features.len()), ("partition", "Mid", 1, 1));
    let mut volumes: Vec<f64> = s.bodies.iter().map(volume).collect();
    volumes.sort_by(f64::total_cmp);
    // The cells make up the plate (its hole as faceted as before); the first is a plain block.
    let whole = volume(&snap_with(&doc, Vec::new()).unwrap().bodies[0]);
    assert!((volumes[0] - 1000.0).abs() < 1e-6, "{volumes:?}");
    assert!((volumes[0] + volumes[1] - whole).abs() < 1e-6, "{volumes:?} {whole}");
    assert!((whole - (4000.0 - PI_ * 9.0 * 10.0)).abs() < 1.0, "{whole}");
    for b in &s.bodies {
        assert!(b.closed);
        let section: Vec<&str> = b.faces.iter().filter(|f| f.features.first() == Some(&edit.features[0])).map(|f| f.name.as_str()).collect();
        assert_eq!(section.len(), 1, "{}: {:?}", b.name, b.faces.iter().map(|f| &f.name).collect::<Vec<_>>());
    }
    // The top, split at x = 15.
    let plain = snap_with(&doc, Vec::new()).unwrap();
    let top = plain.bodies[0].faces.iter().find(|f| matches!(f.surface, Surface::Plane { normal, .. } if normal[2] > 0.5)).unwrap();
    let split = Edit::SplitFace {
        name: "Strip".into(),
        body: plain.bodies[0].name.clone(),
        face: top.name.clone(),
        point: [15.0, 0.0, 0.0],
        normal: [1.0, 0.0, 0.0],
    };
    let s = snap_with(&doc, vec![split]).unwrap();
    let b = &s.bodies[0];
    assert_eq!(b.faces.len(), 9);
    let pieces: Vec<&str> = b.faces.iter().map(|f| f.name.as_str()).filter(|n| n.starts_with(&format!("{}#", top.name))).collect();
    assert_eq!(pieces.len(), 2, "{:?}", b.faces.iter().map(|f| &f.name).collect::<Vec<_>>());
    assert!((volume(b) - whole).abs() < 1e-6, "{} {whole}", volume(b));
    // The edge between the pieces is a line at x = 15.
    assert!(b.edges.iter().any(|e| matches!(e.curve, Curve::Line { start, end } if (start[0] - 15.0).abs() < 1e-9 && (end[0] - 15.0).abs() < 1e-9)));
}

#[test]
fn hostile_edits_are_refused_without_panicking() {
    let doc = holed_doc();
    let plane = |name: &str, normal: [f64; 3]| Edit::Partition { name: name.into(), point: [5.0, 0.0, 0.0], normal, bodies: Vec::new() };
    for e in [
        plane("Z", [0.0, 0.0, 0.0]),
        plane("N", [f64::NAN, 0.0, 0.0]),
        plane("", [1.0, 0.0, 0.0]),
        Edit::Partition { name: "B".into(), point: [5.0, 0.0, 0.0], normal: [1.0, 0.0, 0.0], bodies: vec!["Nope".into()] },
        Edit::SplitFace { name: "S".into(), body: "Nope".into(), face: "F1:+z".into(), point: [0.0; 3], normal: [1.0, 0.0, 0.0] },
        Edit::SplitFace { name: "S".into(), body: "Body1".into(), face: "nope".into(), point: [0.0; 3], normal: [1.0, 0.0, 0.0] },
        Edit::Suppress(u64::MAX),
    ] {
        assert!(snap_with(&doc, vec![e.clone()]).is_err(), "{e:?}");
    }
    assert!(snap_with(&doc, vec![Edit::Suppress(1); solidcraft_exchange::MAX_EDITS + 1]).is_err());
    // A plane that misses every body cuts nothing.
    let miss = Edit::Partition { name: "Far".into(), point: [500.0, 0.0, 0.0], normal: [1.0, 0.0, 0.0], bodies: Vec::new() };
    let s = snap_with(&doc, vec![miss]).unwrap();
    assert_eq!((s.bodies.len(), s.edits[0].features.len()), (1, 0));
    // The JSON form.
    let edits = edits_from_json(r#"[{"suppress": 2}, {"partition": {"name": "Mid", "point": [5, 0, 0], "normal": [1, 0, 0]}}]"#).unwrap();
    assert_eq!(edits[0], Edit::Suppress(2));
    for bad in ["{", "[{\"explode\": 1}]", "[{\"suppress\": -1}]", "[{\"partition\": {\"name\": \"x\"}}]", "[{\"suppress\": 1, \"extra\": 2}]"] {
        assert!(edits_from_json(bad).is_err(), "{bad}");
    }
    // Edits give the same bytes every time.
    let cut = plane("Mid", [1.0, 0.0, 0.0]);
    assert_eq!(to_json(&snap_with(&doc, vec![cut.clone()]).unwrap()).unwrap(), to_json(&snap_with(&doc, vec![cut]).unwrap()).unwrap());
}

/// A point of a NURBS patch (Cox–de Boor), and ∂S/∂u × ∂S/∂v by central differences.
struct Patch<'a> {
    du: usize,
    dv: usize,
    ku: &'a [f64],
    kv: &'a [f64],
    points: &'a [Vec<[f64; 3]>],
    weights: &'a [Vec<f64>],
}

fn basis(knots: &[f64], degree: usize, i: usize, t: f64) -> f64 {
    if degree == 0 {
        let (a, b) = (knots[i], knots[i + 1]);
        let last = knots[knots.len() - 1];
        return if (a <= t && t < b) || (t == last && b == last && a < b) { 1.0 } else { 0.0 };
    }
    let mut v = 0.0;
    let d1 = knots[i + degree] - knots[i];
    if d1 > 0.0 {
        v += (t - knots[i]) / d1 * basis(knots, degree - 1, i, t);
    }
    let d2 = knots[i + degree + 1] - knots[i + 1];
    if d2 > 0.0 {
        v += (knots[i + degree + 1] - t) / d2 * basis(knots, degree - 1, i + 1, t);
    }
    v
}

impl Patch<'_> {
    fn range(&self) -> ([f64; 2], [f64; 2]) {
        let (nu, nv) = (self.points.len(), self.points[0].len());
        ([self.ku[self.du], self.ku[nu]], [self.kv[self.dv], self.kv[nv]])
    }

    fn at(&self, u: f64, v: f64) -> [f64; 3] {
        let (mut sum, mut w) = ([0.0; 3], 0.0);
        for (i, row) in self.points.iter().enumerate() {
            let bu = basis(self.ku, self.du, i, u);
            if bu == 0.0 {
                continue;
            }
            for (j, p) in row.iter().enumerate() {
                let b = bu * basis(self.kv, self.dv, j, v) * self.weights[i][j];
                for k in 0..3 {
                    sum[k] += b * p[k];
                }
                w += b;
            }
        }
        sum.map(|c| c / w)
    }

    /// The parameters of the patch point nearest `p`, coarse to fine.
    fn nearest(&self, p: [f64; 3]) -> (f64, f64) {
        let ([u0, u1], [v0, v1]) = self.range();
        let (mut best, mut span) = (((u0 + u1) / 2.0, (v0 + v1) / 2.0), ((u1 - u0) / 2.0, (v1 - v0) / 2.0));
        for _ in 0..12 {
            let mut found = (f64::INFINITY, best);
            for i in -10..=10 {
                for j in -10..=10 {
                    let u = (best.0 + span.0 * f64::from(i) / 10.0).clamp(u0, u1);
                    let v = (best.1 + span.1 * f64::from(j) / 10.0).clamp(v0, v1);
                    let d = dist(self.at(u, v), p);
                    if d < found.0 {
                        found = (d, (u, v));
                    }
                }
            }
            best = found.1;
            span = (span.0 / 4.0, span.1 / 4.0);
        }
        best
    }

    fn normal(&self, u: f64, v: f64) -> [f64; 3] {
        let ([u0, u1], [v0, v1]) = self.range();
        let (hu, hv) = ((u1 - u0) * 1e-5, (v1 - v0) * 1e-5);
        let (ua, ub) = ((u - hu).max(u0), (u + hu).min(u1));
        let (va, vb) = ((v - hv).max(v0), (v + hv).min(v1));
        let (a, b, c, d) = (self.at(ua, v), self.at(ub, v), self.at(u, va), self.at(u, vb));
        let su = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let sv = [d[0] - c[0], d[1] - c[1], d[2] - c[2]];
        [su[1] * sv[2] - su[2] * sv[1], su[2] * sv[0] - su[0] * sv[2], su[0] * sv[1] - su[1] * sv[0]]
    }
}

/// A closed spline outline extruded 15 mm: its side faces are B-spline patches. Every mesh
/// vertex of such a face lies on its patch, and the patch's normal points the way the face's
/// triangles do (out of the material).
#[test]
fn free_form_faces_are_nurbs_patches_their_mesh_lies_on() {
    let bytes = include_bytes!("fixtures/spline-extrude.json");
    let s = open(bytes).unwrap().snapshot(&[], &Options::default().with_tolerance(Tolerance::Absolute(0.01))).unwrap();
    let b = &s.bodies[0];
    let mut patches = 0;
    for f in &b.faces {
        let Surface::Nurbs { degree_u, degree_v, knots_u, knots_v, points, weights } = &f.surface else { continue };
        patches += 1;
        assert_eq!(knots_u.len(), points.len() + *degree_u as usize + 1, "{}", f.name);
        assert_eq!(knots_v.len(), points[0].len() + *degree_v as usize + 1, "{}", f.name);
        let patch = Patch { du: *degree_u as usize, dv: *degree_v as usize, ku: knots_u, kv: knots_v, points, weights };
        let used: BTreeSet<u32> = f.triangles.iter().flatten().copied().collect();
        for v in used {
            let p = b.positions[v as usize];
            let (u, w) = patch.nearest(p);
            assert!(dist(patch.at(u, w), p) < 1e-6, "{}: vertex {v} is {} off its patch", f.name, dist(patch.at(u, w), p));
        }
        // Orientation, at each triangle's centre.
        for t in &f.triangles {
            let [a, c, d] = t.map(|i| b.positions[i as usize]);
            let centre = [(a[0] + c[0] + d[0]) / 3.0, (a[1] + c[1] + d[1]) / 3.0, (a[2] + c[2] + d[2]) / 3.0];
            let (u, w) = patch.nearest(centre);
            let n = patch.normal(u, w);
            let (x, y) = ([c[0] - a[0], c[1] - a[1], c[2] - a[2]], [d[0] - a[0], d[1] - a[1], d[2] - a[2]]);
            let m = [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
            assert!(n[0] * m[0] + n[1] * m[1] + n[2] * m[2] > 0.0, "{}: the patch faces into the material", f.name);
        }
    }
    assert_eq!(patches, 8, "{:?}", b.faces.iter().map(|f| (&f.name, &f.surface)).collect::<Vec<_>>());
    // The spline edges are no line or circle; the straight seams between the patches are lines.
    let spline_edges = b.edges.iter().filter(|e| matches!(e.curve, Curve::Other)).count();
    assert_eq!(spline_edges, 16);
    assert!(b.edges.iter().all(|e| !e.path.is_empty()), "every edge is traced");
}

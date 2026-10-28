//! The snapshot contract: what analysis tools rely on. Each design is built here with the
//! document API, saved as a design file and opened through `solidcraft_exchange::open`; the
//! expected values come from the geometry the design describes (box, hole, sphere), not from
//! SolidCraft's own output.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::PI;

use solidcraft_doc::{Dir, Document, FeatureKind, Model, Operation, UserMaterial, point_expr};
use solidcraft_exchange::{Error, Options, Override, ParameterKind, Snapshot, Surface, Tolerance, from_json, open, to_json};
use solidcraft_geom::Vec3;

/// A `length` × 10 × 5 mm box at the origin, its length the user parameter `L`.
fn box_doc(length: &str) -> Document {
    let mut doc = Document::new("block");
    doc.set_param("L", length, Some("mm"), None).unwrap();
    doc.add_feature(
        FeatureKind::Box {
            corner: point_expr(Vec3::ZERO),
            length: "L".into(),
            width: "10".into(),
            height: "5".into(),
            operation: Operation::NewBody,
        },
        None,
    )
    .unwrap();
    doc
}

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

/// A radius-8 sphere centred at (1, 2, 3).
fn sphere_doc() -> Document {
    let mut doc = Document::new("ball");
    doc.add_feature(FeatureKind::Sphere { center: point_expr(Vec3::new(1.0, 2.0, 3.0)), radius: "8".into(), operation: Operation::NewBody }, None)
        .unwrap();
    doc
}

fn bytes(doc: &Document) -> Vec<u8> {
    doc.to_json().into_bytes()
}

fn snap(doc: &Document) -> Snapshot {
    open(&bytes(doc)).unwrap().snapshot(&[], &Options::default()).unwrap()
}

/// Volume enclosed by a closed mesh (divergence theorem).
fn volume(positions: &[[f64; 3]], tris: impl Iterator<Item = [u32; 3]>) -> f64 {
    tris.map(|[a, b, c]| {
        let (a, b, c) = (positions[a as usize], positions[b as usize], positions[c as usize]);
        a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0])
    })
    .sum::<f64>()
        / 6.0
}

fn body_volume(b: &solidcraft_exchange::Body) -> f64 {
    volume(&b.positions, b.faces.iter().flat_map(|f| f.triangles.iter().copied()))
}

/// Each undirected edge is shared by exactly two triangles, once in each direction.
fn watertight(b: &solidcraft_exchange::Body) -> bool {
    let mut count: BTreeMap<(u32, u32), i32> = BTreeMap::new();
    for [a, b2, c] in b.faces.iter().flat_map(|f| f.triangles.iter().copied()) {
        for (p, q) in [(a, b2), (b2, c), (c, a)] {
            *count.entry((p.min(q), p.max(q))).or_default() += if p < q { 1 } else { 1000 };
        }
    }
    !count.is_empty() && count.values().all(|n| *n == 1001)
}

fn bbox(b: &solidcraft_exchange::Body) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in &b.positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn face_names(s: &Snapshot) -> BTreeSet<String> {
    s.bodies.iter().flat_map(|b| b.faces.iter().map(|f| f.name.clone())).collect()
}

#[test]
fn a_box_is_six_named_outward_planes_closed_with_its_volume() {
    let s = snap(&box_doc("20 mm"));
    assert_eq!((s.schema, s.unit.as_str(), s.design.as_str()), (1, "mm", "block"));
    let [b] = s.bodies.as_slice() else { panic!("one body: {:?}", s.bodies.len()) };
    assert_eq!(b.name, "Body1");
    assert!(b.closed && watertight(b));
    assert_eq!(b.faces.len(), 6);
    let centre = [10.0, 5.0, 2.5];
    for (i, f) in b.faces.iter().enumerate() {
        assert_eq!(f.index as usize, i);
        assert!(!f.triangles.is_empty(), "face {} has triangles", f.name);
        let Surface::Plane { point, normal } = f.surface else { panic!("face {} is {:?}", f.name, f.surface) };
        // An axis direction, pointing away from the centre, and every vertex of the face on it.
        assert!(normal.iter().filter(|x| x.abs() > 1.0 - 1e-12).count() == 1, "{normal:?}");
        let on = f.triangles.iter().flatten().map(|v| b.positions[*v as usize]).collect::<Vec<_>>();
        let first = on[0];
        assert!(dot([first[0] - centre[0], first[1] - centre[1], first[2] - centre[2]], normal) > 0.0, "outward");
        for p in on {
            assert!(dot([p[0] - point[0], p[1] - point[1], p[2] - point[2]], normal).abs() < 1e-9);
        }
    }
    let names = face_names(&s);
    assert_eq!(names.len(), 6, "unique names: {names:?}");
    assert!(names.iter().all(|n| n.starts_with('F')), "{names:?}");
    assert!((body_volume(b) - 1000.0).abs() < 1e-9);
    assert_eq!(bbox(b), ([0.0, 0.0, 0.0], [20.0, 10.0, 5.0]));
}

#[test]
fn a_hole_is_a_cylinder_face_with_its_axis_and_radius() {
    let doc = holed_doc();
    // A design container (`.sc`) opens as the JSON does.
    let container = solidcraft_io::write_design_container(&doc).unwrap();
    let s = open(&container).unwrap().snapshot(&[], &Options::default()).unwrap();
    assert_eq!(to_json(&s).unwrap(), to_json(&snap(&doc)).unwrap());
    let [b] = s.bodies.as_slice() else { panic!("one body") };
    assert!(b.closed && watertight(b));
    let cylinders: Vec<_> = b.faces.iter().filter(|f| matches!(f.surface, Surface::Cylinder { .. })).collect();
    assert!(!cylinders.is_empty());
    for f in &cylinders {
        let Surface::Cylinder { origin, axis, radius } = f.surface else { unreachable!() };
        assert!((radius - 3.0).abs() < 1e-9 && (axis[2].abs() - 1.0).abs() < 1e-12);
        assert!((origin[0] - 10.0).abs() < 1e-9 && (origin[1] - 10.0).abs() < 1e-9, "{origin:?}");
        // Its vertices lie on the cylinder.
        for v in f.triangles.iter().flatten() {
            let p = b.positions[*v as usize];
            assert!(((p[0] - 10.0).hypot(p[1] - 10.0) - 3.0).abs() < 1e-6);
        }
    }
    let planes = b.faces.iter().filter(|f| matches!(f.surface, Surface::Plane { .. })).count();
    assert_eq!(planes, 6);
    // The chord error removes at most (hole area × tolerance) from the hole.
    let exact = 20.0 * 20.0 * 10.0 - PI * 9.0 * 10.0;
    let tol = 2e-4 * (20f64 * 20.0 + 20.0 * 20.0 + 10.0 * 10.0).sqrt();
    let v = body_volume(b);
    assert!(v >= exact - 1e-9 && v - exact <= 2.0 * PI * 3.0 * 10.0 * tol, "{v} vs {exact}");
}

#[test]
fn a_sphere_is_a_sphere_face_with_its_centre_and_radius() {
    let s = snap(&sphere_doc());
    let [b] = s.bodies.as_slice() else { panic!("one body") };
    assert!(b.closed && watertight(b));
    assert!(b.faces.iter().all(|f| matches!(f.surface, Surface::Sphere { .. })), "{:?}", b.faces);
    for f in &b.faces {
        let Surface::Sphere { center, radius } = f.surface else { unreachable!() };
        assert!((radius - 8.0).abs() < 1e-9);
        assert!(dot(sub(center, [1.0, 2.0, 3.0]), sub(center, [1.0, 2.0, 3.0])) < 1e-18);
    }
    for p in &b.positions {
        assert!((dot(sub(*p, [1.0, 2.0, 3.0]), sub(*p, [1.0, 2.0, 3.0])).sqrt() - 8.0).abs() < 1e-6);
    }
    let exact = 4.0 / 3.0 * PI * 512.0;
    let tol = 2e-4 * 16.0 * 3f64.sqrt();
    let v = body_volume(b);
    assert!(v <= exact && exact - v <= 4.0 * PI * 64.0 * tol, "{v} vs {exact}");
    // A finer tolerance comes closer.
    let fine = open(&bytes(&sphere_doc())).unwrap().snapshot(&[], &Options::default().with_tolerance(Tolerance::Absolute(0.001))).unwrap();
    assert!(exact - body_volume(&fine.bodies[0]) < exact - v);
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[test]
fn materials_carry_density_and_card_properties() {
    let mut doc = box_doc("20 mm");
    doc.add_feature(FeatureKind::Sphere { center: point_expr(Vec3::new(50.0, 0.0, 0.0)), radius: "4".into(), operation: Operation::NewBody }, None)
        .unwrap();
    doc.add_feature(FeatureKind::Sphere { center: point_expr(Vec3::new(80.0, 0.0, 0.0)), radius: "4".into(), operation: Operation::NewBody }, None)
        .unwrap();
    let card = serde_json::json!({
        "schema_version": 1,
        "designation": "Test alloy",
        "family": "metal",
        "sources": [{"id": "s1", "title": "Made up for a test", "kind": "illustrative", "license": null, "reference": null}],
        "properties": [
            {"data": {"kind": "elasticity", "model": {"symmetry": "isotropic", "young_modulus_pa": 2.0e11, "poisson_ratio": 0.3}}, "source_id": "s1",
             "conditions": {"temperature_k": 293.15, "moisture_dry_basis_kg_kg": null, "strain_rate_per_s": null, "state": "annealed"}},
            {"data": {"kind": "thermal_conductivity", "w_m_k": [40.0, 41.0, 42.0], "axes": "material123"}, "source_id": "s1"}
        ]
    });
    doc.user_materials.push(UserMaterial {
        name: "Alloy X".into(),
        density: 7.5,
        appearance: None,
        physical_card: Some(solidcraft_doc::physical_material::ValidatedCard::from_value(&card).unwrap()),
    });
    doc.materials.insert("Body1".into(), "steel".into());
    doc.materials.insert("Body2".into(), "Alloy X".into());
    let s = snap(&doc);
    let by_name: BTreeMap<&str, Option<&str>> = s.bodies.iter().map(|b| (b.name.as_str(), b.material.as_deref())).collect();
    assert_eq!(by_name, BTreeMap::from([("Body1", Some("Steel")), ("Body2", Some("Alloy X")), ("Body3", None)]));
    let names: Vec<&str> = s.materials.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["Alloy X", "Steel"]);
    assert!((s.materials[1].density_kg_m3 - 7850.0).abs() < 1e-9 && s.materials[1].properties.is_empty());
    let alloy = &s.materials[0];
    assert!((alloy.density_kg_m3 - 7500.0).abs() < 1e-9);
    let elastic = alloy.properties.iter().find(|p| p.kind == "elasticity").unwrap();
    assert_eq!(elastic.values.get("young_modulus_pa"), Some(&2.0e11));
    assert_eq!(elastic.values.get("poisson_ratio"), Some(&0.3));
    assert_eq!(elastic.values.get("temperature_k"), Some(&293.15));
    assert_eq!(elastic.labels.get("symmetry").map(String::as_str), Some("isotropic"));
    assert_eq!(elastic.labels.get("state").map(String::as_str), Some("annealed"));
    let k = alloy.properties.iter().find(|p| p.kind == "thermal_conductivity").unwrap();
    assert_eq!((k.values.get("w_m_k_1"), k.values.get("w_m_k_3")), (Some(&40.0), Some(&42.0)));
    // A body filter keeps only the bodies, and the materials, asked for.
    let one = open(&bytes(&doc)).unwrap().snapshot(&[], &Options::default().with_bodies(vec!["Body1".into()])).unwrap();
    assert_eq!(one.bodies.len(), 1);
    assert_eq!(one.materials.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Steel"]);
}

#[test]
fn an_override_changes_the_dimension_and_keeps_the_face_names() {
    let design = open(&bytes(&box_doc("20 mm"))).unwrap();
    let l = design.parameters().into_iter().find(|p| p.name == "L").unwrap();
    assert_eq!((l.value, l.unit.as_str(), l.kind), (Some(20.0), "mm", ParameterKind::Length));
    let before = design.snapshot(&[], &Options::default()).unwrap();
    let after = design.snapshot(&[("L".into(), Override::Value(30.0))], &Options::default()).unwrap();
    let as_expression = design.snapshot(&[("L".into(), Override::from("3 cm"))], &Options::default()).unwrap();
    assert_eq!(after.bodies, as_expression.bodies);
    assert_eq!(as_expression.parameters.iter().find(|p| p.name == "L").map(|p| (p.expression.as_str(), p.value)), Some(("3 cm", Some(30.0))));
    assert!((body_volume(&before.bodies[0]) - 1000.0).abs() < 1e-9);
    assert!((body_volume(&after.bodies[0]) - 1500.0).abs() < 1e-9);
    assert_eq!(bbox(&after.bodies[0]).1, [30.0, 10.0, 5.0]);
    assert_eq!(face_names(&before), face_names(&after));
    let face_kinds = |s: &Snapshot| s.bodies[0].faces.iter().map(|f| (f.name.clone(), f.surface.clone())).collect::<Vec<_>>();
    // The same face keeps its name and its plane's direction.
    for ((n0, s0), (n1, s1)) in face_kinds(&before).into_iter().zip(face_kinds(&after)) {
        assert_eq!(n0, n1);
        let (Surface::Plane { normal: a, .. }, Surface::Plane { normal: b, .. }) = (s0, s1) else { panic!() };
        assert_eq!(a, b);
    }
    let reported = after.parameters.iter().find(|p| p.name == "L").unwrap();
    assert_eq!(reported.value, Some(30.0));
    // The design itself is unchanged, and going back gives the first snapshot again.
    assert_eq!(design.parameters().into_iter().find(|p| p.name == "L").unwrap().value, Some(20.0));
    assert_eq!(to_json(&design.snapshot(&[], &Options::default()).unwrap()).unwrap(), to_json(&before).unwrap());
}

#[test]
fn snapshots_are_byte_identical() {
    let doc = holed_doc();
    let a = to_json(&snap(&doc)).unwrap();
    let b = to_json(&snap(&doc)).unwrap();
    assert_eq!(a, b);
    let design = open(&bytes(&doc)).unwrap();
    let c = to_json(&design.snapshot(&[], &Options::default()).unwrap()).unwrap();
    let d = to_json(&design.snapshot(&[], &Options::default()).unwrap()).unwrap();
    assert_eq!((&a, &a), (&c, &d));
}

/// The `ft:face` of every triangle, per object, read from the package's model part.
fn face_tags(package: &[u8]) -> Vec<Vec<u32>> {
    use std::io::Read as _;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(package)).unwrap();
    let mut xml = String::new();
    zip.by_name("3D/3dmodel.model").unwrap().read_to_string(&mut xml).unwrap();
    xml.split("<object ")
        .skip(1)
        .map(|object| {
            object
                .split("<triangle ")
                .skip(1)
                .map(|t| {
                    let rest = &t[t.find("ft:face=\"").unwrap() + 9..];
                    rest[..rest.find('"').unwrap()].parse().unwrap()
                })
                .collect()
        })
        .collect()
}

#[test]
fn the_tagged_3mf_export_and_the_snapshot_agree_on_faces() {
    for doc in [box_doc("20 mm"), holed_doc(), sphere_doc()] {
        let s = snap(&doc);
        let mut model = Model::new();
        model.evaluate(&doc);
        let world = solidcraft_doc::world_state(&doc, &model.state());
        let bytes = solidcraft_io::export(&world, &[], solidcraft_io::Format::ThreeMfTagged, "x").unwrap();
        let (objects, _) = solidcraft_io::read_3mf(&bytes).unwrap();
        let tagged = face_tags(&bytes);
        assert_eq!(objects.len(), s.bodies.len());
        for (o, b) in objects.iter().zip(&s.bodies) {
            assert_eq!(o.name, b.name);
            assert!(o.tri_faces.iter().all(|f| (*f as usize) < b.faces.len()));
            let positions: Vec<[f64; 3]> = o.positions.iter().map(|p| [p.x, p.y, p.z]).collect();
            assert_eq!(positions, b.positions);
            let tags = &tagged[objects.iter().position(|x| x.name == o.name).unwrap()];
            assert_eq!(tags.len(), o.triangles.len());
            let mut theirs: Vec<(u32, [u32; 3])> = o.triangles.iter().zip(tags).map(|(t, f)| (*f, *t)).collect();
            let mut ours: Vec<(u32, [u32; 3])> = b.faces.iter().flat_map(|f| f.triangles.iter().map(|t| (f.index, *t))).collect();
            theirs.sort_unstable();
            ours.sort_unstable();
            assert_eq!(theirs, ours);
        }
    }
}

#[test]
fn json_reads_back_and_tolerates_what_it_does_not_know() {
    let s = snap(&holed_doc());
    let text = to_json(&s).unwrap();
    assert_eq!(from_json(&text).unwrap(), s);
    // An added field and an unknown surface kind read; a missing field is its default.
    let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["future_field"] = serde_json::json!({"x": 1});
    v["bodies"][0]["faces"][0]["surface"] = serde_json::json!({"type": "subdivision", "level": 3});
    v["bodies"][0]["faces"][0]["also_new"] = serde_json::json!(true);
    v["parameters"] = serde_json::json!([{"name": "p", "kind": "complex"}]);
    v["bodies"][0].as_object_mut().unwrap().remove("attributes");
    let read = from_json(&v.to_string()).unwrap();
    assert_eq!(read.bodies[0].faces[0].surface, Surface::Other);
    assert_eq!(read.parameters[0].kind, ParameterKind::Other);
    assert!(read.bodies[0].attributes.is_empty());
    // A later, breaking schema is refused.
    v["schema"] = serde_json::json!(2);
    assert!(matches!(from_json(&v.to_string()), Err(Error::NotSupported(_))));
}

#[test]
fn hostile_input_is_refused_without_panicking() {
    assert!(matches!(open(b"not a design"), Err(Error::Read(_))));
    assert!(matches!(open(&[0xff, 0xfe, 0x00]), Err(Error::Read(_))));
    let mut doc = box_doc("20 mm");
    doc.text_params.push(solidcraft_doc::text_params::TextParam { name: "label".into(), text: "A".into(), comment: String::new() });
    let design = open(&bytes(&doc)).unwrap();
    let o = Options::default();
    for bad in [f64::NAN, f64::INFINITY, -f64::INFINITY, 1e300] {
        assert!(matches!(design.snapshot(&[("L".into(), Override::Value(bad))], &o), Err(Error::Parameter(_))), "{bad}");
    }
    assert!(matches!(design.snapshot(&[("nope".into(), Override::Value(1.0))], &o), Err(Error::Parameter(_))));
    assert!(matches!(design.snapshot(&[("label".into(), Override::Value(1.0))], &o), Err(Error::Parameter(_))));
    assert!(matches!(design.snapshot(&[("L".into(), Override::from("L * 2"))], &o), Err(Error::Parameter(_))), "circular");
    assert!(matches!(design.snapshot(&[("L".into(), Override::from("1 +"))], &o), Err(Error::Parameter(_))));
    assert!(matches!(design.snapshot(&[("L".into(), Override::from("\u{0}"))], &o), Err(Error::Parameter(_))));
    assert!(matches!(design.snapshot(&[("L".into(), Override::from("1".repeat(5000).as_str()))], &o), Err(Error::Parameter(_))));
    assert!(matches!(design.snapshot(&[("L".into(), 1.0.into()), ("L".into(), 2.0.into())], &o), Err(Error::Parameter(_))), "duplicate");
    let many: Vec<(String, Override)> = (0..2000).map(|_| ("L".to_string(), Override::Value(1.0))).collect();
    assert!(matches!(design.snapshot(&many, &o), Err(Error::Limit(_))));
    for t in [Tolerance::Relative(0.0), Tolerance::Relative(f64::NAN), Tolerance::Relative(5.0), Tolerance::Absolute(-1.0), Tolerance::Absolute(1e-9)]
    {
        assert!(matches!(design.snapshot(&[], &Options::default().with_tolerance(t)), Err(Error::Invalid(_))), "{t:?}");
    }
    assert!(matches!(design.snapshot(&[], &Options::default().with_bodies(vec!["Body9".into()])), Err(Error::Invalid(_))));
    // A value the feature cannot take fails the evaluation, naming the feature; the design
    // still snapshots at its own values afterwards.
    match design.snapshot(&[("L".into(), Override::Value(0.0))], &o) {
        Err(Error::Evaluation(failed)) => assert!(!failed.is_empty()),
        Err(Error::Parameter(_)) => {}
        other => panic!("a zero-length box: {other:?}"),
    }
    assert_eq!(design.snapshot(&[], &o).unwrap().bodies.len(), 1);
    // A text parameter takes a text.
    assert!(design.snapshot(&[("label".into(), Override::from("\"B\""))], &o).is_ok());
}

//! Independent analytic checks for the fixed-tolerance verification mesh.
#![allow(clippy::unwrap_used)]
use super::*;

// Run the original candidate-index differential tests in ordinary workspace CI;
// the vendored crate's standalone test targets are intentionally disabled.
#[path = "../../../vendor/truck-meshalgo/src/tessellation/boundary_index.rs"]
mod boundary_index;

fn radial_error(mesh: &Mesh, radius: f64, center: Vec3) -> f64 {
    let mut error = 0_f64;
    for indices in &mesh.triangles {
        let points = mesh.tri(indices).unwrap();
        // The cylinder side spans both cap heights. Check every side edge,
        // including triangle diagonals, against the analytic radial distance.
        if points.iter().any(|p| p.z < center.z + 1.) && points.iter().any(|p| p.z > center.z + 5.) {
            for i in 0..3 {
                let midpoint = (points[i] + points[(i + 1) % 3]) * 0.5 - center;
                error = error.max((radius - midpoint.x.hypot(midpoint.y)).abs());
            }
        }
    }
    error
}

#[test]
fn verification_tessellation_keeps_physical_tolerance_for_large_curved_bodies() {
    let tolerance = 0.00125;
    let radius = 5000.;
    let center = Vec3::new(20_000., -10_000., 3.);
    let body = crate::cylinder(center, Vec3::Z, radius, 6.).unwrap();
    let display = body.tessellate(tolerance).unwrap();
    assert!(radial_error(&display, radius, center) > tolerance * 4.);
    let mesh = body.tessellate_for_verification(tolerance, 200_000).unwrap();
    assert!(mesh.triangles.len() > display.triangles.len());
    let volume = mesh.measure().volume;
    assert!(volume >= std::f64::consts::PI * (radius - tolerance).powi(2) * 6.);
    assert!(volume <= std::f64::consts::PI * (radius + tolerance).powi(2) * 6.);
    assert!(radial_error(&mesh, radius, center) <= tolerance, "{}", radial_error(&mesh, radius, center));
    assert_eq!(mesh.tri_face.iter().copied().collect::<std::collections::BTreeSet<_>>().len(), body.face_count());
}

#[test]
fn verification_tessellation_rejects_bad_settings_and_excessive_output() {
    let body = crate::box_solid(Vec3::ZERO, Vec3::new(10., 10., 6.)).unwrap();
    for tolerance in [0., -1., f64::NAN, f64::INFINITY] {
        assert!(body.tessellate_for_verification(tolerance, 100).is_err());
    }
    for limit in [0, 2_000_001] {
        assert!(body.tessellate_for_verification(0.00125, limit).is_err());
    }
    assert!(body.tessellate_for_verification(0.00125, 11).unwrap_err().to_string().contains("triangle budget"));
    let mesh = body.tessellate_for_verification(0.00125, 12).unwrap();
    assert_eq!(mesh.triangles.len(), 12);
    let imported = crate::mesh_body(&mesh.positions, &mesh.triangles).unwrap();
    assert!(imported.tessellate_for_verification(0.00125, 11).is_err());
    assert_eq!(imported.tessellate_for_verification(0.00125, 12).unwrap().triangles.len(), 12);
}

#[test]
fn verification_tessellation_does_not_accept_refinement_budget_fallback() {
    let body = crate::cylinder(Vec3::ZERO, Vec3::Z, 5000., 6.).unwrap();
    let error = body.tessellate_for_verification(1e-10, 200_000).unwrap_err();
    assert!(error.to_string().contains("refinement budget"), "{error}");
    // Curved in both parameters: the surface grid exhausts its budget before
    // any circular boundary needs the edge's 20,000-point limit.
    let sphere = crate::sphere(Vec3::ZERO, 10.).unwrap();
    let error = sphere.tessellate_for_verification(1e-5, 200_000).unwrap_err();
    assert!(error.to_string().contains("refinement budget"), "{error}");
}

#[test]
fn verification_tessellation_preserves_concave_material_and_multiple_holes() {
    use solidcraft_geom::{Loop2, Plane, Region2, Vec2};
    let polygon = |points: &[(f64, f64)]| Loop2::polygon(&points.iter().map(|&(x, y)| Vec2::new(x, y)).collect::<Vec<_>>());
    let region = Region2 {
        outer: polygon(&[(0., 0.), (20., 0.), (20., 10.), (10., 10.), (10., 20.), (0., 20.)]),
        holes: vec![polygon(&[(2., 2.), (2., 4.), (4., 4.), (4., 2.)]), polygon(&[(12., 2.), (12., 4.), (14., 4.), (14., 2.)])],
    };
    let bodies = crate::extrude(&Plane::XY, &[region], 0., 6.).unwrap();
    assert_eq!(bodies.len(), 1);
    let mesh = bodies.first().unwrap().tessellate_for_verification(0.00125, 200_000).unwrap();
    assert!((mesh.measure().volume - 1752.).abs() < 1e-6);
    for (point, material) in [
        (Vec3::new(1., 1., 3.), true),
        (Vec3::new(19., 9., 3.), true),
        (Vec3::new(9., 19., 3.), true),
        (Vec3::new(3., 3., 3.), false),
        (Vec3::new(13., 3., 3.), false),
        (Vec3::new(15., 15., 3.), false),
    ] {
        assert_eq!(mesh.contains(point), material, "{point:?}");
    }
}

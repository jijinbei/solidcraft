//! Independent direction checks for a right-handed cutter with M3 spindle rotation.
//! A climb cut keeps the material being finished to the right of tool travel.
use solidcraft_cam::{Boundary, JobInput, Strategy, corpus, generate, nc};

fn segments(input: &JobInput) -> Vec<nc::Segment> {
    let path = generate::generate(input.clone()).unwrap();
    let parsed = nc::round_trip(&path, &input.setup, &input.tools).unwrap();
    nc::segments(&parsed, &input.setup).unwrap()
}

fn square(lo: f64, hi: f64, reversed: bool) -> Vec<[f64; 2]> {
    let mut ring = vec![[lo, lo], [hi, lo], [hi, hi], [lo, hi]];
    if reversed {
        ring.reverse();
    }
    ring
}

// Collect continuous constant-depth cuts from the independently interpreted NC.
// Signed polygon area checks orientation without calling the offset adapter.
fn loops(input: &JobInput) -> Vec<Vec<nc::Segment>> {
    let mut result = Vec::new();
    let mut current = Vec::new();
    for s in segments(input) {
        if !s.rapid && (s.from.z - 5.).abs() < 1e-8 && (s.to.z - 5.).abs() < 1e-8 {
            current.push(s);
        } else if !current.is_empty() {
            result.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

fn ccw(path: &[nc::Segment]) -> bool {
    let origin = path[0].from;
    assert!((path.last().unwrap().to - origin).len() < 1e-5, "cut must close");
    let area = path
        .iter()
        .map(|s| {
            let a = s.from - origin;
            let b = s.to - origin;
            a.x * b.y - b.x * a.y
        })
        .sum::<f64>();
    assert!(area.abs() > 1.);
    area > 0.
}

#[test]
fn facing_direction_matches_uncut_side_at_every_depth() {
    for climb in [false, true] {
        let mut input = corpus::fixture("face").unwrap();
        input.operations[0].climb = climb;
        input.operations[0].stepdown = 0.5;
        let cuts: Vec<_> = segments(&input).into_iter().filter(|s| !s.rapid && (s.from.z - s.to.z).abs() < 1e-8 && s.to.z < 6.).collect();
        assert_eq!(cuts.len(), 22);
        for depth in cuts.as_chunks::<11>().0 {
            assert!(depth.windows(2).all(|pair| pair[1].from.y > pair[0].from.y));
            // Advancing rows toward +Y leaves uncut material north of the tool.
            // Right of a -X move is +Y; right of a +X move is -Y.
            assert!(depth.iter().all(|s| (s.to.x < s.from.x) == climb));
        }
    }
}

#[test]
fn contour_direction_matches_inside_and_outside_material() {
    for reversed in [false, true] {
        for inside in [false, true] {
            for climb in [false, true] {
                let mut input = corpus::fixture("face").unwrap();
                input.tools[0].diameter = 2.;
                input.operations[0].climb = climb;
                input.operations[0].geometry = Strategy::Contour { boundary: Boundary { outer: square(4., 16., reversed), islands: vec![] }, inside };
                let paths = loops(&input);
                assert_eq!(paths.len(), 1);
                // Pocket wall: material outside => CCW climb. Boss: inside => CW climb.
                assert_eq!(ccw(&paths[0]), inside == climb, "inside={inside}, climb={climb}, reversed={reversed}");
            }
        }
    }
}

#[test]
fn pocket_direction_preserves_opposite_outer_and_island_winding() {
    for reversed in [false, true] {
        for climb in [false, true] {
            let mut input = corpus::fixture("face").unwrap();
            input.tools[0].diameter = 2.;
            input.operations[0].climb = climb;
            input.operations[0].geometry =
                Strategy::Pocket { boundary: Boundary { outer: square(0., 20., reversed), islands: vec![square(4., 16., reversed)] } };
            let paths = loops(&input);
            assert_eq!(paths.len(), 2);
            let mut outer = 0;
            let mut island = 0;
            for path in &paths {
                let is_outer = path.iter().any(|s| s.from.x < 2.);
                if is_outer {
                    outer += 1;
                } else {
                    island += 1;
                }
                assert_eq!(ccw(path), is_outer == climb, "outer={is_outer}, climb={climb}, reversed={reversed}");
            }
            assert_eq!((outer, island), (1, 1));
        }
    }
}

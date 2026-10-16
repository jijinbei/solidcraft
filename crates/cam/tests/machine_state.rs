//! Hand-authored modal programs exercise NIST machine state independently of generation.
use solidcraft_cam::{Motion, corpus, generate, nc};

const HEADER: &str = "G21 G90 G17 G94 G54\nG0 X0 Y0 Z15\nT1 M6\nS6000 M3\nM8\n";

#[test]
fn tool_change_stops_spindle_preserves_coolant_and_requires_restart() {
    let mut input = corpus::fixture("face").unwrap();
    let mut second = input.tools[0].clone();
    second.id = 2;
    second.number = 2;
    input.tools.push(second);
    for number in [1, 2] {
        let source = format!("{HEADER}T{number} M6\nS7000\nG1 X1 F100\nM30");
        let parsed = nc::parse(&source, &input.tools).unwrap();
        let change: Vec<_> = parsed.records.iter().filter(|r| r.line == 6).map(|r| &r.motion).collect();
        assert_eq!(change, [&Motion::Spindle { rpm: 0. }, &Motion::Tool { tool: number as u64, number }]);
        assert!(parsed.records.iter().all(|r| r.line != 7), "S alone cannot restart after M6");
        assert_eq!(parsed.records.last().unwrap().motion, Motion::Coolant { on: false }, "M6 must preserve coolant until M30");
        assert!(nc::segments(&parsed, &input.setup).unwrap_err().0.contains("cutting without tool/spindle at line 8"));

        let restarted = source.replace("S7000\n", "M3\n");
        let parsed = nc::parse(&restarted, &input.tools).unwrap();
        assert_eq!(parsed.records.iter().find(|r| r.line == 7).unwrap().motion, Motion::Spindle { rpm: 6000. });
        let segments = nc::segments(&parsed, &input.setup).unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].tool, number as u64);
        assert_eq!(segments[0].to.x, 1.);
    }
}

#[test]
fn same_block_speed_and_coolant_commands_keep_both_effects() {
    let input = corpus::fixture("face").unwrap();
    for coolant in ["M8", "M9", "M0"] {
        let source = format!("{HEADER}S0 {coolant}\nG1 X1 F100\nM30");
        let parsed = nc::parse(&source, &input.tools).unwrap();
        assert_eq!(parsed.records.iter().find(|r| r.line == 6).unwrap().motion, Motion::Spindle { rpm: 0. });
        assert!(nc::segments(&parsed, &input.setup).unwrap_err().0.contains("cutting without tool/spindle at line 7"));
        let source = source.replace("S0", "S12000");
        let parsed = nc::parse(&source, &input.tools).unwrap();
        assert_eq!(parsed.records.iter().find(|r| r.line == 6).unwrap().motion, Motion::Spindle { rpm: 12000. });
        assert_eq!(nc::segments(&parsed, &input.setup).unwrap().len(), 1);
    }
    let parsed = nc::parse(&format!("{HEADER}S0 G1 X1 F100\nM30"), &input.tools).unwrap();
    assert_eq!(parsed.records.iter().find(|r| r.line == 6).unwrap().motion, Motion::Spindle { rpm: 0. });
    assert!(nc::segments(&parsed, &input.setup).unwrap_err().0.contains("cutting without tool/spindle at line 6"));
}

#[test]
fn reference_post_initializes_state_before_pose_and_keeps_block_provenance() {
    let input = corpus::fixture("face").unwrap();
    let path = generate::generate(input.clone()).unwrap();
    let code = nc::post(&path, &input.setup).unwrap();
    let lines: Vec<_> = code.lines().filter(|l| !l.starts_with('(')).collect();
    assert_eq!(lines[0], "G21 G90 G17 G91.1 G94 G54 G40 G49 G80");
    assert_eq!(&lines[1..3], ["M5", "M9"]);
    assert!(lines[3].starts_with("G0 X0.000000 Y0.000000 Z15.000000"));
    let parsed = nc::round_trip(&path, &input.setup, &input.tools).unwrap();
    assert_eq!(parsed.records.len(), path.blocks.len() + 3);
    for record in &parsed.records[..3] {
        assert!(parsed.source_for_line(record.line).is_none());
    }
    for (i, record) in parsed.records[3..].iter().enumerate() {
        let source = parsed.source_for_line(record.line).unwrap();
        assert_eq!(source.block, i + 1);
        assert_eq!(source.operation, path.blocks[i].operation);
    }
}

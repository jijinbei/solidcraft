//! Original programs exercise cooperative NC processing and its exact boundary limits.
use solidcraft_cam::{self as cam, Block, Motion, Toolpath, nc};
use solidcraft_geom::Vec3;
use std::{
    cell::Cell,
    future::Future,
    task::{Context, Poll, Waker},
};

fn yield_once() -> impl Future<Output = ()> {
    let mut first = true;
    std::future::poll_fn(move |_| if std::mem::take(&mut first) { Poll::Pending } else { Poll::Ready(()) })
}
fn drive<T>(future: impl Future<Output = cam::Result<T>>) -> (cam::Result<T>, usize) {
    let mut future = std::pin::pin!(future);
    let mut polls = 0;
    loop {
        polls += 1;
        assert!(polls < 20_000, "unbounded checkpoint count");
        if let Poll::Ready(result) = future.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            return (result, polls);
        }
    }
}
fn path(count: usize) -> Toolpath {
    let mut blocks = vec![
        Block { operation: 1, motion: Motion::Tool { tool: 1, number: 1 } },
        Block { operation: 1, motion: Motion::Spindle { rpm: 6000. } },
        Block { operation: 1, motion: Motion::Coolant { on: true } },
    ];
    for i in 0..count - 6 {
        blocks.push(Block {
            operation: if i < count / 2 { 1 } else { 2 },
            motion: Motion::Linear { to: Vec3::new(if i % 2 == 0 { 1. } else { 19. }, 10., 5.), feed: 100. },
        });
    }
    blocks.extend([
        Block { operation: 2, motion: Motion::Coolant { on: false } },
        Block { operation: 2, motion: Motion::Spindle { rpm: 0. } },
        Block { operation: 2, motion: Motion::Stop },
    ]);
    Toolpath { fingerprint: "original-independent-test".into(), blocks }
}
#[test]
fn maximum_path_includes_startup_records_and_excess_is_rejected() {
    let input = cam::corpus::fixture("face").unwrap();
    let mut path = path(cam::MAX_MOVES);
    let (parsed, polls) = drive(nc::round_trip_cooperative(&path, &input.setup, &input.tools, || false, yield_once));
    let parsed = parsed.unwrap();
    assert_eq!(parsed.records.len(), cam::MAX_MOVES + 3);
    assert!(polls > 3000);
    let last = parsed.records.last().unwrap();
    let source = parsed.source_for_line(last.line).unwrap();
    assert_eq!((source.block, source.operation), (cam::MAX_MOVES, 2));
    path.blocks.push(path.blocks.last().unwrap().clone());
    assert!(nc::post(&path, &input.setup).is_err());
    // External NC records remain bounded independently of the posting limit.
    let mut external = "G21 G90 G17 G94 G54\nG0 X0 Y0 Z15\n".to_owned();
    external.push_str(&"G0 Z15\n".repeat(cam::MAX_MOVES + 3));
    external.push_str("M30\n");
    assert!(nc::parse(&external, &input.tools).unwrap_err().to_string().contains("NC motion limit"));
}
#[test]
fn synchronous_and_cooperative_processing_keep_bytes_state_and_provenance() {
    let input = cam::corpus::fixture("face").unwrap();
    let path = path(1024);
    let source = nc::post(&path, &input.setup).unwrap();
    assert!(source.starts_with("(solidcraft-reference-3axis/2; not a qualified machine post)\n(Fingerprint original-independent-test)\nG21 G90 G17 G91.1 G94 G54 G40 G49 G80\nM5\nM9\nG0 X0.000000 Y0.000000 Z15.000000\n(Operation 1)\nT1 M6\n"));
    assert_eq!(drive(nc::post_cooperative(&path, &input.setup, || false, yield_once)).0.unwrap(), source);
    let synchronous = nc::round_trip(&path, &input.setup, &input.tools).unwrap();
    let cooperative = drive(nc::round_trip_cooperative(&path, &input.setup, &input.tools, || false, yield_once)).0.unwrap();
    assert_eq!(serde_json::to_value(&synchronous).unwrap(), serde_json::to_value(&cooperative).unwrap());
    for record in &synchronous.records {
        assert_eq!(synchronous.source_for_line(record.line), cooperative.source_for_line(record.line));
    }
    let broken = source.replace("(Operation 2)", "G123");
    let ordinary = nc::parse(&broken, &input.tools).unwrap_err();
    let incremental = drive(nc::parse_cooperative(&broken, &input.tools, || false, yield_once)).0.unwrap_err();
    assert_eq!(ordinary.to_string(), incremental.to_string());
    assert!(incremental.to_string().contains("unsupported G code"));
}
#[test]
fn cancellation_during_each_stage_returns_no_partial_program_or_line_mapping() {
    let input = cam::corpus::fixture("face").unwrap();
    let path = path(1024);
    let count = Cell::new(0);
    let full = drive(nc::round_trip_cooperative(
        &path,
        &input.setup,
        &input.tools,
        || false,
        || {
            count.set(count.get() + 1);
            yield_once()
        },
    ))
    .0
    .unwrap();
    assert_eq!(full.records.len(), 1027);
    // Every checkpoint, including post, reader and correspondence stages.
    for stop in 0..=count.get() {
        let at = Cell::new(0);
        let result = drive(nc::round_trip_cooperative(
            &path,
            &input.setup,
            &input.tools,
            || at.get() >= stop,
            || {
                at.set(at.get() + 1);
                yield_once()
            },
        ))
        .0;
        assert!(result.unwrap_err().to_string().contains("cancelled"), "checkpoint {stop}");
    }
}

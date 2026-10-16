//! Strict reference-dialect post and independent modal NC reader.
//! The reader has no dependency on the toolpath generator.
use crate::*;
use serde::{Deserialize, Serialize};
use solidcraft_geom::Vec3;
use std::{
    future::Future,
    task::{Context, Poll, Waker},
};

pub const DIALECT: &str = "solidcraft-reference-3axis/2; not a qualified machine post";
// A maximum-size path also emits the three explicit startup events below.
const MAX_RECORDS: usize = MAX_MOVES + 3;
fn complete<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(result) => result,
        Poll::Pending => invalid("synchronous NC checkpoint unexpectedly suspended"),
    }
}
async fn checkpoint<F: Future<Output = ()>>(cancelled: &impl Fn() -> bool, yield_control: &mut impl FnMut() -> F) -> Result<()> {
    if cancelled() {
        return invalid("NC processing cancelled");
    }
    yield_control().await;
    if cancelled() { invalid("NC processing cancelled") } else { Ok(()) }
}
fn startup(setup: &Setup) -> [Motion; 3] {
    [Motion::Spindle { rpm: 0.0 }, Motion::Coolant { on: false }, Motion::Rapid { to: Vec3::new(0., 0., setup.clearance) }]
}
pub fn post(path: &Toolpath, setup: &Setup) -> Result<String> {
    complete(post_cooperative(path, setup, || false, || std::future::ready(())))
}
/// Preserve post bytes while yielding every 256 blocks. No partial output on cancellation.
pub async fn post_cooperative<F: Future<Output = ()>>(
    path: &Toolpath,
    setup: &Setup,
    cancelled: impl Fn() -> bool,
    mut yield_control: impl FnMut() -> F,
) -> Result<String> {
    checkpoint(&cancelled, &mut yield_control).await?;
    if path.blocks.is_empty() || path.blocks.len() > MAX_MOVES {
        return invalid("cannot post an empty or excessive path");
    }
    let mut out = format!("({DIALECT})\n(Fingerprint {})\nG21 G90 G17 G91.1 G94 G{} G40 G49 G80\nM5\nM9\n", path.fingerprint, setup.work_offset);
    let mut current = Vec3::new(0.0, 0.0, setup.clearance);
    out.push_str(&format!("G0 X0.000000 Y0.000000 Z{:.6}\n", setup.clearance));
    for (index, block) in path.blocks.iter().enumerate() {
        if index.is_multiple_of(256) {
            checkpoint(&cancelled, &mut yield_control).await?;
        }
        out.push_str(&format!("(Operation {})\n", block.operation));
        let line = match &block.motion {
            Motion::Rapid { to } => format!("G0 X{:.6} Y{:.6} Z{:.6}", to.x, to.y, to.z),
            Motion::Linear { to, feed } => format!("G1 X{:.6} Y{:.6} Z{:.6} F{:.6}", to.x, to.y, to.z, feed),
            Motion::Arc { to, center, clockwise, feed } => format!(
                "G{} X{:.6} Y{:.6} Z{:.6} I{:.6} J{:.6} F{:.6}",
                if *clockwise { 2 } else { 3 },
                to.x,
                to.y,
                to.z,
                center.x - current.x,
                center.y - current.y,
                feed
            ),
            Motion::Tool { number, .. } => format!("T{number} M6"),
            Motion::Spindle { rpm } => {
                if *rpm == 0.0 {
                    "M5".into()
                } else {
                    format!("S{rpm:.6} M3")
                }
            }
            Motion::Coolant { on } => {
                if *on {
                    "M8".into()
                } else {
                    "M9".into()
                }
            }
            Motion::Stop => "M0".into(),
        };
        out.push_str(&line);
        out.push('\n');
        if let Motion::Rapid { to } | Motion::Linear { to, .. } | Motion::Arc { to, .. } = &block.motion {
            current = *to;
        }
    }
    out.push_str("M30\n");
    checkpoint(&cancelled, &mut yield_control).await?;
    Ok(out)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub line: usize,
    pub motion: Motion,
}
/// Provenance established only after the independent reader matches a posted block.
/// `block` is one-based and also the completed-block count used by playback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocation {
    pub line: usize,
    pub block: usize,
    pub operation: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Parsed {
    pub records: Vec<Record>,
    pub work_offset: u8,
    // Never accept provenance from comments or deserialized external programs.
    #[serde(skip)]
    sources: Vec<SourceLocation>,
}
impl Parsed {
    pub fn source_for_line(&self, line: usize) -> Option<SourceLocation> {
        self.sources.binary_search_by_key(&line, |s| s.line).ok().and_then(|i| self.sources.get(i).copied())
    }
    /// First interpreted record on a physical NC line, using a zero-based index.
    pub fn record_for_line(&self, line: usize) -> Option<usize> {
        let i = self.records.partition_point(|r| r.line < line);
        self.records.get(i).filter(|r| r.line == line).map(|_| i)
    }
}

/// Strictly consumes every word. Units and distance mode must be declared before motion.
pub fn parse(source: &str, tools: &[Tool]) -> Result<Parsed> {
    complete(parse_cooperative(source, tools, || false, || std::future::ready(())))
}
/// Independent modal reader with cancellation and a checkpoint every 256 physical lines.
pub async fn parse_cooperative<F: Future<Output = ()>>(
    source: &str,
    tools: &[Tool],
    cancelled: impl Fn() -> bool,
    mut yield_control: impl FnMut() -> F,
) -> Result<Parsed> {
    checkpoint(&cancelled, &mut yield_control).await?;
    if source.len() > 32 * 1024 * 1024 {
        return invalid("NC file too large");
    }
    let mut out = Vec::new();
    let mut units = None;
    let mut absolute = None;
    let mut plane = false;
    let mut per_minute = false;
    let mut offset = None;
    let mut pos = Vec3::ZERO;
    let mut have_pos = false;
    let mut mode = None;
    let mut feed = 0.0;
    let mut rpm = 0.0;
    let mut spindle_on = false;
    let mut coolant_on = false;
    let mut selected = None;
    let mut ended = false;
    for (index, line) in source.lines().enumerate() {
        if index.is_multiple_of(256) {
            checkpoint(&cancelled, &mut yield_control).await?;
        }
        let line_number = index + 1;
        let fail = |s: &str| Error(format!("NC line {line_number}: {s}"));
        let mut words = words(line).map_err(|e| fail(&e.0))?;
        // Modal changes apply to the whole block, independent of word order.
        words.sort_by_key(|(letter, _)| if *letter == 'G' { 0 } else { 1 });
        if words.is_empty() {
            continue;
        }
        if ended {
            return Err(fail("content after program end"));
        }
        let mut coordinates = [None; 3];
        let mut ij = [None; 2];
        let mut m = None;
        let mut groups = std::collections::BTreeSet::new();
        let mut seen = std::collections::BTreeSet::new();
        for (letter, value) in words {
            if letter != 'G' && !seen.insert(letter) {
                return Err(fail("duplicate address"));
            }
            match letter {
                'N' => {
                    if value.fract() != 0.0 || value < 0.0 {
                        return Err(fail("invalid line number"));
                    }
                }
                'G' => {
                    let group = match value {
                        0.0..=3.0 if value.fract() == 0.0 => {
                            mode = Some(value as u8);
                            0
                        }
                        17.0 => {
                            plane = true;
                            1
                        }
                        20.0 => {
                            units = Some(25.4);
                            2
                        }
                        21.0 => {
                            units = Some(1.0);
                            2
                        }
                        90.0 => {
                            absolute = Some(true);
                            3
                        }
                        91.0 => {
                            absolute = Some(false);
                            3
                        }
                        94.0 => {
                            per_minute = true;
                            4
                        }
                        54.0..=59.0 if value.fract() == 0.0 => {
                            if offset.is_some_and(|o| o != value as u8) {
                                return Err(fail("multiple work offsets unsupported"));
                            }
                            offset = Some(value as u8);
                            5
                        }
                        40.0 => 6,
                        49.0 => 7,
                        80.0 => {
                            mode = None;
                            0
                        }
                        91.1 => 9,
                        _ => return Err(fail("unsupported G code; no commands are silently discarded")),
                    };
                    if !groups.insert(group) {
                        return Err(fail("conflicting modal codes"));
                    }
                }
                'M' => {
                    if value.fract() != 0.0 {
                        return Err(fail("invalid M code"));
                    }
                    m = Some(value as i32);
                }
                'X' => coordinates[0] = Some(value),
                'Y' => coordinates[1] = Some(value),
                'Z' => coordinates[2] = Some(value),
                'I' => ij[0] = Some(value),
                'J' => ij[1] = Some(value),
                'F' => {
                    let scale = units.ok_or_else(|| fail("declare units before feed"))?;
                    feed = value * scale;
                    if !crate::model::positive(feed) {
                        return Err(fail("feed must be positive"));
                    }
                }
                'S' => {
                    if value < 0.0 {
                        return Err(fail("negative spindle speed"));
                    }
                    rpm = value;
                }
                'T' => {
                    if value.fract() != 0.0 || value < 1.0 || value > 9999.0 {
                        return Err(fail("invalid tool number"));
                    }
                    selected = Some(value as u32);
                }
                _ => return Err(fail("unsupported address")),
            }
        }
        let moving = coordinates.iter().any(Option::is_some) || ij.iter().any(Option::is_some);
        if moving && m.is_some() {
            return Err(fail("combined motion/M blocks unsupported by reference dialect"));
        }
        // Speed changes precede motion and coolant/stop commands in a block.
        // Other spindle-changing M codes below already emit its final state.
        if seen.contains(&'S') && spindle_on && !matches!(m, Some(3 | 5 | 6 | 2 | 30)) {
            out.push(Record { line: line_number, motion: Motion::Spindle { rpm } });
        }
        if moving {
            let scale = units.ok_or_else(|| fail("missing units"))?;
            let abs = absolute.ok_or_else(|| fail("missing distance mode"))?;
            if !plane || !per_minute || offset.is_none() {
                return Err(fail("declare G17, G94 and work offset before motion"));
            }
            if !have_pos && (!abs || coordinates.iter().any(Option::is_none)) {
                return Err(fail("first position requires absolute XYZ"));
            }
            let base = [pos.x, pos.y, pos.z];
            let mut xyz = base;
            for k in 0..3 {
                if let Some(v) = coordinates[k] {
                    xyz[k] = v * scale + if abs { 0.0 } else { base[k] };
                }
            }
            let to = Vec3::new(xyz[0], xyz[1], xyz[2]);
            crate::model::point(to)?;
            let motion = match mode.ok_or_else(|| fail("missing motion mode"))? {
                0 => {
                    if ij.iter().any(Option::is_some) {
                        return Err(fail("arc center on rapid"));
                    }
                    Motion::Rapid { to }
                }
                1 => {
                    if ij.iter().any(Option::is_some) || feed <= 0.0 {
                        return Err(fail("invalid linear feed/center"));
                    }
                    Motion::Linear { to, feed }
                }
                arc => {
                    if !have_pos || feed <= 0.0 || ij.iter().all(Option::is_none) || (to.z - pos.z).abs() > 1e-8 {
                        return Err(fail("only planar XY I/J arcs with positive feed are supported"));
                    }
                    let center = Vec3::new(pos.x + ij[0].unwrap_or(0.0) * scale, pos.y + ij[1].unwrap_or(0.0) * scale, pos.z);
                    let r = pos.dist(center);
                    if r <= 1e-8 || (to.dist(center) - r).abs() > POST_ERROR {
                        return Err(fail("inconsistent arc radius"));
                    }
                    Motion::Arc { to, center, clockwise: arc == 2, feed }
                }
            };
            out.push(Record { line: line_number, motion });
            pos = to;
            have_pos = true;
        }
        if let Some(m) = m {
            let motion = match m {
                0 => Some(Motion::Stop),
                3 => {
                    if rpm <= 0.0 {
                        return Err(fail("spindle start requires positive speed"));
                    }
                    spindle_on = true;
                    Some(Motion::Spindle { rpm })
                }
                5 => {
                    spindle_on = false;
                    Some(Motion::Spindle { rpm: 0.0 })
                }
                6 => {
                    let n = selected.ok_or_else(|| fail("tool change needs T"))?;
                    let matches: Vec<_> = tools.iter().filter(|t| t.number == n).collect();
                    if matches.len() != 1 {
                        return Err(fail("unknown or ambiguous tool number"));
                    }
                    // NIST RS274/NGC §3.6.3: M6 stops the spindle without
                    // restarting it; programmed speed and coolant are retained.
                    if spindle_on {
                        out.push(Record { line: line_number, motion: Motion::Spindle { rpm: 0.0 } });
                    }
                    spindle_on = false;
                    Some(Motion::Tool { tool: matches[0].id, number: n })
                }
                8 => {
                    coolant_on = true;
                    Some(Motion::Coolant { on: true })
                }
                9 => {
                    coolant_on = false;
                    Some(Motion::Coolant { on: false })
                }
                2 | 30 => {
                    // Program end stops spindle and coolant in the interpreted state.
                    if spindle_on {
                        out.push(Record { line: line_number, motion: Motion::Spindle { rpm: 0.0 } });
                    }
                    if coolant_on {
                        out.push(Record { line: line_number, motion: Motion::Coolant { on: false } });
                    }
                    ended = true;
                    None
                }
                _ => return Err(fail("unsupported M code")),
            };
            if let Some(motion) = motion {
                out.push(Record { line: line_number, motion });
            }
        }
        if out.len() > MAX_RECORDS {
            return Err(fail("NC motion limit"));
        }
    }
    if !ended || !have_pos {
        return invalid("NC program requires motion and an explicit program end");
    }
    checkpoint(&cancelled, &mut yield_control).await?;
    Ok(Parsed { records: out, work_offset: offset.ok_or_else(|| Error("missing work offset".into()))?, sources: Vec::new() })
}
fn words(line: &str) -> Result<Vec<(char, f64)>> {
    if line.len() > 4096 {
        return invalid("NC line too long");
    }
    let mut clean = String::new();
    let mut comment = false;
    for c in line.chars() {
        match c {
            '(' if !comment => comment = true,
            ')' if comment => comment = false,
            '(' | ')' => return invalid("malformed comment"),
            ';' if !comment => break,
            _ if !comment => clean.push(c),
            _ => {}
        }
    }
    if comment {
        return invalid("unterminated comment");
    }
    let clean = clean.trim();
    if clean == "%" {
        return Ok(Vec::new());
    }
    let chars: Vec<_> = clean.chars().collect();
    let mut i = 0;
    let mut result = Vec::new();
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let letter = chars[i].to_ascii_uppercase();
        if !letter.is_ascii_uppercase() {
            return invalid("expected address letter");
        }
        i += 1;
        while chars.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
        let start = i;
        while chars.get(i).is_some_and(|c| c.is_ascii_digit() || matches!(c, '.' | '+' | '-')) {
            i += 1;
        }
        let s: String = chars[start..i].iter().collect();
        let v = s.parse::<f64>().map_err(|_| Error("invalid numeric word".into()))?;
        if !crate::model::finite(v) {
            return invalid("non-finite/excessive word");
        }
        result.push((letter, v));
    }
    Ok(result)
}

pub fn round_trip(path: &Toolpath, setup: &Setup, tools: &[Tool]) -> Result<Parsed> {
    complete(round_trip_cooperative(path, setup, tools, || false, || std::future::ready(())))
}
/// Post, independently interpret and compare in bounded batches, retaining trusted line mappings.
pub async fn round_trip_cooperative<F: Future<Output = ()>>(
    path: &Toolpath,
    setup: &Setup,
    tools: &[Tool],
    cancelled: impl Fn() -> bool,
    mut yield_control: impl FnMut() -> F,
) -> Result<Parsed> {
    let source = post_cooperative(path, setup, &cancelled, &mut yield_control).await?;
    let mut parsed = parse_cooperative(&source, tools, &cancelled, &mut yield_control).await?;
    let startup = startup(setup);
    if parsed.work_offset != setup.work_offset || parsed.records.len() != path.blocks.len() + startup.len() {
        return invalid("post changed block count or work offset");
    }
    if !startup.iter().zip(&parsed.records).all(|(a, b)| equivalent(a, &b.motion)) {
        return invalid("post changed startup state");
    }
    let mut original_position = Vec3::new(0., 0., setup.clearance);
    let mut posted_position = original_position;
    if let Some(Record { motion: Motion::Rapid { to }, .. }) = parsed.records.get(startup.len() - 1) {
        posted_position = *to;
    }
    for (i, (a, b)) in path.blocks.iter().zip(parsed.records.iter().skip(startup.len())).enumerate() {
        if i.is_multiple_of(256) {
            checkpoint(&cancelled, &mut yield_control).await?;
        }
        if !equivalent(&a.motion, &b.motion) {
            return invalid(format!("post mismatch at NC line {}", b.line));
        }
        if let (Motion::Arc { to, center, clockwise, .. }, Motion::Arc { to: posted_to, center: posted_center, .. }) = (&a.motion, &b.motion) {
            let before = sweep(original_position, *to, *center, *clockwise);
            let after = sweep(posted_position, *posted_to, *posted_center, *clockwise);
            if (before - after).abs() * original_position.dist(*center) > POST_ERROR {
                return invalid(format!("post changed arc sweep at NC line {}", b.line));
            }
        }
        if let Motion::Rapid { to } | Motion::Linear { to, .. } | Motion::Arc { to, .. } = a.motion {
            original_position = to;
        }
        if let Motion::Rapid { to } | Motion::Linear { to, .. } | Motion::Arc { to, .. } = b.motion {
            posted_position = to;
        }
        parsed.sources.push(SourceLocation { line: b.line, block: i + 1, operation: a.operation });
    }
    checkpoint(&cancelled, &mut yield_control).await?;
    Ok(parsed)
}
fn sweep(from: Vec3, to: Vec3, center: Vec3, clockwise: bool) -> f64 {
    if from.x == to.x && from.y == to.y {
        return if clockwise { -std::f64::consts::TAU } else { std::f64::consts::TAU };
    }
    let a = (from.y - center.y).atan2(from.x - center.x);
    let b = (to.y - center.y).atan2(to.x - center.x);
    if clockwise { -(a - b).rem_euclid(std::f64::consts::TAU) } else { (b - a).rem_euclid(std::f64::consts::TAU) }
}
fn equivalent(a: &Motion, b: &Motion) -> bool {
    match (a, b) {
        (Motion::Rapid { to: a }, Motion::Rapid { to: b }) => a.dist(*b) <= POST_ERROR,
        (Motion::Linear { to: a, feed: f }, Motion::Linear { to: b, feed: g }) => a.dist(*b) <= POST_ERROR && (f - g).abs() <= 1e-6,
        (Motion::Arc { to: a, center: c, clockwise: d, feed: f }, Motion::Arc { to: b, center: e, clockwise: h, feed: g }) => {
            a.dist(*b) <= POST_ERROR && c.dist(*e) <= POST_ERROR && d == h && (f - g).abs() <= 1e-6
        }
        (Motion::Spindle { rpm: a }, Motion::Spindle { rpm: b }) => (a - b).abs() <= 1e-6,
        _ => a == b,
    }
}

#[derive(Clone, Debug)]
pub struct Segment {
    pub from: Vec3,
    pub to: Vec3,
    pub rapid: bool,
    pub feed: f64,
    pub tool: u64,
    pub line: usize,
}
/// Expand interpreted circular motion with a bounded chord error, independent of posting.
pub fn segments(parsed: &Parsed, setup: &Setup) -> Result<Vec<Segment>> {
    let mut out = Vec::new();
    let mut pos = None;
    let mut tool = 0;
    let mut spindle = 0.0;
    for r in &parsed.records {
        match &r.motion {
            Motion::Tool { tool: id, .. } => {
                if pos.is_none_or(|p: Vec3| p.z < setup.clearance - POST_ERROR) {
                    return invalid(format!("tool change below clearance at line {}", r.line));
                }
                if spindle != 0.0 {
                    return invalid("tool change with spindle running");
                }
                tool = *id;
            }
            Motion::Spindle { rpm } => spindle = *rpm,
            Motion::Rapid { to } | Motion::Linear { to, .. } | Motion::Arc { to, .. } => {
                let rapid = matches!(r.motion, Motion::Rapid { .. });
                let feed = match r.motion {
                    Motion::Linear { feed, .. } | Motion::Arc { feed, .. } => feed,
                    _ => 0.0,
                };
                if !rapid && (tool == 0 || spindle <= 0.0) {
                    return invalid(format!("cutting without tool/spindle at line {}", r.line));
                }
                if let Some(from) = pos {
                    if tool == 0 {
                        return invalid(format!("motion before selecting a known tool at line {}", r.line));
                    }
                    if let Motion::Arc { center, clockwise, .. } = r.motion {
                        let a: f64 = (from.y - center.y).atan2(from.x - center.x);
                        let sweep = sweep(from, *to, center, clockwise);
                        let radius = from.dist(center);
                        let step = (2.0 * (1.0 - POST_ERROR / radius).clamp(-1.0, 1.0).acos()).max(1e-6);
                        let count = (sweep.abs() / step).ceil() as usize;
                        if count > MAX_MOVES || out.len() + count > MAX_MOVES {
                            return invalid("arc subdivision limit");
                        }
                        let mut prev = from;
                        for k in 1..=count {
                            let angle = a + sweep * k as f64 / count as f64;
                            let p = if k == count { *to } else { Vec3::new(center.x + radius * angle.cos(), center.y + radius * angle.sin(), to.z) };
                            out.push(Segment { from: prev, to: p, rapid, feed, tool, line: r.line });
                            prev = p;
                        }
                    } else {
                        out.push(Segment { from, to: *to, rapid, feed, tool, line: r.line });
                    }
                } else if !rapid || to.z < setup.clearance - POST_ERROR {
                    return invalid("initial position must be at clearance");
                }
                pos = Some(*to);
            }
            _ => {}
        }
        if out.len() > MAX_MOVES {
            return invalid("simulation segment limit");
        }
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod provenance_tests {
    use super::*;
    #[test]
    fn tiny_arcs_cannot_round_into_unintended_full_circles() {
        let input = corpus::fixture("face").unwrap();
        for clockwise in [false, true] {
            let to = Vec3::new(1., if clockwise { -1e-8 } else { 1e-8 }, 15.);
            let path = Toolpath {
                fingerprint: input.fingerprint.clone(),
                blocks: vec![
                    Block { operation: 1, motion: Motion::Tool { tool: 1, number: 1 } },
                    Block { operation: 1, motion: Motion::Spindle { rpm: 1000. } },
                    Block { operation: 1, motion: Motion::Rapid { to: Vec3::new(1., 0., 15.) } },
                    Block { operation: 1, motion: Motion::Arc { to, center: Vec3::new(0., 0., 15.), clockwise, feed: 100. } },
                    Block { operation: 1, motion: Motion::Spindle { rpm: 0. } },
                ],
            };
            assert!(round_trip(&path, &input.setup, &input.tools).unwrap_err().0.contains("arc sweep"));
        }
        let raw = "G21 G90 G17 G94 G54\nG0 X1 Y0 Z15\nT1 M6\nS1000 M3\nG3 X1 Y0.0000000000001 I-1 J0 F100\nM5\nM30";
        let parsed = parse(raw, &input.tools).unwrap();
        let distance: f64 = segments(&parsed, &input.setup).unwrap().iter().map(|s| s.from.dist(s.to)).sum();
        assert!(distance < 1e-10, "a noncoincident tiny arc is not a full circle: {distance}");
    }
    #[test]
    fn diagnostic_sources_follow_checked_records_and_ignore_untrusted_annotations() {
        let input = corpus::fixture("face").unwrap();
        let path = Toolpath {
            fingerprint: input.fingerprint.clone(),
            blocks: vec![
                Block { operation: 10, motion: Motion::Tool { tool: 1, number: 1 } },
                Block { operation: 10, motion: Motion::Spindle { rpm: 5000. } },
                Block { operation: 10, motion: Motion::Rapid { to: Vec3::new(1., 0., 15.) } },
                Block {
                    operation: 20,
                    motion: Motion::Arc { to: Vec3::new(1., 0., 15.), center: Vec3::new(0., 0., 15.), clockwise: true, feed: 100. },
                },
                Block { operation: 20, motion: Motion::Spindle { rpm: 0. } },
                Block { operation: 20, motion: Motion::Stop },
            ],
        };
        let parsed = round_trip(&path, &input.setup, &input.tools).unwrap();
        let prefix = startup(&input.setup).len();
        for (i, record) in parsed.records.iter().skip(prefix).enumerate() {
            assert_eq!(
                parsed.source_for_line(record.line),
                Some(SourceLocation { line: record.line, block: i + 1, operation: path.blocks[i].operation })
            );
            assert_eq!(parsed.record_for_line(record.line), Some(i + prefix));
        }
        for record in parsed.records.iter().take(prefix) {
            assert!(parsed.source_for_line(record.line).is_none());
        }
        assert!(parsed.source_for_line(7).is_none()); // comment
        let arc = &parsed.records[prefix + 3];
        let segments = segments(&parsed, &input.setup).unwrap();
        assert!(segments.iter().filter(|s| s.line == arc.line).count() > 10);
        assert!(segments.iter().filter(|s| s.line == arc.line).all(|s| parsed.source_for_line(s.line).unwrap().block == 4));
        let raw = parse(&post(&path, &input.setup).unwrap().replace("Operation 20", "Operation 999"), &input.tools).unwrap();
        assert!(raw.source_for_line(arc.line).is_none());
        let mut serialized = serde_json::to_value(&parsed).unwrap();
        serialized["sources"] = serde_json::json!([{"line":arc.line,"block":4,"operation":999}]);
        let decoded: Parsed = serde_json::from_value(serialized).unwrap();
        assert!(decoded.source_for_line(arc.line).is_none());
    }
    #[test]
    fn diagnostic_fault_findings_retain_checked_block_but_raw_nc_has_only_record() {
        let input = corpus::fixture("face").unwrap();
        let mut path = generate::generate(input.clone()).unwrap();
        for block in &mut path.blocks {
            if let Motion::Linear { to, .. } = &mut block.motion
                && to.z == 5.
            {
                to.z = 4.;
            }
        }
        let parsed = round_trip(&path, &input.setup, &input.tools).unwrap();
        let report = verify::simulate(&input, &parsed, Default::default()).unwrap();
        assert_eq!(report.status, verify::Status::Fail);
        let finding = report.findings.iter().find(|f| f.kind == "gouge").unwrap();
        assert_eq!(finding.severity, verify::FindingSeverity::Error);
        assert_eq!(report.findings[0].severity, verify::FindingSeverity::Error);
        let source = finding.source.unwrap();
        assert_eq!(Some(source.line), finding.line);
        assert_eq!(source.operation, path.blocks[source.block - 1].operation);
        assert_eq!(finding.record, Some(source.block + startup(&input.setup).len()));
        let raw = parse(&post(&path, &input.setup).unwrap(), &input.tools).unwrap();
        let raw_report = verify::simulate(&input, &raw, Default::default()).unwrap();
        assert_eq!(raw_report.status, report.status);
        assert_eq!(raw_report.finished_dimensions, report.finished_dimensions);
        assert!(raw_report.findings.iter().filter(|f| f.line.is_some()).all(|f| f.source.is_none() && f.record.is_some()));
        let legacy: verify::Finding =
            serde_json::from_value(serde_json::json!({"kind":"legacy","line":4,"at":[0,0,0],"detail":"old report"})).unwrap();
        assert!(legacy.record.is_none() && legacy.source.is_none());
    }
    #[test]
    fn diagnostic_retention_keeps_errors_and_budget_reasons_bounded() {
        let input = corpus::fixture("face").unwrap();
        let path = generate::generate(input.clone()).unwrap();
        let parsed = round_trip(&path, &input.setup, &input.tools).unwrap();
        let mut report = verify::simulate(&input, &parsed, Default::default()).unwrap();
        report.findings.clear();
        let finding = |line, severity, kind: &str| verify::Finding {
            line,
            severity,
            kind: kind.into(),
            record: None,
            source: None,
            at: Vec3::ZERO,
            detail: "witness".into(),
        };
        for line in 1..=250 {
            report.finding(finding(Some(line), verify::FindingSeverity::Unresolved, "clearance_unresolved"));
        }
        assert_eq!(report.findings.len(), 200);
        report.finding(finding(None, verify::FindingSeverity::Unresolved, "budget_exhausted"));
        report.finding(finding(Some(999), verify::FindingSeverity::Error, "gouge"));
        for _ in 0..250 {
            report.finding(finding(Some(999), verify::FindingSeverity::Error, "gouge"));
        }
        assert_eq!(report.findings.len(), 200);
        assert_eq!(report.findings.iter().filter(|f| f.kind == "gouge").count(), 1);
        assert!(report.findings.iter().any(|f| f.kind == "budget_exhausted"));
        // Diagnostic storage is not an input to acceptance decisions.
        assert_eq!(report.status, verify::Status::Pass);
    }
}

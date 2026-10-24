//! The structure of persistent face and edge names, parsed once instead of split by hand.
//!
//! A face name is a leaf (`F2:top`, `F2:side:l4`, `Body1:face:3`) or nests other names: a fillet
//! or chamfer face holds the edge it rounds (`F5:blend:<face>|<face>`), a shell's inner wall the
//! face it offsets (`F3:inner:<face>`). After any of them come suffixes: a pattern copy
//! (`@F7.2`), a piece of a split face told apart by the faces around it (`#[<face>,…]`, then
//! `#k` for a tie), or an older design's numbered piece (`#k`). An edge is two faces joined by
//! `|`, `~k` for one of several between them.
//!
//! Suffixes after a blend's edge are ambiguous in the text: in `F5:blend:A|B#[…]` the piece
//! qualifier may split `B` (the fillet rounds an edge of a piece made earlier) or the fillet
//! face itself (a later cut split it). A suffix naming a feature later than the blend's belongs
//! to the blend face; any other stays on `B`. A later cut that only older faces tell apart is
//! read as splitting `B`. A trailing `~k` on an edge numbers that edge, not a blend's inner edge.
//!
//! Parsing never panics on hostile names: it is bounded in length and depth, and a name it
//! cannot read is `None`. Every name it reads writes back as the same text.

use std::fmt;

/// The longest name read.
pub const MAX_NAME_LEN: usize = 4096;
/// The deepest nesting read (blends of blends, pieces of pieces).
const MAX_DEPTH: usize = 32;

/// A persistent face name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FaceName {
    /// A name with no nested names: `F2:top`, `F2:side:l4`, `Body1:face:3`.
    Leaf(String),
    /// `F<feature>:blend:<edge>`: a fillet or chamfer face, named by the edge it rounds.
    Blend { feature: u64, edge: Box<EdgeName> },
    /// `F<feature>:blend:<k>`: a blend face named by its edge's index (no edge name was known).
    BlendIndex { feature: u64, index: u64 },
    /// `F<feature>:inner:<face>`: a shell's inner wall, named by the face it offsets.
    Inner { feature: u64, face: Box<FaceName> },
    /// `<face>@F<feature>.<instance>`: a pattern or mirror copy of a face.
    Copy { of: Box<FaceName>, feature: u64, instance: u64 },
    /// `<face>#[<faces around it>]`, `#k` after it for a tie: a piece of a split face.
    Piece { of: Box<FaceName>, around: Vec<FaceName>, tie: Option<u64> },
    /// `<face>#k`: a numbered piece of a design saved before format 14.
    Numbered { of: Box<FaceName>, k: u64 },
}

/// A persistent edge name: the two faces it separates, `~k` for one of several between them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeName {
    pub faces: [FaceName; 2],
    pub tie: Option<u64>,
}

impl FaceName {
    /// The face name `s`, or `None` when it is not one.
    pub fn parse(s: &str) -> Option<FaceName> {
        if s.len() > MAX_NAME_LEN {
            return None;
        }
        let mut p = Parser { s, at: 0 };
        let f = p.face(0)?;
        p.done().then_some(f)
    }

    /// The feature that made the face: the one in its own `F<n>` (a copy's is the copying
    /// feature), `None` for a leaf without one.
    pub fn feature(&self) -> Option<u64> {
        match self {
            FaceName::Leaf(s) => leaf_feature(s),
            FaceName::Blend { feature, .. } | FaceName::BlendIndex { feature, .. } | FaceName::Inner { feature, .. } => Some(*feature),
            FaceName::Copy { feature, .. } => Some(*feature),
            FaceName::Piece { of, .. } | FaceName::Numbered { of, .. } => of.feature(),
        }
    }

    /// The face this one is a piece of, all piece suffixes off (`F2:top` for `F2:top#[…]#1`).
    pub fn stem(&self) -> &FaceName {
        match self {
            FaceName::Piece { of, .. } | FaceName::Numbered { of, .. } => of.stem(),
            other => other,
        }
    }
}

impl EdgeName {
    /// The edge name `s`, or `None` when it is not one.
    pub fn parse(s: &str) -> Option<EdgeName> {
        if s.len() > MAX_NAME_LEN {
            return None;
        }
        // A trailing `~k` numbers this edge (a blend inside keeps any `~k` before its end).
        let (body, tie) = split_tie(s);
        let mut p = Parser { s: body, at: 0 };
        let e = p.edge(0)?;
        if !p.done() || e.tie.is_some() && tie.is_some() {
            return None;
        }
        Some(EdgeName { tie: tie.or(e.tie), ..e })
    }
}

impl fmt::Display for FaceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FaceName::Leaf(s) => f.write_str(s),
            FaceName::Blend { feature, edge } => write!(f, "F{feature}:blend:{edge}"),
            FaceName::BlendIndex { feature, index } => write!(f, "F{feature}:blend:{index}"),
            FaceName::Inner { feature, face } => write!(f, "F{feature}:inner:{face}"),
            FaceName::Copy { of, feature, instance } => write!(f, "{of}@F{feature}.{instance}"),
            FaceName::Piece { of, around, tie } => {
                write!(f, "{of}#[")?;
                for (i, a) in around.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{a}")?;
                }
                f.write_str("]")?;
                match tie {
                    Some(k) => write!(f, "#{k}"),
                    None => Ok(()),
                }
            }
            FaceName::Numbered { of, k } => write!(f, "{of}#{k}"),
        }
    }
}

impl fmt::Display for EdgeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b] = &self.faces;
        write!(f, "{a}|{b}")?;
        match self.tie {
            Some(k) => write!(f, "~{k}"),
            None => Ok(()),
        }
    }
}

/// `s` without a trailing `~k`, and `k`.
fn split_tie(s: &str) -> (&str, Option<u64>) {
    match s.rfind('~') {
        Some(i) => match s.get(i + 1..).and_then(number) {
            Some(k) => (s.get(..i).unwrap_or(s), Some(k)),
            None => (s, None),
        },
        None => (s, None),
    }
}

/// A plain decimal number: digits only, no leading zero (so it writes back the same).
fn number(d: &str) -> Option<u64> {
    let ok = !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()) && (d.len() == 1 || !d.starts_with('0'));
    if ok { d.parse().ok() } else { None }
}

/// The `n` of a leaf that starts `F<n>:` or `F<n>.`.
fn leaf_feature(s: &str) -> Option<u64> {
    let rest = s.strip_prefix('F')?;
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    matches!(rest.get(end..end + 1), Some(":" | ".")).then(|| rest.get(..end).and_then(number)).flatten()
}

/// The latest feature any face of `faces` names.
fn latest(faces: &[FaceName]) -> Option<u64> {
    faces.iter().filter_map(FaceName::feature).max()
}

struct Parser<'a> {
    s: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        self.s.get(self.at..).unwrap_or("")
    }

    fn done(&self) -> bool {
        self.at >= self.s.len()
    }

    fn eat(&mut self, t: &str) -> bool {
        if self.rest().starts_with(t) {
            self.at += t.len();
            true
        } else {
            false
        }
    }

    /// Digits at the cursor as a number, consumed only when they are one.
    fn digits(&mut self) -> Option<u64> {
        let rest = self.rest();
        let n = rest.bytes().take_while(u8::is_ascii_digit).count();
        let v = number(rest.get(..n)?)?;
        self.at += n;
        Some(v)
    }

    /// `F<n>:<kind>:` at the cursor: `n`, the cursor after it.
    fn nested_head(&self, kind: &str) -> Option<(u64, usize)> {
        let rest = self.rest().strip_prefix('F')?;
        let n = rest.bytes().take_while(u8::is_ascii_digit).count();
        let feature = number(rest.get(..n)?)?;
        let after = rest.get(n..)?.strip_prefix(':')?.strip_prefix(kind)?.strip_prefix(':')?;
        Some((feature, self.s.len() - after.len()))
    }

    /// Two faces joined by `|`, then `~k`.
    fn edge(&mut self, depth: usize) -> Option<EdgeName> {
        let a = self.face(depth + 1)?;
        if !self.eat("|") {
            return None;
        }
        let b = self.face(depth + 1)?;
        let tie = if self.rest().starts_with('~') {
            let save = self.at;
            self.at += 1;
            match self.digits() {
                Some(k) => Some(k),
                None => {
                    self.at = save;
                    None
                }
            }
        } else {
            None
        };
        Some(EdgeName { faces: [a, b], tie })
    }

    fn face(&mut self, depth: usize) -> Option<FaceName> {
        if depth > MAX_DEPTH {
            return None;
        }
        let base = if let Some((feature, after)) = self.nested_head("blend") {
            self.at = after;
            let save = self.at;
            match self.digits() {
                Some(index) if self.at_end_of_face() => FaceName::BlendIndex { feature, index },
                _ => {
                    self.at = save;
                    let edge = self.edge(depth + 1)?;
                    return self.suffixes(lift(feature, edge), depth);
                }
            }
        } else if let Some((feature, after)) = self.nested_head("inner") {
            self.at = after;
            FaceName::Inner { feature, face: Box::new(self.face(depth + 1)?) }
        } else {
            FaceName::Leaf(self.leaf()?)
        };
        self.suffixes(base, depth)
    }

    /// Whether the cursor is where a face name may end.
    fn at_end_of_face(&self) -> bool {
        let r = self.rest();
        r.is_empty() || r.starts_with(['|', ',', ']', '~', '#', '@'])
    }

    /// The text up to the next separator outside brackets.
    fn leaf(&mut self) -> Option<String> {
        let rest = self.rest();
        let bytes = rest.as_bytes();
        let mut i = 0;
        while let Some(&b) = bytes.get(i) {
            let next = bytes.get(i + 1).copied();
            let stop = match b {
                b'|' | b',' | b']' => true,
                b'#' => next.is_some_and(|n| n == b'[' || n.is_ascii_digit()),
                b'~' => next.is_some_and(|n| n.is_ascii_digit()),
                b'@' => next == Some(b'F'),
                _ => false,
            };
            if stop {
                break;
            }
            i += 1;
        }
        let leaf = rest.get(..i).filter(|l| !l.is_empty())?.to_string();
        self.at += i;
        Some(leaf)
    }

    /// `face` with the suffixes after the cursor.
    fn suffixes(&mut self, mut face: FaceName, depth: usize) -> Option<FaceName> {
        loop {
            if self.rest().starts_with("@F") {
                let save = self.at;
                self.at += 2;
                let copy = self.digits().and_then(|feature| self.eat(".").then_some(feature)).and_then(|feature| Some((feature, self.digits()?)));
                match copy {
                    Some((feature, instance)) => face = FaceName::Copy { of: Box::new(face), feature, instance },
                    None => {
                        self.at = save;
                        return Some(face);
                    }
                }
            } else if self.eat("#[") {
                let mut around = Vec::new();
                loop {
                    around.push(self.face(depth + 1)?);
                    if self.eat("]") {
                        break;
                    }
                    if !self.eat(",") {
                        return None;
                    }
                }
                let tie = if self.rest().starts_with('#') {
                    let save = self.at;
                    self.at += 1;
                    let k = self.digits();
                    if k.is_none() {
                        self.at = save;
                    }
                    k
                } else {
                    None
                };
                face = FaceName::Piece { of: Box::new(face), around, tie };
            } else if self.rest().starts_with('#') {
                let save = self.at;
                self.at += 1;
                match self.digits() {
                    Some(k) => face = FaceName::Numbered { of: Box::new(face), k },
                    None => {
                        self.at = save;
                        return Some(face);
                    }
                }
            } else {
                return Some(face);
            }
        }
    }
}

/// The blend face of `feature` rounding `edge`, the suffixes its second face took that belong
/// to the blend face moved onto it: those naming a feature later than the blend.
fn lift(feature: u64, mut edge: EdgeName) -> FaceName {
    let mut outer: Vec<FaceName> = Vec::new();
    let [a, b] = edge.faces;
    let mut b = b;
    loop {
        let later = match &b {
            FaceName::Copy { feature: f, .. } => *f > feature,
            FaceName::Piece { around, .. } => latest(around).is_some_and(|f| f > feature),
            _ => false,
        };
        if !later {
            break;
        }
        // Peel the outermost suffix off `b`, keep it to wrap the blend face.
        match b {
            FaceName::Copy { of, feature: f, instance } => {
                outer.push(FaceName::Copy { of: Box::new(FaceName::Leaf(String::new())), feature: f, instance });
                b = *of;
            }
            FaceName::Piece { of, around, tie } => {
                outer.push(FaceName::Piece { of: Box::new(FaceName::Leaf(String::new())), around, tie });
                b = *of;
            }
            other => {
                b = other;
                break;
            }
        }
    }
    edge.faces = [a, b];
    let mut face = FaceName::Blend { feature, edge: Box::new(edge) };
    // The innermost peeled suffix applies first.
    while let Some(s) = outer.pop() {
        face = match s {
            FaceName::Copy { feature, instance, .. } => FaceName::Copy { of: Box::new(face), feature, instance },
            FaceName::Piece { around, tie, .. } => FaceName::Piece { of: Box::new(face), around, tie },
            other => other,
        };
    }
    face
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(s: &str) -> FaceName {
        let f = FaceName::parse(s).unwrap_or_else(|| panic!("{s} reads"));
        assert_eq!(f.to_string(), s, "writes back");
        f
    }

    fn edge(s: &str) -> EdgeName {
        let e = EdgeName::parse(s).unwrap_or_else(|| panic!("{s} reads"));
        assert_eq!(e.to_string(), s, "writes back");
        e
    }

    fn leaf(s: &str) -> FaceName {
        FaceName::Leaf(s.into())
    }

    #[test]
    fn leaves_and_their_features() {
        assert_eq!(face("F2:top"), leaf("F2:top"));
        assert_eq!(face("F2:side:s1.l4").feature(), Some(2));
        assert_eq!(face("Body1:face:3").feature(), None);
        assert_eq!(face("F12:boss:side:3").feature(), Some(12));
        assert_eq!(face("F3:+x").feature(), Some(3));
    }

    #[test]
    fn pieces_copies_and_numbered_pieces() {
        let p = face("F2:top#[F2:side:l2,F4:side:l2]");
        assert_eq!(p, FaceName::Piece { of: Box::new(leaf("F2:top")), around: vec![leaf("F2:side:l2"), leaf("F4:side:l2")], tie: None });
        assert_eq!(p.stem(), &leaf("F2:top"));
        assert!(matches!(face("F2:top#[F4:side:l2]#1"), FaceName::Piece { tie: Some(1), .. }));
        assert!(matches!(face("F2:top#2"), FaceName::Numbered { k: 2, .. }));
        assert!(matches!(face("F2:side:l1@F7.2"), FaceName::Copy { feature: 7, instance: 2, .. }));
        // A piece of a piece.
        let pp = face("F2:top#[F4:side:l2#[F5:x]]");
        assert!(matches!(&pp, FaceName::Piece { around, .. } if matches!(around.first(), Some(FaceName::Piece { .. }))));
    }

    /// The fillet rounds an edge of a piece the slot made before it: the qualifier is the
    /// piece's (corpus `naming-split-pieces`).
    #[test]
    fn a_blend_of_an_earlier_piece_keeps_the_qualifier_inside() {
        let f = face("F5:blend:F2:side:l2|F2:top#[F2:side:l2,F4:side:l2]");
        let FaceName::Blend { feature: 5, edge } = &f else { panic!("{f:?}") };
        assert_eq!(edge.faces[0], leaf("F2:side:l2"));
        assert!(matches!(&edge.faces[1], FaceName::Piece { of, .. } if **of == leaf("F2:top")));
    }

    /// A slot cut after the fillet splits the fillet face: the qualifier, naming the later slot,
    /// is the fillet face's.
    #[test]
    fn a_blend_split_by_a_later_cut_takes_the_qualifier() {
        let f = face("F4:blend:F3:+x|F3:+z#[F3:-y,F5:-y]");
        let FaceName::Piece { of, around, .. } = &f else { panic!("{f:?}") };
        assert_eq!(around, &vec![leaf("F3:-y"), leaf("F5:-y")]);
        assert_eq!(**of, FaceName::Blend { feature: 4, edge: Box::new(EdgeName { faces: [leaf("F3:+x"), leaf("F3:+z")], tie: None }) });
        assert_eq!(f.stem(), &**of);
        // Copied by a later pattern, then split: both belong to the blend face.
        let g = face("F4:blend:F3:+x|F3:+z@F6.1#[F7:a]");
        assert!(
            matches!(&g, FaceName::Piece { of, .. } if matches!(**of, FaceName::Copy { feature: 6, ref of, .. } if matches!(**of, FaceName::Blend { .. })))
        );
    }

    /// What the text cannot tell: a later cut that only older faces tell apart reads as a piece
    /// of the inner face.
    #[test]
    fn a_later_cut_told_apart_by_older_faces_reads_as_the_inner_faces() {
        let f = face("F4:blend:F3:+x|F3:+z#[F3:-y]");
        assert!(matches!(&f, FaceName::Blend { edge, .. } if matches!(edge.faces[1], FaceName::Piece { .. })));
    }

    #[test]
    fn edges_split_into_their_two_faces() {
        let e = edge("F2:side:l1|F2:side:l2");
        assert_eq!(e.faces, [leaf("F2:side:l1"), leaf("F2:side:l2")]);
        assert_eq!(edge("F2:side:l1|F2:top~1").tie, Some(1));
        // The blend face first: the whole blend name, then the slot's wall.
        let e = edge("F4:blend:F3:+x|F3:+z#[F3:-y,F5:-y]|F5:-y");
        assert_eq!(e.faces[0].to_string(), "F4:blend:F3:+x|F3:+z#[F3:-y,F5:-y]");
        assert_eq!(e.faces[1], leaf("F5:-y"));
        // The blend face second.
        let e = edge("F2:side:l1|F5:blend:F2:side:l2|F2:top#[F2:side:l2,F4:side:l2]");
        assert_eq!(e.faces[0], leaf("F2:side:l1"));
        assert!(matches!(e.faces[1], FaceName::Blend { feature: 5, .. }));
        // A blend of a blend's edge.
        let e = edge("F6:blend:F4:blend:A|B|C|D");
        assert!(matches!(&e.faces[0], FaceName::Blend { feature: 6, edge } if matches!(edge.faces[0], FaceName::Blend { feature: 4, .. })));
        assert_eq!(e.faces[1], leaf("D"));
        // A trailing `~k` numbers the edge, not the blend's inner edge.
        let e = edge("F2:x|F5:blend:A|B~1");
        assert_eq!(e.tie, Some(1));
        assert!(matches!(&e.faces[1], FaceName::Blend { edge, .. } if edge.tie.is_none()));
        // A blend named by its edge's index.
        assert!(matches!(edge("F5:blend:3|F2:top").faces[0], FaceName::BlendIndex { feature: 5, index: 3 }));
    }

    #[test]
    fn inner_walls_nest_a_face() {
        let f = face("F3:inner:F2:top#[F2:side:l2]");
        assert!(matches!(&f, FaceName::Inner { feature: 3, face } if matches!(**face, FaceName::Piece { .. })));
    }

    #[test]
    fn hostile_names_never_panic() {
        let long = "F1:".repeat(MAX_NAME_LEN);
        let deep = format!("{}x{}", "F1:inner:".repeat(64), "");
        let deep_pieces = format!("x{}", "#[x".repeat(64)) + &"]".repeat(64);
        for s in [
            "",
            "|",
            "||",
            "#[",
            "#[]",
            "x#[",
            "x#[]",
            "x#[y",
            "x#[y,",
            "]",
            "x]",
            "F1:blend:",
            "F1:blend:x",
            "F1:blend:x|",
            "@F",
            "x@F1.",
            "x@F",
            "x~",
            "x|y~",
            "x|y~01",
            "F01:top",
            "x#0x",
            "\u{1F600}|\u{1F601}",
            long.as_str(),
            deep.as_str(),
            deep_pieces.as_str(),
        ] {
            if let Some(f) = FaceName::parse(s) {
                assert_eq!(f.to_string(), s);
            }
            if let Some(e) = EdgeName::parse(s) {
                assert_eq!(e.to_string(), s);
            }
        }
        assert_eq!(FaceName::parse(&long), None);
        assert_eq!(FaceName::parse(&deep), None);
        assert_eq!(EdgeName::parse("x"), None);
        assert_eq!(EdgeName::parse("x|y|z"), None);
    }
}

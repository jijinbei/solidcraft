//! Persistent face and edge names as values, and their text form.
//!
//! A face name says what made the face: a feature and its role there (`F2:top`, `F2:side:l4`), a
//! fillet or chamfer and the edge it rounds (`F5:blend:<edge>`), a shell and the face it offsets
//! (`F3:inner:<face>`), or, without history, a body and an index (`Body1:face:3`). Suffixes
//! follow: a pattern copy (`@F7.2`), a piece of a split face told apart by the faces around it
//! (`#[<face>,…]`, `#k` after it for a tie), an older design's numbered piece (`#k`). An edge is
//! the two faces it separates, `~k` for one of several between them.
//!
//! The text form, read by [`FaceName::parse`] and [`EdgeName::parse`] and written by `Display`:
//!
//! ```text
//! edge     = face "|" face [TIE]
//! face     = base { suffix }
//! base     = FEATURE ":" "blend" ":" ( edge | NUMBER )
//!          | FEATURE ":" "inner" ":" face
//!          | FEATURE ":" role
//!          | text                              (`<body>:face:<k>`, or any other text)
//! suffix   = COPY | "#[" face { "," face } "]" [HASH] | HASH
//! FEATURE  = "F" NUMBER        COPY = "@F" NUMBER "." NUMBER
//! HASH     = "#" NUMBER        TIE  = "~" NUMBER
//! ```
//!
//! Suffixes after a nested name are ambiguous in the text: in `F5:blend:A|B#[…]` the qualifier
//! may split `B` (the fillet rounds an edge of a piece made before it) or the fillet face itself
//! (a later cut split it), and so for a shell's inner wall `F3:inner:X#[…]`. A suffix naming a
//! feature later than the outer face's belongs to the outer face; any other stays inside, so a
//! later cut told apart only by older faces reads as a piece of the inner name. A `~k` ending an
//! edge numbers that edge, not a blend's edge inside it.
//!
//! Finding a face by name still goes through `naming::strip_piece` and `parent_piece`, which
//! read a trailing qualifier as the whole name's; they move here once the text form says where a
//! qualifier belongs.
//!
//! Reading is bounded in length and depth and never panics; whatever it reads writes back as the
//! same text.

use std::fmt;

/// The longest name read.
pub const MAX_NAME_LEN: usize = 4096;
/// The deepest nesting read (blends of blends, pieces of pieces).
const MAX_DEPTH: usize = 32;

/// A feature of the timeline, as names refer to it (`F12`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FeatureId(pub u64);

/// A persistent face name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceName {
    pub base: Base,
    /// Innermost first.
    pub suffixes: Vec<Suffix>,
}

/// What made a face.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Base {
    /// `F<feature>:<role>`.
    Made { feature: FeatureId, role: Role },
    /// `F<feature>:blend:…`: a fillet or chamfer face.
    Blend { feature: FeatureId, rounds: Rounds },
    /// `F<feature>:inner:<face>`: a shell's inner wall.
    Inner { feature: FeatureId, offsets: Box<FaceName> },
    /// `<body>:face:<k>`: a face of a body without history, by index.
    BodyFace { body: String, index: u32 },
    /// Text of no form above.
    Other(String),
}

/// What a blend face rounds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rounds {
    /// The edge, by name.
    Edge(Box<EdgeName>),
    /// The edge's index among the feature's edges (no name was known).
    Index(u32),
}

/// A face's role in the feature that made it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    Top,
    Bottom,
    Start,
    End,
    Base,
    Section,
    /// `side`: a cylinder's wall, or a tool's faces swept from its whole profile.
    Wall,
    /// `side:<curve>`, `side:s<k>.<curve>` for profile `k`: swept from a sketch curve.
    Side {
        profile: Option<u32>,
        curve: String,
    },
    /// `+x`, `-z`, …: a box's face.
    BoxFace {
        axis: Axis,
        high: bool,
    },
    /// `face:<k>`: the feature's face `k`, named from geometry.
    Index(u32),
    /// A role of another form (a cap's, a tool part's, a sheet's), as written.
    Other(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

/// What follows a face's base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Suffix {
    /// `@F<feature>.<instance>`: a pattern or mirror copy.
    Copy { feature: FeatureId, instance: u32 },
    /// `#[…]`, `#k` after it: a piece of a split face.
    Piece(Qualifier),
    /// `#k`: a numbered piece of a design saved before format 14.
    Numbered(u32),
}

/// The faces only one piece of a split face touches, and its number among pieces they don't
/// tell apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Qualifier {
    around: Vec<FaceName>,
    pub tie: Option<u32>,
}

/// A persistent edge name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeName {
    pub faces: [FaceName; 2],
    pub tie: Option<u32>,
}

/// Why a name could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameError {
    TooLong,
    TooDeep,
    /// At byte `at`, something else was expected.
    Expected {
        at: usize,
        what: &'static str,
    },
}

impl FaceName {
    pub fn parse(s: &str) -> Result<FaceName, NameError> {
        let mut p = Parser::new(s)?;
        let f = p.face(0)?;
        p.end()?;
        Ok(f)
    }

    /// The feature that made the face: its base's, a copy's when it is one.
    pub fn feature(&self) -> Option<FeatureId> {
        let copied = self.suffixes.iter().rev().find_map(|s| match s {
            Suffix::Copy { feature, .. } => Some(*feature),
            _ => None,
        });
        copied.or(match &self.base {
            Base::Made { feature, .. } | Base::Blend { feature, .. } | Base::Inner { feature, .. } => Some(*feature),
            Base::BodyFace { .. } | Base::Other(_) => None,
        })
    }

    /// The face this one is a piece of: the trailing piece suffixes off.
    pub fn stem(&self) -> FaceName {
        let keep = self.suffixes.iter().rposition(|s| matches!(s, Suffix::Copy { .. })).map_or(0, |i| i + 1);
        FaceName { base: self.base.clone(), suffixes: self.suffixes.get(..keep).unwrap_or_default().to_vec() }
    }
}

impl Qualifier {
    /// A qualifier listing `around`, at least one face.
    pub fn new(around: Vec<FaceName>, tie: Option<u32>) -> Option<Qualifier> {
        (!around.is_empty()).then_some(Qualifier { around, tie })
    }

    pub fn around(&self) -> &[FaceName] {
        &self.around
    }
}

impl EdgeName {
    pub fn parse(s: &str) -> Result<EdgeName, NameError> {
        let mut p = Parser::new(s)?;
        // A `~k` ending the text numbers this edge.
        let tie = match p.toks.last() {
            Some(Tok { kind: Kind::Tie(k), .. }) => {
                let k = *k;
                p.toks.pop();
                Some(k)
            }
            _ => None,
        };
        let e = p.edge(0)?;
        p.end()?;
        Ok(EdgeName { tie, ..e })
    }
}

impl Role {
    fn from_text(t: &str) -> Role {
        match t {
            "top" => return Role::Top,
            "bottom" => return Role::Bottom,
            "start" => return Role::Start,
            "end" => return Role::End,
            "base" => return Role::Base,
            "section" => return Role::Section,
            "side" => return Role::Wall,
            _ => {}
        }
        if let Some(rest) = t.strip_prefix("side:").filter(|r| !r.is_empty()) {
            let profiled =
                rest.strip_prefix('s').and_then(|r| r.split_once('.')).and_then(|(k, c)| Some((number(k)?, c))).filter(|(_, c)| !c.is_empty());
            return match profiled {
                Some((k, c)) => Role::Side { profile: Some(k), curve: c.to_string() },
                None => Role::Side { profile: None, curve: rest.to_string() },
            };
        }
        if let Some(k) = t.strip_prefix("face:").and_then(number) {
            return Role::Index(k);
        }
        let mut c = t.chars();
        let box_face = match (c.next(), c.next(), c.next()) {
            (Some(sign @ ('+' | '-')), Some(a), None) => match a {
                'x' => Some(Axis::X),
                'y' => Some(Axis::Y),
                'z' => Some(Axis::Z),
                _ => None,
            }
            .map(|axis| Role::BoxFace { axis, high: sign == '+' }),
            _ => None,
        };
        box_face.unwrap_or_else(|| Role::Other(t.to_string()))
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::Top => f.write_str("top"),
            Role::Bottom => f.write_str("bottom"),
            Role::Start => f.write_str("start"),
            Role::End => f.write_str("end"),
            Role::Base => f.write_str("base"),
            Role::Section => f.write_str("section"),
            Role::Wall => f.write_str("side"),
            Role::Side { profile: Some(k), curve } => write!(f, "side:s{k}.{curve}"),
            Role::Side { profile: None, curve } => write!(f, "side:{curve}"),
            Role::BoxFace { axis, high } => {
                let a = match axis {
                    Axis::X => 'x',
                    Axis::Y => 'y',
                    Axis::Z => 'z',
                };
                write!(f, "{}{a}", if *high { '+' } else { '-' })
            }
            Role::Index(k) => write!(f, "face:{k}"),
            Role::Other(t) => f.write_str(t),
        }
    }
}

impl fmt::Display for FaceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.base {
            Base::Made { feature, role } => write!(f, "F{}:{role}", feature.0)?,
            Base::Blend { feature, rounds: Rounds::Edge(e) } => write!(f, "F{}:blend:{e}", feature.0)?,
            Base::Blend { feature, rounds: Rounds::Index(k) } => write!(f, "F{}:blend:{k}", feature.0)?,
            Base::Inner { feature, offsets } => write!(f, "F{}:inner:{offsets}", feature.0)?,
            Base::BodyFace { body, index } => write!(f, "{body}:face:{index}")?,
            Base::Other(t) => f.write_str(t)?,
        }
        for s in &self.suffixes {
            match s {
                Suffix::Copy { feature, instance } => write!(f, "@F{}.{instance}", feature.0)?,
                Suffix::Piece(q) => {
                    f.write_str("#[")?;
                    for (i, a) in q.around.iter().enumerate() {
                        if i > 0 {
                            f.write_str(",")?;
                        }
                        write!(f, "{a}")?;
                    }
                    f.write_str("]")?;
                    if let Some(k) = q.tie {
                        write!(f, "#{k}")?;
                    }
                }
                Suffix::Numbered(k) => write!(f, "#{k}")?,
            }
        }
        Ok(())
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

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NameError::TooLong => write!(f, "a name is at most {MAX_NAME_LEN} bytes"),
            NameError::TooDeep => write!(f, "a name nests at most {MAX_DEPTH} deep"),
            NameError::Expected { at, what } => write!(f, "expected {what} at byte {at}"),
        }
    }
}

impl std::error::Error for NameError {}

/// The trailing suffixes of `inner`, the last name nested in a face of `feature`, that name a
/// later feature, taken off it: they are the outer face's (a later cut split it, a later pattern
/// copied it).
fn later_suffixes(feature: FeatureId, inner: &mut FaceName) -> Vec<Suffix> {
    let later = |s: &Suffix| match s {
        Suffix::Copy { feature: f, .. } => *f > feature,
        Suffix::Piece(q) => q.around.iter().filter_map(FaceName::feature).max().is_some_and(|f| f > feature),
        Suffix::Numbered(_) => false,
    };
    let keep = inner.suffixes.iter().rposition(|s| !later(s)).map_or(0, |i| i + 1);
    inner.suffixes.split_off(keep)
}

/// The feature whose `F<n>:` starts the face name `name` (a copy's is its source's): typed when
/// the name reads, else the leading token as written.
pub fn made_by(name: &str) -> Option<u64> {
    match FaceName::parse(name) {
        Ok(f) => f.made_by().map(|id| id.0),
        Err(_) => name.strip_prefix('F').and_then(|r| r.split_once(':')).and_then(|(id, _)| id.parse().ok()),
    }
}

/// `name` without the `F<n>:` of the feature that made it, and `n`.
pub fn split_feature(name: &str) -> Option<(u64, &str)> {
    let id = made_by(name)?;
    Some((id, name.strip_prefix(&format!("F{id}:"))?))
}

/// Whether `name` is a pattern's or mirror's copy of a face.
pub fn is_copy(name: &str) -> bool {
    FaceName::parse(name).map_or_else(|_| name.contains('@'), |f| f.is_copy())
}

/// Whether `name` is a fillet's or chamfer's face.
pub fn is_blend(name: &str) -> bool {
    FaceName::parse(name).map_or_else(|_| name.contains(":blend:"), |f| f.is_blend())
}

/// Whether `name` is a piece told apart by a number (an older design's piece, or a tie).
pub fn is_numbered_piece(name: &str) -> bool {
    FaceName::parse(name).map_or_else(
        |_| name.match_indices('#').any(|(i, _)| name.get(i + 1..).is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))),
        |f| f.is_numbered_piece(),
    )
}

/// Whether `name` is a whole `side` face: a cylinder's wall, or a tool's faces swept from its
/// whole profile.
pub fn is_wall(name: &str) -> bool {
    FaceName::parse(name).map_or_else(|_| name.ends_with(":side"), |f| f.suffixes.is_empty() && matches!(f.base, Base::Made { role: Role::Wall, .. }))
}

/// `name` with every feature id `ids` maps replaced, the rest kept; `None` when it doesn't read.
pub fn map_features(name: &str, ids: &std::collections::BTreeMap<u64, u64>) -> Option<String> {
    let f = FaceName::parse(name).ok()?;
    let mapped = f.map(&mut |id, _| Some(ids.get(&id.0).map_or(id, |n| FeatureId(*n))), &mut |_| None)?;
    Some(mapped.to_string())
}

/// Where a feature id sits in a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// It names what made a face: a base, a nested face's base, a copy.
    Name,
    /// It names a face around a piece (`#[…]`).
    Qualifier,
}

impl FaceName {
    /// The feature whose `F<n>:` starts the name.
    pub fn made_by(&self) -> Option<FeatureId> {
        match &self.base {
            Base::Made { feature, .. } | Base::Blend { feature, .. } | Base::Inner { feature, .. } => Some(*feature),
            Base::BodyFace { .. } | Base::Other(_) => None,
        }
    }

    /// Every feature id in the name, in the order the text gives them.
    pub fn features(&self) -> Vec<FeatureId> {
        let mut out = Vec::new();
        self.push_features(&mut out);
        out
    }

    fn push_features(&self, out: &mut Vec<FeatureId>) {
        match &self.base {
            Base::Made { feature, .. } | Base::Blend { feature, rounds: Rounds::Index(_) } => out.push(*feature),
            Base::Blend { feature, rounds: Rounds::Edge(e) } => {
                out.push(*feature);
                e.faces.iter().for_each(|f| f.push_features(out));
            }
            Base::Inner { feature, offsets } => {
                out.push(*feature);
                offsets.push_features(out);
            }
            Base::BodyFace { .. } | Base::Other(_) => {}
        }
        for s in &self.suffixes {
            match s {
                Suffix::Copy { feature, .. } => out.push(*feature),
                Suffix::Piece(q) => q.around.iter().for_each(|f| f.push_features(out)),
                Suffix::Numbered(_) => {}
            }
        }
    }

    /// A fillet's or chamfer's face (a piece or copy of one too).
    pub fn is_blend(&self) -> bool {
        matches!(self.base, Base::Blend { .. })
    }

    /// A pattern's or mirror's copy of a face (a piece of one too).
    pub fn is_copy(&self) -> bool {
        self.suffixes.iter().any(|s| matches!(s, Suffix::Copy { .. }))
    }

    /// A piece told apart by a number: an older design's numbered piece, or a tie.
    pub fn is_numbered_piece(&self) -> bool {
        self.suffixes.iter().any(|s| matches!(s, Suffix::Numbered(_) | Suffix::Piece(Qualifier { tie: Some(_), .. })))
    }

    /// The name with each feature id mapped by `f` (given where it sits) and each body face's
    /// body by `body`; `None` when `f` refuses one. A role's text is left as written.
    pub fn map(&self, f: &mut impl FnMut(FeatureId, Place) -> Option<FeatureId>, body: &mut impl FnMut(&str) -> Option<String>) -> Option<FaceName> {
        self.map_in(Place::Name, f, body)
    }

    fn map_in(
        &self,
        place: Place,
        f: &mut impl FnMut(FeatureId, Place) -> Option<FeatureId>,
        body: &mut impl FnMut(&str) -> Option<String>,
    ) -> Option<FaceName> {
        let base = match &self.base {
            Base::Made { feature, role } => Base::Made { feature: f(*feature, place)?, role: role.clone() },
            Base::Blend { feature, rounds } => {
                let rounds = match rounds {
                    Rounds::Edge(e) => Rounds::Edge(Box::new(e.map_in(place, f, body)?)),
                    Rounds::Index(k) => Rounds::Index(*k),
                };
                Base::Blend { feature: f(*feature, place)?, rounds }
            }
            Base::Inner { feature, offsets } => {
                let offsets = Box::new(offsets.map_in(place, f, body)?);
                Base::Inner { feature: f(*feature, place)?, offsets }
            }
            Base::BodyFace { body: b, index } => Base::BodyFace { body: body(b).unwrap_or_else(|| b.clone()), index: *index },
            Base::Other(t) => Base::Other(t.clone()),
        };
        let mut suffixes = Vec::with_capacity(self.suffixes.len());
        for s in &self.suffixes {
            suffixes.push(match s {
                Suffix::Copy { feature, instance } => Suffix::Copy { feature: f(*feature, place)?, instance: *instance },
                Suffix::Piece(q) => {
                    let around = q.around.iter().map(|a| a.map_in(Place::Qualifier, f, body)).collect::<Option<Vec<_>>>()?;
                    Suffix::Piece(Qualifier { around, tie: q.tie })
                }
                Suffix::Numbered(k) => Suffix::Numbered(*k),
            });
        }
        Some(FaceName { base, suffixes })
    }
}

impl EdgeName {
    fn map_in(
        &self,
        place: Place,
        f: &mut impl FnMut(FeatureId, Place) -> Option<FeatureId>,
        body: &mut impl FnMut(&str) -> Option<String>,
    ) -> Option<EdgeName> {
        let [a, b] = &self.faces;
        Some(EdgeName { faces: [a.map_in(place, f, body)?, b.map_in(place, f, body)?], tie: self.tie })
    }
}

/// A plain decimal number: digits only, no leading zero (so it writes back the same).
fn number<T: std::str::FromStr>(d: &str) -> Option<T> {
    let ok = !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()) && (d.len() == 1 || !d.starts_with('0'));
    if ok { d.parse().ok() } else { None }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// `F<n>` before a `:`.
    Feature(u64),
    /// Text between separators.
    Word,
    Colon,
    Pipe,
    Comma,
    /// `#[`
    Open,
    /// `]`
    Close,
    /// `#<n>`
    Hash(u32),
    /// `~<n>`
    Tie(u32),
    /// `@F<n>.<k>`
    Copy(u64, u32),
}

#[derive(Clone, Copy, Debug)]
struct Tok {
    kind: Kind,
    start: usize,
    end: usize,
}

/// The digits of `s` from byte `i` as a number, and where they end.
fn digits_at<T: std::str::FromStr>(s: &str, i: usize) -> Option<(T, usize)> {
    let rest = s.get(i..)?;
    let n = rest.bytes().take_while(u8::is_ascii_digit).count();
    Some((number(rest.get(..n)?)?, i + n))
}

/// The separator token starting at byte `i`, if one does.
fn separator_at(s: &str, i: usize) -> Option<Tok> {
    let b = s.as_bytes();
    let tok = |kind, end| Some(Tok { kind, start: i, end });
    match b.get(i)? {
        b':' => tok(Kind::Colon, i + 1),
        b'|' => tok(Kind::Pipe, i + 1),
        b',' => tok(Kind::Comma, i + 1),
        b']' => tok(Kind::Close, i + 1),
        b'#' if b.get(i + 1) == Some(&b'[') => tok(Kind::Open, i + 2),
        b'#' => digits_at(s, i + 1).and_then(|(k, end)| tok(Kind::Hash(k), end)),
        b'~' => digits_at(s, i + 1).and_then(|(k, end)| tok(Kind::Tie(k), end)),
        b'@' if b.get(i + 1) == Some(&b'F') => {
            let (feature, dot) = digits_at(s, i + 2)?;
            if b.get(dot) != Some(&b'.') {
                return None;
            }
            let (instance, end) = digits_at(s, dot + 1)?;
            tok(Kind::Copy(feature, instance), end)
        }
        _ => None,
    }
}

fn lex(s: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if let Some(t) = separator_at(s, i) {
            toks.push(t);
            i = t.end;
            continue;
        }
        // `F<n>` right before a `:` names a feature.
        if s.as_bytes().get(i) == Some(&b'F')
            && let Some((n, end)) = digits_at::<u64>(s, i + 1)
            && s.as_bytes().get(end) == Some(&b':')
        {
            toks.push(Tok { kind: Kind::Feature(n), start: i, end });
            i = end;
            continue;
        }
        let start = i;
        while i < s.len() && separator_at(s, i).is_none() {
            i += s.get(i..).and_then(|r| r.chars().next()).map_or(1, char::len_utf8);
        }
        toks.push(Tok { kind: Kind::Word, start, end: i });
    }
    toks
}

struct Parser<'a> {
    s: &'a str,
    toks: Vec<Tok>,
    at: usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Result<Parser<'a>, NameError> {
        if s.len() > MAX_NAME_LEN {
            return Err(NameError::TooLong);
        }
        Ok(Parser { s, toks: lex(s), at: 0 })
    }

    fn peek(&self, ahead: usize) -> Option<Tok> {
        self.toks.get(self.at + ahead).copied()
    }

    fn byte(&self) -> usize {
        self.peek(0).map_or(self.s.len(), |t| t.start)
    }

    fn expected<T>(&self, what: &'static str) -> Result<T, NameError> {
        Err(NameError::Expected { at: self.byte(), what })
    }

    fn end(&self) -> Result<(), NameError> {
        if self.peek(0).is_some() { self.expected("the end of the name") } else { Ok(()) }
    }

    fn word(&self, ahead: usize) -> Option<&'a str> {
        self.peek(ahead).filter(|t| t.kind == Kind::Word).and_then(|t| self.s.get(t.start..t.end))
    }

    /// `<word>:` at the cursor.
    fn keyword(&self, w: &str) -> bool {
        self.word(0) == Some(w) && self.peek(1).is_some_and(|t| t.kind == Kind::Colon)
    }

    fn edge(&mut self, depth: usize) -> Result<EdgeName, NameError> {
        let a = self.face(depth + 1)?;
        if self.peek(0).map(|t| t.kind) != Some(Kind::Pipe) {
            return self.expected("`|` between an edge's two faces");
        }
        self.at += 1;
        let b = self.face(depth + 1)?;
        let tie = match self.peek(0).map(|t| t.kind) {
            Some(Kind::Tie(k)) => {
                self.at += 1;
                Some(k)
            }
            _ => None,
        };
        Ok(EdgeName { faces: [a, b], tie })
    }

    fn face(&mut self, depth: usize) -> Result<FaceName, NameError> {
        if depth > MAX_DEPTH {
            return Err(NameError::TooDeep);
        }
        let mut face = match (self.peek(0).map(|t| t.kind), self.peek(1).map(|t| t.kind)) {
            (Some(Kind::Feature(n)), Some(Kind::Colon)) => {
                self.at += 2;
                let feature = FeatureId(n);
                if self.keyword("blend") {
                    self.at += 2;
                    self.blend(feature, depth)?
                } else if self.keyword("inner") {
                    self.at += 2;
                    let mut offsets = self.face(depth + 1)?;
                    let moved = later_suffixes(feature, &mut offsets);
                    FaceName { base: Base::Inner { feature, offsets: Box::new(offsets) }, suffixes: moved }
                } else {
                    let role = Role::from_text(self.text()?);
                    FaceName { base: Base::Made { feature, role }, suffixes: Vec::new() }
                }
            }
            _ => {
                let t = self.text()?;
                let body_face = t.rsplit_once(":face:").and_then(|(b, k)| Some((b, number(k)?))).filter(|(b, _)| !b.is_empty());
                let base = match body_face {
                    Some((body, index)) => Base::BodyFace { body: body.to_string(), index },
                    None => Base::Other(t.to_string()),
                };
                FaceName { base, suffixes: Vec::new() }
            }
        };
        self.suffixes(&mut face, depth)?;
        Ok(face)
    }

    /// A blend face of `feature`, the cursor after `blend:`.
    fn blend(&mut self, feature: FeatureId, depth: usize) -> Result<FaceName, NameError> {
        let ends_face = |k: Option<Kind>| !matches!(k, Some(Kind::Word | Kind::Colon | Kind::Feature(_)));
        if let Some(k) = self.word(0).and_then(number::<u32>)
            && ends_face(self.peek(1).map(|t| t.kind))
        {
            self.at += 1;
            return Ok(FaceName { base: Base::Blend { feature, rounds: Rounds::Index(k) }, suffixes: Vec::new() });
        }
        let mut edge = self.edge(depth + 1)?;
        let [_, b] = &mut edge.faces;
        let moved = later_suffixes(feature, b);
        Ok(FaceName { base: Base::Blend { feature, rounds: Rounds::Edge(Box::new(edge)) }, suffixes: moved })
    }

    /// Words and colons up to the next other token, as written.
    fn text(&mut self) -> Result<&'a str, NameError> {
        let start = self.byte();
        let mut end = start;
        while let Some(t) = self.peek(0).filter(|t| matches!(t.kind, Kind::Word | Kind::Colon | Kind::Feature(_))) {
            end = t.end;
            self.at += 1;
        }
        match self.s.get(start..end).filter(|t| !t.is_empty()) {
            Some(t) => Ok(t),
            None => self.expected("a face"),
        }
    }

    fn suffixes(&mut self, face: &mut FaceName, depth: usize) -> Result<(), NameError> {
        while let Some(t) = self.peek(0) {
            match t.kind {
                Kind::Copy(feature, instance) => {
                    self.at += 1;
                    face.suffixes.push(Suffix::Copy { feature: FeatureId(feature), instance });
                }
                Kind::Open => {
                    self.at += 1;
                    let mut around = vec![self.face(depth + 1)?];
                    loop {
                        match self.peek(0).map(|t| t.kind) {
                            Some(Kind::Comma) => {
                                self.at += 1;
                                around.push(self.face(depth + 1)?);
                            }
                            Some(Kind::Close) => {
                                self.at += 1;
                                break;
                            }
                            _ => return self.expected("`,` or `]` in a piece's faces"),
                        }
                    }
                    let tie = match self.peek(0).map(|t| t.kind) {
                        Some(Kind::Hash(k)) => {
                            self.at += 1;
                            Some(k)
                        }
                        _ => None,
                    };
                    face.suffixes.push(Suffix::Piece(Qualifier { around, tie }));
                }
                Kind::Hash(k) => {
                    self.at += 1;
                    face.suffixes.push(Suffix::Numbered(k));
                }
                _ => return Ok(()),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(s: &str) -> FaceName {
        let f = FaceName::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"));
        assert_eq!(f.to_string(), s, "writes back");
        f
    }

    fn edge(s: &str) -> EdgeName {
        let e = EdgeName::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"));
        assert_eq!(e.to_string(), s, "writes back");
        e
    }

    fn made(feature: u64, role: Role) -> FaceName {
        FaceName { base: Base::Made { feature: FeatureId(feature), role }, suffixes: Vec::new() }
    }

    fn other(role: &str) -> Role {
        Role::Other(role.into())
    }

    #[test]
    fn roles_are_typed_and_the_rest_kept_as_written() {
        assert_eq!(face("F2:top"), made(2, Role::Top));
        assert_eq!(face("F4:side#1").stem(), made(4, Role::Wall));
        assert_eq!(face("F2:side:l4"), made(2, Role::Side { profile: None, curve: "l4".into() }));
        assert_eq!(face("F2:side:s1.l4"), made(2, Role::Side { profile: Some(1), curve: "l4".into() }));
        assert_eq!(face("F3:+x"), made(3, Role::BoxFace { axis: Axis::X, high: true }));
        assert_eq!(face("F3:-z"), made(3, Role::BoxFace { axis: Axis::Z, high: false }));
        assert_eq!(face("F2:face:7"), made(2, Role::Index(7)));
        assert_eq!(face("F9:cap:high:0"), made(9, other("cap:high:0")));
        assert_eq!(face("F12:boss:side:3"), made(12, other("boss:side:3")));
        assert_eq!(face("Body1:face:3").base, Base::BodyFace { body: "Body1".into(), index: 3 });
        assert_eq!(face("Plate:rim.end").base, Base::Other("Plate:rim.end".into()));
        // Numbers that would not write back the same stay text.
        assert_eq!(face("F2:side:s01.l4"), made(2, Role::Side { profile: None, curve: "s01.l4".into() }));
    }

    #[test]
    fn pieces_copies_and_numbered_pieces() {
        let p = face("F2:top#[F2:side:l2,F4:side:l2]");
        let Some(Suffix::Piece(q)) = p.suffixes.first() else { panic!("{p:?}") };
        assert_eq!(q.around().len(), 2);
        assert_eq!(p.stem(), made(2, Role::Top));
        assert!(matches!(face("F2:top#[F4:side:l2]#1").suffixes.as_slice(), [Suffix::Piece(Qualifier { tie: Some(1), .. })]));
        assert_eq!(face("F2:top#2").suffixes, vec![Suffix::Numbered(2)]);
        let c = face("F2:side:l1@F7.2");
        assert_eq!(c.suffixes, vec![Suffix::Copy { feature: FeatureId(7), instance: 2 }]);
        assert_eq!(c.feature(), Some(FeatureId(7)));
        assert_eq!(face("F2:side:l1@F7.2#[F8:x]").stem(), c);
        assert!(Qualifier::new(Vec::new(), None).is_none());
    }

    /// The fillet rounds an edge of a piece the slot made before it: the qualifier is the
    /// piece's (corpus `naming-split-pieces`).
    #[test]
    fn a_blend_of_an_earlier_piece_keeps_the_qualifier_inside() {
        let f = face("F5:blend:F2:side:l2|F2:top#[F2:side:l2,F4:side:l2]");
        let Base::Blend { feature: FeatureId(5), rounds: Rounds::Edge(e) } = &f.base else { panic!("{f:?}") };
        assert!(f.suffixes.is_empty());
        assert_eq!(e.faces[1].stem(), made(2, Role::Top));
        assert_eq!(e.faces[1].suffixes.len(), 1);
    }

    /// A slot cut after the fillet splits the fillet face: the qualifier, naming the later slot,
    /// is the fillet face's.
    #[test]
    fn a_blend_split_by_a_later_cut_takes_the_qualifier() {
        let f = face("F4:blend:F3:+x|F3:+z#[F3:-y,F5:-y]");
        let Base::Blend { feature: FeatureId(4), rounds: Rounds::Edge(e) } = &f.base else { panic!("{f:?}") };
        assert_eq!(e.faces[1], made(3, Role::BoxFace { axis: Axis::Z, high: true }));
        assert!(matches!(f.suffixes.as_slice(), [Suffix::Piece(_)]));
        // Copied by a later pattern, then split: both are the blend face's, in order.
        let g = face("F4:blend:F3:+x|F3:+z@F6.1#[F7:a]");
        assert!(matches!(g.suffixes.as_slice(), [Suffix::Copy { feature: FeatureId(6), .. }, Suffix::Piece(_)]));
        assert_eq!(g.feature(), Some(FeatureId(6)));
    }

    /// What the text cannot tell: a later cut told apart only by older faces reads as a piece of
    /// the inner face.
    #[test]
    fn a_later_cut_told_apart_by_older_faces_reads_as_the_inner_faces() {
        let f = face("F4:blend:F3:+x|F3:+z#[F3:-y]");
        let Base::Blend { rounds: Rounds::Edge(e), .. } = &f.base else { panic!("{f:?}") };
        assert!(f.suffixes.is_empty() && e.faces[1].suffixes.len() == 1);
    }

    #[test]
    fn edges_split_into_their_two_faces() {
        let e = edge("F2:side:l1|F2:side:l2");
        assert_eq!(e.faces[0], made(2, Role::Side { profile: None, curve: "l1".into() }));
        assert_eq!(edge("F2:side:l1|F2:top~1").tie, Some(1));
        // The blend face first: the whole blend name, then the slot's wall.
        let e = edge("F4:blend:F3:+x|F3:+z#[F3:-y,F5:-y]|F5:-y");
        assert_eq!(e.faces[0].to_string(), "F4:blend:F3:+x|F3:+z#[F3:-y,F5:-y]");
        assert_eq!(e.faces[1], made(5, Role::BoxFace { axis: Axis::Y, high: false }));
        // The blend face second.
        let e = edge("F2:side:l1|F5:blend:F2:side:l2|F2:top#[F2:side:l2,F4:side:l2]");
        assert!(matches!(e.faces[1].base, Base::Blend { feature: FeatureId(5), .. }));
        // A blend of a blend's edge.
        let e = edge("F6:blend:F4:blend:a|b|c|d");
        let Base::Blend { rounds: Rounds::Edge(inner), .. } = &e.faces[0].base else { panic!("{e:?}") };
        assert!(matches!(inner.faces[0].base, Base::Blend { feature: FeatureId(4), .. }));
        assert_eq!(e.faces[1].base, Base::Other("d".into()));
        // A `~k` ending the edge numbers it, not the blend's edge.
        let e = edge("F2:x|F5:blend:a|b~1");
        assert_eq!(e.tie, Some(1));
        assert!(matches!(&e.faces[1].base, Base::Blend { rounds: Rounds::Edge(b), .. } if b.tie.is_none()));
        // A blend named by its edge's index.
        assert_eq!(edge("F5:blend:3|F2:top").faces[0].base, Base::Blend { feature: FeatureId(5), rounds: Rounds::Index(3) });
    }

    #[test]
    fn inner_walls_nest_a_face() {
        let f = face("F3:inner:F2:top#[F2:side:l2]");
        let Base::Inner { feature: FeatureId(3), offsets } = &f.base else { panic!("{f:?}") };
        assert_eq!(offsets.stem(), made(2, Role::Top));
    }

    #[test]
    fn what_made_a_face_and_its_kind() {
        assert_eq!(made_by("F2:side:l1@F7.2"), Some(2));
        assert_eq!(made_by("F3:inner:F5:blend:a|b"), Some(3));
        assert_eq!(made_by("Body1:face:3"), None);
        assert_eq!(split_feature("F12:boss:side:3"), Some((12, "boss:side:3")));
        // A blend named only inside another name is not one.
        assert!(is_blend("F5:blend:F2:x|F2:y#[F6:z]"));
        assert!(!is_blend("F2:top#[F5:blend:a|b]"));
        assert!(!is_blend("F3:inner:F5:blend:a|b"));
        // A copy only inside a blend's edge doesn't make the blend face a copy.
        assert!(is_copy("F2:side:l1@F7.2#[F8:x]"));
        assert!(!is_copy("F5:blend:F2:x@F3.1|F2:y"));
        assert!(is_numbered_piece("F2:top#2") && is_numbered_piece("F2:top#[F4:x]#1"));
        assert!(!is_numbered_piece("F5:blend:F2:top#2|F2:x") && !is_numbered_piece("F2:top#[F4:x]"));
        assert!(is_wall("F4:side") && !is_wall("F4:side#1") && !is_wall("F5:blend:F2:x|F4:side"));
    }

    /// A shell's inner wall split by a later cut: the qualifier, naming the cut, is the wall's.
    #[test]
    fn an_inner_wall_split_by_a_later_cut_takes_the_qualifier() {
        let f = face("F3:inner:F2:top#[F2:side:l2,F6:x]");
        assert!(matches!(f.suffixes.as_slice(), [Suffix::Piece(_)]));
        assert_eq!(f.stem().to_string(), "F3:inner:F2:top");
    }

    #[test]
    fn features_in_text_order() {
        let ids = |s: &str| face(s).features().into_iter().map(|f| f.0).collect::<Vec<_>>();
        assert_eq!(ids("F3:inner:F2:blend:F1:+y|F1:-x"), vec![3, 2, 1, 1]);
        assert_eq!(ids("F82:-z@F84.2"), vec![82, 84]);
        assert_eq!(ids("F2:top#[F2:side:l2,F4:side:l2]"), vec![2, 2, 4]);
        assert_eq!(ids("F1:side:曲線F10"), vec![1]);
        assert!(ids("Body1:face:3").is_empty());
    }

    #[test]
    fn feature_ids_map_everywhere_but_role_text() {
        let ids = std::collections::BTreeMap::from([(2, 12), (5, 15)]);
        let mapped = map_features("F5:blend:F2:side:l2|F2:top#[F2:side:l2,F4:side:l2]@F2.1", &ids);
        assert_eq!(mapped.as_deref(), Some("F15:blend:F12:side:l2|F12:top#[F12:side:l2,F4:side:l2]@F12.1"));
        // Where an id sits: a name's ids must map, a qualifier's may stay.
        let f = face("F2:top#[F9:x]");
        let strict = f.map(
            &mut |id, place| match place {
                Place::Name => ids.get(&id.0).map(|n| FeatureId(*n)),
                Place::Qualifier => Some(ids.get(&id.0).map_or(id, |n| FeatureId(*n))),
            },
            &mut |_| None,
        );
        assert_eq!(strict.map(|f| f.to_string()).as_deref(), Some("F12:top#[F9:x]"));
        assert!(face("F9:top").map(&mut |id, _| ids.get(&id.0).map(|n| FeatureId(*n)), &mut |_| None).is_none());
        let body = face("Body1:face:3").map(&mut |id, _| Some(id), &mut |b| (b == "Body1").then(|| "Body2".to_string()));
        assert_eq!(body.map(|f| f.to_string()).as_deref(), Some("Body2:face:3"));
    }

    #[test]
    fn errors_say_where() {
        assert_eq!(EdgeName::parse("F2:top"), Err(NameError::Expected { at: 6, what: "`|` between an edge's two faces" }));
        assert_eq!(FaceName::parse("F2:top#[F3:x"), Err(NameError::Expected { at: 12, what: "`,` or `]` in a piece's faces" }));
        assert_eq!(FaceName::parse("a|b"), Err(NameError::Expected { at: 1, what: "the end of the name" }));
        assert_eq!(FaceName::parse(""), Err(NameError::Expected { at: 0, what: "a face" }));
    }

    #[test]
    fn hostile_names_never_panic() {
        let long = "F1:".repeat(MAX_NAME_LEN);
        let deep = format!("{}x", "F1:inner:".repeat(64));
        let deep_pieces = format!("x{}{}", "#[x".repeat(64), "]".repeat(64));
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
            "F99999999999999999999999:top",
            "x#99999999999",
            "\u{1F600}|\u{1F601}",
            long.as_str(),
            deep.as_str(),
            deep_pieces.as_str(),
        ] {
            if let Ok(f) = FaceName::parse(s) {
                assert_eq!(f.to_string(), s);
            }
            if let Ok(e) = EdgeName::parse(s) {
                assert_eq!(e.to_string(), s);
            }
        }
        assert_eq!(FaceName::parse(&long), Err(NameError::TooLong));
        assert_eq!(FaceName::parse(&deep), Err(NameError::TooDeep));
        assert_eq!(FaceName::parse(&deep_pieces), Err(NameError::TooDeep));
        assert!(EdgeName::parse("x|y|z").is_err());
    }
}

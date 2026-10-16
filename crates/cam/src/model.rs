use crate::{MAX_ITEMS, Result, TOLERANCE, invalid};
use serde::{Deserialize, Serialize};
use solidcraft_geom::Vec3;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manufacturing {
    #[serde(default)]
    pub setups: Vec<Setup>,
    #[serde(default)]
    pub tools: Vec<Tool>,
    #[serde(default)]
    pub operations: Vec<Operation>,
    #[serde(default)]
    pub programs: Vec<Program>,
}
impl Manufacturing {
    pub fn is_empty(&self) -> bool {
        self.setups.is_empty() && self.tools.is_empty() && self.operations.is_empty() && self.programs.is_empty()
    }
    pub fn validate(&self) -> Result<()> {
        for ids in [
            self.setups.iter().map(|x| x.id).collect::<Vec<_>>(),
            self.tools.iter().map(|x| x.id).collect(),
            self.operations.iter().map(|x| x.id).collect(),
            self.programs.iter().map(|x| x.id).collect(),
        ] {
            if ids.len() > MAX_ITEMS || ids.contains(&0) || ids.iter().collect::<std::collections::BTreeSet<_>>().len() != ids.len() {
                return invalid("too many items or duplicate/zero IDs");
            }
        }
        for s in &self.setups {
            s.validate()?;
        }
        for t in &self.tools {
            t.validate()?;
        }
        for o in &self.operations {
            let s = self.setups.iter().find(|s| s.id == o.setup).ok_or_else(|| crate::Error("missing setup".into()))?;
            let t = self.tools.iter().find(|t| t.id == o.tool).ok_or_else(|| crate::Error("missing tool".into()))?;
            o.validate(s, t)?;
        }
        for p in &self.programs {
            name(&p.name)?;
            if p.operations.is_empty()
                || p.operations.len() > MAX_ITEMS
                || p.operations.iter().collect::<std::collections::BTreeSet<_>>().len() != p.operations.len()
            {
                return invalid("program needs unique ordered operations");
            }
            for id in &p.operations {
                if !self.operations.iter().any(|o| o.id == *id && o.setup == p.setup) {
                    return invalid("program operation is missing or belongs to another setup");
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyRef {
    pub body: String,
    #[serde(default)]
    pub occurrence: Vec<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub origin: Vec3,
    pub x: Vec3,
    pub y: Vec3,
    pub z: Vec3,
}
impl Default for Frame {
    fn default() -> Self {
        Self { origin: Vec3::ZERO, x: Vec3::X, y: Vec3::Y, z: Vec3::Z }
    }
}
impl Frame {
    pub fn validate(&self) -> Result<()> {
        for v in [self.origin, self.x, self.y, self.z] {
            point(v)?;
        }
        if (self.x.dot(self.x) - 1.0).abs() > 1e-8
            || (self.y.dot(self.y) - 1.0).abs() > 1e-8
            || (self.z.dot(self.z) - 1.0).abs() > 1e-8
            || self.x.dot(self.y).abs() > 1e-8
            || self.x.cross(self.y).dist(self.z) > 1e-8
        {
            return invalid("setup frame must be right-handed and orthonormal");
        }
        Ok(())
    }
    pub fn local(&self, p: Vec3) -> Vec3 {
        let p = p - self.origin;
        Vec3::new(p.dot(self.x), p.dot(self.y), p.dot(self.z))
    }
    pub fn world(&self, p: Vec3) -> Vec3 {
        self.origin + self.x * p.x + self.y * p.y + self.z * p.z
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stock {
    pub min: Vec3,
    pub max: Vec3,
}
impl Stock {
    pub fn validate(&self) -> Result<()> {
        point(self.min)?;
        point(self.max)?;
        if self.min.x >= self.max.x || self.min.y >= self.max.y || self.min.z >= self.max.z {
            return invalid("stock bounds must have positive size");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    pub id: u64,
    pub name: String,
    pub stock: Stock,
    #[serde(default)]
    pub frame: Frame,
    pub models: Vec<BodyRef>,
    #[serde(default)]
    pub fixtures: Vec<BodyRef>,
    pub clearance: f64,
    pub retract: f64,
    #[serde(default = "work_offset")]
    pub work_offset: u8,
}
fn work_offset() -> u8 {
    54
}
impl Setup {
    pub fn validate(&self) -> Result<()> {
        name(&self.name)?;
        self.stock.validate()?;
        self.frame.validate()?;
        if self.models.is_empty() || self.models.len() + self.fixtures.len() > MAX_ITEMS {
            return invalid("setup needs model references (at most 1000)");
        }
        for r in self.models.iter().chain(&self.fixtures) {
            name(&r.body)?;
            if r.occurrence.len() > 64 {
                return invalid("occurrence path too deep");
            }
        }
        if !finite(self.clearance)
            || !finite(self.retract)
            || self.clearance < self.retract
            || self.retract <= self.stock.max.z
            || !(54..=59).contains(&self.work_offset)
        {
            return invalid("clearance >= retract > stock top; work offset must be G54–G59");
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    FlatEndMill,
    TwistDrill,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub id: u64,
    pub name: String,
    pub number: u32,
    pub kind: ToolKind,
    pub diameter: f64,
    pub cutting_length: f64,
    pub shank_diameter: f64,
    pub overall_length: f64,
    pub holder_diameter: f64,
    pub holder_length: f64,
    #[serde(default = "drill_angle")]
    pub tip_angle: f64,
}
fn drill_angle() -> f64 {
    118.0_f64.to_radians()
}
impl Tool {
    pub fn validate(&self) -> Result<()> {
        name(&self.name)?;
        if self.number == 0
            || self.number > 9999
            || [self.diameter, self.cutting_length, self.shank_diameter, self.overall_length, self.holder_diameter, self.holder_length]
                .iter()
                .any(|v| !positive(*v))
            || self.overall_length < self.cutting_length
            || !self.tip_angle.is_finite()
            || !(0.1..3.1).contains(&self.tip_angle)
        {
            return invalid("invalid tool dimensions, number or drill angle (radians)");
        }
        Ok(())
    }
    pub fn tip_length(&self) -> f64 {
        if self.kind == ToolKind::TwistDrill { self.diameter * 0.5 / (self.tip_angle * 0.5).tan() } else { 0.0 }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Boundary {
    pub outer: Vec<[f64; 2]>,
    #[serde(default)]
    pub islands: Vec<Vec<[f64; 2]>>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileRef {
    pub sketch: u64,
    pub curves: Vec<String>,
    #[serde(default)]
    pub occurrence: Vec<u64>,
}
/// Document-local identities only. The engine resolves these into setup XY centers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HoleSource {
    Face { target: BodyRef, name: String },
    Profile { profile: ProfileRef },
}
impl HoleSource {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Face { target, name: face } => {
                name(&target.body)?;
                name(face)?;
                if target.occurrence.len() > 64 {
                    return invalid("hole occurrence path too deep");
                }
            }
            Self::Profile { profile } => {
                if profile.sketch == 0 || profile.curves.is_empty() || profile.curves.len() > 10_000 || profile.occurrence.len() > 64 {
                    return invalid("invalid circular profile reference");
                }
                for curve in &profile.curves {
                    name(curve)?;
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case", deny_unknown_fields)]
pub enum Strategy {
    Face,
    Contour {
        boundary: Boundary,
        inside: bool,
    },
    Pocket {
        boundary: Boundary,
    },
    Drill {
        holes: Vec<[f64; 2]>,
        #[serde(default)]
        peck: Option<f64>,
        #[serde(default)]
        full_diameter: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub id: u64,
    pub name: String,
    pub setup: u64,
    pub tool: u64,
    pub geometry: Strategy,
    #[serde(default)]
    pub source: Option<ProfileRef>,
    /// When nonempty, replaces every preview XY center during snapshot preparation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hole_sources: Vec<HoleSource>,
    pub top: f64,
    pub bottom: f64,
    pub stepdown: f64,
    pub stepover: f64,
    pub feed: f64,
    pub plunge_feed: f64,
    pub spindle: f64,
    #[serde(default)]
    pub allowance: f64,
    #[serde(default = "yes")]
    pub climb: bool,
    #[serde(default = "tolerance")]
    pub tolerance: f64,
    #[serde(default)]
    pub coolant: bool,
    /// Optional entry ramp angle in radians; None uses a center-cutting plunge.
    #[serde(default)]
    pub ramp_angle: Option<f64>,
    #[serde(default)]
    pub lead_distance: f64,
}
fn yes() -> bool {
    true
}
fn tolerance() -> f64 {
    TOLERANCE
}
impl Operation {
    pub fn validate(&self, s: &Setup, t: &Tool) -> Result<()> {
        name(&self.name)?;
        if !finite(self.lead_distance)
            || self.lead_distance < 0.0
            || self.ramp_angle.is_some_and(|a| !a.is_finite() || !(0.001..=0.174533).contains(&a))
        {
            return invalid("lead must be nonnegative; ramp angle must be 0.06–10 degrees");
        }
        if (self.ramp_angle.is_some() || self.lead_distance > 0.0) && matches!(self.geometry, Strategy::Face | Strategy::Drill { .. }) {
            return invalid("lead/ramp entry is supported for contour and pocket operations");
        }
        if let Some(source) = &self.source {
            if source.sketch == 0
                || source.curves.is_empty()
                || source.curves.len() > 10_000
                || source.occurrence.len() > 64
                || !matches!(self.geometry, Strategy::Contour { .. } | Strategy::Pocket { .. })
            {
                return invalid("profile association requires a bounded contour/pocket reference");
            }
            for c in &source.curves {
                name(c)?;
            }
        }
        if !self.hole_sources.is_empty() {
            let Strategy::Drill { holes, .. } = &self.geometry else {
                return invalid("hole references require drilling");
            };
            if self.hole_sources.len() > MAX_ITEMS || self.hole_sources.len() != holes.len() {
                return invalid("each linked hole needs exactly one source reference");
            }
            for (index, source) in self.hole_sources.iter().enumerate() {
                source.validate()?;
                if self.hole_sources.iter().take(index).any(|old| old == source) {
                    return invalid("duplicate hole reference");
                }
            }
        }
        if [self.stepdown, self.stepover, self.feed, self.plunge_feed, self.spindle, self.tolerance].iter().any(|v| !positive(*v))
            || !finite(self.top)
            || !finite(self.bottom)
            || !finite(self.allowance)
            || self.allowance < 0.0
            || self.bottom >= self.top
            || self.top > s.stock.max.z
            || self.top < s.stock.min.z
            || self.stepover > t.diameter
            || self.top - self.bottom + t.tip_length() > t.cutting_length
            || (self.top - self.bottom) / self.stepdown > 10_000.0
            || self.tolerance < 1e-5
            || self.tolerance > 1.0
        {
            return invalid("invalid heights, feeds, step sizes, tolerance or cutting reach");
        }
        match &self.geometry {
            Strategy::Drill { holes, peck, .. } => {
                if t.kind != ToolKind::TwistDrill
                    || self.allowance != 0.0
                    || holes.is_empty()
                    || holes.len() > MAX_ITEMS
                    || peck.is_some_and(|v| !positive(v) || (self.top - self.bottom) / v > 10_000.0)
                {
                    return invalid(
                        "drilling needs a drill, bounded holes, positive peck and zero stock allowance; set the required depth explicitly",
                    );
                }
                for p in holes {
                    point(Vec3::new(p[0], p[1], 0.0))?;
                }
            }
            Strategy::Contour { boundary, .. } | Strategy::Pocket { boundary } => {
                if t.kind != ToolKind::FlatEndMill {
                    return invalid("milling needs a flat end mill");
                }
                boundary.validate()?;
            }
            Strategy::Face => {
                if t.kind != ToolKind::FlatEndMill {
                    return invalid("facing needs a flat end mill");
                }
            }
        }
        Ok(())
    }
}
impl Boundary {
    pub fn validate(&self) -> Result<()> {
        if self.islands.len() > 100 {
            return invalid("too many islands");
        }
        let mut total = 0;
        for ring in std::iter::once(&self.outer).chain(&self.islands) {
            total += ring.len();
            if ring.len() < 3 || total > 10_000 {
                return invalid("boundary needs 3–10000 vertices");
            }
            for p in ring {
                point(Vec3::new(p[0], p[1], 0.0))?;
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Program {
    pub id: u64,
    pub name: String,
    pub setup: u64,
    pub operations: Vec<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Geometry {
    pub target: Vec<[Vec3; 3]>,
    pub fixtures: Vec<[Vec3; 3]>,
    /// Triangle counts for consecutive fixture bodies. Shells of one body share
    /// a winding sum so enclosed cavities remain empty. Legacy snapshots contain
    /// one triangle soup; an empty list treats that soup as one body.
    #[serde(default)]
    pub fixture_groups: Vec<usize>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobInput {
    pub setup: Setup,
    pub tools: Vec<Tool>,
    pub operations: Vec<Operation>,
    pub geometry: Geometry,
    pub fingerprint: String,
}
impl JobInput {
    pub fn validate(&self) -> Result<()> {
        let manufacturing = Manufacturing {
            setups: vec![self.setup.clone()],
            tools: self.tools.clone(),
            operations: self.operations.clone(),
            programs: vec![Program {
                id: 1,
                name: "Snapshot".into(),
                setup: self.setup.id,
                operations: self.operations.iter().map(|o| o.id).collect(),
            }],
        };
        manufacturing.validate()?;
        if self.tools.iter().map(|t| t.number).collect::<std::collections::BTreeSet<_>>().len() != self.tools.len() {
            return invalid("ambiguous tool numbers");
        }
        if self.geometry.target.is_empty() || self.geometry.target.len() + self.geometry.fixtures.len() > 200_000 {
            return invalid("empty or excessive verification geometry");
        }
        if !self.geometry.fixture_groups.is_empty()
            && (self.geometry.fixture_groups.len() > MAX_ITEMS
                || self.geometry.fixture_groups.contains(&0)
                || self.geometry.fixture_groups.iter().try_fold(0usize, |sum, n| sum.checked_add(*n)) != Some(self.geometry.fixtures.len()))
        {
            return invalid("invalid fixture body triangle groups");
        }
        for triangle in self.geometry.target.iter().chain(&self.geometry.fixtures) {
            for p in triangle {
                point(*p)?;
            }
        }
        for p in self.geometry.target.iter().flatten() {
            let a = self.setup.stock.min;
            let b = self.setup.stock.max;
            if p.x < a.x - TOLERANCE
                || p.y < a.y - TOLERANCE
                || p.z < a.z - TOLERANCE
                || p.x > b.x + TOLERANCE
                || p.y > b.y + TOLERANCE
                || p.z > b.z + TOLERANCE
            {
                return invalid("rectangular stock must enclose selected model geometry");
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Motion {
    Rapid { to: Vec3 },
    Linear { to: Vec3, feed: f64 },
    Arc { to: Vec3, center: Vec3, clockwise: bool, feed: f64 },
    Tool { tool: u64, number: u32 },
    Spindle { rpm: f64 },
    Coolant { on: bool },
    Stop,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub operation: u64,
    pub motion: Motion,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Toolpath {
    pub fingerprint: String,
    pub blocks: Vec<Block>,
}
pub(crate) fn finite(v: f64) -> bool {
    v.is_finite() && v.abs() <= 1e6
}
pub(crate) fn positive(v: f64) -> bool {
    finite(v) && v > 0.0
}
pub(crate) fn point(v: Vec3) -> Result<()> {
    if [v.x, v.y, v.z].into_iter().all(finite) { Ok(()) } else { invalid("non-finite or excessive coordinate") }
}
fn name(s: &str) -> Result<()> {
    if s.trim().is_empty() || s.len() > 256 || s.chars().any(char::is_control) { invalid("invalid name") } else { Ok(()) }
}

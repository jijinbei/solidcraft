//! Quantities and geometry with their units in their types. Lengths are millimetres, angles
//! radians. In JSON a quantity is its number, a point or direction its `[x, y, z]`.

use serde::{Deserialize, Serialize};

/// A length in millimetres.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Mm(pub f64);

/// An angle in radians.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Radians(pub f64);

/// A position in space (mm).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Point(pub [f64; 3]);

/// A position in a sketch's plane (mm).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Point2(pub [f64; 2]);

/// A unit vector. Made from any finite vector with a length ([`Direction::new`]); read from JSON
/// the same way, so a zero or non-finite vector is refused.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "[f64; 3]", into = "[f64; 3]")]
pub struct Direction([f64; 3]);

impl Direction {
    pub const X: Direction = Direction([1.0, 0.0, 0.0]);
    pub const Y: Direction = Direction([0.0, 1.0, 0.0]);
    pub const Z: Direction = Direction([0.0, 0.0, 1.0]);

    /// `v` scaled to unit length; `None` for a vector with no length or that is not finite.
    pub fn new(v: [f64; 3]) -> Option<Direction> {
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        (v.iter().all(|c| c.is_finite()) && n.is_finite() && n > 1e-300).then(|| Direction(v.map(|c| c / n + 0.0)))
    }

    pub fn get(self) -> [f64; 3] {
        self.0
    }
}

impl Default for Direction {
    fn default() -> Self {
        Direction::Z
    }
}

impl TryFrom<[f64; 3]> for Direction {
    type Error = String;

    fn try_from(v: [f64; 3]) -> Result<Self, String> {
        Direction::new(v).ok_or_else(|| format!("{v:?} is not a direction (a finite vector with a length)"))
    }
}

impl From<Direction> for [f64; 3] {
    fn from(d: Direction) -> Self {
        d.0
    }
}

/// A plane: the points `p` with `(p − point) · normal = 0`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Plane {
    pub point: Point,
    pub normal: Direction,
}

impl Plane {
    pub fn new(point: Point, normal: Direction) -> Plane {
        Plane { point, normal }
    }
}

/// A plane with axes: a sketch's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Frame {
    pub origin: Point,
    pub x: Direction,
    pub y: Direction,
    pub normal: Direction,
}

/// A rigid or mirroring transform as a row-major 4×4 matrix (the last column the translation, mm).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Transform(pub [[f64; 4]; 4]);

impl Default for Transform {
    fn default() -> Self {
        Transform([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }
}

/// A number with what it measures. JSON: `{"length": 3.0}`, `{"angle": 0.5}`, `{"number": 4}`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Quantity {
    Length(Mm),
    Angle(Radians),
    /// A count, a factor or another unit-less number.
    Number(f64),
}

/// The length unit of a snapshot: always millimetres.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LengthUnit {
    #[default]
    Mm,
}

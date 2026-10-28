//! What a snapshot is taken with: parameter overrides, the chord tolerance, a body filter, edits.

use crate::schema::{BodyName, Edit, ParameterName, Quantity};

/// A new value for a parameter.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Override {
    /// A length (mm), angle (radians) or number, of the parameter's kind.
    Value(Quantity),
    /// An expression as the parameters table takes it (`25 mm`, `width / 2`, a quoted text).
    Expression(String),
}

impl From<Quantity> for Override {
    fn from(q: Quantity) -> Self {
        Override::Value(q)
    }
}

impl From<&str> for Override {
    fn from(e: &str) -> Self {
        Override::Expression(e.to_string())
    }
}

/// How closely the meshes follow curved faces: the largest distance between a triangle and its
/// face (chord tolerance).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Tolerance {
    /// A share of each body's size (its box diagonal), within [`crate::RELATIVE_TOLERANCE`];
    /// never finer than [`crate::MIN_TOLERANCE_MM`].
    Relative(f64),
    /// Millimetres, from [`crate::MIN_TOLERANCE_MM`] to [`crate::MAX_TOLERANCE_MM`].
    Absolute(f64),
}

impl Default for Tolerance {
    fn default() -> Self {
        Tolerance::Relative(crate::DEFAULT_RELATIVE_TOLERANCE)
    }
}

/// What a snapshot contains and is taken with.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Options {
    pub overrides: Vec<(ParameterName, Override)>,
    pub tolerance: Tolerance,
    /// Only these bodies; all when `None`.
    pub bodies: Option<Vec<BodyName>>,
    pub edits: Vec<Edit>,
}

impl Options {
    /// The options with this parameter overridden (after the overrides already given).
    pub fn with_override(mut self, name: impl Into<ParameterName>, value: impl Into<Override>) -> Self {
        self.overrides.push((name.into(), value.into()));
        self
    }

    pub fn with_tolerance(mut self, tolerance: Tolerance) -> Self {
        self.tolerance = tolerance;
        self
    }

    pub fn with_bodies(mut self, bodies: Vec<BodyName>) -> Self {
        self.bodies = Some(bodies);
        self
    }

    pub fn with_edits(mut self, edits: Vec<Edit>) -> Self {
        self.edits = edits;
        self
    }
}

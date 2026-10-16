//! Explicit hole picks and immutable resolution of their persistent identities.
use super::snapshot::{PlacedBody, Profile};
use crate::{
    EngineError, Result, Session, cam,
    geom::{Seg2, Vec3},
};
use serde::{Deserialize, Serialize};
use solidcraft_doc::apply_point;

fn bad(message: &str) -> EngineError {
    EngineError::Other(message.into())
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Pick {
    Face {
        body: String,
        index: usize,
        #[serde(default)]
        occurrence: Option<Vec<u64>>,
    },
    Profile {
        sketch: u64,
        index: usize,
        #[serde(default)]
        occurrence: Vec<u64>,
    },
}
#[derive(Serialize)]
pub(crate) struct Selected {
    pub center: [f64; 2],
    pub diameter: f64,
}
pub(super) enum Hole {
    Face { body: Box<PlacedBody>, name: String },
    Profile(Profile),
}
impl Hole {
    pub(super) fn capture(s: &Session, setup: &cam::Setup, source: &cam::HoleSource, tolerance: f64) -> Result<Self> {
        source.validate().map_err(super::error)?;
        match source {
            cam::HoleSource::Face { target, name } => {
                if !setup.models.contains(target) {
                    return Err(bad("select a hole on one of this setup's model occurrences"));
                }
                Ok(Self::Face { body: Box::new(PlacedBody::capture(s, target)?), name: name.clone() })
            }
            cam::HoleSource::Profile { profile } => Ok(Self::Profile(Profile::capture(s, setup, profile, tolerance)?)),
        }
    }
    pub(super) fn resolve(self, frame: cam::Frame, tolerance: f64) -> Result<Selected> {
        let (origin, axis, diameter) = match self {
            Self::Face { body, name } => {
                let names = solidcraft_doc::naming::face_names(&body.body);
                let mut indices = names.iter().enumerate().filter_map(|(i, n)| (*n == name).then_some(i));
                let index = indices.next().ok_or_else(|| bad("CAM hole face missing; reselect it"))?;
                if indices.next().is_some() {
                    return Err(bad("CAM hole face is ambiguous; reselect it"));
                }
                let (origin, axis, radius) = crate::kernel::hole_axis(&body.body.body, index, cam::profile_tolerance(tolerance))?;
                let point = |p| frame.local(apply_point(&body.placement, p));
                let center = point(origin);
                let x = axis.any_perp();
                let y = axis.cross(x);
                let dx = point(origin + x) - center;
                let dy = point(origin + y) - center;
                if (dx.len() - dy.len()).abs() > 1e-9 || dx.dot(dy).abs() > 1e-9 {
                    return Err(bad("non-circular placed hole; select a rigid occurrence"));
                }
                (center, point(origin + axis) - center, 2. * radius * dx.len())
            }
            Self::Profile(profile) => {
                if !profile.region.holes.is_empty() {
                    return Err(bad("select one circular profile per drill center"));
                }
                let Some(Seg2::Arc { center, radius, .. }) = profile.region.outer.segs.first() else {
                    return Err(bad("drilling profile must be circular"));
                };
                let (center, radius) = (*center, *radius);
                let mut sweep = 0.;
                let segments = &profile.region.outer.segs;
                for (a, next) in segments.iter().zip(segments.iter().cycle().skip(1)).take(segments.len()) {
                    let Seg2::Arc { center: c, radius: r, sweep: angle, .. } = a else {
                        return Err(bad("drilling profile must be circular"));
                    };
                    if c.dist(center) > 1e-9 || (r - radius).abs() > 1e-9 || a.end().dist(next.start()) > 1e-8 {
                        return Err(bad("drilling profile must be a closed circle"));
                    }
                    sweep += angle;
                }
                if !sweep.is_finite() || (sweep.abs() - std::f64::consts::TAU).abs() > 1e-8 || radius <= 0. {
                    return Err(bad("drilling profile must be one complete circle"));
                }
                let (x, y) = (profile.plane.x, profile.plane.y);
                if (x.len() - y.len()).abs() > 1e-9 || x.dot(y).abs() > 1e-9 {
                    return Err(bad("non-circular placed profile; select a rigid occurrence"));
                }
                (profile.plane.to_world(center), x.cross(y), 2. * radius * x.len())
            }
        };
        let axis = axis.normalized().ok_or_else(|| bad("invalid hole axis"))?;
        if !origin.is_finite() || !axis.is_finite() || !diameter.is_finite() || diameter <= 0. || axis.cross(Vec3::Z).len() > 1e-9 {
            return Err(bad("drilling needs a finite circular hole parallel to setup Z"));
        }
        Ok(Selected { center: [origin.x, origin.y], diameter })
    }
}

pub(crate) fn select(s: &Session, setup: &cam::Setup, picks: Vec<Pick>) -> Result<(Vec<cam::HoleSource>, Vec<Selected>)> {
    if picks.is_empty() || picks.len() > cam::MAX_ITEMS {
        return Err(bad("select between one and 1000 holes"));
    }
    let state = s.model.state();
    let doc = s.placed_document();
    let mut sources = Vec::new();
    let mut seen = Vec::new();
    let mut selected = Vec::new();
    for pick in picks {
        let source = match pick {
            Pick::Face { body, index, occurrence } => {
                let target = if let Some(occurrence) = occurrence {
                    cam::BodyRef { body, occurrence }
                } else {
                    if setup.models.len().saturating_mul(doc.occurrences.len() + state.bodies.len() + 64) > 2_000_000 {
                        return Err(bad("hole selection work limit; provide a definition body and occurrence path"));
                    }
                    // Only these explicit model placements may be selected. Do not expand
                    // an entire shared assembly merely to interpret a viewport label.
                    let mut matching = setup.models.iter().filter(|target| {
                        state.body(&target.body).is_some_and(|model| {
                            doc.world_body_name(&state, &model.name, doc.model_body_component(model), &target.occurrence) == body
                        })
                    });
                    let target = matching.next().ok_or_else(|| bad("selected hole body is not in this setup; reselect its model occurrence"))?;
                    if matching.next().is_some() {
                        return Err(bad("selected hole body ambiguous; provide its occurrence path"));
                    }
                    target.clone()
                };
                let placed = PlacedBody::capture(s, &target)?;
                let name = solidcraft_doc::naming::face_names(&placed.body).get(index).cloned().ok_or_else(|| bad("selected hole face missing"))?;
                cam::HoleSource::Face { target, name }
            }
            Pick::Profile { sketch, index, occurrence } => {
                let profile = state
                    .sketches
                    .iter()
                    .find(|s| s.feature == sketch)
                    .and_then(|s| s.profiles.get(index))
                    .ok_or_else(|| bad("selected circular profile missing"))?;
                cam::HoleSource::Profile { profile: cam::ProfileRef { sketch, curves: profile.curve_ids(), occurrence } }
            }
        };
        if seen.contains(&source) {
            return Err(bad("duplicate hole selection"));
        }
        seen.push(source.clone());
        let hole = Hole::capture(s, setup, &source, cam::TOLERANCE)?.resolve(setup.frame, cam::TOLERANCE)?;
        if let Some((old, prior)) = sources
            .iter()
            .zip(&selected)
            .find(|(_, p): &(_, &Selected)| (p.center[0] - hole.center[0]).hypot(p.center[1] - hole.center[1]) < cam::TOLERANCE)
        {
            // A viewport pick expands a cylinder across its topological seam. Its
            // faces still describe one hole on one explicitly placed body.
            if matches!((old, &source), (cam::HoleSource::Face { target: a, .. }, cam::HoleSource::Face { target: b, .. }) if a == b)
                && (prior.center[0] - hole.center[0]).hypot(prior.center[1] - hole.center[1]) < 1e-8
                && (prior.diameter - hole.diameter).abs() < 1e-8
            {
                continue;
            }
            return Err(bad("selected hole axes coincide; remove duplicate selections"));
        }
        sources.push(source);
        selected.push(hole);
    }
    Ok((sources, selected))
}

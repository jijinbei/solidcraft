//! Machine state at a typed toolpath boundary, shared by inspection and playback.
use crate::*;
use solidcraft_geom::Vec3;
#[derive(Clone, Debug, serde::Serialize)]
pub struct State {
    pub completed: usize,
    pub position: Vec3,
    pub operation: Option<u64>,
    pub tool: Option<u64>,
    pub spindle: f64,
    pub coolant: bool,
    pub feed: Option<f64>,
}
pub fn at(path: &Toolpath, setup: &Setup, completed: usize) -> Result<State> {
    if completed > path.blocks.len() {
        return invalid("playback motion exceeds toolpath length");
    }
    let mut state =
        State { completed, position: Vec3::new(0., 0., setup.clearance), operation: None, tool: None, spindle: 0., coolant: false, feed: None };
    for block in path.blocks.iter().take(completed) {
        state.operation = Some(block.operation);
        match block.motion {
            Motion::Rapid { to } => state.position = to,
            Motion::Linear { to, feed } | Motion::Arc { to, feed, .. } => {
                state.position = to;
                state.feed = Some(feed);
            }
            Motion::Tool { tool, .. } => state.tool = Some(tool),
            Motion::Spindle { rpm } => state.spindle = rpm,
            Motion::Coolant { on } => state.coolant = on,
            Motion::Stop => {
                state.spindle = 0.;
                state.coolant = false;
            }
        }
    }
    Ok(state)
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn initial_pose_tool_changes_and_shutdown() {
        let input = corpus::fixture("face").unwrap();
        let path = Toolpath {
            fingerprint: "test".into(),
            blocks: vec![
                Block { operation: 10, motion: Motion::Tool { tool: 3, number: 7 } },
                Block { operation: 10, motion: Motion::Spindle { rpm: 5000. } },
                Block { operation: 10, motion: Motion::Coolant { on: true } },
                Block { operation: 10, motion: Motion::Linear { to: Vec3::new(2., 3., 4.), feed: 100. } },
                Block { operation: 11, motion: Motion::Tool { tool: 4, number: 8 } },
                Block { operation: 11, motion: Motion::Stop },
            ],
        };
        let start = at(&path, &input.setup, 0).unwrap();
        assert_eq!(start.position, Vec3::new(0., 0., 15.));
        assert!(start.tool.is_none());
        let cut = at(&path, &input.setup, 4).unwrap();
        assert_eq!(cut.position, Vec3::new(2., 3., 4.));
        assert_eq!(cut.tool, Some(3));
        assert_eq!(cut.spindle, 5000.);
        assert!(cut.coolant);
        assert_eq!(cut.feed, Some(100.));
        let end = at(&path, &input.setup, 6).unwrap();
        assert_eq!(end.tool, Some(4));
        assert_eq!(end.operation, Some(11));
        assert_eq!(end.spindle, 0.);
        assert!(!end.coolant);
        assert!(at(&path, &input.setup, 7).is_err());
    }
}

//! Artifact jobs retain immutable source data; only a fresh result command may publish output.
use super::*;

#[derive(Default)]
pub(crate) enum State {
    #[default]
    Idle,
    Running,
    Ready {
        content: String,
        destination: Option<String>,
    },
    Failed(String),
}
pub fn status(job: &Job) -> Result<serde_json::Value> {
    if job.cancelled.load(Ordering::Relaxed) {
        return Ok(serde_json::json!({"state":"cancelled"}));
    }
    let state = job.export.lock().map_err(|_| EngineError::Other("CAM export unavailable".into()))?;
    Ok(match &*state {
        State::Idle => serde_json::json!({"state":"idle"}),
        State::Running => serde_json::json!({"state":"running"}),
        State::Ready { content, .. } => serde_json::json!({"state":"ready","bytes":content.len()}),
        State::Failed(error) => serde_json::json!({"state":"failed","error":error}),
    })
}
pub fn busy(job: &Job) -> Result<bool> {
    Ok(matches!(*job.export.lock().map_err(|_| EngineError::Other("CAM export unavailable".into()))?, State::Running | State::Ready { .. }))
}
pub fn take(job: &Job) -> Result<(String, Option<String>)> {
    if job.cancelled.load(Ordering::Relaxed) {
        return Err(EngineError::Other("CAM export cancelled".into()));
    }
    let mut state = job.export.lock().map_err(|_| EngineError::Other("CAM export unavailable".into()))?;
    if !matches!(*state, State::Ready { .. }) {
        return Err(EngineError::Other("CAM export is not ready".into()));
    }
    match std::mem::take(&mut *state) {
        State::Ready { content, destination } => Ok((content, destination)),
        _ => Err(EngineError::Other("CAM export is not ready".into())),
    }
}
pub fn start(job: Arc<Job>, format: String, destination: Option<String>) -> Result<()> {
    if !matches!(format.as_str(), "gcode" | "toolpath" | "validation" | "stock" | "setup_sheet") {
        return Err(EngineError::Other("unknown CAM artifact format".into()));
    }
    let original = job.state()?;
    if !matches!(original, JobState::Generated(_) | JobState::Verified(..)) {
        return Err(EngineError::Other("no generated result".into()));
    }
    {
        let mut state = job.export.lock().map_err(|_| EngineError::Other("CAM export unavailable".into()))?;
        if matches!(*state, State::Running | State::Ready { .. }) {
            return Err(EngineError::Other("finish or cancel the current export first".into()));
        }
        *state = State::Running;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let failed = job.clone();
        if let Err(e) = std::thread::Builder::new().name("cam-export".into()).spawn(move || {
            let future = task(&job, original, format, destination);
            // A ready result may already have been collected and another job started.
            // Completing an export must never restore its old generation/verification state.
            run_future(job, future, false);
        }) {
            if let Ok(mut state) = failed.export.lock() {
                *state = State::Failed(e.to_string());
            }
            return Err(EngineError::Other(e.to_string()));
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let future = task(&job, original, format, destination);
        *job.pending.lock().map_err(|_| EngineError::Other("CAM scheduler unavailable".into()))? = Some(Box::pin(future));
    }
    Ok(())
}
fn task(job: &Job, original: JobState, format: String, destination: Option<String>) -> impl std::future::Future<Output = Result<JobState>> + use<> {
    let input = job.input.clone();
    let cancelled = job.cancelled.clone();
    let result = job.export.clone();
    async move {
        YieldOnce::default().await;
        let produce = async {
            let input = input.get().ok_or_else(|| EngineError::Other("CAM prepared input missing".into()))?;
            let (path, report) = match &original {
                JobState::Generated(path) => (path, None),
                JobState::Verified(path, report) => (path, Some(report.as_ref())),
                _ => return Err(EngineError::Other("no generated result".into())),
            };
            if format == "gcode" {
                let report = report.ok_or_else(|| EngineError::Other("simulate before posting".into()))?;
                if report.status != cam::verify::Status::Pass || report.finished_dimensions != cam::verify::Status::Pass || !report.stock_complete {
                    return Err(EngineError::Other("posting blocked by failed or inconclusive verification".into()));
                }
                cam::nc::round_trip_cooperative(path, &input.setup, &input.tools, || cancelled.load(Ordering::Relaxed), YieldOnce::default)
                    .await
                    .map_err(error)?;
                cam::nc::post_cooperative(path, &input.setup, || cancelled.load(Ordering::Relaxed), YieldOnce::default).await.map_err(error)
            } else {
                crate::io::cam::artifact_cooperative(input, path, report, &format, || cancelled.load(Ordering::Relaxed), YieldOnce::default)
                    .await
                    .map_err(|e| EngineError::Other(e.to_string()))
            }
        }
        .await;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(JobState::Cancelled);
        }
        let mut state = result.lock().map_err(|_| EngineError::Other("CAM export unavailable".into()))?;
        *state = match produce {
            Ok(content) => State::Ready { content, destination },
            Err(error) => State::Failed(error.to_string()),
        };
        Ok(original)
    }
}

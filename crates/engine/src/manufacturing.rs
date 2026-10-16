//! Immutable CAM jobs, independent of the desktop UI and document undo state.
use crate::{EngineError, Result, Session, cam};
pub mod exports;
pub(crate) mod holes;
mod snapshot;
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Default)]
pub struct Jobs {
    pub jobs: BTreeMap<u64, Arc<Job>>,
    pub next: u64,
}
pub struct Job {
    pub source_doc: Arc<solidcraft_doc::Document>,
    pub source_moves: BTreeMap<u64, solidcraft_doc::Mat>,
    input: Arc<OnceLock<cam::JobInput>>,
    snapshot: Mutex<Option<snapshot::Snapshot>>,
    stock: Arc<Mutex<Option<std::result::Result<Arc<crate::geom::Mesh>, String>>>>,
    pub(crate) export: Arc<Mutex<exports::State>>,
    pub cancelled: Arc<AtomicBool>,
    #[cfg(target_arch = "wasm32")]
    pending: Mutex<Option<std::pin::Pin<Box<dyn std::future::Future<Output = Result<JobState>>>>>>,
    pub progress: Arc<AtomicUsize>,
    pub result: Arc<Mutex<JobState>>,
}
#[derive(Clone)]
pub enum JobState {
    Preparing,
    Running,
    Generated(Arc<cam::Toolpath>),
    Verifying(Arc<cam::Toolpath>),
    Verified(Arc<cam::Toolpath>, Arc<cam::verify::Report>),
    Failed(String),
    Cancelled,
}
impl Job {
    /// Display geometry is a derived cache. It never substitutes for a verification report.
    pub fn stock_mesh(&self) -> Result<Option<Arc<crate::geom::Mesh>>> {
        if !matches!(self.state()?, JobState::Verified(..)) {
            return Ok(None);
        }
        match self.stock.lock().map_err(|_| EngineError::Other("CAM stock display unavailable".into()))?.as_ref() {
            Some(Ok(mesh)) => Ok(Some(mesh.clone())),
            Some(Err(e)) => Err(EngineError::Other(e.clone())),
            None => Ok(None),
        }
    }
    pub fn input(&self) -> Result<&cam::JobInput> {
        self.input.get().ok_or_else(|| EngineError::Other("CAM input is still being prepared".into()))
    }
    pub fn state(&self) -> Result<JobState> {
        self.result.lock().map(|s| s.clone()).map_err(|_| EngineError::Other("CAM worker state unavailable".into()))
    }
    pub fn set(&self, state: JobState) {
        if let Ok(mut s) = self.result.lock() {
            *s = if self.cancelled.load(Ordering::Relaxed) { JobState::Cancelled } else { state };
        }
    }
}
pub fn error(e: cam::Error) -> EngineError {
    EngineError::Other(e.to_string())
}
pub fn input(s: &Session, program: u64) -> Result<cam::JobInput> {
    snapshot::ready(snapshot::Snapshot::capture(s, program)?.build(&|| false, &|_| {}, &mut || std::future::ready(())))
}
pub fn profile_boundary(s: &Session, setup: &cam::Setup, source: &cam::ProfileRef, tolerance: f64) -> Result<cam::Boundary> {
    snapshot::ready(snapshot::Profile::capture(s, setup, source, tolerance)?.sample(&|| false, &mut || std::future::ready(())))
}
pub fn start(s: &mut Session, program: u64, background: bool) -> Result<u64> {
    if s.cam_jobs.jobs.len() >= 16 {
        let removable = s.cam_jobs.jobs.iter().find_map(|(id, j)| match j.state() {
            Ok(JobState::Preparing | JobState::Running | JobState::Verifying(_)) => None,
            Ok(JobState::Generated(_) | JobState::Verified(..)) if exports::busy(j).unwrap_or(true) => None,
            _ => Some(*id),
        });
        if let Some(id) = removable {
            s.cam_jobs.jobs.remove(&id);
        } else {
            return Err(EngineError::Other("too many running CAM jobs".into()));
        }
    }
    let snapshot = snapshot::Snapshot::capture(s, program)?;
    let job = Arc::new(Job {
        source_doc: s.doc.clone(),
        source_moves: s.pending_moves.clone(),
        input: Arc::new(OnceLock::new()),
        snapshot: Mutex::new(Some(snapshot)),
        stock: Arc::new(Mutex::new(None)),
        export: Arc::new(Mutex::new(exports::State::Idle)),
        cancelled: Arc::new(AtomicBool::new(false)),
        #[cfg(target_arch = "wasm32")]
        pending: Mutex::new(None),
        progress: Arc::new(AtomicUsize::new(0)),
        result: Arc::new(Mutex::new(JobState::Preparing)),
    });
    s.cam_jobs.next = s.cam_jobs.next.checked_add(1).ok_or_else(|| EngineError::Other("CAM job ID exhausted".into()))?;
    let id = s.cam_jobs.next;
    s.cam_jobs.jobs.insert(id, job.clone());
    dispatch(job, background, false)?;
    Ok(id)
}
pub fn dispatch(job: Arc<Job>, background: bool, verify: bool) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    if background {
        let failed = job.clone();
        if let Err(e) = std::thread::Builder::new().name("cam".into()).spawn(move || run(job, verify)) {
            failed.set(JobState::Failed(e.to_string()));
            return Err(EngineError::Other(e.to_string()));
        }
        return Ok(());
    }
    #[cfg(target_arch = "wasm32")]
    if background {
        let future = task(&job, verify)?;
        *job.pending.lock().map_err(|_| EngineError::Other("CAM scheduler unavailable".into()))? = Some(Box::pin(future));
        return Ok(());
    }
    run(job, verify);
    Ok(())
}
// Capture owned task components instead of Job itself: a browser's pending
// future must not keep its owning Job alive through an Arc cycle.
fn task(job: &Job, verify: bool) -> Result<impl std::future::Future<Output = Result<JobState>> + use<>> {
    let input = job.input.clone();
    let cancelled = job.cancelled.clone();
    let progress = job.progress.clone();
    let state = job.result.clone();
    let stock = job.stock.clone();
    let path = if verify {
        match job.state()? {
            JobState::Verifying(path) => Some(path),
            _ => return Err(EngineError::Other("generate a path before simulation".into())),
        }
    } else {
        None
    };
    let snapshot = if verify { None } else { job.snapshot.lock().map_err(|_| EngineError::Other("CAM snapshot unavailable".into()))?.take() };
    Ok(async move {
        YieldOnce::default().await;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(JobState::Cancelled);
        }
        if let Some(path) = path {
            let input = input.get().ok_or_else(|| EngineError::Other("CAM prepared input missing".into()))?;
            let parsed = cam::nc::round_trip_cooperative(&path, &input.setup, &input.tools, || cancelled.load(Ordering::Relaxed), YieldOnce::default)
                .await
                .map_err(error)?;
            let report =
                cam::verify::simulate_cooperative(input, &parsed, Default::default(), || cancelled.load(Ordering::Relaxed), YieldOnce::default)
                    .await
                    .map_err(error)?;
            YieldOnce::default().await;
            if cancelled.load(Ordering::Relaxed) {
                return Ok(JobState::Cancelled);
            }
            let display = stock_display(&report, input.setup.frame, input.setup.stock.min.z, &cancelled).await;
            if cancelled.load(Ordering::Relaxed) {
                return Ok(JobState::Cancelled);
            }
            if let Ok(mut cache) = stock.lock() {
                *cache = Some(display.map(Arc::new).map_err(|e| e.to_string()));
            }
            Ok(JobState::Verified(path, Arc::new(report)))
        } else {
            let snapshot = snapshot.ok_or_else(|| EngineError::Other("CAM snapshot already consumed".into()))?;
            let prepared = snapshot
                .build(
                    &|| cancelled.load(Ordering::Relaxed),
                    &|n| {
                        progress.store(n, Ordering::Relaxed);
                    },
                    &mut YieldOnce::default,
                )
                .await?;
            if cancelled.load(Ordering::Relaxed) {
                return Ok(JobState::Cancelled);
            }
            input.set(prepared).map_err(|_| EngineError::Other("CAM snapshot already prepared".into()))?;
            if let Ok(mut state) = state.lock() {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(JobState::Cancelled);
                }
                *state = JobState::Running;
            }
            let mut generator =
                cam::generate::Generator::new(input.get().ok_or_else(|| EngineError::Other("CAM prepared input missing".into()))?.clone())
                    .map_err(error)?;
            loop {
                YieldOnce::default().await;
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(JobState::Cancelled);
                }
                let done = generator.step().map_err(error)?;
                progress.store(20 + (generator.progress() * 80.) as usize, Ordering::Relaxed);
                if done {
                    break;
                }
            }
            Ok(JobState::Generated(Arc::new(generator.finish().map_err(error)?)))
        }
    })
}
async fn stock_display(report: &cam::verify::Report, frame: cam::Frame, bottom: f64, cancelled: &AtomicBool) -> Result<crate::geom::Mesh> {
    let triangles =
        cam::verify::stock_triangles_cooperative(report, bottom, || cancelled.load(Ordering::Relaxed), YieldOnce::default).await.map_err(error)?;
    let mut mesh = crate::geom::Mesh::default();
    for (i, triangle) in triangles.into_iter().enumerate() {
        if i.is_multiple_of(256) {
            YieldOnce::default().await;
            if cancelled.load(Ordering::Relaxed) {
                return Err(EngineError::Other("CAM stock display cancelled".into()));
            }
        }
        let triangle = triangle.map(|p| frame.world(p));
        let normal = (triangle[1] - triangle[0])
            .cross(triangle[2] - triangle[0])
            .normalized()
            .ok_or_else(|| EngineError::Other("invalid stock display triangle".into()))?;
        let base = mesh.positions.len() as u32;
        mesh.positions.extend(triangle);
        mesh.normals.extend([normal; 3]);
        mesh.triangles.push([base, base + 1, base + 2]);
    }
    Ok(mesh)
}
fn run(job: Arc<Job>, verify: bool) {
    match task(&job, verify) {
        Ok(future) => run_future(job, future, true),
        Err(e) => job.set(JobState::Failed(e.to_string())),
    }
}
fn run_future(job: Arc<Job>, future: impl std::future::Future<Output = Result<JobState>>, publish_state: bool) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<JobState> {
        let mut future = std::pin::pin!(future);
        loop {
            if job.cancelled.load(Ordering::Relaxed) {
                return Ok(JobState::Cancelled);
            }
            if let std::task::Poll::Ready(result) =
                std::future::Future::poll(future.as_mut(), &mut std::task::Context::from_waker(std::task::Waker::noop()))
            {
                return result;
            }
        }
    }));
    let state = match outcome {
        Ok(Ok(state)) => state,
        Ok(Err(e)) => JobState::Failed(e.to_string()),
        Err(_) => JobState::Failed("CAM worker failed; no result published".into()),
    };
    if publish_state {
        job.set(state);
    } else if let JobState::Failed(error) = state
        && let Ok(mut export) = job.export.lock()
    {
        *export = exports::State::Failed(error);
    }
}
pub fn job(s: &Session, id: u64) -> Result<Arc<Job>> {
    s.cam_jobs.jobs.get(&id).cloned().ok_or_else(|| EngineError::Other("CAM job missing; regenerate after reopening".into()))
}
pub fn current(s: &Session, job: &Job) -> Result<()> {
    if (!Arc::ptr_eq(&s.doc, &job.source_doc) && *s.doc != *job.source_doc) || s.pending_moves != job.source_moves {
        Err(EngineError::Other("CAM result is stale; regenerate after editing the design, tools or setup".into()))
    } else {
        Ok(())
    }
}

/// Resume one browser job checkpoint. Returns true while more work is pending.
/// The host controls its frame budget; status queries can still advance one step.
#[cfg(target_arch = "wasm32")]
pub fn advance(job: &Job) -> bool {
    let Ok(mut pending) = job.pending.lock() else { return false };
    if job.cancelled.load(Ordering::Relaxed) {
        *pending = None;
        job.set(JobState::Cancelled);
        return false;
    }
    let Some(future) = pending.as_mut() else { return false };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        future.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
    }));
    let state = match result {
        Ok(std::task::Poll::Pending) => return true,
        Ok(std::task::Poll::Ready(Ok(state))) => state,
        Ok(std::task::Poll::Ready(Err(error))) => JobState::Failed(error.to_string()),
        Err(_) => JobState::Failed("CAM worker failed; no result published".into()),
    };
    *pending = None;
    job.progress.store(100, Ordering::Relaxed);
    job.set(state);
    false
}
#[derive(Default)]
struct YieldOnce(bool);
impl std::future::Future for YieldOnce {
    type Output = ();
    fn poll(mut self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        if self.0 {
            std::task::Poll::Ready(())
        } else {
            self.0 = true;
            std::task::Poll::Pending
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    fn captured() -> (Session, Arc<Job>) {
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length":20,"width":20,"height":5})).unwrap();
        let mut input = cam::corpus::fixture("face").unwrap();
        input.setup.models[0].body = s.model.state().bodies[0].name.clone();
        let m = &mut s.doc_mut().manufacturing;
        m.setups.push(input.setup);
        m.tools = input.tools;
        m.operations = input.operations;
        m.programs.push(cam::Program { id: 1, name: "Test".into(), setup: 1, operations: vec![1] });
        let job = Arc::new(Job {
            source_doc: s.doc.clone(),
            source_moves: s.pending_moves.clone(),
            input: Arc::new(OnceLock::new()),
            snapshot: Mutex::new(Some(snapshot::Snapshot::capture(&s, 1).unwrap())),
            stock: Arc::new(Mutex::new(None)),
            export: Arc::new(Mutex::new(exports::State::Idle)),
            cancelled: Arc::new(AtomicBool::new(false)),
            #[cfg(target_arch = "wasm32")]
            pending: Mutex::new(None),
            progress: Arc::new(AtomicUsize::new(0)),
            result: Arc::new(Mutex::new(JobState::Preparing)),
        });
        (s, job)
    }
    #[test]
    fn completed_export_worker_cannot_restore_state_after_result_collection() {
        let (_, job) = captured();
        let ready = Arc::new(std::sync::Barrier::new(2));
        let collected = Arc::new(std::sync::Barrier::new(2));
        let worker_job = job.clone();
        let worker_ready = ready.clone();
        let worker_collected = collected.clone();
        let worker = std::thread::spawn(move || {
            let result = worker_job.export.clone();
            run_future(
                worker_job,
                async move {
                    *result.lock().unwrap() = exports::State::Ready { content: "complete".into(), destination: None };
                    worker_ready.wait();
                    worker_collected.wait();
                    Ok(JobState::Preparing)
                },
                false,
            );
        });
        ready.wait();
        assert_eq!(exports::take(&job).unwrap().0, "complete");
        job.set(JobState::Running);
        collected.wait();
        worker.join().unwrap();
        assert!(matches!(job.state().unwrap(), JobState::Running));
    }

    #[test]
    fn snapshot_is_immutable_and_preparation_is_deferred() {
        let (mut s, job) = captured();
        let expected = input(&s, 1).unwrap();
        let mut future = std::pin::pin!(task(&job, false).unwrap());
        let mut cx = Context::from_waker(Waker::noop());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert!(job.input().is_err());
        s.execute("solid.box", &json!({"length":2,"width":2,"height":2})).unwrap();
        s.doc_mut().manufacturing.operations[0].feed = 900.;
        let mut polls = 0;
        let mut previous = 0;
        let state = loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Pending => {
                    polls += 1;
                    let progress = job.progress.load(Ordering::Relaxed);
                    assert!(progress >= previous);
                    previous = progress;
                }
                Poll::Ready(result) => break result.unwrap(),
            }
        };
        assert!(polls > 2);
        assert!(matches!(state, JobState::Generated(_)));
        assert_eq!(serde_json::to_value(job.input().unwrap()).unwrap(), serde_json::to_value(&expected).unwrap());
        assert!(current(&s, &job).is_err());
    }
    #[test]
    fn preparation_cancellation_never_publishes_or_revives_job() {
        let (_, job) = captured();
        let mut future = std::pin::pin!(task(&job, false).unwrap());
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..2 {
            assert!(future.as_mut().poll(&mut cx).is_pending());
        }
        job.cancelled.store(true, Ordering::Relaxed);
        job.set(JobState::Cancelled);
        let result = loop {
            if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
                break result;
            }
        };
        assert!(result.is_err() || matches!(result, Ok(JobState::Cancelled)));
        assert!(job.input().is_err());
        job.set(JobState::Running);
        assert!(matches!(job.state().unwrap(), JobState::Cancelled));
    }
    #[test]
    fn native_worker_prepares_generates_and_releases_its_snapshot() {
        let (_, job) = captured();
        let worker = job.clone();
        std::thread::spawn(move || run(worker, false)).join().unwrap();
        assert!(matches!(job.state().unwrap(), JobState::Generated(_)));
        assert!(job.input().is_ok());
        assert!(job.snapshot.lock().unwrap().is_none());
        assert_eq!(job.progress.load(Ordering::Relaxed), 100);
    }

    #[test]
    fn cam_snapshot_uses_fixed_tolerance_for_large_curved_geometry() {
        let mut session = Session::default();
        session.execute("solid.cylinder", &json!({"radius":5000,"height":6})).unwrap();
        let mut recipe = cam::corpus::fixture("face").unwrap();
        recipe.setup.models[0].body = session.model.state().bodies[0].name.clone();
        recipe.setup.stock.min = crate::geom::Vec3::new(-5000., -5000., 0.);
        recipe.setup.stock.max = crate::geom::Vec3::new(5000., 5000., 7.);
        let manufacturing = &mut session.doc_mut().manufacturing;
        manufacturing.setups.push(recipe.setup);
        manufacturing.tools = recipe.tools;
        manufacturing.operations = recipe.operations;
        manufacturing.programs.push(cam::Program { id: 1, name: "Large cylinder".into(), setup: 1, operations: vec![1] });
        let prepared = input(&session, 1).unwrap();
        let mut checked = 0;
        for triangle in &prepared.geometry.target {
            if triangle.iter().any(|p| p.z < 1.) && triangle.iter().any(|p| p.z > 5.) {
                for i in 0..3 {
                    let midpoint = (triangle[i] + triangle[(i + 1) % 3]) * 0.5;
                    assert!((5000. - midpoint.x.hypot(midpoint.y)).abs() <= cam::VERIFY_ERROR / 2.);
                    checked += 1;
                }
            }
        }
        assert!(checked > 1000);
    }
}

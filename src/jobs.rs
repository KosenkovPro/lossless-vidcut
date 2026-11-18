//! Background jobs with progress reporting and cancellation.
//!
//! Every long running `ffmpeg` invocation is represented by a [`Job`]. The
//! jobs are started on a Tokio runtime and their progress is polled once per
//! frame by the user interface.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Result;

use crate::processor;

/// The shared Tokio runtime that runs the background jobs.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("could not start the tokio runtime")
    })
}

/// The state of a job.
#[derive(Debug, Clone)]
pub enum Status {
    /// The job is still running.
    Running,
    /// The job finished and wrote the given file.
    Finished(PathBuf),
    /// The job failed with the given message.
    Failed(String),
    /// The job was cancelled by the user.
    Cancelled,
}

/// A running or finished background job.
pub struct Job {
    /// Unique identifier of the job.
    pub id: u64,
    /// Description shown in the user interface.
    pub title: String,
    /// The outcome of the job.
    pub status: Status,
    progress: Arc<Mutex<processor::Progress>>,
    cancel: Arc<AtomicBool>,
    result: Arc<Mutex<Option<Result<PathBuf>>>>,
}

impl Job {
    /// Completion ratio between 0 and 1.
    pub fn fraction(&self) -> f32 {
        self.progress
            .lock()
            .map(|state| state.fraction)
            .unwrap_or(0.0)
    }

    /// Description of the current step.
    pub fn stage(&self) -> String {
        self.progress
            .lock()
            .map(|state| state.stage.clone())
            .unwrap_or_default()
    }

    /// `true` while the job is running.
    pub fn is_running(&self) -> bool {
        matches!(self.status, Status::Running)
    }

    /// Ask the job to stop.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

/// The collection of all jobs of the current session.
#[derive(Default)]
pub struct JobManager {
    jobs: Vec<Job>,
    next_id: u64,
}

impl JobManager {
    /// Start a new background job.
    ///
    /// The closure receives the cancellation flag and the progress handle and
    /// returns the file the job produced.
    pub fn start<F>(&mut self, title: impl Into<String>, task: F)
    where
        F: FnOnce(
                Arc<AtomicBool>,
                Arc<Mutex<processor::Progress>>,
            ) -> Pin<Box<dyn Future<Output = Result<PathBuf>> + Send>>
            + Send
            + 'static,
    {
        let id = self.next_id;
        self.next_id += 1;

        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(processor::Progress::default()));
        let result: Arc<Mutex<Option<Result<PathBuf>>>> = Arc::new(Mutex::new(None));

        let task_cancel = cancel.clone();
        let task_progress = progress.clone();
        let task_result = result.clone();
        runtime().spawn(async move {
            let outcome = task(task_cancel, task_progress).await;
            if let Ok(mut slot) = task_result.lock() {
                *slot = Some(outcome);
            }
        });

        self.jobs.push(Job {
            id,
            title: title.into(),
            status: Status::Running,
            progress,
            cancel,
            result,
        });
    }

    /// Collect the results of the finished jobs and drop the old ones.
    pub fn poll(&mut self) {
        for job in &mut self.jobs {
            if !job.is_running() {
                continue;
            }
            let finished = job.result.lock().ok().and_then(|mut slot| slot.take());
            if let Some(result) = finished {
                job.status = match result {
                    Ok(path) => Status::Finished(path),
                    Err(error) => {
                        let text = format!("{error:#}");
                        if job.cancel.load(Ordering::SeqCst) {
                            Status::Cancelled
                        } else {
                            Status::Failed(text)
                        }
                    }
                };
            }
        }
        // Keep the list short, but never drop a running job.
        while self.jobs.len() > 12 {
            if let Some(index) = self.jobs.iter().position(|job| !job.is_running()) {
                self.jobs.remove(index);
            } else {
                break;
            }
        }
    }

    /// `true` when at least one job is still running.
    pub fn any_running(&self) -> bool {
        self.jobs.iter().any(Job::is_running)
    }

    /// The jobs of this manager.
    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }
}

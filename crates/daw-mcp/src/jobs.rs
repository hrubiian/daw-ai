//! Bounded offline listening jobs with immutable revision-linked artifacts.
use daw_core::{
    project::{format, Project},
    render_control::RenderControl,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobReport {
    pub id: String,
    pub state: String,
    pub progress: u32,
    pub project_hash: String,
    pub session_revision: Option<u64>,
    pub engine_version: String,
    pub sample_rate: u32,
    pub normalized: bool,
    pub warmup: String,
    pub range: Option<(u32, u32)>,
    pub artifacts: Vec<PathBuf>,
    pub assets: BTreeMap<String, String>,
    pub analysis: Option<serde_json::Value>,
    pub error: Option<String>,
}
struct Job {
    directory: PathBuf,
    report: JobReport,
    control: Arc<RenderControl>,
}
pub struct Jobs {
    jobs: Mutex<BTreeMap<String, Job>>,
    active: AtomicUsize,
}
pub fn manager() -> &'static Jobs {
    static JOBS: OnceLock<Jobs> = OnceLock::new();
    JOBS.get_or_init(|| Jobs {
        jobs: Mutex::new(BTreeMap::new()),
        active: AtomicUsize::new(0),
    })
}
// Publish reports atomically: interrupted writes leave the previous report readable.
fn write_report(directory: &Path, report: &JobReport) -> Result<(), String> {
    use std::io::Write;
    let temporary = directory.join(format!(".report-{}.tmp", daw_io::session::operation_id()));
    let result = (|| -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(report)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, directory.join("report.json"))?;
        std::fs::File::open(directory)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result.map_err(|e| e.to_string())
}

fn render_directory(base: &Path, id: &str) -> Result<PathBuf, String> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("invalid render id".into());
    }
    let root = base
        .join("renders")
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let directory = root.join(id).canonicalize().map_err(|e| e.to_string())?;
    if directory.parent() != Some(root.as_path()) {
        return Err("render directory escapes history".into());
    }
    Ok(directory)
}

impl Jobs {
    pub fn submit(
        &'static self,
        project: Project,
        revision: Option<u64>,
        base: PathBuf,
        sf2: PathBuf,
        range: Option<(u32, u32)>,
    ) -> Result<String, String> {
        project.validate().map_err(|e| e.to_string())?;
        if let Some((a, b)) = range {
            if a >= b {
                return Err("range requires start < end".into());
            }
        }
        let hash = format::project_revision(&project).map_err(|e| e.to_string())?;
        self.active
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < 2).then_some(n + 1)
            })
            .map_err(|_| "render queue busy: at most two jobs may run")?;
        let id = daw_io::session::operation_id();
        let control = Arc::new(RenderControl::new(Duration::from_secs(300)));
        let report = JobReport {
            id: id.clone(),
            state: "running".into(),
            progress: 0,
            project_hash: hash,
            session_revision: revision,
            engine_version: env!("CARGO_PKG_VERSION").into(),
            sample_rate: 44100,
            normalized: false,
            warmup: "from song start; fixed 1024-frame blocks".into(),
            range,
            artifacts: vec![],
            assets: BTreeMap::new(),
            analysis: None,
            error: None,
        };
        let directory = match (|| -> Result<PathBuf, String> {
            let root = base.join("renders");
            std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            let directory = root.join(&id);
            std::fs::create_dir(&directory).map_err(|e| e.to_string())?;
            let directory = directory.canonicalize().map_err(|e| e.to_string())?;
            write_report(&directory, &report)?;
            Ok(directory)
        })() {
            Ok(directory) => directory,
            Err(error) => {
                self.active.fetch_sub(1, Ordering::SeqCst);
                return Err(error);
            }
        };
        {
            let mut jobs = self.jobs.lock().unwrap();
            if jobs.len() >= 32 {
                if let Some(key) = jobs
                    .iter()
                    .find(|(_, j)| j.report.state != "running")
                    .map(|(k, _)| k.clone())
                {
                    jobs.remove(&key);
                }
            }
            jobs.insert(
                id.clone(),
                Job {
                    directory: directory.clone(),
                    report,
                    control: control.clone(),
                },
            );
        }
        let job_id = id.clone();
        std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> anyhow::Result<(Vec<PathBuf>, BTreeMap<String,String>, serde_json::Value)> {
                control.check()?;
                let runtime = format::resolve_assets(&project, &base, &std::env::current_dir()?);
                let assets = format::asset_fingerprints(&runtime, &sf2)?;
                let (audio, sr) = daw_core::audio::export::render_range_samples(&runtime, &sf2, false, range, &control)?;
                control.check()?;
                anyhow::ensure!(assets == format::asset_fingerprints(&runtime, &sf2)?, "assets changed during render; retry with stable inputs");
                let left: Vec<_> = audio.iter().step_by(2).copied().collect();
                let right: Vec<_> = audio.iter().skip(1).step_by(2).copied().collect();
                let analysis = serde_json::to_value(crate::analysis::analyze(&left, &right, sr, runtime.project.bpm))?;
                control.check()?;
                let dir = &directory;
                let wav = dir.join("audio.wav");
                let mut writer = hound::WavWriter::create(&wav, hound::WavSpec { channels: 2, sample_rate: sr, bits_per_sample: 32, sample_format: hound::SampleFormat::Float })?;
                for sample in audio { writer.write_sample(sample)?; } writer.finalize()?;
                let snapshot = dir.join("project.json"); std::fs::write(&snapshot, serde_json::to_vec_pretty(&project)?)?;
                Ok((vec![wav, snapshot], assets, analysis))
            }));
            let mut jobs = self.jobs.lock().unwrap();
            let job = jobs.get_mut(&job_id).unwrap();
            match outcome {
                Ok(Ok((artifacts, assets, analysis))) => {
                    job.report.state = "completed".into();
                    job.report.artifacts = artifacts;
                    job.report.assets = assets;
                    job.report.analysis = Some(analysis);
                    job.report.progress = 100;
                }
                result => {
                    job.report.state = if control.cancelled.load(Ordering::Relaxed) {
                        "cancelled"
                    } else {
                        "failed"
                    }
                    .into();
                    job.report.error = Some(match result {
                        Ok(Err(e)) => e.to_string(),
                        _ => "render worker panicked".into(),
                    });
                }
            }
            if let Err(e) = write_report(&directory, &job.report) {
                job.report.error = Some(format!("report write failed: {e}"));
                job.report.state = "failed".into();
            }
            self.active.fetch_sub(1, Ordering::SeqCst);
        });
        Ok(id)
    }
    pub fn status(&self, id: &str) -> Result<JobReport, String> {
        let jobs = self.jobs.lock().unwrap();
        let job = jobs.get(id).ok_or("unknown job")?;
        let mut report = job.report.clone();
        if report.state == "running" {
            report.progress = job.control.progress.load(Ordering::Relaxed).min(99);
        }
        Ok(report)
    }
    /// Read current-process state or a durable report scoped to this project directory.
    pub fn status_in(&self, base: &Path, id: &str) -> Result<JobReport, String> {
        let directory = render_directory(base, id)?;
        {
            let jobs = self.jobs.lock().unwrap();
            if let Some(job) = jobs.get(id).filter(|j| j.directory == directory) {
                let mut report = job.report.clone();
                if report.state == "running" {
                    report.progress = job.control.progress.load(Ordering::Relaxed).min(99);
                }
                return Ok(report);
            }
        }
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(directory.join("report.json"))
            .map_err(|e| e.to_string())?
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("render report exceeds 4 MiB".into());
        }
        let mut report: JobReport =
            serde_json::from_slice(&bytes).map_err(|e| format!("invalid render report: {e}"))?;
        if report.id != id {
            return Err("render report id does not match directory".into());
        }
        if report.state == "running" {
            report.state = "unavailable".into();
            report.error = Some("Job is not owned by this process; it may still run elsewhere or have been interrupted.".into());
        }
        // Stored absolute paths can be stale after moving a project. Never trust them for writes.
        for artifact in &mut report.artifacts {
            let name = artifact.file_name().ok_or("invalid artifact path")?;
            if name != "audio.wav" && name != "project.json" {
                return Err("unexpected render artifact".into());
            }
            *artifact = directory.join(name);
        }
        Ok(report)
    }
    pub fn history(
        &self,
        base: &Path,
        offset: usize,
        limit: usize,
    ) -> Result<serde_json::Value, String> {
        if !(1..=100).contains(&limit) {
            return Err("history limit must be 1–100".into());
        }
        let entries = match std::fs::read_dir(base.join("renders")) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(serde_json::json!({"renders": [], "errors": [], "next_offset": null}))
            }
            Err(e) => return Err(e.to_string()),
        };
        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                ids.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        ids.sort();
        let mut reports = Vec::new();
        let mut errors = Vec::new();
        for id in ids.iter().skip(offset).take(limit) {
            match self.status_in(base, id) {
                Ok(report) => reports.push(report),
                Err(error) => errors.push(serde_json::json!({"id": id, "error": error})),
            }
        }
        let next = offset.saturating_add(limit);
        Ok(
            serde_json::json!({"renders": reports, "errors": errors, "next_offset": (next < ids.len()).then_some(next)}),
        )
    }
    pub fn rate_in(
        &self,
        base: &Path,
        id: &str,
        score: u8,
        comment: &str,
    ) -> Result<PathBuf, String> {
        Self::rate_report(self.status_in(base, id)?, score, comment)
    }
    pub fn compare_in(&self, base: &Path, a: &str, b: &str) -> Result<serde_json::Value, String> {
        Self::compare_reports(self.status_in(base, a)?, self.status_in(base, b)?)
    }
    /// Store musician feedback alongside the immutable render, as a new event.
    pub fn rate(&self, id: &str, score: u8, comment: &str) -> Result<PathBuf, String> {
        Self::rate_report(self.status(id)?, score, comment)
    }
    fn rate_report(report: JobReport, score: u8, comment: &str) -> Result<PathBuf, String> {
        let id = &report.id;
        if !(1..=5).contains(&score) || comment.len() > 4000 {
            return Err("rating requires score 1–5 and at most 4000 comment bytes".into());
        }
        if report.state != "completed" {
            return Err("only completed renders can be rated".into());
        }
        let dir = report
            .artifacts
            .first()
            .and_then(|p| p.parent())
            .ok_or("render artifacts unavailable")?;
        let path = dir.join(format!("rating-{}.json", daw_io::session::operation_id()));
        let value = serde_json::json!({"job_id": id, "project_hash": report.project_hash, "session_revision": report.session_revision, "score": score, "comment": comment});
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        Ok(path)
    }
    pub fn compare(&self, a: &str, b: &str) -> Result<serde_json::Value, String> {
        Self::compare_reports(self.status(a)?, self.status(b)?)
    }
    fn compare_reports(a: JobReport, b: JobReport) -> Result<serde_json::Value, String> {
        if a.state != "completed" || b.state != "completed" {
            return Err("A/B comparison requires two completed renders".into());
        }
        if a.range != b.range || a.sample_rate != b.sample_rate || a.normalized != b.normalized {
            return Err("A/B render settings differ; render both with identical settings".into());
        }
        Ok(
            serde_json::json!({"a": a, "b": b, "guidance": "Audition the audio artifacts. Measurements describe differences, not musical quality."}),
        )
    }
    pub fn cancel(&self, id: &str) -> Result<String, String> {
        let jobs = self.jobs.lock().unwrap();
        let job = jobs.get(id).ok_or("unknown job")?;
        if job.report.state == "running" {
            job.control.cancel();
        } else {
            return Ok(format!(
                "job already {}; no running work to cancel",
                job.report.state
            ));
        }
        Ok("cancellation requested; native plugin calls must return before cancellation can complete".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_is_not_a_failed_render() {
        let mut project = Project::default_project();
        project.tracks.clear();
        project.patterns.clear();
        project.arrangement.clear();
        let base =
            std::env::temp_dir().join(format!("daw-cancel-{}", daw_io::session::operation_id()));
        let id = manager()
            .submit(project, None, base.clone(), base.join("unused.sf2"), None)
            .unwrap();
        manager().cancel(&id).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let report = manager().status(&id).unwrap();
            if report.state != "running" {
                assert!(
                    matches!(report.state.as_str(), "cancelled" | "completed"),
                    "{} {:?}",
                    report.state,
                    report.error
                );
                assert!(manager().cancel(&id).unwrap().starts_with("job already"));
                let saved: JobReport = serde_json::from_slice(
                    &std::fs::read(base.join("renders").join(&id).join("report.json")).unwrap(),
                )
                .unwrap();
                assert_eq!(saved.state, report.state);
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        if base.exists() {
            std::fs::remove_dir_all(base).unwrap();
        }
    }
    #[test]
    fn render_artifacts_provenance_and_rating() {
        let mut project = Project::default_project();
        project.tracks.clear();
        project.patterns.clear();
        project.arrangement.clear();
        let base =
            std::env::temp_dir().join(format!("daw-jobs-{}", daw_io::session::operation_id()));
        let id = manager()
            .submit(
                project.clone(),
                Some(4),
                base.clone(),
                base.join("absent.sf2"),
                Some((0, 1)),
            )
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let report = loop {
            let report = manager().status(&id).unwrap();
            if report.state != "running" {
                break report;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(report.state, "completed", "{:?}", report.error);
        assert_eq!(
            report.project_hash,
            format::project_revision(&project).unwrap()
        );
        assert_eq!(report.session_revision, Some(4));
        assert!(report.artifacts.iter().all(|p| p.exists()));
        assert!(report.analysis.is_some());
        let rating = manager().rate(&id, 4, "Useful starting point").unwrap();
        assert!(rating.exists());
        assert!(manager().rate(&id, 6, "invalid").is_err());
        assert!(manager().compare(&id, &id).is_ok());
        // A fresh manager represents a restarted server; moved projects must keep working.
        let restarted = Jobs {
            jobs: Mutex::new(BTreeMap::new()),
            active: AtomicUsize::new(0),
        };
        let moved = base.with_extension("moved");
        std::fs::rename(&base, &moved).unwrap();
        let restored = restarted.status_in(&moved, &id).unwrap();
        assert_eq!(restored.project_hash, report.project_hash);
        assert!(restored
            .artifacts
            .iter()
            .all(|p| p.exists() && p.starts_with(&moved)));
        assert!(restarted
            .rate_in(&moved, &id, 5, "After restarting")
            .unwrap()
            .starts_with(&moved));
        assert!(restarted.compare_in(&moved, &id, &id).is_ok());
        assert!(restarted.status_in(&moved, "../escape").is_err());
        assert!(restarted.status_in(&base, &id).is_err());

        let root = moved.join("renders");
        std::fs::create_dir(root.join("bad")).unwrap();
        std::fs::write(root.join("bad/report.json"), b"{truncated").unwrap();
        let history = restarted.history(&moved, 0, 100).unwrap();
        assert_eq!(history["renders"].as_array().unwrap().len(), 1);
        assert_eq!(history["errors"].as_array().unwrap().len(), 1);
        let first = restarted.history(&moved, 0, 1).unwrap();
        assert_eq!(first["next_offset"], 1);
        assert!(restarted.history(&moved, 1, 1).unwrap()["next_offset"].is_null());
        assert!(restarted.history(&moved, 0, 101).is_err());

        let directory = root.join(&id);
        let mut saved = restored.clone();
        saved.state = "running".into();
        write_report(&directory, &saved).unwrap();
        assert_eq!(
            restarted.status_in(&moved, &id).unwrap().state,
            "unavailable"
        );
        assert!(restarted.rate_in(&moved, &id, 3, "").is_err());
        // Reads never rewrite another process's report.
        let disk: JobReport =
            serde_json::from_slice(&std::fs::read(directory.join("report.json")).unwrap()).unwrap();
        assert_eq!(disk.state, "running");
        saved.id = "wrong-id".into();
        write_report(&directory, &saved).unwrap();
        assert!(restarted
            .status_in(&moved, &id)
            .unwrap_err()
            .contains("id does not match"));
        std::fs::remove_dir_all(moved).unwrap();
    }
}

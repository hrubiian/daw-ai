//! Project-file access for the MCP server: load (default when missing),
//! validate-before-write, atomic save (JAM-safe: the GUI's mtime poll
//! never sees a torn file).

use daw_core::project::{format, Project};
use std::path::PathBuf;

pub struct ProjectStore {
    pub path: PathBuf,
}

impl ProjectStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Result<Project, String> {
        if !self.path.exists() {
            return Ok(Project::default_project());
        }
        format::load(&self.path).map_err(|e| format!("cannot load {}: {e}", self.path.display()))
    }

    /// Validate by JSON round-trip, then write `<path>.tmp` and rename.
    pub fn save(&self, project: &Project) -> Result<(), String> {
        let json = serde_json::to_string_pretty(project).map_err(|e| e.to_string())?;
        let _: Project = serde_json::from_str(&json)
            .map_err(|e| format!("project failed validation, not saved: {e}"))?;
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, &json).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| format!("rename to {}: {e}", self.path.display()))?;
        Ok(())
    }

    /// Load → apply `op` → save. The op's Ok(String) is the tool reply body;
    /// on Err nothing reaches disk.
    pub fn mutate(
        &self,
        op: impl FnOnce(&mut Project) -> Result<String, String>,
    ) -> Result<String, String> {
        let mut p = self.load()?;
        let msg = op(&mut p)?;
        self.save(&p)?;
        Ok(msg)
    }
}

/// Compact orientation text: the cheap first call, and appended to every
/// mutation reply so the model never goes stale.
pub fn summary(p: &Project) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let m = &p.project;
    let lr = &p.loop_region;
    let _ = writeln!(
        s,
        "{} — bpm {} · {} {} · swing {:.2} · loop bars {}..{} ({}{}{})",
        m.name,
        m.bpm,
        m.key,
        m.scale,
        m.swing,
        lr.start_bar,
        lr.end_bar,
        if lr.enabled { "on" } else { "off" },
        if lr.punch_in { " +punch-in" } else { "" },
        if lr.punch_out { " +punch-out" } else { "" }
    );
    let _ = writeln!(s, "tracks:");
    for t in &p.tracks {
        let inst = match &t.instrument {
            daw_core::project::Instrument::Soundfont { preset, bank, .. } => {
                format!("sf2 {bank}:{preset}")
            }
            daw_core::project::Instrument::AudioFile { .. } => "audio-file".into(),
            daw_core::project::Instrument::Sampler { pads } => {
                format!("sampler({} pads)", pads.len())
            }
            daw_core::project::Instrument::Clap(c) => format!("clap {}", c.id),
        };
        let clap_fx = if t.clap_fx.is_empty() {
            String::new()
        } else {
            format!(
                " clap_fx[{}]",
                t.clap_fx
                    .iter()
                    .map(|c| format!("{}={}", c.uid, c.id))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        let _ = writeln!(
            s,
            "  {} \"{}\" {:?} {} vol {:.2} pan {:+.1}{} rev {:.2} dly {:.2}{}",
            t.id,
            t.name,
            t.track_type,
            inst,
            t.volume,
            t.pan,
            if t.muted { " MUTED" } else { "" },
            t.effects.reverb,
            t.effects.send_delay,
            clap_fx
        );
        let _ = writeln!(
            s,
            "    patterns {:?} | clips {:?}",
            t.patterns,
            p.arrangement
                .iter()
                .filter(|a| a.track == t.id)
                .map(|a| (a.bar, a.pattern.clone(), a.len_bars))
                .collect::<Vec<_>>()
        );
    }
    let _ = writeln!(s, "patterns:");
    for pat in &p.patterns {
        match pat {
            daw_core::project::Pattern::Step(sp) => {
                let _ = writeln!(s, "  {} step {} steps × {} rows", sp.id, sp.steps, sp.rows.len());
            }
            daw_core::project::Pattern::PianoRoll(pr) => {
                let _ = writeln!(s, "  {} piano-roll {} bars, {} notes", pr.id, pr.bars, pr.notes.len());
            }
        }
    }
    if !p.audio_clips.is_empty() {
        let _ = writeln!(s, "audio clips:");
        for (i, c) in p.audio_clips.iter().enumerate() {
            let _ = writeln!(
                s,
                "  [{i}] {} on {} @ {:.2}s len {:.2}s gain {:.2}",
                c.path, c.track, c.start_sec, c.len_sec, c.gain
            );
        }
    }
    let _ = writeln!(
        s,
        "returns: reverb gain {:.2}{} room {:.2} | delay gain {:.2}{} {}ms fb {:.2}",
        p.returns.reverb.gain,
        if p.returns.reverb.muted { " MUTED" } else { "" },
        p.returns.reverb.room_size,
        p.returns.delay.gain,
        if p.returns.delay.muted { " MUTED" } else { "" },
        p.returns.delay.time_ms,
        p.returns.delay.feedback
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_project(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("daw_mcp_store_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("song.daw.json")
    }

    #[test]
    fn creates_default_on_missing_and_round_trips() {
        let path = tmp_project("roundtrip");
        let _ = std::fs::remove_file(&path);
        let store = ProjectStore::new(path.clone());
        let p = store.load().unwrap();
        assert_eq!(p.tracks.len(), 2, "default project");
        store.save(&p).unwrap();
        assert!(path.exists());
        assert!(!path.with_extension("tmp").exists(), "atomic save leaves no temp file");
        let p2 = store.load().unwrap();
        assert_eq!(p2.tracks.len(), 2);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn mutate_applies_validates_and_saves() {
        let path = tmp_project("mutate");
        let _ = std::fs::remove_file(&path);
        let store = ProjectStore::new(path.clone());
        let out = store
            .mutate(|p| {
                p.project.bpm = 99.0;
                Ok("ok".to_string())
            })
            .unwrap();
        assert_eq!(out, "ok");
        assert_eq!(store.load().unwrap().project.bpm, 99.0);
        store.mutate(|_| Err("nope".to_string())).unwrap_err();
        assert_eq!(store.load().unwrap().project.bpm, 99.0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn summary_lists_tracks_patterns_region() {
        let p = daw_core::project::Project::default_project();
        let s = summary(&p);
        assert!(s.contains("Drums"), "{s}");
        assert!(s.contains("Bass"));
        assert!(s.contains("bpm 120"));
        assert!(s.contains("loop"), "region line present");
        assert!(s.contains("p1") && s.contains("p2"), "pattern ids");
    }
}

//! Project-file access for the MCP server: load (default when missing),
//! validate-before-write, atomic save (JAM-safe: the GUI's mtime poll
//! never sees a torn file).

use daw_core::project::{format, Project};
use std::path::PathBuf;

pub struct ProjectStore {
    pub path: PathBuf,
    pub live_socket: Option<PathBuf>,
    revision: std::sync::Mutex<Option<format::Revision>>,
}

impl ProjectStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path, live_socket: None, revision: std::sync::Mutex::new(None) }
    }

    pub fn live(socket: PathBuf) -> Result<Self, String> {
        let mut store = Self::new(std::env::current_dir().map_err(|e| e.to_string())?.join("live.daw.json"));
        store.live_socket = Some(socket);
        let state = store.inspect_live()?;
        if let Some(base) = state.project_directory { store.path = base.join("live.daw.json"); }
        Ok(store)
    }
    pub fn inspect_live(&self) -> Result<daw_core::session::Response, String> {
        self.live_request(daw_core::session::Request { version: 1, operation_id: daw_io::session::operation_id(), expected_revision: None, command: daw_core::session::Command::Inspect })
    }
    pub fn live_request(&self, request: daw_core::session::Request) -> Result<daw_core::session::Response, String> {
        let socket = self.live_socket.as_ref().ok_or("not connected to a live session")?;
        daw_io::session::call(socket, &request).map_err(|e| format!("live session unavailable (no file fallback): {e}"))
    }
    pub fn load(&self) -> Result<Project, String> {
        if self.live_socket.is_some() { return Ok(self.inspect_live()?.project); }
        let mut revision = self.revision.lock().map_err(|e| e.to_string())?;
        if !self.path.exists() { *revision = None; return Ok(Project::default_project()); }
        let (p, r) = format::load_versioned(&self.path).map_err(|e| e.to_string())?;
        *revision = Some(r);
        Ok(p)
    }

    pub fn save(&self, project: &Project) -> Result<(), String> {
        if self.live_socket.is_some() { return Err("live writes require a revision-checked mutation".into()); }
        let mut revision = self.revision.lock().map_err(|e| e.to_string())?;
        let next = format::save_if_unchanged(project, &self.path, revision.as_deref()).map_err(|e| e.to_string())?;
        *revision = Some(next);
        Ok(())
    }

    /// Load → apply `op` → save. The op's Ok(String) is the tool reply body;
    /// on Err nothing reaches disk.
    pub fn mutate(
        &self,
        op: impl FnOnce(&mut Project) -> Result<String, String>,
    ) -> Result<String, String> {
        if self.live_socket.is_some() {
            let snapshot = self.inspect_live()?;
            let mut p = snapshot.project;
            let msg = op(&mut p)?;
            let action: String = msg.lines().next().unwrap_or("edit").chars().take(100).collect();
            let response = self.live_request(daw_core::session::Request { version: 1, operation_id: daw_io::session::operation_id(), expected_revision: Some(snapshot.revision), command: daw_core::session::Command::Batch { label: format!("Agent: {action}"), edits: vec![daw_core::session::Edit::Replace { project: p }] } })?;
            if let Some(e) = response.error { return Err(format!("{}: {}", e.code, e.message)); }
            return Ok(format!("{msg} · revision {} · engine {}", response.revision, response.engine_status));
        }
        let (mut p, expected) = if self.path.exists() {
            let (p, r) = format::load_versioned(&self.path).map_err(|e| e.to_string())?;
            (p, Some(r))
        } else { (Project::default_project(), None) };
        let msg = op(&mut p)?;
        let next = format::save_if_unchanged(&p, &self.path, expected.as_deref()).map_err(|e| e.to_string())?;
        *self.revision.lock().map_err(|e| e.to_string())? = Some(next);
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
    let tc = if p.tempo_changes.is_empty() {
        String::new()
    } else {
        format!(" (+{} tempo)", p.tempo_changes.len())
    };
    let _ = writeln!(
        s,
        "{} — bpm {}{} · {}/{} · {} {} · swing {:.2} · timing {:.2} · loop bars {}..{} ({}{}{})",
        m.name,
        m.bpm,
        tc,
        m.time_signature[0],
        m.time_signature[1],
        m.key,
        m.scale,
        m.swing,
        m.timing,
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
                // Pad pitches (g = gated: follows note length) so an agent
                // can write notes for them without fetching the whole project.
                let list = pads.iter().take(24)
                    .map(|p| format!("{}{}", p.pitch, if p.gate { "g" } else { "" }))
                    .collect::<Vec<_>>().join(",");
                let lfos = if t.modulators.is_empty() { String::new() } else {
                    format!(" lfo[{}]", t.modulators.iter().map(|m| m.dest.label()).collect::<Vec<_>>().join(","))
                };
                format!("sampler(pads {list}){lfos}")
            }
            daw_core::project::Instrument::Clap(c) => format!("clap {}", c.id),
            daw_core::project::Instrument::Sfz { path } => {
                let name = std::path::Path::new(path).file_name().and_then(|s| s.to_str()).unwrap_or(path);
                format!("sfz {name}")
            }
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
        let auto = if t.volume_automation.is_empty() {
            String::new()
        } else {
            format!(" auto[{}]", t.volume_automation.len())
        };
        let _ = writeln!(
            s,
            "  {} \"{}\" {:?} {} vol {:.2}{} pan {:+.1}{} rev {:.2} dly {:.2}{}",
            t.id,
            t.name,
            t.track_type,
            inst,
            t.volume,
            auto,
            t.pan,
            if t.muted { " MUTED" } else { "" },
            t.effects.reverb,
            t.effects.send_delay,
            clap_fx
        );
        let _ = writeln!(
            s,
            "    patterns {:?} | clips [{}]",
            t.patterns,
            p.arrangement
                .iter()
                .filter(|a| a.track == t.id)
                .map(|a| {
                    let off = if a.offset_ticks != 0 { format!("+{}t", a.offset_ticks) } else { String::new() };
                    let len = a.len_bars.map(|n| format!("×{n}")).unwrap_or_default();
                    let gain = if (a.gain - 1.0).abs() > 1e-3 { format!("*{:.2}", a.gain) } else { String::new() };
                    format!("{}@{}{}{}{}", a.pattern, a.bar, off, len, gain)
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
        for comment in &t.comments {
            let location = comment.bar.map_or_else(|| "whole track".to_owned(), |bar| {
                format!("bar {} (bar index {bar})", bar + 1)
            });
            let _ = writeln!(s, "    comment on {location}: {:?}", comment.text);
        }
    }
    let _ = writeln!(s, "patterns:");
    for pat in &p.patterns {
        match pat {
            daw_core::project::Pattern::Step(sp) => {
                let _ = writeln!(s, "  {} step {} steps × {} rows", sp.id, sp.steps, sp.rows.len());
            }
            daw_core::project::Pattern::PianoRoll(pr) => {
                let leg = if pr.legato {
                    format!(" leg(+{}t)", pr.legato_overlap_ticks)
                } else {
                    String::new()
                };
                let cc = if pr.cc_events.is_empty() {
                    String::new()
                } else {
                    format!(" cc[{}]", pr.cc_events.len())
                };
                let pb = if pr.pitch_bends.is_empty() {
                    String::new()
                } else {
                    format!(" pb[{}]", pr.pitch_bends.len())
                };
                let _ = writeln!(s, "  {} piano-roll {} bars, {} notes{}{}{}", pr.id, pr.bars, pr.notes.len(), leg, cc, pb);
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
    fn summary_lists_tracks_patterns_region_and_comments() {
        let mut p = daw_core::project::Project::default_project();
        p.tracks[0].set_comment(None, "Keep the kick dry.").unwrap();
        p.tracks[1].set_comment(Some(4), "Leave room for the guitar.").unwrap();
        let s = summary(&p);
        assert!(s.contains("Drums"), "{s}");
        assert!(s.contains("Bass"));
        assert!(s.contains("bpm 120"));
        assert!(s.contains("loop"), "region line present");
        assert!(s.contains("p1") && s.contains("p2"), "pattern ids");
        assert!(s.contains("comment on whole track: \"Keep the kick dry.\""));
        assert!(s.contains("comment on bar 5 (bar index 4): \"Leave room for the guitar.\""));
    }
}

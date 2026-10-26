//! Sync tool core: every method = load → pure daw-core op → validate →
//! atomic save, returning a text reply ending in the fresh project summary.
//! The rmcp wrappers in `main.rs` are one-liners over this, so everything
//! here is testable without MCP plumbing.

use crate::store::{summary, ProjectStore};
use daw_core::project::{Note, Pattern, Project, TrackType};
use std::sync::OnceLock;

pub struct DawTools {
    pub store: ProjectStore,
    pub sf2_path: std::path::PathBuf,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct NoteArg {
    /// MIDI pitch (60 = C4).
    pub pitch: u8,
    /// Start tick within the pattern (480 ticks per 4/4 bar, 120 per beat).
    pub tick: u32,
    /// Length in ticks.
    pub duration: u32,
    /// Velocity 1-127.
    pub vel: u8,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct StepRowArg {
    /// MIDI pitch this row triggers (GM drums: 36 kick, 38 snare, 42 hat…).
    pub pitch: u8,
    /// One char per step: 'x'/'X' = hit, '.' = rest, '1'-'9' = velocity*14.
    pub steps: String,
}

/// Readable GM drum-row names for set_steps (falls back to "p<pitch>").
fn gm_drum_name(pitch: u8) -> String {
    match pitch {
        35 | 36 => "Kick".into(),
        37 => "Rim".into(),
        38 | 40 => "Snare".into(),
        39 => "Clap".into(),
        42 => "CHat".into(),
        44 => "PHat".into(),
        46 => "OHat".into(),
        49 | 57 => "Crash".into(),
        51 | 59 => "Ride".into(),
        other => format!("p{other}"),
    }
}

fn track_ids(p: &Project) -> String {
    p.tracks.iter().map(|t| t.id.clone()).collect::<Vec<_>>().join(", ")
}

fn clap_catalog() -> &'static std::sync::Mutex<std::sync::Arc<Vec<daw_core::audio::clap::ClapPluginDesc>>> {
    static CACHE: OnceLock<std::sync::Mutex<std::sync::Arc<Vec<daw_core::audio::clap::ClapPluginDesc>>>> = OnceLock::new();
    CACHE.get_or_init(||std::sync::Mutex::new(std::sync::Arc::new(daw_core::audio::clap::discover())))
}
fn clap_cache() -> std::sync::Arc<Vec<daw_core::audio::clap::ClapPluginDesc>> {
    clap_catalog().lock().unwrap_or_else(|e|e.into_inner()).clone()
}

impl DawTools {
    pub fn new(path: std::path::PathBuf) -> Self {
        let sf2_path = std::env::var("DAW_SF2")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("assets/GeneralUser.sf2"));
        Self { store: ProjectStore::new(path), sf2_path }
    }

    fn with_summary(&self, head: String) -> Result<String, String> {
        let p = self.store.load()?;
        let live = if self.store.live_socket.is_some() {
            let state = self.store.inspect_live()?;
            format!("\nSession revision {} · engine {}", state.revision, state.engine_status)
        } else { String::new() };
        Ok(format!("{head}{live}\n\n{}", summary(&p)))
    }

    // ── Orientation ──────────────────────────────────────────────────────────

    pub fn project_summary(&self) -> Result<String, String> {
        Ok(summary(&self.store.load()?))
    }

    pub fn get_project(&self) -> Result<String, String> {
        serde_json::to_string_pretty(&self.store.load()?).map_err(|e| e.to_string())
    }

    pub fn set_project(&self, json: String) -> Result<String, String> {
        let p: Project = serde_json::from_str(&json)
            .map_err(|e| format!("invalid project JSON (nothing saved): {e}"))?;
        self.store.mutate(|current| { *current = p; Ok("project replaced".into()) })?;
        self.with_summary("project replaced".into())
    }

    // ── Tracks + tempo ───────────────────────────────────────────────────────

    pub fn add_track(&self, track_type: String) -> Result<String, String> {
        let ty = match track_type.as_str() {
            "drum" => TrackType::Drum,
            "melodic" => TrackType::Melodic,
            "audio" => TrackType::Audio,
            other => return Err(format!("unknown track_type '{other}' (drum|melodic|audio)")),
        };
        let id = self.store.mutate(|p| Ok(p.add_track(ty)))?;
        self.with_summary(format!("added track {id}"))
    }

    pub fn remove_track(&self, track_id: String) -> Result<String, String> {
        self.store.mutate(|p| {
            if p.remove_track(&track_id) {
                Ok(format!("removed {track_id}"))
            } else {
                Err(format!("no track '{track_id}' — tracks: {}", track_ids(p)))
            }
        })?;
        self.with_summary(format!("removed {track_id}"))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_track(
        &self,
        track_id: String,
        volume: Option<f32>,
        pan: Option<f32>,
        muted: Option<bool>,
        name: Option<String>,
        preset: Option<u8>,
        soundfont: Option<String>,
    ) -> Result<String, String> {
        self.store.mutate(|p| {
            {
                let Some(t) = p.tracks.iter_mut().find(|t| t.id == track_id) else {
                    return Err(format!("no track '{track_id}' — tracks: {}", track_ids(p)));
                };
                if let Some(v) = volume {
                    t.volume = v.clamp(0.0, 2.0); // 1.0 = unity, up to +6 dB boost
                }
                if let Some(v) = pan {
                    t.pan = v.clamp(-1.0, 1.0);
                }
                if let Some(v) = muted {
                    t.muted = v;
                }
                if let Some(v) = name {
                    t.name = v;
                }
                if let Some(v) = soundfont {
                    if let daw_core::project::Instrument::Soundfont { soundfont: sf, .. } =
                        &mut t.instrument
                    {
                        *sf = if v.is_empty() { None } else { Some(v) };
                    }
                }
            }
            if let Some(pr) = preset {
                p.set_track_preset(&track_id, pr);
            }
            Ok(format!("updated {track_id}"))
        })?;
        self.with_summary(format!("updated {track_id}"))
    }

    pub fn set_tempo(&self, bpm: f64) -> Result<String, String> {
        self.store.mutate(|p| {
            p.project.bpm = bpm.clamp(20.0, 300.0);
            Ok(format!("bpm {}", p.project.bpm))
        })?;
        self.with_summary(format!("tempo set to {bpm}"))
    }

    pub fn set_swing(&self, amount: f32) -> Result<String, String> {
        self.store.mutate(|p| {
            p.set_swing(amount);
            Ok(format!("swing {:.2}", p.project.swing))
        })?;
        self.with_summary(format!("swing set to {amount}"))
    }

    pub fn set_timing(&self, amount: f32) -> Result<String, String> {
        self.store.mutate(|p| { p.set_timing(amount); Ok(format!("timing {:.2}", p.project.timing)) })?;
        self.with_summary(format!("timing set to {amount}"))
    }

    pub fn set_legato(&self, pattern_id: String, on: bool) -> Result<String, String> {
        self.store.mutate(|p| {
            if p.set_legato(&pattern_id, on) { Ok(format!("legato {on}")) }
            else { Err(format!("no piano-roll pattern '{pattern_id}'")) }
        })?;
        self.with_summary(format!("legato {on} on {pattern_id}"))
    }

    // ── Patterns + notes ─────────────────────────────────────────────────────

    pub fn add_pattern(&self, track_id: String) -> Result<String, String> {
        let pid = self.store.mutate(|p| {
            p.add_pattern(&track_id)
                .ok_or_else(|| format!("no track '{track_id}' — tracks: {}", track_ids(p)))
        })?;
        self.with_summary(format!("added pattern {pid} on {track_id}"))
    }

    pub fn set_steps(&self, pattern_id: String, rows: Vec<StepRowArg>) -> Result<String, String> {
        let nrows = rows.len();
        self.store.mutate(|p| {
            let Some(Pattern::Step(sp)) = p.patterns.iter_mut().find(|x| x.id() == pattern_id)
            else {
                return Err(format!("no step pattern '{pattern_id}'"));
            };
            sp.rows = rows
                .iter()
                .map(|r| daw_core::project::StepRow {
                    name: gm_drum_name(r.pitch),
                    pitch: r.pitch,
                    vel: 100,
                    steps: r
                        .steps
                        .chars()
                        .map(|c| match c {
                            'x' | 'X' => 1u8,
                            '1'..='9' => (c as u8 - b'0') * 14,
                            _ => 0,
                        })
                        .collect(),
                })
                .collect();
            let len = rows.iter().map(|r| r.steps.len()).max().unwrap_or(16) as u16;
            sp.steps = len.max(1);
            Ok(format!("{nrows} rows set on {pattern_id}"))
        })?;
        self.with_summary(format!("steps set on {pattern_id}"))
    }

    pub fn set_notes(&self, pattern_id: String, notes: Vec<NoteArg>) -> Result<String, String> {
        let count = notes.len();
        self.store.mutate(|p| {
            let Some(Pattern::PianoRoll(pr)) = p.patterns.iter_mut().find(|x| x.id() == pattern_id)
            else {
                return Err(format!("no piano-roll pattern '{pattern_id}'"));
            };
            pr.notes = notes
                .iter()
                .map(|n| Note { pitch: n.pitch, tick: n.tick, duration: n.duration, vel: n.vel })
                .collect();
            Ok(format!("{count} notes set on {pattern_id}"))
        })?;
        self.with_summary(format!("notes set on {pattern_id}"))
    }

    pub fn apply_mini_notation(&self, track_id: String, source: String) -> Result<String, String> {
        let n = self.store.mutate(|p| {
            p.apply_mini_notation(&track_id, &source).map(|n| n.to_string())
        })?;
        self.with_summary(format!("mini-notation applied to {track_id}: {n} notes"))
    }

    // ── Arrangement ──────────────────────────────────────────────────────────

    pub fn place_clip(
        &self,
        track_id: String,
        pattern_id: String,
        bar: u32,
        len_bars: Option<u32>,
        offset_ticks: Option<u32>,
    ) -> Result<String, String> {
        self.store.mutate(|p| {
            if !p.place_pattern_id(&track_id, bar, &pattern_id) {
                return Err(format!(
                    "cannot place '{pattern_id}' on '{track_id}' at bar {bar} — tracks: {}",
                    track_ids(p)
                ));
            }
            if let Some(len) = len_bars {
                p.resize_clip(&track_id, bar, len);
            }
            if let Some(off) = offset_ticks {
                p.set_clip_offset(&track_id, bar, off);
            }
            Ok(format!("placed {pattern_id} on {track_id} @ bar {bar}"))
        })?;
        self.with_summary(format!("placed {pattern_id} at bar {bar}"))
    }

    /// Nudge an existing clip off the bar line by `offset_ticks` (480/bar; e.g.
    /// 240 = half a bar, 120 = one beat). Whole bars fold into the slot's bar.
    pub fn set_clip_offset(&self, track_id: String, bar: u32, offset_ticks: u32) -> Result<String, String> {
        self.store.mutate(|p| {
            if p.set_clip_offset(&track_id, bar, offset_ticks) {
                Ok("offset set".into())
            } else {
                Err(format!("no clip on '{track_id}' at bar {bar}"))
            }
        })?;
        self.with_summary(format!("clip at bar {bar} on {track_id} offset {offset_ticks}t"))
    }

    pub fn remove_clip(&self, track_id: String, bar: u32) -> Result<String, String> {
        self.store.mutate(|p| {
            if p.remove_clip(&track_id, bar) {
                Ok("removed".into())
            } else {
                Err(format!("no clip on '{track_id}' at bar {bar}"))
            }
        })?;
        self.with_summary(format!("removed clip at bar {bar} on {track_id}"))
    }

    pub fn resize_clip(&self, track_id: String, bar: u32, len_bars: u32) -> Result<String, String> {
        self.store.mutate(|p| {
            if p.resize_clip(&track_id, bar, len_bars) {
                Ok("resized".into())
            } else {
                Err(format!("no clip on '{track_id}' at bar {bar}"))
            }
        })?;
        self.with_summary(format!("clip at bar {bar} resized to {len_bars} bars"))
    }

    // ── Effects / sends / returns / region ───────────────────────────────────

    pub fn set_sampler_variants(&self, track_id: String, pitch: u8, variants: serde_json::Value) -> Result<String, String> {
        let variants: Vec<daw_core::project::SampleVariant> = serde_json::from_value(variants).map_err(|e| e.to_string())?;
        self.store.mutate(|p| {
            p.set_sampler_variants(&track_id, pitch, variants)?;
            Ok("sampler takes updated".into())
        })?;
        self.with_summary(format!("velocity layers / alternate takes updated on {track_id} p{pitch}"))
    }

    pub fn set_effects(&self, track_id: String, params: serde_json::Value) -> Result<String, String> {
        self.store.mutate(|p| {
            let Some(t) = p.tracks.iter_mut().find(|t| t.id == track_id) else {
                return Err(format!("no track '{track_id}' — tracks: {}", track_ids(p)));
            };
            let mut fx_json = serde_json::to_value(&t.effects).map_err(|e| e.to_string())?;
            let obj = params.as_object().ok_or("params must be an object")?;
            let valid = fx_json.as_object().cloned().unwrap_or_default();
            for (k, v) in obj {
                if !valid.contains_key(k) {
                    return Err(format!(
                        "unknown effect field '{k}' — valid: {}",
                        valid.keys().cloned().collect::<Vec<_>>().join(", ")
                    ));
                }
                fx_json[k.as_str()] = v.clone();
            }
            t.effects = serde_json::from_value(fx_json).map_err(|e| e.to_string())?;
            Ok(format!("effects updated on {track_id}"))
        })?;
        self.with_summary(format!("effects updated on {track_id}"))
    }

    pub fn set_returns(&self, params: serde_json::Value) -> Result<String, String> {
        self.store.mutate(|p| {
            let mut json = serde_json::to_value(&p.returns).map_err(|e| e.to_string())?;
            let obj = params.as_object().ok_or("params must be an object: {\"reverb\": {..}, \"delay\": {..}}")?;
            for (k, v) in obj {
                let Some(slot) = json.get_mut(k.as_str()) else {
                    return Err(format!("unknown return '{k}' — valid: reverb, delay"));
                };
                let fields = v.as_object().ok_or("return params must be objects")?;
                let valid = slot.as_object().cloned().unwrap_or_default();
                for (fk, fv) in fields {
                    if !valid.contains_key(fk) {
                        return Err(format!(
                            "unknown field '{fk}' on {k} — valid: {}",
                            valid.keys().cloned().collect::<Vec<_>>().join(", ")
                        ));
                    }
                    slot[fk.as_str()] = fv.clone();
                }
            }
            p.returns = serde_json::from_value(json).map_err(|e| e.to_string())?;
            Ok("returns updated".into())
        })?;
        self.with_summary("returns updated".into())
    }

    pub fn apply_spatial_preset(&self, name: String) -> Result<String, String> {
        let preset: daw_core::project::SpatialPreset = serde_json::from_value(serde_json::Value::String(name))
            .map_err(|e| format!("unknown spatial preset: {e}"))?;
        self.store.mutate(|p| { p.apply_spatial_preset(preset); Ok("space preset applied".into()) })?;
        self.with_summary(format!("{} applied; shared bus gain/mute preserved",preset.label()))
    }

    pub fn set_loop_region(
        &self,
        start_bar: u32,
        end_bar: u32,
        enabled: Option<bool>,
        punch_in: Option<bool>,
        punch_out: Option<bool>,
    ) -> Result<String, String> {
        self.store.mutate(|p| {
            p.set_loop_region(start_bar, end_bar);
            if let Some(v) = enabled {
                p.loop_region.enabled = v;
            }
            if let Some(v) = punch_in {
                p.loop_region.punch_in = v;
            }
            if let Some(v) = punch_out {
                p.loop_region.punch_out = v;
            }
            Ok("region set".into())
        })?;
        self.with_summary(format!("loop region bars {start_bar}..{end_bar}"))
    }

    // ── Audio clips ──────────────────────────────────────────────────────────

    pub fn add_audio_clip(&self, track_id: String, path: String, start_sec: f64) -> Result<String, String> {
        let src_len = daw_core::audio::wav_duration_sec(std::path::Path::new(&path))
            .map_err(|e| format!("cannot read {path}: {e}"))?;
        let idx = self.store.mutate(|p| {
            if !p.tracks.iter().any(|t| t.id == track_id) {
                return Err(format!("no track '{track_id}' — tracks: {}", track_ids(p)));
            }
            Ok(p.add_audio_clip(&track_id, &path, start_sec, src_len).to_string())
        })?;
        self.with_summary(format!("audio clip [{idx}] added on {track_id}"))
    }

    pub fn audio_clip(
        &self,
        index: usize,
        move_to_sec: Option<f64>,
        gain: Option<f32>,
        len_sec: Option<f64>,
        remove: Option<bool>,
    ) -> Result<String, String> {
        self.store.mutate(|p| {
            if index >= p.audio_clips.len() {
                return Err(format!("no audio clip [{index}] — {} clips", p.audio_clips.len()));
            }
            if remove.unwrap_or(false) {
                p.remove_audio_clip(index);
                return Ok(format!("clip [{index}] removed"));
            }
            if let Some(s) = move_to_sec {
                p.move_audio_clip(index, s);
            }
            if let Some(g) = gain {
                p.set_audio_clip_gain(index, g);
            }
            if let Some(l) = len_sec {
                p.crop_audio_clip_end(index, l);
            }
            Ok(format!("clip [{index}] updated"))
        })?;
        self.with_summary(format!("audio clip [{index}] updated"))
    }

    // ── CLAP ─────────────────────────────────────────────────────────────────

    pub fn list_clap_plugins(&self) -> Result<String, String> {
        let plugs = clap_cache();
        if plugs.is_empty() {
            return Ok("no CLAP plugins found ($CLAP_PATH, ~/.clap, /usr/lib/clap)".into());
        }
        Ok(plugs
            .iter()
            .map(|d| {
                format!(
                    "{} — {} [{}{}] {}",
                    d.id,
                    d.name,
                    if d.is_instrument { "I" } else { "" },
                    if d.is_effect { "FX" } else { "" },
                    d.path
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    pub fn rescan_clap_plugins(&self) -> Result<String, String> {
        let found=std::sync::Arc::new(daw_core::audio::clap::discover());
        *clap_catalog().lock().unwrap_or_else(|e|e.into_inner())=found;
        self.list_clap_plugins()
    }

    /// Read plugin-declared parameter bounds/defaults from a separate instance.
    /// Saved overrides are identified explicitly; this does not claim to read
    /// an active plugin's meter, automation or unsaved internal state.
    pub fn get_clap_params(&self, uid: String) -> Result<String, String> {
        let (project,base)=if self.store.live_socket.is_some() {
            let state=self.store.inspect_live()?;
            (state.project,state.project_directory.unwrap_or_default())
        } else {(self.store.load()?,self.store.path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf())};
        let resolved=daw_core::project::format::resolve_assets(&project,&base,std::path::Path::new("."));
        let spec=daw_core::audio::clap::slot_specs(&resolved).into_iter().find(|s|s.uid==uid)
            .ok_or_else(||format!("no CLAP slot '{uid}'"))?;
        let mut instance=daw_core::audio::clap::ClapInstance::from_spec(&spec,44100)?;
        let params: Vec<_>=instance.handle.param_infos.iter().map(|p|serde_json::json!({
            "id":p.id,"name":p.name,"min":p.min,"max":p.max,"default":p.default,"stepped":p.stepped,"readonly":p.readonly,
            "saved_override":spec.params.iter().find(|(id,_)|*id==p.id).map(|(_,value)|*value)
        })).collect();
        let name=instance.handle.name.clone();
        if let Some(processor)=instance.processor.take() {instance.handle.deactivate(processor);}
        Ok(serde_json::json!({"uid":uid,"name":name,"source":"plugin metadata and saved overrides; not live readback","params":params}).to_string())
    }

    fn resolve_clap(&self, plugin_id: &str) -> Result<(String, String), String> {
        clap_cache()
            .iter()
            .find(|d| d.id == plugin_id)
            .map(|d| (d.path.clone(), d.id.clone()))
            .ok_or_else(|| {
                format!(
                    "no CLAP plugin '{plugin_id}' — available: {}",
                    clap_cache().iter().map(|d| d.id.clone()).collect::<Vec<_>>().join(", ")
                )
            })
    }

    pub fn add_clap_fx(&self, track_id: String, plugin_id: String) -> Result<String, String> {
        let (path, id) = self.resolve_clap(&plugin_id)?;
        let uid = self.store.mutate(|p| {
            p.add_clap_fx(&track_id, &path, &id)
                .ok_or_else(|| format!("no track '{track_id}' — tracks: {}", track_ids(p)))
        })?;
        self.with_summary(format!("clap fx {plugin_id} added as {uid} on {track_id}"))
    }

    pub fn set_clap_instrument(&self, track_id: String, plugin_id: String) -> Result<String, String> {
        let (path, id) = self.resolve_clap(&plugin_id)?;
        let uid = self.store.mutate(|p| {
            p.set_track_clap_instrument(&track_id, &path, &id)
                .ok_or_else(|| format!("no track '{track_id}' — tracks: {}", track_ids(p)))
        })?;
        self.with_summary(format!("{track_id} instrument = {plugin_id} ({uid})"))
    }

    pub fn remove_clap_fx(&self, track_id: String, uid: String) -> Result<String, String> {
        self.store.mutate(|p| {
            if p.remove_clap_fx(&track_id, &uid) {
                Ok("removed".into())
            } else {
                Err(format!("no clap slot '{uid}' on '{track_id}'"))
            }
        })?;
        self.with_summary(format!("clap slot {uid} removed"))
    }

    pub fn set_clap_param(&self, uid: String, param_id: u32, value: f64) -> Result<String, String> {
        self.store.mutate(|p| {
            if p.set_clap_param(&uid, param_id, value) {
                Ok("set".into())
            } else {
                Err(format!("no clap slot with uid '{uid}'"))
            }
        })?;
        self.with_summary(format!("clap {uid} param {param_id} = {value}"))
    }

    /// Solo-render every track, measure active-bar RMS, and set volumes so
    /// all tracks hit the median loudness. Returns the per-track report.
    pub fn auto_balance(&self) -> Result<String, String> {
        let p = self.store.load()?;
        if !self.sf2_path.exists() {
            return Err(format!(
                "soundfont missing at {} — download GeneralUser.sf2 (see README)",
                self.sf2_path.display()
            ));
        }
        let plan = daw_core::audio::balance::auto_balance(&p, &self.sf2_path)
            .map_err(|e| e.to_string())?;
        let mut report = String::from("balanced:\n");
        for r in &plan {
            report.push_str(&format!(
                "  {} \"{}\" {}: vol {:.2} -> {:.2}\n",
                r.track_id,
                r.name,
                if r.rms_db.is_finite() {
                    format!("{:.1} dB", r.rms_db)
                } else {
                    "silent".into()
                },
                r.old_volume,
                r.new_volume
            ));
        }
        let analyzed_hash = daw_core::project::format::project_revision(&p).map_err(|e| e.to_string())?;
        self.store.mutate(|p| {
            if daw_core::project::format::project_revision(p).map_err(|e| e.to_string())? != analyzed_hash {
                return Err("project changed during analysis; rerun auto_balance".into());
            }
            for r in &plan {
                p.set_track_volume(&r.track_id, r.new_volume);
            }
            Ok(String::new())
        })?;
        self.with_summary(report)
    }

    /// Symbolic groove report: how notes sit against the 16th grid plus the
    /// project's swing/humanize/timing. No render needed.
    pub fn analyze_groove(&self) -> Result<String, String> {
        Ok(crate::groove::groove_report(&self.store.load()?))
    }

    /// Baseline snapshot path: `<project>.snapshot.json` beside the project.
    fn snapshot_path(&self) -> std::path::PathBuf {
        let mut p = self.store.path.clone();
        p.set_extension("snapshot.json");
        p
    }

    /// Render the project and save an analyzed fingerprint as the CI baseline.
    pub fn snapshot(&self) -> Result<String, String> {
        let p = self.store.load()?;
        if !self.sf2_path.exists() {
            return Err(format!("soundfont missing at {} — download GeneralUser.sf2 (see README)", self.sf2_path.display()));
        }
        let fp = crate::snapshot::render_fingerprint(&p, &self.sf2_path).map_err(|e| e.to_string())?;
        let path = self.snapshot_path();
        let json = serde_json::to_string_pretty(&fp).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| e.to_string())?;
        Ok(format!(
            "snapshot saved to {}: peak {:.1} dBFS, rms {:.1} dBFS, {} tracks, {} notes, {} clipped",
            path.display(), fp.peak_db, fp.rms_db, fp.tracks, fp.notes, fp.clipped_samples
        ))
    }

    /// Re-render and report regressions vs the saved snapshot baseline.
    pub fn check(&self) -> Result<String, String> {
        let p = self.store.load()?;
        if !self.sf2_path.exists() {
            return Err(format!("soundfont missing at {} — download GeneralUser.sf2 (see README)", self.sf2_path.display()));
        }
        let path = self.snapshot_path();
        let baseline: crate::snapshot::Fingerprint = std::fs::read_to_string(&path)
            .map_err(|_| format!("no snapshot at {} — run `snapshot` first", path.display()))
            .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))?;
        let current = crate::snapshot::render_fingerprint(&p, &self.sf2_path).map_err(|e| e.to_string())?;
        let regs = crate::snapshot::regressions(&baseline, &current);
        if regs.is_empty() {
            return Ok(format!("PASS: no regressions vs {}", path.display()));
        }
        let mut s = format!("{} regression(s) vs {}:\n", regs.len(), path.display());
        for r in &regs {
            s.push_str(&format!("  [{:?}] {}\n", r.level, r.msg));
        }
        Ok(s)
    }

    /// Describe the musical changes from another `.daw.json` (the "before") to
    /// the current project.
    pub fn diff_project(&self, other_path: String) -> Result<String, String> {
        let current = self.store.load()?;
        let other = daw_core::project::format::load(&other_path)
            .map_err(|e| format!("load {other_path}: {e}"))?;
        let changes = crate::diff::diff_projects(&other, &current);
        let body = changes.iter().map(|c| format!("  {c}")).collect::<Vec<_>>().join("\n");
        Ok(format!("changes from {other_path} to current project:\n{body}"))
    }

    /// Report the song's density (tracks active per bar) and dynamic arc, and
    /// flag flat/static textures or missing intro builds.
    pub fn analyze_arrangement(&self) -> Result<String, String> {
        let p = self.store.load()?;
        if !self.sf2_path.exists() {
            return Err(format!(
                "soundfont missing at {} — download GeneralUser.sf2 (see README)",
                self.sf2_path.display()
            ));
        }
        crate::arrangement::arrangement_report(&p, &self.sf2_path).map_err(|e| e.to_string())
    }

    /// Solo-render each track, profile its spectrum, and report where each
    /// track lives + which tracks mask each other (frequency collisions).
    pub fn analyze_mix(&self) -> Result<String, String> {
        let p = self.store.load()?;
        if !self.sf2_path.exists() {
            return Err(format!(
                "soundfont missing at {} — download GeneralUser.sf2 (see README)",
                self.sf2_path.display()
            ));
        }
        crate::mix_analysis::mix_report(&p, &self.sf2_path).map_err(|e| e.to_string())
    }

    // ── Listening ────────────────────────────────────────────────────────────

    /// Render `[start_bar, end_bar)` (whole song by default; `tracks` solos a
    /// subset on an in-memory copy). Returns (wav_path, bpm).
    fn render_range(
        &self,
        start_bar: Option<u32>,
        end_bar: Option<u32>,
        tracks: Option<Vec<String>>,
    ) -> Result<(std::path::PathBuf, f64), String> {
        let mut p = self.store.load()?;
        if let Some(keep) = &tracks {
            for id in keep {
                if !p.tracks.iter().any(|t| &t.id == id) {
                    return Err(format!("no track '{id}' — tracks: {}", track_ids(&p)));
                }
            }
            for t in &mut p.tracks {
                if !keep.contains(&t.id) {
                    t.muted = true;
                }
            }
        }
        if !self.sf2_path.exists() {
            return Err(format!(
                "soundfont missing at {} — download GeneralUser.sf2 (see README)",
                self.sf2_path.display()
            ));
        }
        let dir = self
            .store
            .path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("renders");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let stem = self
            .store
            .path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let s = start_bar.unwrap_or(0);
        let e_label = end_bar.map(|e| e.to_string()).unwrap_or_else(|| "end".into());
        let out = dir.join(format!("{stem}-{s}-{e_label}-{}.wav", daw_io::session::operation_id()));
        let bpm = p.project.bpm;
        let p = daw_core::project::format::resolve_assets(&p, self.store.path.parent().unwrap_or(std::path::Path::new(".")), &std::env::current_dir().map_err(|e| e.to_string())?);
        let range = if start_bar.is_some() || end_bar.is_some() { Some((s, end_bar.unwrap_or_else(|| p.song_bars()))) } else { None };
        let (samples, sr) = daw_core::audio::export::render_range_samples(&p, &self.sf2_path, false, range, &Default::default()).map_err(|e| e.to_string())?;
        let spec = hound::WavSpec { channels: 2, sample_rate: sr, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let mut writer = hound::WavWriter::create(&out, spec).map_err(|e| e.to_string())?;
        for sample in samples { writer.write_sample(sample).map_err(|e| e.to_string())?; }
        writer.finalize().map_err(|e| e.to_string())?;
        Ok((out, bpm))
    }

    pub fn render(
        &self,
        start_bar: Option<u32>,
        end_bar: Option<u32>,
        tracks: Option<Vec<String>>,
    ) -> Result<String, String> {
        let (path, _bpm) = self.render_range(start_bar, end_bar, tracks)?;
        let dur = daw_core::audio::wav_duration_sec(&path).unwrap_or(0.0);
        Ok(format!("rendered {} ({dur:.2}s) — play it to audition", path.display()))
    }

    pub fn listen(
        &self,
        start_bar: Option<u32>,
        end_bar: Option<u32>,
        tracks: Option<Vec<String>>,
    ) -> Result<String, String> {
        let (path, bpm) = self.render_range(start_bar, end_bar, tracks)?;
        let rep = crate::analysis::analyze_wav_file(&path, bpm)?;
        Ok(format!("render: {}\n{}", path.display(), rep.to_text()))
    }

    pub fn analyze_wav(&self, path: String) -> Result<String, String> {
        let bpm = self.store.load().map(|p| p.project.bpm).unwrap_or(120.0);
        let rep = crate::analysis::analyze_wav_file(std::path::Path::new(&path), bpm)?;
        Ok(rep.to_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools(name: &str) -> (DawTools, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("daw_mcp_tools_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.daw.json");
        let _ = std::fs::remove_file(&path);
        (DawTools::new(path.clone()), path)
    }

    #[test]
    fn sampler_variants_are_validated_and_preserved_by_mcp_mutations() {
        let (t,path)=tools("sampler-variants");
        t.store.mutate(|p| {
            p.tracks[0].instrument=serde_json::from_value(serde_json::json!({"type":"sampler","pads":[{"pitch":36,"path":"base.wav"}]})).unwrap();
            Ok("sampler".into())
        }).unwrap();
        t.set_sampler_variants("t1".into(),36,serde_json::json!([
            {"path":"soft.wav","min_velocity":1,"max_velocity":63},
            {"path":"soft-2.wav","min_velocity":1,"max_velocity":63}
        ])).unwrap();
        let before=std::fs::read(&path).unwrap();
        assert!(t.set_sampler_variants("t1".into(),36,serde_json::json!([
            {"path":"one.wav","min_velocity":1,"max_velocity":80},
            {"path":"two.wav","min_velocity":60,"max_velocity":127}
        ])).is_err());
        assert_eq!(before,std::fs::read(&path).unwrap());
        t.set_sampler_variants("t1".into(),36,serde_json::json!([])).unwrap();
        let p=t.store.load().unwrap();
        let daw_core::project::Instrument::Sampler { pads }=&p.tracks[0].instrument else { panic!("sampler") };
        assert!(pads[0].variants.is_empty());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn track_and_tempo_ops() {
        let (t, path) = tools("track");
        let out = t.add_track("melodic".into()).unwrap();
        assert!(out.contains("t3"), "{out}");
        t.set_track("t3".into(), Some(0.5), Some(-0.2), Some(true), Some("Lead".into()), Some(81), None)
            .unwrap();
        t.set_tempo(140.0).unwrap();
        let p = daw_core::project::format::load(&path).unwrap();
        assert_eq!(p.project.bpm, 140.0);
        let tr = p.tracks.iter().find(|x| x.id == "t3").unwrap();
        assert_eq!((tr.volume, tr.muted, tr.name.as_str()), (0.5, true, "Lead"));
        let err = t.set_track("nope".into(), Some(1.0), None, None, None, None, None).unwrap_err();
        assert!(err.contains("t1"), "error lists valid ids: {err}");
    }

    #[test]
    fn notes_steps_and_mini_notation() {
        let (t, path) = tools("notes");
        t.set_steps("p1".into(), vec![StepRowArg { pitch: 36, steps: "x...x...x...x...".into() }])
            .unwrap();
        t.set_notes("p2".into(), vec![NoteArg { pitch: 48, tick: 0, duration: 240, vel: 100 }])
            .unwrap();
        t.apply_mini_notation("t2".into(), "c3 e3 g3 c4".into()).unwrap();
        let p = daw_core::project::format::load(&path).unwrap();
        match p.patterns.iter().find(|x| x.id() == "p1").unwrap() {
            daw_core::project::Pattern::Step(sp) => {
                assert_eq!(sp.rows[0].pitch, 36);
                assert_eq!(sp.rows[0].steps.iter().filter(|&&v| v > 0).count(), 4);
            }
            _ => panic!("p1 is a step pattern"),
        }
    }

    #[test]
    fn arrange_region_effects_and_raw_json() {
        let (t, path) = tools("arrange");
        t.place_clip("t1".into(), "p1".into(), 4, Some(2), Some(240)).unwrap();
        t.set_loop_region(0, 4, Some(true), None, None).unwrap();
        t.set_effects("t1".into(), serde_json::json!({"reverb": 0.4, "drive": 0.2})).unwrap();
        let p = daw_core::project::format::load(&path).unwrap();
        assert!(p
            .arrangement
            .iter()
            .any(|a| a.track == "t1" && a.bar == 4 && a.len_bars == Some(2) && a.offset_ticks == 240));
        assert!(p.loop_region.enabled);
        let fx = &p.tracks[0].effects;
        assert_eq!((fx.reverb, fx.drive), (0.4, 0.2));
        let err = t.set_effects("t1".into(), serde_json::json!({"bogus": 1.0})).unwrap_err();
        assert!(err.contains("bogus"));
        // Raw escape hatch.
        let mut full: serde_json::Value = serde_json::from_str(&t.get_project().unwrap()).unwrap();
        full["project"]["name"] = "Renamed".into();
        t.set_project(full.to_string()).unwrap();
        assert_eq!(daw_core::project::format::load(&path).unwrap().project.name, "Renamed");
        assert!(t.set_project("{не json".into()).is_err());
    }

    #[test]
    fn auto_balance_tool_applies_volumes() {
        let sf2 = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/GeneralUser.sf2");
        if !sf2.exists() {
            assert!(std::env::var_os("DAW_REQUIRE_TEST_ASSETS").is_none(), "required audio test assets missing"); eprintln!("skip: no soundfont");
            return;
        }
        let (_, path) = tools("balance");
        let t = DawTools { store: crate::store::ProjectStore::new(path.clone()), sf2_path: sf2 };
        t.set_track("t1".into(), Some(0.2), None, None, None, None, None).unwrap();
        let out = t.auto_balance().unwrap();
        assert!(out.contains("balanced:"), "{out}");
        let p = daw_core::project::format::load(&path).unwrap();
        assert!(p.tracks[0].volume > 0.2, "quiet drums boosted, got {}", p.tracks[0].volume);
    }

    #[test]
    fn listen_renders_and_reports() {
        let sf2 = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/GeneralUser.sf2");
        if !sf2.exists() {
            assert!(std::env::var_os("DAW_REQUIRE_TEST_ASSETS").is_none(), "required audio test assets missing"); eprintln!("skip: no soundfont");
            return;
        }
        let (_, path) = tools("listen");
        let t = DawTools { store: crate::store::ProjectStore::new(path.clone()), sf2_path: sf2 };
        let report = t.listen(None, None, None).unwrap();
        assert!(report.contains("peak"), "{report}");
        assert!(report.contains("per-bar RMS"));
        assert!(report.contains(".wav"), "mentions the render path");
        // Range render: 1 bar at 120bpm = 2.0s.
        let report = t.listen(Some(0), Some(1), None).unwrap();
        assert!(report.contains("duration 2.0"), "{report}");
        // Unknown solo track errors with the valid list.
        let err = t.listen(None, None, Some(vec!["t9_missing".into()])).unwrap_err();
        assert!(err.contains("t1"), "unknown track listed: {err}");
        // Solo a real track works.
        let report = t.listen(Some(0), Some(1), Some(vec!["t2".into()])).unwrap();
        assert!(report.contains("peak"), "{report}");
    }

    #[test]
    fn returns_and_region_merge() {
        let (t, path) = tools("returns");
        t.set_returns(serde_json::json!({"reverb": {"room_size": 0.8}, "delay": {"time_ms": 500}}))
            .unwrap();
        let p = daw_core::project::format::load(&path).unwrap();
        assert_eq!(p.returns.reverb.room_size, 0.8);
        assert_eq!(p.returns.delay.time_ms, 500);
        let err = t.set_returns(serde_json::json!({"chorus": {}})).unwrap_err();
        assert!(err.contains("chorus"));
    }

    #[test]
    fn spatial_options_are_saved_and_invalid_controls_do_not_write() {
        let (t,path)=tools("spatial");
        t.set_returns(serde_json::json!({"reverb":{"gain":0.37,"predelay_ms":24,"low_cut_hz":180.,"high_cut_hz":7000.},"delay":{"gain":0.28,"sync":"eighth_dotted","modulation_ms":2.5}})).unwrap();
        t.apply_spatial_preset("tape_echo".into()).unwrap();
        let p=daw_core::project::format::load(&path).unwrap();
        assert_eq!(p.returns.delay.sync,Some(daw_core::project::EchoDivision::EighthDotted));
        assert_eq!(p.returns.delay.gain,0.28);assert_eq!(p.returns.reverb.predelay_ms,24);
        let saved=std::fs::read(&path).unwrap();
        for bad in [serde_json::json!({"delay":{"modulation_ms":21}}),serde_json::json!({"reverb":{"low_cut_hz":1800,"high_cut_hz":1000}}),serde_json::json!({"delay":{"sync":"random"}})] {
            assert!(t.set_returns(bad).is_err()); assert_eq!(std::fs::read(&path).unwrap(),saved);
        }
        assert!(t.apply_spatial_preset("unknown".into()).is_err());assert_eq!(std::fs::read(&path).unwrap(),saved);
    }

    #[test]
    fn clap_parameter_metadata_uses_real_ids_without_editing_project() {
        let (t,path)=tools("clap-metadata");
        let plugin=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/libdaw_clap_testplug_gain.so");
        assert!(plugin.exists(),"build the workspace test plugin before this regression");
        let uid=t.store.mutate(|p|Ok(p.add_clap_fx("t1",plugin.to_str().unwrap(),"test.gain").unwrap())).unwrap();
        let saved=std::fs::read(&path).unwrap();
        let data: serde_json::Value=serde_json::from_str(&t.get_clap_params(uid).unwrap()).unwrap();
        let gain=&data["params"][0];
        assert_eq!(gain["name"],"Gain");assert_eq!(gain["default"],1.0);
        assert_eq!(gain["readonly"],false);assert_eq!(gain["stepped"],false);
        assert!(gain["id"].is_u64());assert_eq!(std::fs::read(&path).unwrap(),saved);
        assert!(t.get_clap_params("missing".into()).is_err());
    }
}

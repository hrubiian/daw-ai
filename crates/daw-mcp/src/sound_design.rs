//! Sound design on Surge XT through MCP: find and load factory patches,
//! read and shape a sound through the curated control map
//! (`synth_map`), and step through its versions. Every edit goes through
//! the project store, so in live mode the open GUI hears it at once.

use crate::synth_map::{resolve, surge_controls};
use crate::tools::DawTools;
use base64::Engine as _;
use daw_core::audio::clap::{fxp_chunk, slot_specs, ClapInstance};
use daw_core::audition::{audition_project, default_phrase, phrase_notes, validate_notes, AuditionNote, PhraseKind};
use daw_core::project::{ClapSlot, Instrument, Pattern, PianoRollPattern, Project, TrackType};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

pub const SURGE_ID: &str = "org.surge-synth-team.surge-xt";
/// Versions kept per designed sound.
const HISTORY_LIMIT: usize = 50;

/// One saved version of a designed sound.
#[derive(Clone, Debug)]
pub struct SoundVersion {
    pub label: String,
    pub state: Option<String>,
    pub params: BTreeMap<String, f64>,
}

pub type SoundHistory = std::sync::Mutex<HashMap<String, Vec<SoundVersion>>>;

/// Where Surge XT patches are read from: the deb's factory and
/// third-party folders, plus `$SURGE_PATCH_DIRS` (colon-separated).
pub fn patch_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from("/usr/share/surge-xt/patches_factory"),
        PathBuf::from("/usr/share/surge-xt/patches_3rdparty"),
    ];
    if let Some(extra) = std::env::var_os("SURGE_PATCH_DIRS") {
        roots.extend(std::env::split_paths(&extra));
    }
    roots
}

fn fxp_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            // Recurse into real folders only: a folder symlink could point
            // back up the tree and loop forever.
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(p);
            } else if p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("fxp")) {
                out.push(p);
            }
        }
    }
    out
}

fn root_author(root: &Path) -> String {
    match root.file_name().map(|n| n.to_string_lossy().into_owned()) {
        Some(n) if n == "patches_factory" => "factory".into(),
        Some(n) => n,
        None => String::new(),
    }
}

/// Patches under `roots` whose name or category contains `query`
/// (case-insensitive), optionally in one `category`, sorted, at most
/// `limit` (default 20). JSON list of `{name, category, author, path}`.
pub fn find_patches_in(roots: &[PathBuf], query: &str, category: Option<String>, limit: Option<usize>) -> Result<String, String> {
    let q = query.trim().to_lowercase();
    let want = category.map(|c| c.to_lowercase());
    let mut hits = Vec::new();
    for root in roots {
        for path in fxp_files(root) {
            let rel: Vec<String> = path.strip_prefix(root).map(|r| r.iter().map(|c| c.to_string_lossy().into_owned()).collect()).unwrap_or_default();
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let (author, cat) = match rel.len() {
                n if n >= 3 => (rel[0].clone(), rel[n - 2].clone()),
                2 => (root_author(root), rel[0].clone()),
                _ => (root_author(root), String::new()),
            };
            if want.as_ref().is_some_and(|w| &cat.to_lowercase() != w) {
                continue;
            }
            if !q.is_empty() && !name.to_lowercase().contains(&q) && !cat.to_lowercase().contains(&q) {
                continue;
            }
            hits.push((cat, name, author, path.to_string_lossy().into_owned()));
        }
    }
    hits.sort();
    hits.truncate(limit.unwrap_or(20).min(200));
    let list: Vec<_> = hits.into_iter()
        .map(|(category, name, author, path)| serde_json::json!({"name": name, "category": category, "author": author, "path": path}))
        .collect();
    serde_json::to_string_pretty(&list).map_err(|e| e.to_string())
}

fn instrument_slot_mut<'a>(p: &'a mut Project, uid: &str) -> Result<&'a mut ClapSlot, String> {
    p.tracks.iter_mut().find_map(|t| match &mut t.instrument {
        Instrument::Clap(s) if s.uid == uid => Some(s),
        _ => None,
    }).ok_or_else(|| format!("no instrument slot '{uid}'"))
}

fn instrument_name(i: &Instrument) -> String {
    match i {
        Instrument::Soundfont { preset, .. } => format!("a soundfont instrument (preset {preset})"),
        Instrument::Sampler { .. } => "a sampler".into(),
        Instrument::AudioFile { .. } => "audio".into(),
        Instrument::Clap(c) => format!("the plugin {}", c.id),
        Instrument::Sfz { path } => format!("the SFZ instrument {path}"),
    }
}

fn pattern_is_empty(p: &Project, pid: &str) -> bool {
    match p.patterns.iter().find(|x| x.id() == pid) {
        Some(Pattern::PianoRoll(pr)) => pr.notes.is_empty(),
        Some(Pattern::Step(sp)) => sp.rows.iter().all(|r| r.steps.iter().all(|v| *v == 0)),
        None => true,
    }
}

/// Make a designed sound visible and playable: its phrase lives in its own
/// pattern (`<uid>_phrase`, created or updated) on its track, with a clip at
/// `bar`. Never overwrites another clip already covering that bar, and
/// never turns a loop-mode project (no arrangement) into arrangement mode,
/// which would silence every other track. Returns what happened.
fn place_phrase(p: &mut Project, tid: &str, uid: &str, notes: &[AuditionNote], bar: u32) -> Result<String, String> {
    let pid = format!("{uid}_phrase");
    let (pnotes, bars) = phrase_notes(notes, p.project.bpm);
    let track_name = p.tracks.iter().find(|t| t.id == tid).map(|t| t.name.clone()).ok_or_else(|| format!("no track '{tid}'"))?;
    match p.patterns.iter_mut().find(|x| x.id() == pid) {
        Some(Pattern::PianoRoll(pr)) => {
            pr.notes = pnotes;
            pr.bars = bars;
        }
        Some(_) => return Err(format!("pattern '{pid}' exists but isn't a piano roll")),
        None => p.patterns.push(Pattern::PianoRoll(PianoRollPattern {
            id: pid.clone(),
            name: format!("{track_name} phrase"),
            bars,
            ticks_per_bar: p.ticks_per_bar(),
            notes: pnotes,
            legato: false,
            cc_events: Vec::new(),
            pitch_bends: Vec::new(),
            legato_overlap_ticks: 0,
            legato_max_gap_ticks: None,
            swing: None,
        })),
    }
    let others: Vec<String> = p.tracks.iter().find(|t| t.id == tid).expect("track").patterns.iter().filter(|x| **x != pid).cloned().collect();
    let others_empty = others.iter().all(|x| pattern_is_empty(p, x));
    let track = p.tracks.iter_mut().find(|t| t.id == tid).expect("track");
    if !track.patterns.contains(&pid) {
        track.patterns.push(pid.clone());
    }
    if p.arrangement.is_empty() {
        // Loop mode: each track loops its first pattern.
        if others_empty {
            track.patterns.retain(|x| *x != pid);
            track.patterns.insert(0, pid);
            return Ok("its phrase loops on the track (loop-mode project)".into());
        }
        return Ok("phrase saved as a pattern on the track (loop-mode project; its first pattern keeps playing)".into());
    }
    if p.arrangement.iter().any(|s| s.track == tid && s.pattern == pid) {
        return Ok("updated its clip".into());
    }
    let covered = p.arrangement.iter().filter(|s| s.track == tid).any(|s| {
        let span = s.len_bars.unwrap_or_else(|| p.patterns.iter().find(|x| x.id() == s.pattern).map(|x| x.bars()).unwrap_or(1)).max(1);
        s.bar <= bar && bar < s.bar + span
    });
    if covered {
        return Ok(format!("phrase not placed: the track already has a clip at bar {}", bar + 1));
    }
    if !p.place_pattern_id(tid, bar, &pid) {
        return Err(format!("couldn't place the phrase on '{tid}'"));
    }
    Ok(format!("placed its clip at bar {}", bar + 1))
}

/// Write interleaved stereo f32 samples as a 32-bit float WAV.
fn write_stereo_wav(path: &Path, samples: &[f32], sr: u32) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let spec = hound::WavSpec { channels: 2, sample_rate: sr, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for s in samples {
        w.write_sample(*s).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

/// A file-name-safe version of a track name.
fn file_safe(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if s.is_empty() { "sound".into() } else { s }
}

fn shutdown(mut inst: ClapInstance) {
    if let Some(p) = inst.processor.take() {
        inst.handle.deactivate(p);
    }
}

impl DawTools {
    fn push_version(&self, uid: &str, label: String, slot: &ClapSlot) {
        let mut h = self.history.lock().unwrap_or_else(|e| e.into_inner());
        let v = h.entry(uid.to_string()).or_default();
        v.push(SoundVersion { label, state: slot.state.clone(), params: slot.params.clone() });
        if v.len() > HISTORY_LIMIT {
            v.remove(0);
        }
    }

    /// The Surge XT slot `uid` (saved values) and a private instance of it
    /// with its state and overrides applied, for reading and parsing values.
    fn surge_instance(&self, uid: &str) -> Result<(ClapInstance, ClapSlot), String> {
        let p = self.runtime_project(&self.store.load()?)?;
        let slot = p.tracks.iter().find_map(|t| match &t.instrument {
            Instrument::Clap(s) if s.uid == uid => Some(s.clone()),
            _ => None,
        }).ok_or_else(|| format!("no instrument slot '{uid}'"))?;
        if slot.id != SURGE_ID {
            return Err(format!("slot '{uid}' is {}, not Surge XT", slot.id));
        }
        let spec = slot_specs(&p).into_iter().find(|s| s.uid == uid).ok_or_else(|| format!("no plugin spec for '{uid}'"))?;
        Ok((ClapInstance::from_spec(&spec, 44_100)?, slot))
    }

    pub fn find_patches(&self, query: String, category: Option<String>, limit: Option<usize>) -> Result<String, String> {
        let roots = patch_roots();
        let out = find_patches_in(&roots, &query, category, limit)?;
        if out.trim() == "[]" && !roots.iter().any(|r| r.exists()) {
            return Ok(format!("{out}\nNo Surge XT patch folders found — install Surge XT (e.g. the surge-xt deb) or set $SURGE_PATCH_DIRS."));
        }
        Ok(out)
    }

    pub fn load_patch(&self, patch_path: String, track_id: Option<String>, name: Option<String>, replace: Option<bool>) -> Result<String, String> {
        self.load_patch_in(&patch_roots(), patch_path, track_id, name, replace)
    }

    /// `load_patch` with explicit patch folders (tests use temp folders).
    pub fn load_patch_in(&self, roots: &[PathBuf], patch_path: String, track_id: Option<String>, name: Option<String>, replace: Option<bool>) -> Result<String, String> {
        let real = std::fs::canonicalize(&patch_path).map_err(|_| format!("no patch file '{patch_path}'"))?;
        let allowed = roots.iter().filter_map(|r| std::fs::canonicalize(r).ok()).any(|r| real.starts_with(&r));
        if !allowed || !real.extension().is_some_and(|x| x.eq_ignore_ascii_case("fxp")) {
            return Err(format!(
                "'{patch_path}' is not a Surge patch under the patch folders ({})",
                roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ")
            ));
        }
        let bytes = std::fs::read(&real).map_err(|e| format!("read {}: {e}", real.display()))?;
        let state = base64::engine::general_purpose::STANDARD.encode(fxp_chunk(&bytes)?);
        let (plugin_path, plugin_id) = self.resolve_clap(SURGE_ID).map_err(|e| format!("Surge XT isn't available: {e}"))?;
        let stem = real.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let bar = self.playhead_bar();
        let mut placed = String::new();
        // Results come back through `ids`, never the store's reply text
        // (in live mode that text carries a revision note).
        let mut ids: Option<(String, String)> = None;
        self.store.mutate(|p| {
            let tid = match &track_id {
                None => {
                    let tid = p.add_track(TrackType::Melodic);
                    p.set_track_name(&tid, name.as_deref().unwrap_or(&stem));
                    tid
                }
                Some(t) => p.tracks.iter().find(|x| &x.id == t).map(|x| x.id.clone()).ok_or_else(|| format!("no track '{t}'"))?,
            };
            let track = p.tracks.iter().find(|x| x.id == tid).expect("track exists");
            let uid = match &track.instrument {
                Instrument::Clap(s) if s.id == plugin_id => s.uid.clone(),
                other => {
                    if track_id.is_some() && replace != Some(true) {
                        return Err(format!(
                            "track '{tid}' plays {}; pass replace: true to replace it with Surge XT, or omit track_id for a new track",
                            instrument_name(other)
                        ));
                    }
                    p.set_track_clap_instrument(&tid, &plugin_path, &plugin_id).ok_or_else(|| format!("no track '{tid}'"))?
                }
            };
            let slot = instrument_slot_mut(p, &uid)?;
            slot.state = Some(state.clone());
            slot.params.clear();
            let notes = default_phrase(PhraseKind::Note, &p.project.key, &p.project.scale, 4, p.project.bpm);
            placed = place_phrase(p, &tid, &uid, &notes, bar)?;
            ids = Some((uid, tid));
            Ok(String::new())
        })?;
        let (uid, tid) = ids.as_ref().map(|(u, t)| (u.as_str(), t.as_str())).ok_or("patch load produced no slot")?;
        let slot = ClapSlot { uid: uid.into(), path: plugin_path, id: plugin_id, state: Some(state), params: Default::default() };
        self.push_version(uid, format!("load {stem}"), &slot);
        self.with_summary(format!("loaded patch '{stem}' on track {tid} as Surge XT slot {uid}; {placed}"))
    }

    pub fn describe_sound(&self, uid: String) -> Result<String, String> {
        let (mut inst, slot) = self.surge_instance(&uid)?;
        let (found, missing) = resolve(surge_controls(), &inst.handle.param_infos);
        let controls: Vec<_> = found.iter().map(|(c, id)| {
            let value = slot.params.get(&id.to_string()).copied().or_else(|| inst.handle.param_value(*id));
            let text = value.and_then(|v| inst.handle.value_to_text(*id, v));
            serde_json::json!({"name": c.name, "group": c.group, "help": c.help, "value": value, "text": text})
        }).collect();
        shutdown(inst);
        serde_json::to_string_pretty(&serde_json::json!({"uid": uid, "controls": controls, "missing": missing})).map_err(|e| e.to_string())
    }

    pub fn shape_sound(&self, uid: String, controls: serde_json::Map<String, serde_json::Value>) -> Result<String, String> {
        if controls.is_empty() {
            return Err("give at least one control, e.g. {\"filter_cutoff\": \"800 Hz\"}".into());
        }
        let (mut inst, _) = self.surge_instance(&uid)?;
        let parsed = (|| {
            let (found, _) = resolve(surge_controls(), &inst.handle.param_infos);
            let mut updates = Vec::new();
            for (name, v) in &controls {
                let Some((_, id)) = found.iter().find(|(c, _)| &c.name == name) else {
                    return Err(format!(
                        "unknown control '{name}' — valid: {}",
                        found.iter().map(|(c, _)| c.name.as_str()).collect::<Vec<_>>().join(", ")
                    ));
                };
                let info = inst.handle.param_infos.iter().find(|i| i.id == *id).cloned().expect("resolved id");
                let value = match v {
                    serde_json::Value::Number(n) => {
                        let x = n.as_f64().unwrap_or(f64::NAN);
                        if !(info.min..=info.max).contains(&x) {
                            return Err(format!("{name}: {x} is outside {}..={} (or give text like \"800 Hz\")", info.min, info.max));
                        }
                        x
                    }
                    serde_json::Value::String(text) => {
                        let unreadable = || format!("{name}: can't read '{text}' — try a number {}..={} or text like the describe_sound readout", info.min, info.max);
                        let v = inst.handle.text_to_value(*id, text).ok_or_else(unreadable)?;
                        // Surge maps unknown words to some value (e.g. "loud" →
                        // the minimum), so wordy text must read back as itself
                        // ("Wavetable" for a type); numeric text is trusted.
                        if !text.chars().any(|c| c.is_ascii_digit()) {
                            let back = inst.handle.value_to_text(*id, v).unwrap_or_default();
                            if !back.trim().eq_ignore_ascii_case(text.trim()) {
                                return Err(unreadable());
                            }
                        }
                        v
                    }
                    other => return Err(format!("{name}: expected a number or text, got {other}")),
                };
                let text = inst.handle.value_to_text(*id, value).unwrap_or_else(|| format!("{value:.3}"));
                updates.push((name.clone(), *id, value, text));
            }
            Ok(updates)
        })();
        shutdown(inst);
        let updates = parsed?;
        let mut saved: Option<(Option<String>, BTreeMap<String, f64>)> = None;
        self.store.mutate(|p| {
            let s = instrument_slot_mut(p, &uid)?;
            for (_, id, value, _) in &updates {
                s.params.insert(id.to_string(), *value);
            }
            saved = Some((s.state.clone(), s.params.clone()));
            Ok(String::new())
        })?;
        let (state, params) = saved.ok_or("shape produced no slot")?;
        let names: Vec<_> = updates.iter().map(|u| u.0.as_str()).collect();
        self.push_version(&uid, format!("shape {}", names.join(", ")), &ClapSlot { uid: uid.clone(), path: String::new(), id: SURGE_ID.into(), state, params });
        Ok(updates.iter().map(|(n, _, _, t)| format!("{n} = {t}")).collect::<Vec<_>>().join("\n"))
    }

    /// The track holding instrument slot `uid`: (track id, track name).
    fn slot_track(p: &Project, uid: &str) -> Result<(String, String), String> {
        p.tracks.iter().find(|t| matches!(&t.instrument, Instrument::Clap(s) if s.uid == uid))
            .map(|t| (t.id.clone(), t.name.clone()))
            .ok_or_else(|| format!("no instrument slot '{uid}'"))
    }

    /// The phrase to audition: `notes` if given (validated), else the
    /// built-in `kind` (default Note) in the project's key at `octave`
    /// (default 4).
    fn phrase(p: &Project, kind: Option<PhraseKind>, notes: Option<Vec<AuditionNote>>, octave: Option<i8>) -> Result<Vec<AuditionNote>, String> {
        let notes = match notes {
            Some(n) => n,
            None => default_phrase(kind.unwrap_or(PhraseKind::Note), &p.project.key, &p.project.scale, octave.unwrap_or(4), p.project.bpm),
        };
        validate_notes(&notes)?;
        Ok(notes)
    }

    /// The playhead's bar in live mode (the open GUI); bar 0 offline.
    fn playhead_bar(&self) -> u32 {
        if self.store.live_socket.is_some() {
            let tpb = self.store.load().map(|p| p.ticks_per_bar()).unwrap_or(daw_core::sequencer::TICKS_PER_BAR);
            self.store.inspect_live().map(|r| r.playback_tick / tpb).unwrap_or(0)
        } else {
            0
        }
    }

    fn project_dir(&self) -> PathBuf {
        self.store.path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf()
    }

    pub fn audition_sound(&self, uid: String, kind: Option<PhraseKind>, notes: Option<Vec<AuditionNote>>, octave: Option<i8>) -> Result<String, String> {
        if let Some(n) = &notes {
            validate_notes(n)?;
        }
        let stored = self.store.load()?;
        let (tid, _) = Self::slot_track(&stored, &uid)?;
        let notes = Self::phrase(&stored, kind, notes, octave)?;
        let runtime = self.runtime_project(&stored)?;
        let solo = audition_project(&runtime, &tid, &notes)?;
        let (samples, sr) = daw_core::audio::render_project_samples(&solo, &self.sf2_path, false).map_err(|e| e.to_string())?;
        let wav = self.project_dir().join("renders").join(format!("audition-{uid}-{}.wav", daw_io::session::operation_id()));
        write_stereo_wav(&wav, &samples, sr)?;
        let mono: Vec<f32> = samples.chunks(2).map(|f| (f[0] + f.get(1).copied().unwrap_or(f[0])) * 0.5).collect();
        let traits = crate::sound_traits::sound_traits(&mono, sr, notes.first().map(|n| n.pitch));
        let bar = self.playhead_bar();
        let mut placed = String::new();
        self.store.mutate(|p| {
            placed = place_phrase(p, &tid, &uid, &notes, bar)?;
            Ok(String::new())
        })?;
        // Live mode: also play the phrase on the track in the open GUI.
        let live = if self.store.live_socket.is_some() {
            let played = self.store.inspect_live().and_then(|state| self.store.live_request(daw_core::session::Request {
                version: daw_core::session::PROTOCOL_VERSION,
                operation_id: daw_io::session::operation_id(),
                expected_revision: Some(state.revision),
                command: daw_core::session::Command::Audition { track_id: tid.clone(), notes: notes.clone() },
            }));
            match played {
                Ok(r) if r.error.is_none() => "\nplaying the phrase on the track in the GUI now".to_string(),
                Ok(r) => format!("\ncouldn't play it in the GUI: {}", r.error.map(|e| e.message).unwrap_or_default()),
                Err(e) => format!("\ncouldn't play it in the GUI: {e}"),
            }
        } else {
            String::new()
        };
        Ok(format!(
            "audition of {uid} on track {tid} ({} notes) — WAV: {}{live}\nclip: {placed}\n{}\nThese numbers only guide the next tweak; the musician should listen and judge the sound.",
            notes.len(),
            wav.display(),
            traits.to_text()
        ))
    }

    pub fn print_sound(&self, uid: String, notes: Option<Vec<AuditionNote>>, kind: Option<PhraseKind>, start_bar: Option<u32>, end_bar: Option<u32>, at_bar: Option<u32>) -> Result<String, String> {
        let stored = self.store.load()?;
        let (tid, name) = Self::slot_track(&stored, &uid)?;
        let runtime = self.runtime_project(&stored)?;
        let (samples, sr) = if start_bar.is_some() || end_bar.is_some() {
            // The track's own part over a bar range, everything else muted.
            let mut solo = runtime.clone();
            for t in &mut solo.tracks {
                t.muted = t.id != tid;
            }
            let s = start_bar.unwrap_or(0);
            let e = end_bar.unwrap_or_else(|| solo.song_bars());
            daw_core::audio::export::render_range_samples(&solo, &self.sf2_path, false, Some((s, e)), &Default::default()).map_err(|e| e.to_string())?
        } else {
            let notes = Self::phrase(&stored, kind, notes, None)?;
            daw_core::audio::render_project_samples(&audition_project(&runtime, &tid, &notes)?, &self.sf2_path, false).map_err(|e| e.to_string())?
        };
        let file = format!("{}-{}.wav", file_safe(&name), daw_io::session::operation_id());
        write_stereo_wav(&self.project_dir().join("prints").join(&file), &samples, sr)?;
        let len_sec = samples.len() as f64 / 2.0 / sr as f64;
        let bar = at_bar.or(start_bar).unwrap_or(0);
        let print_name = format!("{name} (print)");
        let clip_path = format!("prints/{file}");
        self.store.mutate(|p| {
            let start_sec = bar as f64 * 240.0 / p.project.bpm;
            let audio = p.add_track(TrackType::Audio);
            p.set_track_name(&audio, &print_name);
            p.add_audio_clip(&audio, &clip_path, start_sec, len_sec);
            Ok(String::new())
        })?;
        self.with_summary(format!("printed {uid} to {clip_path} ({len_sec:.2}s) on new audio track \"{print_name}\" at bar {bar}"))
    }

    pub fn sound_history(&self, uid: String) -> Result<String, String> {
        let h = self.history.lock().unwrap_or_else(|e| e.into_inner());
        let versions = h.get(&uid).ok_or_else(|| format!("no history for '{uid}' in this session"))?;
        Ok(versions.iter().enumerate().map(|(i, v)| format!("{i}: {}", v.label)).collect::<Vec<_>>().join("\n"))
    }

    pub fn revert_sound(&self, uid: String, version: usize) -> Result<String, String> {
        let v = {
            let h = self.history.lock().unwrap_or_else(|e| e.into_inner());
            let versions = h.get(&uid).ok_or_else(|| format!("no history for '{uid}' in this session"))?;
            versions.get(version).cloned().ok_or_else(|| format!("no version {version} (have 0–{})", versions.len() - 1))?
        };
        self.store.mutate(|p| {
            let s = instrument_slot_mut(p, &uid)?;
            s.state = v.state.clone();
            s.params = v.params.clone();
            Ok(String::new())
        })?;
        self.push_version(&uid, format!("revert to {version} ({})", v.label), &ClapSlot { uid: uid.clone(), path: String::new(), id: SURGE_ID.into(), state: v.state, params: v.params });
        self.with_summary(format!("{uid} reverted to version {version}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::DawTools;
    use daw_core::project::{Instrument, Project};
    use std::path::{Path, PathBuf};

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("daw_sound_design_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn tools(name: &str) -> (DawTools, PathBuf) {
        let path = temp(name).join("song.daw.json");
        let t = DawTools::new(path.clone());
        t.store.save(&Project::default_project()).unwrap();
        (t, path)
    }

    /// Minimal FXP "FPCh" file carrying `chunk`.
    fn fxp(chunk: &[u8]) -> Vec<u8> {
        let mut b = b"CcnK".to_vec();
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(b"FPCh");
        b.resize(56, 0);
        b.extend_from_slice(&(chunk.len() as u32).to_be_bytes());
        b.extend_from_slice(chunk);
        b
    }

    /// A real Surge XT factory bass patch, when Surge XT is installed.
    fn surge_patch() -> Option<String> {
        let dir = Path::new("/usr/share/surge-xt/patches_factory/Basses");
        if !Path::new("/usr/lib/clap/Surge XT.clap").exists() || !dir.exists() {
            eprintln!("skip: Surge XT not installed");
            return None;
        }
        let mut v: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "fxp")).collect();
        v.sort();
        v.first().map(|p| p.to_string_lossy().into_owned())
    }

    fn surge_slot(p: &Project, uid: &str) -> daw_core::project::ClapSlot {
        p.tracks.iter().find_map(|t| match &t.instrument {
            Instrument::Clap(s) if s.uid == uid => Some(s.clone()),
            _ => None,
        }).expect("slot")
    }

    #[test]
    fn find_patches_by_category_and_query() {
        let root = temp("find");
        for (dir, name) in [("Basses", "Deep Sub"), ("Pads", "Air")] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join(format!("{name}.fxp")), fxp(b"x")).unwrap();
        }
        let out: serde_json::Value = serde_json::from_str(&find_patches_in(std::slice::from_ref(&root), "sub", None, None).unwrap()).unwrap();
        let list = out.as_array().unwrap();
        assert_eq!(list.len(), 1, "{out}");
        assert_eq!((list[0]["name"].as_str(), list[0]["category"].as_str()), (Some("Deep Sub"), Some("Basses")));
        let pads: serde_json::Value = serde_json::from_str(&find_patches_in(&[root], "", Some("pads".into()), None).unwrap()).unwrap();
        assert_eq!(pads.as_array().unwrap().len(), 1, "category filter is case-insensitive");
        let none: serde_json::Value = serde_json::from_str(&find_patches_in(&[], "sub", None, None).unwrap()).unwrap();
        assert!(none.as_array().unwrap().is_empty());
    }

    #[test]
    fn find_patches_survives_symlink_loops() {
        // A folder link back up the tree must not make the patch walk loop
        // forever (it would hang the tool and hold a worker slot).
        let root = temp("loop");
        std::fs::create_dir_all(root.join("Pads")).unwrap();
        std::fs::write(root.join("Pads/Air.fxp"), fxp(b"x")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("Pads/up")).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let r = root.clone();
        std::thread::spawn(move || { let _ = tx.send(find_patches_in(&[r], "", None, None)); });
        let out = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("find_patches returned (no infinite loop)").unwrap();
        let list: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1, "each patch once: {list}");
    }

    #[test]
    fn live_mode_tools_report_cleanly() {
        // Against a running session (the GUI's live mode), the store's edit
        // reply carries a revision note; tools must not parse data out of it.
        let Some(patch) = surge_patch() else { return };
        let path = temp("live").join("song.daw.json");
        let (host, _commands, _audio) = daw_io::Host::headless();
        let live = daw_io::session::LiveSession::start(Project::default_project(), Some(&path), &host).unwrap();
        let mut t = DawTools::new(path);
        t.store = crate::store::ProjectStore::live(live.socket.clone()).unwrap();
        let out = t.load_patch(patch, None, Some("Live Tom".into()), None).unwrap();
        let p = t.store.load().unwrap();
        let track = p.tracks.iter().find(|x| x.name == "Live Tom").expect("live track");
        let Instrument::Clap(slot) = &track.instrument else { panic!("Surge slot") };
        assert!(out.contains(&format!("on track {} as Surge XT slot {}", track.id, slot.uid)), "{out}");
        let c: serde_json::Map<String, serde_json::Value> = serde_json::from_str(r#"{"filter_cutoff":"700 Hz"}"#).unwrap();
        let shaped = t.shape_sound(slot.uid.clone(), c).unwrap();
        assert!(shaped.starts_with("filter_cutoff = 7"), "{shaped}");
        assert_eq!(surge_slot(&t.store.load().unwrap(), &slot.uid).params.len(), 1);
        assert!(t.sound_history(slot.uid.clone()).unwrap().contains("shape"));
        // Don't leave a dead session socket in the user's runtime folder.
        std::fs::remove_file(&live.socket).ok();
    }

    #[test]
    fn load_patch_rejects_paths_outside_roots() {
        let root = temp("roots").join("patches");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.parent().unwrap().join("x.fxp"), fxp(b"x")).unwrap();
        let (t, path) = tools("outside");
        let before = std::fs::read(&path).unwrap();
        for bad in ["/etc/hostname".to_string(), root.join("../x.fxp").to_string_lossy().into_owned()] {
            let err = t.load_patch_in(std::slice::from_ref(&root), bad.clone(), None, None, None).unwrap_err();
            assert!(err.contains("patch"), "{bad}: {err}");
        }
        assert_eq!(std::fs::read(&path).unwrap(), before, "nothing saved");
    }

    #[test]
    fn load_patch_creates_own_track_and_sets_state() {
        let Some(patch) = surge_patch() else { return };
        let (t, _) = tools("load");
        let out = t.load_patch(patch, None, Some("Dusty Tom".into()), None).unwrap();
        let p = t.store.load().unwrap();
        let track = p.tracks.iter().find(|x| x.name == "Dusty Tom").expect("own track");
        let Instrument::Clap(slot) = &track.instrument else { panic!("Surge slot") };
        assert_eq!(slot.id, SURGE_ID);
        assert!(slot.state.is_some() && slot.params.is_empty());
        assert!(out.contains(&slot.uid) && out.contains(&track.id), "{out}");
        p.validate().unwrap();
    }

    #[test]
    fn load_patch_refuses_to_replace_other_instruments() {
        let Some(patch) = surge_patch() else { return };
        let (t, path) = tools("replace");
        let before = std::fs::read(&path).unwrap();
        let err = t.load_patch(patch, Some("t1".into()), None, None).unwrap_err();
        assert!(err.contains("replace"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), before, "nothing saved");
    }

    #[test]
    fn shape_sound_validates_before_saving() {
        let Some(patch) = surge_patch() else { return };
        let (t, path) = tools("shape");
        t.load_patch(patch, None, Some("Designed Sub".into()), None).unwrap();
        let uid = t.store.load().unwrap().tracks.iter().find(|x| x.name == "Designed Sub").and_then(|x| match &x.instrument {
            Instrument::Clap(s) => Some(s.uid.clone()), _ => None }).unwrap();
        let before = std::fs::read(&path).unwrap();
        let bad: serde_json::Map<String, serde_json::Value> = serde_json::from_str(r#"{"filter_cutoff":"800 Hz","cutof":1}"#).unwrap();
        let err = t.shape_sound(uid.clone(), bad).unwrap_err();
        assert!(err.contains("cutof"), "{err}");
        let loud: serde_json::Map<String, serde_json::Value> = serde_json::from_str(r#"{"filter_cutoff":"loud"}"#).unwrap();
        assert!(t.shape_sound(uid.clone(), loud).unwrap_err().contains("loud"));
        assert_eq!(std::fs::read(&path).unwrap(), before, "nothing saved");
        let good: serde_json::Map<String, serde_json::Value> = serde_json::from_str(r#"{"filter_cutoff":"800 Hz","amp_attack":0.2}"#).unwrap();
        t.shape_sound(uid.clone(), good).unwrap();
        assert_eq!(surge_slot(&t.store.load().unwrap(), &uid).params.len(), 2);
        let desc: serde_json::Value = serde_json::from_str(&t.describe_sound(uid).unwrap()).unwrap();
        let cutoff = desc["controls"].as_array().unwrap().iter().find(|c| c["name"] == "filter_cutoff").unwrap();
        let hz: f64 = cutoff["text"].as_str().unwrap().split_whitespace().next().unwrap().parse().unwrap();
        assert!((hz - 800.0).abs() / 800.0 < 0.05, "{cutoff}");
        assert!(desc["missing"].as_array().unwrap().is_empty());
        // Word values that read back as themselves (a type name) are accepted.
        let osc = desc["controls"].as_array().unwrap().iter().find(|c| c["name"] == "osc1_type").unwrap();
        let type_name = osc["text"].as_str().unwrap().to_string();
        assert!(!type_name.chars().any(|c| c.is_ascii_digit()), "type readout is a word: {type_name}");
        let same: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&format!(r#"{{"osc1_type":"{type_name}"}}"#)).unwrap();
        t.shape_sound(desc["uid"].as_str().unwrap().into(), same).unwrap();
    }

    fn designed(t: &DawTools, patch: String, name: &str) -> String {
        t.load_patch(patch, None, Some(name.into()), None).unwrap();
        t.store.load().unwrap().tracks.iter().find(|x| x.name == name).and_then(|x| match &x.instrument {
            Instrument::Clap(s) => Some(s.uid.clone()), _ => None }).unwrap()
    }

    fn phrase_slots(p: &Project, tid: &str, uid: &str) -> Vec<daw_core::project::ArrangementSlot> {
        p.arrangement.iter().filter(|a| a.track == tid && a.pattern == format!("{uid}_phrase")).cloned().collect()
    }

    fn phrase_notes_of(p: &Project, uid: &str) -> Vec<daw_core::project::Note> {
        match p.patterns.iter().find(|x| x.id() == format!("{uid}_phrase")) {
            Some(daw_core::project::Pattern::PianoRoll(pr)) => pr.notes.clone(),
            _ => vec![],
        }
    }

    #[test]
    fn new_sounds_get_a_phrase_clip_at_the_playhead() {
        let Some(patch) = surge_patch() else { return };
        let (t, _) = tools("place");
        let uid = designed(&t, patch, "Clip Tom");
        let p = t.store.load().unwrap();
        let tid = p.tracks.iter().find(|x| x.name == "Clip Tom").unwrap().id.clone();
        let slots = phrase_slots(&p, &tid, &uid);
        assert_eq!(slots.len(), 1, "visible on the track row");
        assert_eq!(slots[0].bar, 0, "file mode: the playhead is bar 1");
        assert!(!phrase_notes_of(&p, &uid).is_empty(), "it has notes to play");
        // Auditioning again updates the same clip's notes.
        t.audition_sound(uid.clone(), Some(daw_core::audition::PhraseKind::Chord), None, Some(3)).unwrap();
        let p = t.store.load().unwrap();
        assert_eq!(phrase_slots(&p, &tid, &uid).len(), 1);
        assert_eq!(phrase_notes_of(&p, &uid).len(), 3, "chord replaces the phrase");
        p.validate().unwrap();
    }

    #[test]
    fn placement_never_overwrites_a_clip() {
        let Some(patch) = surge_patch() else { return };
        let (t, _) = tools("noclobber");
        let uid = designed(&t, patch, "Busy Tom");
        let tid = t.store.load().unwrap().tracks.iter().find(|x| x.name == "Busy Tom").unwrap().id.clone();
        // The musician puts another clip of this track at bar 1 in its place.
        t.store.mutate(|p| {
            p.arrangement.retain(|a| a.track != tid);
            let other = p.tracks.iter().find(|x| x.id == tid).unwrap().patterns[0].clone();
            assert!(p.place_pattern_id(&tid, 0, &other));
            Ok(String::new())
        }).unwrap();
        let out = t.audition_sound(uid.clone(), None, None, Some(3)).unwrap();
        assert!(out.contains("not placed"), "{out}");
        let p = t.store.load().unwrap();
        let on_track: Vec<_> = p.arrangement.iter().filter(|a| a.track == tid).collect();
        assert_eq!(on_track.len(), 1, "the musician's clip stays");
        assert_ne!(on_track[0].pattern, format!("{uid}_phrase"));
    }

    #[test]
    fn loop_mode_projects_stay_in_loop_mode() {
        let Some(patch) = surge_patch() else { return };
        let (t, _) = tools("loopmode");
        t.store.mutate(|p| { p.arrangement.clear(); Ok(String::new()) }).unwrap();
        let uid = designed(&t, patch, "Loop Tom");
        let p = t.store.load().unwrap();
        assert!(p.arrangement.is_empty(), "adding a clip would silence every other track");
        let track = p.tracks.iter().find(|x| x.name == "Loop Tom").unwrap();
        assert_eq!(track.patterns[0], format!("{uid}_phrase"), "the new track loops its phrase");
    }

    #[test]
    fn audition_rejects_bad_notes() {
        let (t, _) = tools("badnotes");
        let note = daw_core::audition::AuditionNote { pitch: 200, velocity: 100, start_ms: 0, dur_ms: 500 };
        let err = t.audition_sound("c1".into(), None, Some(vec![note]), None).unwrap_err();
        assert!(err.contains("pitch"), "{err}");
    }

    #[test]
    fn audition_reports_traits_and_writes_wav() {
        let Some(patch) = surge_patch() else { return };
        let (t, path) = tools("audition");
        let uid = designed(&t, patch, "Audition Bass");
        let out = t.audition_sound(uid, Some(daw_core::audition::PhraseKind::Note), None, Some(2)).unwrap();
        assert!(out.contains("pitch") && out.contains("attack"), "{out}");
        assert!(out.contains("listen"), "the musician judges: {out}");
        let renders = path.parent().unwrap().join("renders");
        let wavs: Vec<_> = std::fs::read_dir(&renders).unwrap().flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("audition-")).collect();
        assert_eq!(wavs.len(), 1);
    }

    #[test]
    fn designed_sound_survives_reload() {
        let Some(patch) = surge_patch() else { return };
        let (t, path) = tools("reload");
        let uid = designed(&t, patch, "Reload Bass");
        let c: serde_json::Map<String, serde_json::Value> = serde_json::from_str(r#"{"filter_cutoff":"600 Hz"}"#).unwrap();
        t.shape_sound(uid.clone(), c).unwrap();
        let render = |t: &DawTools| {
            let p = t.runtime_project(&t.store.load().unwrap()).unwrap();
            let tid = p.tracks.iter().find(|x| x.name == "Reload Bass").unwrap().id.clone();
            let notes = daw_core::audition::default_phrase(daw_core::audition::PhraseKind::Note, &p.project.key, &p.project.scale, 2, p.project.bpm);
            let a = daw_core::audition::audition_project(&p, &tid, &notes).unwrap();
            daw_core::audio::render_project_samples(&a, &t.sf2_path, false).unwrap().0
        };
        let first = render(&t);
        let reloaded = DawTools::new(path);
        let second = render(&reloaded);
        assert_eq!(first.len(), second.len());
        let max = first.iter().zip(&second).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(max < 1e-6, "renders differ after reload by {max}");
        assert!(first.iter().any(|s| s.abs() > 1e-4), "the sound plays");
    }

    #[test]
    fn print_places_clip_on_new_audio_track() {
        let Some(patch) = surge_patch() else { return };
        let (t, path) = tools("print");
        let uid = designed(&t, patch, "Print Bass");
        t.print_sound(uid, None, Some(daw_core::audition::PhraseKind::Line), None, None, Some(2)).unwrap();
        let p = t.store.load().unwrap();
        let track = p.tracks.iter().find(|x| x.name == "Print Bass (print)").expect("print track");
        assert_eq!(track.track_type, daw_core::project::TrackType::Audio);
        let clip = p.audio_clips.iter().find(|c| c.track == track.id).expect("clip");
        assert!((clip.start_sec - 2.0 * 240.0 / p.project.bpm).abs() < 1e-9, "bar 2: {}", clip.start_sec);
        let file = path.parent().unwrap().join(&clip.path);
        assert!(file.exists(), "{}", file.display());
        assert!(daw_core::audio::wav_duration_sec(&file).unwrap() > 1.0);
        p.validate().unwrap();
    }

    #[test]
    fn revert_restores_earlier_version() {
        let Some(patch) = surge_patch() else { return };
        let (t, _) = tools("revert");
        t.load_patch(patch, None, Some("Pad".into()), None).unwrap();
        let uid = t.store.load().unwrap().tracks.iter().find(|x| x.name == "Pad").and_then(|x| match &x.instrument {
            Instrument::Clap(s) => Some(s.uid.clone()), _ => None }).unwrap();
        let loaded = surge_slot(&t.store.load().unwrap(), &uid);
        let c: serde_json::Map<String, serde_json::Value> = serde_json::from_str(r#"{"amp_release":0.9}"#).unwrap();
        t.shape_sound(uid.clone(), c).unwrap();
        assert!(t.sound_history(uid.clone()).unwrap().contains("shape"));
        t.revert_sound(uid.clone(), 0).unwrap();
        let back = surge_slot(&t.store.load().unwrap(), &uid);
        assert!(back.params.is_empty());
        assert_eq!(back.state, loaded.state);
        assert!(t.revert_sound(uid, 99).is_err());
    }
}

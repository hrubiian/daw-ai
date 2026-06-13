//! CLAP plugin hosting (effects + instruments) built on clack-host.
//!
//! Lifecycle: `ClapInstance::load` (any non-RT thread) → split into a
//! main-thread `ClapHandle` (params/state/deactivation; must stay on the
//! creating thread) and a `Send` `ClapProcessor` that the mixer drives on
//! the RT thread. Processors are returned to their creating thread for
//! teardown — never dropped on the RT thread.
//! See `docs/superpowers/specs/2026-06-11-clap-hosting-design.md`.

use clack_extensions::params::{ParamInfoBuffer, PluginParams};
use clack_extensions::state::PluginState;
use clack_host::events::event_types::{NoteOffEvent, NoteOnEvent, ParamValueEvent};
use clack_host::events::Match;
use clack_host::utils::Cookie;
use clack_host::prelude::*;
use std::ffi::CString;
use std::path::PathBuf;

/// Max frames per process call — matches the mixer's block cap.
const CLAP_MAX_FRAMES: u32 = 8192;

// ── Host handler boilerplate ──────────────────────────────────────────────────

pub struct DawClapShared;
impl<'a> SharedHandler<'a> for DawClapShared {
    fn request_restart(&self) {
        eprintln!("clap: plugin requested restart (ignored this slice)");
    }
    fn request_process(&self) {}
    fn request_callback(&self) {}
}

#[derive(Default)]
pub struct DawClapMainThread<'a> {
    plugin: Option<InitializedPluginHandle<'a>>,
    params: Option<PluginParams>,
    state: Option<PluginState>,
}
impl<'a> MainThreadHandler<'a> for DawClapMainThread<'a> {
    fn initialized(&mut self, instance: InitializedPluginHandle<'a>) {
        self.params = instance.get_extension();
        self.state = instance.get_extension();
        self.plugin = Some(instance);
    }
}

pub struct DawClapHost;
impl HostHandlers for DawClapHost {
    type Shared<'a> = DawClapShared;
    type MainThread<'a> = DawClapMainThread<'a>;
    type AudioProcessor<'a> = ();
}

fn host_info() -> HostInfo {
    HostInfo::new("daw-ai", "daw-ai", "https://github.com/daw-ai/daw-ai", "0.1.0")
        .expect("static host info")
}

// ── Discovery ─────────────────────────────────────────────────────────────────

/// One plugin found during discovery.
#[derive(Debug, Clone)]
pub struct ClapPluginDesc {
    pub path: String,
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub is_instrument: bool,
    pub is_effect: bool,
}

/// Standard CLAP search paths (Linux) + `$CLAP_PATH`.
pub fn standard_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(p) = std::env::var("CLAP_PATH") {
        v.extend(std::env::split_paths(&p));
    }
    if let Some(home) = std::env::var_os("HOME") {
        v.push(PathBuf::from(home).join(".clap"));
    }
    v.push(PathBuf::from("/usr/lib/clap"));
    v.push(PathBuf::from("/usr/local/lib/clap"));
    v
}

pub fn discover() -> Vec<ClapPluginDesc> {
    discover_in(&standard_paths())
}

/// Scan directories recursively for `.clap`/`.so` bundles and enumerate
/// their descriptors. Unloadable files are skipped silently.
pub fn discover_in(paths: &[PathBuf]) -> Vec<ClapPluginDesc> {
    let mut out = Vec::new();
    for root in paths {
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "clap" || x == "so") {
                    scan_bundle(&p, &mut out);
                }
            }
        }
    }
    // The same plugin can appear at several paths (e.g. a build dir and its
    // deps/ copies) — keep the first occurrence of each plugin id.
    out.sort_by(|a, b| a.name.cmp(&b.name));
    let mut seen = std::collections::HashSet::new();
    out.retain(|d| seen.insert(d.id.clone()));
    out
}

fn scan_bundle(path: &std::path::Path, out: &mut Vec<ClapPluginDesc>) {
    // Loading an arbitrary shared library runs its constructors — inherent
    // to plugin hosting; the search paths are user-controlled.
    // SAFETY: loading a plugin dylib runs its constructors — inherent to
    // CLAP hosting; paths come from the user's plugin directories.
    let Ok(entry) = (unsafe { PluginEntry::load(path) }) else { return };
    let Some(factory) = entry.get_plugin_factory() else { return };
    for d in factory.plugin_descriptors() {
        let (Some(id), Some(name)) = (d.id(), d.name()) else { continue };
        let feats: Vec<String> = d
            .features()
            .map(|f| f.to_string_lossy().to_string())
            .collect();
        out.push(ClapPluginDesc {
            path: path.to_string_lossy().to_string(),
            id: id.to_string_lossy().to_string(),
            name: name.to_string_lossy().to_string(),
            vendor: d
                .vendor()
                .map(|v| v.to_string_lossy().to_string())
                .unwrap_or_default(),
            is_instrument: feats.iter().any(|f| f == "instrument"),
            is_effect: feats.iter().any(|f| f == "audio-effect" || f == "audio_effect"),
        });
    }
}

// ── Param metadata ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ClapParamInfo {
    pub id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
}

// ── Spec for building instances (sequencer → relay / exporter) ───────────────

/// Everything needed to (re)build one plugin instance from the project.
#[derive(Debug, Clone)]
pub struct ClapSlotSpec {
    pub uid: String,
    pub path: String,
    pub id: String,
    /// Raw state blob (already base64-decoded).
    pub state: Option<Vec<u8>>,
    pub params: Vec<(u32, f64)>,
}

/// All CLAP slots in a project, decoded and ready to instantiate.
pub fn slot_specs(project: &crate::project::Project) -> Vec<ClapSlotSpec> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut out = Vec::new();
    let mut push = |s: &crate::project::ClapSlot| {
        out.push(ClapSlotSpec {
            uid: s.uid.clone(),
            path: s.path.clone(),
            id: s.id.clone(),
            state: s.state.as_deref().and_then(|b| b64.decode(b).ok()),
            params: s
                .params
                .iter()
                .filter_map(|(k, v)| k.parse::<u32>().ok().map(|id| (id, *v)))
                .collect(),
        });
    };
    for t in &project.tracks {
        if let crate::project::Instrument::Clap(s) = &t.instrument {
            push(s);
        }
        for s in &t.clap_fx {
            push(s);
        }
    }
    out
}

// ── Live instance ─────────────────────────────────────────────────────────────

/// Main-thread half: keep on the creating thread.
pub struct ClapHandle {
    instance: PluginInstance<DawClapHost>,
    pub name: String,
    pub param_infos: Vec<ClapParamInfo>,
}

/// RT half: drives audio. `Send`; owned by the Mixer while live.
pub struct ClapProcessor {
    started: clack_host::process::StartedPluginAudioProcessor<DawClapHost>,
    events_in: EventBuffer,
    events_out: EventBuffer,
    ports_in: AudioPorts,
    ports_out: AudioPorts,
    buf_in: [Vec<f32>; 2],
    buf_out: [Vec<f32>; 2],
    /// Held note keys, so panic / teardown can release them.
    held: Vec<u8>,
    steady: u64,
}

impl std::fmt::Debug for ClapProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ClapProcessor")
    }
}

/// A loaded, activated plugin (both halves together, pre-split).
pub struct ClapInstance {
    pub handle: ClapHandle,
    pub processor: Option<ClapProcessor>,
}

impl ClapInstance {
    /// Load a bundle, instantiate `plugin_id`, activate at `sample_rate`,
    /// start processing, and read param metadata. Errors are stringly —
    /// callers report them on the status line.
    pub fn load(
        path: &str,
        plugin_id: &str,
        sample_rate: u32,
        max_frames: u32,
    ) -> Result<Self, String> {
        // SAFETY: see scan_bundle — dylib loading is inherent to hosting.
        let entry =
            unsafe { PluginEntry::load(path) }.map_err(|e| format!("load {path}: {e}"))?;
        // Display name from the bundle's descriptor (fall back to the id).
        let name = entry
            .get_plugin_factory()
            .and_then(|f| {
                f.plugin_descriptors()
                    .find(|d| d.id().is_some_and(|i| i.to_string_lossy() == plugin_id))
                    .and_then(|d| d.name().map(|n| n.to_string_lossy().to_string()))
            })
            .unwrap_or_else(|| plugin_id.to_string());
        let id_c = CString::new(plugin_id).map_err(|_| "bad plugin id".to_string())?;
        let mut instance = PluginInstance::<DawClapHost>::new(
            |_| DawClapShared,
            |_| DawClapMainThread::default(),
            &entry,
            &id_c,
            &host_info(),
        )
        .map_err(|e| format!("instantiate {plugin_id}: {e}"))?;

        let config = PluginAudioConfiguration {
            sample_rate: sample_rate as f64,
            min_frames_count: 1,
            max_frames_count: max_frames.min(CLAP_MAX_FRAMES),
        };
        let stopped = instance
            .activate(|_, _| (), config)
            .map_err(|e| format!("activate {plugin_id}: {e}"))?;
        let started = stopped
            .start_processing()
            .map_err(|e| format!("start {plugin_id}: {e}"))?;

        // Param metadata (main thread).
        let mut param_infos = Vec::new();
        let params_ext = instance.access_handler(|mt: &DawClapMainThread| mt.params);
        if let Some(params) = params_ext {
            let mut handle = instance.plugin_handle();
            let count = params.count(&mut handle);
            let mut buf = ParamInfoBuffer::new();
            for i in 0..count {
                if let Some(info) = params.get_info(&mut handle, i, &mut buf) {
                    param_infos.push(ClapParamInfo {
                        id: info.id.into(),
                        name: String::from_utf8_lossy(info.name).to_string(),
                        min: info.min_value,
                        max: info.max_value,
                        default: info.default_value,
                    });
                }
            }
        }

        let frames = max_frames.min(CLAP_MAX_FRAMES) as usize;
        Ok(ClapInstance {
            handle: ClapHandle { instance, name, param_infos },
            processor: Some(ClapProcessor {
                started,
                events_in: EventBuffer::new(),
                events_out: EventBuffer::new(),
                ports_in: AudioPorts::with_capacity(2, 1),
                ports_out: AudioPorts::with_capacity(2, 1),
                buf_in: [vec![0.0; frames], vec![0.0; frames]],
                buf_out: [vec![0.0; frames], vec![0.0; frames]],
                held: Vec::new(),
                steady: 0,
            }),
        })
    }

    /// Build from a slot spec: load, then apply state + param overrides.
    pub fn from_spec(spec: &ClapSlotSpec, sample_rate: u32) -> Result<Self, String> {
        let mut inst = Self::load(&spec.path, &spec.id, sample_rate, CLAP_MAX_FRAMES)?;
        if let Some(blob) = &spec.state {
            if let Err(e) = inst.handle.load_state(blob) {
                eprintln!("clap {}: state load failed: {e}", spec.uid);
            }
        }
        if let Some(p) = inst.processor.as_mut() {
            for (id, v) in &spec.params {
                p.queue_param(*id, *v);
            }
        }
        Ok(inst)
    }

    /// Stop, deactivate, and drop everything (call on the creating thread).
    pub fn shutdown(mut self) {
        if let Some(p) = self.processor.take() {
            self.handle.deactivate(p);
        }
    }
}

impl ClapHandle {
    pub fn collect_state(&mut self) -> Option<Vec<u8>> {
        let state = self.instance.access_handler(|mt: &DawClapMainThread| mt.state)?;
        let mut out = Vec::new();
        let mut handle = self.instance.plugin_handle();
        state.save(&mut handle, &mut out).ok()?;
        Some(out)
    }

    pub fn load_state(&mut self, blob: &[u8]) -> Result<(), String> {
        let Some(state) = self.instance.access_handler(|mt: &DawClapMainThread| mt.state) else {
            return Err("plugin has no state extension".into());
        };
        let mut handle = self.instance.plugin_handle();
        let mut cursor = std::io::Cursor::new(blob);
        state
            .load(&mut handle, &mut cursor)
            .map_err(|e| e.to_string())
    }

    /// Tear down a returned processor (stop → deactivate).
    pub fn deactivate(&mut self, processor: ClapProcessor) {
        let stopped = processor.started.stop_processing();
        self.instance.deactivate(stopped);
    }
}

impl ClapProcessor {
    pub fn queue_note_on(&mut self, key: u8, velocity: u8) {
        self.events_in.push(&NoteOnEvent::new(
            0,
            Pckn::new(0u16, 0u16, key as u16, Match::All),
            velocity as f64 / 127.0,
        ));
        self.held.push(key);
    }

    pub fn queue_note_off(&mut self, key: u8) {
        self.events_in.push(&NoteOffEvent::new(
            0,
            Pckn::new(0u16, 0u16, key as u16, Match::All),
            0.0,
        ));
        self.held.retain(|&k| k != key);
    }

    /// Release everything (transport panic / strip mute).
    pub fn queue_all_notes_off(&mut self) {
        for key in std::mem::take(&mut self.held) {
            self.events_in.push(&NoteOffEvent::new(
                0,
                Pckn::new(0u16, 0u16, key as u16, Match::All),
                0.0,
            ));
        }
    }

    pub fn queue_param(&mut self, param_id: u32, value: f64) {
        self.events_in.push(&ParamValueEvent::new(
            0,
            ClapId::new(param_id),
            Pckn::match_all(),
            value,
            Cookie::empty(),
        ));
    }

    /// Process one block in place (effect: filters `l`/`r`; instrument:
    /// overwrites them). `l.len() == r.len()` and both fit `max_frames`.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let frames = l.len().min(r.len()).min(self.buf_in[0].len());
        self.buf_in[0][..frames].copy_from_slice(&l[..frames]);
        self.buf_in[1][..frames].copy_from_slice(&r[..frames]);
        self.buf_out[0][..frames].fill(0.0);
        self.buf_out[1][..frames].fill(0.0);

        {
            let input_events = InputEvents::from_buffer(&self.events_in);
            let mut output_events = OutputEvents::from_buffer(&mut self.events_out);

            let input_audio = self.ports_in.with_input_buffers([AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_input_only(
                    self.buf_in
                        .iter_mut()
                        .map(|b| InputChannel::variable(&mut b[..frames])),
                ),
            }]);
            let mut output_audio = self.ports_out.with_output_buffers([AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_output_only(
                    self.buf_out.iter_mut().map(|b| &mut b[..frames]),
                ),
            }]);

            if let Err(e) = self.started.process(
                &input_audio,
                &mut output_audio,
                &input_events,
                &mut output_events,
                Some(self.steady),
                None,
            ) {
                eprintln!("clap process error: {e}");
            }
        }

        self.events_in.clear();
        self.events_out.clear();
        self.steady = self.steady.wrapping_add(frames as u64);
        l[..frames].copy_from_slice(&self.buf_out[0][..frames]);
        r[..frames].copy_from_slice(&self.buf_out[1][..frames]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn testplug(name: &str) -> Option<std::path::PathBuf> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug")
            .join(name);
        if p.exists() {
            Some(p)
        } else {
            eprintln!("skip: {name} not built (cargo build --workspace)");
            None
        }
    }

    #[test]
    fn slot_specs_collects_and_decodes() {
        use base64::Engine as _;
        let mut p = crate::project::Project::default_project();
        let t0 = p.tracks[0].id.clone();
        let uid = p.add_clap_fx(&t0, "/x.clap", "test.gain").unwrap();
        p.set_clap_param(&uid, 1, 0.5);
        p.set_clap_state(
            &uid,
            base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]),
        );
        let specs = slot_specs(&p);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].state.as_deref(), Some(&[1u8, 2, 3][..]));
        assert_eq!(specs[0].params, vec![(1, 0.5)]);
    }

    #[test]
    fn discovers_test_plugins() {
        let Some(p) = testplug("libdaw_clap_testplug_gain.so") else { return };
        let dir = p.parent().unwrap();
        let found = discover_in(&[dir.to_path_buf()]);
        let gain = found.iter().find(|d| d.id == "test.gain").expect("test.gain found");
        assert!(gain.is_effect && !gain.is_instrument);
        assert_eq!(gain.name, "DAW Test Gain");
        if testplug("libdaw_clap_testplug_tone.so").is_some() {
            let tone = found.iter().find(|d| d.id == "test.tone").expect("test.tone found");
            assert!(tone.is_instrument);
        }
    }

    #[test]
    fn gain_plugin_processes_and_takes_params() {
        let Some(p) = testplug("libdaw_clap_testplug_gain.so") else { return };
        let mut inst = ClapInstance::load(p.to_str().unwrap(), "test.gain", 48_000, 256)
            .expect("load test.gain");
        let gi = inst
            .handle
            .param_infos
            .iter()
            .find(|i| i.name == "Gain")
            .expect("Gain param listed")
            .clone();
        assert_eq!(gi.default, 1.0);

        let proc_ = inst.processor.as_mut().unwrap();
        let (mut l, mut r) = (vec![0.5f32; 64], vec![0.5f32; 64]);
        proc_.process(&mut l, &mut r);
        assert!((l[0] - 0.5).abs() < 1e-6, "unity gain passes through, got {}", l[0]);

        proc_.queue_param(gi.id, 2.0);
        let (mut l, mut r) = (vec![0.5f32; 64], vec![0.5f32; 64]);
        proc_.process(&mut l, &mut r);
        assert!((l[0] - 1.0).abs() < 1e-6, "gain 2.0 doubles, got {}", l[0]);
        inst.shutdown();
    }

    #[test]
    fn tone_instrument_follows_notes() {
        let Some(p) = testplug("libdaw_clap_testplug_tone.so") else { return };
        let mut inst = ClapInstance::load(p.to_str().unwrap(), "test.tone", 48_000, 256)
            .expect("load test.tone");
        {
            let proc_ = inst.processor.as_mut().unwrap();
            let (mut l, mut r) = (vec![0f32; 64], vec![0f32; 64]);
            proc_.process(&mut l, &mut r);
            assert_eq!(l[0], 0.0, "silent before note");
            proc_.queue_note_on(60, 100);
            proc_.process(&mut l, &mut r);
            assert!((l[0] - 0.25).abs() < 1e-6, "tone while held, got {}", l[0]);
            proc_.queue_note_off(60);
            proc_.process(&mut l, &mut r);
            assert_eq!(l[0], 0.0, "silent after note off");
        }
        inst.shutdown();
    }

    #[test]
    fn state_round_trip_restores_param() {
        let Some(p) = testplug("libdaw_clap_testplug_gain.so") else { return };
        let path = p.to_str().unwrap();
        let mut a = ClapInstance::load(path, "test.gain", 48_000, 256).unwrap();
        let gid = a.handle.param_infos[0].id;
        a.processor.as_mut().unwrap().queue_param(gid, 1.5);
        let (mut l, mut r) = (vec![1.0f32; 8], vec![1.0f32; 8]);
        a.processor.as_mut().unwrap().process(&mut l, &mut r);

        let blob = a.handle.collect_state().expect("state blob");
        let mut b = ClapInstance::load(path, "test.gain", 48_000, 256).unwrap();
        b.handle.load_state(&blob).expect("state load");
        let (mut l, mut r) = (vec![1.0f32; 8], vec![1.0f32; 8]);
        b.processor.as_mut().unwrap().process(&mut l, &mut r);
        assert!((l[0] - 1.5).abs() < 1e-6, "restored gain 1.5, got {}", l[0]);
        a.shutdown();
        b.shutdown();
    }
}

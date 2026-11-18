//! CLAP plugin hosting (effects + instruments) built on clack-host.
//!
//! Lifecycle: `ClapInstance::load` (any non-RT thread) → split into a
//! main-thread `ClapHandle` (params/state/deactivation; must stay on the
//! creating thread) and a `Send` `ClapProcessor` that the mixer drives on
//! the RT thread. Processors are returned to their creating thread for
//! teardown — never dropped on the RT thread.

use clack_extensions::params::{ParamInfoBuffer, ParamInfoFlags, PluginParams};
use clack_extensions::state::{PluginState, HostState, HostStateImpl};
use clack_extensions::audio_ports::{PluginAudioPorts, AudioPortInfoBuffer};
use clack_extensions::gui::{PluginGui, HostGui, HostGuiImpl, GuiConfiguration, GuiApiType, GuiSize};
use clack_extensions::timer::{HostTimer, HostTimerImpl, PluginTimer, TimerId};
use clack_extensions::params::{HostParams, HostParamsImplShared, HostParamsImplMainThread, ParamRescanFlags, ParamClearFlags};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use clack_host::events::{UnknownEvent, EventHeader};
use clack_host::events::io::{OutputEventBuffer, TryPushError};
use clack_host::events::spaces::CoreEventSpace;
use clack_host::events::event_types::{TransportEvent, TransportFlags};
use std::time::{Instant, Duration};
use clack_host::events::event_types::{NoteOffEvent, NoteOnEvent, ParamValueEvent};
use clack_host::events::Match;
use clack_host::utils::Cookie;
use clack_host::prelude::*;
use std::ffi::CString;
use std::path::PathBuf;

/// Max frames per process call — matches the mixer's block cap.
const CLAP_MAX_FRAMES: u32 = 8192;

pub static DROPPED_CLAP_EVENTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static CLAP_PROCESS_ERRORS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const MAX_INPUT_EVENTS: u32 = 512;

// ── Host handler boilerplate ──────────────────────────────────────────────────

#[derive(Default)]
pub struct DawClapShared {
    allow_gui: bool,
    callback: AtomicBool,
    gui_action: AtomicU8,
    dirty: Arc<AtomicBool>,
}
impl<'a> SharedHandler<'a> for DawClapShared {
    fn request_restart(&self) {
        eprintln!("clap: plugin requested restart (ignored this slice)");
    }
    fn request_process(&self) {}
    fn request_callback(&self) { self.callback.store(true, Ordering::Release); }
}

impl HostGuiImpl for DawClapShared {
    fn resize_hints_changed(&self) {}
    fn request_resize(&self, _size: GuiSize) -> Result<(), HostError> { Ok(()) }
    fn request_show(&self) -> Result<(), HostError> { self.gui_action.store(1, Ordering::Release); Ok(()) }
    fn request_hide(&self) -> Result<(), HostError> { self.gui_action.store(2, Ordering::Release); Ok(()) }
    fn closed(&self, destroyed: bool) { self.gui_action.store(if destroyed { 3 } else { 2 }, Ordering::Release); }
}
impl HostParamsImplShared for DawClapShared {
    fn request_flush(&self) { self.dirty.store(true, Ordering::Release); }
}

#[derive(Default)]
pub struct DawClapMainThread<'a> {
    plugin: Option<InitializedPluginHandle<'a>>,
    params: Option<PluginParams>,
    state: Option<PluginState>,
    timers: Vec<(TimerId, Duration, Instant)>,
    next_timer: u32,
    dirty: bool,
}
impl<'a> MainThreadHandler<'a> for DawClapMainThread<'a> {
    fn initialized(&mut self, instance: InitializedPluginHandle<'a>) {
        self.params = instance.get_extension();
        self.state = instance.get_extension();
        self.plugin = Some(instance);
    }
}

impl HostTimerImpl for DawClapMainThread<'_> {
    fn register_timer(&mut self, period_ms: u32) -> Result<TimerId, HostError> {
        self.next_timer = self.next_timer.wrapping_add(1);
        let id = TimerId(self.next_timer);
        self.timers.push((id, Duration::from_millis(period_ms.max(10) as u64), Instant::now()));
        Ok(id)
    }
    fn unregister_timer(&mut self, id: TimerId) -> Result<(), HostError> {
        self.timers.retain(|t| t.0 != id); Ok(())
    }
}
impl HostParamsImplMainThread for DawClapMainThread<'_> {
    fn rescan(&mut self, _flags: ParamRescanFlags) { self.dirty = true; }
    fn clear(&mut self, _id: ClapId, _flags: ParamClearFlags) {}
}
impl HostStateImpl for DawClapMainThread<'_> {
    fn mark_dirty(&mut self) { self.dirty = true; }
}

pub struct DawClapHost;
impl HostHandlers for DawClapHost {
    type Shared<'a> = DawClapShared;
    type MainThread<'a> = DawClapMainThread<'a>;
    type AudioProcessor<'a> = ();
    fn declare_extensions(builder: &mut HostExtensions<Self>, shared: &Self::Shared<'_>) {
        builder.register::<HostParams>().register::<HostState>();
        // Only the live guitar frontend currently services native UI/timers.
        if shared.allow_gui { builder.register::<HostGui>().register::<HostTimer>(); }
    }
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

/// True if `path` resolves (symlinks followed) to a file inside one of
/// `roots`. Project files are shareable and LLM-editable, so a slot's `path`
/// is untrusted input: loading it unchecked would `dlopen` (and run the
/// constructors of) any shared library named in a downloaded song.
pub fn is_trusted_plugin_path(path: &str, roots: &[PathBuf]) -> bool {
    let Ok(real) = std::fs::canonicalize(path) else { return false };
    if !real.is_file() {
        return false;
    }
    roots
        .iter()
        .filter_map(|r| std::fs::canonicalize(r).ok())
        .any(|root| real.starts_with(root))
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
    pub stepped: bool,
    pub readonly: bool,
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

/// The opaque chunk of an FXP "FPCh" preset file (the layout Surge XT's
/// `.fxp` patches use): `CcnK` at 0, `FPCh` at 8, big-endian chunk size at
/// 56, chunk data from 60. Loading that chunk as plugin state loads the patch.
pub fn fxp_chunk(bytes: &[u8]) -> Result<&[u8], String> {
    if bytes.len() < 60 || &bytes[0..4] != b"CcnK" {
        return Err("not an FXP preset file".into());
    }
    if &bytes[8..12] != b"FPCh" {
        return Err("FXP preset has no opaque chunk (only FPCh presets are supported)".into());
    }
    let size = u32::from_be_bytes([bytes[56], bytes[57], bytes[58], bytes[59]]) as usize;
    bytes.get(60..60usize.saturating_add(size)).ok_or_else(|| "FXP chunk size runs past the end of the file".into())
}

/// What identifies a running plugin instance for reuse across live edits:
/// its bundle, plugin id and saved state. Param overrides are left out
/// (they're applied to a running instance as parameter changes); a new
/// state (e.g. a loaded patch) requires rebuilding the instance.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SlotFingerprint {
    pub path: String,
    pub id: String,
    pub state_digest: Option<u64>,
}

pub fn slot_fingerprint(spec: &ClapSlotSpec) -> SlotFingerprint {
    use std::hash::{Hash, Hasher};
    let state_digest = spec.state.as_ref().map(|blob| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        blob.hash(&mut h);
        h.finish()
    });
    SlotFingerprint { path: spec.path.clone(), id: spec.id.clone(), state_digest }
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
    gui_created: bool,
}

/// RT half: drives audio. `Send`; owned by the Mixer while live.
pub struct ClapProcessor {
    started: clack_host::process::StartedPluginAudioProcessor<DawClapHost>,
    events_in: EventBuffer,
    ports_in: AudioPorts,
    ports_out: AudioPorts,
    buf_in: [Vec<f32>; 2],
    buf_out: [Vec<f32>; 2],
    steady: u64,
    input_channels: usize,
    output_channels: usize,
    dirty: Arc<AtomicBool>,
    tempo: Option<f64>,
    sample_rate: f64,
    pub process_failed: bool,
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
        Self::load_with_editor(path, plugin_id, sample_rate, max_frames, false)
    }

    fn load_with_editor(path: &str, plugin_id: &str, sample_rate: u32, max_frames: u32, allow_gui: bool) -> Result<Self, String> {
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
            |_| DawClapShared { allow_gui, ..Default::default() },
            |_| DawClapMainThread::default(),
            &entry,
            &id_c,
            &host_info(),
        )
        .map_err(|e| format!("instantiate {plugin_id}: {e}"))?;

        let (input_channels, output_channels) = {
            let mut plugin = instance.plugin_handle();
            if let Some(ports) = plugin.get_extension::<PluginAudioPorts>() {
                let mut channels = |input| -> Result<usize, String> {
                    let count = ports.count(&mut plugin, input);
                    if count == 0 { return Ok(0); }
                    // Route the main bus, preserving the existing host's
                    // behavior for instruments with optional auxiliary outputs.
                    let mut buffer = AudioPortInfoBuffer::new();
                    let n = ports.get(&mut plugin, 0, input, &mut buffer).ok_or("missing CLAP audio port")?.channel_count as usize;
                    if !(1..=2).contains(&n) { return Err(format!("unsupported CLAP channel count: {n}")); }
                    Ok(n)
                };
                (channels(true)?, channels(false)?)
            } else { (2, 2) }
        };
        if output_channels == 0 { return Err("CLAP plugin has no audio output".into()); }

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
                        stepped: info.flags.contains(ParamInfoFlags::IS_STEPPED),
                        readonly: info.flags.contains(ParamInfoFlags::IS_READONLY),
                    });
                }
            }
        }

        let dirty = instance.access_shared_handler(|s| Arc::clone(&s.dirty));
        let frames = max_frames.min(CLAP_MAX_FRAMES) as usize;
        Ok(ClapInstance {
            handle: ClapHandle { instance, name, param_infos, gui_created: false },
            processor: Some(ClapProcessor {
                started,
                events_in: EventBuffer::with_capacity(MAX_INPUT_EVENTS as usize),
                ports_in: AudioPorts::with_capacity(2, 1),
                ports_out: AudioPorts::with_capacity(2, 1),
                buf_in: [vec![0.0; frames], vec![0.0; frames]],
                buf_out: [vec![0.0; frames], vec![0.0; frames]],
                steady: 0,
                input_channels,
                output_channels,
                dirty,
                tempo: None,
                sample_rate: sample_rate as f64,
                process_failed: false,
            }),
        })
    }

    /// Build from a slot spec: load, then apply state + param overrides.
    /// The spec comes from a project file, so its path must lie inside the
    /// standard CLAP search paths (`$CLAP_PATH`, `~/.clap`, `/usr/lib/clap`…).
    pub fn from_spec(spec: &ClapSlotSpec, sample_rate: u32) -> Result<Self, String> {
        Self::from_spec_with_editor(spec, sample_rate, false)
    }

    pub fn from_live_spec(spec: &ClapSlotSpec, sample_rate: u32) -> Result<Self, String> {
        Self::from_spec_with_editor(spec, sample_rate, true)
    }

    fn from_spec_with_editor(spec: &ClapSlotSpec, sample_rate: u32, allow_gui: bool) -> Result<Self, String> {
        if !is_trusted_plugin_path(&spec.path, &standard_paths()) {
            return Err(format!(
                "refusing to load {}: not inside a CLAP search path ($CLAP_PATH, ~/.clap, /usr/lib/clap, /usr/local/lib/clap)",
                spec.path
            ));
        }
        let mut inst = Self::load_with_editor(&spec.path, &spec.id, sample_rate, CLAP_MAX_FRAMES, allow_gui)?;
        if let Some(blob) = &spec.state {
            if let Err(e) = inst.handle.load_state(blob) {
                inst.shutdown();
                return Err(format!("clap {}: state load failed: {e}", spec.uid));
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
    /// Service main-thread callbacks and GUI timers from the frontend's pump.
    pub fn pump(&mut self) -> bool {
        if self.instance.access_shared_handler(|s| s.callback.swap(false, Ordering::AcqRel)) {
            self.instance.call_on_main_thread_callback();
        }
        let due = self.instance.access_handler_mut(|mt| {
            let now = Instant::now();
            mt.timers.iter_mut().filter_map(|(id, period, last)| {
                if now.duration_since(*last) >= *period { *last = now; Some(*id) } else { None }
            }).collect::<Vec<_>>()
        });
        let mut plugin = self.instance.plugin_handle();
        if let Some(timer) = plugin.get_extension::<PluginTimer>() {
            for id in due { timer.on_timer(&mut plugin, id); }
        }
        let action = self.instance.access_shared_handler(|s| s.gui_action.swap(0, Ordering::AcqRel));
        match action {
            1 => { let _ = self.show_editor(); },
            2 => self.hide_editor(),
            3 => self.close_editor(),
            _ => {}
        }
        let dirty = self.instance.access_shared_handler(|s| s.dirty.swap(false, Ordering::AcqRel));
        self.instance.access_handler_mut(|mt| std::mem::take(&mut mt.dirty)) || dirty
    }

    pub fn show_editor(&mut self) -> Result<(), String> {
        let mut plugin = self.instance.plugin_handle();
        let gui = plugin.get_extension::<PluginGui>().ok_or("plugin has no native editor")?;
        if !self.gui_created {
            let api_type = if cfg!(target_os = "windows") { GuiApiType::WIN32 }
                else if cfg!(target_os = "macos") { GuiApiType::COCOA } else { GuiApiType::X11 };
            let config = GuiConfiguration { api_type, is_floating: true };
            if !gui.is_api_supported(&mut plugin, config) { return Err("plugin does not support a floating editor on this desktop".into()); }
            gui.create(&mut plugin, config).map_err(|e| e.to_string())?;
            self.gui_created = true;
            let title = CString::new(format!("daw-ai · {}", self.name)).map_err(|e| e.to_string())?;
            gui.suggest_title(&mut plugin, &title);
        }
        gui.show(&mut plugin).map_err(|e| e.to_string())
    }

    pub fn hide_editor(&mut self) {
        if self.gui_created {
            let mut plugin = self.instance.plugin_handle();
            if let Some(gui) = plugin.get_extension::<PluginGui>() { let _ = gui.hide(&mut plugin); }
        }
    }

    pub fn close_editor(&mut self) {
        if self.gui_created {
            let mut plugin = self.instance.plugin_handle();
            if let Some(gui) = plugin.get_extension::<PluginGui>() { gui.destroy(&mut plugin); }
            self.gui_created = false;
        }
    }

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

    /// The plugin's current value of parameter `id`.
    pub fn param_value(&mut self, id: u32) -> Option<f64> {
        let params = self.instance.access_handler(|mt: &DawClapMainThread| mt.params)?;
        let mut handle = self.instance.plugin_handle();
        params.get_value(&mut handle, ClapId::new(id))
    }

    /// The plugin's own display text for `value` of parameter `id`
    /// (e.g. "800 Hz"), or `None` when it can't format it.
    pub fn value_to_text(&mut self, id: u32, value: f64) -> Option<String> {
        let params = self.instance.access_handler(|mt: &DawClapMainThread| mt.params)?;
        let mut handle = self.instance.plugin_handle();
        let mut buf = [0u8; 256];
        let text = params.value_to_text(&mut handle, ClapId::new(id), value, &mut buf).ok()?;
        Some(String::from_utf8_lossy(text).trim().to_string())
    }

    /// Parse display text (e.g. "800 Hz") into a value of parameter `id`,
    /// as the plugin itself does; `None` when it rejects the text.
    pub fn text_to_value(&mut self, id: u32, text: &str) -> Option<f64> {
        let params = self.instance.access_handler(|mt: &DawClapMainThread| mt.params)?;
        let text = CString::new(text).ok()?;
        let mut handle = self.instance.plugin_handle();
        params.text_to_value(&mut handle, ClapId::new(id), &text)
    }

    /// Tear down a returned processor (stop → deactivate).
    pub fn deactivate(&mut self, processor: ClapProcessor) {
        self.close_editor();
        let stopped = processor.started.stop_processing();
        self.instance.deactivate(stopped);
    }
}

/// Compute saved-project parameter changes for an already running plugin.
/// Removing a saved override restores the plugin's declared default (undo).
pub fn parameter_updates(previous: &[(u32, f64)], next: &[(u32, f64)], infos: &[ClapParamInfo]) -> Vec<(u32, f64)> {
    let mut updates: Vec<_> = next.iter().copied()
        .filter(|(id, value)| previous.iter().find(|(old_id, _)| old_id == id).map(|(_, old)| old) != Some(value))
        .collect();
    for (id, _) in previous {
        if !next.iter().any(|(new_id, _)| new_id == id) {
            if let Some(info) = infos.iter().find(|info| info.id == *id) { updates.push((*id, info.default)); }
        }
    }
    updates
}

// Notice native-editor parameter changes without allocating an event list on
// the audio thread. The frontend snapshots state/values on its own thread.
struct ParameterNotice<'a>(&'a AtomicBool);
impl OutputEventBuffer for ParameterNotice<'_> {
    fn try_push(&mut self, event: &UnknownEvent) -> Result<(), TryPushError> {
        if let Some(CoreEventSpace::ParamValue(_)) = event.as_core_event() {
            self.0.store(true, Ordering::Release);
        }
        Ok(())
    }
}

impl ClapProcessor {
    pub fn set_tempo(&mut self, bpm: f64) {
        self.tempo = bpm.is_finite().then_some(bpm.clamp(20., 300.));
    }
    fn reserve_event(&self) -> bool {
        if self.events_in.len() < MAX_INPUT_EVENTS { return true; }
        DROPPED_CLAP_EVENTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        false
    }
    pub fn queue_note_on(&mut self, key: u8, velocity: u8) {
        if key > 127 || !self.reserve_event() { return; }
        self.events_in.push(&NoteOnEvent::new(
            0,
            Pckn::new(0u16, 0u16, key as u16, Match::All),
            velocity as f64 / 127.0,
        ));
    }

    pub fn queue_note_off(&mut self, key: u8) {
        if key > 127 { return; }
        if !self.reserve_event() { self.queue_all_notes_off(); return; }
        self.events_in.push(&NoteOffEvent::new(
            0,
            Pckn::new(0u16, 0u16, key as u16, Match::All),
            0.0,
        ));
    }

    /// Release everything (transport panic / strip mute).
    pub fn queue_all_notes_off(&mut self) {
        // Emergency release takes priority over queued notes/parameters. Using
        // all MIDI keys also covers releases already queued before this panic.
        self.events_in.clear();
        for key in 0..128u16 {
            self.events_in.push(&NoteOffEvent::new(
                0,
                Pckn::new(0u16, 0u16, key, Match::All),
                0.0,
            ));
        }
    }

    pub fn queue_param(&mut self, param_id: u32, value: f64) {
        if !self.reserve_event() { return; }
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
        if self.input_channels == 1 {
            for i in 0..frames { self.buf_in[0][i] = 0.5 * (l[i] + r[i]); }
        }
        self.buf_out[0][..frames].fill(0.0);
        self.buf_out[1][..frames].fill(0.0);

        {
            let input_events = InputEvents::from_buffer(&self.events_in);
            let mut notice = ParameterNotice(&self.dirty);
            let mut output_events = OutputEvents::from_buffer(&mut notice);

            let input_audio = self.ports_in.with_input_buffers([AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_input_only(
                    self.buf_in
                        .iter_mut().take(self.input_channels)
                        .map(|b| InputChannel::variable(&mut b[..frames])),
                ),
            }].into_iter().take(usize::from(self.input_channels > 0)));
            let mut output_audio = self.ports_out.with_output_buffers([AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_output_only(
                    self.buf_out.iter_mut().take(self.output_channels).map(|b| &mut b[..frames]),
                ),
            }]);

            let transport = self.tempo.map(|tempo| {
                let seconds = self.steady as f64 / self.sample_rate;
                let beats = seconds * tempo / 60.;
                TransportEvent {
                    header: EventHeader::default(),
                    flags: TransportFlags::HAS_TEMPO | TransportFlags::HAS_TIME_SIGNATURE
                        | TransportFlags::HAS_BEATS_TIMELINE | TransportFlags::HAS_SECONDS_TIMELINE,
                    tempo, tempo_inc: 0., song_pos_beats: beats.into(), song_pos_seconds: seconds.into(),
                    loop_start_beats: Default::default(), loop_end_beats: Default::default(),
                    loop_start_seconds: Default::default(), loop_end_seconds: Default::default(),
                    bar_start: ((beats / 4.).floor() * 4.).into(), bar_number: (beats / 4.) as i32,
                    time_signature_numerator: 4, time_signature_denominator: 4,
                }
            });
            if self.started.process(
                &input_audio,
                &mut output_audio,
                &input_events,
                &mut output_events,
                Some(self.steady),
                transport.as_ref(),
            ).is_err() {
                self.process_failed = true;
                CLAP_PROCESS_ERRORS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.buf_out[0][..frames].fill(0.0);
                self.buf_out[1][..frames].fill(0.0);
            }
        }

        self.events_in.clear();
        self.steady = self.steady.wrapping_add(frames as u64);
        l[..frames].copy_from_slice(&self.buf_out[0][..frames]);
        r[..frames].copy_from_slice(&self.buf_out[if self.output_channels == 1 { 0 } else { 1 }][..frames]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_changes_with_state_not_params() {
        let spec = |state: Option<Vec<u8>>, params: Vec<(u32, f64)>| ClapSlotSpec {
            uid: "c1".into(), path: "/p.clap".into(), id: "x".into(), state, params,
        };
        assert_eq!(
            slot_fingerprint(&spec(Some(vec![1]), vec![])),
            slot_fingerprint(&spec(Some(vec![1]), vec![(3, 0.5)])),
            "param overrides alone don't rebuild the plugin"
        );
        assert_ne!(slot_fingerprint(&spec(Some(vec![1]), vec![])), slot_fingerprint(&spec(Some(vec![2]), vec![])));
        assert_ne!(slot_fingerprint(&spec(None, vec![])), slot_fingerprint(&spec(Some(vec![]), vec![])));
    }

    #[test]
    fn fxp_chunk_extracts_opaque_chunk() {
        let fxp = |magic: &[u8; 4], size: u32, data: &[u8]| {
            let mut b = b"CcnK".to_vec();
            b.extend_from_slice(&[0; 4]);
            b.extend_from_slice(magic);
            b.resize(56, 0);
            b.extend_from_slice(&size.to_be_bytes());
            b.extend_from_slice(data);
            b
        };
        let b = fxp(b"FPCh", 5, b"hello");
        assert_eq!(fxp_chunk(&b), Ok(&b"hello"[..]));
        assert!(fxp_chunk(b"CcnK").is_err(), "truncated header");
        assert!(fxp_chunk(&fxp(b"FxCk", 5, b"hello")).is_err(), "parameter preset, not a chunk");
        assert!(fxp_chunk(&fxp(b"FPCh", 9, b"hello")).is_err(), "size past the end");
    }

    fn testplug(name: &str) -> Option<std::path::PathBuf> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug")
            .join(name);
        if p.exists() {
            Some(p)
        } else {
            assert!(std::env::var_os("DAW_REQUIRE_TEST_ASSETS").is_none(), "required audio test assets missing"); eprintln!("skip: {name} not built (cargo build --workspace)");
            None
        }
    }

    #[test]
    fn event_overload_is_bounded_and_note_off_still_releases() {
        let Some(path) = testplug("libdaw_clap_testplug_tone.so") else { return; };
        let mut instance = ClapInstance::load(path.to_str().unwrap(), "test.tone", 48000, 256).unwrap();
        let processor = instance.processor.as_mut().unwrap();
        let (mut left, mut right) = ([0.; 64], [0.; 64]);
        processor.queue_note_on(60, 100);
        processor.process(&mut left, &mut right);
        assert!(left.iter().any(|sample| *sample != 0.));
        for _ in 0..MAX_INPUT_EVENTS { processor.queue_param(999, 0.); }
        assert_eq!(processor.events_in.len(), MAX_INPUT_EVENTS);
        let before = DROPPED_CLAP_EVENTS.load(std::sync::atomic::Ordering::Relaxed);
        processor.queue_note_off(60);
        assert_eq!(processor.events_in.len(), 128, "overload prioritizes bounded all-notes-off");
        assert!(DROPPED_CLAP_EVENTS.load(std::sync::atomic::Ordering::Relaxed) > before);
        processor.process(&mut left, &mut right);
        assert!(left.iter().all(|sample| *sample == 0.));
        processor.queue_note_on(64, 100);
        processor.queue_note_off(64);
        processor.queue_all_notes_off();
        assert_eq!(processor.events_in.len(), 128);
        instance.shutdown();
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
    fn untrusted_plugin_paths_are_rejected() {
        let dir = std::env::temp_dir().join(format!("daw_clap_trust_{}", std::process::id()));
        let root = dir.join("plugins");
        std::fs::create_dir_all(&root).unwrap();
        let inside = root.join("ok.clap");
        let outside = dir.join("evil.so");
        std::fs::write(&inside, b"").unwrap();
        std::fs::write(&outside, b"").unwrap();
        let roots = vec![root.clone()];
        assert!(is_trusted_plugin_path(inside.to_str().unwrap(), &roots));
        assert!(!is_trusted_plugin_path(outside.to_str().unwrap(), &roots));
        // `..` escapes are resolved before the prefix check.
        let escape = root.join("../evil.so");
        assert!(!is_trusted_plugin_path(escape.to_str().unwrap(), &roots));
        assert!(!is_trusted_plugin_path("/nonexistent/x.clap", &roots));
        // from_spec refuses before any dlopen.
        let spec = ClapSlotSpec {
            uid: "c1".into(),
            path: outside.to_string_lossy().into_owned(),
            id: "x".into(),
            state: None,
            params: vec![],
        };
        let err = ClapInstance::from_spec(&spec, 48_000).err().expect("refused");
        assert!(err.contains("refusing"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
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
    fn handle_reads_param_value() {
        let Some(p) = testplug("libdaw_clap_testplug_gain.so") else { return };
        let mut inst = ClapInstance::load(p.to_str().unwrap(), "test.gain", 48_000, 256).expect("load test.gain");
        let id = inst.handle.param_infos.iter().find(|i| i.name == "Gain").expect("Gain").id;
        assert_eq!(inst.handle.param_value(id), Some(1.0));
        assert_eq!(inst.handle.value_to_text(id, 1.5).as_deref(), Some("1.50"));
        assert_eq!(inst.handle.text_to_value(id, "0.25"), Some(0.25));
        assert_eq!(inst.handle.text_to_value(id, "loud"), None, "unparseable text");
        inst.shutdown();
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
        assert_eq!(parameter_updates(&[(gi.id, 2.0)], &[], std::slice::from_ref(&gi)), vec![(gi.id, 1.0)]);
        assert!(parameter_updates(&[(gi.id, 2.0)], &[(gi.id, 2.0)], std::slice::from_ref(&gi)).is_empty());
        assert_eq!(parameter_updates(&[(gi.id, 1.0)], &[(gi.id, 2.0)], std::slice::from_ref(&gi)), vec![(gi.id, 2.0)]);

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

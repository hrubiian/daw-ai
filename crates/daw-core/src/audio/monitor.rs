//! Low-latency live monitoring: instrument input → `GuitarRig` → output.
//!
//! This is its own pair of small-buffer streams, separate from the song
//! mixer's output stream. The mixer renders plugins and many voices and is
//! opened with the device's comfortable default buffer; pushing all of that
//! down to a 32-frame period would trade the guitar's latency for dropouts in
//! the song. Here the per-sample work is the amp, cabinet and built-in pedals.
//!
//! Path and where the delay comes from:
//!   input period → lock-free ring (a small cushion) → output device queue
//! The dry DSP path adds no block delay (see `amp.rs`); pedal delay effects
//! are intentional. `MonitorStats::latency_ms`
//! reports the sum of those three as measured while running; converter and
//! USB transfer delay inside the interface are on top and cannot be seen
//! from here.

use super::amp::{AmpSettings, GuitarRig};
use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Buffer sizes offered in the UI, in frames. The stream is opened with this
/// as the device buffer; the period (callback size) is a quarter of it on
/// ALSA. 128 at 48 kHz is a 0.7 ms period and at most 2.7 ms queued.
pub const BUFFER_CHOICES: [u32; 5] = [64, 128, 256, 512, 1024];

#[derive(Debug, Clone, PartialEq)]
pub struct MonitorConfig {
    /// cpal input device name; `None` = system default input.
    pub input_device: Option<String>,
    /// Zero-based input channel the instrument is plugged into.
    pub input_channel: u16,
    /// Full physical channel count when the sound server reports it.
    pub source_channels: Option<u16>,
    /// cpal output device name; `None` = system default output.
    pub output_device: Option<String>,
    /// Requested device buffer in frames (see `BUFFER_CHOICES`).
    pub buffer_frames: u32,
    /// Sound-server (PipeWire) node names to capture from / play to. When
    /// either is set the streams are opened on the system default devices and
    /// pointed at these nodes, instead of opening the named ALSA devices: a
    /// card the sound server already holds cannot be opened directly, and the
    /// server lets the song and the guitar share one interface.
    pub server_source: Option<String>,
    pub server_sink: Option<String>,
}

/// Serializes the brief `PIPEWIRE_NODE` environment hand-off in `with_target`.
static TARGET_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run `open` with the PipeWire ALSA plugin told which node to connect the
/// stream it is about to create to. The plugin reads `PIPEWIRE_NODE` once,
/// when the stream is opened; the variable is restored straight after.
pub(crate) fn with_target<T>(node: Option<&str>, open: impl FnOnce() -> T) -> T {
    let Some(node) = node else { return open() };
    let _guard = TARGET_ENV.lock().unwrap_or_else(|e| e.into_inner());
    let previous = std::env::var_os("PIPEWIRE_NODE");
    std::env::set_var("PIPEWIRE_NODE", node);
    let result = open();
    match previous {
        Some(v) => std::env::set_var("PIPEWIRE_NODE", v),
        None => std::env::remove_var("PIPEWIRE_NODE"),
    }
    result
}

/// The interface's native rate when the device offers it. The default config
/// of the system device is often 44.1 kHz, which makes the sound server
/// resample both directions (extra delay) when the card runs at 48 kHz.
const PREFERRED_RATE: u32 = 48_000;

fn output_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    let default = device.default_output_config()?;
    if default.sample_rate().0 == PREFERRED_RATE { return Ok(default); }
    let better = device.supported_output_configs().ok().and_then(|mut ranges| ranges.find(|r|
        r.channels() == default.channels() && r.sample_format() == default.sample_format()
            && r.min_sample_rate().0 <= PREFERRED_RATE && PREFERRED_RATE <= r.max_sample_rate().0));
    Ok(better.map(|r| r.with_sample_rate(cpal::SampleRate(PREFERRED_RATE))).unwrap_or(default))
}

/// Live numbers shared with the UI (all lock-free).
#[derive(Default)]
pub struct MonitorShared {
    pub sample_rate: AtomicU32,
    /// Frames per input / output callback, as delivered by the device.
    pub in_period: AtomicU32,
    pub out_period: AtomicU32,
    /// Output device buffer actually granted (frames).
    pub out_buffer: AtomicU32,
    /// Smoothed ring fill seen by the output callback, in 1/16 frames.
    ring_fill_x16: AtomicU32,
    /// Output callbacks that found too little input (a gap was played).
    pub underruns: AtomicU64,
    /// Input frames discarded to pull the delay back after a stall.
    pub dropped_frames: AtomicU64,
    /// Frames currently kept queued as a safety cushion (grows after dropouts).
    pub cushion: AtomicU32,
    /// Output callbacks spent waiting for the cushion (silence played).
    pub waiting_callbacks: AtomicU64,
    /// Output callbacks in total, and the largest one seen (frames).
    pub out_callbacks: AtomicU64,
    pub out_period_max: AtomicU32,
    in_peak: AtomicU32,
    out_peak: AtomicU32,
    pub in_clipped: AtomicBool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorStats {
    pub sample_rate: u32,
    pub in_period: u32,
    pub out_period: u32,
    pub out_buffer: u32,
    pub ring_fill: f32,
    pub underruns: u64,
    pub dropped_frames: u64,
    pub in_peak: f32,
    pub out_peak: f32,
    pub in_clipped: bool,
}

impl MonitorStats {
    /// Input period + ring cushion + output queue, in milliseconds. The
    /// interface's own converter and USB delay (typically 1–3 ms) is extra.
    pub fn latency_ms(&self) -> f32 {
        if self.sample_rate == 0 { return 0.0; }
        (self.in_period as f32 + self.ring_fill + self.out_buffer.max(self.out_period) as f32) * 1000.0 / self.sample_rate as f32
    }
}

impl MonitorShared {
    pub fn stats(&self) -> MonitorStats {
        MonitorStats {
            sample_rate: self.sample_rate.load(Ordering::Relaxed),
            in_period: self.in_period.load(Ordering::Relaxed),
            out_period: self.out_period.load(Ordering::Relaxed),
            out_buffer: self.out_buffer.load(Ordering::Relaxed),
            ring_fill: self.ring_fill_x16.load(Ordering::Relaxed) as f32 / 16.0,
            underruns: self.underruns.load(Ordering::Relaxed),
            dropped_frames: self.dropped_frames.load(Ordering::Relaxed),
            in_peak: f32::from_bits(self.in_peak.load(Ordering::Relaxed)),
            out_peak: f32::from_bits(self.out_peak.load(Ordering::Relaxed)),
            in_clipped: self.in_clipped.load(Ordering::Relaxed),
        }
    }
}

enum Cmd {
    Settings(AmpSettings),
    Ir(Vec<f32>),
}

/// The output side: pulls input from the ring, keeps the cushion small, runs
/// the rig. Separate from the stream so the policy is testable without audio
/// hardware.
pub struct MonitorCore {
    rig: GuitarRig,
    plugin: Option<super::live_plugin::LivePluginProcessor>,
    /// Reading has started (the cushion was reached at least once since the
    /// last underrun).
    primed: bool,
    fill_avg: f32,
    /// Extra cushion learned from dropouts (frames). Each dropout while
    /// running adds half an input period, so the path settles on the smallest
    /// cushion this machine can actually hold instead of crackling forever at
    /// a setting that is slightly too tight.
    extra: usize,
}

impl MonitorCore {
    pub fn new(sample_rate: u32) -> Self {
        MonitorCore { rig: GuitarRig::new(sample_rate), plugin: None, primed: false, fill_avg: 0.0, extra: 0 }
    }

    pub fn rig_mut(&mut self) -> &mut GuitarRig { &mut self.rig }

    /// Fill `out` (mono) from `input`.
    ///
    /// `cushion` is how many frames to keep queued so ordinary scheduling
    /// jitter between the two callbacks does not cause gaps: one input
    /// period, plus whatever earlier dropouts showed is needed. Playback starts once `out.len() + cushion` frames are waiting;
    /// if a stall leaves more than three cushions extra, the oldest frames
    /// are discarded so the delay returns to one cushion instead of staying
    /// long for the rest of the session.
    pub fn fill(&mut self, input: &mut impl Consumer<Item = f32>, out: &mut [f32], cushion: usize, shared: &MonitorShared) {
        let n = out.len();
        shared.out_callbacks.fetch_add(1, Ordering::Relaxed);
        shared.out_period_max.fetch_max(n as u32, Ordering::Relaxed);
        let period = cushion.max(1);
        let cushion = period + self.extra;
        shared.cushion.store(cushion as u32, Ordering::Relaxed);
        let mut avail = input.occupied_len();
        if !self.primed {
            if avail < n + cushion {
                out.fill(0.0);
                shared.waiting_callbacks.fetch_add(1, Ordering::Relaxed);
                return;
            }
            self.primed = true;
        }
        if avail > n + 3 * cushion {
            let skip = avail - (n + cushion);
            input.skip(skip);
            shared.dropped_frames.fetch_add(skip as u64, Ordering::Relaxed);
            avail -= skip;
        }
        let got = input.pop_slice(out);
        if got < n {
            // Not enough input: play what there is, pad, and re-prime so the
            // cushion is rebuilt rather than underrunning every callback.
            out[got..].fill(0.0);
            shared.underruns.fetch_add(1, Ordering::Relaxed);
            self.primed = false;
            self.extra = (self.extra + period.div_ceil(2)).min(period * 6);
        }
        self.fill_avg += 0.02 * ((avail - got) as f32 - self.fill_avg);
        shared.ring_fill_x16.store((self.fill_avg * 16.0) as u32, Ordering::Relaxed);
        let mut peak = 0f32;
        if let Some(plugin) = self.plugin.as_mut() {
            plugin.process(&mut out[..got]);
        }
        for s in out[..got].iter_mut() {
            if self.plugin.is_none() { *s = self.rig.process(*s); }
            peak = peak.max(s.abs());
        }
        shared.out_peak.store(peak.to_bits(), Ordering::Relaxed);
    }
}

pub struct MonitorStream {
    _input: cpal::Stream,
    _output: cpal::Stream,
    // Declared after streams: the callback returns its processor before teardown.
    pub plugin: Option<super::live_plugin::LivePlugin>,
    pub shared: Arc<MonitorShared>,
    pub input_name: String,
    pub output_name: String,
    pub config: MonitorConfig,
    cmd_tx: crossbeam_channel::Sender<Cmd>,
    /// Old IR buffers come back here to be freed off the audio thread.
    garbage_rx: crossbeam_channel::Receiver<Vec<f32>>,
}

fn find_device(host: &cpal::Host, name: Option<&str>, input: bool) -> Result<cpal::Device> {
    if let Some(want) = name {
        let found = if input { host.input_devices()?.find(|d| d.name().is_ok_and(|n| n == want)) }
                    else { host.output_devices()?.find(|d| d.name().is_ok_and(|n| n == want)) };
        if let Some(d) = found { return Ok(d); }
        if input { anyhow::bail!("input device '{want}' is unavailable"); }
    }
    let d = if input { host.default_input_device() } else { host.default_output_device() };
    d.ok_or_else(|| anyhow::anyhow!("no {} device available", if input { "input" } else { "output" }))
}

pub(super) fn required_input_channel(selected: u16, source_channels: Option<u16>) -> Result<u16> {
    let Some(count) = source_channels else { return Ok(selected); };
    anyhow::ensure!(selected < count, "input channel {} is unavailable (device has {count} input(s))", u32::from(selected) + 1);
    // Opening only stereo on a larger sound-server node can remix rear
    // inputs into the front channels. Open the complete input bus instead.
    Ok(count - 1)
}

/// An input config at exactly `rate` (no resampling, so no resampler delay
/// between the two streams) with enough channels to reach `channel`. Through
/// a sound server the default is stereo, which would hide inputs 3 and up.
pub(crate) fn input_config_at(device: &cpal::Device, rate: u32, channel: u16) -> Result<cpal::SupportedStreamConfig> {
    let default = device.default_input_config()?;
    // At least a stereo pair: a mono stream from a multi-input interface is
    // a mix of its inputs, not input 1.
    let channel = channel.max(1);
    if default.sample_rate().0 == rate && default.channels() > channel { return Ok(default); }
    let mut best: Option<cpal::SupportedStreamConfigRange> = None;
    if let Ok(ranges) = device.supported_input_configs() {
        for range in ranges {
            if !(range.min_sample_rate().0 <= rate && rate <= range.max_sample_rate().0) { continue; }
            // Fewest channels that still include the wanted one; prefer the
            // default sample format among equals.
            let reaches = range.channels() > channel;
            let better = match &best {
                None => true,
                Some(b) => {
                    let b_reaches = b.channels() > channel;
                    (reaches && !b_reaches)
                        || (reaches == b_reaches && range.channels() < b.channels() && reaches)
                        || (reaches == b_reaches && range.channels() == b.channels()
                            && range.sample_format() == default.sample_format() && b.sample_format() != default.sample_format())
                }
            };
            if better { best = Some(range); }
        }
    }
    best.map(|r| r.with_sample_rate(cpal::SampleRate(rate))).ok_or_else(|| anyhow::anyhow!(
        "input runs at {} Hz but the output runs at {rate} Hz — set both to the same rate", default.sample_rate().0))
}

fn build_input<T>(device: &cpal::Device, config: &cpal::StreamConfig, channel: usize,
    mut producer: ringbuf::HeapProd<f32>, shared: Arc<MonitorShared>) -> Result<cpal::Stream>
where T: cpal::SizedSample, f32: cpal::FromSample<T> {
    use cpal::Sample;
    let ch = config.channels.max(1) as usize;
    let channel = channel.min(ch - 1);
    Ok(device.build_input_stream(config, move |raw: &[T], _| {
        let frames = raw.len() / ch;
        shared.in_period.store(frames as u32, Ordering::Relaxed);
        let mut peak = 0f32;
        for frame in raw.chunks_exact(ch) {
            let s = f32::from_sample(frame[channel]);
            peak = peak.max(s.abs());
            // A full ring means the output stalled; the output side trims.
            let _ = producer.try_push(s);
        }
        shared.in_peak.store(peak.to_bits(), Ordering::Relaxed);
        if peak >= 0.999 { shared.in_clipped.store(true, Ordering::Relaxed); }
    }, |e| eprintln!("monitor input stream error: {e}"), None)?)
}

#[allow(clippy::too_many_arguments)]
fn build_output<T>(device: &cpal::Device, config: &cpal::StreamConfig, mut consumer: ringbuf::HeapCons<f32>,
    mut core: MonitorCore, shared: Arc<MonitorShared>, cmd_rx: crossbeam_channel::Receiver<Cmd>,
    garbage_tx: crossbeam_channel::Sender<Vec<f32>>) -> Result<cpal::Stream>
where T: cpal::SizedSample + cpal::FromSample<f32> {
    let ch = config.channels.max(1) as usize;
    let mut mono = vec![0f32; 16_384];
    Ok(device.build_output_stream(config, move |data: &mut [T], _| {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Cmd::Settings(s) => core.rig_mut().set(s),
                Cmd::Ir(ir) => { let old = core.rig_mut().swap_ir(ir); let _ = garbage_tx.try_send(old); }
            }
        }
        let frames = (data.len() / ch).min(mono.len());
        shared.out_period.store(frames as u32, Ordering::Relaxed);
        let cushion = shared.in_period.load(Ordering::Relaxed).max(16) as usize;
        core.fill(&mut consumer, &mut mono[..frames], cushion, &shared);
        for (frame, &s) in data.chunks_exact_mut(ch).zip(mono.iter()) {
            let v = T::from_sample(s);
            for (i, out) in frame.iter_mut().enumerate() {
                // The instrument goes to the first stereo pair only.
                *out = if i < 2 { v } else { T::from_sample(0.0f32) };
            }
        }
    }, |e| eprintln!("monitor output stream error: {e}"), None)?)
}

macro_rules! by_format {
    ($format:expr, $what:literal, $build:ident ( $($arg:expr),* )) => {
        match $format {
            cpal::SampleFormat::F32 => $build::<f32>($($arg),*),
            cpal::SampleFormat::I32 => $build::<i32>($($arg),*),
            cpal::SampleFormat::I16 => $build::<i16>($($arg),*),
            cpal::SampleFormat::U16 => $build::<u16>($($arg),*),
            cpal::SampleFormat::I8 => $build::<i8>($($arg),*),
            cpal::SampleFormat::U8 => $build::<u8>($($arg),*),
            cpal::SampleFormat::F64 => $build::<f64>($($arg),*),
            other => Err(anyhow::anyhow!(concat!("unsupported ", $what, " sample format {:?}"), other)),
        }
    };
}

impl MonitorStream {
    /// Open both streams and start monitoring with `settings`.
    ///
    /// The requested buffer is tried first; if the device refuses it, larger
    /// sizes are tried in turn, and finally the device default, so monitoring
    /// starts (at a higher latency, visible in the stats) rather than failing.
    pub fn open(config: &MonitorConfig, settings: AmpSettings, ir: Option<Vec<f32>>) -> Result<MonitorStream> {
        Self::open_dsp(config, settings, ir, None, 120.)
    }

    pub fn open_plugin(config: &MonitorConfig, slot: &crate::project::ClapSlot, bpm: f64) -> Result<MonitorStream> {
        Self::open_dsp(config, AmpSettings::default(), None, Some(slot), bpm)
    }

    fn open_dsp(config: &MonitorConfig, settings: AmpSettings, ir: Option<Vec<f32>>, slot: Option<&crate::project::ClapSlot>, bpm: f64) -> Result<MonitorStream> {
        let host = cpal::default_host();
        // Through the sound server: the system default devices, each stream
        // pointed at its node. Otherwise the named devices (no sound server).
        let via_server = config.server_source.is_some() || config.server_sink.is_some();
        let out_name = if via_server { None } else { config.output_device.as_deref() };
        let in_name = if config.server_source.is_some() { None } else { config.input_device.as_deref() };
        let out_dev = find_device(&host, out_name, false)?;
        let in_dev = find_device(&host, in_name, true)?;
        let out_cfg = output_config(&out_dev)?;
        let rate = out_cfg.sample_rate().0;
        let required = required_input_channel(config.input_channel, config.source_channels)?;
        let in_cfg = input_config_at(&in_dev, rate, required)?;
        anyhow::ensure!(required < in_cfg.channels(), "cannot open all {} required guitar input channels", u32::from(required) + 1);

        let mut sizes: Vec<cpal::BufferSize> = BUFFER_CHOICES.iter().copied()
            .filter(|b| *b >= config.buffer_frames.max(16)).map(cpal::BufferSize::Fixed).collect();
        sizes.push(cpal::BufferSize::Default);
        let mut last_err = anyhow::anyhow!("no buffer size tried");
        for size in sizes {
            match Self::open_with(config, settings, ir.clone(), slot, bpm, &in_dev, &in_cfg, &out_dev, &out_cfg, size) {
                Ok(stream) => return Ok(stream),
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    #[allow(clippy::too_many_arguments)]
    fn open_with(config: &MonitorConfig, settings: AmpSettings, ir: Option<Vec<f32>>, slot: Option<&crate::project::ClapSlot>, bpm: f64,
        in_dev: &cpal::Device, in_cfg: &cpal::SupportedStreamConfig,
        out_dev: &cpal::Device, out_cfg: &cpal::SupportedStreamConfig, size: cpal::BufferSize) -> Result<MonitorStream> {
        let rate = out_cfg.sample_rate().0;
        let shared = Arc::new(MonitorShared::default());
        shared.sample_rate.store(rate, Ordering::Relaxed);
        if let cpal::BufferSize::Fixed(frames) = size { shared.out_buffer.store(frames, Ordering::Relaxed); }

        let (plugin, processor) = match slot {
            Some(slot) => {
                let (owner, processor) = super::live_plugin::LivePlugin::load(slot, rate).map_err(anyhow::Error::msg)?;
                owner.set_tempo(bpm);
                (Some(owner), Some(processor))
            }
            None => (None, None),
        };
        let mut core = MonitorCore::new(rate);
        core.plugin = processor;
        core.rig_mut().set(settings);
        if let Some(ir) = ir { core.rig_mut().set_ir(&ir); }

        // Room for a long stall; steady state holds about one input period.
        let (producer, consumer) = HeapRb::<f32>::new(16_384).split();
        let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(64);
        let (garbage_tx, garbage_rx) = crossbeam_channel::bounded(8);

        let mut in_stream: cpal::StreamConfig = in_cfg.clone().into();
        in_stream.buffer_size = size;
        let mut out_stream: cpal::StreamConfig = out_cfg.clone().into();
        out_stream.buffer_size = size;

        let channel = config.input_channel as usize;
        let output = with_target(config.server_sink.as_deref(), || by_format!(out_cfg.sample_format(), "output",
            build_output(out_dev, &out_stream, consumer, core, Arc::clone(&shared), cmd_rx, garbage_tx)))?;
        let input = with_target(config.server_source.as_deref(), || by_format!(in_cfg.sample_format(), "input",
            build_input(in_dev, &in_stream, channel, producer, Arc::clone(&shared))))?;
        input.play()?;
        output.play()?;
        Ok(MonitorStream {
            _input: input, _output: output, plugin, shared,
            input_name: config.server_source.clone().unwrap_or_else(|| in_dev.name().unwrap_or_default()),
            output_name: config.server_sink.clone().unwrap_or_else(|| out_dev.name().unwrap_or_default()),
            config: config.clone(), cmd_tx, garbage_rx,
        })
    }

    /// Send new control values to the running rig.
    pub fn set(&self, settings: AmpSettings) {
        let _ = self.cmd_tx.try_send(Cmd::Settings(settings));
        self.collect_garbage();
    }

    /// Install an impulse response prepared with `amp::prepare_ir` at this
    /// stream's sample rate (empty = remove it).
    pub fn set_ir(&self, ir: Vec<f32>) {
        let _ = self.cmd_tx.try_send(Cmd::Ir(ir));
        self.collect_garbage();
    }

    fn collect_garbage(&self) {
        while self.garbage_rx.try_recv().is_ok() {}
    }

    pub fn stats(&self) -> MonitorStats { self.shared.stats() }
    pub fn sample_rate(&self) -> u32 { self.shared.sample_rate.load(Ordering::Relaxed) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::amp::Cabinet;
    use ringbuf::traits::Observer;

    const CLEAN: AmpSettings = AmpSettings { amp_on: false, gain: 0.0, bass: 0.5, mid: 0.5, treble: 0.5, level: 0.5, gate: 0.0, cabinet: Cabinet::Off, pedals: super::super::pedals::PedalSettings::BYPASSED };

    fn core() -> MonitorCore { let mut c = MonitorCore::new(48000); c.rig_mut().set(CLEAN); c }

    #[test]
    fn waits_for_the_cushion_then_passes_input_through_in_order() {
        let (mut tx, mut rx) = HeapRb::<f32>::new(4096).split();
        let shared = MonitorShared::default();
        let mut c = core();
        let mut out = [9.0f32; 32];
        // 40 frames waiting: less than 32 + a 16-frame cushion, so stay silent.
        tx.push_slice(&(0..40).map(|i| i as f32 * 0.001).collect::<Vec<_>>());
        c.fill(&mut rx, &mut out, 16, &shared);
        assert!(out.iter().all(|v| *v == 0.0));
        assert_eq!(rx.occupied_len(), 40, "nothing consumed while priming");
        // Enough now: output is the input from the beginning, sample for sample.
        tx.push_slice(&(40..64).map(|i| i as f32 * 0.001).collect::<Vec<_>>());
        c.fill(&mut rx, &mut out, 16, &shared);
        for (i, v) in out.iter().enumerate() { assert!((v - i as f32 * 0.001).abs() < 1e-6, "sample {i}: {v}"); }
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 0);
        assert_eq!(rx.occupied_len(), 32);
    }

    #[test]
    fn a_stall_is_trimmed_back_to_one_cushion_and_an_underrun_reprimes() {
        let (mut tx, mut rx) = HeapRb::<f32>::new(8192).split();
        let shared = MonitorShared::default();
        let mut c = core();
        let mut out = [0f32; 32];
        // The output stalled and 2000 frames (42 ms) piled up.
        tx.push_slice(&(0..2000).map(|i| i as f32).map(|i| i * 1e-4).collect::<Vec<_>>());
        c.fill(&mut rx, &mut out, 16, &shared);
        assert_eq!(rx.occupied_len(), 16, "only the cushion stays queued");
        assert_eq!(shared.dropped_frames.load(Ordering::Relaxed), 2000 - 48);
        assert!((out[31] - 1983.0 * 1e-4).abs() < 1e-6, "the newest audio is played, not the 42 ms old");
        // Input stops: one short read, padded with silence, counted once.
        c.fill(&mut rx, &mut out, 16, &shared);
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 1);
        assert!(out[16..].iter().all(|v| *v == 0.0));
        // Then it waits for a full cushion again instead of underrunning each time.
        tx.push_slice(&[0.1; 20]);
        c.fill(&mut rx, &mut out, 16, &shared);
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 1);
        assert!(out.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn dropouts_grow_the_cushion_until_the_path_is_stable() {
        let (mut tx, mut rx) = HeapRb::<f32>::new(8192).split();
        let shared = MonitorShared::default();
        let mut c = core();
        let mut out = [0f32; 64];
        // Prime with exactly one period of cushion, then starve it.
        tx.push_slice(&[0.1; 128]);
        c.fill(&mut rx, &mut out, 64, &shared);
        assert_eq!(shared.cushion.load(Ordering::Relaxed), 64);
        tx.push_slice(&[0.1; 20]);
        let mut big = [0f32; 128]; // a double-size catch-up read
        c.fill(&mut rx, &mut big, 64, &shared);
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 1);
        // Now it insists on a period and a half before resuming.
        tx.push_slice(&[0.1; 128]);
        c.fill(&mut rx, &mut out, 64, &shared);
        assert!(out.iter().all(|v| *v == 0.0), "64 + 96 frames are not there yet");
        assert_eq!(shared.cushion.load(Ordering::Relaxed), 96);
        tx.push_slice(&[0.1; 32]);
        c.fill(&mut rx, &mut out, 64, &shared);
        assert!(out.iter().all(|v| *v != 0.0));
        assert_eq!(rx.occupied_len(), 96);
        // The growth is bounded: a broken device cannot push the delay up forever.
        for _ in 0..40 { c.primed = true; c.fill(&mut rx, &mut big, 64, &shared); }
        assert_eq!(shared.cushion.load(Ordering::Relaxed), 64 + 64 * 6);
    }

    #[test]
    fn reported_latency_is_period_plus_cushion_plus_output_queue() {
        let s = MonitorStats { sample_rate: 48000, in_period: 32, out_period: 32, out_buffer: 128, ring_fill: 32.0,
            underruns: 0, dropped_frames: 0, in_peak: 0.0, out_peak: 0.0, in_clipped: false };
        assert!((s.latency_ms() - 4.0).abs() < 0.01, "{}", s.latency_ms());
    }

    /// Opens the real default devices for two seconds with the output level
    /// at zero (so nothing is heard and nothing can feed back) and prints
    /// what the system actually granted.
    /// `cargo test -p daw-core monitor_smoke -- --ignored --nocapture`
    #[test]
    #[ignore = "needs audio devices"]
    fn monitor_smoke_reports_granted_buffers() {
        for buffer_frames in [128, 256] {
            // DAW_MONITOR_SOURCE / DAW_MONITOR_SINK: sound-server node names to test a specific interface.
            let cfg = MonitorConfig { input_device: None, input_channel: 0, source_channels: None, output_device: None, buffer_frames,
                server_source: std::env::var("DAW_MONITOR_SOURCE").ok(), server_sink: std::env::var("DAW_MONITOR_SINK").ok() };
            let silent = AmpSettings { level: 0.0, ..AmpSettings::default() };
            match MonitorStream::open(&cfg, silent, None) {
                Ok(m) => {
                    // The first second includes start-up; report the steady state after it.
                    std::thread::sleep(std::time::Duration::from_secs(4));
                    let start = m.stats();
                    std::thread::sleep(std::time::Duration::from_secs(4));
                    let s = m.stats();
                    println!("cushion settled at {} frames (max output callback {})", m.shared.cushion.load(Ordering::Relaxed),
                        m.shared.out_period_max.load(Ordering::Relaxed));
                    println!("requested {buffer_frames}: {} -> {} · {s:?} · {:.1} ms · steady 4 s: {} dropouts, {} frames trimmed",
                        m.input_name, m.output_name, s.latency_ms(), s.underruns - start.underruns, s.dropped_frames - start.dropped_frames);
                    assert!(s.in_period > 0 && s.out_period > 0, "both callbacks ran");
                }
                Err(e) => println!("requested {buffer_frames}: could not open: {e}"),
            }
        }
    }
}

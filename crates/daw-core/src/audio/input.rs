//! Audio input capture, independent of the output `AudioEngine`.
//!
//! `CaptureStream` owns its own cpal input stream; the RT callback only pushes
//! into a lock-free ring and updates atomics. `run_writer` (a normal thread)
//! drains the ring and streams takes to WAV. `RecordingSession` and
//! `take_placement` are pure logic, testable without hardware.

use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{Receiver, Sender};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::HeapRb;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Transport states shared between the sequencer, the capture callback, and the UI.
pub const TRANSPORT_STOPPED: u8 = 0;
pub const TRANSPORT_COUNT_IN: u8 = 1;
pub const TRANSPORT_ROLLING: u8 = 2;

/// Which channel(s) of the interleaved input to keep.
/// Serializes as `{"mono": 0}` / `{"stereo": [0, 1]}` (config file format).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputChannels {
    Mono(u16),
    Stereo(u16, u16),
}

impl InputChannels {
    pub fn out_channels(&self) -> u16 {
        match self {
            InputChannels::Mono(_) => 1,
            InputChannels::Stereo(..) => 2,
        }
    }

    /// Clamp channel indices into a device's channel count.
    pub fn clamped(self, dev_channels: u16) -> InputChannels {
        let max = dev_channels.max(1) - 1;
        match self {
            InputChannels::Mono(c) => InputChannels::Mono(c.min(max)),
            InputChannels::Stereo(l, r) => InputChannels::Stereo(l.min(max), r.min(max)),
        }
    }
}

/// Extract the selected channel(s) from an interleaved buffer. The result is
/// interleaved at `sel.out_channels()` (mono = 1, stereo = 2).
pub fn extract_channels(interleaved: &[f32], in_channels: u16, sel: InputChannels) -> Vec<f32> {
    let ch = in_channels.max(1) as usize;
    let frames = interleaved.len() / ch;
    match sel.clamped(in_channels) {
        InputChannels::Mono(c) => {
            let c = c as usize;
            (0..frames).map(|f| interleaved[f * ch + c]).collect()
        }
        InputChannels::Stereo(l, r) => {
            let (l, r) = (l as usize, r as usize);
            let mut out = Vec::with_capacity(frames * 2);
            for f in 0..frames {
                out.push(interleaved[f * ch + l]);
                out.push(interleaved[f * ch + r]);
            }
            out
        }
    }
}

/// UTC timestamp `YYYYMMDD-HHMMSS` for take filenames (no chrono dependency).
/// Uses Howard Hinnant's civil_from_days algorithm, shifted to the 1970 epoch.
pub fn utc_timestamp(unix_secs: u64) -> String {
    let days = unix_secs / 86_400;
    let secs = unix_secs % 86_400;
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
}

/// Compute where a finished take lands on the timeline, using the existing
/// `AudioClip` crop field for latency compensation: the audio performed on the
/// beat arrives `latency_trim_ms` late in the file, so the clip starts at the
/// roll position and crops that much off the head.
///
/// Returns `(start_sec, offset_sec, len_sec)`, or `None` when the take is too
/// short to be a clip (shorter than `MIN_CLIP_SEC` after the crop).
pub fn take_placement(
    roll_tick: u32,
    bpm: f64,
    latency_trim_ms: f32,
    recorded_sec: f64,
) -> Option<(f64, f64, f64)> {
    let spt = 60.0 / bpm.max(1.0) / crate::sequencer::TICKS_PER_BEAT as f64;
    let start_sec = roll_tick as f64 * spt;
    let offset_sec = (latency_trim_ms.max(0.0) as f64) / 1000.0;
    let len_sec = recorded_sec - offset_sec;
    if len_sec < crate::project::MIN_CLIP_SEC {
        return None;
    }
    Some((start_sec, offset_sec, len_sec))
}

/// UI-side phase of the audio-record flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordPhase {
    #[default]
    Idle,
    /// Capture stream open, meter live, no take yet.
    Armed,
    /// Take started, waiting for the transport to roll (count-in).
    AwaitingRoll,
    Recording,
}

/// Tracks one arm→record→finish cycle and computes the clip placement.
#[derive(Debug, Default)]
pub struct RecordingSession {
    pub phase: RecordPhase,
    pub roll_tick: u32,
    /// Completed loop passes this session (status line).
    pub pass: u32,
    /// Index of the previous pass's clip in `project.audio_clips`
    /// (silenced when the next pass lands).
    pub last_clip: Option<usize>,
}

impl RecordingSession {
    pub fn arm(&mut self) {
        self.phase = RecordPhase::Armed;
    }
    pub fn disarm(&mut self) {
        *self = Self::default();
    }
    pub fn begin(&mut self) {
        self.phase = RecordPhase::AwaitingRoll;
        self.pass = 0;
        self.last_clip = None;
    }
    /// The transport started rolling at `tick`.
    pub fn rolled(&mut self, tick: u32) {
        self.roll_tick = tick;
        self.phase = RecordPhase::Recording;
    }
    /// A take finished; back to Armed. `start_tick` is the take's own gate
    /// window start (per loop pass). Returns the clip placement (or `None`
    /// for a too-short take).
    pub fn finish(&mut self, start_tick: u32, bpm: f64, latency_trim_ms: f32, recorded_sec: f64) -> Option<(f64, f64, f64)> {
        let placement = take_placement(start_tick, bpm, latency_trim_ms, recorded_sec);
        self.phase = RecordPhase::Armed;
        placement
    }
}

// ── Capture shared state + take writer ───────────────────────────────────────

/// Lock-free state shared between the capture callback, the writer thread,
/// and the UI. Everything here is written from the RT callback with atomics.
pub struct CaptureShared {
    /// Total frames the callback has seen since the stream opened.
    pub frames_seen: AtomicU64,
    /// Peak of the selected channels over the last buffer (f32 bits).
    pub peak: AtomicU32,
    /// Latched when any selected sample reaches full scale; UI resets it.
    pub clipped: AtomicBool,
    /// Frames lost because the ring was full (diagnostics only).
    pub dropped_frames: AtomicU64,
}

impl CaptureShared {
    pub fn new() -> Self {
        CaptureShared {
            frames_seen: AtomicU64::new(0),
            peak: AtomicU32::new(0f32.to_bits()),
            clipped: AtomicBool::new(false),
            dropped_frames: AtomicU64::new(0),
        }
    }
}

impl Default for CaptureShared {
    fn default() -> Self {
        Self::new()
    }
}

pub enum WriterCmd {
    /// Begin a recording session: takes are written as `<base>-p<N>.wav`,
    /// one per take-gate open window.
    StartSession { base: PathBuf },
    /// End the session (finalizes any open take).
    StopSession,
    /// Drain the ring, finalize any open take, and exit the thread.
    Shutdown,
}

#[derive(Debug)]
pub struct TakeResult {
    pub path: PathBuf,
    pub duration_sec: f64,
    pub dropped_frames: u64,
    /// Playhead tick where this take's gate window opened (placement).
    pub start_tick: u32,
}

/// An open take file: WAV writer, destination path, frames written so far.
type OpenTake = (hound::WavWriter<std::io::BufWriter<std::fs::File>>, PathBuf, u64);

/// Drain up to `buf.len()` samples (whole frames only) from the ring, append
/// the kept span of the active take to its WAV. Returns false when the ring
/// was empty.
#[allow(clippy::too_many_arguments)]
fn drain_ring(
    consumer: &mut ringbuf::HeapCons<f32>,
    frames_consumed: &mut u64,
    take: &mut Option<OpenTake>,
    buf: &mut [f32],
    in_channels: u16,
    selection: InputChannels,
) -> bool {
    let ch = in_channels.max(1) as usize;
    let avail = consumer.occupied_len() / ch * ch;
    if avail == 0 {
        return false;
    }
    let n = avail.min(buf.len());
    let got = consumer.pop_slice(&mut buf[..n]);
    let frames = got / ch;
    if let Some((wav, _, written)) = take.as_mut() {
        for s in extract_channels(&buf[..frames * ch], in_channels, selection) {
            let _ = wav.write_sample(s);
        }
        *written += frames as u64;
    }
    *frames_consumed += frames as u64;
    true
}

/// Finalize an open take (deleting zero-frame files) and report it.
fn finalize_take(
    take: Option<OpenTake>,
    shared: &CaptureShared,
    sample_rate: u32,
    start_tick: u32,
    out_tx: &Sender<TakeResult>,
) {
    if let Some((wav, path, written)) = take {
        let _ = wav.finalize();
        if written == 0 {
            let _ = std::fs::remove_file(&path);
            return;
        }
        let _ = out_tx.send(TakeResult {
            path,
            duration_sec: written as f64 / sample_rate.max(1) as f64,
            dropped_frames: shared.dropped_frames.load(Ordering::Relaxed),
            start_tick,
        });
    }
}

/// Writer-thread body. Drains the ring continuously (discarding when no take
/// is active so the ring never backs up) and streams active takes to WAV.
/// Runs until `WriterCmd::Shutdown`.
pub fn run_writer(
    mut consumer: ringbuf::HeapCons<f32>,
    shared: Arc<CaptureShared>,
    in_channels: u16,
    selection: InputChannels,
    sample_rate: u32,
    take_gate: Arc<AtomicU32>,
    roll_start_tick: Arc<AtomicU32>,
    ctrl_rx: Receiver<WriterCmd>,
    out_tx: Sender<TakeResult>,
) {
    let ch = in_channels.max(1) as usize;
    let mut frames_consumed: u64 = 0;
    let mut take: Option<OpenTake> = None;
    let mut buf = vec![0f32; 4096 * ch];
    let mut session: Option<PathBuf> = None;
    let mut pass: u32 = 0;
    let mut take_tick: u32 = 0;
    // Sentinel: the first iteration always reconciles against the live gate,
    // so an open that lands before this thread's first check isn't swallowed.
    let mut last_gate: u32 = u32::MAX;

    let spec = hound::WavSpec {
        channels: selection.out_channels(),
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    loop {
        match ctrl_rx.try_recv() {
            Ok(WriterCmd::StartSession { base }) => {
                // Defensive: close a stale take before starting a session.
                finalize_take(take.take(), &shared, sample_rate, take_tick, &out_tx);
                session = Some(base);
                pass = 0;
            }
            Ok(WriterCmd::StopSession) => {
                while drain_ring(&mut consumer, &mut frames_consumed, &mut take, &mut buf, in_channels, selection) {}
                finalize_take(take.take(), &shared, sample_rate, take_tick, &out_tx);
                session = None;
            }
            Ok(WriterCmd::Shutdown) => {
                while drain_ring(&mut consumer, &mut frames_consumed, &mut take, &mut buf, in_channels, selection) {}
                finalize_take(take.take(), &shared, sample_rate, take_tick, &out_tx);
                return;
            }
            Err(_) => {}
        }

        // Gate transitions: a close finalizes the pass's WAV; an open (while
        // a session is active) starts the next pass file.
        let gate = take_gate.load(Ordering::Acquire);
        if gate != last_gate {
            last_gate = gate;
            if gate % 2 == 0 {
                while drain_ring(&mut consumer, &mut frames_consumed, &mut take, &mut buf, in_channels, selection) {}
                finalize_take(take.take(), &shared, sample_rate, take_tick, &out_tx);
            } else if let Some(base) = &session {
                finalize_take(take.take(), &shared, sample_rate, take_tick, &out_tx);
                pass += 1;
                take_tick = roll_start_tick.load(Ordering::Acquire);
                let path = PathBuf::from(format!("{}-p{pass}.wav", base.display()));
                match hound::WavWriter::create(&path, spec) {
                    Ok(w) => take = Some((w, path, 0)),
                    Err(e) => eprintln!("cannot create take {}: {e}", path.display()),
                }
            }
        }

        if !drain_ring(&mut consumer, &mut frames_consumed, &mut take, &mut buf, in_channels, selection) {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}

// ── Device enumeration + capture stream ──────────────────────────────────────

#[derive(Debug, Clone)]
pub struct InputDeviceInfo {
    pub name: String,
    pub channels: u16,
    pub sample_rate: u32,
}

/// True for ALSA PCM aliases that only add noise to a device picker
/// (`surround40:CARD=…`, `dsnoop:…`, rate converters, …). The useful entries
/// are the servers (`pipewire`, `pulse`, `default`) and the per-card
/// `hw:`/`plughw:` devices — e.g. `hw:CARD=U192k` is the UMC404HD's raw
/// 4-channel device.
fn is_alsa_alias_noise(name: &str) -> bool {
    const NOISE: [&str; 10] = [
        "surround", "front:", "rear:", "center_lfe:", "side:", "dsnoop:",
        "sysdefault:", "samplerate:", "speexrate:", "lavrate:",
    ];
    NOISE.iter().any(|p| name.starts_with(p)) || name == "null"
}

/// Enumerate usable input devices (for the settings dropdown).
pub fn list_input_devices() -> Vec<InputDeviceInfo> {
    let host = cpal::default_host();
    let mut out = Vec::new();
    if let Ok(devices) = host.input_devices() {
        for d in devices {
            let Ok(name) = d.name() else { continue };
            if is_alsa_alias_noise(&name) {
                continue;
            }
            let Ok(cfg) = d.default_input_config() else { continue };
            out.push(InputDeviceInfo {
                name,
                channels: cfg.channels(),
                sample_rate: cfg.sample_rate().0,
            });
        }
    }
    out
}

/// An open input stream + its writer thread. Dropping it closes the stream
/// and joins the writer (finalizing any open take).
pub struct CaptureStream {
    _stream: cpal::Stream,
    pub shared: Arc<CaptureShared>,
    /// Resolved device name (may differ from the requested one on fallback).
    pub device_name: String,
    pub channels: u16,
    pub sample_rate: u32,
    pub selection: InputChannels,
    ctrl_tx: Sender<WriterCmd>,
    pub take_rx: Receiver<TakeResult>,
    writer_handle: Option<std::thread::JoinHandle<()>>,
}

/// Build the input stream for native sample type `T`, converting to f32 in
/// the RT callback (into a reusable scratch buffer — no steady-state
/// allocation) before the shared ring/peak/latch logic.
fn build_capture_stream<T>(
    device: &cpal::Device,
    stream_config: &cpal::StreamConfig,
    channels: u16,
    selection: InputChannels,
    take_gate: Arc<AtomicU32>,
    cb_shared: Arc<CaptureShared>,
    mut producer: ringbuf::HeapProd<f32>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    use cpal::Sample;
    let ch = channels.max(1) as usize;
    let mut scratch: Vec<f32> = Vec::with_capacity(65_536);
    let stream = device.build_input_stream(
        stream_config,
        move |raw: &[T], _| {
            scratch.clear();
            scratch.extend(raw.iter().map(|&s| f32::from_sample(s)));
            let data: &[f32] = &scratch;

            // Peak + clip latch over the selected channels only.
            let mut peak = 0f32;
            match selection {
                InputChannels::Mono(c) => {
                    let c = c as usize;
                    for f in data.chunks_exact(ch) {
                        peak = peak.max(f[c].abs());
                    }
                }
                InputChannels::Stereo(l, r) => {
                    let (l, r) = (l as usize, r as usize);
                    for f in data.chunks_exact(ch) {
                        peak = peak.max(f[l].abs()).max(f[r].abs());
                    }
                }
            }
            cb_shared.peak.store(peak.to_bits(), Ordering::Relaxed);
            if peak >= 0.999 {
                cb_shared.clipped.store(true, Ordering::Relaxed);
            }

            // The ring only carries gated (recording) samples; the meter
            // above runs unconditionally.
            if take_gate.load(Ordering::Acquire) % 2 == 1 {
                let pushed = producer.push_slice(data);
                if pushed < data.len() {
                    cb_shared
                        .dropped_frames
                        .fetch_add(((data.len() - pushed) / ch) as u64, Ordering::Relaxed);
                }
            }
            cb_shared.frames_seen.fetch_add((data.len() / ch) as u64, Ordering::Release);
        },
        |err| eprintln!("input stream error: {err}"),
        None,
    )?;
    Ok(stream)
}

impl CaptureStream {
    /// Open `device_name` (fallback: default input device) with its default
    /// config. `transport_state` is the sequencer's shared atomic; the take
    /// start latches on the first buffer after it reads `TRANSPORT_ROLLING`.
    pub fn open(
        device_name: Option<&str>,
        selection: InputChannels,
        take_gate: Arc<AtomicU32>,
        roll_start_tick: Arc<AtomicU32>,
    ) -> Result<CaptureStream> {
        let host = cpal::default_host();
        let device = match device_name {
            Some(want) => host
                .input_devices()
                .ok()
                .and_then(|mut it| it.find(|d| d.name().map(|n| n == want).unwrap_or(false))),
            None => None,
        }
        .or_else(|| host.default_input_device())
        .ok_or_else(|| anyhow::anyhow!("no input device available"))?;

        let resolved = device.name().unwrap_or_default();
        let config = device.default_input_config()?;
        let channels = config.channels();
        let sample_rate = config.sample_rate().0;
        let selection = selection.clamped(channels);

        let shared = Arc::new(CaptureShared::new());
        // 8 seconds of headroom so a stalled writer never costs samples.
        let rb = HeapRb::<f32>::new(sample_rate as usize * channels as usize * 8);
        let (producer, consumer) = rb.split();

        // Open the stream in the device's *native* sample format and convert
        // to f32 in the callback. Requesting f32 unconditionally makes raw
        // ALSA `hw:` devices (e.g. the UMC404HD, S32_LE only) fail with
        // EINVAL in snd_pcm_hw_params_set_format; only plug layers convert.
        let sample_format = config.sample_format();
        let stream_config: cpal::StreamConfig = config.into();
        let stream = match sample_format {
            cpal::SampleFormat::F32 => build_capture_stream::<f32>(&device, &stream_config, channels, selection, Arc::clone(&take_gate), Arc::clone(&shared), producer),
            cpal::SampleFormat::I32 => build_capture_stream::<i32>(&device, &stream_config, channels, selection, Arc::clone(&take_gate), Arc::clone(&shared), producer),
            cpal::SampleFormat::I16 => build_capture_stream::<i16>(&device, &stream_config, channels, selection, Arc::clone(&take_gate), Arc::clone(&shared), producer),
            cpal::SampleFormat::U16 => build_capture_stream::<u16>(&device, &stream_config, channels, selection, Arc::clone(&take_gate), Arc::clone(&shared), producer),
            cpal::SampleFormat::I8  => build_capture_stream::<i8>(&device, &stream_config, channels, selection, Arc::clone(&take_gate), Arc::clone(&shared), producer),
            cpal::SampleFormat::U8  => build_capture_stream::<u8>(&device, &stream_config, channels, selection, Arc::clone(&take_gate), Arc::clone(&shared), producer),
            cpal::SampleFormat::F64 => build_capture_stream::<f64>(&device, &stream_config, channels, selection, Arc::clone(&take_gate), Arc::clone(&shared), producer),
            other => anyhow::bail!("unsupported input sample format {other:?}"),
        }?;
        stream.play()?;

        let (ctrl_tx, ctrl_rx) = crossbeam_channel::unbounded();
        let (take_tx, take_rx) = crossbeam_channel::unbounded();
        let w_shared = Arc::clone(&shared);
        let w_gate = Arc::clone(&take_gate);
        let writer_handle = std::thread::spawn(move || {
            run_writer(consumer, w_shared, channels, selection, sample_rate, w_gate, roll_start_tick, ctrl_rx, take_tx)
        });

        Ok(CaptureStream {
            _stream: stream,
            shared,
            device_name: resolved,
            channels,
            sample_rate,
            selection,
            ctrl_tx,
            take_rx,
            writer_handle: Some(writer_handle),
        })
    }

    /// Begin a recording session; takes split per take-gate open window,
    /// written as `<base>-p<N>.wav`.
    pub fn begin_session(&self, base: PathBuf) {
        let _ = self.ctrl_tx.send(WriterCmd::StartSession { base });
    }

    /// End the session; any open take finalizes into a `TakeResult`.
    pub fn end_session(&self) {
        let _ = self.ctrl_tx.send(WriterCmd::StopSession);
    }

    /// `(peak 0..=1, clip latch)` for the meter.
    pub fn meter(&self) -> (f32, bool) {
        (
            f32::from_bits(self.shared.peak.load(Ordering::Relaxed)),
            self.shared.clipped.load(Ordering::Relaxed),
        )
    }

    pub fn reset_clip_latch(&self) {
        self.shared.clipped.store(false, Ordering::Relaxed);
    }
}

impl Drop for CaptureStream {
    fn drop(&mut self) {
        let _ = self.ctrl_tx.send(WriterCmd::Shutdown);
        if let Some(h) = self.writer_handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_mono_channel_from_4ch() {
        // frame i: ch1 = i, other channels are -1 markers.
        let mut data = Vec::new();
        for i in 0..5 {
            for c in 0..4 {
                data.push(if c == 1 { i as f32 } else { -1.0 });
            }
        }
        let out = extract_channels(&data, 4, InputChannels::Mono(1));
        assert_eq!(out, vec![0.0, 1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn extract_stereo_pair_from_4ch() {
        let mut data = Vec::new();
        for i in 0..3 {
            for c in 0..4 {
                data.push((i * 10 + c) as f32);
            }
        }
        let out = extract_channels(&data, 4, InputChannels::Stereo(2, 3));
        assert_eq!(out, vec![2.0, 3.0, 12.0, 13.0, 22.0, 23.0]);
    }

    #[test]
    fn extract_clamps_out_of_range_channel() {
        let data = vec![1.0, 2.0, 3.0, 4.0]; // 2 frames, 2 channels
        let out = extract_channels(&data, 2, InputChannels::Mono(7));
        assert_eq!(out, vec![2.0, 4.0]); // clamped to last channel
    }

    #[test]
    fn input_channels_serde_format() {
        assert_eq!(serde_json::to_string(&InputChannels::Mono(0)).unwrap(), r#"{"mono":0}"#);
        assert_eq!(serde_json::to_string(&InputChannels::Stereo(0, 1)).unwrap(), r#"{"stereo":[0,1]}"#);
        let m: InputChannels = serde_json::from_str(r#"{"mono":2}"#).unwrap();
        assert_eq!(m, InputChannels::Mono(2));
    }

    #[test]
    fn utc_timestamp_known_values() {
        assert_eq!(utc_timestamp(0), "19700101-000000");
        // 2023-11-14 22:13:20 UTC
        assert_eq!(utc_timestamp(1_700_000_000), "20231114-221320");
    }

    #[test]
    fn take_placement_maths() {
        // 480 ticks @ 120 bpm = 4 beats * 0.5 s = 2.0 s.
        let (start, off, len) = take_placement(480, 120.0, 12.0, 3.0).unwrap();
        assert!((start - 2.0).abs() < 1e-9);
        assert!((off - 0.012).abs() < 1e-9);
        assert!((len - 2.988).abs() < 1e-9);
    }

    #[test]
    fn take_placement_rejects_too_short() {
        assert!(take_placement(0, 120.0, 0.0, 0.005).is_none());
        // recorded shorter than the latency crop
        assert!(take_placement(0, 120.0, 50.0, 0.04).is_none());
    }

    #[test]
    fn alsa_alias_noise_filter() {
        assert!(is_alsa_alias_noise("surround40:CARD=U192k,DEV=0"));
        assert!(is_alsa_alias_noise("dsnoop:CARD=Generic_1,DEV=0"));
        assert!(is_alsa_alias_noise("sysdefault:CARD=Generic_1"));
        assert!(is_alsa_alias_noise("null"));
        assert!(!is_alsa_alias_noise("pipewire"));
        assert!(!is_alsa_alias_noise("default"));
        assert!(!is_alsa_alias_noise("hw:CARD=U192k,DEV=0"));
        assert!(!is_alsa_alias_noise("plughw:CARD=U192k,DEV=0"));
    }

    #[test]
    #[ignore] // needs an input device; run with -- --ignored
    fn smoke_capture_stream_sees_frames() {
        // DAW_INPUT_DEVICE selects a specific device (e.g. "hw:CARD=U192k,DEV=0"
        // to exercise a raw integer-format ALSA device); default input otherwise.
        let want = std::env::var("DAW_INPUT_DEVICE").ok();
        let gate = Arc::new(AtomicU32::new(0));
        let roll = Arc::new(AtomicU32::new(0));
        let cs = CaptureStream::open(want.as_deref(), InputChannels::Mono(0), gate, roll).expect("open input");
        eprintln!("capturing from '{}' ({} ch @ {} Hz)", cs.device_name, cs.channels, cs.sample_rate);
        std::thread::sleep(std::time::Duration::from_secs(1));
        assert!(cs.shared.frames_seen.load(Ordering::Relaxed) > 0, "no frames captured");
    }

    #[test]
    fn writer_splits_takes_per_gate_window() {
        let dir = std::env::temp_dir().join(format!("daw_gate_writer_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("take");

        let rb = ringbuf::HeapRb::<f32>::new(48_000);
        let (mut prod, cons) = rb.split();
        let shared = Arc::new(CaptureShared::new());
        let gate = Arc::new(AtomicU32::new(0));
        let roll_tick = Arc::new(AtomicU32::new(0));
        let (ctrl_tx, ctrl_rx) = crossbeam_channel::unbounded();
        let (take_tx, take_rx) = crossbeam_channel::unbounded();

        let g = Arc::clone(&gate);
        let rt = Arc::clone(&roll_tick);
        let sh = Arc::clone(&shared);
        let h = std::thread::spawn(move || {
            run_writer(cons, sh, 1, InputChannels::Mono(0), 48_000, g, rt, ctrl_rx, take_tx)
        });

        ctrl_tx.send(WriterCmd::StartSession { base: base.clone() }).unwrap();
        // Pass 1: open at tick 480, 100 frames, close.
        roll_tick.store(480, Ordering::Release);
        gate.fetch_add(1, Ordering::Release); // open (1)
        prod.push_slice(&[0.5f32; 100]);
        std::thread::sleep(std::time::Duration::from_millis(40));
        gate.fetch_add(1, Ordering::Release); // close (2)
        std::thread::sleep(std::time::Duration::from_millis(40));
        // Pass 2: open at tick 960, 50 frames, close.
        roll_tick.store(960, Ordering::Release);
        gate.fetch_add(1, Ordering::Release); // open (3)
        prod.push_slice(&[0.25f32; 50]);
        std::thread::sleep(std::time::Duration::from_millis(40));
        gate.fetch_add(1, Ordering::Release); // close (4)
        std::thread::sleep(std::time::Duration::from_millis(40));
        ctrl_tx.send(WriterCmd::Shutdown).unwrap();
        h.join().unwrap();

        let takes: Vec<TakeResult> = take_rx.try_iter().collect();
        assert_eq!(takes.len(), 2, "one TakeResult per gate window, got {}", takes.len());
        assert_eq!(takes[0].start_tick, 480);
        assert_eq!(takes[1].start_tick, 960);
        assert!(takes[0].path.to_string_lossy().contains("-p1"));
        assert!(takes[1].path.to_string_lossy().contains("-p2"));
        assert!((takes[0].duration_sec - 100.0 / 48_000.0).abs() < 1e-6);
        assert!((takes[1].duration_sec - 50.0 / 48_000.0).abs() < 1e-6);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn writer_extracts_selected_channel() {
        // 100 frames of 4-channel ring data; ch1 carries the frame index.
        // (The callback gates what enters the ring, so the writer keeps all.)
        let rb = ringbuf::HeapRb::<f32>::new(4096);
        let (mut prod, cons) = rb.split();
        let mut data = Vec::new();
        for i in 0..100 {
            for c in 0..4 {
                data.push(if c == 1 { i as f32 } else { -1.0 });
            }
        }
        assert_eq!(prod.push_slice(&data), data.len());

        let dir = std::env::temp_dir().join(format!("daw_writer_ch_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("take");

        let shared = Arc::new(CaptureShared::new());
        let gate = Arc::new(AtomicU32::new(1)); // already open
        let roll_tick = Arc::new(AtomicU32::new(0));
        let (ctrl_tx, ctrl_rx) = crossbeam_channel::unbounded();
        let (take_tx, take_rx) = crossbeam_channel::unbounded();
        ctrl_tx.send(WriterCmd::StartSession { base: base.clone() }).unwrap();
        let g2 = Arc::clone(&gate);
        let h = std::thread::spawn(move || {
            run_writer(cons, shared, 4, InputChannels::Mono(1), 48_000, gate, roll_tick, ctrl_rx, take_tx)
        });
        std::thread::sleep(std::time::Duration::from_millis(40));
        g2.fetch_add(1, Ordering::Release); // close -> finalize
        std::thread::sleep(std::time::Duration::from_millis(40));
        ctrl_tx.send(WriterCmd::Shutdown).unwrap();
        h.join().unwrap();

        let result = take_rx.try_recv().expect("a TakeResult");
        let mut reader = hound::WavReader::open(&result.path).unwrap();
        assert_eq!(reader.spec().channels, 1);
        let samples: Vec<f32> = reader.samples::<f32>().map(|s| s.unwrap()).collect();
        let expected: Vec<f32> = (0..100).map(|i| i as f32).collect();
        assert_eq!(samples, expected);
        std::fs::remove_dir_all(std::path::Path::new(&result.path).parent().unwrap()).ok();
    }

    #[test]
    fn writer_zero_frame_take_deletes_file_and_stays_silent() {
        let rb = ringbuf::HeapRb::<f32>::new(64);
        let (_prod, cons) = rb.split();
        let shared = Arc::new(CaptureShared::new());
        let gate = Arc::new(AtomicU32::new(0));
        let roll_tick = Arc::new(AtomicU32::new(0));
        let dir = std::env::temp_dir().join(format!("daw_writer_empty_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("take");

        let (ctrl_tx, ctrl_rx) = crossbeam_channel::unbounded();
        let (take_tx, take_rx) = crossbeam_channel::unbounded();
        ctrl_tx.send(WriterCmd::StartSession { base: base.clone() }).unwrap();
        let g2 = Arc::clone(&gate);
        let h = std::thread::spawn(move || {
            run_writer(cons, shared, 2, InputChannels::Mono(0), 48_000, gate, roll_tick, ctrl_rx, take_tx)
        });
        std::thread::sleep(std::time::Duration::from_millis(20));
        g2.fetch_add(2, Ordering::Release); // open+close with nothing pushed
        std::thread::sleep(std::time::Duration::from_millis(20));
        ctrl_tx.send(WriterCmd::Shutdown).unwrap();
        h.join().unwrap();

        assert!(take_rx.try_recv().is_err(), "zero-frame takes emit no TakeResult");
        assert!(!dir.join("take-p1.wav").exists(), "zero-frame take file deleted");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn session_phases_and_finish() {
        let mut s = RecordingSession::default();
        assert_eq!(s.phase, RecordPhase::Idle);
        s.arm();
        assert_eq!(s.phase, RecordPhase::Armed);
        s.begin();
        assert_eq!(s.phase, RecordPhase::AwaitingRoll);
        s.rolled(960);
        assert_eq!(s.phase, RecordPhase::Recording);
        let (start, _, _) = s.finish(960, 120.0, 0.0, 1.0).unwrap();
        assert!((start - 4.0).abs() < 1e-9); // 960 ticks @ 120 bpm = 4 s
        assert_eq!(s.phase, RecordPhase::Armed);
        s.disarm();
        assert_eq!(s.phase, RecordPhase::Idle);
    }
}

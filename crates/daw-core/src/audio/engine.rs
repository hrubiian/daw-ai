pub static LATE_AUDIO_EVENTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static CALLBACK_OVERRUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static CALLBACK_MAX_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static AUDIO_STREAM_ERRORS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AudioDiagnostics {
    pub dropped_events: u64,
    #[serde(default)]
    pub late_events: u64,
    #[serde(default)]
    pub dropped_plugin_events: u64,
    #[serde(default)]
    pub plugin_process_errors: u64,
    #[serde(default)]
    pub sampler_voice_steals: u64,
    pub callback_overruns: u64,
    pub callback_max_ns: u64,
    pub stream_errors: u64,
}
fn enqueue(producer: &mut ringbuf::HeapProd<AudioEvent>, event: AudioEvent) -> bool {
    if producer.try_push(event).is_err() {
        DROPPED_AUDIO_EVENTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        false
    } else { true }
}
pub fn diagnostics() -> AudioDiagnostics {
    use std::sync::atomic::Ordering::Relaxed;
    AudioDiagnostics { sampler_voice_steals: super::mixer::SAMPLER_VOICE_STEALS.load(Relaxed), dropped_plugin_events: super::clap::DROPPED_CLAP_EVENTS.load(Relaxed), plugin_process_errors: super::clap::CLAP_PROCESS_ERRORS.load(Relaxed), late_events: LATE_AUDIO_EVENTS.load(Relaxed), dropped_events: DROPPED_AUDIO_EVENTS.load(Relaxed), callback_overruns: CALLBACK_OVERRUNS.load(Relaxed), callback_max_ns: CALLBACK_MAX_NS.load(Relaxed), stream_errors: AUDIO_STREAM_ERRORS.load(Relaxed) }
}
pub static DROPPED_AUDIO_EVENTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
use super::mixer::{AudioEvent, InsertChain, Mixer, StripMeter};
use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::traits::{Consumer, Producer, Split, Observer};
use ringbuf::HeapRb;
use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};
use std::path::Path;
use std::sync::{Arc, Mutex};

// ── Public recording state ────────────────────────────────────────────────────

pub struct RecordState {
    pub available:   bool,
    pub active:      bool,
    pub samples:     Vec<f32>,
    pub sample_rate: u32,
    pub channels:    u16,
}

impl Default for RecordState {
    fn default() -> Self {
        RecordState { available: false, active: false, samples: vec![], sample_rate: 0, channels: 0 }
    }
}

#[derive(Clone)]
pub struct RecordController(Arc<Mutex<RecordState>>);

impl RecordController {
    pub fn new(state: Arc<Mutex<RecordState>>) -> Self { RecordController(state) }
    pub fn available(&self) -> bool   { self.0.lock().map(|s| s.available).unwrap_or(false) }
    pub fn is_active(&self) -> bool   { self.0.lock().map(|s| s.active).unwrap_or(false) }

    pub fn start(&self) {
        if let Ok(mut s) = self.0.lock() {
            if s.available {
                s.samples.clear();
                s.active = true;
            }
        }
    }

    pub fn stop(&self, path: &Path) -> Result<()> {
        let (samples, sample_rate, channels) = {
            let mut s = self.0.lock()
                .map_err(|_| anyhow::anyhow!("record mutex poisoned"))?;
            s.active = false;
            (std::mem::take(&mut s.samples), s.sample_rate, s.channels)
        };
        if samples.is_empty() { return Ok(()); }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        write_wav(path, &samples, sample_rate, channels)
    }
}

pub fn write_wav(path: &Path, samples: &[f32], sample_rate: u32, channels: u16) -> Result<()> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &s in samples { writer.write_sample(s)?; }
    writer.finalize()?;
    Ok(())
}

// ── WAV loading helper (used by relay thread) ─────────────────────────────────

pub fn load_wav_stereo(path: &Path, target_sr: u32) -> Result<(Vec<f32>, Vec<f32>)> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let source_sr = spec.sample_rate;
    let ch = spec.channels as usize;

    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect(),
        hound::SampleFormat::Int => {
            if spec.bits_per_sample <= 16 {
                reader.samples::<i16>().map(|s| s.unwrap_or(0) as f32 / 32768.0).collect()
            } else {
                reader.samples::<i32>().map(|s| s.unwrap_or(0) as f32 / 2_147_483_648.0).collect()
            }
        }
    };

    let frames = raw.len() / ch.max(1);
    let left:  Vec<f32> = (0..frames).map(|i| raw[i * ch]).collect();
    let right: Vec<f32> = if ch >= 2 {
        (0..frames).map(|i| raw[i * ch + 1]).collect()
    } else {
        left.clone()
    };

    if source_sr == target_sr || target_sr == 0 {
        return Ok((left, right));
    }

    let ratio = source_sr as f64 / target_sr as f64;
    let out_frames = ((frames as f64 / ratio).ceil() as usize).max(1);
    let resample = |src: &[f32]| -> Vec<f32> {
        (0..out_frames).map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            let a = src.get(idx).copied().unwrap_or(0.0);
            let b = src.get(idx + 1).copied().unwrap_or(0.0);
            a + (b - a) * frac
        }).collect()
    };

    Ok((resample(&left), resample(&right)))
}

/// Convert seconds to a sample index at `sr` (rounded).
pub fn sec_to_sample(sec: f64, sr: u32) -> usize {
    (sec.max(0.0) * sr as f64).round() as usize
}

/// Duration of a WAV in seconds (frames / sample_rate), without decoding samples.
pub fn wav_duration_sec(path: &Path) -> Result<f64> {
    let reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let frames = reader.len() as f64 / spec.channels.max(1) as f64;
    Ok(frames / spec.sample_rate.max(1) as f64)
}

/// Compute `buckets` waveform peaks (max-abs magnitude, mono mix, 0.0..=1.0),
/// evenly spaced across the whole file. No resampling — this is for display.
pub fn wav_peaks(path: &Path, buckets: usize) -> Result<Vec<f32>> {
    let buckets = buckets.max(1);
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let ch = spec.channels.max(1) as usize;

    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect(),
        hound::SampleFormat::Int => {
            if spec.bits_per_sample <= 16 {
                reader.samples::<i16>().map(|s| s.unwrap_or(0) as f32 / 32768.0).collect()
            } else {
                reader.samples::<i32>().map(|s| s.unwrap_or(0) as f32 / 2_147_483_648.0).collect()
            }
        }
    };

    let frames = raw.len() / ch;
    if frames == 0 { return Ok(vec![0.0; buckets]); }

    let mut out = vec![0.0f32; buckets];
    for b in 0..buckets {
        let lo = b * frames / buckets;
        let hi = ((b + 1) * frames / buckets).max(lo + 1).min(frames);
        let mut peak = 0.0f32;
        for f in lo..hi {
            let mut acc = 0.0f32;
            for c in 0..ch { acc += raw[f * ch + c]; }
            peak = peak.max((acc / ch as f32).abs());
        }
        out[b] = peak;
    }
    Ok(out)
}

// ── Audio engine ──────────────────────────────────────────────────────────────

/// Legacy TUI preview-clip player (whole-file loop with its own FX chain).
struct ClipPlayer {
    left:     Vec<f32>,
    right:    Vec<f32>,
    position: usize,
    playing:  bool,
    volume:   f32,
}

pub struct AudioEngine {
    _output_stream: cpal::Stream,
    _input_stream:  Option<cpal::Stream>,
    producer:       ringbuf::HeapProd<AudioEvent>,
    pub schedule_at: Option<(u64, u64)>,
    pub sample_rate: u32,
    // Declared last: stream teardown stops the producer before worker shutdown.
    _retirement_lifetime: Arc<()>,
}

fn find_output_device(host: &cpal::Host) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    // Enumerating output_devices probes every ALSA PCM, including optional JACK,
    // OSS and capture-only aliases. Avoid that noise (and startup delay) when
    // the desktop's default output is already usable.
    let mut last_err = String::from("no default output device");
    if let Some(device) = host.default_output_device() {
        match device.default_output_config() {
            Ok(cfg) => return Ok((device, cfg)),
            Err(e) => last_err = format!("default output: {e}"),
        }
    }
    let devices = host.output_devices()
        .map_err(|e| anyhow::anyhow!("{last_err}; cannot enumerate fallback outputs: {e}"))?;
    for device in devices {
        let name = device.name().unwrap_or_default();
        match device.default_output_config() {
            Ok(cfg) => return Ok((device, cfg)),
            Err(e) => last_err = format!("{name}: {e}"),
        }
    }
    anyhow::bail!("no usable audio output device found ({last_err})")
}

impl AudioEngine {
    pub fn new(
        sf2_path: &Path,
        record_state: Arc<Mutex<RecordState>>,
        vis_buf: Option<Arc<Mutex<Vec<f32>>>>,
        enable_input: bool,
        master_meter: Arc<StripMeter>,
        return_meters: [Arc<StripMeter>; 2],
        clap_garbage: crossbeam_channel::Sender<(String, super::clap::ClapProcessor)>,
        clock: Arc<super::scheduling::AudioClock>,
    ) -> Result<Self> {
        let host = cpal::default_host();
        let (device, config) = find_output_device(&host)?;
        let sample_rate = config.sample_rate().0;
        clock.sample_rate.store(sample_rate, std::sync::atomic::Ordering::Release);

        // ── Utility synth (metronome/previews) on the main soundfont ─────
        let mut sf2_file = std::fs::File::open(sf2_path)
            .map_err(|e| anyhow::anyhow!("cannot open SF2 at {}: {e}", sf2_path.display()))?;
        let sound_font = Arc::new(SoundFont::new(&mut sf2_file)?);
        let settings   = SynthesizerSettings::new(sample_rate as i32);
        let utility    = Synthesizer::new(&sound_font, &settings)?;

        // ── The mixer: all strips render through it ───────────────────────
        let mut mixer = Mixer::new(sample_rate, Some(utility), master_meter, return_meters);

        // ── Ring buffer ───────────────────────────────────────────────────
        let rb = HeapRb::<AudioEvent>::new(4096);
        let (producer, mut consumer) = rb.split();
        // Resource swaps enqueue old buffers/strips for destruction on a worker.
        // If this bounded queue is full, defer event processing to the next block.
        let (mut retire_tx, mut retire_rx) = HeapRb::<AudioEvent>::new(4096).split();
        let retirement_lifetime = Arc::new(());
        let lifetime = Arc::downgrade(&retirement_lifetime);
        std::thread::spawn(move || {
            while lifetime.strong_count() > 0 || !retire_rx.is_empty() {
                while let Some(retired) = retire_rx.try_pop() { drop(retired); }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });

        // ── Render buffers ────────────────────────────────────────────────
        let max_frames = 8192usize;
        let mut left_buf  = vec![0f32; max_frames];
        let mut right_buf = vec![0f32; max_frames];

        // ── Legacy preview-clip state (TUI path; lives in the callback) ───
        let mut clip: Option<ClipPlayer> = None;
        let mut legacy_chain = InsertChain::new(sample_rate);

        // ── Output stream ─────────────────────────────────────────────────
        let stream_config: cpal::StreamConfig = config.into();
        let channels = stream_config.channels as usize;
        let vis_write = vis_buf;

        let mut scheduled = super::scheduling::EventQueue::new(4096);
        let mut device_frame = 0u64;
        let output_stream = device.build_output_stream(
            &stream_config,
            move |data: &mut [f32], _| {
                let started = std::time::Instant::now();
                for _ in 0..1024 {
                    if retire_tx.vacant_len() < 2 { break; }
                    let Some(ev) = consumer.try_pop() else { break; };
                    if let AudioEvent::Scheduled(event) = ev {
                        if let Err(event) = scheduled.push(event) {
                            DROPPED_AUDIO_EVENTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let _ = retire_tx.try_push(AudioEvent::Scheduled(event));
                        }
                    } else {
                        apply_callback_event(ev, &mut mixer, &mut clip, &mut legacy_chain, &mut retire_tx, &clap_garbage);
                    }
                }

                data.fill(0.0); // Oversized callbacks must never expose uninitialized output.
                let frames = (data.len() / channels).min(max_frames);
                clock.quantum.store((data.len() / channels) as u32, std::sync::atomic::Ordering::Relaxed);
                render_scheduled_block(&mut mixer, &mut scheduled, device_frame,
                    clock.generation.load(std::sync::atomic::Ordering::Acquire),
                    &mut left_buf[..frames], &mut right_buf[..frames],
                    &mut clip, &mut legacy_chain, &mut retire_tx, &clap_garbage);
                device_frame += (data.len() / channels) as u64;
                clock.frame.store(device_frame, std::sync::atomic::Ordering::Release);

                for (i, frame) in data.chunks_mut(channels).enumerate().take(frames) {
                    for (ch, sample) in frame.iter_mut().enumerate() {
                        *sample = if ch == 0 { left_buf[i] } else if ch == 1 { right_buf[i] } else { 0.0 };
                    }
                }

                if let Some(ref vb) = vis_write {
                    if let Ok(mut vis) = vb.try_lock() {
                        let count = frames.min(vis.len());
                        vis.rotate_left(count);
                        let offset = vis.len() - count;
                        for i in 0..count { vis[offset + i] = (left_buf[frames - count + i] + right_buf[frames - count + i]) * 0.5; }
                    }
                }
                let elapsed = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
                CALLBACK_MAX_NS.fetch_max(elapsed, std::sync::atomic::Ordering::Relaxed);
                if frames < data.len() / channels || elapsed > (data.len() / channels) as u64 * 1_000_000_000 / sample_rate as u64 {
                    CALLBACK_OVERRUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            },
            |err| {
                AUDIO_STREAM_ERRORS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                eprintln!("audio stream error: {err}");
            },
            None,
        )?;
        output_stream.play()?;

        // ── Input stream (legacy TUI recording; the GPUI frontend uses
        // daw_core::audio::input::CaptureStream instead) ───────────────────
        let input_stream = if enable_input {
            Self::try_open_input(&host, record_state)
        } else {
            None
        };

        Ok(AudioEngine {
            _output_stream: output_stream,
            _retirement_lifetime: retirement_lifetime,
            _input_stream: input_stream,
            producer,
            schedule_at: None,
            sample_rate,
        })
    }

    fn try_open_input(host: &cpal::Host, record_state: Arc<Mutex<RecordState>>) -> Option<cpal::Stream> {
        let device = host.default_input_device()?;
        let config = device.default_input_config().ok()?;
        let in_sr = config.sample_rate().0;
        let in_ch = config.channels();

        {
            let mut rs = record_state.lock().ok()?;
            rs.sample_rate = in_sr;
            rs.channels    = in_ch;
            rs.available   = true;
        }

        // Max 10 minutes of recording at the input sample rate and channel count
        let max_samples = 10 * 60 * in_sr as usize * in_ch as usize;

        let stream_config: cpal::StreamConfig = config.into();
        let stream = device.build_input_stream(
            &stream_config,
            move |data: &[f32], _| {
                if let Ok(mut rs) = record_state.try_lock() {
                    if rs.active {
                        let remaining = max_samples.saturating_sub(rs.samples.len());
                        if remaining > 0 {
                            rs.samples.extend_from_slice(&data[..data.len().min(remaining)]);
                        }
                    }
                }
            },
            |err| eprintln!("input stream error: {err}"),
            None,
        ).ok()?;
        stream.play().ok()?;
        Some(stream)
    }

    pub fn send(&mut self, event: AudioEvent) {
        let event = match event {
            AudioEvent::SetAudioClips { clips } => AudioEvent::PreparedAudioClips { clips: clips.into_iter().map(|(id, l, r)| (id, (l, r))).collect() },
            other => other,
        };
        let event = if let Some((frame, generation)) = self.schedule_at {
            AudioEvent::Scheduled(Box::new(super::scheduling::ScheduledEvent { frame, generation, event }))
        } else { event };
        enqueue(&mut self.producer, event);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #[test]
    fn queue_saturation_is_measured() {
        use super::*;
        let (mut producer, mut consumer) = HeapRb::new(1).split();
        let before = diagnostics().dropped_events;
        assert!(enqueue(&mut producer, AudioEvent::Panic));
        assert!(!enqueue(&mut producer, AudioEvent::Panic));
        assert_eq!(diagnostics().dropped_events, before + 1);
        assert!(consumer.try_pop().is_some());
        assert!(enqueue(&mut producer, AudioEvent::Panic));
    }

    use super::*;

    fn write_test_wav(path: &std::path::Path, sr: u32, frames: usize, amp: f32) {
        let spec = hound::WavSpec {
            channels: 1, sample_rate: sr,
            bits_per_sample: 32, sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for _ in 0..frames { w.write_sample(amp).unwrap(); }
        w.finalize().unwrap();
    }

    #[test]
    fn wav_duration_and_peaks() {
        let dir = std::env::temp_dir();
        let p = dir.join("daw_core_peaks_test.wav");
        write_test_wav(&p, 1000, 1000, 0.5); // 1000 frames @ 1000 Hz = 1.0s, const 0.5

        let dur = wav_duration_sec(&p).unwrap();
        assert!((dur - 1.0).abs() < 1e-6, "duration {dur}");

        let peaks = wav_peaks(&p, 10).unwrap();
        assert_eq!(peaks.len(), 10);
        for v in &peaks { assert!((v - 0.5).abs() < 1e-3, "peak {v}"); }

        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn sec_to_sample_rounds() {
        assert_eq!(sec_to_sample(1.0, 44100), 44100);
        assert_eq!(sec_to_sample(0.0, 48000), 0);
        assert_eq!(sec_to_sample(0.5, 44100), 22050);
    }

    #[test]
    #[ignore]
    fn smoke_plays_note() {
        // Per-strip routing: build one melodic strip, then play through it.
        let sf2 = Path::new("../../assets/GeneralUser.sf2");
        let state = Arc::new(Mutex::new(RecordState::default()));
        let master = Arc::new(StripMeter::new());
        let mut engine = AudioEngine::new(
            sf2, state, None, true, Arc::clone(&master),
            [Arc::new(StripMeter::new()), Arc::new(StripMeter::new())],
            crossbeam_channel::unbounded().0,
            Arc::new(super::super::scheduling::AudioClock::default()),
        )
        .expect("audio engine init failed");
        let spec = super::super::mixer::StripSpec {
            kind: super::super::mixer::StripKind::Melodic { font: None, bank: 0, preset: 0 },
            gain: 1.0, pan: 0.0, muted: false,
            effects: crate::project::EffectChain::default(),
            clap_instrument: None,
            clap_fx: Vec::new(),
            tuning_cents: 0.0,
        };
        let (strips, _meters) = super::super::mixer::build_strips(&[spec], sf2, engine.sample_rate);
        engine.send(AudioEvent::SetStrips { strips });
        engine.send(AudioEvent::NoteOn { strip: 0, pitch: 60, velocity: 100 });
        std::thread::sleep(std::time::Duration::from_secs(1));
        engine.send(AudioEvent::NoteOff { strip: 0, pitch: 60 });
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(master.read().0 >= 0.0);
    }
}

fn apply_callback_event(
    ev: AudioEvent, mixer: &mut Mixer, clip: &mut Option<ClipPlayer>,
    legacy_chain: &mut InsertChain, retire_tx: &mut ringbuf::HeapProd<AudioEvent>,
    clap_garbage: &crossbeam_channel::Sender<(String, super::clap::ClapProcessor)>,
) {
                    match ev {
                        AudioEvent::SetStrips { strips } => {
                            let old = std::mem::replace(&mut mixer.strips, strips);
                            let _ = retire_tx.try_push(AudioEvent::SetStrips { strips: old });
                        }
                        AudioEvent::PreparedAudioClips { clips } => {
                            let old = std::mem::replace(&mut mixer.audio_clips, clips);
                            let _ = retire_tx.try_push(AudioEvent::PreparedAudioClips { clips: old });
                        }
                        // Legacy preview clip (unchanged TUI semantics).
                        AudioEvent::LoadClip { left, right, volume } => {
                            let old = clip.replace(ClipPlayer { left, right, position: 0, playing: false, volume });
                            if let Some(old) = old {
                                let _ = retire_tx.try_push(AudioEvent::LoadClip { left: old.left, right: old.right, volume: old.volume });
                            }
                        }
                        AudioEvent::PlayClip => {
                            if let Some(c) = clip { c.position = 0; c.playing = true; }
                        }
                        AudioEvent::StopClip => {
                            if let Some(c) = clip { c.playing = false; }
                        }
                        AudioEvent::SetClipEffects {
                            delay_time_ms, delay_feedback, delay_mix, cutoff, drive,
                            eq_low_gain, eq_mid_gain, eq_high_gain, eq_mid_freq,
                            comp_threshold, comp_ratio, comp_attack_ms, comp_release_ms,
                        } => {
                            legacy_chain.configure_raw(
                                delay_time_ms, delay_feedback, delay_mix, cutoff, drive,
                                eq_low_gain, eq_mid_gain, eq_high_gain, eq_mid_freq,
                                comp_threshold, comp_ratio, comp_attack_ms, comp_release_ms,
                            );
                        }
                        // Retire a CLAP processor: extract it and return it
                        // to the relay for stop/deactivate off the RT thread.
                        AudioEvent::RemoveClapInstance { uid } => {
                            if let Some(p) = mixer.remove_clap(&uid) {
                                let _ = clap_garbage.try_send((uid, p));
                            }
                        }
                        // Relay-thread events never reach the callback.
                        AudioEvent::LoadClipFromFile { .. }
                        | AudioEvent::LoadStrips { .. }
                        | AudioEvent::LoadAudioClips { .. } => {}
                        // Everything else is mixer routing.
                        other => mixer.handle_event(other),
                    }
}

#[allow(clippy::too_many_arguments)]
fn render_scheduled_block(
    mixer: &mut Mixer, scheduled: &mut super::scheduling::EventQueue,
    device_frame: u64, generation: u64, left_buf: &mut [f32], right_buf: &mut [f32],
    clip: &mut Option<ClipPlayer>, legacy_chain: &mut InsertChain,
    retire_tx: &mut ringbuf::HeapProd<AudioEvent>,
    clap_garbage: &crossbeam_channel::Sender<(String, super::clap::ClapProcessor)>,
) {
    let frames = left_buf.len();
                let end_frame = device_frame + frames as u64;
                let mut cursor = device_frame;
                let mut dispatched = 0;
                while cursor < end_frame {
                    while dispatched < 1024 && retire_tx.vacant_len() >= 2 {
                        let Some(mut event) = scheduled.pop_due(cursor) else { break; };
                        if !event.cancelled(generation) {
                            if event.frame < cursor { LATE_AUDIO_EVENTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
                            let payload = std::mem::replace(&mut event.event, AudioEvent::Panic);
                            apply_callback_event(payload, mixer, clip, legacy_chain, retire_tx, clap_garbage);
                        }
                        // Deallocate the scheduling envelope (and cancelled payloads) off-thread.
                        let _ = retire_tx.try_push(AudioEvent::Scheduled(event));
                        dispatched += 1;
                    }
                    let next = if dispatched == 1024 || retire_tx.vacant_len() < 2 {
                        end_frame
                    } else { scheduled.next_frame().unwrap_or(end_frame).min(end_frame) };
                    let a = (cursor - device_frame) as usize;
                    let b = (next - device_frame) as usize;
                    mixer.render_block(&mut left_buf[a..b], &mut right_buf[a..b]);
                    if let Some(c) = clip.as_mut().filter(|c| c.playing && !c.left.is_empty()) {
                        let clip_len = c.left.len();
                        for i in a..b {
                            let pos = (c.position + i - a) % clip_len;
                            let (l, r) = legacy_chain.process(c.left[pos] * c.volume, c.right[pos] * c.volume);
                            left_buf[i] += l; right_buf[i] += r;
                        }
                        c.position = (c.position + b - a) % clip_len;
                    }
                    cursor = next;
                }

}

#[cfg(test)]
mod scheduled_playback_tests {
    use super::*;
    use crate::audio::{mixer::Strip, scheduling::{EventQueue, ScheduledEvent}};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn mixer() -> Mixer {
        let mut mixer = Mixer::new(44100, None, Arc::new(StripMeter::new()),
            [Arc::new(StripMeter::new()), Arc::new(StripMeter::new())]);
        let mut strip = Strip::new(44100);
        let mut pulse = vec![0.; 512]; pulse[0] = 0.25;
        let pulse = Arc::new(pulse);
        strip.pads.insert(36, super::super::mixer::SamplerPadAudio::new(pulse.clone(), pulse, 1.));
        mixer.strips.push(strip);
        mixer
    }
    fn event(frame: u64, generation: u64) -> Box<ScheduledEvent> {
        Box::new(ScheduledEvent { frame, generation,
            event: AudioEvent::TriggerSample { strip: 0, pitch: 36, gain: 1. } })
    }
    #[test]
    fn callback_scheduling_preserves_hits_for_different_buffer_sizes() {
        for size in [128, 257, 1024] {
            let mut mixer = mixer();
            let mut queue = EventQueue::new(16);
            for frame in [184, 368, 1286] { queue.push(event(frame, 0)).unwrap(); }
            let (mut retire, mut retired) = HeapRb::new(64).split();
            let (garbage, _) = crossbeam_channel::unbounded();
            let mut chain = InsertChain::new(44100); let mut clip = None;
            let mut output = Vec::new(); let mut pos = 0;
            while pos < 2048 {
                let frames = size.min(2048 - pos);
                let (mut l, mut r) = (vec![0.; frames], vec![0.; frames]);
                render_scheduled_block(&mut mixer, &mut queue, pos as u64, 0,
                    &mut l, &mut r, &mut clip, &mut chain, &mut retire, &garbage);
                output.extend(l); pos += frames;
                while retired.try_pop().is_some() {}
            }
            let onsets: Vec<_> = output.iter().enumerate().filter(|(_, s)| s.abs() > 0.00001).map(|(i, _)| i).collect();
            assert_eq!(onsets, [184, 368, 1286], "callback size {size}");
        }
    }
    #[test]
    fn seek_cancels_old_notes_but_keeps_revision_ack_at_its_frame() {
        let mut mixer = mixer(); let mut queue = EventQueue::new(8);
        queue.push(event(100, 0)).unwrap(); queue.push(event(368, 1)).unwrap();
        let ack = Arc::new(AtomicU64::new(0));
        queue.push(Box::new(ScheduledEvent { frame: 184, generation: 0,
            event: AudioEvent::Applied { revision: 7, ack: ack.clone() } })).unwrap();
        let (mut retire, _retired) = HeapRb::new(64).split();
        let (garbage, _) = crossbeam_channel::unbounded();
        let mut chain = InsertChain::new(44100); let mut clip = None;
        let (mut l, mut r) = (vec![0.; 184], vec![0.; 184]);
        render_scheduled_block(&mut mixer, &mut queue, 0, 1, &mut l, &mut r, &mut clip, &mut chain, &mut retire, &garbage);
        assert!(l.iter().all(|s| *s == 0.)); assert_eq!(ack.load(Ordering::Acquire), 0);
        let (mut l, mut r) = (vec![0.; 256], vec![0.; 256]);
        render_scheduled_block(&mut mixer, &mut queue, 184, 1, &mut l, &mut r, &mut clip, &mut chain, &mut retire, &garbage);
        assert_eq!(ack.load(Ordering::Acquire), 7);
        assert!(l[..184].iter().all(|s| *s == 0.)); assert!(l[184] > 0.);
    }
    #[test]
    fn callback_event_budget_defers_overload_and_counts_lateness() {
        let mut mixer = mixer(); let mut queue = EventQueue::new(1025);
        for _ in 0..1025 {
            queue.push(Box::new(ScheduledEvent { frame: 0, generation: 0,
                event: AudioEvent::RawCC { strip: 0, cc: 1, value: 0 } })).unwrap();
        }
        assert!(queue.push(event(1, 0)).is_err());
        let (mut retire, _retired) = HeapRb::new(4096).split();
        let (garbage, _) = crossbeam_channel::unbounded();
        let mut chain = InsertChain::new(44100); let mut clip = None;
        let (mut l, mut r) = ([0.; 64], [0.; 64]);
        render_scheduled_block(&mut mixer, &mut queue, 0, 0, &mut l, &mut r, &mut clip, &mut chain, &mut retire, &garbage);
        assert_eq!(queue.next_frame(), Some(0));
        let before = LATE_AUDIO_EVENTS.load(Ordering::Relaxed);
        render_scheduled_block(&mut mixer, &mut queue, 64, 0, &mut l, &mut r, &mut clip, &mut chain, &mut retire, &garbage);
        assert!(LATE_AUDIO_EVENTS.load(Ordering::Relaxed) > before); assert_eq!(queue.next_frame(), None);
    }
}

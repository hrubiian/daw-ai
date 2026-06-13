use super::mixer::{AudioEvent, InsertChain, Mixer, StripMeter};
use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::traits::{Consumer, Producer, Split};
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

pub(crate) fn write_wav(path: &Path, samples: &[f32], sample_rate: u32, channels: u16) -> Result<()> {
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
    pub sample_rate: u32,
}

fn find_output_device(host: &cpal::Host) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    let mut all: Vec<cpal::Device> = host.default_output_device().into_iter().collect();
    if let Ok(devices) = host.output_devices() { all.extend(devices); }
    let mut last_err = String::from("no devices found");
    for device in all {
        let name = device.name().unwrap_or_default();
        match device.default_output_config() {
            Ok(cfg) => return Ok((device, cfg)),
            Err(e)  => last_err = format!("{name}: {e}"),
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
    ) -> Result<Self> {
        let host = cpal::default_host();
        let (device, config) = find_output_device(&host)?;
        let sample_rate = config.sample_rate().0;

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

        let output_stream = device.build_output_stream(
            &stream_config,
            move |data: &mut [f32], _| {
                while let Some(ev) = consumer.try_pop() {
                    match ev {
                        // Legacy preview clip (unchanged TUI semantics).
                        AudioEvent::LoadClip { left, right, volume } => {
                            clip = Some(ClipPlayer { left, right, position: 0, playing: false, volume });
                        }
                        AudioEvent::PlayClip => {
                            if let Some(c) = &mut clip { c.position = 0; c.playing = true; }
                        }
                        AudioEvent::StopClip => {
                            if let Some(c) = &mut clip { c.playing = false; }
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

                let frames = (data.len() / channels).min(max_frames);
                mixer.render_block(&mut left_buf[..frames], &mut right_buf[..frames]);

                // Legacy preview clip on top (post-mixer, straight to master).
                if let Some(c) = &mut clip {
                    if c.playing && !c.left.is_empty() {
                        let clip_len = c.left.len();
                        for i in 0..frames {
                            let pos = (c.position + i) % clip_len;
                            let (l, r) = legacy_chain.process(
                                c.left[pos] * c.volume,
                                c.right[pos] * c.volume,
                            );
                            left_buf[i] += l;
                            right_buf[i] += r;
                        }
                        c.position = (c.position + frames) % clip_len;
                    }
                }

                for (i, frame) in data.chunks_mut(channels).enumerate().take(frames) {
                    for (ch, sample) in frame.iter_mut().enumerate() {
                        *sample = if ch == 0 { left_buf[i] } else if ch == 1 { right_buf[i] } else { 0.0 };
                    }
                }

                if let Some(ref vb) = vis_write {
                    if let Ok(mut vis) = vb.try_lock() {
                        for i in 0..frames {
                            vis.push((left_buf[i] + right_buf[i]) * 0.5);
                        }
                        if vis.len() > 4096 {
                            let excess = vis.len() - 4096;
                            vis.drain(..excess);
                        }
                    }
                }
            },
            |err| eprintln!("audio stream error: {err}"),
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
            _input_stream: input_stream,
            producer,
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
        let _ = self.producer.try_push(event);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
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
        )
        .expect("audio engine init failed");
        let spec = super::super::mixer::StripSpec {
            kind: super::super::mixer::StripKind::Melodic { font: None, bank: 0, preset: 0 },
            gain: 1.0, pan: 0.0, muted: false,
            effects: crate::project::EffectChain::default(),
            clap_instrument: None,
            clap_fx: Vec::new(),
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

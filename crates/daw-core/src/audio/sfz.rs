//! Minimal, dependency-free SFZ v1 / common v2 sample instrument engine.
//!
//! Supports key/velocity splits, note names (`C4`), `#define` / `#include`,
//! `<control>` `default_path` + `set_cc`, `<global>` / `<master>` / `<group>` /
//! `<region>` inheritance, `seq_length` / `seq_position` round-robin,
//! `lorand` / `hirand`, `locc` / `hicc`, `pan_oncc`, choke `group` / `off_by`,
//! ADSR (`ampeg_*`), `loop_continuous` / `loop_sustain` / `one_shot` (plus WAV
//! `smpl` loop chunks), `.flac -> .wav` fallback, CC1 vibrato, CC11 expression,
//! CC64 sustain, and wide pitch bend / microtonal `base_tuning_cents`.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_INCLUDE_DEPTH: usize = 16;
const MAX_SFZ_VOICES: usize = 64;
const VIBRATO_HZ: f32 = 5.4;
const VIBRATO_MAX_CENTS: f32 = 28.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopMode {
    #[default]
    NoLoop,
    OneShot,
    LoopContinuous,
    LoopSustain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TriggerMode {
    #[default]
    Attack,
    Release,
    First,
    Legato,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OffMode {
    Fast,
    #[default]
    Normal,
}

#[derive(Debug, Clone)]
pub struct SampleBuf {
    pub left: Arc<Vec<f32>>,
    pub right: Arc<Vec<f32>>,
    pub embedded_loop: Option<(usize, usize)>,
}

#[derive(Debug, Clone)]
pub struct SfzRegion {
    pub sample: Arc<SampleBuf>,
    pub lokey: u8,
    pub hikey: u8,
    pub pitch_keycenter: u8,
    pub lovel: u8,
    pub hivel: u8,
    pub pitch_keytrack: f32,
    pub tune_cents: f32,
    pub transpose: i8,
    pub volume_gain: f32,
    pub pan: f32,
    pub pan_oncc: Vec<(u8, f32)>,
    pub amp_veltrack: f32,
    pub loop_mode: LoopMode,
    pub loop_start: usize,
    pub loop_end: usize,
    pub offset: usize,
    pub ampeg_attack: f32,
    pub ampeg_decay: f32,
    pub ampeg_sustain: f32,
    pub ampeg_release: f32,
    pub group: u32,
    pub off_by: u32,
    pub off_mode: OffMode,
    pub seq_length: u32,
    pub seq_position: u32,
    pub lorand: f32,
    pub hirand: f32,
    pub locc: Vec<(u8, u8)>,
    pub hicc: Vec<(u8, u8)>,
}

#[derive(Debug)]
pub struct SfzInstrument {
    pub path: PathBuf,
    pub initial_cc: [u8; 128],
    pub regions: Vec<SfzRegion>,
}

#[derive(Debug, Clone)]
struct RegionProps {
    sample: Option<String>,
    base_dir: PathBuf,
    default_path: Option<String>,
    lokey: u8,
    hikey: u8,
    pitch_keycenter: Option<u8>,
    lovel: u8,
    hivel: u8,
    pitch_keytrack: f32,
    tune_cents: f32,
    transpose: i8,
    volume_db: f32,
    pan_pct: f32,
    pan_oncc: Vec<(u8, f32)>,
    amp_veltrack: f32,
    loop_mode: Option<LoopMode>,
    loop_start: Option<usize>,
    loop_end: Option<usize>,
    offset: usize,
    ampeg_attack: f32,
    ampeg_decay: f32,
    ampeg_sustain: f32,
    ampeg_release: Option<f32>,
    group: u32,
    off_by: u32,
    off_mode: OffMode,
    seq_length: u32,
    seq_position: u32,
    lorand: f32,
    hirand: f32,
    trigger: TriggerMode,
    locc: Vec<(u8, u8)>,
    hicc: Vec<(u8, u8)>,
}

impl RegionProps {
    fn root(base_dir: PathBuf) -> Self {
        Self {
            sample: None,
            base_dir,
            default_path: None,
            lokey: 0,
            hikey: 127,
            pitch_keycenter: None,
            lovel: 0,
            hivel: 127,
            pitch_keytrack: 100.0,
            tune_cents: 0.0,
            transpose: 0,
            volume_db: 0.0,
            pan_pct: 0.0,
            pan_oncc: Vec::new(),
            amp_veltrack: 100.0,
            loop_mode: None,
            loop_start: None,
            loop_end: None,
            offset: 0,
            ampeg_attack: 0.002,
            ampeg_decay: 0.0,
            ampeg_sustain: 100.0,
            ampeg_release: None,
            group: 0,
            off_by: 0,
            off_mode: OffMode::Normal,
            seq_length: 1,
            seq_position: 1,
            lorand: 0.0,
            hirand: 1.0,
            trigger: TriggerMode::Attack,
            locc: Vec::new(),
            hicc: Vec::new(),
        }
    }

    fn apply_opcode(&mut self, key: &str, raw_val: &str, cur_dir: &Path) {
        let val = raw_val.trim();
        let k = key.to_ascii_lowercase();
        match k.as_str() {
            "sample" => {
                self.sample = Some(normalize_sfz_path(val));
                self.base_dir = cur_dir.to_path_buf();
            }
            "default_path" => {
                self.default_path = Some(normalize_sfz_path(val));
            }
            "key" => {
                if let Some(n) = parse_note(val) {
                    self.lokey = n;
                    self.hikey = n;
                    self.pitch_keycenter = Some(n);
                }
            }
            "lokey" => {
                if let Some(n) = parse_note(val) {
                    self.lokey = n;
                }
            }
            "hikey" => {
                if let Some(n) = parse_note(val) {
                    self.hikey = n;
                }
            }
            "pitch_keycenter" => {
                if let Some(n) = parse_note(val) {
                    self.pitch_keycenter = Some(n);
                }
            }
            "lovel" => {
                if let Ok(v) = val.parse::<u8>() {
                    self.lovel = v.min(127);
                }
            }
            "hivel" => {
                if let Ok(v) = val.parse::<u8>() {
                    self.hivel = v.min(127);
                }
            }
            "pitch_keytrack" => {
                if let Ok(v) = val.parse::<f32>() {
                    self.pitch_keytrack = v;
                }
            }
            "tune" => {
                if let Ok(v) = val.parse::<f32>() {
                    self.tune_cents = v;
                }
            }
            "transpose" => {
                if let Ok(v) = val.parse::<i8>() {
                    self.transpose = v;
                }
            }
            "volume" => {
                if let Ok(v) = val.parse::<f32>() {
                    self.volume_db = v;
                }
            }
            "pan" => {
                if let Ok(v) = val.parse::<f32>() {
                    self.pan_pct = v;
                }
            }
            "amp_veltrack" => {
                if let Ok(v) = val.parse::<f32>() {
                    self.amp_veltrack = v.clamp(0.0, 100.0);
                }
            }
            "loop_mode" => {
                self.loop_mode = Some(match val.to_ascii_lowercase().as_str() {
                    "one_shot" => LoopMode::OneShot,
                    "loop_continuous" => LoopMode::LoopContinuous,
                    "loop_sustain" => LoopMode::LoopSustain,
                    _ => LoopMode::NoLoop,
                });
            }
            "loop_start" => {
                if let Ok(v) = val.parse::<usize>() {
                    self.loop_start = Some(v);
                }
            }
            "loop_end" => {
                if let Ok(v) = val.parse::<usize>() {
                    self.loop_end = Some(v);
                }
            }
            "offset" => {
                if let Ok(v) = val.parse::<usize>() {
                    self.offset = v;
                }
            }
            "ampeg_attack" => {
                if let Ok(v) = val.parse::<f32>() {
                    self.ampeg_attack = v.max(0.001);
                }
            }
            "ampeg_decay" => {
                if let Ok(v) = val.parse::<f32>() {
                    self.ampeg_decay = v.max(0.0);
                }
            }
            "ampeg_sustain" => {
                if let Ok(v) = val.parse::<f32>() {
                    self.ampeg_sustain = v.clamp(0.0, 100.0);
                }
            }
            "ampeg_release" => {
                if let Ok(v) = val.parse::<f32>() {
                    self.ampeg_release = Some(v.max(0.002));
                }
            }
            "group" => {
                if let Ok(v) = val.parse::<u32>() {
                    self.group = v;
                }
            }
            "off_by" => {
                if let Ok(v) = val.parse::<u32>() {
                    self.off_by = v;
                }
            }
            "off_mode" => {
                self.off_mode = if val.eq_ignore_ascii_case("fast") {
                    OffMode::Fast
                } else {
                    OffMode::Normal
                };
            }
            "seq_length" => {
                if let Ok(v) = val.parse::<u32>() {
                    self.seq_length = v.max(1);
                }
            }
            "seq_position" => {
                if let Ok(v) = val.parse::<u32>() {
                    self.seq_position = v.max(1);
                }
            }
            "lorand" => {
                if let Ok(v) = val.parse::<f32>() {
                    self.lorand = v;
                }
            }
            "hirand" => {
                if let Ok(v) = val.parse::<f32>() {
                    self.hirand = v;
                }
            }
            "trigger" => {
                self.trigger = match val.to_ascii_lowercase().as_str() {
                    "attack" => TriggerMode::Attack,
                    "first" => TriggerMode::First,
                    "legato" => TriggerMode::Legato,
                    "release" | "release_key" => TriggerMode::Release,
                    _ => TriggerMode::Other,
                };
            }
            _ => {
                if let Some(cc) = k.strip_prefix("locc").and_then(|s| s.parse::<u8>().ok()) {
                    if cc < 128 {
                        if let Ok(v) = val.parse::<u8>() {
                            set_pair(&mut self.locc, cc, v.min(127));
                        }
                    }
                } else if let Some(cc) = k.strip_prefix("hicc").and_then(|s| s.parse::<u8>().ok()) {
                    if cc < 128 {
                        if let Ok(v) = val.parse::<u8>() {
                            set_pair(&mut self.hicc, cc, v.min(127));
                        }
                    }
                } else if let Some(cc) = k.strip_prefix("pan_oncc").and_then(|s| s.parse::<u8>().ok())
                {
                    if cc < 128 {
                        if let Ok(v) = val.parse::<f32>() {
                            set_pair_f32(&mut self.pan_oncc, cc, v);
                        }
                    }
                }
            }
        }
    }
}

fn set_pair(list: &mut Vec<(u8, u8)>, cc: u8, val: u8) {
    if let Some(entry) = list.iter_mut().find(|(c, _)| *c == cc) {
        entry.1 = val;
    } else {
        list.push((cc, val));
    }
}

fn set_pair_f32(list: &mut Vec<(u8, f32)>, cc: u8, val: f32) {
    if let Some(entry) = list.iter_mut().find(|(c, _)| *c == cc) {
        entry.1 = val;
    } else {
        list.push((cc, val));
    }
}

fn normalize_sfz_path(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches('"').replace('\\', "/");
    trimmed.strip_prefix("./").unwrap_or(&trimmed).to_string()
}

/// Parse MIDI number (`0..=127`) or note name (`C4` = 60, `Db2` = 37, `F#3` = 54, `C-1` = 0).
pub fn parse_note(raw: &str) -> Option<u8> {
    let s = raw.trim();
    if let Ok(n) = s.parse::<i32>() {
        return (0..=127).contains(&n).then_some(n as u8);
    }
    let bytes = s.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    let base: i32 = match bytes[0].to_ascii_lowercase() {
        b'c' => 0,
        b'd' => 2,
        b'e' => 4,
        b'f' => 5,
        b'g' => 7,
        b'a' => 9,
        b'b' => 11,
        _ => return None,
    };
    let mut idx = 1;
    let mut accidental = 0i32;
    if idx < bytes.len() {
        match bytes[idx] {
            b'#' => {
                accidental = 1;
                idx += 1;
            }
            b'b' | b'B' => {
                accidental = -1;
                idx += 1;
            }
            _ => {}
        }
    }
    let oct: i32 = s[idx..].parse().ok()?;
    let midi = (oct + 1) * 12 + base + accidental;
    (0..=127).contains(&midi).then_some(midi as u8)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Control,
    Global,
    Master,
    Group,
    Region,
    Ignore,
}

fn header_kind(tag: &str) -> Section {
    match tag.to_ascii_lowercase().as_str() {
        "control" => Section::Control,
        "global" => Section::Global,
        "master" => Section::Master,
        "group" => Section::Group,
        "region" => Section::Region,
        _ => Section::Ignore,
    }
}

fn preprocess_sfz(
    path: &Path,
    defines: &mut Vec<(String, String)>,
    out_chunks: &mut Vec<(PathBuf, String)>,
    depth: usize,
) -> Result<()> {
    anyhow::ensure!(depth <= MAX_INCLUDE_DEPTH, "SFZ #include recursion exceeds {MAX_INCLUDE_DEPTH}");
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read SFZ file {}", path.display()))?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
    let mut cur_buf = String::new();

    for raw_line in text.lines() {
        let line = match raw_line.find("//") {
            Some(i) => &raw_line[..i],
            None => raw_line,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("#define") {
            let mut parts = rest.trim().splitn(2, char::is_whitespace);
            if let (Some(name), Some(val)) = (parts.next(), parts.next()) {
                let name = name.trim().to_string();
                let val = val.trim().to_string();
                if let Some(entry) = defines.iter_mut().find(|(n, _)| *n == name) {
                    entry.1 = val;
                } else {
                    defines.push((name, val));
                    defines.sort_by_key(|b| std::cmp::Reverse(b.0.len()));
                }
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("#include") {
            if !cur_buf.is_empty() {
                out_chunks.push((base_dir.clone(), std::mem::take(&mut cur_buf)));
            }
            let rel = apply_defines(rest.trim().trim_matches('"'), defines);
            let rel = normalize_sfz_path(&rel);
            let inc_path = base_dir.join(&rel);
            preprocess_sfz(&inc_path, defines, out_chunks, depth + 1)?;
            continue;
        }
        cur_buf.push_str(&apply_defines(line, defines));
        cur_buf.push('\n');
    }

    if !cur_buf.is_empty() {
        out_chunks.push((base_dir, cur_buf));
    }
    Ok(())
}

fn apply_defines(line: &str, defines: &[(String, String)]) -> String {
    if !line.contains('$') || defines.is_empty() {
        return line.to_string();
    }
    let mut out = line.to_string();
    for (k, v) in defines {
        if out.contains(k.as_str()) {
            out = out.replace(k.as_str(), v.as_str());
        }
    }
    out
}

/// Extract `(opcode, value)` pairs inside one header body while preserving spaces inside values.
fn parse_opcodes(body: &str) -> Vec<(String, String)> {
    let bytes = body.as_bytes();
    let mut boundaries: Vec<(usize, usize, usize)> = Vec::new(); // (token_start, eq_pos, val_start)
    for (eq_idx, &b) in bytes.iter().enumerate() {
        if b != b'=' || eq_idx == 0 {
            continue;
        }
        let mut start = eq_idx;
        while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
            start -= 1;
        }
        if start == eq_idx || !bytes[start].is_ascii_alphabetic() {
            continue;
        }
        if start == 0 || bytes[start - 1].is_ascii_whitespace() {
            boundaries.push((start, eq_idx, eq_idx + 1));
        }
    }
    let mut ops = Vec::with_capacity(boundaries.len());
    for i in 0..boundaries.len() {
        let (tok_start, eq_idx, val_start) = boundaries[i];
        let val_end = if i + 1 < boundaries.len() {
            boundaries[i + 1].0
        } else {
            body.len()
        };
        let key = body[tok_start..eq_idx].trim().to_string();
        let val = body[val_start..val_end].trim().to_string();
        ops.push((key, val));
    }
    ops
}

fn resolve_sample_path(root_dir: &Path, props: &RegionProps, rel: &str) -> Option<PathBuf> {
    let joined = match &props.default_path {
        Some(dp) if !dp.is_empty() => format!("{}/{}", dp.trim_end_matches('/'), rel.trim_start_matches('/')),
        _ => rel.to_string(),
    };
    let candidates = [
        props.base_dir.join(&joined),
        root_dir.join(&joined),
        props.base_dir.join(rel),
        root_dir.join(rel),
    ];
    for cand in candidates {
        if cand.exists() {
            return Some(cand);
        }
        let wav = cand.with_extension("wav");
        if wav.exists() {
            return Some(wav);
        }
    }
    None
}

/// Inspect a WAV header for its native sample rate and optional RIFF `smpl` sustain loop.
fn inspect_wav_metadata(path: &Path) -> (u32, Option<(usize, usize)>) {
    let Ok(mut f) = std::fs::File::open(path) else {
        return (0, None);
    };
    let mut riff = [0u8; 12];
    if f.read_exact(&mut riff).is_err() || &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return (0, None);
    }
    let mut sample_rate = 0u32;
    let mut smpl_loop = None;
    let mut hdr = [0u8; 8];
    while f.read_exact(&mut hdr).is_ok() {
        let size = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
        let padded = size + (size & 1);
        if &hdr[0..4] == b"fmt " && size >= 8 {
            let mut fmt = [0u8; 8];
            if f.read_exact(&mut fmt).is_ok() {
                sample_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
                let rem = padded.saturating_sub(8) as i64;
                if rem > 0 {
                    let _ = f.seek(SeekFrom::Current(rem));
                }
                continue;
            }
            break;
        } else if &hdr[0..4] == b"smpl" && size >= 60 {
            let mut buf = vec![0u8; size.min(128)];
            if f.read_exact(&mut buf).is_ok() {
                let num_loops = u32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]);
                if num_loops > 0 && buf.len() >= 60 {
                    let start = u32::from_le_bytes([buf[44], buf[45], buf[46], buf[47]]) as usize;
                    let end = u32::from_le_bytes([buf[48], buf[49], buf[50], buf[51]]) as usize;
                    if end > start {
                        smpl_loop = Some((start, end));
                    }
                }
                let rem = padded.saturating_sub(buf.len()) as i64;
                if rem > 0 {
                    let _ = f.seek(SeekFrom::Current(rem));
                }
                continue;
            }
            break;
        }
        if f.seek(SeekFrom::Current(padded as i64)).is_err() {
            break;
        }
    }
    (sample_rate, smpl_loop)
}

fn load_sample_cached(
    path: &Path,
    target_sr: u32,
    cache: &mut HashMap<PathBuf, Arc<SampleBuf>>,
) -> Result<(Arc<SampleBuf>, u32)> {
    let (src_sr, raw_loop) = inspect_wav_metadata(path);
    if let Some(buf) = cache.get(path) {
        return Ok((Arc::clone(buf), src_sr));
    }
    let wav_path = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav")) {
        path.to_path_buf()
    } else {
        let alt = path.with_extension("wav");
        if alt.exists() { alt } else { path.to_path_buf() }
    };
    let (src_sr, raw_loop) = if wav_path != path {
        inspect_wav_metadata(&wav_path)
    } else {
        (src_sr, raw_loop)
    };
    if let Some(buf) = cache.get(&wav_path) {
        return Ok((Arc::clone(buf), src_sr));
    }
    let (left, right) = super::load_wav_stereo(&wav_path, target_sr)?;
    let scale = if src_sr > 0 && target_sr > 0 && src_sr != target_sr {
        target_sr as f64 / src_sr as f64
    } else {
        1.0
    };
    let embedded_loop = raw_loop
        .map(|(s, e)| (((s as f64) * scale).round() as usize, ((e as f64) * scale).round() as usize))
        .filter(|(s, e)| *e > *s && *e < left.len());
    let buf = Arc::new(SampleBuf {
        left: Arc::new(left),
        right: Arc::new(right),
        embedded_loop,
    });
    cache.insert(wav_path, Arc::clone(&buf));
    Ok((buf, src_sr))
}

impl SfzInstrument {
    pub fn load(path: &Path, sample_rate: u32) -> Result<Self> {
        let mut cache = HashMap::new();
        Self::load_with_cache(path, sample_rate, &mut cache)
    }

    pub fn load_with_cache(
        path: &Path,
        sample_rate: u32,
        sample_cache: &mut HashMap<PathBuf, Arc<SampleBuf>>,
    ) -> Result<Self> {
        let root_dir = path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        let mut defines = Vec::new();
        let mut chunks = Vec::new();
        preprocess_sfz(path, &mut defines, &mut chunks, 0)?;

        let mut initial_cc = [0u8; 128];
        initial_cc[7] = 100;
        initial_cc[10] = 64;
        initial_cc[11] = 127;

        let mut global_props = RegionProps::root(root_dir.clone());
        let mut master_props = global_props.clone();
        let mut group_props = master_props.clone();
        let mut current_region: Option<RegionProps> = None;
        let mut current_section = Section::Region;
        let mut raw_regions: Vec<RegionProps> = Vec::new();

        for (chunk_dir, chunk_text) in chunks {
            let mut pos = 0usize;
            while pos < chunk_text.len() {
                if let Some(lt_rel) = chunk_text[pos..].find('<') {
                    let lt = pos + lt_rel;
                    if lt > pos {
                        apply_section_body(
                            current_section,
                            &chunk_text[pos..lt],
                            &chunk_dir,
                            &mut initial_cc,
                            &mut global_props,
                            &mut master_props,
                            &mut group_props,
                            &mut current_region,
                        );
                    }
                    let Some(gt_rel) = chunk_text[lt + 1..].find('>') else {
                        break;
                    };
                    let gt = lt + 1 + gt_rel;
                    let tag = chunk_text[lt + 1..gt].trim();
                    let next_sec = header_kind(tag);
                    if let Some(reg) = current_region.take() {
                        raw_regions.push(reg);
                    }
                    match next_sec {
                        Section::Control | Section::Ignore => {}
                        Section::Global => {
                            let saved_default_path = global_props.default_path.clone();
                            global_props = RegionProps::root(root_dir.clone());
                            global_props.default_path = saved_default_path;
                            master_props = global_props.clone();
                            group_props = master_props.clone();
                        }
                        Section::Master => {
                            master_props = global_props.clone();
                            group_props = master_props.clone();
                        }
                        Section::Group => {
                            group_props = master_props.clone();
                        }
                        Section::Region => {
                            current_region = Some(group_props.clone());
                        }
                    }
                    current_section = next_sec;
                    pos = gt + 1;
                } else {
                    apply_section_body(
                        current_section,
                        &chunk_text[pos..],
                        &chunk_dir,
                        &mut initial_cc,
                        &mut global_props,
                        &mut master_props,
                        &mut group_props,
                        &mut current_region,
                    );
                    break;
                }
            }
        }
        if let Some(reg) = current_region.take() {
            raw_regions.push(reg);
        }

        let mut regions = Vec::new();
        for props in raw_regions {
            if matches!(props.trigger, TriggerMode::Release | TriggerMode::Other) {
                continue;
            }
            let Some(sample_rel) = props.sample.as_deref() else {
                continue;
            };
            let Some(sample_path) = resolve_sample_path(&root_dir, &props, sample_rel) else {
                continue;
            };
            let Ok((sample, src_sr)) = load_sample_cached(&sample_path, sample_rate, sample_cache) else {
                continue;
            };
            let len = sample.left.len();
            if len == 0 {
                continue;
            }
            let scale = if src_sr > 0 && sample_rate > 0 && src_sr != sample_rate {
                sample_rate as f64 / src_sr as f64
            } else {
                1.0
            };
            let scale_idx = |v: usize| ((v as f64) * scale).round() as usize;
            let mut loop_mode = props.loop_mode.unwrap_or_else(|| {
                if sample.embedded_loop.is_some() {
                    LoopMode::LoopContinuous
                } else {
                    LoopMode::NoLoop
                }
            });
            let (loop_start, loop_end) = match loop_mode {
                LoopMode::LoopContinuous | LoopMode::LoopSustain => {
                    let explicit = match (props.loop_start, props.loop_end) {
                        (Some(s), Some(e)) => {
                            let ss = scale_idx(s).min(len.saturating_sub(2));
                            let ee = scale_idx(e).min(len.saturating_sub(1));
                            (ee > ss).then_some((ss, ee))
                        }
                        _ => None,
                    };
                    if let Some(bounds) = explicit.or(sample.embedded_loop) {
                        bounds
                    } else {
                        loop_mode = LoopMode::NoLoop;
                        (0, 0)
                    }
                }
                _ => (0, 0),
            };
            let pitch_keycenter = props.pitch_keycenter.unwrap_or(props.lokey);
            let default_rel = if loop_mode == LoopMode::OneShot { 0.03 } else { 0.12 };
            regions.push(SfzRegion {
                sample,
                lokey: props.lokey.min(props.hikey),
                hikey: props.hikey.max(props.lokey),
                pitch_keycenter,
                lovel: props.lovel.min(props.hivel),
                hivel: props.hivel.max(props.lovel),
                pitch_keytrack: props.pitch_keytrack,
                tune_cents: props.tune_cents,
                transpose: props.transpose,
                volume_gain: 10.0f32.powf(props.volume_db / 20.0),
                pan: (props.pan_pct / 100.0).clamp(-1.0, 1.0),
                pan_oncc: props.pan_oncc,
                amp_veltrack: props.amp_veltrack,
                loop_mode,
                loop_start,
                loop_end,
                offset: scale_idx(props.offset).min(len.saturating_sub(1)),
                ampeg_attack: props.ampeg_attack,
                ampeg_decay: props.ampeg_decay,
                ampeg_sustain: props.ampeg_sustain / 100.0,
                ampeg_release: props.ampeg_release.unwrap_or(default_rel),
                group: props.group,
                off_by: props.off_by,
                off_mode: props.off_mode,
                seq_length: props.seq_length,
                seq_position: props.seq_position,
                lorand: props.lorand,
                hirand: props.hirand,
                locc: props.locc,
                hicc: props.hicc,
            });
        }

        anyhow::ensure!(!regions.is_empty(), "SFZ {} produced 0 playable regions", path.display());
        Ok(Self {
            path: path.to_path_buf(),
            initial_cc,
            regions,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_section_body(
    section: Section,
    body: &str,
    cur_dir: &Path,
    initial_cc: &mut [u8; 128],
    global_props: &mut RegionProps,
    master_props: &mut RegionProps,
    group_props: &mut RegionProps,
    current_region: &mut Option<RegionProps>,
) {
    let ops = parse_opcodes(body);
    match section {
        Section::Ignore => {}
        Section::Control => {
            for (k, v) in ops {
                let kl = k.to_ascii_lowercase();
                if kl == "default_path" {
                    global_props.default_path = Some(normalize_sfz_path(&v));
                    master_props.default_path = global_props.default_path.clone();
                    group_props.default_path = global_props.default_path.clone();
                } else if let Some(cc) = kl.strip_prefix("set_cc").and_then(|s| s.parse::<usize>().ok()) {
                    if cc < 128 {
                        if let Ok(val) = v.trim().parse::<u8>() {
                            initial_cc[cc] = val.min(127);
                        }
                    }
                }
            }
        }
        Section::Global => {
            for (k, v) in &ops {
                global_props.apply_opcode(k, v, cur_dir);
                master_props.apply_opcode(k, v, cur_dir);
                group_props.apply_opcode(k, v, cur_dir);
            }
        }
        Section::Master => {
            for (k, v) in &ops {
                master_props.apply_opcode(k, v, cur_dir);
                group_props.apply_opcode(k, v, cur_dir);
            }
        }
        Section::Group => {
            for (k, v) in &ops {
                group_props.apply_opcode(k, v, cur_dir);
            }
        }
        Section::Region => {
            if let Some(reg) = current_region.as_mut() {
                for (k, v) in &ops {
                    reg.apply_opcode(k, v, cur_dir);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnvPhase {
    Attack,
    Decay,
    Sustain,
    Release,
    Done,
}

#[derive(Debug)]
struct SfzVoice {
    region_idx: usize,
    pitch: u8,
    pos: f64,
    gain: f32,
    pan: f32,
    phase: EnvPhase,
    env: f32,
    sustained: bool,
    release_rate: f32,
}

#[derive(Debug)]
pub struct SfzSynth {
    pub instrument: Arc<SfzInstrument>,
    sample_rate: u32,
    voices: Vec<SfzVoice>,
    cc: [u8; 128],
    rr_counter: [u32; 128],
    rng_state: u64,
    pitch_bend_semitones: f32,
    base_tuning_cents: f32,
    vibrato_phase: f32,
}

impl SfzSynth {
    pub fn new(instrument: Arc<SfzInstrument>, sample_rate: u32) -> Self {
        let cc = instrument.initial_cc;
        Self {
            instrument,
            sample_rate: sample_rate.max(1),
            voices: Vec::with_capacity(MAX_SFZ_VOICES),
            cc,
            rr_counter: [0; 128],
            rng_state: 0x9E37_79B9_7F4A_7C15,
            pitch_bend_semitones: 0.0,
            base_tuning_cents: 0.0,
            vibrato_phase: 0.0,
        }
    }

    fn next_rand01(&mut self) -> f32 {
        self.rng_state = self.rng_state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        ((self.rng_state >> 40) as f32) / ((1u32 << 24) as f32)
    }

    pub fn set_base_tuning_cents(&mut self, cents: f32) {
        self.base_tuning_cents = cents;
    }

    pub fn set_pitch_bend(&mut self, lsb: u8, msb: u8, range_semitones: u8) {
        let raw = ((i32::from(msb) << 7) | i32::from(lsb)) as f32;
        let norm = (raw - 8192.0) / 8192.0;
        self.pitch_bend_semitones = norm * f32::from(range_semitones.max(1));
    }

    pub fn control_change(&mut self, cc: u8, value: u8) {
        let val = value.min(127);
        self.cc[usize::from(cc)] = val;
        match cc {
            64 if val < 64 => {
                for v in &mut self.voices {
                    if v.sustained {
                        v.sustained = false;
                        v.phase = EnvPhase::Release;
                    }
                }
            }
            120 => self.voices.clear(),
            123 => {
                for v in &mut self.voices {
                    v.sustained = false;
                    v.phase = EnvPhase::Release;
                }
            }
            _ => {}
        }
    }

    pub fn note_on(&mut self, pitch: u8, velocity: u8) {
        if velocity == 0 {
            self.note_off(pitch);
            return;
        }
        let pitch_idx = usize::from(pitch.min(127));
        let step_num = self.rr_counter[pitch_idx];
        self.rr_counter[pitch_idx] = step_num.wrapping_add(1);
        let rand_val = self.next_rand01();
        let cc = self.cc;

        let mut matched: Vec<usize> = Vec::new();
        for (idx, reg) in self.instrument.regions.iter().enumerate() {
            if pitch < reg.lokey || pitch > reg.hikey {
                continue;
            }
            if velocity < reg.lovel || velocity > reg.hivel {
                continue;
            }
            if reg.seq_length > 1 {
                let expected = (step_num % reg.seq_length) + 1;
                if reg.seq_position != expected {
                    continue;
                }
            }
            if rand_val < reg.lorand || rand_val > reg.hirand {
                continue;
            }
            let cc_ok = reg.locc.iter().all(|&(c, lo)| cc[usize::from(c)] >= lo)
                && reg.hicc.iter().all(|&(c, hi)| cc[usize::from(c)] <= hi);
            if !cc_ok {
                continue;
            }
            matched.push(idx);
        }

        let fast_choke = 1.0 / (self.sample_rate as f32 * 0.008).max(1.0);
        for reg_idx in matched {
            let reg = &self.instrument.regions[reg_idx];
            if reg.off_by != 0 {
                for v in &mut self.voices {
                    let v_group = self.instrument.regions[v.region_idx].group;
                    if v_group != 0 && v_group == reg.off_by && v.phase != EnvPhase::Done {
                        v.sustained = false;
                        v.phase = EnvPhase::Release;
                        if reg.off_mode == OffMode::Fast {
                            v.release_rate = v.release_rate.max(fast_choke);
                        }
                    }
                }
            }
            if self.voices.len() >= MAX_SFZ_VOICES {
                self.voices.remove(0);
            }
            let vel_norm = f32::from(velocity) / 127.0;
            let track = reg.amp_veltrack / 100.0;
            let vel_gain = (1.0 - track) + track * vel_norm.powf(1.5);
            let mut pan_val = reg.pan;
            for &(c, depth_pct) in &reg.pan_oncc {
                pan_val += (depth_pct / 100.0) * (f32::from(self.cc[usize::from(c)]) / 127.0);
            }
            let rel_sec = if reg.off_by != 0 && reg.off_mode == OffMode::Fast {
                reg.ampeg_release.min(0.08)
            } else {
                reg.ampeg_release
            };
            self.voices.push(SfzVoice {
                region_idx: reg_idx,
                pitch,
                pos: reg.offset as f64,
                gain: reg.volume_gain * vel_gain,
                pan: pan_val.clamp(-1.0, 1.0),
                phase: EnvPhase::Attack,
                env: 0.0,
                sustained: false,
                release_rate: 1.0 / (self.sample_rate as f32 * rel_sec).max(1.0),
            });
        }
    }

    pub fn note_off(&mut self, pitch: u8) {
        let pedal = self.cc[64] >= 64;
        for v in &mut self.voices {
            if v.pitch != pitch || v.phase == EnvPhase::Release || v.phase == EnvPhase::Done {
                continue;
            }
            if self.instrument.regions[v.region_idx].loop_mode == LoopMode::OneShot {
                continue;
            }
            if pedal {
                v.sustained = true;
            } else {
                v.phase = EnvPhase::Release;
            }
        }
    }

    pub fn active_keys(&self) -> [u64; 2] {
        let mut mask = [0u64; 2];
        for v in &self.voices {
            if v.phase != EnvPhase::Release && v.phase != EnvPhase::Done && v.pitch < 128 {
                mask[usize::from(v.pitch) / 64] |= 1 << (v.pitch % 64);
            }
        }
        mask
    }

    pub fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let frames = out_l.len().min(out_r.len());
        if self.voices.is_empty() || frames == 0 {
            return;
        }
        let sr = self.sample_rate as f32;
        let expr = f32::from(self.cc[11]) / 127.0;
        let mod_depth = f32::from(self.cc[1]) / 127.0;
        let vib_cents = if mod_depth > 0.0 {
            let phase_step = std::f32::consts::TAU * VIBRATO_HZ * frames as f32 / sr;
            let s = (self.vibrato_phase + phase_step * 0.5).sin();
            self.vibrato_phase = (self.vibrato_phase + phase_step) % std::f32::consts::TAU;
            s * mod_depth * VIBRATO_MAX_CENTS
        } else {
            0.0
        };
        let global_semi = self.pitch_bend_semitones + (self.base_tuning_cents + vib_cents) / 100.0;

        self.voices.retain_mut(|v| {
            let reg = &self.instrument.regions[v.region_idx];
            let left = &reg.sample.left;
            let right = &reg.sample.right;
            let len = left.len();
            if len == 0 || v.phase == EnvPhase::Done {
                return false;
            }
            let atk_rate = 1.0 / (sr * reg.ampeg_attack).max(1.0);
            let dec_rate = if reg.ampeg_decay > 0.001 {
                (1.0 - reg.ampeg_sustain) / (sr * reg.ampeg_decay).max(1.0)
            } else {
                1.0
            };
            let semi = (f32::from(v.pitch) - f32::from(reg.pitch_keycenter))
                * (reg.pitch_keytrack / 100.0)
                + f32::from(reg.transpose)
                + reg.tune_cents / 100.0
                + global_semi;
            let step = 2.0f64.powf(f64::from(semi) / 12.0);
            let pan_l = (1.0 - v.pan.max(0.0)).sqrt();
            let pan_r = (1.0 + v.pan.min(0.0)).sqrt();
            let g_l = v.gain * expr * pan_l;
            let g_r = v.gain * expr * pan_r;

            let loop_active = match reg.loop_mode {
                LoopMode::LoopContinuous => reg.loop_end > reg.loop_start,
                LoopMode::LoopSustain => reg.loop_end > reg.loop_start && v.phase != EnvPhase::Release,
                _ => false,
            };

            for i in 0..frames {
                if loop_active && v.pos >= reg.loop_end as f64 {
                    let span = (reg.loop_end - reg.loop_start) as f64;
                    v.pos = reg.loop_start as f64 + ((v.pos - reg.loop_start as f64) % span);
                }
                let idx = v.pos as usize;
                if idx >= len {
                    v.phase = EnvPhase::Done;
                    return false;
                }
                let frac = (v.pos - idx as f64) as f32;
                let idx2 = if idx + 1 < len {
                    idx + 1
                } else if loop_active {
                    reg.loop_start
                } else {
                    idx
                };
                let sl = left[idx] + (left[idx2] - left[idx]) * frac;
                let sr_smp = right[idx] + (right[idx2] - right[idx]) * frac;

                match v.phase {
                    EnvPhase::Attack => {
                        v.env += atk_rate;
                        if v.env >= 1.0 {
                            v.env = 1.0;
                            v.phase = if reg.ampeg_sustain < 0.999 && reg.ampeg_decay > 0.001 {
                                EnvPhase::Decay
                            } else {
                                EnvPhase::Sustain
                            };
                        }
                    }
                    EnvPhase::Decay => {
                        v.env -= dec_rate;
                        if v.env <= reg.ampeg_sustain {
                            v.env = reg.ampeg_sustain;
                            v.phase = EnvPhase::Sustain;
                        }
                    }
                    EnvPhase::Sustain => v.env = reg.ampeg_sustain,
                    EnvPhase::Release => {
                        v.env -= v.release_rate;
                        if v.env <= 0.0 {
                            v.env = 0.0;
                            v.phase = EnvPhase::Done;
                            return false;
                        }
                    }
                    EnvPhase::Done => return false,
                }
                out_l[i] += sl * g_l * v.env;
                out_r[i] += sr_smp * g_r * v.env;
                v.pos += step;
            }
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::{SampleFormat, WavSpec, WavWriter};

    fn write_test_wav(path: &Path, sr: u32, freq: f32, frames: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let spec = WavSpec {
            channels: 1,
            sample_rate: sr,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(path, spec).unwrap();
        for i in 0..frames {
            let s = (std::f32::consts::TAU * freq * i as f32 / sr as f32).sin() * 0.7;
            w.write_sample((s * i16::MAX as f32) as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn parses_defines_includes_note_names_pan_cc_rr_choke_and_fallback() {
        let dir = std::env::temp_dir().join(format!("daw-sfz-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Programs")).unwrap();
        write_test_wav(&dir.join("Samples/tone_a.wav"), 44_100, 440.0, 22050);
        write_test_wav(&dir.join("Samples/tone_b.wav"), 44_100, 880.0, 22050);
        write_test_wav(&dir.join("Samples/hat_open.wav"), 44_100, 2000.0, 22050);
        write_test_wav(&dir.join("Samples/hat_closed.wav"), 44_100, 4000.0, 4410);

        std::fs::write(
            dir.join("Programs/sub.sfz"),
            "// sub-include\n<group> lovel=64 seq_length=2\n<region> lokey=C4 hikey=C4 pitch_keycenter=A4 sample=Samples\\tone_a.flac\n<region> key=C4 seq_position=2 sample=Samples\\tone_b.wav\n",
        )
        .unwrap();
        let main_sfz = dir.join("main.sfz");
        std::fs::write(
            &main_sfz,
            "#define $OP_KEY 46\n<control>\ndefault_path=\nset_cc10=64\n<global> pan=-100 pan_oncc10=200\n#include \"Programs\\sub.sfz\"\n<group> loop_mode=one_shot\n<region> key=$OP_KEY group=1 off_by=1 sample=Samples/hat_open.wav\n<region> key=42 group=1 off_by=1 off_mode=fast sample=Samples/hat_closed.wav\n",
        )
        .unwrap();

        let inst = SfzInstrument::load(&main_sfz, 44_100).unwrap();
        assert_eq!(inst.regions.len(), 4);
        assert_eq!(inst.regions[0].lokey, 60);
        assert_eq!(inst.regions[0].pitch_keycenter, 69);
        assert_eq!(inst.regions[2].lokey, 46);

        let mut synth = SfzSynth::new(Arc::new(inst), 44_100);
        // Round robin step 1 -> tone_a (region 0), step 2 -> tone_b (region 1)
        synth.note_on(60, 100);
        assert_eq!(synth.voices.last().unwrap().region_idx, 0);
        // Center pan via pan=-100 + pan_oncc10=200 + set_cc10=64
        assert!(synth.voices.last().unwrap().pan.abs() < 0.02);
        synth.note_on(60, 100);
        assert_eq!(synth.voices.last().unwrap().region_idx, 1);

        // Choke: open hat (group 1) chokes on closed hat (off_by 1)
        synth.note_on(46, 110);
        synth.note_on(42, 110);
        assert!(synth.voices.iter().any(|v| v.pitch == 46 && v.phase == EnvPhase::Release));

        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        synth.render(&mut l, &mut r);
        let energy: f32 = l.iter().map(|x| x * x).sum();
        assert!(energy > 0.1, "SFZ synth must render audible audio, energy={energy}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn offline_export_renders_sfz_track_with_wide_pitch_bend() {
        let dir = std::env::temp_dir().join(format!("daw-sfz-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_test_wav(&dir.join("tone.wav"), 44_100, 440.0, 44_100);
        let sfz_path = dir.join("horn.sfz");
        std::fs::write(&sfz_path, "<region> key=60 sample=tone.wav ampeg_release=0.05\n").unwrap();

        let mut p = crate::project::Project::default_project();
        p.tracks.retain(|t| t.id == "t2");
        p.arrangement.retain(|a| a.track == "t2");
        p.set_track_sfz("t2", sfz_path.to_string_lossy().into_owned()).unwrap();
        p.set_track_pitch_bend_range("t2", 12).unwrap();
        if let crate::project::Pattern::PianoRoll(pr) = &mut p.patterns[1] {
            pr.bars = 1;
            pr.notes = vec![crate::project::Note { pitch: 60, tick: 0, duration: 240, vel: 115 }];
        }
        p.validate().unwrap();
        let (plain, _) = crate::audio::render_project_samples(&p, Path::new("/missing.sf2"), false).unwrap();
        assert!(plain.iter().any(|&x| x.abs() > 0.05), "SFZ track must render sound");

        let mut bent = p.clone();
        bent.set_pattern_pitch_bends("p2", vec![crate::project::PitchBendPoint { tick: 0, cents: 700.0 }]).unwrap();
        let (shifted, _) = crate::audio::render_project_samples(&bent, Path::new("/missing.sf2"), false).unwrap();
        let diff: f32 = plain.iter().zip(&shifted).take(44100).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 50.0, "SFZ wide pitch bend must shift output, diff={diff}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preserves_control_default_path_across_global_section() {
        let dir = std::env::temp_dir().join(format!("daw-sfz-defpath-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_test_wav(&dir.join("sub/tone.wav"), 44_100, 440.0, 22050);
        let sfz_path = dir.join("defpath.sfz");
        std::fs::write(
            &sfz_path,
            "<control>\ndefault_path=sub/\n<global>\nampeg_release=0.2\n<region>\nsample=tone.wav\nlokey=60\nhikey=60\n",
        )
        .unwrap();

        let inst = SfzInstrument::load(&sfz_path, 44_100).unwrap();
        assert_eq!(inst.regions.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

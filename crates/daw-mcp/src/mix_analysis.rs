//! "AI ears": per-track spectral profiling + frequency-masking detection.
//!
//! `listen` gives one averaged whole-mix spectrum; this solo-renders each track
//! so the agent can see *where each track lives* and *where tracks collide*.

use daw_core::project::Project;
use std::path::Path;

fn db(x: f32) -> f32 {
    if x <= 1e-9 { -120.0 } else { 20.0 * x.log10() }
}

/// One solo-rendered track's spectral fingerprint.
#[derive(Clone)]
pub struct TrackProfile {
    pub id: String,
    pub name: String,
    /// Overall loudness of the solo render over its active bars, dBFS.
    pub rms_db: f32,
    /// Per-band level (name, dB), low band -> high band.
    pub bands: Vec<(&'static str, f32)>,
    /// 0-based bars where the track had audible energy (> -65 dBFS).
    pub active_bars: Vec<u32>,
    /// Linear energy in the focal presence range (500..6000 Hz) per 0-based bar.
    pub bar_focal_energy: Vec<f32>,
}

/// A band where two or more audible tracks pile up — a masking risk.
pub struct MaskingFinding {
    pub band: &'static str,
    /// The contending tracks and their in-band level, loudest first.
    pub tracks: Vec<(String, f32)>,
}

/// A track quieter than this (dBFS) is treated as inaudible for masking.
const AUDIBLE_DB: f32 = -50.0;
/// A solo render this quiet (dBFS) is effectively silent — no meaningful spectrum.
const SILENT_DB: f32 = -80.0;
/// A band within this many dB of a track's own loudest band is one it "lives" in.
const LIVES_WITHIN_DB: f32 = 6.0;
/// A living band must also be within this many band-slots of the track's spectral
/// centroid — this rejects the phantom low-frequency floor that envelope/gate
/// leakage puts in the `sub` band even for high-pitched material.
const CENTROID_SPREAD: f32 = 2.0;
/// A band only counts as a real collision if its level is within this many dB of
/// the mix's loudest band — cuts noise-floor overlaps between quiet tracks.
const MASK_WINDOW_DB: f32 = 24.0;

/// Energy-weighted centre of a track's spectrum, as a fractional band index.
fn centroid_index(p: &TrackProfile) -> f32 {
    let (mut num, mut den) = (0.0f64, 0.0f64);
    for (i, (_, d)) in p.bands.iter().enumerate() {
        let e = 10f64.powf(*d as f64 / 10.0); // dB -> linear energy
        num += i as f64 * e;
        den += e;
    }
    if den > 0.0 { (num / den) as f32 } else { 0.0 }
}

/// The band a track's energy is centred on (robust to low-frequency floor noise).
pub fn dominant_band(p: &TrackProfile) -> &'static str {
    if p.bands.is_empty() {
        return "sub";
    }
    let idx = centroid_index(p).round().clamp(0.0, (p.bands.len() - 1) as f32) as usize;
    p.bands[idx].0
}

/// The bands a track "lives" in: within `LIVES_WITHIN_DB` of its loudest band
/// AND within `CENTROID_SPREAD` slots of its centroid (drops the phantom sub).
fn living_bands(p: &TrackProfile) -> Vec<bool> {
    let cen = centroid_index(p);
    let own_max = p.bands.iter().map(|(_, v)| *v).fold(f32::MIN, f32::max);
    p.bands
        .iter()
        .enumerate()
        .map(|(j, (_, lvl))| *lvl >= own_max - LIVES_WITHIN_DB && (j as f32 - cen).abs() <= CENTROID_SPREAD)
        .collect()
}

/// Bands where >= 2 audible tracks both live (and overlap in time) — ordered low band -> high.
pub fn detect_masking(profiles: &[TrackProfile]) -> Vec<MaskingFinding> {
    let Some(first) = profiles.first() else { return Vec::new() };
    let n_bands = first.bands.len();
    let living: Vec<Vec<bool>> = profiles.iter().map(living_bands).collect();
    // The mix's loudest band across all tracks — the reference for "audible" energy.
    let global_max = profiles
        .iter()
        .flat_map(|p| p.bands.iter().map(|(_, v)| *v))
        .fold(f32::MIN, f32::max);
    let bars_overlap = |a: &TrackProfile, b: &TrackProfile| -> bool {
        a.active_bars.is_empty()
            || b.active_bars.is_empty()
            || a.active_bars.iter().any(|bar| b.active_bars.contains(bar))
    };
    let mut findings = Vec::new();
    for j in 0..n_bands {
        let mut candidates: Vec<usize> = Vec::new();
        for (pi, p) in profiles.iter().enumerate() {
            if p.rms_db <= AUDIBLE_DB {
                continue; // inaudible track can't mask anything
            }
            // Real collision only if this band carries near-peak energy, not floor noise.
            if living[pi][j] && p.bands[j].1 >= global_max - MASK_WINDOW_DB {
                candidates.push(pi);
            }
        }
        let mut contenders: Vec<(String, f32)> = candidates
            .iter()
            .copied()
            .filter(|&pi| {
                candidates
                    .iter()
                    .any(|&pj| pi != pj && bars_overlap(&profiles[pi], &profiles[pj]))
            })
            .map(|pi| (profiles[pi].name.clone(), profiles[pi].bands[j].1))
            .collect();
        if contenders.len() >= 2 {
            contenders.sort_by(|a, b| b.1.total_cmp(&a.1)); // loudest first
            findings.push(MaskingFinding { band: first.bands[j].0, tracks: contenders });
        }
    }
    findings
}

fn focal_band_energy(bands: &[(&'static str, f32)]) -> f32 {
    bands
        .iter()
        .filter(|(name, _)| matches!(*name, "mid" | "high-mid"))
        .map(|(_, d)| 10f32.powf(*d / 10.0))
        .sum()
}

/// Solo-render every unmuted track and profile its active-bar loudness + 7-band spectrum.
pub fn track_profiles(project: &Project, sf2: &Path) -> anyhow::Result<Vec<TrackProfile>> {
    let mut out = Vec::new();
    let bpm = project.project.bpm.max(1.0);
    let song_bars = project.song_bars();
    for t in project.tracks.iter().filter(|t| !t.muted) {
        let mut solo = project.clone();
        for o in &mut solo.tracks {
            o.muted = o.id != t.id;
        }
        let (samples, sr) = daw_core::audio::export::render_project_samples(&solo, sf2, false)?;
        let frames = samples.len() / 2;
        if frames == 0 {
            continue;
        }
        let mono: Vec<f32> = samples.as_chunks::<2>().0.iter().map(|c| (c[0] + c[1]) * 0.5).collect();
        let mut active_bars = Vec::new();
        let mut active_mono = Vec::new();
        let mut bar_focal_energy = Vec::with_capacity(song_bars as usize);
        for bi in 0..song_bars {
            let lo = (daw_core::musical_time::bar_sample_map(bi, bpm, project.project.time_signature, &project.tempo_changes, sr) as usize).min(mono.len());
            let hi = (daw_core::musical_time::bar_sample_map(bi + 1, bpm, project.project.time_signature, &project.tempo_changes, sr) as usize).min(mono.len());
            if hi <= lo {
                bar_focal_energy.push(0.0);
                continue;
            }
            let slice = &mono[lo..hi];
            let bar_rms = (slice.iter().map(|&m| (m * m) as f64).sum::<f64>() / slice.len() as f64).sqrt() as f32;
            let bar_bands = crate::analysis::spectral_bands(slice, sr);
            bar_focal_energy.push(focal_band_energy(&bar_bands));
            if db(bar_rms) > -65.0 {
                active_bars.push(bi);
                active_mono.extend_from_slice(slice);
            }
        }
        let used = if active_mono.is_empty() { &mono[..] } else { &active_mono[..] };
        let rms = (used.iter().map(|&m| (m * m) as f64).sum::<f64>() / used.len().max(1) as f64).sqrt() as f32;
        let bands = crate::analysis::spectral_bands(used, sr);
        out.push(TrackProfile {
            id: t.id.clone(),
            name: t.name.clone(),
            rms_db: db(rms),
            bands,
            active_bars,
            bar_focal_energy,
        });
    }
    Ok(out)
}

/// Human/agent-readable mix report: per-track active-bar spectral home + masking findings.
pub fn mix_report(project: &Project, sf2: &Path) -> anyhow::Result<String> {
    mix_report_with_lead(project, sf2, None, None, None)
}

/// Mix report plus optional foreground Lead-to-Accompaniment Ratio (LAR) in 500..6000 Hz.
pub fn mix_report_with_lead(
    project: &Project,
    sf2: &Path,
    lead_track_id: Option<&str>,
    start_bar: Option<u32>,
    end_bar: Option<u32>,
) -> anyhow::Result<String> {
    if let Some(lid) = lead_track_id {
        anyhow::ensure!(
            project.tracks.iter().any(|t| t.id == lid),
            "unknown lead_track_id '{lid}'"
        );
    }
    let profiles = track_profiles(project, sf2)?;
    if profiles.is_empty() {
        return Ok("no audible tracks to analyze.".into());
    }
    let mut s = String::from("per-track spectral profile (active bars):\n");
    for p in &profiles {
        // A silent solo render has an all-floor spectrum, whose centroid is a
        // meaningless middle band — label it "silent" instead of a bogus home.
        let home = if p.rms_db <= SILENT_DB {
            "silent".to_string()
        } else {
            format!("lives in {} ({} active bars)", dominant_band(p), p.active_bars.len())
        };
        s.push_str(&format!("  {:<22} {:>6.1} dBFS  {}\n", p.name, p.rms_db, home));
    }
    let findings = detect_masking(&profiles);
    if findings.is_empty() {
        s.push_str("\nno masking: tracks occupy distinct bands.\n");
    } else {
        s.push_str("\nmasking risks (low band first — carve EQ / re-voice / pan / drop a layer):\n");
        for f in &findings {
            let who = f
                .tracks
                .iter()
                .map(|(n, d)| format!("{n} ({d:.0} dB)"))
                .collect::<Vec<_>>()
                .join(", ");
            s.push_str(&format!("  {}: {}\n", f.band, who));
        }
    }

    if let Some(lid) = lead_track_id {
        s.push_str(&foreground_lar_report(
            &profiles,
            lid,
            start_bar.unwrap_or(0),
            end_bar.unwrap_or_else(|| project.song_bars()),
        ));
    }
    Ok(s)
}

pub fn foreground_lar_report(
    profiles: &[TrackProfile],
    lead_track_id: &str,
    start_bar: u32,
    end_bar: u32,
) -> String {
    let Some(lead) = profiles.iter().find(|p| p.id == lead_track_id) else {
        return format!("\nforeground check ({lead_track_id}): track is muted or silent.\n");
    };
    let mut bars_lar: Vec<(u32, f32)> = Vec::new();
    for &b in &lead.active_bars {
        if b < start_bar || b >= end_bar {
            continue;
        }
        let Some(&lead_e) = lead.bar_focal_energy.get(b as usize) else { continue };
        let accomp_e: f32 = profiles
            .iter()
            .filter(|p| p.id != lead_track_id)
            .filter_map(|p| p.bar_focal_energy.get(b as usize).copied())
            .sum();
        let lar_db = 10.0 * (lead_e.max(1e-12) / accomp_e.max(1e-12)).log10();
        bars_lar.push((b, lar_db));
    }
    if bars_lar.is_empty() {
        return format!(
            "\nforeground check ({} in bars {start_bar}..{end_bar}): no active lead bars.\n",
            lead.name
        );
    }
    let mean_lar = bars_lar.iter().map(|(_, v)| *v).sum::<f32>() / bars_lar.len() as f32;
    let &(min_bar, min_lar) = bars_lar
        .iter()
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .expect("non-empty");
    let mut out = format!(
        "\nforeground Lead-to-Accompaniment Ratio (500–6000 Hz) for \"{}\" (bars {start_bar}..{end_bar}):\n  mean LAR: {mean_lar:+.1} dB | min LAR: {min_lar:+.1} dB at bar {}\n",
        lead.name,
        min_bar + 1
    );
    if min_lar < 0.0 {
        out.push_str(&format!(
            "  - BURIAL: accompaniment exceeds lead in 500–6000 Hz at bar {} ({min_lar:+.1} dB) — duck accompaniment or lift lead.\n",
            min_bar + 1
        ));
    } else if mean_lar < 2.5 {
        out.push_str(&format!(
            "  - CROWDED FOREGROUND: mean LAR {mean_lar:+.1} dB (< +2.5 dB) — lead sits too close to accompaniment bed.\n"
        ));
    } else {
        out.push_str("  lead sits clearly in front of accompaniment.\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: [&str; 7] = ["sub", "bass", "low-mid", "mid", "high-mid", "presence", "air"];

    /// Build a profile: bands listed in `peaks` get that level, the rest sit at -80.
    fn profile(name: &str, rms_db: f32, peaks: &[(&'static str, f32)]) -> TrackProfile {
        let bands = NAMES
            .iter()
            .map(|n| (*n, peaks.iter().find(|(b, _)| b == n).map(|(_, v)| *v).unwrap_or(-80.0)))
            .collect();
        TrackProfile {
            id: name.to_lowercase(),
            name: name.into(),
            rms_db,
            bands,
            active_bars: Vec::new(),
            bar_focal_energy: Vec::new(),
        }
    }

    #[test]
    fn dominant_band_is_the_loudest() {
        let p = profile("Bass", -10.0, &[("bass", -6.0), ("sub", -18.0)]);
        assert_eq!(dominant_band(&p), "bass");
    }

    #[test]
    fn two_tracks_in_same_band_are_flagged() {
        let a = profile("Bass", -8.0, &[("bass", -5.0)]);
        let b = profile("Sub Drone", -10.0, &[("bass", -6.0)]);
        let f = detect_masking(&[a, b]);
        assert_eq!(f.len(), 1, "one masking band expected");
        assert_eq!(f[0].band, "bass");
        let names: Vec<&str> = f[0].tracks.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"Bass") && names.contains(&"Sub Drone"), "{names:?}");
    }

    #[test]
    fn tracks_in_disjoint_bands_are_not_flagged() {
        let a = profile("Bass", -8.0, &[("bass", -5.0)]);
        let b = profile("Cymbal", -9.0, &[("air", -4.0)]);
        assert!(detect_masking(&[a, b]).is_empty());
    }

    #[test]
    fn near_floor_overlap_is_not_masking() {
        // A loud lead sets the mix's spectral peak; two audible-but-quiet tracks
        // share 'bass' only ~45 dB below that peak — noise-floor overlap, not a
        // real collision, so it must not be flagged.
        let lead = profile("Lead", -8.0, &[("high-mid", -5.0)]);
        let a = profile("Pad A", -40.0, &[("bass", -50.0)]);
        let b = profile("Pad B", -42.0, &[("bass", -51.0)]);
        let f = detect_masking(&[lead, a, b]);
        assert!(f.iter().all(|x| x.band != "bass"), "near-floor 'bass' shouldn't be flagged: {:?}",
            f.iter().map(|x| x.band).collect::<Vec<_>>());
    }

    #[test]
    fn inaudible_track_does_not_cause_masking() {
        let loud = profile("Bass", -8.0, &[("bass", -5.0)]);
        let quiet = profile("Ghost", -70.0, &[("bass", -6.0)]); // below AUDIBLE_DB
        assert!(detect_masking(&[loud, quiet]).is_empty());
    }

    fn band_index(name: &str) -> usize {
        NAMES.iter().position(|n| *n == name).unwrap()
    }

    #[test]
    #[ignore] // needs assets/GeneralUser.sf2; run with --ignored
    fn track_profiles_separate_bass_from_high_lead() {
        // Two melodic tracks: a low bass note and a high, bright lead note.
        let json = r#"{
          "version":"0.1",
          "project":{"name":"t","bpm":120.0,"time_signature":[4,4],"key":"E","scale":"minor"},
          "tracks":[
            {"id":"bass","name":"Bass","type":"melodic",
             "instrument":{"type":"soundfont","preset":38,"bank":0},
             "volume":0.9,"pan":0.0,"muted":false,"patterns":["pb"]},
            {"id":"lead","name":"Lead","type":"melodic",
             "instrument":{"type":"soundfont","preset":81,"bank":0},
             "volume":0.9,"pan":0.0,"muted":false,"patterns":["pl"]}
          ],
          "patterns":[
            {"type":"piano_roll","id":"pb","name":"b","bars":1,"ticks_per_bar":480,
             "notes":[{"pitch":28,"tick":0,"duration":460,"vel":110}]},
            {"type":"piano_roll","id":"pl","name":"l","bars":1,"ticks_per_bar":480,
             "notes":[{"pitch":88,"tick":0,"duration":460,"vel":110}]}
          ],
          "arrangement":[
            {"track":"bass","pattern":"pb","bar":0},
            {"track":"lead","pattern":"pl","bar":0}
          ]
        }"#;
        let proj: Project = serde_json::from_str(json).expect("parse");
        let sf2 = Path::new("../../assets/GeneralUser.sf2");
        let profiles = track_profiles(&proj, sf2).expect("render");
        assert_eq!(profiles.len(), 2);
        let bass = profiles.iter().find(|p| p.name == "Bass").unwrap();
        let lead = profiles.iter().find(|p| p.name == "Lead").unwrap();
        let (bb, lb) = (dominant_band(bass), dominant_band(lead));
        eprintln!("bass lives in {bb}, lead lives in {lb}");
        // The bass sits in the low third; the lead's energy sits clearly higher.
        assert!(band_index(bb) <= band_index("low-mid"), "bass should be low, got {bb}");
        assert!(band_index(lb) > band_index(bb), "lead should be higher than bass, got {lb} vs {bb}");
    }

    #[test]
    fn non_overlapping_active_bars_do_not_collide_and_lar_flags_burial() {
        let mut a = profile("Flute", -32.0, &[("mid", -28.0)]);
        a.active_bars = vec![0, 1];
        a.bar_focal_energy = vec![0.002, 0.002, 0.0, 0.0];
        let mut b = profile("Horn", -32.0, &[("mid", -28.0)]);
        b.active_bars = vec![2, 3];
        b.bar_focal_energy = vec![0.0, 0.0, 0.005, 0.005];
        assert!(detect_masking(&[a.clone(), b.clone()]).is_empty(), "non-overlapping bars do not mask");

        let mut pad = profile("Pad", -24.0, &[("mid", -20.0)]);
        pad.active_bars = vec![0, 1];
        pad.bar_focal_energy = vec![0.010, 0.010, 0.0, 0.0];
        let lar = foreground_lar_report(&[a, pad], "flute", 0, 2);
        assert!(lar.contains("BURIAL"), "{lar}");
    }
}

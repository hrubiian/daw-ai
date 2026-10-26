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
pub struct TrackProfile {
    pub name: String,
    /// Overall loudness of the solo render, dBFS.
    pub rms_db: f32,
    /// Per-band level (name, dB), low band -> high band.
    pub bands: Vec<(&'static str, f32)>,
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

/// Bands where >= 2 audible tracks both live — ordered low band -> high.
pub fn detect_masking(profiles: &[TrackProfile]) -> Vec<MaskingFinding> {
    let Some(first) = profiles.first() else { return Vec::new() };
    let n_bands = first.bands.len();
    let living: Vec<Vec<bool>> = profiles.iter().map(living_bands).collect();
    // The mix's loudest band across all tracks — the reference for "audible" energy.
    let global_max = profiles
        .iter()
        .flat_map(|p| p.bands.iter().map(|(_, v)| *v))
        .fold(f32::MIN, f32::max);
    let mut findings = Vec::new();
    for j in 0..n_bands {
        let mut contenders: Vec<(String, f32)> = Vec::new();
        for (pi, p) in profiles.iter().enumerate() {
            if p.rms_db <= AUDIBLE_DB {
                continue; // inaudible track can't mask anything
            }
            // Real collision only if this band carries near-peak energy, not floor noise.
            if living[pi][j] && p.bands[j].1 >= global_max - MASK_WINDOW_DB {
                contenders.push((p.name.clone(), p.bands[j].1));
            }
        }
        if contenders.len() >= 2 {
            contenders.sort_by(|a, b| b.1.total_cmp(&a.1)); // loudest first
            findings.push(MaskingFinding { band: first.bands[j].0, tracks: contenders });
        }
    }
    findings
}

/// Solo-render every unmuted track and profile its loudness + 7-band spectrum.
/// Mirrors `auto_balance`'s solo-render loop; silent renders are skipped.
pub fn track_profiles(project: &Project, sf2: &Path) -> anyhow::Result<Vec<TrackProfile>> {
    let mut out = Vec::new();
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
        let mono: Vec<f32> = samples.chunks_exact(2).map(|c| (c[0] + c[1]) * 0.5).collect();
        let rms = (mono.iter().map(|&m| (m * m) as f64).sum::<f64>() / frames as f64).sqrt() as f32;
        let bands = crate::analysis::spectral_bands(&mono, sr);
        out.push(TrackProfile { name: t.name.clone(), rms_db: db(rms), bands });
    }
    Ok(out)
}

/// Human/agent-readable mix report: per-track spectral home + masking findings.
pub fn mix_report(project: &Project, sf2: &Path) -> anyhow::Result<String> {
    let profiles = track_profiles(project, sf2)?;
    if profiles.is_empty() {
        return Ok("no audible tracks to analyze.".into());
    }
    let mut s = String::from("per-track spectral profile:\n");
    for p in &profiles {
        // A silent solo render has an all-floor spectrum, whose centroid is a
        // meaningless middle band — label it "silent" instead of a bogus home.
        let home = if p.rms_db <= SILENT_DB {
            "silent".to_string()
        } else {
            format!("lives in {}", dominant_band(p))
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
    Ok(s)
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
        TrackProfile { name: name.into(), rms_db, bands }
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
}

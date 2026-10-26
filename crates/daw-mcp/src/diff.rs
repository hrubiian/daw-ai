//! Music CI, slice B: semantic musical diff. Describes the *musical* changes
//! between two project versions (added tracks, volume/instrument/effect edits,
//! note-count changes, arrangement moves) rather than a JSON line-diff.

use daw_core::project::{Instrument, Pattern, Project, Track};

fn instrument_label(i: &Instrument) -> String {
    match i {
        Instrument::Soundfont { preset, bank, .. } => format!("sf {bank}:{preset}"),
        Instrument::Sampler { pads } => format!("sampler ({} pads)", pads.len()),
        Instrument::AudioFile { .. } => "audio".into(),
        Instrument::Clap(_) => "clap".into(),
    }
}

fn changed(a: f32, b: f32) -> bool {
    (a - b).abs() > 1e-6
}

fn diff_meta(o: &Project, n: &Project, out: &mut Vec<String>) {
    let (om, nm) = (&o.project, &n.project);
    if changed(om.bpm as f32, nm.bpm as f32) {
        out.push(format!("tempo: {:.0} -> {:.0} bpm", om.bpm, nm.bpm));
    }
    if om.time_signature != nm.time_signature {
        out.push(format!(
            "time signature: {}/{} -> {}/{}",
            om.time_signature[0], om.time_signature[1], nm.time_signature[0], nm.time_signature[1]
        ));
    }
    if om.key != nm.key || om.scale != nm.scale {
        out.push(format!("key: {} {} -> {} {}", om.key, om.scale, nm.key, nm.scale));
    }
    for (name, a, b) in [
        ("swing", om.swing, nm.swing),
        ("humanize", om.humanize, nm.humanize),
        ("timing", om.timing, nm.timing),
    ] {
        if changed(a, b) {
            out.push(format!("{name}: {a:.2} -> {b:.2}"));
        }
    }
}

fn diff_track_fields(o: &Track, n: &Track, out: &mut Vec<String>) {
    let who = &n.name;
    if o.name != n.name {
        out.push(format!("track {}: renamed \"{}\" -> \"{}\"", o.id, o.name, n.name));
    }
    if changed(o.volume, n.volume) {
        out.push(format!("{who}: volume {:.2} -> {:.2}", o.volume, n.volume));
    }
    if changed(o.pan, n.pan) {
        out.push(format!("{who}: pan {:+.2} -> {:+.2}", o.pan, n.pan));
    }
    if o.muted != n.muted {
        out.push(format!("{who}: {}", if n.muted { "muted" } else { "unmuted" }));
    }
    let (ol, nl) = (instrument_label(&o.instrument), instrument_label(&n.instrument));
    if ol != nl {
        out.push(format!("{who}: instrument {ol} -> {nl}"));
    }
    if changed(o.tuning_cents, n.tuning_cents) {
        out.push(format!("{who}: tuning {:+.0}c -> {:+.0}c", o.tuning_cents, n.tuning_cents));
    }
    for (name, a, b) in [
        ("reverb", o.effects.reverb, n.effects.reverb),
        ("chorus", o.effects.chorus, n.effects.chorus),
        ("cutoff", o.effects.cutoff, n.effects.cutoff),
        ("resonance", o.effects.resonance, n.effects.resonance),
        ("send_delay", o.effects.send_delay, n.effects.send_delay),
        ("drive", o.effects.drive, n.effects.drive),
    ] {
        if changed(a, b) {
            out.push(format!("{who}: {name} {a:.2} -> {b:.2}"));
        }
    }
}

fn diff_tracks(old: &Project, new: &Project, out: &mut Vec<String>) {
    for n in &new.tracks {
        if !old.tracks.iter().any(|o| o.id == n.id) {
            out.push(format!("added track \"{}\" ({})", n.name, instrument_label(&n.instrument)));
        }
    }
    for o in &old.tracks {
        match new.tracks.iter().find(|n| n.id == o.id) {
            None => out.push(format!("removed track \"{}\"", o.name)),
            Some(n) => diff_track_fields(o, n, out),
        }
    }
}

fn active_steps(p: &Pattern) -> usize {
    match p {
        Pattern::Step(s) => s.rows.iter().flat_map(|r| &r.steps).filter(|&&v| v != 0).count(),
        _ => 0,
    }
}

fn diff_patterns(old: &Project, new: &Project, out: &mut Vec<String>) {
    for n in &new.patterns {
        if !old.patterns.iter().any(|o| o.id() == n.id()) {
            out.push(format!("added pattern \"{}\"", n.name()));
        }
    }
    for o in &old.patterns {
        let Some(n) = new.patterns.iter().find(|x| x.id() == o.id()) else {
            out.push(format!("removed pattern \"{}\"", o.name()));
            continue;
        };
        match (o, n) {
            (Pattern::PianoRoll(a), Pattern::PianoRoll(b)) => {
                if a.notes.len() != b.notes.len() {
                    out.push(format!(
                        "pattern \"{}\": {} -> {} notes",
                        n.name(),
                        a.notes.len(),
                        b.notes.len()
                    ));
                }
                if a.legato != b.legato {
                    out.push(format!("pattern \"{}\": legato {}", n.name(), if b.legato { "on" } else { "off" }));
                }
            }
            (Pattern::Step(_), Pattern::Step(_)) => {
                let (a, b) = (active_steps(o), active_steps(n));
                if a != b {
                    out.push(format!("pattern \"{}\": {a} -> {b} active steps", n.name()));
                }
            }
            _ => out.push(format!("pattern \"{}\": type changed", n.name())),
        }
    }
}

fn diff_arrangement(old: &Project, new: &Project, out: &mut Vec<String>) {
    let key = |s: &daw_core::project::ArrangementSlot| (s.track.clone(), s.pattern.clone());
    for n in &new.arrangement {
        match old.arrangement.iter().find(|o| key(o) == key(n)) {
            None => out.push(format!("clip: added {} on {} @bar {}", n.pattern, n.track, n.bar + 1)),
            Some(o) => {
                if o.bar != n.bar {
                    out.push(format!("clip {} on {}: moved bar {} -> {}", n.pattern, n.track, o.bar + 1, n.bar + 1));
                }
                if o.len_bars != n.len_bars {
                    out.push(format!("clip {} on {}: length {:?} -> {:?}", n.pattern, n.track, o.len_bars, n.len_bars));
                }
            }
        }
    }
    for o in &old.arrangement {
        if !new.arrangement.iter().any(|n| key(n) == key(o)) {
            out.push(format!("clip: removed {} on {}", o.pattern, o.track));
        }
    }
}

/// Readable list of musical changes from `old` to `new`; a single
/// "no musical changes" line when the two are musically identical.
pub fn diff_projects(old: &Project, new: &Project) -> Vec<String> {
    let mut out = Vec::new();
    diff_meta(old, new, &mut out);
    diff_tracks(old, new, &mut out);
    diff_patterns(old, new, &mut out);
    diff_arrangement(old, new, &mut out);
    if out.is_empty() {
        out.push("no musical changes".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_core::project::{Note, Pattern, Project, TrackType};

    fn base() -> Project {
        Project::default_project()
    }

    fn contains(v: &[String], needle: &str) -> bool {
        v.iter().any(|s| s.contains(needle))
    }

    #[test]
    fn identical_projects_report_no_changes() {
        let p = base();
        assert_eq!(diff_projects(&p, &p.clone()), vec!["no musical changes".to_string()]);
    }

    #[test]
    fn tempo_change_reported() {
        let old = base();
        let mut new = old.clone();
        new.project.bpm = old.project.bpm + 10.0;
        assert!(contains(&diff_projects(&old, &new), "tempo"), "{:?}", diff_projects(&old, &new));
    }

    #[test]
    fn added_track_reported() {
        let old = base();
        let mut new = old.clone();
        new.add_track(TrackType::Melodic);
        assert!(contains(&diff_projects(&old, &new), "added track"), "{:?}", diff_projects(&old, &new));
    }

    #[test]
    fn volume_change_reported() {
        let old = base();
        let mut new = old.clone();
        new.set_track_volume("t2", 1.5);
        let d = diff_projects(&old, &new);
        assert!(contains(&d, "volume"), "{d:?}");
    }

    #[test]
    fn notes_added_reported() {
        let old = base();
        let mut new = old.clone();
        for pat in &mut new.patterns {
            if let Pattern::PianoRoll(pr) = pat {
                if pr.id == "p2" {
                    pr.notes.push(Note { pitch: 60, tick: 960, duration: 120, vel: 80 });
                }
            }
        }
        assert!(contains(&diff_projects(&old, &new), "notes"), "{:?}", diff_projects(&old, &new));
    }
}

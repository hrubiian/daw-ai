//! Surge XT integration (runs only where the plugin is installed).
use daw_core::audio::clap::{fxp_chunk, ClapInstance};
use std::path::{Path, PathBuf};

const SURGE: &str = "/usr/lib/clap/Surge XT.clap";
const SURGE_ID: &str = "org.surge-synth-team.surge-xt";

fn surge() -> Option<&'static str> {
    if Path::new(SURGE).exists() {
        Some(SURGE)
    } else {
        eprintln!("skip: Surge XT not installed");
        None
    }
}

/// First `.fxp` (sorted) in a factory category folder.
fn factory_patch(category: &str) -> PathBuf {
    let dir = Path::new("/usr/share/surge-xt/patches_factory").join(category);
    let mut patches: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "fxp"))
        .collect();
    patches.sort();
    patches.into_iter().next().expect("a factory patch")
}

/// Half a second of middle C at 48 kHz, left channel.
fn render_note(inst: &mut ClapInstance) -> Vec<f32> {
    let p = inst.processor.as_mut().unwrap();
    p.queue_note_on(60, 100);
    let mut out = Vec::new();
    for _ in 0..(24_000 / 256) {
        let (mut l, mut r) = (vec![0f32; 256], vec![0f32; 256]);
        p.process(&mut l, &mut r);
        out.extend_from_slice(&l);
    }
    out
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

#[test]
fn surge_loads_factory_patch_as_state() {
    let Some(path) = surge() else { return };
    let mut init = ClapInstance::load(path, SURGE_ID, 48_000, 256).expect("load Surge XT");
    let baseline = render_note(&mut init);
    init.shutdown();

    let patch = factory_patch("Basses");
    let bytes = std::fs::read(&patch).unwrap();
    let mut inst = ClapInstance::load(path, SURGE_ID, 48_000, 256).expect("load Surge XT");
    inst.handle
        .load_state(fxp_chunk(&bytes).expect("FPCh chunk"))
        .unwrap_or_else(|e| panic!("{}: {e}", patch.display()));
    let patched = render_note(&mut inst);
    inst.shutdown();

    eprintln!("{}: init rms {:.5}, patch rms {:.5}", patch.display(), rms(&baseline), rms(&patched));
    assert!(rms(&patched) > 1e-4, "patch renders sound");
    let diff: f32 = baseline.iter().zip(&patched).map(|(a, b)| (a - b).abs()).sum();
    assert!(diff > 1e-3, "the loaded patch changes the sound (diff {diff})");
}

#[test]
fn surge_converts_cutoff_text() {
    let Some(path) = surge() else { return };
    let mut inst = ClapInstance::load(path, SURGE_ID, 48_000, 256).expect("load Surge XT");
    let id = inst.handle.param_infos.iter().find(|i| i.name == "A Filter 1 Cutoff").expect("cutoff").id;
    let text = inst.handle.value_to_text(id, 0.5).expect("cutoff text");
    assert!(text.contains("Hz"), "cutoff reads in Hz: {text}");
    let v = inst.handle.text_to_value(id, "800 Hz").expect("parse 800 Hz");
    assert!((0.0..=1.0).contains(&v), "normalized: {v}");
    let back = inst.handle.value_to_text(id, v).unwrap();
    let hz: f64 = back.split_whitespace().next().unwrap().parse().unwrap_or_else(|_| panic!("number in {back}"));
    assert!((hz - 800.0).abs() / 800.0 < 0.05, "round trip 800 Hz -> {back}");
    inst.shutdown();
}

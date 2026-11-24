use anyhow::{ensure, Context, Result};
use daw_core::audio::{clap, engine};
use daw_core::project::{format, Instrument, Project};
use std::path::Path;

pub fn run(
    project_path: Option<&Path>,
    project: &Project,
    sf2: &Path,
    headless: bool,
) -> Result<()> {
    let mut failures = 0;
    let mut check = |label: &str, result: Result<String>| match result {
        Ok(detail) => println!("OK  {label}: {detail}"),
        Err(error) => {
            failures += 1;
            println!("FAIL  {label}: {error:#}");
        }
    };

    let sf2 = std::path::absolute(sf2)?;
    check(
        "Default SoundFont",
        engine::probe_soundfont(&sf2)
            .with_context(|| format!("{} (see README Quickstart for setup)", sf2.display()))
            .map(|()| sf2.display().to_string()),
    );

    let plugin_paths = clap::standard_paths();
    if let Some(path) = project_path {
        println!("OK  Project: {} ({})", project.project.name, path.display());
        for asset in format::referenced_asset_paths(project, &sf2) {
            if asset != sf2 {
                check("Project asset", readable_file(&asset));
            }
        }
        let plugins = project.tracks.iter().flat_map(|track| {
            let instrument = match &track.instrument {
                Instrument::Clap(slot) => Some(slot),
                _ => None,
            };
            instrument.into_iter().chain(&track.clap_fx)
        });
        for plugin in plugins {
            let result = clap::is_available_plugin_path(&plugin.path)
                .then(|| format!("{} at {}", plugin.id, plugin.path))
                .with_context(|| {
                    format!(
                        "{} at {} is missing or outside the installed plugin directories",
                        plugin.id, plugin.path
                    )
                });
            check("Project CLAP plugin", result);
        }
    }
    println!("CLAP search paths (plugins are not loaded):");
    for path in plugin_paths {
        let status = if path.is_dir() || path.is_file() { "present" } else { "missing" };
        println!("  {} ({status})", path.display());
    }
    if let Some(path) = daw_core::audio::flow::library_dir() {
        println!("Flow Music plugin library: {}", path.display());
    }

    if !headless {
        check(
            "Audio output configuration",
            engine::probe_output_device()
                .map(|(name, rate, channels)| format!("{name}, {rate} Hz, {channels} channels")),
        );
        let ports = daw_io::list_midi_ports();
        let detail = if ports.is_empty() {
            "none detected".into()
        } else {
            ports.join(", ")
        };
        println!("MIDI inputs (optional): {detail}");
    }
    ensure!(failures == 0, "doctor found {failures} failed check(s)");
    Ok(())
}

fn readable_file(path: &Path) -> Result<String> {
    ensure!(
        path.is_file(),
        "missing or not a regular file: {}",
        path.display()
    );
    std::fs::File::open(path).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(path.display().to_string())
}

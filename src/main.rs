use anyhow::Result;
use daw_core::audio::{export_mp3, export_wav, ExportFormat};
use daw_core::project::{format as project_fmt, Project};
use daw_io::Host;
use daw_tui::app::App;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

mod doctor;

/// Make relative asset paths (`assets/GeneralUser.sf2`, `assets/samples/…`)
/// resolve regardless of how the app was launched. A desktop-icon launch starts
/// in `$HOME`, not the repo, so the soundfont wasn't found and the audio engine
/// was silently disabled — Play produced nothing. If the assets aren't reachable
/// from the current directory, walk up from the executable's location to find the
/// project root (it contains `assets/`) and switch the working directory to it.
fn ensure_asset_cwd() {
    const MARKER: &str = "assets/GeneralUser.sf2";
    if Path::new(MARKER).exists() {
        return; // launched somewhere assets already resolve (e.g. `cargo run`)
    }
    if let Ok(exe) = std::env::current_exe() {
        // ancestors(): the exe path, then each parent directory upward.
        for base in exe.ancestors().skip(1) {
            if base.join(MARKER).exists() {
                let _ = std::env::set_current_dir(base);
                return;
            }
        }
    }
}

#[derive(Default)]
struct LaunchOptions {
    gui: bool,
    project: Option<PathBuf>,
    export: Option<(ExportFormat, PathBuf)>,
    doctor: bool,
    headless: bool,
    import_plugin: Option<PathBuf>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<LaunchOptions> {
    let mut options = LaunchOptions::default();
    let mut args = args.into_iter().peekable();
    if args.peek().is_some_and(|arg| arg == "doctor") {
        options.doctor = true;
        args.next();
    }
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--gui" => options.gui = true,
            "--headless" => options.headless = true,
            "--import-plugin" => {
                anyhow::ensure!(options.import_plugin.is_none(), "specify only one plugin to import");
                let path = args.next().filter(|a| !a.starts_with("--"))
                    .ok_or_else(|| anyhow::anyhow!("--import-plugin requires a Flow Music .fmx file"))?;
                options.import_plugin = Some(PathBuf::from(path));
            }
            "--export-mp3" | "--export-wav" => {
                anyhow::ensure!(options.export.is_none(), "specify only one export");
                let path = args
                    .next()
                    .filter(|a| !a.starts_with("--"))
                    .ok_or_else(|| anyhow::anyhow!("{arg} requires an output filename"))?;
                let format = if arg == "--export-wav" {
                    ExportFormat::Wav
                } else {
                    ExportFormat::Mp3
                };
                options.export = Some((format, PathBuf::from(path)));
            }
            flag if flag.starts_with("--") => anyhow::bail!("unknown option: {flag}"),
            _ => {
                anyhow::ensure!(options.project.is_none(), "specify only one project file");
                options.project = Some(PathBuf::from(arg));
            }
        }
    }
    anyhow::ensure!(
        options.export.is_none() || options.project.is_some(),
        "export requires a project file"
    );
    anyhow::ensure!(
        !(options.gui && options.export.is_some()),
        "use CLI export without --gui, or export from the app's File menu"
    );
    anyhow::ensure!(
        !options.headless || options.doctor,
        "--headless requires doctor"
    );
    anyhow::ensure!(
        !options.doctor || (!options.gui && options.export.is_none()),
        "doctor accepts only a project path and --headless"
    );
    anyhow::ensure!(
        options.import_plugin.is_none() || !(options.gui || options.project.is_some() || options.doctor),
        "--import-plugin is a standalone command"
    );
    Ok(options)
}

fn main() -> Result<()> {
    let options = parse_args(std::env::args().skip(1))?;
    if let Some(path) = options.import_plugin {
        let plugin = daw_io::flow::import_plugin(&path)?;
        println!("Imported {} ({})", plugin.name, plugin.path);
        return Ok(());
    }
    let gui = options.gui;
    let project_path = options
        .project
        .map(|p| project_fmt::absolute(&p))
        .transpose()?;
    // Resolve both paths before locating the soundfont changes the working directory.
    let export = options
        .export
        .map(|(format, path)| project_fmt::absolute(&path).map(|path| (format, path)))
        .transpose()?;

    // Load the project relative to the launch directory (a relative path arg must
    // still resolve), then anchor the working directory to the asset root so the
    // soundfont and samples load no matter how we were started (icon vs terminal).
    let project = match &project_path {
        Some(path) => project_fmt::load(path)?,
        None => Project::default_project(),
    };
    ensure_asset_cwd();

    // Resolve assets the same way for the frontends, diagnostics and exports.
    let sf2_path = PathBuf::from("assets/GeneralUser.sf2");
    let sf2_str = sf2_path.to_string_lossy().into_owned();
    let runtime_project = project_fmt::resolve_assets(
        &project,
        project_path
            .as_deref()
            .and_then(Path::parent)
            .unwrap_or(Path::new(".")),
        &std::env::current_dir()?,
    );
    if options.doctor {
        return doctor::run(
            project_path.as_deref(),
            &runtime_project,
            &sf2_path,
            options.headless,
        );
    }
    if let Some((format, output)) = export {
        match format {
            ExportFormat::Wav => export_wav(&runtime_project, &sf2_path, &output)?,
            ExportFormat::Mp3 => export_mp3(&runtime_project, &sf2_path, &output)?,
        }
        println!("Exported {}: {}", format.label(), output.display());
        return Ok(());
    }
    let mut host = Host::start(&runtime_project, sf2_path);
    match daw_io::session::LiveSession::start(project.clone(), project_path.as_deref(), &host) {
        Ok(session) => host.session = Some(session),
        Err(e) => {
            *host.engine_error.lock().unwrap() = Some(format!("live session unavailable: {e}"));
            eprintln!("live session unavailable: {e}");
        }
    }

    // Dispatch to a frontend.
    if gui {
        daw_gpui::run(host, project, project_path, sf2_str);
        return Ok(());
    }

    // TUI: clone the channel/Arc handles for the App and keep `host` alive (it owns the
    // live MIDI connection) until `run` returns.
    let mut app = App::new(
        project,
        project_path,
        host.cmd_tx.clone(),
        daw_tui::app::RecordTransport {
            take_gate: Arc::clone(&host.take_gate),
            roll_start_tick: Arc::clone(&host.roll_start_tick),
            transport_state: Arc::clone(&host.transport_state),
        },
        host.midi_ev_rx.clone(),
        Arc::clone(&host.midi_channel),
        host.midi_device.clone(),
        Arc::clone(&host.midi_map),
        Arc::clone(&host.playback_tick),
        Arc::clone(&host.vis_buf),
    );
    app.session = host.session.clone();
    app.refresh_all_waveforms();
    daw_tui::run(app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Result<LaunchOptions> {
        parse_args(values.iter().map(|v| (*v).to_owned()))
    }

    #[test]
    fn mp3_export_accepts_paths_with_spaces_in_either_order() {
        for values in [
            ["song title.daw.json", "--export-mp3", "output title.mp3"],
            ["--export-mp3", "output title.mp3", "song title.daw.json"],
        ] {
            let options = args(&values).unwrap();
            assert_eq!(options.project, Some(PathBuf::from("song title.daw.json")));
            assert_eq!(
                options.export,
                Some((ExportFormat::Mp3, PathBuf::from("output title.mp3")))
            );
            assert!(!options.gui);
        }
    }

    #[test]
    fn incomplete_or_ambiguous_exports_fail_before_starting_audio() {
        for values in [
            vec!["--export-mp3"],
            vec!["--export-mp3", "--gui", "song.json"],
            vec!["--export-mp3", "out.mp3"],
            vec!["--gui", "song.json", "--export-mp3", "out.mp3"],
            vec!["song.json", "other.json"],
            vec!["--unknown"],
        ] {
            assert!(args(&values).is_err(), "{values:?}");
        }
        assert!(args(&[]).is_ok());
        assert!(args(&["--gui", "song.json"]).unwrap().gui);
    }
}

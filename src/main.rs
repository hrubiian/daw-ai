use anyhow::Result;
use daw_core::project::{format as project_fmt, Project};
use daw_io::Host;
use daw_tui::app::App;
use std::{path::{Path, PathBuf}, sync::Arc};

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

fn main() -> Result<()> {
    // 1. Parse args: `--gui` selects the GPUI frontend; the first non-flag arg is the
    //    project file. `cargo run` opens the TUI; `cargo run -- --gui song.daw.json`
    //    opens the GPUI window.
    let mut gui = false;
    let mut project_path: Option<PathBuf> = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--gui" => gui = true,
            other => project_path = Some(PathBuf::from(other)),
        }
    }

    // Load the project relative to the launch directory (a relative path arg must
    // still resolve), then anchor the working directory to the asset root so the
    // soundfont and samples load no matter how we were started (icon vs terminal).
    let project = match &project_path {
        Some(path) => project_fmt::load(path)?,
        None => Project::default_project(),
    };
    ensure_asset_cwd();

    // 2. Boot the engine: audio thread + sequencer + MIDI input, project loaded.
    let sf2_path = PathBuf::from("assets/GeneralUser.sf2");
    let sf2_str = sf2_path.to_string_lossy().into_owned();
    // The legacy in-engine input recorder is TUI-only; the GPUI frontend uses
    // the dedicated capture subsystem (no second stream grabbing input devices).
    let host = Host::start(&project, sf2_path, !gui);

    // 3. Dispatch to a frontend.
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
        host.record_ctrl.clone(),
        host.midi_ev_rx.clone(),
        Arc::clone(&host.midi_channel),
        host.midi_device.clone(),
        Arc::clone(&host.midi_map),
        Arc::clone(&host.playback_tick),
        Arc::clone(&host.vis_buf),
    );
    app.refresh_all_waveforms();
    daw_tui::run(app)?;
    Ok(())
}

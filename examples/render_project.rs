//! Render a project to a stereo WAV through the shared mixer.
//!
//! ```bash
//! cargo run --release --example render_project -- examples/demo.daw.json demo.wav
//! ```
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let project_path = args.next().unwrap_or_else(|| "examples/demo.daw.json".into());
    let out_path = PathBuf::from(args.next().unwrap_or_else(|| "demo.wav".into()));
    let sf2_path = PathBuf::from("assets/GeneralUser.sf2");

    println!("Loading project: {project_path}");
    let project = daw_core::project::format::load(&project_path)?;
    if let Some(dir) = out_path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    println!("Rendering with soundfont {} ...", sf2_path.display());
    daw_core::audio::export_wav(&project, &sf2_path, &out_path)?;
    println!("Saved {}", out_path.display());
    Ok(())
}

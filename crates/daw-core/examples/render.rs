// Headless renderer: render a .daw.json project to a stereo WAV.
//
//   cargo run -p daw-core --example render -- <project.daw.json> <out.wav> [soundfont.sf2]
//
// Soundfont defaults to assets/GeneralUser.sf2.

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let project_path = args.next().expect("usage: render <project.daw.json> <out.wav> [sf2]");
    let out_path     = args.next().expect("usage: render <project.daw.json> <out.wav> [sf2]");
    let sf2_path     = args.next().unwrap_or_else(|| "assets/GeneralUser.sf2".to_string());

    let project = daw_core::project::format::load(&project_path)?;
    daw_core::audio::export::export_wav(&project, &PathBuf::from(&sf2_path), &PathBuf::from(&out_path))?;

    println!("rendered {project_path} -> {out_path}");
    Ok(())
}

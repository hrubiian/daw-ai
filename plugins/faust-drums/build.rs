use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=drums.dsp");
    println!("cargo:rerun-if-env-changed=FAUST");
    println!("cargo:rustc-check-cfg=cfg(feature, values(\"default-boxed\"))");

    let compiler = env::var_os("FAUST").unwrap_or_else(|| "faust".into());
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR")).join("drums.rs");
    let status = Command::new(compiler)
        .args([
            "-lang",
            "rust",
            "-cn",
            "DrumDsp",
            "-ftz",
            "1",
            "drums.dsp",
            "-o",
        ])
        .arg(output)
        .status()
        .expect("Install the Faust compiler or set FAUST to its executable");
    assert!(status.success(), "Faust could not compile drums.dsp");
}

fn main() {
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../crates");
    println!("cargo:rerun-if-changed=../../src");
    println!("cargo:rerun-if-changed=../../.git/refs");
    let revision = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "source".into());
    let dirty = std::process::Command::new("git")
        .args([
            "diff",
            "--quiet",
            "--",
            "../../crates",
            "../../src",
            "../../Cargo.toml",
        ])
        .status()
        .map(|s| !s.success())
        .unwrap_or(false);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    println!(
        "cargo:rustc-env=DAW_BUILD_REVISION={revision}{} · {stamp}",
        if dirty { "+dirty" } else { "" }
    );
}

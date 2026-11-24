use std::path::{Path, PathBuf};
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_path(name: &str) -> Option<PathBuf> {
    git(&["rev-parse", "--git-path", name]).map(PathBuf::from)
}

fn watch(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
}

fn main() {
    watch(Path::new("build.rs"));
    let mut revision = None;
    if Path::new("../../.git").exists() {
        revision = git(&["rev-parse", "--short", "HEAD"]);
        if let Some(head) = git_path("HEAD").filter(|path| path.is_file()) {
            watch(&head);
        }
        if let Some(reference) =
            git(&["symbolic-ref", "-q", "HEAD"]).and_then(|name| git_path(&name))
        {
            if reference.is_file() {
                watch(&reference);
            } else {
                // A commit on a packed branch creates a loose ref.
                if let Some(dir) = reference.ancestors().skip(1).find(|path| path.is_dir()) {
                    watch(dir);
                }
                if let Some(packed) = git_path("packed-refs").filter(|path| path.is_file()) {
                    watch(&packed);
                }
            }
        }
    }

    println!(
        "cargo:rustc-env=DAW_BUILD_REVISION={}",
        revision.as_deref().unwrap_or("source")
    );
}

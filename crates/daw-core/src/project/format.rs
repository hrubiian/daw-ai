use crate::project::{Instrument, Project};
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub type Revision = String;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub fn digest(bytes: &[u8]) -> Revision {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn project_revision(project: &Project) -> Result<Revision> {
    Ok(digest(&serde_json::to_vec(project)?))
}
pub fn revision(path: &Path) -> Result<Option<Revision>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(digest(&bytes))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub fn absolute(path: &Path) -> Result<PathBuf> {
    let path = std::path::absolute(path)?;
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let parent = path
        .parent()
        .context("project needs a parent directory")?
        .canonicalize()?;
    Ok(parent.join(path.file_name().context("project needs a file name")?))
}
pub fn load_versioned(path: &Path) -> Result<(Project, Revision)> {
    let bytes = std::fs::read(path)?;
    let project: Project = serde_json::from_slice(&bytes)?;
    project.validate()?;
    Ok((project, digest(&bytes)))
}
pub fn load(path: impl AsRef<Path>) -> Result<Project> {
    Ok(load_versioned(path.as_ref())?.0)
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = sibling(
        path,
        &format!(
            ".{}.{}.tmp",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ),
    );
    let result = (|| {
        let mut f = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}
/// Compare-and-swap under a cross-process advisory lock. The sidecar stays in
/// place so every writer locks the same inode, even after crashes and renames.
pub fn save_if_unchanged(
    project: &Project,
    path: &Path,
    expected: Option<&str>,
) -> Result<Revision> {
    project.validate()?;
    let path = absolute(path)?;
    let bytes = serde_json::to_vec_pretty(project)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(sibling(&path, ".lock"))?;
    lock.lock()?;
    let current = revision(&path)?;
    ensure!(
        current.as_deref() == expected,
        "save conflict: {} changed externally; local changes retained",
        path.display()
    );
    if current.is_some() {
        atomic_write(&sibling(&path, ".bak"), &std::fs::read(&path)?)?;
    }
    atomic_write(&path, &bytes)?;
    Ok(digest(&bytes))
}
/// Explicit overwrite helper for noninteractive callers. Interactive editors
/// must retain their loaded revision and use save_if_unchanged.
pub fn save(project: &Project, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let expected = revision(path)?;
    save_if_unchanged(project, path, expected.as_deref())?;
    Ok(())
}

pub fn resolve_asset_path(value: &str, base: &Path, legacy_root: &Path) -> PathBuf {
    if value.is_empty() || Path::new(value).is_absolute() {
        return value.into();
    }
    let local = base.join(value);
    let fallback = legacy_root.join(value);
    if local.exists() || !fallback.exists() {
        local
    } else {
        fallback
    }
}

/// Hash the actual runtime assets used by a render (paths should be resolved).
pub fn asset_fingerprints(
    project: &Project,
    default_sf2: &Path,
) -> Result<std::collections::BTreeMap<String, String>> {
    let mut paths = std::collections::BTreeSet::new();
    for track in &project.tracks {
        match &track.instrument {
            Instrument::Soundfont { soundfont, .. } => {
                paths.insert(
                    soundfont
                        .as_ref()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| default_sf2.into()),
                );
            }
            Instrument::AudioFile { path } if !path.is_empty() => {
                paths.insert(PathBuf::from(path));
            }
            Instrument::Sampler { pads } => {
                paths.extend(pads.iter().map(|p| PathBuf::from(&p.path)));
                paths.extend(pads.iter().flat_map(|p| p.variants.iter().map(|v| PathBuf::from(&v.path))));
            }
            Instrument::Clap(plugin) => {
                paths.insert(PathBuf::from(&plugin.path));
            }
            _ => {}
        }
        paths.extend(track.clap_fx.iter().map(|p| PathBuf::from(&p.path)));
    }
    paths.extend(project.audio_clips.iter().map(|c| PathBuf::from(&c.path)));
    paths
        .into_iter()
        .map(|path| {
            let hash =
                digest(&std::fs::read(&path).with_context(|| format!("asset {}", path.display()))?);
            Ok((path.display().to_string(), hash))
        })
        .collect()
}

/// Resolve resource paths on a runtime clone, never rewriting the saved file.
/// Repository-relative legacy assets are a fallback only when project-local
/// resolution fails. This keeps older example projects working.
pub fn resolve_assets(project: &Project, base: &Path, legacy_root: &Path) -> Project {
    let mut p = project.clone();
    let resolve = |value: &mut String| {
        *value = resolve_asset_path(value, base, legacy_root)
            .to_string_lossy()
            .into_owned();
    };
    for t in &mut p.tracks {
        match &mut t.instrument {
            Instrument::Soundfont {
                soundfont: Some(path),
                ..
            } => resolve(path),
            Instrument::AudioFile { path } => resolve(path),
            Instrument::Sampler { pads } => {
                for pad in pads {
                    resolve(&mut pad.path);
                    for variant in &mut pad.variants { resolve(&mut variant.path); }
                }
            }
            Instrument::Clap(c) => resolve(&mut c.path),
            _ => {}
        }
        for c in &mut t.clap_fx {
            resolve(&mut c.path);
        }
    }
    for c in &mut p.audio_clips {
        resolve(&mut c.path);
    }
    p
}

#[cfg(test)]
mod tests {
    use crate::project::Project;

    #[test]
    fn stale_writes_and_invalid_projects_preserve_disk_and_backup() {
        let dir = std::env::temp_dir().join(format!(
            "daw-save-{}-{}",
            std::process::id(),
            super::TEMP_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.json");
        let mut p = Project::default_project();
        let first = super::save_if_unchanged(&p, &path, None).unwrap();
        p.project.bpm = 100.;
        let second = super::save_if_unchanged(&p, &path, Some(&first)).unwrap();
        assert!(super::save_if_unchanged(&p, &path, Some(&first)).is_err());
        assert_eq!(
            super::load(super::sibling(&path, ".bak"))
                .unwrap()
                .project
                .bpm,
            120.
        );
        p.project.bpm = 0.;
        assert!(super::save_if_unchanged(&p, &path, Some(&second)).is_err());
        assert_eq!(super::load(&path).unwrap().project.bpm, 100.);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn concurrent_writers_have_one_winner() {
        let dir = std::env::temp_dir().join(format!("daw-race-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.json");
        let _ = std::fs::remove_file(&path);
        let first = super::save_if_unchanged(&Project::default_project(), &path, None).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let workers: Vec<_> = (0..2)
            .map(|i| {
                let path = path.clone();
                let first = first.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut p = Project::default_project();
                    p.project.bpm = 90. + i as f64;
                    barrier.wait();
                    super::save_if_unchanged(&p, &path, Some(&first)).is_ok()
                })
            })
            .collect();
        assert_eq!(
            workers
                .into_iter()
                .filter_map(|w| w.join().ok())
                .filter(|ok| *ok)
                .count(),
            1
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn round_trip_default_project() {
        let original = Project::default_project();
        let json = serde_json::to_string_pretty(&original).unwrap();
        let restored: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.version, original.version);
        assert_eq!(restored.project.bpm, original.project.bpm);
        assert_eq!(restored.tracks.len(), original.tracks.len());
        assert_eq!(restored.patterns.len(), original.patterns.len());
        assert_eq!(restored.tracks[0].id, original.tracks[0].id);
        assert_eq!(restored.tracks[0].track_type, original.tracks[0].track_type);
        assert_eq!(restored.patterns[0].id(), original.patterns[0].id());
    }

    #[test]
    fn load_from_json_string() {
        let json = r#"{
            "version": "0.1",
            "project": {"name":"Test","bpm":140.0,"time_signature":[4,4],"key":"A","scale":"minor"},
            "tracks": [],
            "patterns": [],
            "arrangement": []
        }"#;
        let p: Project = serde_json::from_str(json).unwrap();
        assert_eq!(p.project.bpm, 140.0);
        assert_eq!(p.project.name, "Test");
    }
}

#[cfg(test)]
mod sampler_variant_asset_tests {
    use super::*;
    #[test]
    fn alternate_take_paths_resolve_and_affect_asset_identity() {
        let dir=std::env::temp_dir().join(format!("daw-take-assets-{}-{}",std::process::id(),TEMP_ID.fetch_add(1,std::sync::atomic::Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("base.wav"),b"base").unwrap();
        std::fs::write(dir.join("take.wav"),b"take one").unwrap();
        let mut p=Project::default_project();p.tracks.truncate(1);
        p.tracks[0].instrument=serde_json::from_value(serde_json::json!({"type":"sampler","pads":[{"pitch":36,"path":"base.wav","variants":[{"path":"take.wav","min_velocity":1,"max_velocity":127}]}]})).unwrap();
        let resolved=resolve_assets(&p,&dir,Path::new("/missing"));
        let Instrument::Sampler { pads }=&resolved.tracks[0].instrument else { panic!("sampler") };
        assert_eq!(PathBuf::from(&pads[0].variants[0].path),dir.join("take.wav"));
        let first=asset_fingerprints(&resolved,Path::new("/missing.sf2")).unwrap();
        assert_eq!(first.len(),2);
        std::fs::write(dir.join("take.wav"),b"take two").unwrap();
        assert_ne!(first,asset_fingerprints(&resolved,Path::new("/missing.sf2")).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

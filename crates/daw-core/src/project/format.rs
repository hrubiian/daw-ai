use crate::project::Project;
use anyhow::Result;
use std::path::Path;

pub fn load(path: impl AsRef<Path>) -> Result<Project> {
    let content = std::fs::read_to_string(path.as_ref())?;
    let project = serde_json::from_str(&content)?;
    Ok(project)
}

pub fn save(project: &Project, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let tmp_name = format!("{}.tmp", path.file_name().unwrap_or_default().to_string_lossy());
    let tmp = path.with_file_name(tmp_name);
    let content = serde_json::to_string_pretty(project)?;
    std::fs::write(&tmp, &content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::project::Project;

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

//! Install a Flow Music export in the local plugin library.

use anyhow::{ensure, Context, Result};
use daw_core::audio::{
    clap::ClapPluginDesc,
    flow::{self, PluginExport},
};
use std::{fs, io::Write, path::Path};

pub fn import_plugin(source: &Path) -> Result<ClapPluginDesc> {
    let root = flow::library_dir().context("Could not locate the FMX plugin library")?;
    import_into(source, &root)
}

fn import_into(source: &Path, root: &Path) -> Result<ClapPluginDesc> {
    ensure!(
        flow::is_fmx(source),
        "Choose a Flow Music plugin export (.fmx)"
    );
    let bytes = flow::read(source).with_context(|| format!("Read {}", source.display()))?;
    let export = PluginExport::parse(&bytes)?;
    export.validate_audio()?;
    PluginExport::validate_editor(&bytes)?;

    fs::create_dir_all(root).with_context(|| format!("Create {}", root.display()))?;
    let root = root.canonicalize()?;
    let path = root.join(format!("{}.fmx", export.id()));
    let plugin = export.descriptor(&path)?;
    if !fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file())
        || !flow::read(&path).is_ok_and(|existing| existing == bytes)
    {
        let mut temp = tempfile::NamedTempFile::new_in(root)?;
        temp.write_all(&bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(&path)
            .with_context(|| format!("Install FMX plugin at {}", path.display()))?;
    }
    Ok(plugin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_core::audio::clap::ClapInstance;

    // ABI 1 fixture with one parameter and no reset export. It writes the
    // parameter to the first sample of each channel while a note is held.
    const DSP: &str = concat!(
        "H4sIAAAAAAACA1WMOw7CMBBEZ23HkTBcgAoBFSegQz4An4raATnIhR1kh4ICOYfgJJwQk46V",
        "Zt+MtDtokicANK8MKBt6wbCcYaioyr8Br2U5YFxUnMCkpEwaqt7Xs5vt9cWdbUyuC8C05MPDn5",
        "rY+AQSLri+PISut8dQakbTthD1PXZXmxIq6a3v4pNhspMCmtS4GJQsXI9AwRILpodBbBhW+Ly3DAv+n9UXXhCNfcoAAAA="
    );
    // Stereo effect without MIDI exports. It multiplies the first sample in
    // each input block by the gain and writes it to its corresponding output.
    const EFFECT_DSP: &str = concat!(
        "H4sIAAAAAAACA1WKMQ4CIRRE539AE9HCG1BZ2Jts7LiAsbIGDTEULAZ2CwuzewgPLNo5xUzey8DX",
        "RABoKxxocvSCU9M3EEo2z0oQeHlUm3sY7DVeQqkx98C68WlMZ198qiAZ+zi046PkW6gVYpFCyuXJ",
        "WB2khCX9K4bewbDBnmHnWbZ5dwwjDP0b/QFntDBymgAAAA=="
    );
    const HTML: &str =
        "H4sIAAAAAAACA7NRTMlPLqksSFXIKMnNsbOBkqmJKXY2+hAqKT+l0i4ktbjERh/MBIqDFAEAlfhFfDoAAAA=";

    fn export() -> serde_json::Value {
        serde_json::json!({
            "type": "vst_instrument",
            "name": "Flow Test Synth",
            "wasm": DSP,
            "html": HTML,
            "metadata": r#"{"params":[{"id":0,"name":"Gain","min":0,"max":1,"default":0.25}]}"#
        })
    }

    #[test]
    fn imports_once_and_restores_plugin_parameters() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Test Synth.FMX");
        let library = dir.path().join("plugins");
        fs::write(&source, export().to_string()).unwrap();
        let desc = import_into(&source, &library).unwrap();
        let duplicate = import_into(&source, &library).unwrap();
        assert_eq!(desc.name, "Flow Test Synth");
        assert_eq!(desc.id, duplicate.id);
        assert_eq!(desc.path, duplicate.path);
        assert_eq!(fs::read_dir(&library).unwrap().count(), 1);
        fs::remove_file(source).unwrap();

        let mut synth = ClapInstance::load(&desc.path, &desc.id, 48_000, 128).unwrap();
        assert_eq!(synth.handle.param_infos[0].default, 0.25);
        let mut left = [0.0; 128];
        let mut right = [0.0; 128];
        let processor = synth.processor.as_mut().unwrap();
        processor.process(&mut left, &mut right);
        assert_eq!(left, [0.0; 128]);
        processor.queue_note_on(60, 100);
        processor.process(&mut left, &mut right);
        assert_eq!((left[0], right[0]), (0.25, 0.25));
        processor.queue_param(0, 0.75);
        processor.process(&mut left, &mut right);
        assert_eq!((left[0], right[0]), (0.75, 0.75));
        processor.queue_note_off(60);
        processor.process(&mut left, &mut right);
        assert_eq!(left, [0.0; 128]);
        let state = synth.handle.collect_state().unwrap();
        synth.shutdown();

        let mut reopened = ClapInstance::load(&desc.path, &desc.id, 44_100, 128).unwrap();
        reopened.handle.load_state(&state).unwrap();
        assert_eq!(reopened.handle.param_value(0), Some(0.75));
        reopened.shutdown();
    }

    #[test]
    fn imported_effect_processes_both_input_channels_across_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Test Gain.fmx");
        let mut data = export();
        data["type"] = "vst_effect".into();
        data["name"] = "Test Gain".into();
        data["wasm"] = EFFECT_DSP.into();
        fs::write(&source, data.to_string()).unwrap();
        let desc = import_into(&source, &dir.path().join("plugins")).unwrap();
        assert!(desc.is_effect && !desc.is_instrument);

        let mut effect = ClapInstance::load(&desc.path, &desc.id, 48_000, 2049).unwrap();
        let processor = effect.processor.as_mut().unwrap();
        let (mut left, mut right) = ([0.5; 2049], [-0.25; 2049]);
        processor.process(&mut left, &mut right);
        for start in [0, 1024, 2048] {
            assert_eq!((left[start], right[start]), (0.125, -0.0625));
        }
        processor.queue_param(0, 0.5);
        processor.process(&mut left, &mut right);
        assert_eq!((left[0], right[0]), (0.0625, -0.03125));
        effect.shutdown();
    }

    #[test]
    fn rejects_incompatible_exports_before_installing() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Broken.fmx");
        let library = dir.path().join("plugins");
        for (key, value) in [
            ("type", "unsupported"),
            ("metadata", r#"{"params":[]}"#),
            ("html", "invalid"),
        ] {
            let mut data = export();
            data[key] = value.into();
            fs::write(&source, data.to_string()).unwrap();
            assert!(
                import_into(&source, &library).is_err(),
                "accepted invalid {key}"
            );
            assert!(!library.exists());
        }
    }
}

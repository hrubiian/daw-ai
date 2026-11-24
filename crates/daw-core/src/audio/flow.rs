//! Flow Music exports, hosted as in-process CLAP plugins.

mod dsp;
mod plugin;

use crate::{audio::clap::ClapPluginDesc, project::format::digest};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use clack_host::prelude::PluginEntry;
use flate2::read::MultiGzDecoder;
use serde::Deserialize;
use std::{
    ffi::CString,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

pub const ID_PREFIX: &str = "local.daw-ai.fmx.";
const MAX_EXPORT_BYTES: usize = 4 * 1024 * 1024;
const MAX_DSP_BYTES: usize = 1024 * 1024;
const MAX_EDITOR_BYTES: usize = 8 * 1024 * 1024;
const MAX_PARAMS: usize = 256;

#[derive(Deserialize)]
struct Export {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    wasm: String,
    metadata: String,
    html: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PluginKind {
    Instrument,
    Effect,
}

#[derive(Deserialize)]
struct Metadata {
    params: Vec<Parameter>,
}

#[derive(Deserialize)]
struct Parameter {
    id: usize,
    name: String,
    min: f32,
    max: f32,
    default: f32,
    #[serde(default)]
    label: String,
    #[serde(default)]
    stepped: bool,
    #[serde(default)]
    choices: Vec<String>,
    #[serde(default = "automatable")]
    automatable: bool,
}

fn automatable() -> bool {
    true
}

pub struct PluginExport {
    id: String,
    name: String,
    kind: PluginKind,
    params: Vec<Parameter>,
    wasm: Vec<u8>,
}

impl PluginExport {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let export = Self::read_export(bytes)?;
        let kind = match export.kind.as_str() {
            "vst_instrument" => PluginKind::Instrument,
            "vst_effect" => PluginKind::Effect,
            _ => anyhow::bail!("This FMX file must contain a Flow Music instrument or effect"),
        };
        let Metadata { params } =
            serde_json::from_str(&export.metadata).context("Invalid FMX parameter metadata")?;
        ensure!(params.len() <= MAX_PARAMS, "Too many FMX parameters");
        for (index, p) in params.iter().enumerate() {
            ensure!(
                p.id == index
                    && !p.name.trim().is_empty()
                    && !p.name.contains('\0')
                    && p.min.is_finite()
                    && p.max.is_finite()
                    && (p.min..=p.max).contains(&p.default)
                    && (!p.stepped || [p.min, p.max, p.default].iter().all(|v| v.fract() == 0.0)),
                "Invalid FMX definition for parameter {index}"
            );
        }
        Ok(Self {
            id: format!("{ID_PREFIX}{}", digest(bytes)),
            name: export.name,
            kind,
            params,
            wasm: decompress(&export.wasm, MAX_DSP_BYTES).context("Invalid FMX DSP payload")?,
        })
    }

    fn read_export(bytes: &[u8]) -> Result<Export> {
        ensure!(bytes.len() <= MAX_EXPORT_BYTES, "FMX file is too large");
        let export: Export =
            serde_json::from_slice(bytes).context("Invalid Flow Music FMX file")?;
        ensure!(
            !export.name.trim().is_empty() && !export.name.chars().any(char::is_control),
            "Invalid FMX plugin name"
        );
        Ok(export)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn descriptor(&self, path: &Path) -> Result<ClapPluginDesc> {
        Ok(ClapPluginDesc {
            path: path
                .to_str()
                .context("FMX library path must be valid UTF-8")?
                .into(),
            id: self.id.clone(),
            name: self.name.clone(),
            vendor: "Flow Music".into(),
            features: Vec::new(),
            is_instrument: self.kind == PluginKind::Instrument,
            is_effect: self.kind == PluginKind::Effect,
        })
    }

    pub fn validate_audio(&self) -> Result<()> {
        let mut dsp = dsp::Dsp::new(&self.wasm, self.kind, self.params.len(), 48_000.0)
            .context("Unsupported Flow Music DSP")?;
        let params: Vec<_> = self.params.iter().map(|p| p.default).collect();
        let mut left = [0.0; 128];
        let mut right = [0.0; 128];
        if self.kind == PluginKind::Effect {
            let signal = std::array::from_fn::<_, 128, _>(|i| 0.25 * (i as f32 * 0.11).sin());
            dsp.render(&params, Some([&signal, &signal]), &mut left, &mut right)?;
        } else {
            dsp.render(&params, None, &mut left, &mut right)?;
            dsp.note_on(60, 100)?;
            dsp.render(&params, None, &mut left, &mut right)?;
            dsp.note_off(60)?;
        }
        Ok(())
    }

    pub fn validate_editor(bytes: &[u8]) -> Result<()> {
        let html = decompress(&Self::read_export(bytes)?.html, MAX_EDITOR_BYTES)
            .context("Invalid FMX editor")?;
        let html = std::str::from_utf8(&html).context("FMX editor must be UTF-8 HTML")?;
        ensure!(!html.trim().is_empty(), "The FMX editor is empty");
        Ok(())
    }
}

fn decompress(encoded: &str, limit: usize) -> Result<Vec<u8>> {
    let compressed = STANDARD.decode(encoded)?;
    let mut bytes = Vec::new();
    MultiGzDecoder::new(compressed.as_slice())
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "FMX payload is too large");
    Ok(bytes)
}

pub fn read(path: &Path) -> Result<Vec<u8>> {
    ensure!(path.metadata()?.is_file(), "Select an FMX file");
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_EXPORT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_EXPORT_BYTES, "FMX file is too large");
    Ok(bytes)
}

pub fn is_fmx(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("fmx"))
}

pub fn library_dir() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("DAW_FLOW_PLUGIN_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return Some(path);
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
        })?;
    Some(data.join("daw-ai/flow-plugins"))
}

pub(super) fn load_entry(path: &Path) -> Result<PluginEntry> {
    let path = CString::new(path.to_str().context("FMX path must be valid UTF-8")?)?;
    PluginEntry::load_from_clack::<plugin::FlowEntry>(&path)
        .context("Could not load the FMX plugin")
}

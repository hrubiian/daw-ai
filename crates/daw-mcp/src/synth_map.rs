//! Curated musical controls for Surge XT 1.3 ("filter_cutoff", "amp_decay"
//! …), each naming the Surge parameter it drives. Claude designs sounds
//! through these instead of Surge's 775 raw, partly generic parameters.
//! The map holds parameter names only (facts about the installed plugin).

use daw_core::audio::clap::ClapParamInfo;
use serde::Deserialize;
use std::sync::OnceLock;

/// One musical control and the Surge parameter (by reported name) behind it.
#[derive(Deserialize, Clone, Debug)]
pub struct Control {
    pub name: String,
    pub surge: String,
    pub group: String,
    pub help: String,
}

/// The Surge XT 1.3 control map (`data/surge-xt-1.3.json`).
pub fn surge_controls() -> &'static [Control] {
    static MAP: OnceLock<Vec<Control>> = OnceLock::new();
    MAP.get_or_init(|| {
        serde_json::from_str(include_str!("../data/surge-xt-1.3.json")).expect("valid surge-xt-1.3.json")
    })
}

/// Pair each control with its parameter id in `infos`; the second list
/// names Surge parameters that weren't found (e.g. another Surge version).
pub fn resolve(controls: &[Control], infos: &[ClapParamInfo]) -> (Vec<(Control, u32)>, Vec<String>) {
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for c in controls {
        match infos.iter().find(|i| i.name == c.surge) {
            Some(i) => found.push((c.clone(), i.id)),
            None => missing.push(c.surge.clone()),
        }
    }
    (found, missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_core::audio::clap::ClapParamInfo;

    fn info(id: u32, name: &str) -> ClapParamInfo {
        ClapParamInfo { id, name: name.into(), min: 0.0, max: 1.0, default: 0.0, stepped: false, readonly: false }
    }

    #[test]
    fn controls_parse_and_are_unique() {
        let c = surge_controls();
        assert!(c.len() >= 35, "only {} controls", c.len());
        let names: std::collections::HashSet<_> = c.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names.len(), c.len(), "duplicate control names");
        assert!(c.iter().all(|x| !x.help.is_empty() && !x.group.is_empty()));
    }

    #[test]
    fn resolve_maps_ids_and_reports_missing() {
        let all = surge_controls();
        let pick: Vec<Control> = all.iter().filter(|c| c.name == "filter_cutoff" || c.name == "amp_attack").cloned().collect();
        let (found, missing) = resolve(&pick, &[info(7, "A Filter 1 Cutoff")]);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].0.name.as_str(), found[0].1), ("filter_cutoff", 7));
        assert_eq!(missing, vec!["A Amp EG Attack".to_string()]);
    }

    #[test]
    fn all_controls_exist_in_surge_1_3() {
        let path = "/usr/lib/clap/Surge XT.clap";
        if !std::path::Path::new(path).exists() {
            eprintln!("skip: Surge XT not installed");
            return;
        }
        let mut inst = daw_core::audio::clap::ClapInstance::load(path, "org.surge-synth-team.surge-xt", 48_000, 256).unwrap();
        let (found, missing) = resolve(surge_controls(), &inst.handle.param_infos);
        assert!(missing.is_empty(), "missing in Surge: {missing:?}");
        assert_eq!(found.len(), surge_controls().len());
        if let Some(p) = inst.processor.take() { inst.handle.deactivate(p); }
    }
}

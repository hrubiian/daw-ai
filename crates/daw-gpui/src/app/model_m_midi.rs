use super::DawView;
use base64::Engine as _;
use daw_core::audio::clap::ClapParamInfo;
use daw_core::project::{ClapSlot, Instrument};
use daw_core::sequencer::SequencerCommand;
use daw_io::midi::KeyLabControl;
use std::collections::BTreeMap;

pub(super) const PLUGIN_ID: &str = "local.daw-ai.flow.model-m-v4";

pub(super) struct Control {
    id: u32,
    position: Position,
}

enum Position {
    Relative(i8),
    Absolute(f64),
}

impl Control {
    pub(super) fn keylab(control: KeyLabControl) -> Option<Self> {
        Some(match control {
            KeyLabControl::Encoder { index, delta } => Self {
                id: *[1, 2, 3, 16, 17, 18, 15, 14].get(index as usize)?,
                position: Position::Relative(delta),
            },
            KeyLabControl::Fader { index, position } => Self {
                id: *[4, 5, 6, 7, 19, 20, 21, 22, 0].get(index as usize)?,
                position: Position::Absolute(f64::from(position) / 16383.0),
            },
        })
    }

    pub(super) fn midi_cc(cc: u8, value: u8) -> Option<Self> {
        let id = match cc {
            74 => 1,     // Cutoff
            71 => 2,     // Resonance
            76 => 16,    // LFO rate
            77 => 17,    // LFO to cutoff
            73 => 4,     // Amp attack
            75 => 5,     // Amp decay
            79 => 6,     // Amp sustain
            72 => 7,     // Amp release
            7 | 85 => 0, // Volume
            _ => return None,
        };
        Some(Self {
            id,
            position: Position::Absolute(f64::from(value) / 127.0),
        })
    }

    fn value(&self, info: &ClapParamInfo, current: f64) -> f64 {
        let min = info.min;
        let max = info.max;
        let current = current.clamp(min, max);
        let to_normalized = |value: f64| match self.id {
            1 => (value / min).ln() / (max / min).ln(),
            4 | 5 | 7 | 19 | 20 | 22 => ((value - min) / (max - min)).sqrt(),
            _ => (value - min) / (max - min),
        };
        let norm = match self.position {
            Position::Relative(delta) => to_normalized(current) + f64::from(delta) / 150.0,
            Position::Absolute(norm) => norm,
        }
        .clamp(0.0, 1.0);
        match self.id {
            1 => min * (max / min).powf(norm),
            4 | 5 | 7 | 19 | 20 | 22 => min + (max - min) * norm.powi(2),
            _ => min + (max - min) * norm,
        }
    }
}

impl DawView {
    fn selected_model_m(&self) -> Option<&ClapSlot> {
        let id = self.midi_track.as_deref()?;
        let track = self.st.project.tracks.iter().find(|track| track.id == id)?;
        match &track.instrument {
            Instrument::Clap(slot) if slot.id == PLUGIN_ID => Some(slot),
            _ => None,
        }
    }

    /// Collect each encoder tick in order while committing one edit per UI
    /// update. The browser follows the same project parameters as the FX dock.
    pub(super) fn collect_model_m_control(
        &self,
        control: Control,
        updates: &mut BTreeMap<u32, f64>,
    ) -> bool {
        let Some(slot) = self.selected_model_m() else {
            return false;
        };
        let Some(info) = self
            .st
            .clap_meta
            .get(&slot.uid)
            .and_then(|(_, infos)| infos.iter().find(|info| info.id == control.id))
            .filter(|info| !info.readonly && info.min < info.max)
        else {
            return false;
        };
        let current = updates
            .get(&control.id)
            .copied()
            .or_else(|| slot.params.get(&control.id.to_string()).copied())
            .or_else(|| saved_parameter(slot, control.id))
            .unwrap_or(info.default);
        let next = control.value(info, current);
        if (next - current).abs() > f64::EPSILON * info.max.abs().max(1.0) {
            updates.insert(control.id, next);
        }
        true
    }

    pub(super) fn apply_model_m_controls(&mut self, updates: BTreeMap<u32, f64>) {
        if updates.is_empty() {
            return;
        }
        let Some(slot) = self.selected_model_m() else {
            return;
        };
        let uid = slot.uid.clone();
        if let Some(session) = &self.host.session {
            let mut project = self.st.project.clone();
            for (id, value) in updates {
                project.set_clap_param(&uid, id, value);
            }
            if let Err(error) = session.submit(&project) {
                self.st.set_status(format!("KeyLab: {error}"));
                return;
            }
            self.st.project = project;
        } else {
            self.undo.record("KeyLab: Model-M", &self.st.project);
            for (param_id, value) in updates {
                self.st.project.set_clap_param(&uid, param_id, value);
                let _ = self.host.cmd_tx.try_send(SequencerCommand::SetClapParam {
                    uid: uid.clone(),
                    param_id,
                    value,
                });
            }
        }
    }
}

fn saved_parameter(slot: &ClapSlot, id: u32) -> Option<f64> {
    // Model-M v4 stores its 23 plain parameter values behind a four-byte tag.
    let blob = base64::engine::general_purpose::STANDARD
        .decode(slot.state.as_deref()?)
        .ok()?;
    if !blob.starts_with(b"MDM1") || blob.len() != 96 || id >= 23 {
        return None;
    }
    let start = 4 + id as usize * 4;
    let value = f32::from_le_bytes(blob[start..start + 4].try_into().ok()?);
    value.is_finite().then_some(value.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutoff_encoder_moves_in_both_directions_from_current_patch() {
        let info = ClapParamInfo {
            id: 1,
            name: "Cutoff".into(),
            min: 20.0,
            max: 20_000.0,
            default: 1800.0,
            stepped: false,
            readonly: false,
        };
        let raise = Control::keylab(KeyLabControl::Encoder { index: 0, delta: 3 }).unwrap();
        let lower = Control::keylab(KeyLabControl::Encoder {
            index: 0,
            delta: -3,
        })
        .unwrap();
        let raised = raise.value(&info, 1450.0);
        assert!(raised > 1450.0 && raised < 1800.0);
        assert!((lower.value(&info, raised) - 1450.0).abs() < 0.01);
        assert_eq!(raise.value(&info, 20_000.0), 20_000.0);
        assert_eq!(lower.value(&info, 20.0), 20.0);
    }
}

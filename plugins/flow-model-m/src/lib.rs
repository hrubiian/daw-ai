//! Local CLAP instrument wrapping the Model-M v4 Flow Music DSP.

mod dsp;

use clack_extensions::{audio_ports::*, note_ports::*, params::*, state::*};
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::events::Match;
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use dsp::{Dsp, MAX_BLOCK, PARAM_COUNT};
use serde::Deserialize;
use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{Read, Write as _};
use std::sync::atomic::{AtomicU32, Ordering};

const METADATA: &str = include_str!(concat!(env!("MODEL_M_PAYLOAD"), "/model-m.json"));
const STATE_VERSION: &[u8; 4] = b"MDM1";

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
    label: String,
    stepped: bool,
    choices: Vec<String>,
}

impl Parameter {
    fn constrain(&self, value: f64) -> Option<f32> {
        if !value.is_finite() {
            return None;
        }
        let value = (value as f32).clamp(self.min, self.max);
        Some(if self.stepped { value.round() } else { value })
    }
}

pub struct ModelShared {
    definitions: [Parameter; PARAM_COUNT],
    values: [AtomicU32; PARAM_COUNT],
}

impl PluginShared<'_> for ModelShared {}

impl ModelShared {
    fn get(&self, index: usize) -> f32 {
        f32::from_bits(self.values[index].load(Ordering::Relaxed))
    }

    fn parameter_event(&self, event: &UnknownEvent) {
        if let Some(CoreEventSpace::ParamValue(value)) = event.as_core_event() {
            let Some(id) = value.param_id() else { return };
            let index = id.get() as usize;
            if let Some(value) = self
                .parameter(index)
                .and_then(|p| p.constrain(value.value()))
            {
                self.values[index].store(value.to_bits(), Ordering::Relaxed);
            }
        }
    }

    fn parameter(&self, index: usize) -> Option<&Parameter> {
        self.definitions.get(index)
    }
}

pub struct ModelMain<'a>(&'a ModelShared);
impl<'a> PluginMainThread<'a, ModelShared> for ModelMain<'a> {}

pub struct ModelPlugin;

impl Plugin for ModelPlugin {
    type AudioProcessor<'a> = ModelProcessor<'a>;
    type Shared<'a> = ModelShared;
    type MainThread<'a> = ModelMain<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&ModelShared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginNotePorts>()
            .register::<PluginParams>()
            .register::<PluginState>();
    }
}

impl DefaultPluginFactory for ModelPlugin {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::*;
        PluginDescriptor::new("local.daw-ai.flow.model-m-v4", "Model-M Modular Synth")
            .with_features([INSTRUMENT, SYNTHESIZER, STEREO])
    }

    fn new_shared(_host: HostSharedHandle<'_>) -> Result<ModelShared, PluginError> {
        let invalid = || PluginError::Message("Invalid Model-M parameter metadata");
        let metadata: Metadata = serde_json::from_str(METADATA).map_err(|_| invalid())?;
        let definitions: [Parameter; PARAM_COUNT] =
            metadata.params.try_into().map_err(|_| invalid())?;
        for (index, parameter) in definitions.iter().enumerate() {
            if parameter.id != index
                || !parameter.min.is_finite()
                || !parameter.max.is_finite()
                || parameter.min > parameter.max
                || !(parameter.min..=parameter.max).contains(&parameter.default)
                || parameter.name.contains('\0')
            {
                return Err(invalid());
            }
        }
        let values =
            std::array::from_fn(|index| AtomicU32::new(definitions[index].default.to_bits()));
        Ok(ModelShared {
            definitions,
            values,
        })
    }

    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        shared: &'a ModelShared,
    ) -> Result<ModelMain<'a>, PluginError> {
        Ok(ModelMain(shared))
    }
}

pub struct ModelProcessor<'a> {
    dsp: Dsp,
    shared: &'a ModelShared,
    held: [bool; 128],
}

impl ModelProcessor<'_> {
    fn render(&mut self, left: &mut [f32], right: &mut [f32]) -> Result<(), PluginError> {
        let values = std::array::from_fn(|index| self.shared.get(index));
        for (left, right) in left.chunks_mut(MAX_BLOCK).zip(right.chunks_mut(MAX_BLOCK)) {
            self.dsp.render(&values, left, right).map_err(dsp_error)?;
        }
        Ok(())
    }

    fn release(&mut self, key: Match<u16>) -> Result<(), PluginError> {
        for (index, held) in self.held.iter_mut().enumerate() {
            if *held && key.matches(Match::Specific(index as u16)) {
                self.dsp.note_off(index as u8).map_err(dsp_error)?;
                *held = false;
            }
        }
        Ok(())
    }
}

fn dsp_error(_: wasmi::Error) -> PluginError {
    PluginError::Message("Model-M DSP execution failed")
}

impl<'a> PluginAudioProcessor<'a, ModelShared, ModelMain<'a>> for ModelProcessor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &mut ModelMain<'a>,
        shared: &'a ModelShared,
        config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        let dsp = Dsp::new(config.sample_rate as f32).map_err(|error| {
            eprintln!("Model-M activation: {error}");
            dsp_error(error)
        })?;
        Ok(Self {
            dsp,
            shared,
            held: [false; 128],
        })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        let mut output = audio
            .output_port(0)
            .ok_or(PluginError::Message("Missing Model-M output"))?;
        let mut channels = output
            .channels()?
            .into_f32()
            .ok_or(PluginError::Message("Expected f32 audio"))?;
        let mut channels = channels.iter_mut();
        let (Some(left), Some(right)) = (channels.next(), channels.next()) else {
            return Err(PluginError::Message("Expected two Model-M output channels"));
        };

        let mut cursor = 0;
        for event in events.input {
            let frame = (event.header().time() as usize).clamp(cursor, left.len());
            self.render(&mut left[cursor..frame], &mut right[cursor..frame])?;
            cursor = frame;
            match event.as_core_event() {
                Some(CoreEventSpace::NoteOn(note)) => {
                    if let Match::Specific(key @ 0..=127) = note.key() {
                        // CLAP treats zero-velocity note-ons as notes too.
                        let velocity = (note.velocity() * 127.0).round().clamp(1.0, 127.0);
                        self.dsp
                            .note_on(key as u8, velocity as u8)
                            .map_err(dsp_error)?;
                        self.held[key as usize] = true;
                    }
                }
                Some(CoreEventSpace::NoteOff(note)) => self.release(note.key())?,
                Some(CoreEventSpace::NoteChoke(note)) if note.key().is_all() => {
                    self.dsp.reset().map_err(dsp_error)?;
                    self.held.fill(false);
                }
                Some(CoreEventSpace::NoteChoke(note)) => self.release(note.key())?,
                Some(CoreEventSpace::ParamValue(_)) => self.shared.parameter_event(event),
                _ => {}
            }
        }
        self.render(&mut left[cursor..], &mut right[cursor..])?;
        Ok(ProcessStatus::Continue)
    }

    fn reset(&mut self) {
        let _ = self.dsp.reset();
        self.held.fill(false);
    }
}

impl PluginAudioPortsImpl for ModelMain<'_> {
    fn count(&mut self, is_input: bool) -> u32 {
        u32::from(!is_input)
    }

    fn get(&mut self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index == 0 && !is_input {
            writer.set(&AudioPortInfo {
                id: ClapId::new(0),
                name: b"Main",
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
}

impl PluginNotePortsImpl for ModelMain<'_> {
    fn count(&mut self, is_input: bool) -> u32 {
        u32::from(is_input)
    }

    fn get(&mut self, index: u32, is_input: bool, writer: &mut NotePortInfoWriter) {
        if index == 0 && is_input {
            writer.set(&NotePortInfo {
                id: ClapId::new(0),
                name: b"Notes",
                supported_dialects: NoteDialect::Clap.into(),
                preferred_dialect: Some(NoteDialect::Clap),
            });
        }
    }
}

impl PluginMainThreadParams for ModelMain<'_> {
    fn count(&mut self) -> u32 {
        PARAM_COUNT as u32
    }

    fn get_info(&mut self, index: u32, writer: &mut ParamInfoWriter) {
        if let Some(param) = self.0.parameter(index as usize) {
            let mut flags = ParamInfoFlags::IS_AUTOMATABLE | ParamInfoFlags::REQUIRES_PROCESS;
            if param.stepped {
                flags |= ParamInfoFlags::IS_STEPPED;
            }
            writer.set(&ParamInfo {
                id: ClapId::new(index),
                flags,
                cookie: Default::default(),
                name: param.name.as_bytes(),
                module: b"",
                min_value: param.min.into(),
                max_value: param.max.into(),
                default_value: param.default.into(),
            });
        }
    }

    fn get_value(&mut self, id: ClapId) -> Option<f64> {
        self.0
            .values
            .get(id.get() as usize)
            .map(|v| f32::from_bits(v.load(Ordering::Relaxed)).into())
    }

    fn value_to_text(
        &mut self,
        id: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter,
    ) -> std::fmt::Result {
        let parameter = self.0.parameter(id.get() as usize).ok_or(std::fmt::Error)?;
        if parameter.stepped {
            return match parameter
                .choices
                .get((value - f64::from(parameter.min)) as usize)
            {
                Some(label) => write!(writer, "{label}"),
                None => write!(writer, "{value:.0}"),
            };
        }
        if parameter.label.is_empty() {
            write!(writer, "{value:.2}")
        } else {
            write!(writer, "{value:.2} {}", parameter.label)
        }
    }

    fn text_to_value(&mut self, id: ClapId, text: &CStr) -> Option<f64> {
        let parameter = self.0.parameter(id.get() as usize)?;
        let text = text.to_str().ok()?.trim();
        if let Some(index) = parameter
            .choices
            .iter()
            .position(|choice| choice.eq_ignore_ascii_case(text))
        {
            return Some(f64::from(parameter.min) + index as f64);
        }
        let number = text.strip_suffix(&parameter.label).unwrap_or(text).trim();
        parameter.constrain(number.parse().ok()?).map(f64::from)
    }

    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        for event in input {
            self.0.parameter_event(event);
        }
    }
}

impl PluginAudioProcessorParams for ModelProcessor<'_> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        for event in input {
            self.shared.parameter_event(event);
        }
    }
}

impl PluginStateImpl for ModelMain<'_> {
    fn save(&mut self, output: &mut OutputStream) -> Result<(), PluginError> {
        output.write_all(STATE_VERSION)?;
        for index in 0..PARAM_COUNT {
            output.write_all(&self.0.get(index).to_le_bytes())?;
        }
        Ok(())
    }

    fn load(&mut self, input: &mut InputStream) -> Result<(), PluginError> {
        let invalid = || PluginError::Message("Invalid Model-M plugin state");
        let mut bytes = [0; 4];
        input.read_exact(&mut bytes)?;
        if &bytes != STATE_VERSION {
            return Err(invalid());
        }
        let mut values = [0; PARAM_COUNT];
        for (bits, definition) in values.iter_mut().zip(&self.0.definitions) {
            input.read_exact(&mut bytes)?;
            let value = f32::from_le_bytes(bytes);
            *bits = definition
                .constrain(value.into())
                .ok_or_else(invalid)?
                .to_bits();
        }
        for (value, bits) in self.0.values.iter().zip(values) {
            value.store(bits, Ordering::Relaxed);
        }
        Ok(())
    }
}

clack_export_entry!(SinglePluginEntry<ModelPlugin>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_m_renders_notes_and_responds_to_velocity_and_gain() {
        let metadata: Metadata = serde_json::from_str(METADATA).unwrap();
        let mut params = std::array::from_fn(|index| metadata.params[index].default);
        let mut left = [0.0; MAX_BLOCK];
        let mut right = [0.0; MAX_BLOCK];

        let peak = |params: &[f32; PARAM_COUNT], velocity| {
            let mut dsp = Dsp::new(44_100.0).unwrap();
            let mut peak = 0.0_f32;
            let mut left = [0.0; MAX_BLOCK];
            let mut right = [0.0; MAX_BLOCK];
            dsp.note_on(69, velocity).unwrap();
            for _ in 0..4 {
                dsp.render(params, &mut left, &mut right).unwrap();
                assert_eq!(left, right);
                peak = peak.max(left.iter().fold(0.0_f32, |p, x| p.max(x.abs())));
            }
            peak
        };

        let mut dsp = Dsp::new(44_100.0).unwrap();
        dsp.render(&params, &mut left, &mut right).unwrap();
        assert!(left.iter().all(|sample| *sample == 0.0));
        let loud = peak(&params, 100);
        assert!(loud > 0.05 && peak(&params, 20) < loud * 0.6);
        params[0] = -36.0;
        assert!(peak(&params, 100) < loud * 0.1);
        dsp.note_on(69, 100).unwrap();
        for _ in 0..4 {
            dsp.render(&params, &mut left, &mut right).unwrap();
        }
        let held_peak = left.iter().fold(0.0_f32, |p, x| p.max(x.abs()));
        dsp.note_off(69).unwrap();
        for _ in 0..172 {
            dsp.render(&params, &mut left, &mut right).unwrap();
        }
        let release_peak = left.iter().fold(0.0_f32, |p, x| p.max(x.abs()));
        assert!(held_peak > 0.005 && release_peak < held_peak * 0.01);
        dsp.reset().unwrap();
        dsp.render(&params, &mut left, &mut right).unwrap();
        assert!(left.iter().all(|sample| *sample == 0.0));
    }
}

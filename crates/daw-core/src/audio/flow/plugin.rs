use super::{
    dsp::{Dsp, MAX_BLOCK},
    Parameter, PluginExport, PluginKind, MAX_PARAMS,
};
use clack_extensions::{audio_ports::*, note_ports::*, params::*, state::*};
use clack_plugin::entry::prelude::*;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::events::Match;
use clack_plugin::prelude::*;
use clack_plugin::process::audio::ChannelPair;
use clack_plugin::stream::{InputStream, OutputStream};
use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{Read, Write as _};
use std::ops::Range;
use std::sync::atomic::{AtomicU32, Ordering};

const STATE_VERSION: &[u8; 4] = b"FMX1";

pub(super) struct FlowEntry(PluginFactoryWrapper<FlowFactory>);

impl Entry for FlowEntry {
    fn new(path: Option<&CStr>) -> Result<Self, EntryLoadError> {
        let path = path
            .and_then(|path| path.to_str().ok())
            .ok_or(EntryLoadError)?;
        let bundle = super::read(std::path::Path::new(path))
            .and_then(|bytes| PluginExport::parse(&bytes))
            .map_err(|error| {
                eprintln!("FMX {path}: {error:#}");
                EntryLoadError
            })?;
        use clack_plugin::plugin::features::*;
        let descriptor = PluginDescriptor::new(bundle.id.as_str(), bundle.name.as_str())
            .with_vendor("Flow Music");
        let descriptor = match bundle.kind {
            PluginKind::Instrument => descriptor.with_features([INSTRUMENT, SYNTHESIZER, STEREO]),
            PluginKind::Effect => descriptor.with_features([AUDIO_EFFECT, STEREO]),
        };
        Ok(Self(PluginFactoryWrapper::new(FlowFactory {
            descriptor,
            bundle,
        })))
    }

    fn declare_factories<'a>(&'a self, builder: &mut EntryFactories<'a>) {
        builder.register_factory(&self.0);
    }
}

struct FlowFactory {
    descriptor: PluginDescriptor,
    bundle: PluginExport,
}

impl PluginFactoryImpl for FlowFactory {
    fn plugin_count(&self) -> u32 {
        1
    }

    fn plugin_descriptor(&self, index: u32) -> Option<&PluginDescriptor> {
        (index == 0).then_some(&self.descriptor)
    }

    fn create_plugin<'a>(&'a self, host: HostInfo<'a>, id: &CStr) -> Option<PluginInstance<'a>> {
        if self.descriptor.id() != Some(id) {
            return None;
        }
        Some(PluginInstance::new::<FlowPlugin>(
            host,
            &self.descriptor,
            |_| {
                Ok(FlowShared {
                    bundle: &self.bundle,
                    values: self
                        .bundle
                        .params
                        .iter()
                        .map(|p| AtomicU32::new(p.default.to_bits()))
                        .collect(),
                })
            },
            |_, shared| Ok(FlowMain(shared)),
        ))
    }
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

struct FlowShared<'a> {
    bundle: &'a PluginExport,
    values: Vec<AtomicU32>,
}

impl<'a> PluginShared<'a> for FlowShared<'a> {}

impl FlowShared<'_> {
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
        self.bundle.params.get(index)
    }
}

struct FlowMain<'a>(&'a FlowShared<'a>);
impl<'a> PluginMainThread<'a, FlowShared<'a>> for FlowMain<'a> {}

struct FlowPlugin;

impl Plugin for FlowPlugin {
    type AudioProcessor<'a> = FlowProcessor<'a>;
    type Shared<'a> = FlowShared<'a>;
    type MainThread<'a> = FlowMain<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&FlowShared<'_>>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginNotePorts>()
            .register::<PluginParams>()
            .register::<PluginState>();
    }
}

struct FlowProcessor<'a> {
    dsp: Dsp,
    shared: &'a FlowShared<'a>,
    held: [bool; 128],
    muted_after_reset: bool,
}

impl FlowProcessor<'_> {
    fn render(
        &mut self,
        input: Option<[&[f32]; 2]>,
        left: &mut [f32],
        right: &mut [f32],
        frames: Range<usize>,
    ) -> Result<(), PluginError> {
        if frames.is_empty() {
            return Ok(());
        }
        let count = self.shared.values.len();
        let mut values = [0.0; MAX_PARAMS];
        for (index, value) in values[..count].iter_mut().enumerate() {
            *value = self.shared.get(index);
        }
        for start in frames.clone().step_by(MAX_BLOCK) {
            let block = start..(start + MAX_BLOCK).min(frames.end);
            let input = input.map(|channels| channels.map(|samples| &samples[block.clone()]));
            let left = &mut left[block.clone()];
            let right = &mut right[block];
            self.dsp
                .render(&values[..count], input, left, right)
                .map_err(dsp_error)?;
            if self.muted_after_reset {
                left.fill(0.0);
                right.fill(0.0);
            }
        }
        Ok(())
    }

    fn reset_notes(&mut self) -> Result<(), PluginError> {
        if self.dsp.reset().map_err(dsp_error)? {
            self.held.fill(false);
        } else {
            // Some ABI 1 instruments only support note-off. Let their release
            // tails advance silently after a host reset.
            self.release(Match::All)?;
            self.muted_after_reset = true;
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
    PluginError::Message("FMX DSP execution failed")
}

impl<'a> PluginAudioProcessor<'a, FlowShared<'a>, FlowMain<'a>> for FlowProcessor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &mut FlowMain<'a>,
        shared: &'a FlowShared<'a>,
        config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        let dsp = Dsp::new(
            &shared.bundle.wasm,
            shared.bundle.kind,
            shared.values.len(),
            config.sample_rate as f32,
        )
        .map_err(|error| {
            eprintln!("FMX activation: {error}");
            dsp_error(error)
        })?;
        Ok(Self {
            dsp,
            shared,
            held: [false; 128],
            muted_after_reset: false,
        })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        let mut port = audio
            .port_pair(0)
            .ok_or(PluginError::Message("Missing FMX audio port"))?;
        let mut channels = port
            .channels()?
            .into_f32()
            .ok_or(PluginError::Message("Expected f32 audio"))?;
        let mut channels = channels.iter_mut();
        use ChannelPair::{InputOutput, OutputOnly};
        let (input, left, right) = match (self.shared.bundle.kind, channels.next(), channels.next())
        {
            (PluginKind::Instrument, Some(OutputOnly(left)), Some(OutputOnly(right))) => {
                (None, left, right)
            }
            (PluginKind::Effect, Some(InputOutput(in_l, left)), Some(InputOutput(in_r, right))) => {
                (Some([in_l, in_r]), left, right)
            }
            _ => return Err(PluginError::Message("Unexpected FMX audio port layout")),
        };

        let mut cursor = 0;
        for event in events.input {
            let frame = (event.header().time() as usize).clamp(cursor, left.len());
            self.render(input, left, right, cursor..frame)?;
            cursor = frame;
            match event.as_core_event() {
                Some(CoreEventSpace::ParamValue(_)) => self.shared.parameter_event(event),
                _ if input.is_some() => {}
                Some(CoreEventSpace::NoteOn(note)) => {
                    if let Match::Specific(key @ 0..=127) = note.key() {
                        // CLAP treats zero-velocity note-ons as notes too.
                        let velocity = (note.velocity() * 127.0).round().clamp(1.0, 127.0);
                        self.dsp
                            .note_on(key as u8, velocity as u8)
                            .map_err(dsp_error)?;
                        self.held[key as usize] = true;
                        self.muted_after_reset = false;
                    }
                }
                Some(CoreEventSpace::NoteOff(note)) => self.release(note.key())?,
                Some(CoreEventSpace::NoteChoke(note)) if note.key().is_all() => {
                    self.reset_notes()?;
                }
                Some(CoreEventSpace::NoteChoke(note)) => self.release(note.key())?,
                _ => {}
            }
        }
        self.render(input, left, right, cursor..left.len())?;
        Ok(ProcessStatus::Continue)
    }

    fn reset(&mut self) {
        if self.shared.bundle.kind == PluginKind::Instrument {
            let _ = self.reset_notes();
        } else {
            let _ = self.dsp.reset();
        }
    }
}

impl PluginAudioPortsImpl for FlowMain<'_> {
    fn count(&mut self, is_input: bool) -> u32 {
        u32::from(!is_input || self.0.bundle.kind == PluginKind::Effect)
    }

    fn get(&mut self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index == 0 && PluginAudioPortsImpl::count(self, is_input) != 0 {
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

impl PluginNotePortsImpl for FlowMain<'_> {
    fn count(&mut self, is_input: bool) -> u32 {
        u32::from(is_input && self.0.bundle.kind == PluginKind::Instrument)
    }

    fn get(&mut self, index: u32, is_input: bool, writer: &mut NotePortInfoWriter) {
        if index == 0 && PluginNotePortsImpl::count(self, is_input) != 0 {
            writer.set(&NotePortInfo {
                id: ClapId::new(0),
                name: b"Notes",
                supported_dialects: NoteDialect::Clap.into(),
                preferred_dialect: Some(NoteDialect::Clap),
            });
        }
    }
}

impl PluginMainThreadParams for FlowMain<'_> {
    fn count(&mut self) -> u32 {
        self.0.values.len() as u32
    }

    fn get_info(&mut self, index: u32, writer: &mut ParamInfoWriter) {
        if let Some(param) = self.0.parameter(index as usize) {
            let mut flags = ParamInfoFlags::REQUIRES_PROCESS;
            if param.automatable {
                flags |= ParamInfoFlags::IS_AUTOMATABLE;
            }
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

impl PluginAudioProcessorParams for FlowProcessor<'_> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        for event in input {
            self.shared.parameter_event(event);
        }
    }
}

impl PluginStateImpl for FlowMain<'_> {
    fn save(&mut self, output: &mut OutputStream) -> Result<(), PluginError> {
        output.write_all(STATE_VERSION)?;
        for index in 0..self.0.values.len() {
            output.write_all(&self.0.get(index).to_le_bytes())?;
        }
        Ok(())
    }

    fn load(&mut self, input: &mut InputStream) -> Result<(), PluginError> {
        let invalid = || PluginError::Message("Invalid FMX plugin state");
        let mut bytes = [0; 4];
        input.read_exact(&mut bytes)?;
        if &bytes != STATE_VERSION {
            return Err(invalid());
        }
        let mut values = [0; MAX_PARAMS];
        for (bits, definition) in values.iter_mut().zip(&self.0.bundle.params) {
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

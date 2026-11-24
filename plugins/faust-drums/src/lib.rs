mod faust;

use clack_extensions::{audio_ports::*, note_ports::*, params::*, state::*};
use clack_plugin::events::{spaces::CoreEventSpace, Match};
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use faust::{DrumDsp, FaustDsp};
use std::{
    ffi::CStr,
    fmt::Write as _,
    io::{Read, Write as _},
    sync::atomic::{AtomicU32, Ordering},
};

const FIRST_NOTE: u16 = 36;
const VOICES: usize = 6;
const BLOCK: usize = 256;
const LEVEL: u32 = 1;
const DEFAULT_LEVEL: f32 = 0.85;

struct DrumPlugin;
struct Shared {
    level: AtomicU32,
}
struct Main<'a>(&'a Shared);

impl PluginShared<'_> for Shared {}
impl<'a> PluginMainThread<'a, Shared> for Main<'a> {}

impl Shared {
    fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    fn set_level(&self, value: f64) {
        if value.is_finite() {
            self.level
                .store((value.clamp(0.0, 1.0) as f32).to_bits(), Ordering::Relaxed);
        }
    }

    fn parameter_event(&self, event: &UnknownEvent) {
        if let Some(CoreEventSpace::ParamValue(change)) = event.as_core_event() {
            if change.param_id() == ClapId::new(LEVEL) {
                self.set_level(change.value());
            }
        }
    }

    fn flush_parameters(&self, input: &InputEvents) {
        for event in input {
            self.parameter_event(event);
        }
    }
}

impl Plugin for DrumPlugin {
    type Shared<'a> = Shared;
    type MainThread<'a> = Main<'a>;
    type AudioProcessor<'a> = Processor<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _: Option<&Shared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginNotePorts>()
            .register::<PluginParams>()
            .register::<PluginState>();
    }
}

impl DefaultPluginFactory for DrumPlugin {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::{DRUM, INSTRUMENT, STEREO};
        PluginDescriptor::new("local.daw-ai.faust-drums", "West African Drum Kit")
            .with_features([INSTRUMENT, DRUM, STEREO])
    }

    fn new_shared(_: HostSharedHandle<'_>) -> Result<Shared, PluginError> {
        Ok(Shared {
            level: AtomicU32::new(DEFAULT_LEVEL.to_bits()),
        })
    }

    fn new_main_thread<'a>(
        _: HostMainThreadHandle<'a>,
        shared: &'a Shared,
    ) -> Result<Main<'a>, PluginError> {
        Ok(Main(shared))
    }
}

struct Processor<'a> {
    shared: &'a Shared,
    dsp: DrumDsp,
    pending_hits: [f32; VOICES],
    impulses: [[f32; BLOCK]; VOICES],
}

impl Processor<'_> {
    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let level = self.shared.level();
        for (left, right) in left.chunks_mut(BLOCK).zip(right.chunks_mut(BLOCK)) {
            let frames = left.len();
            for (input, hit) in self.impulses.iter_mut().zip(&mut self.pending_hits) {
                input[..frames].fill(0.0);
                input[0] = std::mem::take(hit);
            }
            let inputs = self.impulses.each_ref().map(|input| &input[..frames]);
            self.dsp.compute(frames as i32, &inputs, &mut [left, right]);
            for sample in left.iter_mut().chain(right) {
                *sample *= level;
            }
        }
    }
}

impl<'a> PluginAudioProcessor<'a, Shared, Main<'a>> for Processor<'a> {
    fn activate(
        _: HostAudioProcessorHandle<'a>,
        _: &mut Main<'a>,
        shared: &'a Shared,
        config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        let mut dsp = DrumDsp::new();
        dsp.init(config.sample_rate.round() as i32);
        if dsp.get_num_inputs() as usize != VOICES || dsp.get_num_outputs() != 2 {
            return Err(PluginError::Message(
                "Expected six Faust triggers and stereo output",
            ));
        }
        Ok(Self {
            shared,
            dsp,
            pending_hits: [0.0; VOICES],
            impulses: [[0.0; BLOCK]; VOICES],
        })
    }

    fn process(
        &mut self,
        _: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        let mut output = audio
            .output_port(0)
            .ok_or(PluginError::Message("Missing drum output"))?;
        let mut channels = output
            .channels()?
            .into_f32()
            .ok_or(PluginError::Message("Expected f32 audio"))?;
        let mut channels = channels.iter_mut();
        let (Some(left), Some(right)) = (channels.next(), channels.next()) else {
            return Err(PluginError::Message("Expected stereo drum output"));
        };
        let mut cursor = 0;
        for event in events.input {
            let frame = (event.header().time() as usize).clamp(cursor, left.len());
            self.render(&mut left[cursor..frame], &mut right[cursor..frame]);
            cursor = frame;
            match event.as_core_event() {
                Some(CoreEventSpace::NoteOn(note)) => {
                    if let Match::Specific(key) = note.key() {
                        let hit = key
                            .checked_sub(FIRST_NOTE)
                            .and_then(|voice| self.pending_hits.get_mut(usize::from(voice)));
                        if let Some(hit) = hit {
                            let velocity = note.velocity().clamp(1.0 / 127.0, 1.0) as f32;
                            *hit = hit.max(velocity);
                        }
                    }
                }
                Some(CoreEventSpace::NoteChoke(note)) if note.key().is_all() => self.reset(),
                Some(CoreEventSpace::ParamValue(_)) => self.shared.parameter_event(event),
                // One-shot envelopes continue after note-off.
                _ => {}
            }
        }
        self.render(&mut left[cursor..], &mut right[cursor..]);
        Ok(ProcessStatus::Continue)
    }

    fn reset(&mut self) {
        self.dsp.instance_clear();
        self.pending_hits.fill(0.0);
    }
}

impl PluginAudioPortsImpl for Main<'_> {
    fn count(&mut self, is_input: bool) -> u32 {
        u32::from(!is_input)
    }

    fn get(&mut self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index == 0 && !is_input {
            writer.set(&AudioPortInfo {
                id: ClapId::new(0),
                name: b"Drums",
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
}

impl PluginNotePortsImpl for Main<'_> {
    fn count(&mut self, is_input: bool) -> u32 {
        u32::from(is_input)
    }

    fn get(&mut self, index: u32, is_input: bool, writer: &mut NotePortInfoWriter) {
        if index == 0 && is_input {
            writer.set(&NotePortInfo {
                id: ClapId::new(0),
                name: b"Drum notes",
                supported_dialects: NoteDialect::Clap.into(),
                preferred_dialect: Some(NoteDialect::Clap),
            });
        }
    }
}

impl PluginMainThreadParams for Main<'_> {
    fn count(&mut self) -> u32 {
        1
    }

    fn get_info(&mut self, index: u32, writer: &mut ParamInfoWriter) {
        if index == 0 {
            writer.set(&ParamInfo {
                id: ClapId::new(LEVEL),
                flags: ParamInfoFlags::IS_AUTOMATABLE,
                cookie: Default::default(),
                name: b"Level",
                module: b"",
                min_value: 0.0,
                max_value: 1.0,
                default_value: f64::from(DEFAULT_LEVEL),
            });
        }
    }

    fn get_value(&mut self, id: ClapId) -> Option<f64> {
        (Some(id) == ClapId::new(LEVEL)).then(|| f64::from(self.0.level()))
    }

    fn value_to_text(
        &mut self,
        _: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter,
    ) -> std::fmt::Result {
        write!(writer, "{value:.2}")
    }

    fn text_to_value(&mut self, _: ClapId, text: &CStr) -> Option<f64> {
        text.to_str().ok()?.trim().parse().ok()
    }

    fn flush(&mut self, input: &InputEvents, _: &mut OutputEvents) {
        self.0.flush_parameters(input);
    }
}

impl PluginAudioProcessorParams for Processor<'_> {
    fn flush(&mut self, input: &InputEvents, _: &mut OutputEvents) {
        self.shared.flush_parameters(input);
    }
}

impl PluginStateImpl for Main<'_> {
    fn save(&mut self, output: &mut OutputStream) -> Result<(), PluginError> {
        output.write_all(&self.0.level().to_le_bytes())?;
        Ok(())
    }

    fn load(&mut self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut bytes = [0; 4];
        input.read_exact(&mut bytes)?;
        self.0.set_level(f64::from(f32::from_le_bytes(bytes)));
        Ok(())
    }
}

clack_export_entry!(SinglePluginEntry<DrumPlugin>);

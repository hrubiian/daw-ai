//! Hermetic CLAP test plugin: stereo gain with one param + state.
//! Adapted from clack's gain example (MIT/Apache-2.0). Used only by
//! daw-core's CLAP host tests — never shipped.

use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_extensions::{audio_ports::*, params::*};
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{Read, Write as _};
use std::sync::atomic::{AtomicU32, Ordering};

const PARAM_GAIN: u32 = 1;
const DEFAULT_GAIN: f32 = 1.0;

struct AtomicF32(AtomicU32);
impl AtomicF32 {
    fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
    fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

pub struct GainShared {
    gain: AtomicF32,
}
impl PluginShared<'_> for GainShared {}
impl GainShared {
    fn handle_event(&self, event: &UnknownEvent) {
        if let Some(CoreEventSpace::ParamValue(ev)) = event.as_core_event() {
            if ev.param_id() == ClapId::new(PARAM_GAIN) {
                self.gain.store((ev.value() as f32).clamp(0.0, 2.0));
            }
        }
    }
}

pub struct GainMainThread<'a> {
    shared: &'a GainShared,
}
impl<'a> PluginMainThread<'a, GainShared> for GainMainThread<'a> {}

pub struct GainPlugin;
impl Plugin for GainPlugin {
    type AudioProcessor<'a> = GainProcessor<'a>;
    type Shared<'a> = GainShared;
    type MainThread<'a> = GainMainThread<'a>;
    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&GainShared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginParams>()
            .register::<PluginState>();
    }
}

impl DefaultPluginFactory for GainPlugin {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::*;
        PluginDescriptor::new("test.gain", "DAW Test Gain").with_features([AUDIO_EFFECT, STEREO])
    }
    fn new_shared(_host: HostSharedHandle<'_>) -> Result<Self::Shared<'_>, PluginError> {
        Ok(GainShared { gain: AtomicF32::new(DEFAULT_GAIN) })
    }
    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        shared: &'a Self::Shared<'a>,
    ) -> Result<Self::MainThread<'a>, PluginError> {
        Ok(GainMainThread { shared })
    }
}

pub struct GainProcessor<'a> {
    shared: &'a GainShared,
}

impl<'a> PluginAudioProcessor<'a, GainShared, GainMainThread<'a>> for GainProcessor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &mut GainMainThread,
        shared: &'a GainShared,
        _audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        Ok(Self { shared })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        let mut port_pair = audio
            .port_pair(0)
            .ok_or(PluginError::Message("no port pair"))?;
        let mut channels = port_pair
            .channels()?
            .into_f32()
            .ok_or(PluginError::Message("expected f32"))?;

        let mut bufs = [None, None];
        for (pair, buf) in channels.iter_mut().zip(&mut bufs) {
            *buf = match pair {
                ChannelPair::InPlace(b) => Some(b),
                ChannelPair::InputOutput(i, o) => {
                    o.copy_from_slice(i);
                    Some(o)
                }
                _ => None,
            };
        }

        for batch in events.input.batch() {
            for event in batch.events() {
                self.shared.handle_event(event);
            }
            let g = self.shared.gain.load();
            for buf in bufs.iter_mut().flatten() {
                for s in buf.iter_mut() {
                    *s *= g;
                }
            }
        }
        Ok(ProcessStatus::ContinueIfNotQuiet)
    }
}

impl PluginAudioPortsImpl for GainMainThread<'_> {
    fn count(&mut self, _is_input: bool) -> u32 {
        1
    }
    fn get(&mut self, index: u32, _is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index == 0 {
            writer.set(&AudioPortInfo {
                id: ClapId::new(0),
                name: b"main",
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
}

impl PluginMainThreadParams for GainMainThread<'_> {
    fn count(&mut self) -> u32 {
        1
    }
    fn get_info(&mut self, index: u32, info: &mut ParamInfoWriter) {
        if index != 0 {
            return;
        }
        info.set(&ParamInfo {
            id: ClapId::new(PARAM_GAIN),
            flags: ParamInfoFlags::IS_AUTOMATABLE,
            cookie: Default::default(),
            name: b"Gain",
            module: b"",
            min_value: 0.0,
            max_value: 2.0,
            default_value: DEFAULT_GAIN as f64,
        });
    }
    fn get_value(&mut self, param_id: ClapId) -> Option<f64> {
        (param_id == ClapId::new(PARAM_GAIN)).then(|| self.shared.gain.load() as f64)
    }
    fn value_to_text(
        &mut self,
        _id: ClapId,
        value: f64,
        w: &mut ParamDisplayWriter,
    ) -> std::fmt::Result {
        write!(w, "{value:.2}")
    }
    fn text_to_value(&mut self, _id: ClapId, text: &CStr) -> Option<f64> {
        text.to_str().ok()?.trim().parse().ok()
    }
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        for event in input {
            self.shared.handle_event(event);
        }
    }
}

impl PluginAudioProcessorParams for GainProcessor<'_> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        for event in input {
            self.shared.handle_event(event);
        }
    }
}

impl PluginStateImpl for GainMainThread<'_> {
    fn save(&mut self, output: &mut OutputStream) -> Result<(), PluginError> {
        output.write_all(&self.shared.gain.load().to_le_bytes())?;
        Ok(())
    }
    fn load(&mut self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut buf = [0u8; 4];
        input.read_exact(&mut buf)?;
        self.shared.gain.store(f32::from_le_bytes(buf).clamp(0.0, 2.0));
        Ok(())
    }
}

clack_export_entry!(SinglePluginEntry<GainPlugin>);

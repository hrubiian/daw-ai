//! Hermetic CLAP test instrument: outputs a constant 0.25 on both channels
//! while at least one note is held. Used only by daw-core's CLAP host
//! tests — never shipped.

use clack_extensions::audio_ports::*;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use std::sync::atomic::{AtomicI32, Ordering};

pub struct ToneShared {
    held: AtomicI32,
}
impl PluginShared<'_> for ToneShared {}

pub struct ToneMainThread<'a> {
    _shared: &'a ToneShared,
}
impl<'a> PluginMainThread<'a, ToneShared> for ToneMainThread<'a> {}

pub struct TonePlugin;
impl Plugin for TonePlugin {
    type AudioProcessor<'a> = ToneProcessor<'a>;
    type Shared<'a> = ToneShared;
    type MainThread<'a> = ToneMainThread<'a>;
    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&ToneShared>) {
        builder.register::<PluginAudioPorts>();
    }
}

impl DefaultPluginFactory for TonePlugin {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::*;
        PluginDescriptor::new("test.tone", "DAW Test Tone").with_features([INSTRUMENT, STEREO])
    }
    fn new_shared(_host: HostSharedHandle<'_>) -> Result<Self::Shared<'_>, PluginError> {
        Ok(ToneShared { held: AtomicI32::new(0) })
    }
    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        shared: &'a Self::Shared<'a>,
    ) -> Result<Self::MainThread<'a>, PluginError> {
        Ok(ToneMainThread { _shared: shared })
    }
}

pub struct ToneProcessor<'a> {
    shared: &'a ToneShared,
}

impl<'a> PluginAudioProcessor<'a, ToneShared, ToneMainThread<'a>> for ToneProcessor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &mut ToneMainThread,
        shared: &'a ToneShared,
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
        for event in events.input {
            match event.as_core_event() {
                Some(CoreEventSpace::NoteOn(_)) => {
                    self.shared.held.fetch_add(1, Ordering::Relaxed);
                }
                Some(CoreEventSpace::NoteOff(_)) | Some(CoreEventSpace::NoteChoke(_)) => {
                    // Test fixture: hosts under test always pair on/off.
                    self.shared.held.fetch_sub(1, Ordering::Relaxed);
                }
                _ => {}
            }
        }
        let level = if self.shared.held.load(Ordering::Relaxed) > 0 {
            0.25f32
        } else {
            0.0
        };

        let mut port_pair = audio
            .port_pair(0)
            .ok_or(PluginError::Message("no port pair"))?;
        let mut channels = port_pair
            .channels()?
            .into_f32()
            .ok_or(PluginError::Message("expected f32"))?;
        for pair in channels.iter_mut() {
            let buf = match pair {
                ChannelPair::InPlace(b) => Some(b),
                ChannelPair::InputOutput(_, o) => Some(o),
                ChannelPair::OutputOnly(o) => Some(o),
                _ => None,
            };
            if let Some(buf) = buf {
                for s in buf.iter_mut() {
                    *s = level;
                }
            }
        }
        Ok(ProcessStatus::Continue)
    }
}

impl PluginAudioPortsImpl for ToneMainThread<'_> {
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

clack_export_entry!(SinglePluginEntry<TonePlugin>);

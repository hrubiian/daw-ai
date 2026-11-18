//! Split ownership for a live guitar CLAP effect. The frontend owns the
//! controller; audio/writer threads return the processor for main-thread teardown.
use super::clap::{ClapHandle, ClapInstance, ClapProcessor, ClapSlotSpec};
use crate::project::ClapSlot;
use base64::Engine;
use crossbeam_channel::{Receiver, Sender};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

pub const AMPFORGE_ID: &str = "com.ampforge.main";
const BLOCK: usize = 8192;

pub struct LivePlugin {
    handle: ClapHandle,
    returned: Receiver<ClapProcessor>,
    slot: ClapSlot,
    tempo: Arc<AtomicU64>,
}

pub struct LivePluginProcessor {
    processor: Option<ClapProcessor>,
    returned: Sender<ClapProcessor>,
    right: Vec<f32>,
    tempo: Arc<AtomicU64>,
}

impl LivePlugin {
    pub fn load(slot: &ClapSlot, sample_rate: u32) -> Result<(Self, LivePluginProcessor), String> {
        let spec = ClapSlotSpec {
            uid: slot.uid.clone(),
            path: slot.path.clone(),
            id: slot.id.clone(),
            state: slot
                .state
                .as_ref()
                .map(|s| {
                    base64::engine::general_purpose::STANDARD
                        .decode(s)
                        .map_err(|e| format!("invalid guitar plugin state: {e}"))
                })
                .transpose()?,
            params: slot
                .params
                .iter()
                .map(|(id, value)| {
                    id.parse::<u32>()
                        .map(|id| (id, *value))
                        .map_err(|e| e.to_string())
                })
                .collect::<Result<_, _>>()?,
        };
        let mut instance = ClapInstance::from_live_spec(&spec, sample_rate)?;
        let mut processor = instance
            .processor
            .take()
            .ok_or("plugin did not create an audio processor")?;
        // Apply saved parameter overrides before the first snapshot/editor read.
        processor.process(&mut [0.; 64], &mut [0.; 64]);
        let (tx, rx) = crossbeam_channel::bounded(1);
        let tempo = Arc::new(AtomicU64::new(120f64.to_bits()));
        Ok((
            Self {
                handle: instance.handle,
                returned: rx,
                slot: slot.clone(),
                tempo: Arc::clone(&tempo),
            },
            LivePluginProcessor {
                processor: Some(processor),
                returned: tx,
                right: vec![0.; BLOCK],
                tempo,
            },
        ))
    }

    pub fn snapshot(&mut self) -> ClapSlot {
        if let Some(blob) = self.handle.collect_state() {
            self.slot.state = Some(base64::engine::general_purpose::STANDARD.encode(blob));
        }
        for info in self.handle.param_infos.clone() {
            if let Some(value) = self.handle.param_value(info.id) {
                self.slot.params.insert(info.id.to_string(), value);
            }
        }
        self.slot.clone()
    }

    pub fn set_tempo(&self, bpm: f64) {
        self.tempo.store(bpm.to_bits(), Ordering::Relaxed);
    }
    pub fn pump(&mut self) -> bool {
        self.handle.pump()
    }
    pub fn show_editor(&mut self) -> Result<(), String> {
        self.handle.show_editor()
    }
    pub fn hide_editor(&mut self) {
        self.handle.hide_editor();
    }
}

impl Drop for LivePlugin {
    fn drop(&mut self) {
        // Owners must follow their stream/writer fields in the containing
        // object, so worker teardown returns the processor first.
        match self
            .returned
            .recv_timeout(std::time::Duration::from_secs(1))
        {
            Ok(processor) => self.handle.deactivate(processor),
            Err(_) => {
                eprintln!("guitar plugin processor was not returned before controller teardown")
            }
        }
    }
}

impl LivePluginProcessor {
    /// Process a mono instrument without allocating. Mono plugin outputs are
    /// duplicated by ClapProcessor only when the caller requests stereo.
    pub fn process(&mut self, samples: &mut [f32]) {
        let Some(processor) = self.processor.as_mut() else {
            samples.fill(0.);
            return;
        };
        if processor.process_failed {
            samples.fill(0.);
            return;
        }
        processor.set_tempo(f64::from_bits(self.tempo.load(Ordering::Relaxed)));
        for block in samples.chunks_mut(BLOCK) {
            self.right[..block.len()].copy_from_slice(block);
            processor.process(block, &mut self.right[..block.len()]);
            for sample in block {
                if !sample.is_finite() {
                    *sample = 0.;
                }
                // Same output safety knee as the built-in guitar rack.
                if sample.abs() > 0.9 {
                    *sample = sample.signum() * (0.9 + 0.1 * ((sample.abs() - 0.9) * 10.).tanh());
                }
            }
        }
    }
}

impl Drop for LivePluginProcessor {
    fn drop(&mut self) {
        if let Some(processor) = self.processor.take() {
            if let Err(error) = self.returned.try_send(processor) {
                // An owner lifetime violation must not destroy a plugin on
                // an audio thread. Normal stream/writer teardown never hits this.
                std::mem::forget(error.into_inner());
            }
        }
    }
}

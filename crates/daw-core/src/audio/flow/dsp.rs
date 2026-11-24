//! The ABI 1 DSP interface used by Flow Music exports.

use super::PluginKind;
use wasmi::{
    Config, Engine, Error, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder,
    TypedFunc,
};

pub(super) const MAX_BLOCK: usize = 1024;
const CHANNEL_BYTES: usize = MAX_BLOCK * 4;
const PARAM_ADDRESS: usize = 65_536;
const PAGE_BYTES: usize = 65_536;
const MAX_MEMORY: usize = 16 * 1024 * 1024;
const STARTUP_FUEL: u64 = 10_000_000;
const NOTE_FUEL: u64 = 100_000;
const FUEL_PER_FRAME: u64 = 24_000;

pub(super) struct Dsp {
    store: Store<StoreLimits>,
    memory: Memory,
    buffer_base: usize,
    notes: Option<NoteFunctions>,
    reset: Option<TypedFunc<(), ()>>,
    process: TypedFunc<(i32, i32, i32, i32, i32), ()>,
}

struct NoteFunctions {
    on: TypedFunc<(i32, i32), ()>,
    off: TypedFunc<i32, ()>,
}

impl Dsp {
    pub fn new(
        wasm: &[u8],
        kind: PluginKind,
        param_count: usize,
        sample_rate: f32,
    ) -> Result<Self, Error> {
        let mut config = Config::default();
        config
            .consume_fuel(true)
            .wasm_multi_memory(false)
            .wasm_memory64(false);
        let engine = Engine::new(&config);
        let limits = StoreLimitsBuilder::new()
            .memory_size(MAX_MEMORY)
            .memories(1)
            .tables(1)
            .table_elements(1024)
            .build();
        let mut store = Store::new(&engine, limits);
        store.limiter(|limits| limits);
        store.set_fuel(STARTUP_FUEL)?;
        let mut linker = Linker::new(&engine);
        linker.func_wrap("env", "abort", |_: i32, _: i32, _: i32, _: i32| {
            Err::<(), _>(Error::new("Flow Music DSP aborted"))
        })?;
        // The guest may register display buffers during initialization.
        linker.func_wrap(
            "env",
            "hostExposeBuffer",
            |_: i32, _: i32, _: i32, _: i32| 0i32,
        )?;
        let module = Module::new(&engine, wasm)?;
        let instance = linker.instantiate(&mut store, &module)?.start(&mut store)?;
        let abi = instance.get_typed_func::<(), i32>(&store, "getAbiVersion")?;
        let count = instance.get_typed_func::<(), i32>(&store, "getNumParams")?;
        if abi.call(&mut store, ())? != 1 || count.call(&mut store, ())? != param_count as i32 {
            return Err(Error::new(
                "Expected Flow Music DSP ABI 1 and matching parameter count",
            ));
        }
        let init = instance.get_typed_func::<f32, ()>(&store, "init")?;
        init.call(&mut store, sample_rate)?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| Error::new("Missing Flow Music DSP memory"))?;
        if memory.data_size(&store) < PARAM_ADDRESS + param_count * 4 {
            return Err(Error::new("Missing Flow Music parameter memory"));
        }
        // Reserve the host buffers above the initialized guest memory, then
        // prevent further guest memory allocations on the audio thread.
        let buffer_base = memory.grow(&mut store, 1)? as usize * PAGE_BYTES;
        *store.data_mut() = StoreLimitsBuilder::new()
            .memory_size(buffer_base + PAGE_BYTES)
            .table_elements(0)
            .build();
        let reset = instance
            .get_export(&store, "reset")
            .map(|_| instance.get_typed_func(&store, "reset"))
            .transpose()?;
        let notes = if kind == PluginKind::Instrument {
            Some(NoteFunctions {
                on: instance.get_typed_func(&store, "noteOn")?,
                off: instance.get_typed_func(&store, "noteOff")?,
            })
        } else {
            None
        };
        Ok(Self {
            notes,
            reset,
            process: instance.get_typed_func(&store, "process")?,
            memory,
            buffer_base,
            store,
        })
    }

    pub fn note_on(&mut self, key: u8, velocity: u8) -> Result<(), Error> {
        let notes = self
            .notes
            .as_ref()
            .ok_or_else(|| Error::new("The FMX effect has no note input"))?;
        self.store.set_fuel(NOTE_FUEL)?;
        notes
            .on
            .call(&mut self.store, (key.into(), velocity.into()))
    }

    pub fn note_off(&mut self, key: u8) -> Result<(), Error> {
        let notes = self
            .notes
            .as_ref()
            .ok_or_else(|| Error::new("The FMX effect has no note input"))?;
        self.store.set_fuel(NOTE_FUEL)?;
        notes.off.call(&mut self.store, key.into())
    }

    pub fn reset(&mut self) -> Result<bool, Error> {
        let Some(reset) = self.reset else {
            return Ok(false);
        };
        self.store.set_fuel(NOTE_FUEL)?;
        reset.call(&mut self.store, ())?;
        Ok(true)
    }

    pub fn render(
        &mut self,
        params: &[f32],
        input: Option<[&[f32]; 2]>,
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), Error> {
        debug_assert!(left.len() <= MAX_BLOCK && right.len() == left.len());
        let data = self.memory.data_mut(&mut self.store);
        let param_bytes = &mut data[PARAM_ADDRESS..PARAM_ADDRESS + params.len() * 4];
        for (bytes, value) in param_bytes.as_chunks_mut::<4>().0.iter_mut().zip(params) {
            *bytes = value.to_le_bytes();
        }
        let mut output_base = self.buffer_base;
        let (input_l, input_r) = if let Some(input) = input {
            let addresses = [self.buffer_base, self.buffer_base + CHANNEL_BYTES];
            for (samples, address) in input.into_iter().zip(addresses) {
                debug_assert_eq!(samples.len(), left.len());
                let buffer = &mut data[address..address + samples.len() * 4];
                for (bytes, sample) in buffer.as_chunks_mut::<4>().0.iter_mut().zip(samples) {
                    *bytes = sample.to_le_bytes();
                }
            }
            output_base += 2 * CHANNEL_BYTES;
            (addresses[0], addresses[1])
        } else {
            (0, 0)
        };
        let output_right = output_base + CHANNEL_BYTES;
        data[output_base..output_right + right.len() * 4].fill(0);
        self.store
            .set_fuel(NOTE_FUEL + FUEL_PER_FRAME * left.len() as u64)?;
        self.process.call(
            &mut self.store,
            (
                input_l as i32,
                input_r as i32,
                output_base as i32,
                output_right as i32,
                left.len() as i32,
            ),
        )?;
        let data = self.memory.data(&self.store);
        for (start, out) in [(output_base, left), (output_right, right)] {
            let buffer = &data[start..start + out.len() * 4];
            for (&bytes, sample) in buffer.as_chunks::<4>().0.iter().zip(out) {
                let value = f32::from_le_bytes(bytes);
                *sample = if value.is_finite() { value } else { 0.0 };
            }
        }
        Ok(())
    }
}

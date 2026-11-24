//! Host interface verified against the Model-M v4 DSP embedded by build.py.

use wasmi::{
    Config, Engine, Error, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder,
    TypedFunc,
};

pub const PARAM_COUNT: usize = 23;
pub const MAX_BLOCK: usize = 1024;
const PARAM_ADDRESS: usize = 65_536;
const PAGE_BYTES: usize = 65_536;
const INITIAL_PAGES: usize = 16;
const STARTUP_FUEL: u64 = 10_000_000;
const NOTE_FUEL: u64 = 100_000;
const FUEL_PER_FRAME: u64 = 24_000;

const PAYLOAD: &[u8] = include_bytes!(concat!(env!("MODEL_M_PAYLOAD"), "/model-m.wasm"));

pub struct Dsp {
    store: Store<StoreLimits>,
    memory: Memory,
    output: usize,
    note_on: TypedFunc<(i32, i32), ()>,
    note_off: TypedFunc<i32, ()>,
    reset: TypedFunc<(), ()>,
    process: TypedFunc<(i32, i32, i32, i32, i32), ()>,
}

impl Dsp {
    pub fn new(sample_rate: f32) -> Result<Self, Error> {
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let limits = StoreLimitsBuilder::new()
            .memory_size((INITIAL_PAGES + 1) * PAGE_BYTES)
            .build();
        let mut store = Store::new(&engine, limits);
        store.limiter(|limits| limits);
        store.set_fuel(STARTUP_FUEL)?;
        let mut linker = Linker::new(&engine);
        linker.func_wrap("env", "abort", |_: i32, _: i32, _: i32, _: i32| {
            Err::<(), _>(Error::new("Model-M DSP aborted"))
        })?;
        // The guest publishes an oscilloscope buffer during initialization.
        linker.func_wrap(
            "env",
            "hostExposeBuffer",
            |_: i32, _: i32, _: i32, _: i32| 0i32,
        )?;
        let module = Module::new(&engine, PAYLOAD)?;
        let instance = linker.instantiate(&mut store, &module)?.start(&mut store)?;
        let abi = instance.get_typed_func::<(), i32>(&store, "getAbiVersion")?;
        let count = instance.get_typed_func::<(), i32>(&store, "getNumParams")?;
        if abi.call(&mut store, ())? != 1 || count.call(&mut store, ())? != PARAM_COUNT as i32 {
            return Err(Error::new("Unexpected Model-M DSP ABI"));
        }
        let init = instance.get_typed_func::<f32, ()>(&store, "init")?;
        init.call(&mut store, sample_rate)?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| Error::new("Missing Model-M DSP memory"))?;
        if memory.data_size(&store) != INITIAL_PAGES * PAGE_BYTES {
            return Err(Error::new("Unexpected Model-M memory layout"));
        }
        // Reserve host audio buffers above the guest's initialized heap.
        let output = memory.grow(&mut store, 1)? as usize * PAGE_BYTES;
        Ok(Self {
            note_on: instance.get_typed_func(&store, "noteOn")?,
            note_off: instance.get_typed_func(&store, "noteOff")?,
            reset: instance.get_typed_func(&store, "reset")?,
            process: instance.get_typed_func(&store, "process")?,
            memory,
            output,
            store,
        })
    }

    pub fn note_on(&mut self, key: u8, velocity: u8) -> Result<(), Error> {
        self.store.set_fuel(NOTE_FUEL)?;
        self.note_on
            .call(&mut self.store, (key.into(), velocity.into()))
    }

    pub fn note_off(&mut self, key: u8) -> Result<(), Error> {
        self.store.set_fuel(NOTE_FUEL)?;
        self.note_off.call(&mut self.store, key.into())
    }

    pub fn reset(&mut self) -> Result<(), Error> {
        self.store.set_fuel(NOTE_FUEL)?;
        self.reset.call(&mut self.store, ())
    }

    pub fn render(
        &mut self,
        params: &[f32; PARAM_COUNT],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<(), Error> {
        debug_assert!(left.len() <= MAX_BLOCK && right.len() == left.len());
        let param_bytes = &mut self.memory.data_mut(&mut self.store)
            [PARAM_ADDRESS..PARAM_ADDRESS + PARAM_COUNT * 4];
        for (bytes, value) in param_bytes.chunks_exact_mut(4).zip(params) {
            bytes.copy_from_slice(&value.to_le_bytes());
        }
        self.store
            .set_fuel(NOTE_FUEL + FUEL_PER_FRAME * left.len() as u64)?;
        let right_address = self.output + MAX_BLOCK * 4;
        self.process.call(
            &mut self.store,
            (
                0,
                0,
                self.output as i32,
                right_address as i32,
                left.len() as i32,
            ),
        )?;
        let data = self.memory.data(&self.store);
        for (start, out) in [(self.output, left), (right_address, right)] {
            for (bytes, sample) in data[start..start + out.len() * 4].chunks_exact(4).zip(out) {
                let value = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                *sample = if value.is_finite() { value } else { 0.0 };
            }
        }
        Ok(())
    }
}

//! Guard the live callback contract, including mode changes and tail clearing.
use daw_core::audio::{
    amp::{AmpSettings, GuitarRig},
    pedals::{PedalPreset, PedalSettings},
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

struct TrackedAllocator;
thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static CALLS: Cell<usize> = const { Cell::new(0) };
}
fn record() {
    if TRACK.try_with(Cell::get).unwrap_or(false) {
        let _ = CALLS.try_with(|calls| calls.set(calls.get() + 1));
    }
}
unsafe impl GlobalAlloc for TrackedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        record();
        System.dealloc(pointer, layout)
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record();
        System.realloc(pointer, layout, size)
    }
}
#[global_allocator]
static ALLOCATOR: TrackedAllocator = TrackedAllocator;

#[test]
fn live_pedal_changes_and_audio_do_not_allocate_or_free_memory() {
    let mut rig = GuitarRig::new(48000);
    let presets = [
        PedalPreset::TightRhythm,
        PedalPreset::WideClean,
        PedalPreset::LeadEcho,
        PedalPreset::JetFlange,
        PedalPreset::Dry,
    ];
    let mut peak = 0f32;
    CALLS.with(|count| count.set(0));
    TRACK.with(|track| track.set(true));
    for preset in presets {
        rig.set(AmpSettings {
            pedals: PedalSettings::preset(preset),
            ..Default::default()
        });
        for i in 0..4096 {
            peak = peak.max(rig.process((i as f32 * 0.1).sin() * 0.1).abs());
        }
    }
    TRACK.with(|track| track.set(false));
    assert_eq!(CALLS.with(Cell::get), 0);
    assert!(peak > 0. && peak <= 1.);
}

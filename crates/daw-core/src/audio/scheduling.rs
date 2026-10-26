//! Device-frame scheduling. All storage is reserved before the callback starts.
use super::mixer::AudioEvent;
use std::{
    cmp::Ordering,
    collections::BinaryHeap,
    sync::atomic::{AtomicU32, AtomicU64, Ordering as Memory},
};

#[derive(Default)]
pub struct AudioClock {
    /// First frame that has not yet been rendered.
    pub frame: AtomicU64,
    pub sample_rate: AtomicU32,
    pub quantum: AtomicU32,
    pub generation: AtomicU64,
}
impl AudioClock {
    pub fn horizon(&self) -> u64 {
        // Two device buffers plus 10 ms for relay/thread wake-up jitter.
        let quantum = self.quantum.load(Memory::Relaxed);
        let quantum = if quantum == 0 { 1024 } else { quantum };
        self.frame.load(Memory::Acquire)
            + 2 * quantum as u64
            + self.sample_rate.load(Memory::Relaxed) as u64 / 100
    }
}
#[derive(Debug)]
pub struct ScheduledEvent {
    pub frame: u64,
    pub generation: u64,
    pub event: AudioEvent,
}
impl ScheduledEvent {
    pub fn cancelled(&self, generation: u64) -> bool {
        // Project/control changes still apply and acknowledge on pause. Only
        // obsolete performance events must be suppressed after seek/stop.
        self.generation != generation
            && matches!(
                self.event,
                AudioEvent::NoteOn { .. }
                    | AudioEvent::NoteOff { .. }
                    | AudioEvent::TriggerSample { .. }
                    | AudioEvent::TriggerAudioClip { .. }
                    | AudioEvent::PlayClip
                    | AudioEvent::PublishU32 { .. }
                    | AudioEvent::PublishU8 { .. }
            )
    }
}
struct Entry {
    serial: u64,
    event: Box<ScheduledEvent>,
}
impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.serial == other.serial
    }
}
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        (other.event.frame, other.serial).cmp(&(self.event.frame, self.serial))
    }
}
pub struct EventQueue {
    events: BinaryHeap<Entry>,
    serial: u64,
    capacity: usize,
}
impl EventQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            events: BinaryHeap::with_capacity(capacity),
            serial: 0,
            capacity,
        }
    }
    pub fn push(&mut self, event: Box<ScheduledEvent>) -> Result<(), Box<ScheduledEvent>> {
        if self.events.len() == self.capacity {
            return Err(event);
        }
        self.events.push(Entry {
            serial: self.serial,
            event,
        });
        self.serial = self.serial.wrapping_add(1);
        Ok(())
    }
    pub fn next_frame(&self) -> Option<u64> {
        self.events.peek().map(|e| e.event.frame)
    }
    pub fn pop_due(&mut self, frame: u64) -> Option<Box<ScheduledEvent>> {
        if self.next_frame()? > frame {
            return None;
        }
        self.events.pop().map(|e| e.event)
    }
}

//! Capture-to-mixer monitor tap. The recorder and the monitor have separate queues.

use super::input::InputChannels;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

type StereoFrame = [f32; 2];

#[derive(Debug, Default)]
pub(crate) struct MonitorSwitch {
    generation: AtomicU32,
    period: AtomicU32,
}

impl MonitorSwitch {
    pub(crate) fn set_enabled(&self, enabled: bool) {
        // Only the capture stream owner controls this switch.
        if self.is_enabled() != enabled {
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.generation.load(Ordering::Acquire) % 2 == 1
    }
}

pub(crate) struct MonitorWriter {
    switch: Arc<MonitorSwitch>,
    producer: HeapProd<StereoFrame>,
}

impl MonitorWriter {
    pub(crate) fn write(&mut self, audio: &[f32], channels: usize, selection: InputChannels) {
        if !self.switch.is_enabled() {
            return;
        }

        let (left, right) = match selection {
            InputChannels::Mono(channel) => (channel, channel),
            InputChannels::Stereo(left, right) => (left, right),
        };
        let (left, right) = (usize::from(left), usize::from(right));
        self.switch
            .period
            .store((audio.len() / channels) as u32, Ordering::Relaxed);
        self.producer.push_iter(
            audio
                .chunks_exact(channels)
                .map(|frame| [frame[left], frame[right]]),
        );
    }
}

/// The receiving half, owned by the mixer. Its buffers are allocated when the
/// capture stream opens and retired off the audio thread when the input changes.
pub struct TrackMonitorInput {
    switch: Arc<MonitorSwitch>,
    consumer: HeapCons<StereoFrame>,
    sample_rate: u32,
    generation: u32,
    position: f64,
    primed: bool,
}

impl std::fmt::Debug for TrackMonitorInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TrackMonitorInput")
            .field("sample_rate", &self.sample_rate)
            .finish_non_exhaustive()
    }
}

pub(crate) fn channel(
    sample_rate: u32,
    switch: Arc<MonitorSwitch>,
) -> (MonitorWriter, TrackMonitorInput) {
    let (producer, consumer) = HeapRb::new((sample_rate as usize / 4).max(32_768)).split();
    (
        MonitorWriter {
            switch: switch.clone(),
            producer,
        },
        TrackMonitorInput {
            switch,
            consumer,
            sample_rate,
            generation: 1,
            position: 0.0,
            primed: false,
        },
    )
}

impl TrackMonitorInput {
    pub(crate) fn mix_into(&mut self, left: &mut [f32], right: &mut [f32], output_rate: u32) {
        let generation = self.switch.generation.load(Ordering::Acquire);
        if generation != self.generation {
            self.generation = generation;
            self.consumer.clear();
            self.position = 0.0;
            self.primed = false;
        }
        if generation.is_multiple_of(2) || left.is_empty() {
            return;
        }

        // Input and output may use different rates. Keep one input period
        // queued to absorb scheduling jitter, and trim old audio after a stall.
        let step = self.sample_rate as f64 / output_rate.max(1) as f64;
        let resampling = self.sample_rate != output_rate;
        let frames = (self.position + (left.len() - 1) as f64 * step).floor() as usize
            + 1
            + usize::from(resampling);
        let advance = (self.position + left.len() as f64 * step).floor() as usize;
        let needed = frames.max(advance);
        let cushion = (self.switch.period.load(Ordering::Relaxed) as usize)
            .clamp(1, self.consumer.capacity().get() / 4);
        let available = self.consumer.occupied_len();
        let ready = if self.primed {
            needed
        } else {
            needed + cushion
        };
        if available < ready.min(self.consumer.capacity().get()) {
            self.primed = false;
            return;
        }
        if available > needed + 3 * cushion {
            self.consumer.skip(available - needed - cushion);
        }

        let (head, tail) = self.consumer.as_slices();
        let read = |index: usize| -> Option<StereoFrame> {
            if index < head.len() {
                head.get(index).copied()
            } else {
                tail.get(index - head.len()).copied()
            }
        };
        for (left, right) in left.iter_mut().zip(right) {
            let index = self.position.floor() as usize;
            let Some(a) = read(index) else {
                self.primed = false;
                break;
            };
            let b = if resampling {
                read(index + 1).unwrap_or(a)
            } else {
                a
            };
            let frac = self.position.fract() as f32;
            *left += a[0] + (b[0] - a[0]) * frac;
            *right += a[1] + (b[1] - a[1]) * frac;
            self.position += step;
            self.primed = true;
        }
        let consumed = self.position.floor() as usize;
        self.consumer.skip(consumed);
        self.position -= consumed as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_mono_channel_and_resamples_to_the_output_rate() {
        let switch = Arc::new(MonitorSwitch::default());
        let (mut writer, mut input) = channel(24_000, switch.clone());
        switch.set_enabled(true);
        let frames: Vec<_> = (0..16).flat_map(|i| [0.9, i as f32 * 0.01]).collect();
        for block in frames.as_chunks::<16>().0 {
            writer.write(block, 2, InputChannels::Mono(1));
        }
        let (mut left, mut right) = ([0.; 8], [0.; 8]);
        input.mix_into(&mut left, &mut right, 48_000);
        assert_eq!(left, right);
        for (index, sample) in left.into_iter().enumerate() {
            assert!((sample - index as f32 * 0.005).abs() < 1e-6);
        }
    }
}

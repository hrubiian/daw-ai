use super::*;
use daw_core::dsp::output::{
    OutputAnalysis, OutputAnalyzer, NOTE_NAMES, SPECTRUM_MAX_HZ, SPECTRUM_MIN_HZ,
};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
struct AnalyzerColors {
    signal: u32,
    right: u32,
    screen: u32,
    grid: u32,
    scanline: u32,
    fill: u32,
    readout: u32,
}

impl AnalyzerColors {
    const DARK: Self = Self {
        signal: 0x9ef6ba,
        right: 0xffce81,
        screen: 0x101a16,
        grid: 0x254236,
        scanline: 0x13211b,
        fill: 0x355f48,
        readout: 0x8bb49c,
    };

    const LIGHT: Self = Self {
        signal: 0x21653b,
        right: 0x975012,
        screen: 0xf5faf6,
        grid: 0xc7d9cd,
        scanline: 0xeaf2ed,
        fill: 0xa2c4ad,
        readout: 0x496455,
    };
}

const FREQ_TICKS: [(f32, &str); 5] = [
    (30.0, "30 Hz"),
    (100.0, "100"),
    (1000.0, "1k"),
    (10_000.0, "10k"),
    (20_000.0, "20k"),
];

#[derive(Default)]
pub(super) struct OutputAnalyzerView {
    analyzer: Option<OutputAnalyzer>,
    samples: Vec<[f32; 2]>,
    analysis: OutputAnalysis,
    updated: Option<Instant>,
}

impl OutputAnalyzerView {
    pub(super) fn poll(&mut self, host: &Host) -> bool {
        if self
            .updated
            .is_some_and(|last| last.elapsed() < Duration::from_millis(80))
        {
            return false;
        }
        let rate = host.sample_rate.load(Ordering::Relaxed);
        if rate == 0 {
            return false;
        }
        let Ok(history) = host.vis_buf.try_lock() else {
            return false;
        };
        self.samples.clear();
        self.samples.extend(history.recent(rate as usize / 2));
        drop(history);
        for sample in self.samples.iter_mut().flatten() {
            if !sample.is_finite() {
                *sample = 0.0;
            }
        }
        self.updated = Some(Instant::now());
        let previous = self.analysis.rms;
        self.analysis = self
            .analyzer
            .get_or_insert_with(OutputAnalyzer::default)
            .analyze(&self.samples, rate);
        self.analysis.rms >= 0.0002 || previous >= 0.0002
    }

    pub(super) fn panel(&self, audio_ready: bool, light_theme: bool) -> gpui::Stateful<gpui::Div> {
        let colors = if light_theme {
            AnalyzerColors::LIGHT
        } else {
            AnalyzerColors::DARK
        };
        let analysis = &self.analysis;
        let audible = analysis.rms >= 0.0002;
        let scale = [0.0625, 0.125, 0.25, 0.5, 1.0]
            .into_iter()
            .find(|&scale| analysis.peak[0].max(analysis.peak[1]) <= scale)
            .unwrap_or(1.0);
        let status = if !audio_ready {
            "Audio engine offline"
        } else if audible {
            "Signal"
        } else {
            "Quiet"
        };
        let frequency = analysis.dominant_hz.map_or_else(
            || "—".into(),
            |hz| {
                if hz < 1000.0 {
                    format!("{hz:.0} Hz")
                } else {
                    format!("{:.1} kHz", hz / 1000.0)
                }
            },
        );
        let levels = div()
            .flex()
            .flex_wrap()
            .gap_3()
            .child(readout(colors, "L peak", level_db(analysis.peak[0])))
            .child(readout(colors, "R peak", level_db(analysis.peak[1])))
            .child(readout(colors, "RMS", level_db(analysis.rms)))
            .child(readout(colors, "Strongest frequency", frequency));
        let wave_section = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                plot_heading(colors, "Waveform")
                    .child(div().text_color(rgb(colors.signal)).child("L"))
                    .child(div().text_color(rgb(colors.right)).child("R"))
                    .child(
                        div()
                            .text_color(rgb(colors.readout))
                            .child(format!("±{scale} FS")),
                    ),
            )
            .child(waveform(colors, analysis.waveform.clone(), scale))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_color(rgb(colors.readout))
                    .text_size(px(10.))
                    .child("−17 ms")
                    .child("Now"),
            );
        let spectrum_section = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                plot_heading(colors, "Spectrum")
                    .child(div().text_color(rgb(colors.readout)).child("−72 to 0 dBFS")),
            )
            .child(spectrum(colors, analysis.spectrum))
            .child(
                div()
                    .relative()
                    .h(px(15.))
                    .text_color(rgb(colors.readout))
                    .text_size(px(10.))
                    .children(FREQ_TICKS.into_iter().map(|(hz, label)| {
                        let tick = div().absolute().child(label);
                        if hz == SPECTRUM_MAX_HZ {
                            tick.right_0()
                        } else {
                            tick.left(gpui::relative(frequency_offset(hz)))
                        }
                    })),
            );

        div()
            .id("output-analyzer")
            .debug_selector(|| "output-analyzer".into())
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .rounded_lg()
            .bg(rgb(colors.screen))
            .border_1()
            .border_color(rgb(colors.grid))
            .text_color(rgb(colors.signal))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(div().text_size(px(16.)).child("Master output"))
                    .child(
                        div()
                            .text_color(rgb(if audible {
                                colors.signal
                            } else {
                                colors.readout
                            }))
                            .child(status),
                    ),
            )
            .child(levels)
            .child(wave_section)
            .child(spectrum_section)
            .child(harmony(colors, analysis, audible))
    }
}

fn readout(colors: AnalyzerColors, label: &'static str, value: String) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .min_w(px(53.))
        .child(
            div()
                .text_size(px(10.))
                .text_color(rgb(colors.readout))
                .child(label),
        )
        .child(div().text_size(px(13.)).child(value))
}

fn level_db(amplitude: f32) -> String {
    if amplitude > 0.0001 {
        format!("{:+.1} dBFS", 20.0 * amplitude.log10())
    } else {
        "−∞ dBFS".into()
    }
}

fn plot_heading(colors: AnalyzerColors, title: &'static str) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(11.))
        .child(div().flex_1().text_color(rgb(colors.signal)).child(title))
}

fn fill_plot(
    window: &mut Window,
    bounds: Bounds<gpui::Pixels>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: u32,
) {
    let origin = bounds.origin + gpui::point(bounds.size.width * x, bounds.size.height * y);
    let dimensions = size(bounds.size.width * w, bounds.size.height * h);
    window.paint_quad(gpui::fill(Bounds::new(origin, dimensions), rgb(color)));
}

fn grid(
    colors: AnalyzerColors,
    window: &mut Window,
    bounds: Bounds<gpui::Pixels>,
    xs: impl Iterator<Item = f32>,
) {
    if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
        return;
    }
    let pixel_y = px(1.) / bounds.size.height;
    let pixel_x = px(1.) / bounds.size.width;
    for y in (0..f32::from(bounds.size.height) as usize).step_by(5) {
        fill_plot(
            window,
            bounds,
            0.0,
            y as f32 * pixel_y,
            1.0,
            pixel_y,
            colors.scanline,
        );
    }
    for x in xs {
        fill_plot(
            window,
            bounds,
            x.min(1.0 - pixel_x),
            0.0,
            pixel_x,
            1.0,
            colors.grid,
        );
    }
    for y in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
        fill_plot(
            window,
            bounds,
            0.0,
            y.min(1.0 - pixel_y),
            1.0,
            pixel_y,
            colors.grid,
        );
    }
}

fn waveform(colors: AnalyzerColors, samples: Vec<[f32; 2]>, scale: f32) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            grid(colors, window, bounds, (0..=8).map(|i| i as f32 / 8.0));
            if samples.len() < 2 {
                return;
            }
            let stride = samples.len().div_ceil(640);
            for (channel, color) in [(1, colors.right), (0, colors.signal)] {
                for (width, stroke_color) in [(3.4, colors.fill), (1.3, color)] {
                    let mut path = gpui::PathBuilder::stroke(px(width));
                    for (i, frame) in samples.iter().enumerate().step_by(stride) {
                        let x = bounds.size.width * (i as f32 / (samples.len() - 1) as f32);
                        let y = bounds.size.height
                            * (0.5 - 0.44 * (frame[channel] / scale).clamp(-1.0, 1.0));
                        let point = bounds.origin + gpui::point(x, y);
                        if i == 0 {
                            path.move_to(point);
                        } else {
                            path.line_to(point);
                        }
                    }
                    if let Ok(path) = path.build() {
                        window.paint_path(path, rgb(stroke_color));
                    }
                }
            }
        },
    )
    .w_full()
    .h(px(142.))
    .flex_shrink_0()
}

fn frequency_offset(hz: f32) -> f32 {
    (hz / SPECTRUM_MIN_HZ).ln() / (SPECTRUM_MAX_HZ / SPECTRUM_MIN_HZ).ln()
}

fn spectrum(
    colors: AnalyzerColors,
    bars: [f32; daw_core::dsp::output::SPECTRUM_BARS],
) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            grid(
                colors,
                window,
                bounds,
                FREQ_TICKS.into_iter().map(|(hz, _)| frequency_offset(hz)),
            );
            let column = 1.0 / bars.len() as f32;
            for (i, &height) in bars.iter().enumerate() {
                if height <= 0.0 {
                    continue;
                }
                let x = i as f32 * column;
                fill_plot(
                    window,
                    bounds,
                    x,
                    1.0 - height,
                    column * 0.76,
                    height,
                    colors.fill,
                );
                let cap = height.min(0.025);
                fill_plot(
                    window,
                    bounds,
                    x,
                    1.0 - height,
                    column * 0.76,
                    cap,
                    colors.signal,
                );
            }
        },
    )
    .w_full()
    .h(px(100.))
    .flex_shrink_0()
}

fn harmony(colors: AnalyzerColors, analysis: &OutputAnalysis, audible: bool) -> gpui::Div {
    let estimate = analysis.estimated_triad();
    let summary = if let Some((root, minor)) = estimate {
        format!(
            "{} {} · estimated",
            NOTE_NAMES[root],
            if minor { "minor" } else { "major" }
        )
    } else if analysis.pitch_energy.iter().any(|&value| value > 0.0) {
        "Pitch classes detected".to_owned()
    } else if audible {
        "No clear pitches".to_owned()
    } else {
        "Waiting for signal".to_owned()
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            plot_heading(colors, "Harmony")
                .child(div().text_color(rgb(colors.readout)).child(summary)),
        )
        .child(
            div()
                .id("output-harmony")
                .debug_selector(|| "output-harmony".into())
                .flex()
                .gap_px()
                .children(NOTE_NAMES.into_iter().zip(analysis.pitch_energy).map(
                    |(note, energy)| {
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .bg(rgb(colors.scanline))
                            .child(
                                div().flex().items_end().w_full().h(px(32.)).child(
                                    div()
                                        .w_full()
                                        .h(px(32. * energy))
                                        .bg(rgb(if energy >= 0.33 {
                                            colors.signal
                                        } else {
                                            colors.fill
                                        })),
                                ),
                            )
                            .child(
                                div()
                                    .py_1()
                                    .text_size(px(10.))
                                    .text_color(rgb(if energy >= 0.33 {
                                        colors.signal
                                    } else {
                                        colors.readout
                                    }))
                                    .child(note),
                            )
                    },
                )),
        )
}

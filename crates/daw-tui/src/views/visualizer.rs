use crate::app::App;
use crossterm::event::KeyEvent;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use rustfft::{FftPlanner, num_complex::Complex};

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_oscilloscope(frame, chunks[0], app);
    render_spectrum(frame, chunks[1], app);
}

fn render_oscilloscope(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title("Oscilloscope");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let w = inner.width as usize;
    let h = inner.height as usize;
    if w == 0 || h == 0 { return; }

    let samples = &app.vis_snapshot;
    if samples.is_empty() {
        frame.render_widget(Paragraph::new("No audio signal"), inner);
        return;
    }

    // Downsample/window to fit display width
    let step = (samples.len() / w).max(1);
    let mid = h / 2;

    let mut lines: Vec<Line> = Vec::with_capacity(h);
    for row in 0..h {
        let mut spans: Vec<Span> = Vec::with_capacity(w);
        for col in 0..w {
            let idx = (col * step).min(samples.len() - 1);
            let sample = samples[idx].clamp(-1.0, 1.0);
            // Map sample [-1, 1] to row index [h-1, 0]
            let signal_row = ((1.0 - sample) * 0.5 * (h - 1) as f32).round() as usize;
            let signal_row = signal_row.min(h - 1);

            let ch = if row == signal_row {
                Span::styled("▌", Style::default().fg(Color::Cyan))
            } else if row == mid {
                Span::styled("─", Style::default().fg(Color::DarkGray))
            } else {
                Span::raw(" ")
            };
            spans.push(ch);
        }
        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_spectrum(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title("Spectrum Analyzer  (20 Hz – 20 kHz)");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let w = inner.width as usize;
    let h = inner.height as usize;
    if w == 0 || h == 0 { return; }

    let samples = &app.vis_snapshot;
    if samples.is_empty() {
        frame.render_widget(Paragraph::new("No audio signal"), inner);
        return;
    }

    const FFT_SIZE: usize = 2048;
    let n = FFT_SIZE.min(samples.len());
    let mut input: Vec<Complex<f32>> = (0..FFT_SIZE)
        .map(|i| {
            let s = if i < n { samples[samples.len() - n + i] } else { 0.0 };
            // Hann window
            let w = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos());
            Complex { re: s * w, im: 0.0 }
        })
        .collect();

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    fft.process(&mut input);

    // Map display columns to frequency bins (log scale, 20 Hz – 20 kHz)
    // Assume 44100 Hz sample rate (good enough for display; not critical)
    let sample_rate = 44100.0f32;
    let bin_hz = sample_rate / FFT_SIZE as f32;
    let freq_min = 20.0f32;
    let freq_max = 20000.0f32;
    let log_min = freq_min.log10();
    let log_max = freq_max.log10();

    let magnitudes: Vec<f32> = (0..w)
        .map(|col| {
            let t = col as f32 / (w - 1).max(1) as f32;
            let freq = 10.0f32.powf(log_min + t * (log_max - log_min));
            let bin = (freq / bin_hz).round() as usize;
            let bin = bin.clamp(1, FFT_SIZE / 2 - 1);
            let mag = (input[bin].norm() * 2.0 / FFT_SIZE as f32).max(1e-10);
            20.0 * mag.log10() // dBFS
        })
        .collect();

    // Build display: bars grow from the bottom
    let mut lines: Vec<Line> = Vec::with_capacity(h);
    for row in 0..h {
        // row 0 = top of display, row h-1 = bottom
        let threshold_frac = (h - 1 - row) as f32 / (h - 1) as f32; // 0 = bottom, 1 = top
        // Map to dB range: bottom = -60 dB, top = 0 dB
        let db_threshold = -60.0 + threshold_frac * 60.0;

        let mut spans: Vec<Span> = Vec::with_capacity(w);
        for mag_db in &magnitudes {
            let color = if *mag_db >= db_threshold {
                if *mag_db >= -6.0 { Color::Red }
                else if *mag_db >= -18.0 { Color::Yellow }
                else { Color::Green }
            } else {
                Color::DarkGray
            };
            let ch = if *mag_db >= db_threshold { "█" } else { "░" };
            spans.push(Span::styled(ch, Style::default().fg(color)));
        }
        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

pub fn handle_key(_app: &mut App, _key: KeyEvent) {}

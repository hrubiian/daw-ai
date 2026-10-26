// crates/daw-tui/src/views/mod.rs
pub mod arrange;
pub mod browser;
pub mod code;
pub mod effects;
pub mod mixer;
pub mod piano_roll;
pub mod sequencer;
pub mod tracks;
pub mod visualizer;

use crate::app::{App, DialogMode, Tab};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Tabs},
    Frame,
};

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    render_tab_bar(frame, app, chunks[0]);
    render_transport(frame, app, chunks[2]);

    match app.active_tab {
        Tab::Tracks    => tracks::render(frame, chunks[1], app),
        Tab::Sequencer => sequencer::render(frame, chunks[1], app),
        Tab::PianoRoll => piano_roll::render(frame, chunks[1], app),
        Tab::Mixer     => mixer::render(frame, chunks[1], app),
        Tab::Browser   => browser::render(frame, chunks[1], app),
        Tab::Effects   => effects::render(frame, chunks[1], app),
        Tab::Arrange   => arrange::render(frame, chunks[1], app),
        Tab::Viz       => visualizer::render(frame, chunks[1], app),
        Tab::Code      => code::render(frame, chunks[1], app),
    }
}

fn render_tab_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let titles: Vec<Line> = Tab::all().iter().map(|t| Line::from(t.label())).collect();
    let selected = Tab::all().iter().position(|t| t == &app.active_tab).unwrap_or(0);
    let tabs = Tabs::new(titles)
        .select(selected)
        .block(Block::default().borders(Borders::ALL).title("DAW-CLI"))
        .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
    frame.render_widget(tabs, area);
}

fn render_transport(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    // Dialog prompt overrides the transport bar
    match &app.dialog_mode {
        DialogMode::SaveAs => {
            let line = Line::from(vec![
                Span::styled("Save project as: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{}_", app.dialog_buf), Style::default().fg(Color::White)),
                Span::styled("  [Enter] confirm  [Esc] cancel", Style::default().fg(Color::DarkGray)),
            ]);
            frame.render_widget(Paragraph::new(line), area);
            return;
        }
        DialogMode::ExportWav => {
            let line = Line::from(vec![
                Span::styled("Export to (.wav / .mp3): ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{}_", app.dialog_buf), Style::default().fg(Color::White)),
                Span::styled("  [Enter] export  [Esc] cancel", Style::default().fg(Color::DarkGray)),
            ]);
            frame.render_widget(Paragraph::new(line), area);
            return;
        }
        DialogMode::None => {}
    }

    let play_icon  = if app.is_playing { "▶ PLAYING" } else { "⏸ PAUSED" };
    let play_color = if app.is_playing { Color::Green } else { Color::DarkGray };
    let filename   = app.project_path
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unsaved");

    let swing_pct = (app.project.project.swing * 100.0).round() as u32;
    let hum_pct   = (app.project.project.humanize * 100.0).round() as u32;
    let mut spans = vec![
        Span::styled(play_icon, Style::default().fg(play_color)),
        Span::raw(format!(
            "  {} BPM  swing {}%  hum {}%  {} {}  {}",
            app.project.project.bpm as u32,
            swing_pct,
            hum_pct,
            app.project.project.key,
            app.project.project.scale,
            filename,
        )),
    ];
    if app.is_recording {
        spans.push(Span::styled("  [REC]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)));
    }
    if app.live_play {
        spans.push(Span::styled("  [LIVE]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)));
    }
    if app.project.project.metronome {
        spans.push(Span::styled("  [MET]", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    }
    if app.jam_mode {
        spans.push(Span::styled("  [JAM]", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)));
        if !app.jam_status.is_empty() {
            spans.push(Span::styled(format!("  {}", app.jam_status), Style::default().fg(Color::DarkGray)));
        }
    }
    let midi_label = app.midi_device.as_deref().unwrap_or("No MIDI");
    let midi_color = if app.midi_device.is_some() { Color::Magenta } else { Color::DarkGray };
    spans.push(Span::styled(
        format!("  MIDI:{}", midi_label),
        Style::default().fg(midi_color),
    ));
    if app.midi_map.monitor {
        if let Some((cc, val)) = app.midi_last_cc {
            spans.push(Span::styled(
                format!("  CC:{}={}", cc, val),
                Style::default().fg(Color::Yellow),
            ));
        }
    }
    if !app.dialog_status.is_empty() {
        spans.push(Span::styled(
            format!("  {}", app.dialog_status),
            Style::default().fg(Color::Cyan),
        ));
    }
    spans.push(Span::styled("  [0] panic", Style::default().fg(Color::DarkGray)));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

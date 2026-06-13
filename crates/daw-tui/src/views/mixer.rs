use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent};
use daw_core::sequencer::SequencerCommand;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(area);

    let num_tracks = app.project.tracks.len();
    if num_tracks == 0 {
        frame.render_widget(Paragraph::new("No tracks."), chunks[0]);
        return;
    }

    let track_width = (chunks[0].width / num_tracks as u16).max(6);
    let track_constraints: Vec<Constraint> = app.project.tracks.iter()
        .map(|_| Constraint::Length(track_width))
        .collect();

    let track_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(track_constraints)
        .split(chunks[0]);

    for (i, (track, &track_area)) in app.project.tracks.iter().zip(track_areas.iter()).enumerate() {
        let is_selected = i == app.mixer_track;
        let fader_height = track_area.height.saturating_sub(4) as usize;
        let filled = (track.volume * fader_height as f32) as usize;
        let empty  = fader_height.saturating_sub(filled);

        let mut fader_lines: Vec<String> = Vec::new();
        for _ in 0..empty  { fader_lines.push("  ".to_string()); }
        for _ in 0..filled { fader_lines.push("##".to_string()); }
        let fader = fader_lines.join("\n");

        let pan_pos = ((track.pan + 1.0) * 4.5) as usize;
        let pan_bar: String = (0..9).map(|j| if j == pan_pos { '|' } else { '-' }).collect();

        let block_style = if is_selected {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let name: String = track.name.chars().take(6).collect();
        let mute_label = if track.muted { "[M]" } else { "   " };
        let content = format!(
            "{}\n{:.0}% {}\n{}\n{}",
            fader,
            track.volume * 100.0,
            mute_label,
            pan_bar,
            name,
        );

        frame.render_widget(
            Paragraph::new(content)
                .block(Block::default().borders(Borders::ALL).style(block_style)),
            track_area,
        );
    }

    frame.render_widget(
        Paragraph::new("[Left/Right] select  [Up/Down] volume  [p] pan  [m] mute")
            .block(Block::default().borders(Borders::ALL)),
        chunks[1],
    );
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let max = app.project.tracks.len().saturating_sub(1);
    match key.code {
        KeyCode::Left  => { app.mixer_track = app.mixer_track.saturating_sub(1); }
        KeyCode::Right => { app.mixer_track = (app.mixer_track + 1).min(max); }
        KeyCode::Up    => {
            if let Some(t) = app.project.tracks.get_mut(app.mixer_track) {
                t.volume = (t.volume + 0.05).min(2.0); // 1.0 = unity, up to +6 dB
                let (id, vol) = (t.id.clone(), t.volume);
                app.send(SequencerCommand::SetTrackVolume { track_id: id, volume: vol });
            }
        }
        KeyCode::Down => {
            if let Some(t) = app.project.tracks.get_mut(app.mixer_track) {
                t.volume = (t.volume - 0.05).max(0.0);
                let (id, vol) = (t.id.clone(), t.volume);
                app.send(SequencerCommand::SetTrackVolume { track_id: id, volume: vol });
            }
        }
        KeyCode::Char('p') => {
            if let Some(t) = app.project.tracks.get_mut(app.mixer_track) {
                t.pan = if t.pan < -0.3 { 0.0 } else if t.pan < 0.3 { 0.5 } else { -0.5 };
                let (id, pan) = (t.id.clone(), t.pan);
                app.send(SequencerCommand::SetTrackPan { track_id: id, pan });
            }
        }
        KeyCode::Char('m') => {
            if let Some(t) = app.project.tracks.get_mut(app.mixer_track) {
                t.muted = !t.muted;
                let (id, muted) = (t.id.clone(), t.muted);
                app.send(SequencerCommand::SetTrackMute { track_id: id, muted });
            }
        }
        _ => {}
    }
}

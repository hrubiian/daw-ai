use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent};
use daw_core::project::{Note, Pattern, TrackType};
use daw_core::sequencer::SequencerCommand;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

fn pitch_name(pitch: u8) -> String {
    let octave = (pitch / 12) as i32 - 1;
    let name   = NOTE_NAMES[(pitch % 12) as usize];
    format!("{name}{octave}")
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let track = app.project.tracks.get(app.active_track);
    let Some(track) = track else {
        frame.render_widget(Paragraph::new("No track selected."), area);
        return;
    };

    if track.track_type == TrackType::Drum {
        frame.render_widget(
            Paragraph::new("Selected track is a drum track. Use Sequencer (2) instead."),
            area,
        );
        return;
    }
    if track.track_type == TrackType::Audio {
        render_audio_waveform(frame, area, app, &track.id, &track.name);
        return;
    }

    let pattern = track.patterns.first()
        .and_then(|pid| app.project.patterns.iter().find(|p| p.id() == pid));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(3)])
        .split(area);

    {
        use ratatui::text::{Line, Span};
        use ratatui::style::{Color, Modifier, Style};
        let mut spans = vec![Span::raw(format!(
            " Track: {}  Pattern: {}  Cursor: {} tick {}",
            track.name,
            pattern.map(|p| p.name()).unwrap_or("none"),
            pitch_name(app.pr_pitch),
            app.pr_tick,
        ))];
        if app.pr_midi_record {
            spans.push(Span::styled(
                "  ● REC",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), chunks[0]);
    }

    let Some(Pattern::PianoRoll(pr)) = pattern else {
        frame.render_widget(
            Paragraph::new("No piano roll pattern attached.")
                .block(Block::default().borders(Borders::ALL)),
            chunks[1],
        );
        return;
    };

    let ticks_per_col: u32 = 120;
    let num_cols = (area.width.saturating_sub(8)) as u32;
    let visible_rows = area.height.saturating_sub(5) as u8;
    let pitch_start = app.pr_pitch.saturating_sub(visible_rows / 2);
    let tick_start  = app.pr_tick / ticks_per_col * ticks_per_col;

    let mut lines: Vec<Line> = Vec::new();

    let mut header = vec![Span::raw(format!("{:<7}", ""))];
    for c in 0..num_cols {
        let tick = tick_start + c * ticks_per_col;
        header.push(Span::styled(
            format!("{:^4}", tick / 120),
            Style::default().fg(Color::DarkGray),
        ));
    }
    lines.push(Line::from(header));

    for row_offset in 0..visible_rows {
        let pitch = pitch_start + row_offset;
        let is_cursor_row = pitch == app.pr_pitch;

        let name_style = if is_cursor_row {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else if NOTE_NAMES[(pitch % 12) as usize].contains('#') {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::White)
        };

        let mut spans = vec![Span::styled(format!("{:<6} ", pitch_name(pitch)), name_style)];

        for c in 0..num_cols {
            let col_tick = tick_start + c * ticks_per_col;
            let has_note = pr.notes.iter().any(|n| {
                n.pitch == pitch && n.tick <= col_tick && col_tick < n.tick + n.duration
            });
            let is_cursor = is_cursor_row
                && col_tick == tick_start + (app.pr_tick - tick_start) / ticks_per_col * ticks_per_col;

            let cell = if has_note { "####" } else { "    " };
            let style = if is_cursor {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::REVERSED)
            } else if has_note {
                Style::default().fg(Color::Magenta)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            spans.push(Span::styled(cell, style));
        }
        lines.push(Line::from(spans));
    }

    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Piano Roll")),
        chunks[1],
    );

    let help = if app.pr_midi_record {
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("  ● REC ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Span::raw(" Play notes on MIDI keyboard — recorded with real timing    "),
                Span::styled("[r] stop recording", Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(Span::styled(
                "  NoteOn stamps position, NoteOff sets duration. Pattern loops as it plays.",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .block(Block::default().borders(Borders::ALL).title("MIDI Record"))
    } else if app.live_play {
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("  LIVE ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::raw(" Keys → notes + step-record    "),
                Span::styled("[l] or [Esc] exit live mode", Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(Span::styled(
                "  [2][3] [5][6][7]   [s][d] [g][h][j]    C5 row: q w e r t y u   C4 row: z x c v b n m",
                Style::default().fg(Color::Cyan),
            )),
        ])
        .block(Block::default().borders(Borders::ALL)
            .title("Live Piano  —  step-records into pattern at cursor"))
    } else {
        Paragraph::new("[↑↓] pitch  [←→] time  [Enter] add  [Del] remove  [l] live  [r] MIDI record (while playing)")
            .block(Block::default().borders(Borders::ALL))
    };
    frame.render_widget(help, chunks[2]);
}

fn render_audio_waveform(frame: &mut Frame, area: Rect, app: &App, track_id: &str, track_name: &str) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let samples = app.waveform_cache.get(track_id);
    let sample_count = samples.map(|s| s.len()).unwrap_or(0);

    frame.render_widget(
        Paragraph::new(format!(
            " Audio track: {}   {} samples   [Ctrl+R] record  [Ctrl+1] Tracks tab",
            track_name, sample_count,
        )).style(Style::default().fg(Color::Cyan)),
        chunks[0],
    );

    let w = area.width as usize;
    let h = area.height.saturating_sub(2) as usize;

    if let Some(samples) = samples {
        let lines = waveform_lines(samples, w, h);
        frame.render_widget(
            Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title("Waveform")),
            chunks[1],
        );
    } else {
        frame.render_widget(
            Paragraph::new("  No clip recorded yet.  Use [Ctrl+R] to start recording.")
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().borders(Borders::ALL).title("Waveform")),
            chunks[1],
        );
    }

    frame.render_widget(
        Paragraph::new("  [Ctrl+R] record / stop   [Ctrl+1] go to Tracks tab")
            .style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

fn waveform_lines(samples: &[f32], width: usize, height: usize) -> Vec<Line<'static>> {
    if samples.is_empty() || width < 2 || height < 1 {
        return vec![];
    }
    let inner_w = width.saturating_sub(2).max(1);
    let inner_h = height.saturating_sub(2).max(1);

    // Build RMS amplitude per column
    let chunk_size = (samples.len() / inner_w).max(1);
    let amplitudes: Vec<f32> = (0..inner_w)
        .map(|col| {
            let start = col * chunk_size;
            let end   = ((col + 1) * chunk_size).min(samples.len());
            let rms = (samples[start..end].iter().map(|s| s * s).sum::<f32>() / (end - start) as f32).sqrt();
            rms.min(1.0)
        })
        .collect();

    // Map amplitude to half-height in rows (symmetric display)
    let half = (inner_h / 2).max(1);
    let center_row = inner_h / 2;

    let mut grid = vec![vec![false; inner_w]; inner_h];
    for (col, &amp) in amplitudes.iter().enumerate() {
        let filled = ((amp * half as f32) as usize).min(half);
        for row in (center_row.saturating_sub(filled))..=(center_row + filled).min(inner_h - 1) {
            grid[row][col] = true;
        }
    }

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner_h);
    for row in 0..inner_h {
        let is_center = row == center_row;
        let spans: Vec<Span<'static>> = grid[row]
            .iter()
            .map(|&filled| {
                if filled {
                    Span::styled("▌", Style::default().fg(Color::Green))
                } else if is_center {
                    Span::styled("─", Style::default().fg(Color::DarkGray))
                } else {
                    Span::raw(" ")
                }
            })
            .collect();
        lines.push(Line::from(spans));
    }
    lines
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let track = app.project.tracks.get(app.active_track).cloned();
    let Some(track) = track else { return };
    let pattern_id = track.patterns.first().cloned();
    let Some(pid) = pattern_id else { return };

    let pidx = app.project.patterns.iter().position(|p| p.id() == pid);
    let Some(pidx) = pidx else { return };

    let ticks_per_col: u32 = 120;

    match key.code {
        KeyCode::Char('r') => { app.toggle_pr_midi_record(); return; }
        KeyCode::Up    => { app.pr_pitch = app.pr_pitch.saturating_add(1); }
        KeyCode::Down  => { app.pr_pitch = app.pr_pitch.saturating_sub(1); }
        KeyCode::Right => {
            app.pr_tick += ticks_per_col;
            let pat_len = match &app.project.patterns[pidx] {
                Pattern::PianoRoll(pr) => pr.ticks_per_bar * pr.bars as u32,
                _ => 0,
            };
            if pat_len > 0 {
                app.pr_tick = app.pr_tick.min(pat_len.saturating_sub(ticks_per_col));
            }
        }
        KeyCode::Left  => { app.pr_tick = app.pr_tick.saturating_sub(ticks_per_col); }
        KeyCode::Enter => {
            if let Pattern::PianoRoll(pr) = &mut app.project.patterns[pidx] {
                let exists = pr.notes.iter().any(|n| n.pitch == app.pr_pitch && n.tick == app.pr_tick);
                if !exists {
                    pr.notes.push(Note {
                        pitch:    app.pr_pitch,
                        tick:     app.pr_tick,
                        duration: ticks_per_col,
                        vel:      80,
                    });
                    pr.notes.sort_by_key(|n| (n.tick, n.pitch));
                }
            }
            app.send(SequencerCommand::UpdateLive(app.project.clone()));
        }
        KeyCode::Delete | KeyCode::Backspace => {
            if let Pattern::PianoRoll(pr) = &mut app.project.patterns[pidx] {
                pr.notes.retain(|n| !(n.pitch == app.pr_pitch && n.tick == app.pr_tick));
            }
            app.send(SequencerCommand::UpdateLive(app.project.clone()));
        }
        _ => {}
    }
}

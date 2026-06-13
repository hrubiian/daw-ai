use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent};
use daw_core::project::{Pattern, StepPattern, StepRow, TrackType};
use daw_core::sequencer::SequencerCommand;

fn gm_drum_name(pitch: u8) -> &'static str {
    match pitch {
        35 => "Bass Drum 2",
        36 => "Bass Drum 1",
        37 => "Side Stick",
        38 => "Snare",
        39 => "Hand Clap",
        40 => "Elec Snare",
        41 => "Lo Floor Tom",
        42 => "Closed HH",
        43 => "Hi Floor Tom",
        44 => "Pedal HH",
        45 => "Low Tom",
        46 => "Open HH",
        47 => "Lo-Mid Tom",
        48 => "Hi-Mid Tom",
        49 => "Crash 1",
        50 => "High Tom",
        51 => "Ride 1",
        52 => "Chinese Cym",
        53 => "Ride Bell",
        54 => "Tambourine",
        55 => "Splash Cym",
        56 => "Cowbell",
        57 => "Crash 2",
        58 => "Vibraslap",
        59 => "Ride 2",
        60 => "Hi Bongo",
        61 => "Lo Bongo",
        62 => "Mute Hi Conga",
        63 => "Open Hi Conga",
        64 => "Lo Conga",
        65 => "Hi Timbale",
        66 => "Lo Timbale",
        _  => "—",
    }
}
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let track = app.project.tracks.get(app.active_track);
    let Some(track) = track else {
        frame.render_widget(
            Paragraph::new("No track selected. Press Ctrl+1 to go to Tracks."),
            area,
        );
        return;
    };

    if track.track_type != TrackType::Drum {
        frame.render_widget(
            Paragraph::new("Selected track is melodic. Use Piano Roll (3) instead."),
            area,
        );
        return;
    }

    let pat_count = track.patterns.len();
    let pat_sel   = app.seq_pat.min(pat_count.saturating_sub(1));
    let pattern = track.patterns.get(pat_sel)
        .and_then(|pid| app.project.patterns.iter().find(|p| p.id() == pid));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(3)])
        .split(area);

    let header = format!(
        " Track: {}  Pattern [{}/{}]: {}   [Tab] switch  [n] new",
        track.name,
        if pat_count == 0 { 0 } else { pat_sel + 1 },
        pat_count,
        pattern.map(|p| p.name()).unwrap_or("none"),
    );
    frame.render_widget(Paragraph::new(header), chunks[0]);

    let Some(Pattern::Step(sp)) = pattern else {
        frame.render_widget(
            Paragraph::new("No step pattern attached. Add one in Tracks view.")
                .block(Block::default().borders(Borders::ALL)),
            chunks[1],
        );
        return;
    };

    render_grid(frame, chunks[1], app, sp);

    let help = Paragraph::new("[↑↓] row  [←→] step  [Space] toggle  [<][>] velocity  [u][U] euclid  [,][.] pitch  [r] row  [n] new pattern  [Tab] switch")
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(help, chunks[2]);
}

fn render_grid(frame: &mut Frame, area: Rect, app: &App, sp: &StepPattern) {
    let total = sp.steps as usize;

    // Estimate how many steps fit horizontally.
    // Label column = 18 chars, borders = 2; each step in the header = 2 chars + 1 space every 4.
    // Use 3 chars/step as a conservative estimate.
    let avail = (area.width as usize).saturating_sub(20);
    let visible = (avail / 3).clamp(4, total);

    // Scroll: keep cursor in the visible window.
    let scroll = if app.seq_col < visible {
        0
    } else {
        (app.seq_col + 1).saturating_sub(visible)
    };
    let scroll = scroll.min(total.saturating_sub(visible));
    let end = (scroll + visible).min(total);

    let mut lines: Vec<Line> = Vec::new();

    // Column header row
    let mut header_spans = vec![Span::raw(format!("{:<18}", ""))];
    for s in scroll..end {
        let sep = if s % 4 == 0 && s > scroll { " " } else { "" };
        header_spans.push(Span::styled(
            format!("{sep}{:>2}", s + 1),
            Style::default().fg(Color::DarkGray),
        ));
    }
    lines.push(Line::from(header_spans));

    for (row_idx, row) in sp.rows.iter().enumerate() {
        let is_row_selected = row_idx == app.seq_row;
        let drum_name = gm_drum_name(row.pitch);
        let label = if drum_name == "—" {
            format!("{:<9} p{:>3}", &row.name[..row.name.len().min(9)], row.pitch)
        } else {
            format!("{:<9} {:>8}", &row.name[..row.name.len().min(9)], &drum_name[..drum_name.len().min(8)])
        };

        let row_style = if is_row_selected {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        };
        let mut spans = vec![Span::styled(format!("{:<18}", label), row_style)];

        for s in scroll..end {
            let sep = if s % 4 == 0 && s > scroll { " " } else { "" };
            // Effective velocity for this step (None when off).
            let vel = row.step_velocity(s);
            let is_cursor = row_idx == app.seq_row && s == app.seq_col;
            // Graded glyph + colour by velocity, so accents/ghosts are visible.
            let (glyph, base_color) = match vel {
                None             => ("·", Color::DarkGray),
                Some(v) if v < 48  => ("░", Color::Blue),
                Some(v) if v < 80  => ("▒", Color::Cyan),
                Some(v) if v < 108 => ("▓", Color::Green),
                Some(_)            => ("█", Color::LightGreen),
            };
            let cell_str = format!("{sep}{glyph}");
            let style = if is_cursor {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::REVERSED)
            } else {
                Style::default().fg(base_color)
            };
            spans.push(Span::styled(cell_str, style));
        }
        lines.push(Line::from(spans));
    }

    let scroll_info = if total > visible {
        format!(" ({}-{} of {})", scroll + 1, end, total)
    } else {
        String::new()
    };
    let title = format!("Step Sequencer{scroll_info}  gate {:.2} [(/)]  [e] +16 steps  [,][.] pitch  [r] add row", sp.gate);

    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let track = app.project.tracks.get(app.active_track).cloned();
    let Some(track) = track else { return };
    let pat_count = track.patterns.len();
    if pat_count == 0 { return; }
    app.seq_pat = app.seq_pat.min(pat_count - 1);

    // Tab / Shift+Tab cycle which of the track's patterns is being edited.
    if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
        if matches!(key.code, KeyCode::Tab) {
            app.seq_pat = (app.seq_pat + 1) % pat_count;
        } else {
            app.seq_pat = (app.seq_pat + pat_count - 1) % pat_count;
        }
        app.seq_row = 0;
        app.seq_col = 0;
        return;
    }

    let pid = track.patterns[app.seq_pat].clone();

    let pattern_idx = app.project.patterns.iter().position(|p| p.id() == pid);
    let Some(pidx) = pattern_idx else { return };

    let steps = if let Pattern::Step(sp) = &app.project.patterns[pidx] {
        sp.steps as usize
    } else { return };

    let rows = if let Pattern::Step(sp) = &app.project.patterns[pidx] {
        sp.rows.len()
    } else { return };

    // Clamp cursor to current pattern bounds
    app.seq_row = app.seq_row.min(rows.saturating_sub(1));
    app.seq_col = app.seq_col.min(steps.saturating_sub(1));

    match key.code {
        KeyCode::Up    => { app.seq_row = app.seq_row.saturating_sub(1); }
        KeyCode::Down  => { app.seq_row = (app.seq_row + 1).min(rows.saturating_sub(1)); }
        KeyCode::Left  => { app.seq_col = app.seq_col.saturating_sub(1); }
        KeyCode::Right => { app.seq_col = (app.seq_col + 1).min(steps.saturating_sub(1)); }
        KeyCode::Char(' ') | KeyCode::Enter => {
            if let Pattern::Step(sp) = &mut app.project.patterns[pidx] {
                if let Some(row) = sp.rows.get_mut(app.seq_row) {
                    if let Some(cell) = row.steps.get_mut(app.seq_col) {
                        *cell = if *cell == 0 { 1 } else { 0 };
                    }
                }
            }
            app.send(SequencerCommand::UpdateLive(app.project.clone()));
        }
        // Velocity edit for the cursor step: < quieter, > louder (step 8).
        // Dropping below 1 turns the step off; range is clamped to 1..=127.
        KeyCode::Char('<') | KeyCode::Char('>') => {
            let up = matches!(key.code, KeyCode::Char('>'));
            if let Pattern::Step(sp) = &mut app.project.patterns[pidx] {
                if let Some(row) = sp.rows.get_mut(app.seq_row) {
                    let default_vel = row.vel;
                    if let Some(cell) = row.steps.get_mut(app.seq_col) {
                        // Resolve the current effective velocity, then nudge it.
                        let cur = match *cell { 0 => 0, 1 => default_vel, v => v };
                        *cell = if up {
                            if cur == 0 { default_vel } else { cur.saturating_add(8).min(127) }
                        } else if cur <= 8 {
                            0
                        } else {
                            cur - 8
                        };
                    }
                }
            }
            app.send(SequencerCommand::UpdateLive(app.project.clone()));
        }
        // Euclidean fill: u spreads one more pulse evenly across the row,
        // U removes one. Replaces the selected row's rhythm with E(pulses, steps).
        KeyCode::Char('u') | KeyCode::Char('U') => {
            let densify = matches!(key.code, KeyCode::Char('u'));
            if let Pattern::Step(sp) = &mut app.project.patterns[pidx] {
                let total = sp.steps as usize;
                if let Some(row) = sp.rows.get_mut(app.seq_row) {
                    let cur = row.steps.iter().filter(|&&s| s > 0).count();
                    let pulses = if densify { (cur + 1).min(total) } else { cur.saturating_sub(1) };
                    row.steps = daw_core::generative::euclidean(pulses, total, 0);
                }
            }
            app.send(SequencerCommand::UpdateLive(app.project.clone()));
        }
        // Articulation: ( shorter / ) longer note gate for the whole pattern.
        KeyCode::Char('(') | KeyCode::Char(')') => {
            let up = matches!(key.code, KeyCode::Char(')'));
            if let Pattern::Step(sp) = &mut app.project.patterns[pidx] {
                sp.gate = (sp.gate + if up { 0.05 } else { -0.05 }).clamp(0.1, 2.0);
            }
            app.send(SequencerCommand::UpdateLive(app.project.clone()));
        }
        KeyCode::Char(',') => {
            if let Pattern::Step(sp) = &mut app.project.patterns[pidx] {
                if let Some(row) = sp.rows.get_mut(app.seq_row) {
                    row.pitch = row.pitch.saturating_sub(1);
                    app.send(SequencerCommand::UpdateLive(app.project.clone()));
                }
            }
        }
        KeyCode::Char('.') => {
            if let Pattern::Step(sp) = &mut app.project.patterns[pidx] {
                if let Some(row) = sp.rows.get_mut(app.seq_row) {
                    row.pitch = row.pitch.saturating_add(1).min(127);
                    app.send(SequencerCommand::UpdateLive(app.project.clone()));
                }
            }
        }
        KeyCode::Char('r') => {
            if let Pattern::Step(sp) = &mut app.project.patterns[pidx] {
                sp.rows.push(StepRow {
                    name: format!("Row {}", sp.rows.len() + 1),
                    pitch: 36,
                    vel: 80,
                    steps: vec![0; sp.steps as usize],
                });
            }
            app.send(SequencerCommand::UpdateLive(app.project.clone()));
        }
        // New pattern: add an empty pattern (cloning the current kit layout) to
        // the active track and select it for editing. Arrange it (tab 7) to play.
        KeyCode::Char('n') => {
            let (steps_n, bars_n, gate_v, rows) = if let Pattern::Step(sp) = &app.project.patterns[pidx] {
                let cleared = sp.rows.iter()
                    .map(|r| StepRow { name: r.name.clone(), pitch: r.pitch, vel: r.vel, steps: vec![0; sp.steps as usize] })
                    .collect::<Vec<_>>();
                (sp.steps, sp.bars, sp.gate, cleared)
            } else {
                (16u16, 1u8, 0.5f32, vec![
                    StepRow { name: "Kick".into(),  pitch: 36, vel: 100, steps: vec![0; 16] },
                    StepRow { name: "Snare".into(), pitch: 38, vel: 90,  steps: vec![0; 16] },
                    StepRow { name: "Hat".into(),   pitch: 42, vel: 60,  steps: vec![0; 16] },
                ])
            };
            let mut k = app.project.patterns.len() + 1;
            let new_id = loop {
                let id = format!("p{k}");
                if !app.project.patterns.iter().any(|p| p.id() == id) { break id; }
                k += 1;
            };
            let name = format!("Pattern {}", track.patterns.len() + 1);
            app.project.patterns.push(Pattern::Step(StepPattern {
                id: new_id.clone(), name, bars: bars_n, steps: steps_n, rows, gate: gate_v,
            }));
            if let Some(t) = app.project.tracks.get_mut(app.active_track) {
                t.patterns.push(new_id);
                app.seq_pat = t.patterns.len() - 1;
            }
            app.seq_row = 0;
            app.seq_col = 0;
            app.send(SequencerCommand::UpdateLive(app.project.clone()));
        }
        KeyCode::Char('e') => {
            if let Pattern::Step(sp) = &mut app.project.patterns[pidx] {
                let new_steps = sp.steps.saturating_add(16).min(256);
                let added = (new_steps - sp.steps) as usize;
                if added > 0 {
                    sp.steps = new_steps;
                    for row in &mut sp.rows {
                        row.steps.extend(std::iter::repeat(0).take(added));
                    }
                    app.send(SequencerCommand::UpdateLive(app.project.clone()));
                }
            }
        }
        KeyCode::Char('w') => {
            if let Pattern::Step(sp) = &mut app.project.patterns[pidx] {
                let new_steps = sp.steps.saturating_sub(16).max(16);
                if new_steps != sp.steps {
                    sp.steps = new_steps;
                    let n = new_steps as usize;
                    for row in &mut sp.rows {
                        row.steps.truncate(n);
                    }
                    app.seq_col = app.seq_col.min(n.saturating_sub(1));
                    app.send(SequencerCommand::UpdateLive(app.project.clone()));
                }
            }
        }
        _ => {}
    }
}

use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent};
use daw_core::mini_notation;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

const CHEATSHEET: &[&str] = &[
    "bd sd hh hh   sequence (equal slots)      ~        rest",
    "[a b]         sub-group (one slot)        a , b    stack (layer)",
    "a*3           repeat in slot              a!3      replicate",
    "a@3 b         weight (3:1 time)           bd(3,8)  euclid (+rot: (3,8,2))",
    "tokens: bd sd hh oh cp rs  ·  c3 e4 g#3 eb2  ·  raw MIDI 60",
];

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title("9 Code — live mini-notation");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // target
            Constraint::Length(1), // spacer
            Constraint::Length(1), // input
            Constraint::Length(1), // status
            Constraint::Length(1), // spacer
            Constraint::Min(1),    // cheatsheet
        ])
        .split(inner);

    let tracks = &app.project.tracks;
    if tracks.is_empty() {
        frame.render_widget(Paragraph::new("No tracks. Add one in the Tracks tab."), chunks[0]);
        return;
    }
    let target = app.repl_target.min(tracks.len() - 1);
    let tid = &tracks[target].id;
    let buf = app.repl_bufs.get(tid).cloned().unwrap_or_default();

    frame.render_widget(Paragraph::new(Line::from(vec![
        Span::styled("target ▸ ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{} {}", tid, tracks[target].name),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled("    [↑↓] track   [Enter] eval   [Esc] back", Style::default().fg(Color::DarkGray)),
    ])), chunks[0]);

    // live parse feedback as you type
    let parse_hint = if buf.trim().is_empty() {
        Span::styled("type a pattern…", Style::default().fg(Color::DarkGray))
    } else {
        match mini_notation::parse(&buf) {
            Ok(evs) => Span::styled(format!("  ⟶ {} events", evs.len()), Style::default().fg(Color::Green)),
            Err(e)  => Span::styled(format!("  ⚠ {e}"), Style::default().fg(Color::Yellow)),
        }
    };
    frame.render_widget(Paragraph::new(Line::from(vec![
        Span::styled("> ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{buf}_"), Style::default().fg(Color::White)),
        parse_hint,
    ])), chunks[2]);

    let status_style = if app.repl_status.starts_with('✓') {
        Style::default().fg(Color::Green)
    } else if app.repl_status.starts_with('✗') {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    frame.render_widget(Paragraph::new(Span::styled(app.repl_status.clone(), status_style)), chunks[3]);

    let lines: Vec<Line> = CHEATSHEET.iter()
        .map(|s| Line::from(Span::styled(*s, Style::default().fg(Color::DarkGray))))
        .collect();
    frame.render_widget(Paragraph::new(lines), chunks[5]);
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let n = app.project.tracks.len();
    if n == 0 { return; }
    app.repl_target = app.repl_target.min(n - 1);
    let tid = app.project.tracks[app.repl_target].id.clone();

    match key.code {
        KeyCode::Esc       => { app.active_tab = crate::app::Tab::Tracks; }
        KeyCode::Up        => { app.repl_target = app.repl_target.saturating_sub(1); }
        KeyCode::Down      => { app.repl_target = (app.repl_target + 1).min(n - 1); }
        KeyCode::Enter     => { app.repl_eval(); }
        KeyCode::Backspace => { app.repl_bufs.entry(tid).or_default().pop(); }
        KeyCode::Char(c)   => { app.repl_bufs.entry(tid).or_default().push(c); }
        _ => {}
    }
}

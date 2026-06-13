use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

const LABEL_W: usize = 13;
const CELL_W:  usize = 10; // chars per bar column

fn song_bars(app: &App) -> usize {
    let from_arr = app.project.arrangement.iter()
        .filter_map(|s| {
            let p = app.project.patterns.iter().find(|p| p.id() == s.pattern)?;
            Some(s.bar + p.bars())
        })
        .max()
        .unwrap_or(0) as usize;
    from_arr.max(app.arr_bar + 1).max(8)
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let total = song_bars(app);
    let arr_mode = if app.project.arrangement.is_empty() { "loop" } else { "arrange" };
    frame.render_widget(
        Paragraph::new(format!(
            " Bar: {}   Song: {} bars   Mode: {}   [Enter] place  [Del] remove  [Space] play",
            app.arr_bar + 1, total, arr_mode,
        )).style(Style::default().fg(Color::Cyan)),
        chunks[0],
    );

    render_grid(frame, chunks[1], app, total);

    frame.render_widget(
        Paragraph::new(" [↑↓] track   [←→] bar   [Enter] place / cycle pattern   [Del] remove block")
            .style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

fn render_grid(frame: &mut Frame, area: Rect, app: &App, total_bars: usize) {
    let inner_w = area.width.saturating_sub(2) as usize; // subtract borders
    let avail   = inner_w.saturating_sub(LABEL_W);
    let visible = (avail / CELL_W).max(1);

    // Keep cursor in view
    let scroll = if app.arr_bar < visible {
        0
    } else {
        app.arr_bar + 1 - visible
    };

    let mut lines: Vec<Line> = Vec::new();

    // ── header row: bar numbers ──────────────────────────────────────────────
    let mut hdr: Vec<Span> = vec![Span::raw(format!("{:width$}", "", width = LABEL_W))];
    for b in scroll..(scroll + visible) {
        let label = format!("{:^width$}", b + 1, width = CELL_W);
        hdr.push(Span::styled(
            label,
            if b == app.arr_bar {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ));
    }
    lines.push(Line::from(hdr));

    // ── track rows ───────────────────────────────────────────────────────────
    for (ti, track) in app.project.tracks.iter().enumerate() {
        let is_active = ti == app.arr_track;
        let label_style = if is_active {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let truncated_name: String = track.name.chars().take(LABEL_W - 1).collect();
        let mut spans: Vec<Span> = vec![Span::styled(
            format!("{:<width$}", truncated_name, width = LABEL_W),
            label_style,
        )];

        for b in scroll..(scroll + visible) {
            let b32 = b as u32;
            let is_cursor = is_active && b == app.arr_bar;

            // Find the arrangement slot that covers bar b for this track
            let slot = app.project.arrangement.iter().find(|s| {
                if s.track != track.id { return false; }
                let pb = app.project.patterns.iter()
                    .find(|p| p.id() == s.pattern)
                    .map(|p| p.bars())
                    .unwrap_or(1);
                s.bar <= b32 && b32 < s.bar + pb
            });

            let (cell_text, filled) = if let Some(sl) = slot {
                let pat = app.project.patterns.iter().find(|p| p.id() == sl.pattern);
                let pb   = pat.map(|p| p.bars()).unwrap_or(1);
                let name = pat.map(|p| p.name()).unwrap_or("?");
                let is_start = sl.bar == b32;
                let is_last  = b32 == sl.bar + pb - 1;

                let text = if pb == 1 {
                    // single-bar block: "[name    ]"
                    let n: String = name.chars().take(CELL_W - 2).collect();
                    format!("[{:<width$}]", n, width = CELL_W - 2)
                } else if is_start {
                    // first cell of multi-bar: "[name────"
                    let n: String = name.chars().take(CELL_W - 2).collect();
                    let n_chars = n.chars().count();
                    let dashes = CELL_W - 1 - n_chars;
                    format!("[{}{}", n, "-".repeat(dashes))
                } else if is_last {
                    // last cell: "────────]"
                    format!("{}]", "-".repeat(CELL_W - 1))
                } else {
                    // middle cell
                    "-".repeat(CELL_W)
                };
                (text, true)
            } else {
                ("·".repeat(CELL_W), false)
            };

            let style = if is_cursor && filled {
                Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
            } else if is_cursor {
                Style::default().fg(Color::Yellow).bg(Color::DarkGray)
            } else if filled {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            spans.push(Span::styled(cell_text, style));
        }

        lines.push(Line::from(spans));
    }

    // Pad empty rows if there are fewer tracks than display height
    let used = lines.len();
    let avail_rows = area.height.saturating_sub(2) as usize; // subtract borders
    for _ in used..avail_rows {
        lines.push(Line::from(Span::raw("")));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title("Arrangement")),
        area,
    );
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let track_count = app.project.tracks.len();
    match key.code {
        KeyCode::Up   => {
            app.arr_track = app.arr_track.saturating_sub(1);
        }
        KeyCode::Down => {
            if track_count > 0 {
                app.arr_track = (app.arr_track + 1).min(track_count - 1);
            }
        }
        KeyCode::Left  => {
            app.arr_bar = app.arr_bar.saturating_sub(1);
        }
        KeyCode::Right => {
            app.arr_bar += 1;
        }
        KeyCode::Enter => {
            app.arr_place();
        }
        KeyCode::Delete | KeyCode::Backspace => {
            app.arr_remove();
        }
        _ => {}
    }
}

use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use daw_core::project::{Instrument, TrackType};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(area);

    let items: Vec<ListItem> = app.project.tracks.iter().enumerate().map(|(i, t)| {
        let type_icon = match t.track_type {
            TrackType::Drum    => "DR",
            TrackType::Melodic => "ML",
            TrackType::Audio   => "AU",
        };
        let instrument_label = match &t.instrument {
            Instrument::Soundfont { preset, bank, .. } => format!("sf2 p{preset} b{bank}"),
            Instrument::AudioFile { .. } => String::new(), // waveform rendered as its own span
            Instrument::Sampler { pads } => format!("smplr ({} pads)", pads.len()),
            Instrument::Clap(slot) => format!("clap {}", slot.id),
        };
        let vol_filled = (t.volume * 10.0) as usize;
        let vol_bar = format!(
            "[{}{}]",
            "#".repeat(vol_filled),
            ".".repeat(10usize.saturating_sub(vol_filled))
        );
        let muted = if t.muted { " [M]" } else { "" };

        // Show rename buffer or REC indicator when applicable
        let is_rec = app.is_recording && i == app.active_track;
        let name_display = if i == app.active_track && app.rename_mode {
            format!("{}_", app.rename_buf)
        } else if is_rec {
            format!("[REC] {}", t.name)
        } else {
            t.name.clone()
        };

        let name_style = if is_rec {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let label_span = if t.track_type == TrackType::Audio {
            let mini = mini_waveform(app.waveform_cache.get(t.id.as_str()), 16);
            Span::styled(mini, Style::default().fg(Color::Green))
        } else {
            Span::styled(
                format!("{:<16}", instrument_label),
                Style::default().fg(Color::DarkGray),
            )
        };

        let line = Line::from(vec![
            Span::styled(
                format!("  [{type_icon}] {:<16}", name_display),
                name_style,
            ),
            label_span,
            Span::styled(vol_bar, Style::default().fg(Color::Green)),
            Span::styled(muted, Style::default().fg(Color::Red)),
        ]);
        if i == app.active_track {
            ListItem::new(line).style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
        } else {
            ListItem::new(line)
        }
    }).collect();

    let mut state = ListState::default();
    state.select(Some(app.active_track));

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Tracks"));
    frame.render_stateful_widget(list, chunks[0], &mut state);

    let bottom = if app.rename_mode {
        Paragraph::new(format!("  Rename: {}_", app.rename_buf))
            .style(Style::default().fg(Color::Yellow))
            .block(Block::default().borders(Borders::ALL).title("Rename  [Enter] confirm  [Esc] cancel"))
    } else {
        let rec_hint = if app.record_ctrl.available() { "  [Ctrl+R] rec" } else { "" };
        let met_state = if app.project.project.metronome { "ON " } else { "OFF" };
        let met_style = if app.project.project.metronome { " fg=yellow" } else { "" };
        let _ = met_style; // used via Span below
        let met_label = format!("  [Ctrl+N] metronome:{}", met_state);
        use ratatui::text::Line as TLine;
        use ratatui::text::Span as TSpan;
        let met_color = if app.project.project.metronome { Color::Yellow } else { Color::DarkGray };
        let line = TLine::from(vec![
            TSpan::raw(format!("[↑↓] navigate  [m] mute  [t] type  [r] rename  [a] add  [Ctrl+A] audio  [Ctrl+D] del{rec_hint}")),
            TSpan::styled(met_label, Style::default().fg(met_color).add_modifier(Modifier::BOLD)),
        ]);
        Paragraph::new(line)
            .block(Block::default().borders(Borders::ALL).title("Help"))
    };
    frame.render_widget(bottom, chunks[1]);
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Up   => { app.active_track = app.active_track.saturating_sub(1); }
        KeyCode::Down => {
            let max = app.project.tracks.len().saturating_sub(1);
            app.active_track = (app.active_track + 1).min(max);
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            add_track(app, TrackType::Audio);
        }
        KeyCode::Char('a') => {
            add_track(app, TrackType::Melodic);
        }
        KeyCode::Char('t') => {
            if let Some(track) = app.project.tracks.get_mut(app.active_track) {
                match track.track_type {
                    TrackType::Drum => {
                        track.track_type = TrackType::Melodic;
                        track.instrument = Instrument::Soundfont { preset: 0, bank: 0, soundfont: None };
                    }
                    TrackType::Melodic => {
                        track.track_type = TrackType::Drum;
                        track.instrument = Instrument::Soundfont { preset: 0, bank: 128, soundfont: None };
                    }
                    TrackType::Audio => {} // audio type is set via Ctrl+A; skip cycling into it
                }
                app.send(daw_core::sequencer::SequencerCommand::UpdateLive(app.project.clone()));
            }
        }
        KeyCode::Char('r') => {
            if let Some(track) = app.project.tracks.get(app.active_track) {
                app.rename_buf = track.name.clone();
                app.rename_mode = true;
            }
        }
        KeyCode::Char('m') => {
            // Mute toggles live without restarting the playhead.
            let info = app.project.tracks.get_mut(app.active_track)
                .map(|t| { t.muted = !t.muted; (t.id.clone(), t.muted) });
            if let Some((track_id, muted)) = info {
                app.send(daw_core::sequencer::SequencerCommand::SetTrackMute { track_id, muted });
            }
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.active_track < app.project.tracks.len() {
                let removed = app.project.tracks.remove(app.active_track);

                // Drop arrangement slots that referenced the deleted track.
                app.project.arrangement.retain(|slot| slot.track != removed.id);

                // Drop pattern definitions owned only by the deleted track
                // (keep any still referenced by a surviving track).
                app.project.patterns.retain(|pat| {
                    let owned_by_removed = removed.patterns.iter().any(|pid| pid == pat.id());
                    let used_elsewhere = app.project.tracks.iter()
                        .any(|t| t.patterns.iter().any(|pid| pid == pat.id()));
                    !owned_by_removed || used_elsewhere
                });

                let max = app.project.tracks.len().saturating_sub(1);
                app.active_track = app.active_track.min(max);

                // Reload so the sequencer stops playing the deleted track.
                app.send(daw_core::sequencer::SequencerCommand::UpdateLive(app.project.clone()));
            }
        }
        _ => {}
    }
}

fn mini_waveform(samples: Option<&Vec<f32>>, width: usize) -> String {
    const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let Some(samples) = samples else {
        return format!("{:width$}", "no clip", width = width);
    };
    if samples.is_empty() {
        return format!("{:width$}", "empty", width = width);
    }
    let chunk = (samples.len() / width).max(1);
    (0..width).map(|col| {
        let start = col * chunk;
        let end   = ((col + 1) * chunk).min(samples.len());
        let peak  = samples[start..end].iter().map(|s| s.abs()).fold(0f32, f32::max).min(1.0);
        BLOCKS[(peak * 8.0) as usize]
    }).collect()
}

fn add_track(app: &mut App, track_type: TrackType) {
    use daw_core::project::{
        Instrument, Pattern, PianoRollPattern, StepPattern, StepRow, Track,
    };

    let t_idx = app.project.tracks.iter()
        .filter_map(|t| t.id.trim_start_matches('t').parse::<usize>().ok())
        .max().unwrap_or(0) + 1;
    let p_idx = app.project.patterns.iter()
        .filter_map(|p| p.id().trim_start_matches('p').parse::<usize>().ok())
        .max().unwrap_or(0) + 1;

    let instrument = match track_type {
        TrackType::Audio   => Instrument::AudioFile { path: String::new() },
        TrackType::Drum    => Instrument::Soundfont { preset: 0, bank: 128, soundfont: None },
        TrackType::Melodic => Instrument::Soundfont { preset: 0, bank: 0, soundfont: None },
    };

    // Create a blank pattern for non-audio tracks and attach it immediately.
    let pattern_ids: Vec<String> = match track_type {
        TrackType::Audio => vec![],
        TrackType::Drum => {
            let steps = 16usize;
            let pat = Pattern::Step(StepPattern {
                id:    format!("p{p_idx}"),
                name:  format!("Pattern {p_idx}"),
                bars:  1,
                steps: steps as u16,
                rows: vec![
                    StepRow { name: "Kick".into(),  pitch: 36, vel: 85, steps: vec![0; steps] },
                    StepRow { name: "Snare".into(), pitch: 38, vel: 70, steps: vec![0; steps] },
                    StepRow { name: "HH".into(),    pitch: 42, vel: 52, steps: vec![0; steps] },
                ],
                gate: 0.5,
            });
            app.project.patterns.push(pat);
            vec![format!("p{p_idx}")]
        }
        TrackType::Melodic => {
            let pat = Pattern::PianoRoll(PianoRollPattern {
                id:           format!("p{p_idx}"),
                name:         format!("Pattern {p_idx}"),
                bars:         4,
                ticks_per_bar: 480,
                notes:        vec![],
            });
            app.project.patterns.push(pat);
            vec![format!("p{p_idx}")]
        }
    };

    app.project.tracks.push(Track {
        id: format!("t{t_idx}"),
        name: format!("Track {t_idx}"),
        track_type,
        instrument,
        volume: 0.75,
        pan: 0.0,
        muted: false,
        patterns: pattern_ids,
        effects: daw_core::EffectChain::default(),
        modulators: Vec::new(),
        clap_fx: Vec::new(),
    });

    // Select the new track so the user lands on it immediately.
    app.active_track = app.project.tracks.len() - 1;

}

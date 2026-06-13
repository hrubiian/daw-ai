// crates/daw-tui/src/lib.rs
pub mod app;
pub mod event;
pub mod views;

// MIDI input + mapping now live in daw-io and are shared with daw-gpui.
// Re-exported so existing `crate::midi` / `crate::midi_map` / `daw_tui::midi`
// paths keep working unchanged.
pub use daw_io::{midi, midi_map};

use crate::app::App;
use anyhow::Result;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::Duration;

pub fn run(mut app: App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        // Keep the shared MIDI channel current with whichever track is active
        app.sync_midi_channel();

        // JAM mode: hot-reload the project file if it changed on disk
        app.poll_hot_reload();

        // Drain MIDI events — audio is already sent by the midir thread;
        // here we handle step-recording, button dispatch, and monitor updates
        while let Ok(ev) = app.midi_ev_rx.try_recv() {
            match ev {
                crate::midi::MidiEvent::NoteOn { pitch, velocity } => {
                    if app.active_tab == crate::app::Tab::PianoRoll {
                        if app.pr_midi_record {
                            app.pr_note_on_record(pitch, velocity);
                        } else {
                            app.midi_step_record(pitch, velocity);
                        }
                    }
                }
                crate::midi::MidiEvent::ControlChange { cc, value } => {
                    app.midi_last_cc = Some((cc, value));
                    let knob  = app.midi_map.knob_for_cc(cc);
                    let fader = app.midi_map.fader_for_cc(cc);
                    if let Some(k) = knob        { app.on_knob_cc(k, value); }
                    else if let Some(f) = fader  { app.on_fader_cc(f, value); }
                    let action = app.midi_map.button_for_cc(cc, value);
                    if let Some(action) = action { app.handle_midi_button(action); }
                }
                crate::midi::MidiEvent::NoteOff { pitch } => {
                    if app.pr_midi_record {
                        app.pr_note_off_record(pitch);
                    }
                }
            }
        }

        if let Ok(buf) = app.vis_buf.try_lock() {
            if !buf.is_empty() {
                app.vis_snapshot.clear();
                app.vis_snapshot.extend_from_slice(&buf);
            }
        }

        terminal.draw(|frame| views::render(frame, app))?;

        if crossterm::event::poll(Duration::from_millis(16))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                if key.kind == crossterm::event::KeyEventKind::Press {
                    crate::event::handle_key(app, key);
                }
            }
        }

        if app.should_quit {
            let _ = app.save();
            return Ok(());
        }
    }
}

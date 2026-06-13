// crates/daw-tui/src/event.rs
use crate::app::{App, DialogMode, Tab};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use daw_core::sequencer::SequencerCommand;

/// Maps a QWERTY key to a MIDI pitch for live piano play.
/// Lower row  (z–m + s d g h j):  C4–B4
/// Upper row  (q–u + 2 3 5 6 7):  C5–B5
fn piano_key_pitch(code: KeyCode) -> Option<u8> {
    match code {
        KeyCode::Char('z') => Some(60),  // C4
        KeyCode::Char('s') => Some(61),  // C#4
        KeyCode::Char('x') => Some(62),  // D4
        KeyCode::Char('d') => Some(63),  // D#4
        KeyCode::Char('c') => Some(64),  // E4
        KeyCode::Char('v') => Some(65),  // F4
        KeyCode::Char('g') => Some(66),  // F#4
        KeyCode::Char('b') => Some(67),  // G4
        KeyCode::Char('h') => Some(68),  // G#4
        KeyCode::Char('n') => Some(69),  // A4
        KeyCode::Char('j') => Some(70),  // A#4
        KeyCode::Char('m') => Some(71),  // B4
        KeyCode::Char('q') => Some(72),  // C5
        KeyCode::Char('2') => Some(73),  // C#5
        KeyCode::Char('w') => Some(74),  // D5
        KeyCode::Char('3') => Some(75),  // D#5
        KeyCode::Char('e') => Some(76),  // E5
        KeyCode::Char('r') => Some(77),  // F5
        KeyCode::Char('5') => Some(78),  // F#5
        KeyCode::Char('t') => Some(79),  // G5
        KeyCode::Char('6') => Some(80),  // G#5
        KeyCode::Char('y') => Some(81),  // A5
        KeyCode::Char('7') => Some(82),  // A#5
        KeyCode::Char('u') => Some(83),  // B5
        _ => None,
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    // Dialog mode (Save As / Export WAV): text input, Enter confirms, Esc cancels
    if app.dialog_mode != DialogMode::None {
        match key.code {
            KeyCode::Esc => {
                app.dialog_mode = DialogMode::None;
                app.dialog_buf.clear();
            }
            KeyCode::Enter => {
                match app.dialog_mode.clone() {
                    DialogMode::SaveAs    => app.confirm_save_as(),
                    DialogMode::ExportWav => app.confirm_export_wav(),
                    DialogMode::None      => {}
                }
            }
            KeyCode::Backspace => { app.dialog_buf.pop(); }
            KeyCode::Char(c)   => { app.dialog_buf.push(c); }
            _ => {}
        }
        return;
    }

    // Rename mode: intercept everything except Enter/Esc/Backspace/chars
    if app.rename_mode {
        match key.code {
            KeyCode::Enter => {
                if let Some(track) = app.project.tracks.get_mut(app.active_track) {
                    if !app.rename_buf.is_empty() {
                        track.name = app.rename_buf.clone();
                    }
                }
                app.rename_mode = false;
                app.rename_buf.clear();
            }
            KeyCode::Esc => {
                app.rename_mode = false;
                app.rename_buf.clear();
            }
            KeyCode::Backspace => { app.rename_buf.pop(); }
            KeyCode::Char(c) => { app.rename_buf.push(c); }
            _ => {}
        }
        return;
    }

    // Code tab is a focused text editor: ordinary typing (lowercase letters,
    // digits, space, brackets, …) goes to the mini-notation buffer. But Ctrl
    // combos and capital-letter shortcuts (Shift+J JAM, Shift+H humanize, …) —
    // which mini-notation never uses — fall through to the global handler so
    // they keep working. Esc leaves the tab (handled in the view).
    if app.active_tab == Tab::Code {
        let is_global_shortcut = key.modifiers.contains(KeyModifiers::CONTROL)
            || matches!(key.code, KeyCode::Char(c) if c.is_ascii_uppercase());
        if !is_global_shortcut {
            crate::views::code::handle_key(app, key);
            return;
        }
        // else: fall through to the global shortcut match below
    }

    // Live play mode: piano keys play notes (and step-record on Piano Roll tab).
    // 'l' or Esc exits; all other non-piano keys fall through to normal handling.
    if app.live_play {
        match key.code {
            KeyCode::Char('l') | KeyCode::Esc => {
                app.live_play = false;
                return;
            }
            code => {
                if let Some(pitch) = piano_key_pitch(code) {
                    app.live_play_note(pitch);
                    return;
                }
                // Non-piano key: fall through to normal global/tab handling below
            }
        }
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        // Quit — Ctrl+Q
        KeyCode::Char('q') if ctrl => {
            app.should_quit = true;
            return;
        }
        // Tab switching — plain 1 … 6 (fully disabled in live piano mode)
        KeyCode::Char('1') if !app.live_play => { app.active_tab = Tab::Tracks;    return; }
        KeyCode::Char('2') if !app.live_play => { app.active_tab = Tab::Sequencer; return; }
        KeyCode::Char('3') if !app.live_play => { app.active_tab = Tab::PianoRoll; return; }
        KeyCode::Char('4') if !app.live_play => { app.active_tab = Tab::Mixer;     return; }
        KeyCode::Char('5') if !app.live_play => { app.active_tab = Tab::Browser;   return; }
        KeyCode::Char('6') if !app.live_play => { app.active_tab = Tab::Effects;   return; }
        KeyCode::Char('7') if !app.live_play => { app.active_tab = Tab::Arrange;   return; }
        KeyCode::Char('8') if !app.live_play => { app.active_tab = Tab::Viz;       return; }
        KeyCode::Char('9') if !app.live_play => { app.active_tab = Tab::Code;      return; }
        // Playback
        KeyCode::Char('p') => { app.toggle_play(); return; }
        KeyCode::Char(' ') if app.active_tab == Tab::Tracks
                           || app.active_tab == Tab::Mixer
                           || app.active_tab == Tab::Browser
                           || app.active_tab == Tab::PianoRoll
                           || app.active_tab == Tab::Effects
                           || app.active_tab == Tab::Arrange => {
            app.toggle_play(); return;
        }
        // Save / Save As / Export WAV / Record
        KeyCode::Char('s') if ctrl && key.modifiers.contains(KeyModifiers::SHIFT) => {
            // Ctrl+Shift+S → always prompt for filename
            app.dialog_buf  = app.project_path.as_ref()
                .and_then(|p| p.to_str()).unwrap_or("project.daw.json").into();
            app.dialog_mode = crate::app::DialogMode::SaveAs;
            return;
        }
        KeyCode::Char('s') if ctrl => { app.save_or_prompt(); return; }
        KeyCode::Char('e') if ctrl => {
            app.dialog_buf  = app.project_path.as_ref()
                .and_then(|p| p.file_stem())
                .and_then(|s| s.to_str())
                .unwrap_or("export")
                .trim_end_matches(".daw")
                .to_string();
            app.dialog_mode = crate::app::DialogMode::ExportWav;
            return;
        }
        KeyCode::Char('r') if ctrl => { app.toggle_recording(); return; }
        KeyCode::Char('n') if ctrl => { app.toggle_metronome(); return; }
        // BPM
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.project.project.bpm = (app.project.project.bpm + 1.0).min(300.0);
            app.send(SequencerCommand::SetBpm(app.project.project.bpm));
            return;
        }
        KeyCode::Char('-') => {
            app.project.project.bpm = (app.project.project.bpm - 1.0).max(20.0);
            app.send(SequencerCommand::SetBpm(app.project.project.bpm));
            return;
        }
        // Transpose / panic / live / swing
        KeyCode::Char(']') => { app.transpose(1);           return; }
        KeyCode::Char('[') => { app.transpose(-1);          return; }
        KeyCode::Char('}') => { app.adjust_swing( 0.05);   return; }
        KeyCode::Char('{') => { app.adjust_swing(-0.05);   return; }
        // Velocity humanization — Shift+H cycles 0 → 25 → 50 → 75 → 100 → 0%
        KeyCode::Char('H') => { app.cycle_humanize(); return; }
        // JAM mode — Shift+J toggles live hot-reload of the project file
        KeyCode::Char('J') => { app.toggle_jam(); return; }
        KeyCode::Char('0') => { app.send(SequencerCommand::Panic); return; }
        KeyCode::Char('l') => { app.live_play = true; return; }
        _ => {}
    }

    match app.active_tab {
        Tab::Tracks    => crate::views::tracks::handle_key(app, key),
        Tab::Sequencer => crate::views::sequencer::handle_key(app, key),
        Tab::PianoRoll => crate::views::piano_roll::handle_key(app, key),
        Tab::Mixer     => crate::views::mixer::handle_key(app, key),
        Tab::Browser   => crate::views::browser::handle_key(app, key),
        Tab::Effects   => crate::views::effects::handle_key(app, key),
        Tab::Arrange   => crate::views::arrange::handle_key(app, key),
        Tab::Viz       => crate::views::visualizer::handle_key(app, key),
        Tab::Code      => {} // handled earlier via focused-input routing
    }
}

use crate::app::App;
use crate::midi_map::{FaderParam, KnobParam};
use crossterm::event::{KeyCode, KeyEvent};
use daw_core::project::{LfoShape, ModDest, Modulator, TrackType};
use daw_core::sequencer::SequencerCommand;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

struct EffParam { name: &'static str, step: f32, min: f32, max: f32 }

const MIDI_PARAMS: &[EffParam] = &[
    EffParam { name: "Reverb",    step: 0.05, min: 0.0, max: 1.0 },
    EffParam { name: "Chorus",    step: 0.05, min: 0.0, max: 1.0 },
    EffParam { name: "Cutoff",    step: 0.05, min: 0.0, max: 1.0 },
    EffParam { name: "Resonance", step: 0.05, min: 0.0, max: 1.0 },
    EffParam { name: "LFO Depth", step: 0.05, min: 0.0, max: 1.0 },
];

// Indices 0-4: effects chain, 5-8: EQ, 9-12: Compressor
const AUDIO_PARAMS: &[EffParam] = &[
    // Effect chain
    EffParam { name: "Delay Time (ms)", step: 50.0, min: 0.0,   max: 2000.0 },
    EffParam { name: "Delay Feedback",  step: 0.05, min: 0.0,   max: 1.0    },
    EffParam { name: "Delay Mix",       step: 0.05, min: 0.0,   max: 1.0    },
    EffParam { name: "Cutoff",          step: 0.05, min: 0.0,   max: 1.0    },
    EffParam { name: "Drive",           step: 0.05, min: 0.0,   max: 1.0    },
    // 3-band EQ
    EffParam { name: "EQ Low (dB)",     step: 0.5,  min: -12.0, max: 12.0   },
    EffParam { name: "EQ Mid (dB)",     step: 0.5,  min: -12.0, max: 12.0   },
    EffParam { name: "EQ High (dB)",    step: 0.5,  min: -12.0, max: 12.0   },
    EffParam { name: "EQ Mid Freq (Hz)",step: 100.0,min: 200.0, max: 8000.0 },
    // Compressor
    EffParam { name: "Comp Threshold",  step: 1.0,  min: -60.0, max: 0.0    },
    EffParam { name: "Comp Ratio",      step: 0.5,  min: 1.0,   max: 20.0   },
    EffParam { name: "Comp Attack (ms)",step: 1.0,  min: 0.5,   max: 100.0  },
    EffParam { name: "Comp Release(ms)",step: 10.0, min: 10.0,  max: 1000.0 },
];

const KNOB_ORDER: &[(KnobParam, &str)] = &[
    (KnobParam::Cutoff,    "Cutoff"),
    (KnobParam::Resonance, "Resonance"),
    (KnobParam::LfoRate,   "LFO Rate"),
    (KnobParam::LfoAmt,    "LFO Amt"),
    (KnobParam::Chorus,    "Chorus"),
    (KnobParam::Param1,    "Param 1"),
    (KnobParam::Param2,    "Param 2"),
    (KnobParam::Param3,    "Param 3"),
    (KnobParam::Param4,    "Param 4"),
];

const FADER_ORDER: &[(FaderParam, &str)] = &[
    (FaderParam::Attack,  "Attack"),
    (FaderParam::Decay,   "Decay"),
    (FaderParam::Sustain, "Sustain"),
    (FaderParam::Release, "Release"),
    (FaderParam::HiPass,  "Hi-Pass"),
    (FaderParam::Volume,  "Volume"),
];

fn get_effect_value(app: &App, is_audio: bool, idx: usize) -> f32 {
    let Some(track) = app.project.tracks.get(app.active_track) else { return 0.0 };
    let fx = &track.effects;
    if is_audio {
        match idx {
            0  => fx.delay_time_ms as f32,
            1  => fx.delay_feedback,
            2  => fx.delay_mix,
            3  => fx.cutoff,
            4  => fx.drive,
            5  => fx.eq_low_gain,
            6  => fx.eq_mid_gain,
            7  => fx.eq_high_gain,
            8  => fx.eq_mid_freq,
            9  => fx.comp_threshold,
            10 => fx.comp_ratio,
            11 => fx.comp_attack_ms,
            12 => fx.comp_release_ms,
            _  => 0.0,
        }
    } else {
        match idx {
            0 => fx.reverb,
            1 => fx.chorus,
            2 => fx.cutoff,
            3 => fx.resonance,
            4 => fx.lfo_depth,
            _ => 0.0,
        }
    }
}

fn set_effect_value(app: &mut App, is_audio: bool, idx: usize, delta: f32) {
    let Some(track) = app.project.tracks.get_mut(app.active_track) else { return };
    let fx = &mut track.effects;
    if is_audio {
        match idx {
            0  => { fx.delay_time_ms  = (fx.delay_time_ms as f32 + delta).clamp(0.0, 2000.0) as u32; }
            1  => { fx.delay_feedback = (fx.delay_feedback + delta).clamp(0.0, 0.95); }
            2  => { fx.delay_mix      = (fx.delay_mix      + delta).clamp(0.0, 1.0); }
            3  => { fx.cutoff         = (fx.cutoff         + delta).clamp(0.0, 1.0); }
            4  => { fx.drive          = (fx.drive          + delta).clamp(0.0, 1.0); }
            5  => { fx.eq_low_gain    = (fx.eq_low_gain    + delta).clamp(-12.0, 12.0); }
            6  => { fx.eq_mid_gain    = (fx.eq_mid_gain    + delta).clamp(-12.0, 12.0); }
            7  => { fx.eq_high_gain   = (fx.eq_high_gain   + delta).clamp(-12.0, 12.0); }
            8  => { fx.eq_mid_freq    = (fx.eq_mid_freq    + delta).clamp(200.0, 8000.0); }
            9  => { fx.comp_threshold = (fx.comp_threshold + delta).clamp(-60.0, 0.0); }
            10 => { fx.comp_ratio     = (fx.comp_ratio     + delta).clamp(1.0, 20.0); }
            11 => { fx.comp_attack_ms = (fx.comp_attack_ms + delta).clamp(0.5, 100.0); }
            12 => { fx.comp_release_ms= (fx.comp_release_ms+ delta).clamp(10.0, 1000.0); }
            _  => {}
        }
    } else {
        match idx {
            0 => { fx.reverb    = (fx.reverb    + delta).clamp(0.0, 1.0); }
            1 => { fx.chorus    = (fx.chorus    + delta).clamp(0.0, 1.0); }
            2 => { fx.cutoff    = (fx.cutoff    + delta).clamp(0.0, 1.0); }
            3 => { fx.resonance = (fx.resonance + delta).clamp(0.0, 1.0); }
            4 => { fx.lfo_depth = (fx.lfo_depth + delta).clamp(0.0, 1.0); }
            _ => {}
        }
    }
}

fn bar_float(value: f32, min: f32, max: f32, w: usize) -> String {
    let range = (max - min).max(1e-6);
    let filled = (((value - min) / range) * w as f32).round() as usize;
    let filled = filled.min(w);
    format!("{}{}", "█".repeat(filled), "░".repeat(w - filled))
}

fn bar_cc(value: u8, w: usize) -> String {
    let filled = (value as usize * w) / 127;
    let filled = filled.min(w);
    format!("{}{}", "█".repeat(filled), "░".repeat(w - filled))
}

fn eff_line(name: &str, value: f32, min: f32, max: f32, selected: bool) -> Line<'static> {
    let bar = bar_float(value, min, max, 20);
    let val_str = if max.abs() > 10.0 || min < 0.0 { format!("{:>6.1}", value) } else { format!("{:.2}", value) };
    let cursor = if selected { "▶" } else { " " };
    let ns = if selected { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::White) };
    let bs = if selected { Style::default().fg(Color::Cyan) } else { Style::default().fg(Color::DarkGray) };
    Line::from(vec![
        Span::styled(format!(" {} {:<18}", cursor, name), ns),
        Span::styled(bar, bs),
        Span::styled(format!("  {}", val_str), ns),
    ])
}

fn cc_line(name: &str, cc: u8, value: u8, selected: bool) -> Line<'static> {
    let bar = bar_cc(value, 20);
    let cursor = if selected { "▶" } else { " " };
    let ns = if selected { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::White) };
    let bs = if selected { Style::default().fg(Color::Cyan) } else { Style::default().fg(Color::DarkGray) };
    let cs = Style::default().fg(Color::DarkGray);
    Line::from(vec![
        Span::styled(format!(" {} {:<12}", cursor, name), ns),
        Span::styled(format!("CC{:<4} ", cc), cs),
        Span::styled(bar, bs),
        Span::styled(format!("  {:>3}", value), ns),
    ])
}

fn section_hdr(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("  ── {} ──", title),
        Style::default().fg(Color::DarkGray),
    ))
}

// Returns the flat line index of the selected param (for scroll calculation).
fn selected_line(effects_param: usize, is_midi: bool) -> usize {
    if is_midi {
        let eff_count = MIDI_PARAMS.len();
        if effects_param < eff_count {
            1 + effects_param // header on line 0, params start at 1
        } else if effects_param < eff_count + KNOB_ORDER.len() {
            let ki = effects_param - eff_count;
            eff_count + 3 + ki // blank + knob header + knob index
        } else {
            let fi = effects_param - eff_count - KNOB_ORDER.len();
            eff_count + KNOB_ORDER.len() + 5 + fi
        }
    } else {
        // Audio layout:
        // 0: "Effect Chain" hdr, 1-5: params 0-4
        // 6: blank, 7: "3-Band EQ" hdr, 8-11: params 5-8
        // 12: blank, 13: "Compressor" hdr, 14-17: params 9-12
        match effects_param {
            0..=4  => 1 + effects_param,
            5..=8  => 3 + effects_param,  // +2 for blank+EQ hdr
            9..=12 => 5 + effects_param,  // +4 for blank+EQ hdr+blank+comp hdr
            _      => 1 + effects_param,
        }
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let Some(track) = app.project.tracks.get(app.active_track) else {
        frame.render_widget(
            Paragraph::new("No track selected").block(Block::default().borders(Borders::ALL).title("FX")),
            area,
        );
        return;
    };

    let is_audio = track.track_type == TrackType::Audio;
    let is_midi  = !is_audio;
    let type_lbl = if is_audio { "AU" } else if track.track_type == TrackType::Drum { "DR" } else { "ML" };
    let title    = format!("FX — {}  [{}]  — [↑][↓] select  [←][→] adjust", track.name, type_lbl);

    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let eff_params = if is_audio { AUDIO_PARAMS } else { MIDI_PARAMS };
    let eff_count  = eff_params.len();
    let sel        = app.effects_param;

    let mut lines: Vec<Line> = Vec::new();

    if is_audio {
        // Section 1a: Effect Chain (indices 0-4)
        lines.push(section_hdr("Effect Chain"));
        for i in 0..5usize {
            let p = &eff_params[i];
            let value = get_effect_value(app, true, i);
            lines.push(eff_line(p.name, value, p.min, p.max, sel == i));
        }
        // Section 1b: EQ (indices 5-8)
        lines.push(Line::from(""));
        lines.push(section_hdr("3-Band EQ"));
        for i in 5..9usize {
            let p = &eff_params[i];
            let value = get_effect_value(app, true, i);
            lines.push(eff_line(p.name, value, p.min, p.max, sel == i));
        }
        // Section 1c: Compressor (indices 9-12)
        lines.push(Line::from(""));
        lines.push(section_hdr("Compressor"));
        for i in 9..13usize {
            let p = &eff_params[i];
            let value = get_effect_value(app, true, i);
            lines.push(eff_line(p.name, value, p.min, p.max, sel == i));
        }
    } else {
        // Section 1: MIDI Effect Parameters
        lines.push(section_hdr("Effect Parameters"));
        for (i, p) in eff_params.iter().enumerate() {
            let value = get_effect_value(app, false, i);
            lines.push(eff_line(p.name, value, p.min, p.max, sel == i));
        }
    }

    // Sections 2 & 3: Hardware CC (MIDI / Drum only)
    if is_midi {
        lines.push(Line::from(""));
        lines.push(section_hdr("Hardware Knobs"));
        for (ki, (knob, name)) in KNOB_ORDER.iter().enumerate() {
            let idx   = eff_count + ki;
            let value = app.knob_values.get(knob).copied().unwrap_or(64);
            let cc    = app.midi_map.knobs.get(knob).copied().unwrap_or(0);
            lines.push(cc_line(name, cc, value, sel == idx));
        }

        lines.push(Line::from(""));
        lines.push(section_hdr("Hardware Faders"));
        for (fi, (fader, name)) in FADER_ORDER.iter().enumerate() {
            let idx   = eff_count + KNOB_ORDER.len() + fi;
            let value = app.fader_values.get(fader).copied().unwrap_or(64);
            let cc    = app.midi_map.faders.get(fader).copied().unwrap_or(0);
            lines.push(cc_line(name, cc, value, sel == idx));
        }
    }

    // Section 4: per-track LFO modulators (the modulation matrix)
    lines.push(Line::from(""));
    lines.push(section_hdr("Modulators  [m] add  [x] del  [Tab] sel  [d] dest  [s] shape  [,/.] rate  [</>] depth"));
    if track.modulators.is_empty() {
        lines.push(Line::from(Span::styled(
            "    (none — press m to add an LFO)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for (i, m) in track.modulators.iter().enumerate() {
            let selected = i == app.mod_sel.min(track.modulators.len().saturating_sub(1));
            let cursor = if selected { "▶" } else { " " };
            let st = if selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            lines.push(Line::from(Span::styled(
                format!(" {} {:<7} {:<4}  rate {:>5.2} Hz   depth {:.2}",
                    cursor, m.dest.label(), m.shape.label(), m.rate_hz, m.depth),
                st,
            )));
        }
    }

    // Scroll to keep the selected param visible
    let sel_line = selected_line(sel, is_midi);
    let visible  = inner.height as usize;
    let scroll   = if sel_line + 1 > visible {
        (sel_line + 1 - visible) as u16
    } else {
        0
    };

    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

fn adjust_param(
    app: &mut App,
    is_audio: bool,
    is_midi:  bool,
    eff_params: &[EffParam],
    eff_count:  usize,
    cc_delta:   i16,
    eff_sign:   f32,
) {
    let idx = app.effects_param;
    if idx < eff_count {
        let step = eff_params[idx].step * eff_sign;
        set_effect_value(app, is_audio, idx, step);
        app.apply_effects();
    } else if is_midi {
        let hw = idx - eff_count;
        if hw < KNOB_ORDER.len() {
            let (knob, _) = KNOB_ORDER[hw];
            let current  = app.knob_values.get(&knob).copied().unwrap_or(64);
            let new_val  = ((current as i16) + cc_delta).clamp(0, 127) as u8;
            let cc       = app.midi_map.knobs.get(&knob).copied().unwrap_or(0);
            let channel  = app.active_midi_channel().unwrap_or(0);
            app.on_knob_cc(knob, new_val);
            app.send(SequencerCommand::RawCC { channel, cc, value: new_val });
        } else {
            let fi = hw - KNOB_ORDER.len();
            if fi < FADER_ORDER.len() {
                let (fader, _) = FADER_ORDER[fi];
                let current  = app.fader_values.get(&fader).copied().unwrap_or(64);
                let new_val  = ((current as i16) + cc_delta).clamp(0, 127) as u8;
                let cc       = app.midi_map.faders.get(&fader).copied().unwrap_or(0);
                let channel  = app.active_midi_channel().unwrap_or(0);
                app.on_fader_cc(fader, new_val);
                app.send(SequencerCommand::RawCC { channel, cc, value: new_val });
            }
        }
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let is_audio = app.project.tracks.get(app.active_track)
        .map(|t| t.track_type == TrackType::Audio)
        .unwrap_or(false);
    if app.project.tracks.get(app.active_track).is_none() { return; }

    let is_midi    = !is_audio;
    let eff_params = if is_audio { AUDIO_PARAMS } else { MIDI_PARAMS };
    let eff_count  = eff_params.len();
    let hw_count   = if is_midi { KNOB_ORDER.len() + FADER_ORDER.len() } else { 0 };
    let total      = eff_count + hw_count;

    // Clamp cursor to valid range whenever track type changes
    app.effects_param = app.effects_param.min(total.saturating_sub(1));

    match key.code {
        KeyCode::Up   => { app.effects_param = app.effects_param.saturating_sub(1); }
        KeyCode::Down => { app.effects_param = (app.effects_param + 1).min(total - 1); }
        KeyCode::Left  => adjust_param(app, is_audio, is_midi, eff_params, eff_count, -5, -1.0),
        KeyCode::Right => adjust_param(app, is_audio, is_midi, eff_params, eff_count,  5,  1.0),
        // ── Modulation matrix editing ──────────────────────────────────────
        KeyCode::Char('m') => { add_modulator(app); }
        KeyCode::Char('x') => { remove_modulator(app); }
        KeyCode::Tab       => { cycle_mod_selection(app); }
        KeyCode::Char('d') => edit_modulator(app, |m| m.dest  = next_dest(m.dest)),
        KeyCode::Char('s') => edit_modulator(app, |m| m.shape = next_shape(m.shape)),
        KeyCode::Char('.') => edit_modulator(app, |m| m.rate_hz = (m.rate_hz + 0.05).clamp(0.01, 20.0)),
        KeyCode::Char(',') => edit_modulator(app, |m| m.rate_hz = (m.rate_hz - 0.05).clamp(0.01, 20.0)),
        KeyCode::Char('>') => edit_modulator(app, |m| m.depth = (m.depth + 0.05).clamp(0.0, 1.0)),
        KeyCode::Char('<') => edit_modulator(app, |m| m.depth = (m.depth - 0.05).clamp(0.0, 1.0)),
        _ => {}
    }
}

fn next_dest(d: ModDest) -> ModDest {
    match d {
        ModDest::Cutoff => ModDest::Resonance,
        ModDest::Resonance => ModDest::Volume,
        ModDest::Volume => ModDest::Pan,
        ModDest::Pan => ModDest::Cutoff,
    }
}

fn next_shape(s: LfoShape) -> LfoShape {
    match s {
        LfoShape::Sine => LfoShape::Triangle,
        LfoShape::Triangle => LfoShape::Saw,
        LfoShape::Saw => LfoShape::Square,
        LfoShape::Square => LfoShape::Sine,
    }
}

fn add_modulator(app: &mut App) {
    let sel = {
        let Some(track) = app.project.tracks.get_mut(app.active_track) else { return };
        track.modulators.push(Modulator {
            dest: ModDest::Cutoff, shape: LfoShape::Sine,
            rate_hz: 0.2, depth: 0.4, phase: 0.0,
        });
        track.modulators.len() - 1
    };
    app.mod_sel = sel;
    app.send(SequencerCommand::UpdateLive(app.project.clone()));
}

fn remove_modulator(app: &mut App) {
    let removed = {
        let Some(track) = app.project.tracks.get_mut(app.active_track) else { return };
        if track.modulators.is_empty() { return; }
        let i = app.mod_sel.min(track.modulators.len() - 1);
        track.modulators.remove(i);
        i.saturating_sub(1)
    };
    app.mod_sel = removed;
    app.send(SequencerCommand::UpdateLive(app.project.clone()));
}

fn cycle_mod_selection(app: &mut App) {
    let n = app.project.tracks.get(app.active_track).map(|t| t.modulators.len()).unwrap_or(0);
    if n > 0 { app.mod_sel = (app.mod_sel + 1) % n; }
}

fn edit_modulator(app: &mut App, f: impl FnOnce(&mut Modulator)) {
    let changed = {
        let sel = app.mod_sel;
        match app.project.tracks.get_mut(app.active_track) {
            Some(track) => {
                let i = sel.min(track.modulators.len().saturating_sub(1));
                match track.modulators.get_mut(i) {
                    Some(m) => { f(m); true }
                    None => false,
                }
            }
            None => false,
        }
    };
    if changed {
        app.send(SequencerCommand::UpdateLive(app.project.clone()));
    }
}

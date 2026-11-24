use crate::midi_map::MidiMap;
use crossbeam_channel::{Receiver, Sender};
use daw_core::audio::AudioEvent;
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};

pub enum MidiEvent {
    NoteOn        { pitch: u8, velocity: u8 },
    NoteOff       { pitch: u8 },
    ControlChange { cc: u8, value: u8 },
    KeyLab(KeyLabControl),
}

pub enum KeyLabControl {
    Encoder { index: u8, delta: i8 },
    Fader { index: u8, position: u16 },
}

fn is_keylab_daw_port(name: &str) -> bool {
    name.to_ascii_lowercase().contains("keylab")
        && name.split_whitespace().any(|part| part.eq_ignore_ascii_case("daw"))
}

fn keylab_control(message: &[u8]) -> Option<KeyLabControl> {
    match *message {
        [0xb0, cc @ 16..=23, value @ 0..=127] => {
            let steps = (value & 0x3f) as i8;
            (steps != 0).then_some(KeyLabControl::Encoder {
                index: cc - 16,
                delta: if value & 0x40 != 0 { -steps } else { steps },
            })
        }
        [status @ 0xe0..=0xe8, low @ 0..=127, high @ 0..=127] => {
            Some(KeyLabControl::Fader {
                index: status - 0xe0,
                position: (u16::from(high) << 7) | u16::from(low),
            })
        }
        _ => None,
    }
}

/// Open the separate DAW endpoint on the selected KeyLab's ALSA client.
/// Its Mackie-format controls never enter the musical note stream.
pub fn connect_keylab_daw(midi_port_name: &str, tx: Sender<MidiEvent>) -> Option<MidiHandle> {
    let mut input = midir::MidiInput::new("daw-ai-keylab").ok()?;
    input.ignore(midir::Ignore::All);
    let ports = input.ports();
    let source_id = ports
        .iter()
        .find(|port| input.port_name(port).is_ok_and(|name| {
            name == midi_port_name && !is_keylab_daw_port(&name)
        }))?
        .id();
    let (client, _) = source_id.split_once(':')?;
    let port = ports.iter().find(|port| {
        port.id().split_once(':').is_some_and(|(id, _)| id == client)
            && input.port_name(port).is_ok_and(|name| is_keylab_daw_port(&name))
    })?;
    input
        .connect(port, "daw-keylab-controls", move |_, message, _| {
            if let Some(control) = keylab_control(message) {
                let _ = tx.try_send(MidiEvent::KeyLab(control));
            }
        }, ())
        .ok()
        .map(MidiHandle)
}

/// Keeps the midir connection alive. Drop to disconnect.
pub struct MidiHandle(#[allow(dead_code, reason = "held only to keep the port open")] midir::MidiInputConnection<()>);

/// Names of available MIDI input ports (empty if none / MIDI unavailable).
pub fn list_midi_ports() -> Vec<String> {
    use midir::MidiInput;
    let Ok(input) = MidiInput::new("daw-ai-list") else { return Vec::new() };
    input.ports().iter().filter_map(|p| input.port_name(p).ok()).collect()
}

/// Opens the best available MIDI input (prefers "KeyLab"/"Arturia" by name).
///
/// Returns:
///   - `Receiver<MidiEvent>` — drain in TUI loop for button dispatch + monitor
///   - `Option<MidiHandle>` — keep alive for the duration of the program
///   - `Option<String>`     — device name for display
///   - `Sender<MidiEvent>`  — persistent sender; kept by Host so reconnects can
///     feed the same receiver channel.
pub fn connect_midi(
    audio_tx: Sender<AudioEvent>,
    channel:  Arc<AtomicU8>,
    midi_map: Arc<MidiMap>,
) -> (Receiver<MidiEvent>, Sender<MidiEvent>, Option<MidiHandle>, Option<String>) {
    let (midi_tx, midi_rx) = crossbeam_channel::unbounded::<MidiEvent>();

    // Auto-select port: prefer KeyLab/Arturia, else index 0.
    let port_idx = {
        use midir::{Ignore, MidiInput};
        if let Ok(mut input) = MidiInput::new("daw-ai-probe") {
            input.ignore(Ignore::None);
            let ports = input.ports();
            ports.iter().enumerate()
                .find(|(_, p)| {
                    let n = input.port_name(p).unwrap_or_default().to_lowercase();
                    (n.contains("keylab") || n.contains("arturia")) && !is_keylab_daw_port(&n)
                })
                .map(|(i, _)| i)
                .unwrap_or(0)
        } else {
            0
        }
    };

    match connect_midi_port(port_idx, audio_tx, midi_tx.clone(), channel, midi_map) {
        Some((handle, name)) => (midi_rx, midi_tx, Some(handle), Some(name)),
        None                 => (midi_rx, midi_tx, None, None),
    }
}

/// Connect to a specific MIDI input port by index, feeding events into the
/// provided `midi_tx` / `audio_tx`. Returns the live handle + port name, or
/// `None` if the port does not exist or the connection fails.
pub fn connect_midi_port(
    port_idx: usize,
    audio_tx: Sender<AudioEvent>,
    midi_tx:  Sender<MidiEvent>,
    channel:  Arc<AtomicU8>,
    midi_map: Arc<MidiMap>,
) -> Option<(MidiHandle, String)> {
    use midir::{Ignore, MidiInput};

    let mut input = MidiInput::new("daw-ai").ok()?;
    input.ignore(Ignore::None);

    let ports = input.ports();
    let port = ports.get(port_idx)?;
    let name = input.port_name(port).ok()?;

    let conn = input
        .connect(
            port,
            "daw-midi-in",
            move |_ts, msg, _| {
                if msg.len() < 2 {
                    return;
                }
                let status = msg[0] & 0xF0;
                // The shared atomic carries a mixer *strip index* (= track
                // index), not a GM channel — frontends point it at the
                // selected track so live MIDI plays the selected instrument.
                let ch     = channel.load(Ordering::Relaxed);

                match status {
                    0x90 => {
                        let pitch = msg[1];
                        let vel   = if msg.len() > 2 { msg[2] } else { 0 };
                        if vel > 0 {
                            let _ = audio_tx.try_send(AudioEvent::NoteOn { strip: ch, pitch, velocity: vel });
                            let _ = midi_tx.try_send(MidiEvent::NoteOn { pitch, velocity: vel });
                        } else {
                            let _ = audio_tx.try_send(AudioEvent::NoteOff { strip: ch, pitch });
                            let _ = midi_tx.try_send(MidiEvent::NoteOff { pitch });
                        }
                    }
                    0x80 => {
                        let pitch = msg[1];
                        let _ = audio_tx.try_send(AudioEvent::NoteOff { strip: ch, pitch });
                        let _ = midi_tx.try_send(MidiEvent::NoteOff { pitch });
                    }
                    0xB0 => {
                        let cc    = msg[1];
                        let value = if msg.len() > 2 { msg[2] } else { 0 };
                        // Knob/fader CCs: send directly to audio for zero-latency parameter control
                        if midi_map.is_continuous(cc) {
                            let _ = audio_tx.try_send(AudioEvent::RawCC { strip: ch, cc, value });
                        }
                        // All CCs also go to TUI for monitor display and button dispatch
                        let _ = midi_tx.try_send(MidiEvent::ControlChange { cc, value });
                    }
                    0xE0
                        if midi_map.pitch_bend && msg.len() >= 3 => {
                            let _ = audio_tx.try_send(AudioEvent::PitchBend {
                                strip: ch,
                                lsb: msg[1],
                                msb: msg[2],
                            });
                        }
                    _ => {}
                }
            },
            (),
        )
        .ok()?;

    Some((MidiHandle(conn), name))
}

#[cfg(test)]
mod tests {
    #[test]
    fn list_midi_ports_no_panic() {
        let _ = super::list_midi_ports(); // length >= 0; must not panic
    }

    #[test]
    fn decodes_observed_keylab_encoders_and_faders() {
        use super::{keylab_control, KeyLabControl};
        assert!(matches!(keylab_control(&[0xb0, 16, 3]), Some(KeyLabControl::Encoder { index: 0, delta: 3 })));
        assert!(matches!(keylab_control(&[0xb0, 16, 65]), Some(KeyLabControl::Encoder { index: 0, delta: -1 })));
        assert!(matches!(keylab_control(&[0xe1, 1, 64]), Some(KeyLabControl::Fader { index: 1, position: 8193 })));
        assert!(keylab_control(&[0xb0, 16, 64]).is_none());
        assert!(keylab_control(&[0x90, 60, 100]).is_none());
    }
}

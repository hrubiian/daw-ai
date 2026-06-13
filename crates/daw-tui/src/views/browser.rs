use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};

// All 128 General MIDI instruments grouped into the 16 standard families.
// browser_left = category index (0–15), browser_right = preset index within category (0–7).
const GM_CATEGORIES: &[(&str, &[(&str, u8)])] = &[
    ("Piano", &[
        ("Acoustic Grand Piano",   0),
        ("Bright Acoustic Piano",  1),
        ("Electric Grand Piano",   2),
        ("Honky-tonk Piano",       3),
        ("Electric Piano 1",       4),
        ("Electric Piano 2",       5),
        ("Harpsichord",            6),
        ("Clavinet",               7),
    ]),
    ("Chromatic Perc", &[
        ("Celesta",       8),
        ("Glockenspiel",  9),
        ("Music Box",    10),
        ("Vibraphone",   11),
        ("Marimba",      12),
        ("Xylophone",    13),
        ("Tubular Bells",14),
        ("Dulcimer",     15),
    ]),
    ("Organ", &[
        ("Drawbar Organ",    16),
        ("Percussive Organ", 17),
        ("Rock Organ",       18),
        ("Church Organ",     19),
        ("Reed Organ",       20),
        ("Accordion",        21),
        ("Harmonica",        22),
        ("Bandoneon",        23),
    ]),
    ("Guitar", &[
        ("Nylon Guitar",      24),
        ("Steel Guitar",      25),
        ("Jazz Guitar",       26),
        ("Clean Guitar",      27),
        ("Muted Guitar",      28),
        ("Overdriven Guitar", 29),
        ("Distortion Guitar", 30),
        ("Guitar Harmonics",  31),
    ]),
    ("Bass", &[
        ("Acoustic Bass",        32),
        ("Electric Bass (finger)",33),
        ("Electric Bass (pick)", 34),
        ("Fretless Bass",        35),
        ("Slap Bass 1",          36),
        ("Slap Bass 2",          37),
        ("Synth Bass 1",         38),
        ("Synth Bass 2",         39),
    ]),
    ("Strings", &[
        ("Violin",            40),
        ("Viola",             41),
        ("Cello",             42),
        ("Contrabass",        43),
        ("Tremolo Strings",   44),
        ("Pizzicato Strings", 45),
        ("Orchestral Harp",   46),
        ("Timpani",           47),
    ]),
    ("Ensemble", &[
        ("String Ensemble 1", 48),
        ("String Ensemble 2", 49),
        ("Synth Strings 1",   50),
        ("Synth Strings 2",   51),
        ("Choir Aahs",        52),
        ("Voice Oohs",        53),
        ("Synth Choir",       54),
        ("Orchestra Hit",     55),
    ]),
    ("Brass", &[
        ("Trumpet",       56),
        ("Trombone",      57),
        ("Tuba",          58),
        ("Muted Trumpet", 59),
        ("French Horn",   60),
        ("Brass Section", 61),
        ("Synth Brass 1", 62),
        ("Synth Brass 2", 63),
    ]),
    ("Reed", &[
        ("Soprano Sax",  64),
        ("Alto Sax",     65),
        ("Tenor Sax",    66),
        ("Baritone Sax", 67),
        ("Oboe",         68),
        ("English Horn", 69),
        ("Bassoon",      70),
        ("Clarinet",     71),
    ]),
    ("Pipe", &[
        ("Piccolo",       72),
        ("Flute",         73),
        ("Recorder",      74),
        ("Pan Flute",     75),
        ("Blown Bottle",  76),
        ("Shakuhachi",    77),
        ("Whistle",       78),
        ("Ocarina",       79),
    ]),
    ("Synth Lead", &[
        ("Lead 1 (square)",    80),
        ("Lead 2 (sawtooth)",  81),
        ("Lead 3 (calliope)",  82),
        ("Lead 4 (chiff)",     83),
        ("Lead 5 (charang)",   84),
        ("Lead 6 (voice)",     85),
        ("Lead 7 (fifths)",    86),
        ("Lead 8 (bass+lead)", 87),
    ]),
    ("Synth Pad", &[
        ("Pad 1 (new age)",   88),
        ("Pad 2 (warm)",      89),
        ("Pad 3 (polysynth)", 90),
        ("Pad 4 (choir)",     91),
        ("Pad 5 (bowed)",     92),
        ("Pad 6 (metallic)",  93),
        ("Pad 7 (halo)",      94),
        ("Pad 8 (sweep)",     95),
    ]),
    ("Synth FX", &[
        ("FX 1 (rain)",       96),
        ("FX 2 (soundtrack)", 97),
        ("FX 3 (crystal)",    98),
        ("FX 4 (atmosphere)", 99),
        ("FX 5 (brightness)",100),
        ("FX 6 (goblins)",   101),
        ("FX 7 (echoes)",    102),
        ("FX 8 (sci-fi)",    103),
    ]),
    ("Ethnic", &[
        ("Sitar",    104),
        ("Banjo",    105),
        ("Shamisen", 106),
        ("Koto",     107),
        ("Kalimba",  108),
        ("Bagpipe",  109),
        ("Fiddle",   110),
        ("Shanai",   111),
    ]),
    ("Percussive", &[
        ("Tinkle Bell",    112),
        ("Agogo",          113),
        ("Steel Drums",    114),
        ("Woodblock",      115),
        ("Taiko Drum",     116),
        ("Melodic Tom",    117),
        ("Synth Drum",     118),
        ("Reverse Cymbal", 119),
    ]),
    ("Sound FX", &[
        ("Guitar Fret Noise", 120),
        ("Breath Noise",      121),
        ("Seashore",          122),
        ("Bird Tweet",        123),
        ("Telephone Ring",    124),
        ("Helicopter",        125),
        ("Applause",          126),
        ("Gunshot",           127),
    ]),
];

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
        .split(area);

    // Left pane — categories
    let cat_items: Vec<ListItem> = GM_CATEGORIES.iter().enumerate().map(|(i, (name, _))| {
        let style = if i == app.browser_left {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        ListItem::new(*name).style(style)
    }).collect();

    let mut cat_state = ListState::default();
    cat_state.select(Some(app.browser_left));

    frame.render_stateful_widget(
        List::new(cat_items)
            .block(Block::default().borders(Borders::ALL).title("Category  [←][→]")),
        chunks[0],
        &mut cat_state,
    );

    // Right pane — presets in selected category
    let (_, presets) = GM_CATEGORIES[app.browser_left];
    let preset_items: Vec<ListItem> = presets.iter().enumerate().map(|(i, (name, prog))| {
        let style = if i == app.browser_right {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        ListItem::new(format!("{:>3}  {name}", prog)).style(style)
    }).collect();

    let mut preset_state = ListState::default();
    preset_state.select(Some(app.browser_right));

    frame.render_stateful_widget(
        List::new(preset_items)
            .block(Block::default().borders(Borders::ALL)
                .title("Instruments  [↑][↓] navigate  [Enter] load")),
        chunks[1],
        &mut preset_state,
    );
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let (_, presets) = GM_CATEGORIES[app.browser_left];
    match key.code {
        KeyCode::Left => {
            app.browser_left = app.browser_left.saturating_sub(1);
            app.browser_right = 0;
        }
        KeyCode::Right => {
            app.browser_left = (app.browser_left + 1).min(GM_CATEGORIES.len() - 1);
            app.browser_right = 0;
        }
        KeyCode::Up => {
            app.browser_right = app.browser_right.saturating_sub(1);
        }
        KeyCode::Down => {
            app.browser_right = (app.browser_right + 1).min(presets.len() - 1);
        }
        KeyCode::Enter => {
            let (_, prog) = presets[app.browser_right];
            if let Some(track) = app.project.tracks.get_mut(app.active_track) {
                if track.track_type != daw_core::project::TrackType::Audio {
                    track.instrument = daw_core::project::Instrument::Soundfont { preset: prog, bank: 0, soundfont: None };
                    app.send(daw_core::sequencer::SequencerCommand::UpdateLive(app.project.clone()));
                }
            }
        }
        _ => {}
    }
}

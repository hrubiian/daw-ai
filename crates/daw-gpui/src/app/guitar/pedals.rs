//! Live guitar pedal controls; all settings travel with AmpSettings/presets.
use super::*;
use daw_core::audio::pedals::{
    DriveMode, GuitarEchoMode, ModulationMode, PedalPreset, PedalSettings, ReverbMode,
};

fn knob(
    id: &'static str,
    label: String,
    value: f32,
    get: fn(&mut PedalSettings) -> &mut f32,
    min: f32,
    max: f32,
    step: f32,
    cx: &mut Context<DawView>,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(div().flex_1().text_color(rgb(TEXT_DIM)).child(label))
        .child(
            cell_btn(format!("pedal-{id}-down"), "−", false, PANEL_ALT).on_click(cx.listener(
                move |this, event: &gpui::ClickEvent, _, cx| {
                    let step = if event.modifiers().shift {
                        step / 5.
                    } else {
                        step
                    };
                    this.edit_amp(|a| *get(&mut a.pedals) = (value - step).clamp(min, max));
                    cx.notify();
                },
            )),
        )
        .child(
            cell_btn(format!("pedal-{id}-up"), "+", false, PANEL_ALT).on_click(cx.listener(
                move |this, event: &gpui::ClickEvent, _, cx| {
                    let step = if event.modifiers().shift {
                        step / 5.
                    } else {
                        step
                    };
                    this.edit_amp(|a| *get(&mut a.pedals) = (value + step).clamp(min, max));
                    cx.notify();
                },
            )),
        )
}

fn pedal_panel(title: &'static str) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .bg(rgb(PANEL))
        .child(section_title(title))
}

impl DawView {
    pub(super) fn pedal_starting_points(&self, cx: &mut Context<Self>) -> gpui::Div {
        let current = self.config.guitar.amp.pedals;
        pedal_panel("Pedal combinations")
            .child(div().flex().flex_wrap().gap_1().children([
                (PedalPreset::Dry, "Dry"), (PedalPreset::TightRhythm, "Tight rhythm"),
                (PedalPreset::WideClean, "Clean chorus"), (PedalPreset::LeadEcho, "Lead echo"),
                (PedalPreset::JetFlange, "Jet flange"),
            ].into_iter().map(|(preset, label)| {
                let settings = PedalSettings::preset(preset);
                cell_btn(format!("pedal-preset-{preset:?}"), label, current == settings, self.accent())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.edit_amp(|a| a.pedals = settings); cx.notify();
                    }))
            })))
            .child(div().text_color(rgb(TEXT_DIM)).child("Changes pedals only · save the whole sound under My presets · Shift + click for fine steps"))
    }

    pub(super) fn drive_pedals(&self, cx: &mut Context<Self>) -> gpui::Div {
        let s = self.config.guitar.amp.pedals.drive;
        pedal_panel("Drive pedal · before the amp")
            .child(
                div().flex().flex_wrap().gap_1().children(
                    [
                        (DriveMode::Off, "Off"),
                        (DriveMode::Boost, "Boost"),
                        (DriveMode::Overdrive, "Overdrive"),
                        (DriveMode::Distortion, "Distortion"),
                        (DriveMode::Fuzz, "Fuzz"),
                    ]
                    .into_iter()
                    .map(|(mode, label)| {
                        cell_btn(
                            format!("pedal-drive-{mode:?}"),
                            label,
                            s.mode == mode,
                            self.accent(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.edit_amp(|a| a.pedals.drive.mode = mode);
                            cx.notify();
                        }))
                    }),
                ),
            )
            .child(knob(
                "drive-gain",
                format!("Drive: {:.0}", s.gain * 100.),
                s.gain,
                |p| &mut p.drive.gain,
                0.,
                1.,
                0.05,
                cx,
            ))
            .child(knob(
                "drive-tone",
                format!("Tone: {:.0}", s.tone * 100.),
                s.tone,
                |p| &mut p.drive.tone,
                0.,
                1.,
                0.05,
                cx,
            ))
            .child(knob(
                "drive-level",
                format!("Pedal level: {:.0}", s.level * 100.),
                s.level,
                |p| &mut p.drive.level,
                0.,
                1.,
                0.05,
                cx,
            ))
    }

    pub(super) fn modulation_pedal(&self, cx: &mut Context<Self>) -> gpui::Div {
        let s = self.config.guitar.amp.pedals.modulation;
        let mut panel = pedal_panel("Modulation · after the cabinet")
            .child(
                div().flex().flex_wrap().gap_1().children(
                    [
                        (ModulationMode::Off, "Off"),
                        (ModulationMode::Flanger, "Flanger"),
                        (ModulationMode::Chorus, "Chorus"),
                        (ModulationMode::Tremolo, "Tremolo"),
                    ]
                    .into_iter()
                    .map(|(mode, label)| {
                        cell_btn(
                            format!("pedal-modulation-{mode:?}"),
                            label,
                            s.mode == mode,
                            self.accent(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.edit_amp(|a| {
                                let p = &mut a.pedals.modulation;
                                p.mode = mode;
                                match mode {
                                    ModulationMode::Flanger => {
                                        p.rate_hz = 0.3;
                                        p.depth = 0.6;
                                        p.feedback = 0.35;
                                        p.mix = 0.4;
                                    }
                                    ModulationMode::Chorus => {
                                        p.rate_hz = 0.8;
                                        p.depth = 0.5;
                                        p.feedback = 0.;
                                        p.mix = 0.3;
                                    }
                                    ModulationMode::Tremolo => {
                                        p.rate_hz = 4.;
                                        p.depth = 0.6;
                                        p.mix = 1.;
                                    }
                                    ModulationMode::Off => {}
                                }
                            });
                            cx.notify();
                        }))
                    }),
                ),
            )
            .child(knob(
                "mod-rate",
                format!("Rate: {:.2} Hz", s.rate_hz),
                s.rate_hz,
                |p| &mut p.modulation.rate_hz,
                0.05,
                8.,
                0.05,
                cx,
            ))
            .child(knob(
                "mod-depth",
                format!("Depth: {:.0}%", s.depth * 100.),
                s.depth,
                |p| &mut p.modulation.depth,
                0.,
                1.,
                0.05,
                cx,
            ));
        if s.mode != ModulationMode::Tremolo {
            panel = panel.child(knob(
                "mod-feedback",
                format!("Feedback: {:.0}%", s.feedback * 100.),
                s.feedback,
                |p| &mut p.modulation.feedback,
                -0.85,
                0.85,
                0.05,
                cx,
            ));
        }
        panel.child(knob(
            "mod-mix",
            format!("Mix: {:.0}%", s.mix * 100.),
            s.mix,
            |p| &mut p.modulation.mix,
            0.,
            1.,
            0.05,
            cx,
        ))
    }

    pub(super) fn echo_pedal(&self, cx: &mut Context<Self>) -> gpui::Div {
        let s = self.config.guitar.amp.pedals.echo;
        let mut panel = pedal_panel("Echo")
            .child(
                div().flex().flex_wrap().gap_1().children(
                    [
                        (GuitarEchoMode::Off, "Off"),
                        (GuitarEchoMode::Digital, "Digital"),
                        (GuitarEchoMode::Tape, "Tape"),
                        (GuitarEchoMode::Slapback, "Slapback"),
                    ]
                    .into_iter()
                    .map(|(mode, label)| {
                        cell_btn(
                            format!("pedal-echo-{mode:?}"),
                            label,
                            s.mode == mode,
                            self.accent(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.edit_amp(|a| {
                                let p = &mut a.pedals.echo;
                                p.mode = mode;
                                match mode {
                                    GuitarEchoMode::Digital => {
                                        p.time_ms = 330.;
                                        p.feedback = 0.3;
                                        p.mix = 0.2;
                                    }
                                    GuitarEchoMode::Tape => {
                                        p.time_ms = 360.;
                                        p.feedback = 0.35;
                                        p.mix = 0.2;
                                    }
                                    GuitarEchoMode::Slapback => {
                                        p.time_ms = 110.;
                                        p.feedback = 0.;
                                        p.mix = 0.2;
                                    }
                                    GuitarEchoMode::Off => {}
                                }
                            });
                            cx.notify();
                        }))
                    }),
                ),
            )
            .child(knob(
                "echo-time",
                format!("Time: {:.0} ms", s.time_ms),
                s.time_ms,
                |p| &mut p.echo.time_ms,
                20.,
                if s.mode == GuitarEchoMode::Slapback {
                    250.
                } else {
                    1500.
                },
                10.,
                cx,
            ));
        if s.mode != GuitarEchoMode::Slapback {
            panel = panel.child(knob(
                "echo-feedback",
                format!("Feedback: {:.0}%", s.feedback * 100.),
                s.feedback,
                |p| &mut p.echo.feedback,
                0.,
                0.85,
                0.05,
                cx,
            ));
        }
        panel.child(knob(
            "echo-mix",
            format!("Mix: {:.0}%", s.mix * 100.),
            s.mix,
            |p| &mut p.echo.mix,
            0.,
            0.7,
            0.05,
            cx,
        ))
    }

    pub(super) fn reverb_pedal(&self, cx: &mut Context<Self>) -> gpui::Div {
        let s = self.config.guitar.amp.pedals.reverb;
        pedal_panel("Reverb")
            .child(
                div().flex().flex_wrap().gap_1().children(
                    [
                        (ReverbMode::Off, "Off"),
                        (ReverbMode::Room, "Room"),
                        (ReverbMode::Hall, "Hall"),
                    ]
                    .into_iter()
                    .map(|(mode, label)| {
                        cell_btn(
                            format!("pedal-reverb-{mode:?}"),
                            label,
                            s.mode == mode,
                            self.accent(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.edit_amp(|a| {
                                let p = &mut a.pedals.reverb;
                                p.mode = mode;
                                match mode {
                                    ReverbMode::Room => {
                                        p.size = 0.4;
                                        p.damping = 0.55;
                                        p.mix = 0.15;
                                    }
                                    ReverbMode::Hall => {
                                        p.size = 0.7;
                                        p.damping = 0.5;
                                        p.mix = 0.2;
                                    }
                                    ReverbMode::Off => {}
                                }
                            });
                            cx.notify();
                        }))
                    }),
                ),
            )
            .child(knob(
                "reverb-size",
                format!("Size / decay: {:.0}", s.size * 100.),
                s.size,
                |p| &mut p.reverb.size,
                0.,
                1.,
                0.05,
                cx,
            ))
            .child(knob(
                "reverb-damping",
                format!("Damping: {:.0}", s.damping * 100.),
                s.damping,
                |p| &mut p.reverb.damping,
                0.,
                1.,
                0.05,
                cx,
            ))
            .child(knob(
                "reverb-mix",
                format!("Mix: {:.0}%", s.mix * 100.),
                s.mix,
                |p| &mut p.reverb.mix,
                0.,
                0.7,
                0.05,
                cx,
            ))
    }
}

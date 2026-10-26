use super::*;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpatialPreset {
    SmallRoom,
    WideHall,
    DarkSpace,
    TapeEcho,
    PingPong,
    Slapback,
}
impl SpatialPreset {
    pub const REVERB: [Self; 3] = [Self::SmallRoom, Self::WideHall, Self::DarkSpace];
    pub const ECHO: [Self; 3] = [Self::TapeEcho, Self::PingPong, Self::Slapback];
    pub fn label(self) -> &'static str {
        match self {
            Self::SmallRoom => "Small room",
            Self::WideHall => "Wide hall",
            Self::DarkSpace => "Dark space",
            Self::TapeEcho => "Tape echo",
            Self::PingPong => "Ping-pong",
            Self::Slapback => "Slapback",
        }
    }
}
impl Project {
    /// Shape the selected shared bus while preserving its fader and mute.
    pub fn apply_spatial_preset(&mut self, preset: SpatialPreset) {
        match preset {
            SpatialPreset::SmallRoom | SpatialPreset::WideHall | SpatialPreset::DarkSpace => {
                let old = &self.returns.reverb;
                let mut next = ReverbReturn {
                    gain: old.gain,
                    muted: old.muted,
                    ..Default::default()
                };
                match preset {
                    SpatialPreset::SmallRoom => {
                        next.room_size = 0.25;
                        next.damping = 0.65;
                        next.width = 0.65;
                        next.predelay_ms = 8;
                        next.low_cut_hz = 180.;
                        next.high_cut_hz = 7000.;
                    }
                    SpatialPreset::WideHall => {
                        next.room_size = 0.85;
                        next.damping = 0.45;
                        next.predelay_ms = 35;
                        next.low_cut_hz = 160.;
                        next.high_cut_hz = 12000.;
                    }
                    _ => {
                        next.room_size = 0.94;
                        next.damping = 0.85;
                        next.predelay_ms = 60;
                        next.low_cut_hz = 280.;
                        next.high_cut_hz = 3500.;
                    }
                }
                self.returns.reverb = next;
            }
            _ => {
                let old = &self.returns.delay;
                let mut next = DelayReturn {
                    gain: old.gain,
                    muted: old.muted,
                    ..Default::default()
                };
                match preset {
                    SpatialPreset::TapeEcho => {
                        next.sync = Some(EchoDivision::EighthDotted);
                        next.feedback = 0.46;
                        next.low_cut_hz = 180.;
                        next.high_cut_hz = 3500.;
                        next.width = 0.72;
                        next.modulation_ms = 2.5;
                        next.modulation_hz = 0.32;
                    }
                    SpatialPreset::PingPong => {
                        next.sync = Some(EchoDivision::Quarter);
                        next.feedback = 0.42;
                        next.low_cut_hz = 220.;
                        next.high_cut_hz = 8500.;
                    }
                    _ => {
                        next.mode = EchoMode::Stereo;
                        next.time_ms = 90;
                        next.feedback = 0.14;
                        next.low_cut_hz = 150.;
                        next.high_cut_hz = 6000.;
                        next.width = 0.8;
                    }
                }
                self.returns.delay = next;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_defaults_and_synced_echoes() {
        let r: Returns = serde_json::from_str(
            r#"{"reverb":{"room_size":0.8},"delay":{"time_ms":417,"feedback":0.27}}"#,
        )
        .unwrap();
        assert_eq!(r.delay.effective_time_ms(90.), 417);
        assert_eq!(r.reverb.predelay_ms, 0);
        assert_eq!(r.reverb.low_cut_hz, 0.);
        assert_eq!(r.delay.modulation_ms, 0.);
        assert_eq!(r.delay.mode, EchoMode::PingPong);
        let mut p = Project::default_project();
        p.returns = r;
        p.returns.delay.sync = Some(EchoDivision::EighthDotted);
        assert_eq!(p.returns.delay.effective_time_ms(90.), 500);
        assert_eq!(p.returns.delay.effective_time_ms(120.), 375);
        p.returns.delay.sync = Some(EchoDivision::Whole);
        assert_eq!(p.returns.delay.effective_time_ms(20.), 12000);
        p.returns.delay.sync = Some(EchoDivision::EighthTriplet);
        assert_eq!(p.returns.delay.effective_time_ms(120.), 167);
        p.validate().unwrap();
    }
    #[test]
    fn spatial_presets_preserve_mix_and_validate() {
        let mut p = Project::default_project();
        p.returns.reverb.gain = 0.37;
        p.returns.reverb.muted = true;
        p.returns.delay.gain = 0.28;
        p.returns.delay.muted = true;
        let patterns = serde_json::to_value(&p.patterns).unwrap();
        for preset in SpatialPreset::REVERB.into_iter().chain(SpatialPreset::ECHO) {
            p.apply_spatial_preset(preset);
            p.validate().unwrap();
            assert_eq!(
                (p.returns.reverb.gain, p.returns.reverb.muted),
                (0.37, true)
            );
            assert_eq!((p.returns.delay.gain, p.returns.delay.muted), (0.28, true));
            assert_eq!(serde_json::to_value(&p.patterns).unwrap(), patterns);
        }
        p.returns.delay.modulation_ms = 21.;
        assert!(p.validate().is_err());
        p.returns.delay.modulation_ms = 2.;
        p.returns.reverb.predelay_ms = 251;
        assert!(p.validate().is_err());
        p.returns.reverb.predelay_ms = 10;
        p.returns.delay.low_cut_hz = 1800.;
        p.returns.delay.high_cut_hz = 1000.;
        assert!(p.validate().is_err());
    }
}

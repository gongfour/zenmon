//! Wall-clock playback of the local index; never publishes recorded messages.
use crate::recording::Recording;
use std::time::{Duration, Instant, SystemTime};

const SPEEDS: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];
pub(crate) struct Playback {
    anchor: Option<(Instant, SystemTime)>,
    speed: usize,
}
impl Default for Playback {
    fn default() -> Self {
        Self {
            anchor: None,
            speed: 2,
        }
    }
}
impl Playback {
    pub fn pause(&mut self) {
        self.anchor = None;
    }
    pub fn toggle(&mut self, recording: &Recording, position: usize, now: Instant) -> usize {
        if self.anchor.is_some() {
            self.pause();
            return position;
        }
        let position = if position + 1 == recording.entries.len() {
            0
        } else {
            position
        };
        self.anchor = Some((now, recording.entries[position].time));
        position
    }
    pub fn speed(&mut self, faster: bool, now: Instant) {
        if let Some((started, time)) = self.anchor {
            self.anchor = Some((
                now,
                advance(
                    time,
                    now.saturating_duration_since(started),
                    SPEEDS[self.speed],
                ),
            ));
        }
        self.speed = if faster {
            (self.speed + 1).min(SPEEDS.len() - 1)
        } else {
            self.speed.saturating_sub(1)
        };
    }
    pub fn target(&mut self, recording: &Recording, position: usize, now: Instant) -> usize {
        let Some((started, time)) = self.anchor else {
            return position;
        };
        let until = advance(
            time,
            now.saturating_duration_since(started),
            SPEEDS[self.speed],
        );
        let target = recording
            .entries
            .partition_point(|e| e.time <= until)
            .saturating_sub(1)
            .max(position);
        if target + 1 == recording.entries.len() {
            self.pause();
        }
        target
    }
    pub fn label(&self) -> String {
        format!(
            "{} {}x",
            if self.anchor.is_some() {
                "PLAY"
            } else {
                "PAUSED"
            },
            SPEEDS[self.speed]
        )
    }
}
fn advance(time: SystemTime, elapsed: Duration, speed: f64) -> SystemTime {
    time.checked_add(elapsed.mul_f64(speed)).unwrap_or(time)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn recording() -> Recording {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for ms in [0, 1000, 1000, 4000] {
            writeln!(file, "{}", serde_json::json!({"schema_version":1,"key_expr":"test/key","payload_base64":"MA==","encoding":"application/json","received_offset_ms":ms,"kind":"PUT"})).unwrap();
        }
        Recording::load(file.path()).unwrap()
    }
    #[test]
    fn playback_respects_time_gaps_equal_times_pause_speed_and_end() {
        let recording = recording();
        let now = Instant::now();
        let mut p = Playback::default();
        assert_eq!(p.toggle(&recording, 3, now), 0);
        assert_eq!(p.target(&recording, 0, now + Duration::from_millis(900)), 0);
        assert_eq!(p.target(&recording, 0, now + Duration::from_secs(1)), 2);
        p.speed(true, now + Duration::from_secs(1));
        assert_eq!(p.target(&recording, 2, now + Duration::from_secs(2)), 2);
        assert_eq!(
            p.target(&recording, 2, now + Duration::from_millis(2500)),
            3
        );
        assert_eq!(p.label(), "PAUSED 2x");
        p.toggle(&recording, 0, now);
        p.pause();
        assert_eq!(p.target(&recording, 0, now + Duration::from_secs(99)), 0);
        assert_eq!(p.toggle(&recording, 2, now), 2);
        assert_eq!(p.toggle(&recording, 2, now), 2);
        assert_eq!(p.label(), "PAUSED 2x");
    }
}

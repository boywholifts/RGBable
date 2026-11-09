//! Wireless fan speeds: follow the motherboard, a fixed speed, or a curve on the
//! HydroShift's coolant temperature.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// About 20%: the lowest speed ever sent, so no setting or bad reading can stall a fan.
pub const MIN_DUTY: u8 = 51;
/// A curve without a fresh coolant reading for this long runs the fans at full speed.
pub const STALE_READING: Duration = Duration::from_secs(5);
/// How long after starting a curve waits for the first coolant reading before it falls back
/// to full speed: readings arrive within a few seconds, and fans jumping to 100% on every
/// start is loud for nothing.
pub const FIRST_SAMPLE_WAIT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum FanMode {
    /// The board's fan signal, measured by the dongle on its fan cable.
    #[default]
    Motherboard,
    Fixed {
        percent: u8,
    },
    /// Coolant °C to fan %, joined by straight lines.
    Curve {
        points: Vec<[u8; 2]>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum PumpMode {
    /// RGBable doesn't touch the pump, so it keeps whatever speed it was last given.
    #[default]
    Unchanged,
    Motherboard,
    Fixed {
        rpm: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FanProfile {
    pub name: String,
    /// Fan devices without their own entry in `fans`.
    #[serde(default)]
    pub all_fans: FanMode,
    #[serde(default)]
    pub fans: BTreeMap<String, FanMode>,
    #[serde(default)]
    pub pump: PumpMode,
}

impl FanProfile {
    pub fn fan_mode(&self, id: &str) -> FanMode {
        self.fans.get(id).unwrap_or(&self.all_fans).clone()
    }
}

/// The profiles a new install starts with; Standard, the active one, leaves fans on the
/// motherboard and the pump alone, as before profiles existed.
pub fn starter_profiles() -> Vec<FanProfile> {
    let profile = |name: &str, all_fans, pump| FanProfile {
        name: name.into(),
        all_fans,
        fans: BTreeMap::new(),
        pump,
    };
    vec![
        profile(
            "Quiet",
            FanMode::Curve {
                points: vec![[30, 25], [35, 35], [40, 60], [45, 100]],
            },
            PumpMode::Fixed { rpm: 2000 },
        ),
        profile("Standard", FanMode::Motherboard, PumpMode::Unchanged),
        profile(
            "Performance",
            FanMode::Curve {
                points: vec![[26, 50], [32, 75], [36, 100]],
            },
            PumpMode::Fixed { rpm: 2735 },
        ),
    ]
}

pub const STARTER_ACTIVE: usize = 1;

pub fn clean_fan_mode(mode: FanMode) -> FanMode {
    match mode {
        FanMode::Fixed { percent } => FanMode::Fixed {
            percent: percent.min(100),
        },
        FanMode::Curve { mut points } => {
            points.truncate(8);
            for point in &mut points {
                point[0] = point[0].min(100);
                point[1] = point[1].min(100);
            }
            points.sort_by_key(|p| p[0]);
            if points.is_empty() {
                default_curve()
            } else {
                FanMode::Curve { points }
            }
        }
        FanMode::Motherboard => FanMode::Motherboard,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Duty(u8),
    /// No signal to follow: leave the fan as it is, unless it has stopped.
    KeepUnlessStopped,
}

fn percent_to_duty(percent: u8) -> u8 {
    ((u16::from(percent.min(100)) * 255 + 50) / 100) as u8
}

pub fn duty_percent(duty: u8) -> u8 {
    ((u16::from(duty) * 100 + 127) / 255) as u8
}

/// `board` is the motherboard duty (0 meaning no signal), `coolant` the latest coolant
/// reading in °C and how old it is.
/// `since_start` is how long RGBable has been running; `coolant` is the latest reading and its
/// age, `None` when none has ever arrived.
pub fn decide(
    mode: &FanMode,
    board: Option<u8>,
    coolant: Option<(u8, Duration)>,
    since_start: Duration,
) -> Decision {
    let duty = match mode {
        FanMode::Motherboard => match board.filter(|&d| d > 0) {
            Some(duty) => duty,
            None => return Decision::KeepUnlessStopped,
        },
        FanMode::Fixed { percent } => percent_to_duty(*percent),
        FanMode::Curve { points } => match coolant.filter(|(_, age)| *age < STALE_READING) {
            Some((celsius, _)) => percent_to_duty(curve_percent(points, celsius)),
            None if coolant.is_none() && since_start < FIRST_SAMPLE_WAIT => {
                return Decision::KeepUnlessStopped
            }
            None => 255,
        },
    };
    Decision::Duty(duty.max(MIN_DUTY))
}

fn curve_percent(points: &[[u8; 2]], celsius: u8) -> u8 {
    let mut points = points.to_vec();
    points.sort_by_key(|p| p[0]);
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return 100;
    };
    if celsius <= first[0] {
        return first[1];
    }
    if celsius >= last[0] {
        return last[1];
    }
    for pair in points.windows(2) {
        let ([t0, p0], [t1, p1]) = (pair[0], pair[1]);
        if (t0..=t1).contains(&celsius) && t1 > t0 {
            let along = f32::from(celsius - t0) / f32::from(t1 - t0);
            return (f32::from(p0) + (f32::from(p1) - f32::from(p0)) * along).round() as u8;
        }
    }
    last[1]
}

pub fn default_curve() -> FanMode {
    FanMode::Curve {
        points: vec![[28, 30], [32, 45], [36, 70], [40, 100]],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNNING: Duration = Duration::from_secs(60);

    const FRESH: Duration = Duration::from_secs(1);

    #[test]
    fn curve_follows_the_coolant_between_points() {
        let curve = default_curve();
        assert_eq!(
            decide(&curve, None, Some((20, FRESH)), RUNNING),
            Decision::Duty(percent_to_duty(30))
        );
        assert_eq!(
            decide(&curve, None, Some((34, FRESH)), RUNNING),
            Decision::Duty(percent_to_duty(58))
        );
        assert_eq!(
            decide(&curve, None, Some((50, FRESH)), RUNNING),
            Decision::Duty(255)
        );
    }

    #[test]
    fn a_curve_waits_briefly_for_its_first_reading() {
        let curve = default_curve();
        let starting = Duration::from_secs(3);
        assert_eq!(
            decide(&curve, None, None, starting),
            Decision::KeepUnlessStopped
        );
        // a reading that has gone stale is a failure, not a start: full speed straight away
        assert_eq!(
            decide(&curve, None, Some((25, Duration::from_secs(9))), starting),
            Decision::Duty(255)
        );
    }

    #[test]
    fn a_curve_without_a_fresh_reading_runs_at_full_speed() {
        let curve = default_curve();
        assert_eq!(decide(&curve, None, None, RUNNING), Decision::Duty(255));
        assert_eq!(
            decide(&curve, None, Some((25, Duration::from_secs(9))), RUNNING),
            Decision::Duty(255)
        );
    }

    #[test]
    fn nothing_can_stop_a_fan() {
        assert_eq!(
            decide(&FanMode::Fixed { percent: 0 }, None, None, RUNNING),
            Decision::Duty(MIN_DUTY)
        );
        assert_eq!(
            decide(&FanMode::Motherboard, Some(10), None, RUNNING),
            Decision::Duty(MIN_DUTY)
        );
        let flat = FanMode::Curve {
            points: vec![[30, 0]],
        };
        assert_eq!(
            decide(&flat, None, Some((30, FRESH)), RUNNING),
            Decision::Duty(MIN_DUTY)
        );
    }

    #[test]
    fn no_board_signal_leaves_running_fans_alone() {
        assert_eq!(
            decide(&FanMode::Motherboard, Some(0), None, RUNNING),
            Decision::KeepUnlessStopped
        );
        assert_eq!(
            decide(&FanMode::Motherboard, None, None, RUNNING),
            Decision::KeepUnlessStopped
        );
        assert_eq!(
            decide(&FanMode::Motherboard, Some(200), None, RUNNING),
            Decision::Duty(200)
        );
    }
}

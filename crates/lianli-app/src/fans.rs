//! Wireless fan speeds: follow the motherboard, a fixed speed, or a curve on the
//! HydroShift's coolant temperature.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// About 20%: the lowest speed ever sent, so no setting or bad reading can stall a fan.
pub const MIN_DUTY: u8 = 51;
/// A curve without a fresh coolant reading for this long runs the fans at full speed.
pub const STALE_READING: Duration = Duration::from_secs(5);

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
pub fn decide(mode: &FanMode, board: Option<u8>, coolant: Option<(u8, Duration)>) -> Decision {
    let duty = match mode {
        FanMode::Motherboard => match board.filter(|&d| d > 0) {
            Some(duty) => duty,
            None => return Decision::KeepUnlessStopped,
        },
        FanMode::Fixed { percent } => percent_to_duty(*percent),
        FanMode::Curve { points } => match coolant.filter(|(_, age)| *age < STALE_READING) {
            Some((celsius, _)) => percent_to_duty(curve_percent(points, celsius)),
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

    const FRESH: Duration = Duration::from_secs(1);

    #[test]
    fn curve_follows_the_coolant_between_points() {
        let curve = default_curve();
        assert_eq!(
            decide(&curve, None, Some((20, FRESH))),
            Decision::Duty(percent_to_duty(30))
        );
        assert_eq!(
            decide(&curve, None, Some((34, FRESH))),
            Decision::Duty(percent_to_duty(58))
        );
        assert_eq!(decide(&curve, None, Some((50, FRESH))), Decision::Duty(255));
    }

    #[test]
    fn a_curve_without_a_fresh_reading_runs_at_full_speed() {
        let curve = default_curve();
        assert_eq!(decide(&curve, None, None), Decision::Duty(255));
        assert_eq!(
            decide(&curve, None, Some((25, Duration::from_secs(9)))),
            Decision::Duty(255)
        );
    }

    #[test]
    fn nothing_can_stop_a_fan() {
        assert_eq!(
            decide(&FanMode::Fixed { percent: 0 }, None, None),
            Decision::Duty(MIN_DUTY)
        );
        assert_eq!(
            decide(&FanMode::Motherboard, Some(10), None),
            Decision::Duty(MIN_DUTY)
        );
        let flat = FanMode::Curve {
            points: vec![[30, 0]],
        };
        assert_eq!(
            decide(&flat, None, Some((30, FRESH))),
            Decision::Duty(MIN_DUTY)
        );
    }

    #[test]
    fn no_board_signal_leaves_running_fans_alone() {
        assert_eq!(
            decide(&FanMode::Motherboard, Some(0), None),
            Decision::KeepUnlessStopped
        );
        assert_eq!(
            decide(&FanMode::Motherboard, None, None),
            Decision::KeepUnlessStopped
        );
        assert_eq!(
            decide(&FanMode::Motherboard, Some(200), None),
            Decision::Duty(200)
        );
    }
}

//! Pure, bounded fan policy. A missing/stale temperature yields hardware
//! automatic mode, never a guessed temperature or a retained manual duty.
use serde::{Deserialize, Serialize};
use wtm_core::hardware::{FanChannel, HardwareReport, SensorKind};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub temperature: f32,
    pub percent: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CoolingMode {
    Automatic,
    Manual(f32),
    Curve(Vec<CurvePoint>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoolingProfile {
    pub mode: CoolingMode,
    pub sensor_id: String,
    pub critical_celsius: f32,
}

impl Default for CoolingProfile {
    fn default() -> Self {
        Self {
            mode: CoolingMode::Automatic,
            sensor_id: String::new(),
            critical_celsius: 90.0,
        }
    }
}

impl CoolingProfile {
    pub fn preset(name: &str) -> Vec<CurvePoint> {
        let values = match name {
            "Quiet" => [(35.0, 30.0), (55.0, 40.0), (70.0, 60.0), (85.0, 100.0)],
            "Performance" => [(35.0, 45.0), (55.0, 65.0), (70.0, 85.0), (85.0, 100.0)],
            _ => [(35.0, 30.0), (55.0, 50.0), (70.0, 75.0), (85.0, 100.0)],
        };
        values
            .into_iter()
            .map(|(temperature, percent)| CurvePoint {
                temperature,
                percent,
            })
            .collect()
    }

    pub fn validate(&self, channel: &FanChannel) -> Result<(), String> {
        if self.mode == CoolingMode::Automatic {
            return Ok(());
        }
        if !channel.writable {
            return Err(channel.reason.clone());
        }
        if !channel.min_percent.is_finite()
            || channel.min_percent < 30.0
            || channel.max_percent != 100.0
        {
            return Err("Channel must offer valid bounds and 100% emergency duty".into());
        }
        if self.sensor_id.is_empty() {
            return Err("Choose a temperature sensor for thermal protection".into());
        }
        if !self.critical_celsius.is_finite() || !(60.0..=95.0).contains(&self.critical_celsius) {
            return Err("Critical temperature must be between 60 and 95 °C".into());
        }
        let valid = |v: f32| {
            v.is_finite()
                && v >= channel.min_percent.max(30.0)
                && v <= channel.max_percent.min(100.0)
        };
        match &self.mode {
            CoolingMode::Automatic => Ok(()),
            CoolingMode::Manual(v) if valid(*v) => Ok(()),
            CoolingMode::Manual(_) => Err("Duty is outside this channel's bounds".into()),
            CoolingMode::Curve(points) => {
                if !(2..=12).contains(&points.len()) {
                    return Err("A curve needs 2 to 12 points".into());
                }
                for (i, p) in points.iter().enumerate() {
                    if !p.temperature.is_finite()
                        || !(0.0..=100.0).contains(&p.temperature)
                        || !valid(p.percent)
                    {
                        return Err("Curve points must have a valid temperature and duty".into());
                    }
                    if i > 0
                        && (points[i - 1].temperature >= p.temperature
                            || points[i - 1].percent > p.percent)
                    {
                        return Err(
                            "Temperatures must increase and fan duty must not decrease".into()
                        );
                    }
                }
                Ok(())
            }
        }
    }

    pub fn duty(&self, report: &HardwareReport, now: u64) -> Option<f32> {
        if self.mode == CoolingMode::Automatic || !report.fresh(now) {
            return None;
        }
        let t = report
            .sensors
            .iter()
            .find(|s| s.id == self.sensor_id && s.kind == SensorKind::Temperature)?
            .value?;
        if !t.is_finite() || !(0.0..=150.0).contains(&t) {
            return None;
        }
        if t >= self.critical_celsius {
            return Some(100.0);
        }
        match &self.mode {
            CoolingMode::Automatic => None,
            CoolingMode::Manual(p) => Some(*p),
            CoolingMode::Curve(points) => {
                let first = points.first()?;
                if t <= first.temperature {
                    return Some(first.percent);
                }
                for pair in points.windows(2) {
                    let (a, b) = (&pair[0], &pair[1]);
                    if t <= b.temperature {
                        return Some(
                            a.percent
                                + (b.percent - a.percent) * (t - a.temperature)
                                    / (b.temperature - a.temperature),
                        );
                    }
                }
                points.last().map(|p| p.percent)
            }
        }
    }
}

/// Upward changes are immediate. Decreases use a 3% deadband and at most five
/// percentage points per second; emergency/full speed bypasses smoothing.
pub fn smooth_duty(previous: f32, requested: f32, elapsed_secs: f32) -> f32 {
    if requested >= previous {
        return requested;
    }
    if previous - requested < 3.0 {
        return previous;
    }
    requested.max(previous - 5.0 * elapsed_secs.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wtm_core::hardware::Sensor;
    fn channel() -> FanChannel {
        FanChannel {
            id: "fan".into(),
            device: "GPU".into(),
            name: "Fan".into(),
            min_percent: 30.0,
            max_percent: 100.0,
            writable: true,
            reason: String::new(),
        }
    }
    fn report(t: Option<f32>) -> HardwareReport {
        HardwareReport {
            sampled_at_ms: 1000,
            sensors: vec![Sensor {
                id: "temp".into(),
                device: "GPU".into(),
                name: "Core".into(),
                kind: SensorKind::Temperature,
                value: t,
            }],
            ..Default::default()
        }
    }
    #[test]
    fn manual_requires_fresh_real_temperature_and_emergency_overrides_it() {
        let p = CoolingProfile {
            mode: CoolingMode::Manual(40.0),
            sensor_id: "temp".into(),
            ..Default::default()
        };
        assert!(p.validate(&channel()).is_ok());
        for (min, max) in [(30.0, 80.0), (f32::NAN, 100.0), (101.0, 100.0)] {
            let mut unavailable = channel();
            unavailable.min_percent = min;
            unavailable.max_percent = max;
            assert!(p.validate(&unavailable).is_err());
        }
        assert_eq!(p.duty(&report(Some(50.0)), 1000), Some(40.0));
        assert_eq!(p.duty(&report(Some(90.0)), 1000), Some(100.0));
        for value in [None, Some(f32::NAN), Some(-1.0)] {
            assert_eq!(p.duty(&report(value), 1000), None);
        }
        assert_eq!(p.duty(&report(Some(50.0)), 11_001), None);
        assert_eq!(smooth_duty(80.0, 40.0, 2.0), 70.0);
        assert_eq!(smooth_duty(40.0, 100.0, 0.1), 100.0);
        assert_eq!(smooth_duty(50.0, 48.0, 2.0), 50.0);
    }
    #[test]
    fn curves_interpolate_and_reject_duplicate_reversed_and_nonfinite_points() {
        let mut p = CoolingProfile {
            mode: CoolingMode::Curve(CoolingProfile::preset("Balanced")),
            sensor_id: "temp".into(),
            ..Default::default()
        };
        assert!(p.validate(&channel()).is_ok());
        assert_eq!(p.duty(&report(Some(45.0)), 1000), Some(40.0));
        for points in [
            vec![],
            vec![
                CurvePoint {
                    temperature: 50.0,
                    percent: 40.0
                };
                2
            ],
            vec![
                CurvePoint {
                    temperature: 30.0,
                    percent: 50.0,
                },
                CurvePoint {
                    temperature: 60.0,
                    percent: 40.0,
                },
            ],
            vec![
                CurvePoint {
                    temperature: f32::NAN,
                    percent: 40.0,
                },
                CurvePoint {
                    temperature: 60.0,
                    percent: 60.0,
                },
            ],
        ] {
            p.mode = CoolingMode::Curve(points);
            assert!(p.validate(&channel()).is_err());
        }
    }
}

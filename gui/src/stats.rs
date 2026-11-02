//! Current statistics, the same as the statistics mode of the CLI example

use crate::recording::SAMPLE_RATE;

/// Current statistics over a number of samples
#[derive(Default, Clone)]
pub struct Stats {
    pub count: u64,
    sum: f64,
    sum_sq: f64,
    min: f32,
    max: f32,
    /// Samples at or above the split current and their sum
    active_count: u64,
    active_sum: f64,
    /// Number of bursts: transitions from below to at or above the split current
    bursts: u64,
    in_burst: bool,
}

impl Stats {
    pub fn add(&mut self, micro_amps: f32, split_ua: Option<f32>) {
        if self.count == 0 {
            self.min = micro_amps;
            self.max = micro_amps;
        }
        let i = f64::from(micro_amps);
        self.count += 1;
        self.sum += i;
        self.sum_sq += i * i;
        self.min = self.min.min(micro_amps);
        self.max = self.max.max(micro_amps);

        if let Some(split) = split_ua {
            let active = micro_amps >= split;
            if active {
                self.active_count += 1;
                self.active_sum += i;
                if !self.in_burst {
                    self.bursts += 1;
                }
            }
            self.in_burst = active;
        }
    }

    /// Add `n` samples of which only their sum, minimum and maximum are known (µA). The rms and the split use
    /// their average.
    pub fn add_samples(&mut self, n: u64, sum: f64, min: f32, max: f32, split_ua: Option<f32>) {
        if n == 1 {
            return self.add(sum as f32, split_ua);
        }
        if self.count == 0 {
            self.min = min;
            self.max = max;
        }
        let avg = sum / n as f64;
        self.count += n;
        self.sum += sum;
        self.sum_sq += avg * avg * n as f64;
        self.min = self.min.min(min);
        self.max = self.max.max(max);

        if let Some(split) = split_ua {
            let active = avg >= f64::from(split);
            if active {
                self.active_count += n;
                self.active_sum += sum;
                if !self.in_burst {
                    self.bursts += 1;
                }
            }
            self.in_burst = active;
        }
    }

    /// Empty statistics that continue the burst state, so a burst spanning two intervals is counted once
    pub fn next(&self) -> Self {
        Self { in_burst: self.in_burst, ..Default::default() }
    }
}

/// Active/sleep split of a [Report]
#[derive(Clone)]
pub struct Split {
    pub threshold_ua: f32,
    pub active_pct: f64,
    pub active_avg_ua: f64,
    pub active_charge_pct: f64,
    pub bursts: u64,
    pub burst_ms: f64,
    pub sleep_pct: f64,
    pub sleep_avg_ua: f64,
}

/// The statistics of a number of samples, ready to display
#[derive(Clone)]
pub struct Report {
    pub samples: u64,
    pub duration_s: f64,
    pub avg_ua: f64,
    pub rms_ua: f64,
    pub min_ua: f32,
    pub max_ua: f32,
    pub charge_uc: f64,
    /// Source voltage and average power, in source meter mode only
    pub power: Option<(u16, f64)>,
    pub split: Option<Split>,
}

/// Average of `count` samples summing up to `sum`, 0 without samples
fn avg_of(sum: f64, count: f64) -> f64 {
    if count > 0. {
        sum / count
    } else {
        0.
    }
}

impl Report {
    pub fn new(stats: &Stats, duration_s: f64, source_mv: Option<u16>, split_ua: Option<f32>) -> Self {
        let n = stats.count as f64;
        let avg = avg_of(stats.sum, n);
        // The PPK2 regulates the source voltage, so the power is simply V * I_avg (µA * V = µW)
        let power = source_mv.map(|mv| (mv, avg * f64::from(mv) / 1000.));
        let split = split_ua.map(|threshold_ua| {
            let active = stats.active_count as f64;
            let sleep = n - active;
            Split {
                threshold_ua,
                active_pct: avg_of(active * 100., n),
                active_avg_ua: avg_of(stats.active_sum, active),
                active_charge_pct: if stats.sum > 0. { stats.active_sum * 100. / stats.sum } else { 0. },
                bursts: stats.bursts,
                burst_ms: if stats.bursts > 0 { active / stats.bursts as f64 * 1000. / SAMPLE_RATE } else { 0. },
                sleep_pct: avg_of(sleep * 100., n),
                sleep_avg_ua: avg_of(stats.sum - stats.active_sum, sleep),
            }
        });
        Self {
            samples: stats.count,
            duration_s,
            avg_ua: avg,
            rms_ua: avg_of(stats.sum_sq, n).sqrt(),
            min_ua: stats.min,
            max_ua: stats.max,
            charge_uc: stats.sum / SAMPLE_RATE, // µA * s = µC
            power,
            split,
        }
    }
}

/// Formats a current in µA with a fitting unit (nA, µA, mA or A)
pub fn format_current(micro_amps: f64) -> String {
    let a = micro_amps.abs();
    if a >= 1e6 {
        format!("{:.3} A", micro_amps / 1e6)
    } else if a >= 1e3 {
        format!("{:.3} mA", micro_amps / 1e3)
    } else if a >= 1. || a == 0. {
        format!("{micro_amps:.3} µA")
    } else {
        format!("{:.1} nA", micro_amps * 1e3)
    }
}

/// Parses a current like "1.5 mA", "500n" or "20" (µA) into µA
pub fn parse_current(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s.find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))).unwrap_or(s.len());
    let value: f64 = s[..split].trim().parse().ok()?;
    let factor = match s[split..].trim() {
        "" | "u" | "uA" | "µ" | "µA" => 1.,
        "n" | "nA" => 1e-3,
        "m" | "mA" => 1e3,
        "A" => 1e6,
        _ => return None,
    };
    Some(value * factor)
}

/// Formats a current in µA for an axis with grid lines `step_ua` apart: the unit and decimals fit the step
pub fn format_axis_current(micro_amps: f64, step_ua: f64) -> String {
    if micro_amps.abs() < step_ua * 1e-6 {
        return "0".into();
    }
    let step = step_ua.abs();
    let (factor, unit) = if step >= 1e5 {
        (1e-6, "A")
    } else if step >= 100. {
        (1e-3, "mA")
    } else if step >= 0.1 {
        (1., "µA")
    } else {
        (1e3, "nA")
    };
    let decimals = (-(step * factor).log10().floor()).max(0.) as usize;
    format!("{:.decimals$} {unit}", micro_amps * factor)
}

/// Formats a power in µW with a fitting unit (µW, mW or W)
pub fn format_power(micro_watts: f64) -> String {
    if micro_watts >= 1e6 {
        format!("{:.3} W", micro_watts / 1e6)
    } else if micro_watts >= 1e3 {
        format!("{:.3} mW", micro_watts / 1e3)
    } else {
        format!("{micro_watts:.3} µW")
    }
}

/// Formats a charge in µC with a fitting unit (µC, mC or C), plus the equivalent in mAh
pub fn format_charge(micro_coulombs: f64) -> String {
    format!("{} ({:.6} mAh)", format_charge_short(micro_coulombs), micro_coulombs / 3.6e6)
}

/// Formats a charge in µC with a fitting unit (µC, mC or C)
pub fn format_charge_short(micro_coulombs: f64) -> String {
    if micro_coulombs.abs() >= 1e6 {
        format!("{:.3} C", micro_coulombs / 1e6)
    } else if micro_coulombs.abs() >= 1e3 {
        format!("{:.3} mC", micro_coulombs / 1e3)
    } else {
        format!("{micro_coulombs:.3} µC")
    }
}

#[cfg(test)]
mod tests {
    use super::parse_current;

    #[test]
    fn parse_currents() {
        assert_eq!(parse_current("20"), Some(20.));
        assert_eq!(parse_current("1.5 mA"), Some(1500.));
        assert_eq!(parse_current("500n"), Some(0.5));
        assert_eq!(parse_current("3 µA"), Some(3.));
        assert_eq!(parse_current("2uA"), Some(2.));
        assert_eq!(parse_current("1 A"), Some(1e6));
        assert_eq!(parse_current("-0.25 mA"), Some(-250.));
        assert_eq!(parse_current("x"), None);
    }

    #[test]
    fn axis_currents() {
        use super::format_axis_current;
        assert_eq!(format_axis_current(9000., 1000.), "9 mA");
        assert_eq!(format_axis_current(8500., 500.), "8.5 mA");
        assert_eq!(format_axis_current(0., 1000.), "0");
        assert_eq!(format_axis_current(50., 10.), "50 µA");
        assert_eq!(format_axis_current(2.5, 0.5), "2.5 µA");
        assert_eq!(format_axis_current(0.05, 0.01), "50 nA");
        assert_eq!(format_axis_current(1e6, 1e5), "1.0 A");
    }
}

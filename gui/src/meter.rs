//! Analog meter: a needle on a logarithmic current scale

use std::f32::consts::PI;

use eframe::egui::{self, pos2, vec2, Align2, Pos2, Sense, Stroke};

use crate::theme;

/// Lowest and highest decade (log10 µA) of the scale: 100 nA to 1 A
const MIN_DECADE: f64 = -1.;
const MAX_DECADE: f64 = 6.;
const LABELS: [&str; 8] = ["100n", "1µ", "10µ", "100µ", "1m", "10m", "100m", "1A"];
/// Angles of the start and end of the scale (radians, counterclockwise from the right)
const START: f32 = 210. * PI / 180.;
const SWEEP: f32 = 240. * PI / 180.;

/// Needle with damping, so it can be followed by eye
#[derive(Default)]
pub struct Meter {
    /// Needle position (log10 µA)
    needle: Option<f64>,
}

impl Meter {
    /// Move the needle towards `micro_amps`, with time constant `tau_s`, after `dt_s` seconds
    pub fn update(&mut self, micro_amps: f64, dt_s: f64, tau_s: f64) {
        let target = to_scale(micro_amps);
        let needle = self.needle.get_or_insert(target);
        *needle += (target - *needle) * (1. - (-dt_s / tau_s).exp());
    }

    pub fn reset(&mut self) {
        self.needle = None;
    }

    /// Draw the meter at most `max_width` wide, with a band from `range.0` to `range.1` (µA), e.g. the recent
    /// minimum and maximum
    pub fn ui(&self, ui: &mut egui::Ui, range: Option<(f64, f64)>, max_width: f32) {
        // Fit the available height as well: the meter is 1.5 radius + 24 high
        let fit_height = 2. * ((ui.available_height() - 24.) / 1.5 + 12.);
        let width = ui.available_width().min(max_width).min(fit_height).max(120.);
        let r = width / 2. - 12.;
        let (rect, _) = ui.allocate_exact_size(vec2(width, r * 1.5 + 24.), Sense::hover());
        let painter = ui.painter_at(rect);
        let (fg, weak) = (theme::p().text, theme::p().label);
        let c = pos2(rect.center().x, rect.top() + r + 10.);
        let at = |frac: f32, radius: f32| -> Pos2 {
            let a = START - frac * SWEEP;
            c + vec2(a.cos(), -a.sin()) * radius
        };
        let arc = |from: f32, to: f32, radius: f32| -> Vec<Pos2> {
            let n = ((to - from).abs() * 80.).ceil().max(2.) as usize;
            (0..=n).map(|i| at(from + (to - from) * i as f32 / n as f32, radius)).collect()
        };

        if let Some((lo, hi)) = range {
            let (lo, hi) = (frac(to_scale(lo)), frac(to_scale(hi)));
            let band = Stroke::new(8., theme::p().accent.gamma_multiply(0.45));
            painter.add(egui::Shape::line(arc(lo, hi.max(lo + 0.003), r - 4.), band));
        }
        painter.add(egui::Shape::line(arc(0., 1., r), Stroke::new(1.5, fg)));

        // The labels grow with the meter
        let label_size = (r * 0.09).clamp(15., 26.);
        let decades = (MAX_DECADE - MIN_DECADE) as usize;
        for (d, label) in LABELS.iter().enumerate() {
            let f = d as f32 / decades as f32;
            painter.line_segment([at(f, r), at(f, r - 12.)], Stroke::new(1.5, fg));
            painter.text(at(f, r - 14. - label_size), Align2::CENTER_CENTER, label, theme::bold(label_size), weak);
            if d < decades {
                for m in 2..10 {
                    let f = (d as f64 + (m as f64).log10()) as f32 / decades as f32;
                    painter.line_segment([at(f, r), at(f, r - 6.)], Stroke::new(1., weak));
                }
            }
        }

        if let Some(needle) = self.needle {
            let f = frac(needle);
            painter.line_segment([c, at(f, r - 3.)], Stroke::new(2.5, theme::p().warn));
        }
        painter.circle_filled(c, 5., fg);
    }
}

/// Position on the scale (log10 µA) of a current, clamped to the scale
fn to_scale(micro_amps: f64) -> f64 {
    micro_amps.max(10f64.powf(MIN_DECADE)).log10().clamp(MIN_DECADE, MAX_DECADE)
}

/// Fraction (0..1) of the scale of a position (log10 µA)
fn frac(scale: f64) -> f32 {
    ((scale - MIN_DECADE) / (MAX_DECADE - MIN_DECADE)) as f32
}

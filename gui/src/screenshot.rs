//! Development aid: with `PPK2_SCREENSHOT=<file.png>` the app shows synthetic samples, saves a screenshot of
//! itself to that file and exits. For checking the looks without a PPK2. `PPK2_THEME=<name>` picks the color theme,
//! `PPK2_COMPACT=1` shows the compact view.

use eframe::egui;

use crate::recording::{Recording, SAMPLE_RATE};

/// Frames to draw before taking the screenshot, so the layout has settled
const FRAMES: u32 = 10;

pub struct Screenshot {
    path: String,
    frames: u32,
}

impl Screenshot {
    pub fn from_env() -> Option<Self> {
        std::env::var("PPK2_SCREENSHOT").ok().map(|path| Self { path, frames: 0 })
    }

    /// Call once per frame; takes the screenshot after [FRAMES] frames
    pub fn frame(&mut self, ctx: &egui::Context) {
        self.frames += 1;
        ctx.request_repaint();
        if self.frames == FRAMES {
            let path = self.path.clone();
            ctx.request_screenshot(move |image| {
                let [w, h] = image.size;
                let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                match image::save_buffer(&path, &rgba, w as u32, h as u32, image::ColorType::Rgba8) {
                    Ok(()) => println!("Saved {path}"),
                    Err(e) => eprintln!("Saving {path} failed: {e}"),
                }
                std::process::exit(0);
            });
        }
    }
}

/// Fill the recording with 12 s of a sensor-like signal: a few µA asleep, waking up every second for a short
/// radio burst (D0 high while awake, D1 toggling while the radio is on)
pub fn demo_samples(rec: &mut Recording) {
    rec.clear();
    rec.source_mv = Some(3000);
    let mut noise = 0x1234_5678u32;
    for i in 0..(12. * SAMPLE_RATE) as u64 {
        // xorshift: cheap, deterministic noise
        noise ^= noise << 13;
        noise ^= noise >> 17;
        noise ^= noise << 5;
        let jitter = (noise as f32 / u32::MAX as f32) - 0.5;
        let t = i as f64 / SAMPLE_RATE % 1.;
        let (ua, pins) = if (0.5..0.503).contains(&t) {
            (6000. + 800. * jitter, 1 | (((i / 50) % 2) as u8) << 1)
        } else if (0.503..0.545).contains(&t) {
            (650. + 120. * jitter, 1)
        } else {
            (4.2 + 0.6 * jitter, 0)
        };
        rec.push(ua, pins);
    }
}

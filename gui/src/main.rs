//! Native GUI for Nordic's Power Profiler Kit 2

mod app;
mod device;
mod meter;
mod recording;
mod screenshot;
mod stats;
mod theme;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("PPK2")
            .with_inner_size([1280., 800.])
            .with_min_inner_size([800., 500.]),
        ..Default::default()
    };
    eframe::run_native("PPK2", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}

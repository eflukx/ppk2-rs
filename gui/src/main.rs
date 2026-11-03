//! Native and web GUI for Nordic's Power Profiler Kit 2

// No console window on Windows for release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod device;
mod meter;
mod recording;
mod screenshot;
mod stats;
mod theme;

#[cfg(not(target_arch = "wasm32"))]
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

/// In the browser: run in the canvas of index.html
#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::JsCast;

    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window().and_then(|w| w.document()).expect("No document");
        let canvas = document
            .get_element_by_id("ppk2")
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("No canvas with id ppk2");
        let result = eframe::WebRunner::new()
            .start(canvas, eframe::WebOptions::default(), Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
            .await;
        if let Err(e) = result {
            web_sys::console::error_2(&"Failed to start:".into(), &e);
        }
    });
}

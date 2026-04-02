#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod protocol;
mod serial;

use app::TokioEguiApp;
use eframe::egui;

fn main() -> eframe::Result {
    let window_title = format!("PCS Electronics STMAX Control - {}", env!("BUILD_VERSION"));
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([960.0, 720.0]),
        #[cfg(target_os = "windows")]
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        &window_title,
        native_options,
        Box::new(|cc| {
            let app = TokioEguiApp::new(cc)?;
            Ok(Box::new(app))
        }),
    )
}

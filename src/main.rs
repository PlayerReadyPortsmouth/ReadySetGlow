// Hide the console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod audio;
mod color;
mod config;
mod devices;
mod effects;
mod engine;
mod govee;
mod noise;
mod palette;
mod presets;
mod sku_table;
mod ui_widgets;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("ReadySetGlow")
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ReadySetGlow",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}

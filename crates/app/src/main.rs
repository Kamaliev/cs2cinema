#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod launch;
mod settings;
mod update;

fn main() -> eframe::Result {
    let icon = app::icon_rgba(64);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("CS2 Cinema")
            .with_inner_size([980.0, 720.0])
            .with_min_inner_size([720.0, 480.0])
            .with_drag_and_drop(true)
            .with_icon(eframe::egui::IconData { rgba: icon, width: 64, height: 64 }),
        ..Default::default()
    };
    eframe::run_native("CS2 Cinema", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}

#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod launch;
mod matches;
#[cfg(debug_assertions)]
mod mock;
mod montage;
mod settings;
mod settings_page;
mod status;
mod ui;
mod update;

fn main() -> eframe::Result {
    let icon = ui::icon_rgba(64);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("CS2 Cinema")
            .with_inner_size([1100.0, 760.0])
            .with_min_inner_size([820.0, 560.0])
            .with_drag_and_drop(true)
            .with_icon(eframe::egui::IconData { rgba: icon, width: 64, height: 64 }),
        ..Default::default()
    };
    eframe::run_native("CS2 Cinema", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}

#[cfg(windows)]
fn tray(ctx: &eframe::egui::Context) -> Option<tray_icon::TrayIcon> {
    use eframe::egui::{self, ViewportCommand};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem},
    };
    let show = MenuItem::with_id("show", "Открыть", true, None);
    let quit = MenuItem::with_id("quit", "Выход", true, None);
    let menu = Menu::new();
    menu.append_items(&[&show, &quit]).ok()?;
    let icon = Icon::from_rgba(ui::icon_rgba(32), 32, 32).ok()?;
    let tray = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("CS2 Cinema").with_icon(icon).build().ok()?;
    let reveal = |ctx: &egui::Context| {
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        ctx.request_repaint();
    };
    let c = ctx.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| match e.id.0.as_str() {
        "show" => reveal(&c),
        "quit" => std::process::exit(0),
        _ => {}
    }));
    let c = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            reveal(&c);
        }
    }));
    Some(tray)
}

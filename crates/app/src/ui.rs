//! Общие цвета, иконка и мелкие виджеты.

use eframe::egui::{self, Color32, RichText, Ui};

pub const ACCENT: Color32 = Color32::from_rgb(255, 140, 26);
pub const OK: Color32 = Color32::from_rgb(110, 200, 120);
pub const BAD: Color32 = Color32::from_rgb(230, 90, 90);
pub const MUTED: Color32 = Color32::from_gray(140);

pub fn apply_theme(ctx: &egui::Context) {
    let mut st = (*ctx.style()).clone();
    st.spacing.item_spacing = egui::vec2(8.0, 6.0);
    st.spacing.button_padding = egui::vec2(10.0, 5.0);
    st.visuals.selection.bg_fill = ACCENT.linear_multiply(0.35);
    st.visuals.hyperlink_color = ACCENT;
    st.visuals.widgets.hovered.bg_stroke.color = ACCENT;
    ctx.set_style(st);
}

/// Иконка приложения: тёмный квадрат, оранжевый круг и «play».
pub fn icon_rgba(n: u32) -> Vec<u8> {
    let mut px = vec![0u8; (n * n * 4) as usize];
    let c = n as f32 / 2.0;
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
            let r = (fx * fx + fy * fy).sqrt();
            let i = ((y * n + x) * 4) as usize;
            let mut rgba = [24u8, 26, 32, 255];
            if r < c * 0.82 {
                rgba = [255, 140, 26, 255];
            }
            let (tx, ty) = (fx / c, fy / c);
            if tx > -0.25 && tx < 0.4 && ty.abs() < (0.4 - tx) * 0.6 {
                rgba = [255, 255, 255, 255];
            }
            px[i..i + 4].copy_from_slice(&rgba);
        }
    }
    px
}

pub fn section(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong().size(15.0));
    egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui);
    });
}

pub fn primary(ui: &mut Ui, enabled: bool, text: &str) -> bool {
    ui.add_enabled(enabled, egui::Button::new(RichText::new(text).color(Color32::WHITE).strong()).fill(ACCENT.linear_multiply(0.85))).clicked()
}

pub fn date(ts: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(ts, 0).single().map(|d| d.format("%d.%m.%Y %H:%M").to_string()).unwrap_or_default()
}

pub fn duration(secs: i64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

pub fn open_path(p: &std::path::Path) {
    #[cfg(windows)]
    {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]).arg(p);
        let _ = crate::launch::no_window(&mut c).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(p).spawn();
    }
}

pub fn open_url(url: &str) {
    open_path(std::path::Path::new(url));
}

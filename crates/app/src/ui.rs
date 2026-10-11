//! Тема, шрифты, цвета и общие виджеты: карточки, плашки, плитки со статистикой.

use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Stroke, TextStyle, Ui, Vec2};

pub const ACCENT: Color32 = Color32::from_rgb(255, 140, 26);
pub const OK: Color32 = Color32::from_rgb(92, 204, 126);
pub const BAD: Color32 = Color32::from_rgb(236, 94, 94);
pub const MUTED: Color32 = Color32::from_rgb(138, 146, 162);
pub const TEXT: Color32 = Color32::from_rgb(230, 232, 238);
/// Фон окна, панелей (сайдбар, строка состояния), карточек и их обводка.
pub const BG: Color32 = Color32::from_rgb(15, 17, 21);
pub const PANEL: Color32 = Color32::from_rgb(22, 26, 34);
pub const CARD: Color32 = Color32::from_rgb(28, 33, 48);
pub const CARD_HOVER: Color32 = Color32::from_rgb(34, 40, 58);
pub const BORDER: Color32 = Color32::from_rgb(42, 48, 64);

pub const RADIUS: u8 = 8;

/// Жирный шрифт — отдельное семейство (у egui нет насыщенности в стиле текста).
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub fn apply_theme(ctx: &egui::Context) {
    install_fonts(ctx);
    let mut st = (*ctx.style()).clone();
    st.spacing.item_spacing = egui::vec2(8.0, 6.0);
    st.spacing.button_padding = egui::vec2(12.0, 6.0);
    st.spacing.interact_size = egui::vec2(40.0, 24.0);
    st.text_styles = [
        (TextStyle::Small, FontId::new(12.5, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(15.0, FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(22.0, bold())),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();

    let v = &mut st.visuals;
    *v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.faint_bg_color = Color32::from_rgb(24, 28, 38);
    v.code_bg_color = BG;
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT.linear_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.window_corner_radius = CornerRadius::same(RADIUS);
    v.menu_corner_radius = CornerRadius::same(RADIUS);
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.noninteractive.bg_fill = CARD;
    v.widgets.noninteractive.weak_bg_fill = CARD;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.inactive.bg_fill = Color32::from_rgb(40, 46, 64);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(40, 46, 64);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.hovered.bg_fill = Color32::from_rgb(52, 60, 82);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(52, 60, 82);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    v.widgets.hovered.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);
    v.widgets.active.bg_fill = ACCENT.linear_multiply(0.6);
    v.widgets.active.weak_bg_fill = ACCENT.linear_multiply(0.6);
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    v.widgets.active.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);
    v.widgets.open.bg_fill = Color32::from_rgb(40, 46, 64);
    v.widgets.open.weak_bg_fill = Color32::from_rgb(40, 46, 64);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(6);
        w.expansion = 0.0;
    }
    ctx.set_style(st);
}

/// Segoe UI (Windows) обычный и жирный; без них — встроенные шрифты egui.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut load = |name: &str, file: &str| -> Option<()> {
        let bytes = std::fs::read(format!("C:\\Windows\\Fonts\\{file}")).ok()?;
        fonts.font_data.insert(name.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
        Some(())
    };
    let has_regular = load("segoe", "segoeui.ttf").is_some();
    let has_bold = load("segoe-bold", "segoeuib.ttf").is_some();
    if has_regular {
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "segoe".to_owned());
    }
    let mut bold_family: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    if has_bold {
        bold_family.insert(0, "segoe-bold".to_owned());
    }
    fonts.families.insert(bold(), bold_family);
    ctx.set_fonts(fonts);
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

/// Логотип тем же рисунком, что и иконка: оранжевый круг с «play».
pub fn logo(ui: &mut Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    p.circle_filled(c, size * 0.46, ACCENT);
    let s = size * 0.5;
    p.add(egui::Shape::convex_polygon(
        vec![c + egui::vec2(-0.22 * s, -0.3 * s), c + egui::vec2(0.36 * s, 0.0), c + egui::vec2(-0.22 * s, 0.3 * s)],
        Color32::WHITE,
        Stroke::NONE,
    ));
}

/// Жирный текст.
pub fn b(text: impl Into<String>) -> RichText {
    RichText::new(text).family(bold())
}

/// Заголовок страницы.
pub fn heading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).family(bold()).size(22.0));
}

/// Карточка: фон, обводка, скругление, отступы; растягивается на всю ширину.
pub fn card<R>(ui: &mut Ui, body: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui)
        })
        .inner
}

/// Раздел: заголовок и карточка под ним.
pub fn section<R>(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui) -> R) -> R {
    ui.add_space(8.0);
    ui.label(RichText::new(title).family(bold()).size(16.0));
    ui.add_space(2.0);
    card(ui, body)
}

/// Цветная плашка: статус, исход, вид задачи.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(color.linear_multiply(0.18))
        .stroke(Stroke::new(1.0_f32, color.linear_multiply(0.6)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| ui.label(RichText::new(text).color(color).size(12.5).family(bold())))
        .response
}

/// Плитка со статистикой: подпись мелко, значение крупно.
pub fn stat_tile(ui: &mut Ui, title: &str, value: &str, color: Option<Color32>) {
    egui::Frame::new()
        .fill(BG)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.set_min_width(96.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(title).color(MUTED).size(12.5));
                ui.label(RichText::new(value).size(24.0).family(bold()).color(color.unwrap_or(TEXT)));
            });
        });
}

/// Главная кнопка страницы.
pub fn primary(ui: &mut Ui, enabled: bool, text: &str) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).color(Color32::WHITE).family(bold())).fill(ACCENT.linear_multiply(0.85)).corner_radius(CornerRadius::same(6)),
    )
    .clicked()
}

/// Цвет уровня FACEIT (1–10), как на сайте.
pub fn level_color(level: u32) -> Color32 {
    match level {
        0..=1 => Color32::from_rgb(190, 190, 190),
        2..=3 => Color32::from_rgb(90, 200, 90),
        4..=7 => Color32::from_rgb(255, 200, 40),
        8..=9 => Color32::from_rgb(255, 120, 40),
        _ => Color32::from_rgb(240, 60, 60),
    }
}

/// Кружок с номером уровня.
pub fn level_badge(ui: &mut Ui, level: u32) {
    let d = 26.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(d), egui::Sense::hover());
    let c = level_color(level);
    ui.painter().circle(rect.center(), d / 2.0, c.linear_multiply(0.2), Stroke::new(2.0_f32, c));
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, level.to_string(), FontId::new(13.0, bold()), c);
}

/// Пустое состояние страницы: крупный глиф, текст и подсказка.
pub fn empty_state(ui: &mut Ui, glyph: &str, title: &str, hint: &str) {
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(glyph).size(44.0).color(MUTED));
        ui.add_space(6.0);
        ui.label(RichText::new(title).size(17.0).family(bold()));
        ui.label(RichText::new(hint).color(MUTED));
        ui.add_space(10.0);
    });
}

/// Заголовок таблицы: приглушённые мелкие подписи колонок.
pub fn table_header(ui: &mut Ui, cols: &[&str]) {
    for c in cols {
        ui.label(RichText::new(*c).color(MUTED).size(12.5));
    }
    ui.end_row();
}

/// Строка таблицы, реагирующая на клик целиком (подсветка при наведении).
pub fn row_hover(ui: &Ui, rect: egui::Rect, id: egui::Id) -> egui::Response {
    let r = ui.interact(rect, id, egui::Sense::click());
    if r.hovered() {
        ui.painter().rect_filled(rect.expand2(egui::vec2(6.0, 2.0)), CornerRadius::same(4), CARD_HOVER);
    }
    r
}

/// Короткая дата «09.10 15:59» и полная «09.10.2026 15:59».
pub fn date(ts: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(ts, 0).single().map(|d| d.format("%d.%m.%Y %H:%M").to_string()).unwrap_or_default()
}

pub fn date_short(ts: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(ts, 0).single().map(|d| d.format("%d.%m %H:%M").to_string()).unwrap_or_default()
}

/// «4:42» из секунд.
pub fn mmss(secs: f32) -> String {
    let s = secs.max(0.0).round() as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

pub fn open_path(p: &std::path::Path) {
    #[cfg(windows)]
    {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]).arg(p);
        let _ = no_window(&mut c).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(p).spawn();
    }
}

pub fn open_url(url: &str) {
    open_path(std::path::Path::new(url));
}

/// Запуск без окна консоли (`CREATE_NO_WINDOW`).
#[cfg(windows)]
fn no_window(cmd: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000)
}

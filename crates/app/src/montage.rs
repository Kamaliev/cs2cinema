//! Страница «Монтаж»: демка, выбор моментов, запись и склейка.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use eframe::egui::{self, RichText, Ui};
use pipeline::{Analysis, PlanOptions, Progress, Stage};

use crate::{
    app::{App, Msg},
    launch,
    settings::Settings,
    ui,
};

pub fn ui(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.heading("Монтаж");
        ui.add_space(12.0);
        let idle = !app.status.busy;
        if ui.add_enabled(idle, egui::Button::new("Открыть демку…")).clicked() {
            if let Some(p) = rfd::FileDialog::new().add_filter("Демки CS2", &["dem", "zst", "gz"]).pick_file() {
                app.open_source(pipeline::Source::File(p));
            }
        }
        ui.label(RichText::new("или перетащите .dem в окно").weak());
        ui.add(egui::TextEdit::singleline(&mut app.montage.link).desired_width(300.0).hint_text("ссылка на матч FACEIT"));
        if ui.add_enabled(idle && !app.montage.link.trim().is_empty(), egui::Button::new("Скачать")).clicked() {
            let link = app.montage.link.clone();
            app.open_faceit(&link);
        }
    });
    ui.separator();

    let Some(a) = &app.montage.analysis else {
        ui.add_space(60.0);
        ui.vertical_centered(|ui| {
            if app.status.busy {
                ui.add(egui::Spinner::new().size(28.0));
                ui.label(RichText::new(&app.status.last).size(18.0));
            } else {
                ui.label(RichText::new("Перетащите сюда демку (.dem, .dem.zst)").size(20.0).weak());
                ui.label(RichText::new("или выберите матч на странице «Матчи»").weak());
            }
        });
        return;
    };

    ui.horizontal(|ui| {
        ui.label(RichText::new(&a.label).strong());
        ui.label(format!("{} · {} моментов", a.m.map, a.found.len()));
        ui.add_space(12.0);
        if ui.small_button("Лучшие 8").clicked() {
            app.montage.checked.iter_mut().enumerate().for_each(|(i, c)| *c = i < 8);
        }
        if ui.small_button("Все").clicked() {
            app.montage.checked.iter_mut().for_each(|c| *c = true);
        }
        if ui.small_button("Снять").clicked() {
            app.montage.checked.iter_mut().for_each(|c| *c = false);
        }
    });
    let n = app.montage.checked.iter().filter(|c| **c).count();
    let secs: f32 = app
        .montage
        .checked
        .iter()
        .zip(&a.found)
        .filter(|(c, _)| **c)
        .map(|(_, h)| (h.last_tick() - h.first_tick()) as f32 / a.m.tick_rate + 6.0)
        .sum();
    ui.label(RichText::new(format!("выбрано {n}, ролик ≈ {secs:.0} с")).weak());

    egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(ui.available_height() - 56.0).show(ui, |ui| {
        egui::Grid::new("moments").striped(true).num_columns(5).spacing([14.0, 4.0]).show(ui, |ui| {
            for h in ["", "Раунд", "Игрок", "Score", "Теги"] {
                ui.label(RichText::new(h).weak().small());
            }
            ui.end_row();
            for (i, h) in a.found.iter().enumerate() {
                ui.checkbox(&mut app.montage.checked[i], "");
                ui.label(format!("R{}", h.round));
                ui.label(a.m.name(h.player));
                ui.label(RichText::new(format!("{:.0}", h.score)).strong());
                ui.label(h.tags.join(", "));
                ui.end_row();
            }
        });
    });

    ui.separator();
    ui.horizontal(|ui| {
        let ready = !app.status.busy && n > 0;
        if ui::primary(ui, ready, "Записать и собрать ролик") {
            start(app, true);
        }
        if ui.add_enabled(ready, egui::Button::new("Только план и скрипты")).clicked() {
            start(app, false);
        }
        if let Some(p) = app.montage.result.clone() {
            if ui.button("Открыть результат").clicked() {
                ui::open_path(&p);
            }
        }
    });
}

fn start(app: &mut App, record: bool) {
    let sel: Vec<usize> = app.montage.checked.iter().enumerate().filter(|(_, c)| **c).map(|(i, _)| i).collect();
    if record {
        if let Err(e) = launch::check_paths(&app.s.hlae_dir, &app.s.cs2_dir) {
            app.status.finish(Err(e));
            app.page = crate::app::Page::Settings;
            return;
        }
    }
    let Some(mut a) = app.montage.analysis.take() else { return };
    app.status.start(if record { "Готовлю запись…" } else { "Строю план…" });
    let s = app.s.clone();
    let out_dir = Path::new(&s.out_root).join(pipeline::console_name(&a.label));
    app.spawn(move |r| {
        let result = run(&mut a, &sel, &s, &out_dir, record, r);
        r.send(Msg::Finished { analysis: a, result });
    });
}

/// Весь путь от плана до готового ролика; исполняется в фоновом потоке.
fn run(a: &mut Analysis, sel: &[usize], s: &Settings, out_dir: &Path, record: bool, p: &dyn Progress) -> Result<PathBuf, String> {
    let o = PlanOptions { fps: s.fps, chronological: s.chronological, flybys: s.flybys, walls: s.walls, ..Default::default() };
    let plan = pipeline::plan(a, sel, &o, out_dir, p)?;
    p.log(&format!("план: {} шотов, ~{:.0} с", plan.timeline.shots.len(), plan.timeline.total_secs()));
    if !record {
        return Ok(out_dir.to_path_buf());
    }
    if !renderer::console_safe(&pipeline::absolute_slash(out_dir)) {
        return Err("в пути результатов есть пробелы — HLAE не загрузит campath; выберите папку без пробелов".into());
    }
    p.stage(Stage::Record);
    let last = plan.timeline.shots.iter().enumerate().max_by_key(|(_, sh)| sh.start_tick).map(|(i, _)| i).unwrap_or(0);
    let demo_name = format!("cs2cinema_{}", pipeline::console_name(&a.label));
    launch::install_files(&s.cs2_dir, out_dir, &demo_name, |path| a.write_demo(path))?;

    let since = SystemTime::now();
    p.log("запускаю CS2 через HLAE…");
    let mut child = launch::launch(&s.hlae_dir, &s.cs2_dir, &demo_name)?;
    p.log(&format!("жду загрузки демки ({} с)…", s.load_wait_secs));
    std::thread::sleep(Duration::from_secs(s.load_wait_secs as u64));
    match launch::send_console(&["exec highlights"], Duration::from_secs(90)) {
        Ok(()) => p.log("скрипт запущен, идёт запись…"),
        Err(e) => p.log(&format!("{e}. Откройте консоль игры (~) и выполните: exec highlights")),
    }
    let root = launch::clips_root(&s.cs2_dir);
    let log = |m: &str| p.log(m);
    launch::wait_for_clips(&root, last, since, &mut || child.try_wait().map(|r| r.is_none()).unwrap_or(false), &log)?;

    p.stage(Stage::Assemble);
    p.log("склеиваю ролик через ffmpeg…");
    let out = out_dir.join("highlights.mp4");
    let opts = renderer::assemble::Options { fps: s.fps, fade_secs: 0.35, out: out.clone() };
    let report = renderer::assemble::assemble(&root, plan.timeline.shots.len(), &opts)?;
    p.log(&format!("ролик {:.0} с: {}", report.total_secs, out.display()));
    ui::open_path(&out);
    Ok(out)
}

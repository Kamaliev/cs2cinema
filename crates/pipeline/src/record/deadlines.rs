//! Таблица дедлайнов записи: единственное место, где решается, сколько ждать каждое событие.
//!
//! Значения по умолчанию — стартовые точки из исследования фазы (D-09); подстройка по реальным
//! замерам — позже. Каждое ожидание машины записи ограничено своим полем, поэтому зависание
//! превращается в ошибку с кодом, а не в бесконечное ожидание.

use std::time::Duration;

use renderer::TakePlan;

/// Нижняя и верхняя граница множителя «запас по времени».
const SCALE_MIN: f32 = 0.5;
const SCALE_MAX: f32 = 5.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Deadlines {
    /// Лаунчер HLAE.exe должен завершиться; если он показывает окно с ошибкой, он не завершится.
    pub launcher: Duration,
    /// `cs2.exe` должен появиться в нашем job после выхода лаунчера.
    pub game_appear: Duration,
    /// Консольный порт игры должен открыться (от появления игры).
    pub console: Duration,
    /// Демка должна загрузиться: метка READY (от отправки запроса).
    pub demo_load: Duration,
    /// Сколько ждать READY после признака «демка уже идёт», прежде чем попросить метку напрямую.
    pub ready_grace: Duration,
    /// Метка SCRIPT после `exec highlights`.
    pub script: Duration,
    /// Первая метка TAKE_START после SCRIPT.
    pub first_take: Duration,
    /// Перемотка между дублями: TAKE_START следующего дубля после TAKE_END предыдущего.
    pub take_seek: Duration,
    /// Во сколько раз запись дубля может быть дольше его игровой длины (с учётом fps записи).
    pub take_factor: f32,
    /// Постоянный запас на дубль сверх `take_factor`.
    pub take_slack: Duration,
    /// Игра должна закрыться после `quit`, а после её выхода job должен опустеть (ffmpeg HLAE дописал
    /// файл); иначе завершаем свой job (это не сбой).
    pub game_close: Duration,
    /// `ffprobe` на одном файле.
    pub ffprobe: Duration,
    /// Нижняя граница дедлайна склейки ffmpeg.
    pub ffmpeg_min: Duration,
    /// Дедлайн склейки: длина ролика в секундах, умноженная на это число.
    pub ffmpeg_factor: f32,
    /// Ответ `-version` у ffmpeg, ffprobe и ffmpeg HLAE в предпроверке.
    pub tool_probe: Duration,
    /// Пробное кодирование NVENC в предпроверке.
    pub nvenc_probe: Duration,
    /// Один кадр превью.
    pub thumbnail: Duration,
    /// Сколько ждать выхода игры после `quit` при отмене или сбое.
    pub quit_wait: Duration,
    /// Подтверждение выхода по handle после завершения своего job.
    pub kill_confirm: Duration,
    /// Как часто смотреть окна своего job.
    pub window_poll: Duration,
    /// D-14: сколько следующая задача ждёт свою прошлую игру.
    pub prev_game_exit: Duration,
    /// D-14: пауза после выхода прошлой игры, прежде чем начать.
    pub next_job_settle: Duration,
}

impl Deadlines {
    /// Таблица по единицам времени: секунды для боя, миллисекунды для тестов.
    fn table(unit: fn(u64) -> Duration) -> Self {
        Deadlines {
            launcher: unit(30),
            game_appear: unit(20),
            console: unit(90),
            demo_load: unit(150),
            ready_grace: unit(10),
            script: unit(30),
            first_take: unit(90),
            take_seek: unit(60),
            take_factor: 2.0,
            take_slack: unit(30),
            game_close: unit(30),
            ffprobe: unit(30),
            ffmpeg_min: unit(600),
            ffmpeg_factor: 15.0,
            tool_probe: unit(10),
            nvenc_probe: unit(20),
            thumbnail: unit(15),
            quit_wait: unit(5),
            kill_confirm: unit(3),
            window_poll: unit(1),
            prev_game_exit: unit(30),
            next_job_settle: unit(3),
        }
    }

    /// Та же таблица в миллисекундах — для чистых тестов машины.
    pub fn fast() -> Self {
        Self::table(Duration::from_millis)
    }

    /// Умножить всю таблицу, включая множители дубля и склейки. `k` ограничен диапазоном 0.5–5.0.
    pub fn scaled(&self, k: f32) -> Self {
        let k = if k.is_finite() {
            k.clamp(SCALE_MIN, SCALE_MAX)
        } else {
            1.0
        };
        let d = |x: Duration| x.mul_f32(k);
        Deadlines {
            launcher: d(self.launcher),
            game_appear: d(self.game_appear),
            console: d(self.console),
            demo_load: d(self.demo_load),
            ready_grace: d(self.ready_grace),
            script: d(self.script),
            first_take: d(self.first_take),
            take_seek: d(self.take_seek),
            take_factor: self.take_factor * k,
            take_slack: d(self.take_slack),
            game_close: d(self.game_close),
            ffprobe: d(self.ffprobe),
            ffmpeg_min: d(self.ffmpeg_min),
            ffmpeg_factor: self.ffmpeg_factor * k,
            tool_probe: d(self.tool_probe),
            nvenc_probe: d(self.nvenc_probe),
            thumbnail: d(self.thumbnail),
            // цели отмены (3 с / ~10 с) и ожидание прошлой игры D-14 не зависят от «Запаса по времени»
            quit_wait: self.quit_wait,
            kill_confirm: self.kill_confirm,
            window_poll: self.window_poll,
            prev_game_exit: self.prev_game_exit,
            next_job_settle: self.next_job_settle,
        }
    }

    /// Сколько можно записывать один дубль (D-12): игровая длина, умноженная на `max(1, fps / 60)` и
    /// на `take_factor` (игра с записью рендерит ~60 к/с, лишние кадры растягивают запись), плюс запас.
    pub fn take_run(&self, t: &TakePlan) -> Duration {
        let fps_k = (t.capture_fps as f64 / 60.0).max(1.0);
        let secs = t.game_secs.max(0.0) as f64 * fps_k * self.take_factor as f64;
        Duration::from_secs_f64(finite(secs)) + self.take_slack
    }

    /// Сколько можно клеить ролик длиной `video_secs` секунд: не меньше `ffmpeg_min`.
    pub fn ffmpeg_for(&self, video_secs: f32) -> Duration {
        let secs = video_secs.max(0.0) as f64 * self.ffmpeg_factor as f64;
        self.ffmpeg_min.max(Duration::from_secs_f64(finite(secs)))
    }
}

/// Неконечное число (NaN, бесконечность) в `Duration` не превращается — берём 0.
fn finite(x: f64) -> f64 {
    if x.is_finite() && x >= 0.0 { x } else { 0.0 }
}

impl Default for Deadlines {
    fn default() -> Self {
        Self::table(Duration::from_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(game_secs: f32, capture_fps: u32) -> TakePlan {
        TakePlan {
            index: 0,
            shot: 0,
            game_secs,
            capture_fps,
        }
    }

    #[test]
    fn take_run_scales_with_game_length_and_capture_fps() {
        let d = Deadlines::default();
        assert_eq!(
            d.take_run(&take(5.0, 60)),
            Duration::from_secs(40),
            "5 с игры -> 5*1*2+30"
        );
        assert_eq!(
            d.take_run(&take(5.0, 30)),
            Duration::from_secs(40),
            "fps ниже 60 дедлайн не сокращает"
        );
        assert_eq!(
            d.take_run(&take(5.0, 240)),
            Duration::from_secs(70),
            "240 к/с: 5*4*2+30"
        );
    }

    #[test]
    fn scaled_multiplies_everything_including_the_take_deadline_within_the_bounds() {
        let d = Deadlines::default();
        let t = take(5.0, 60);
        let x2 = d.scaled(2.0);
        assert_eq!(x2.console, Duration::from_secs(180));
        assert_eq!(
            x2.take_run(&t),
            Duration::from_secs(80),
            "дедлайн дубля тоже вырос вдвое"
        );
        assert_eq!(d.scaled(100.0), d.scaled(5.0), "верхняя граница 5.0");
        assert_eq!(d.scaled(0.0), d.scaled(0.5), "нижняя граница 0.5");
        assert_eq!(d.scaled(f32::NAN), d, "NaN не портит таблицу");
    }

    #[test]
    fn cancel_and_exit_budgets_do_not_grow_with_the_time_margin() {
        let d = Deadlines::default().scaled(5.0);
        assert_eq!(d.quit_wait, Duration::from_secs(5));
        assert_eq!(d.kill_confirm, Duration::from_secs(3));
        assert_eq!(d.window_poll, Duration::from_secs(1));
        assert_eq!(d.prev_game_exit, Duration::from_secs(30));
        assert_eq!(d.next_job_settle, Duration::from_secs(3));
        assert_eq!(
            d.tool_probe,
            Duration::from_secs(50),
            "пробы утилит растут с запасом"
        );
        assert_eq!(d.nvenc_probe, Duration::from_secs(100));
        assert_eq!(d.thumbnail, Duration::from_secs(75));
    }

    #[test]
    fn ffmpeg_deadline_never_drops_below_the_minimum() {
        let d = Deadlines::default();
        assert_eq!(d.ffmpeg_for(10.0), Duration::from_secs(600));
        assert_eq!(d.ffmpeg_for(100.0), Duration::from_secs(1500));
    }
}

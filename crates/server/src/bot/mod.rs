//! Telegram-бот поверх `Core`: шаговый автомат без собственных потоков.
//!
//! Вся логика идёт через `Bot::step()`: опрос `getUpdates`, приём файлов (`ingest_once`), тик уведомлений
//! (`notify_once`) и доставка роликов (`deliver_once`). Потоки (план 07-10) только крутят `step` и опрос.
//! Правила: бот не пишет `jobs.state` (задачи двигает только `Core`); время берётся из `core.clock()`
//! (в тестах — `ManualClock`); с Telegram говорят через `Arc<dyn Tg>`; мьютекс базы не держат во время
//! вызовов Telegram и копирования файлов; текст уходит без `parse_mode` и через `Redactor::clean`.

pub mod commands;
pub mod deliver;
pub mod dispatch;
pub mod ingest;
pub mod notify;
pub mod store;

use crate::Result;
use crate::core::Core;
use crate::log::Log;
use crate::tg::{Me, Tg, TgError};
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Чем отправлять готовый ролик.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendAs {
    Video,
    Document,
}

impl SendAs {
    pub fn as_str(self) -> &'static str {
        match self {
            SendAs::Video => "video",
            SendAs::Document => "document",
        }
    }

    pub fn parse(s: &str) -> Option<SendAs> {
        match s {
            "video" => Some(SendAs::Video),
            "document" => Some(SendAs::Document),
            _ => None,
        }
    }
}

/// Итог одного опроса `getUpdates`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollOutcome {
    /// Разобрано обновлений.
    Updates(usize),
    /// `409`: обновления забирает ещё кто-то (второй экземпляр бота или вебхук).
    Conflict,
    Failed(String),
}

/// Настройки бота с умолчаниями фазы.
#[derive(Debug, Clone)]
pub struct BotSettings {
    /// Корень файлов `telegram-bot-api`, как его возвращает `getFile`.
    pub files_root: PathBuf,
    /// Тот же корень на стороне сервера.
    pub files_root_local: PathBuf,
    /// Папка `blobs/results` сервера, как её видит `telegram-bot-api` (для `file://`).
    pub results_prefix: PathBuf,
    pub send_as: SendAs,
    /// Не чаще одной правки статуса за столько миллисекунд (D-04).
    pub status_edit_ms: i64,
    pub user_max_active: i64,
    pub admin_max_active: i64,
    /// Общий потолок активных задач в очереди (D-03).
    pub queue_max_active: i64,
    /// Незнакомцу отвечают не чаще раза за столько миллисекунд.
    pub stranger_reply_ms: i64,
    pub poll_timeout_secs: u64,
}

impl BotSettings {
    /// Оба корня файлов равны `files_root`, `results_prefix` — абсолютный `data_dir/blobs/results`.
    pub fn new(data_dir: &Path, files_root: PathBuf) -> Self {
        let abs = if data_dir.is_absolute() {
            data_dir.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|d| d.join(data_dir))
                .unwrap_or_else(|_| data_dir.to_path_buf())
        };
        Self {
            files_root_local: files_root.clone(),
            files_root,
            results_prefix: abs.join("blobs").join("results"),
            send_as: SendAs::Video,
            status_edit_ms: 10_000,
            user_max_active: 2,
            admin_max_active: 5,
            queue_max_active: 10,
            stranger_reply_ms: 600_000,
            poll_timeout_secs: 25,
        }
    }
}

/// Что сделал один `step`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StepReport {
    pub updates: usize,
    pub ingested: usize,
    pub notified: usize,
    pub delivered: usize,
}

/// Больше столько однотипных шагов за один `step` не делаем: защита от зацикливания.
const MAX_STEPS: usize = 256;

#[derive(Clone)]
pub struct Bot {
    core: Core,
    tg: Arc<dyn Tg>,
    log: Log,
    settings: Arc<BotSettings>,
    /// Когда последний раз отвечали незнакомцу: `from.id` → unix-мс.
    strangers: Arc<Mutex<HashMap<i64, i64>>>,
}

impl Bot {
    pub fn new(core: Core, tg: Arc<dyn Tg>, log: Log, settings: BotSettings) -> Self {
        Self {
            core,
            tg,
            log,
            settings: Arc::new(settings),
            strangers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn core(&self) -> &Core {
        &self.core
    }

    pub fn tg(&self) -> &Arc<dyn Tg> {
        &self.tg
    }

    pub fn settings(&self) -> &BotSettings {
        &self.settings
    }

    pub fn log(&self) -> &Log {
        &self.log
    }

    pub(crate) fn now_ms(&self) -> i64 {
        self.core.clock().now_ms()
    }

    /// Можно ли ответить незнакомцу `from_id` сейчас; если да — запоминает время ответа.
    pub(crate) fn stranger_may_get_reply(&self, from_id: i64) -> bool {
        let now = self.now_ms();
        let mut seen = self.strangers.lock().unwrap_or_else(|e| e.into_inner());
        match seen.get(&from_id) {
            Some(last) if now - *last < self.settings.stranger_reply_ms => false,
            _ => {
                seen.insert(from_id, now);
                true
            }
        }
    }

    /// Отправляет текст в чат; сбой пишется в журнал, а не поднимается (ответ пользователю не критичен).
    pub(crate) fn say(&self, chat_id: i64, text: &str) -> Option<i64> {
        let clean = self.core.redactor().clean(text, 4000);
        match self.tg.send_message(chat_id, &clean) {
            Ok(id) => Some(id),
            Err(e) => {
                self.log
                    .warn(&format!("бот: не удалось отправить сообщение: {e}"));
                None
            }
        }
    }

    /// Правит сообщение; `message is not modified` — успех. Сбой пишется в журнал.
    pub(crate) fn edit(&self, chat_id: i64, message_id: i64, text: &str) {
        let clean = self.core.redactor().clean(text, 4000);
        match self.tg.edit_message_text(chat_id, message_id, &clean) {
            Ok(()) => {}
            Err(e) if e.is_not_modified() => {}
            Err(e) => self
                .log
                .warn(&format!("бот: не удалось поправить сообщение: {e}")),
        }
    }

    /// Старт: `getMe`, меню команд, восстановление после перезапуска.
    pub fn startup(&self) -> Result<Me> {
        let me = self.tg.get_me().map_err(|e| e.to_string())?;
        if let Err(e) = self.tg.set_my_commands(&commands::menu()) {
            self.log
                .warn(&format!("бот: меню команд не обновлено: {e}"));
        }
        self.recover()?;
        Ok(me)
    }

    /// `downloading` → `pending`, `sending` → `uncertain` (без повторной отправки).
    pub fn recover(&self) -> Result<()> {
        let (inbox, sending) = store::recover(self.core.db(), self.now_ms())?;
        if inbox + sending.len() > 0 {
            self.log.info(&format!(
                "бот: после перезапуска вернул в очередь приёма {inbox}, неясных отправок {}",
                sending.len()
            ));
        }
        // Друг должен узнать, что ролик мог не дойти, и про `/resend` (WR-02).
        for id in sending {
            deliver::tell_uncertain_after_restart(self, id);
        }
        Ok(())
    }

    /// Один опрос `getUpdates`; смещение и принятое сообщение пишутся одной транзакцией.
    pub fn poll_once(&self, timeout_secs: u64) -> Result<usize> {
        match self.poll_tick(timeout_secs) {
            PollOutcome::Updates(n) => Ok(n),
            PollOutcome::Conflict => Err(
                "Telegram ответил ошибкой 409: обновления забирает другой экземпляр бота"
                    .to_owned(),
            ),
            PollOutcome::Failed(e) => Err(e),
        }
    }

    /// Тот же опрос, но с различением `409` (второй экземпляр бота) от прочих сбоев.
    pub fn poll_tick(&self, timeout_secs: u64) -> PollOutcome {
        self.poll_inner(timeout_secs, None)
    }

    /// Опрос потока: если за время ожидания пришла остановка, обновления не разбираются и смещение не
    /// двигается — Telegram отдаст их снова при следующем запуске.
    fn poll_inner(&self, timeout_secs: u64, stop: Option<&AtomicBool>) -> PollOutcome {
        let offset = match store::offset(self.core.db()) {
            Ok(o) => o,
            Err(e) => return PollOutcome::Failed(e),
        };
        let updates = match self.tg.get_updates(offset, timeout_secs) {
            Ok(u) => u,
            Err(TgError::Api { code: 409, .. }) => return PollOutcome::Conflict,
            Err(e) => return PollOutcome::Failed(e.to_string()),
        };
        if stop.is_some_and(|s| s.load(Ordering::SeqCst)) {
            return PollOutcome::Updates(0);
        }
        let n = updates.len();
        for u in updates {
            if let Err(e) = dispatch::handle_update(self, u) {
                return PollOutcome::Failed(e);
            }
        }
        PollOutcome::Updates(n)
    }

    /// Принимает одно ожидающее входящее: `getFile`, `Core::submit`. `false` — ждать нечего.
    pub fn ingest_once(&self) -> Result<bool> {
        ingest::ingest_once(self)
    }

    pub fn notify_once(&self) -> Result<usize> {
        notify::tick(self)
    }

    /// Отправляет один ожидающий ролик. `false` — ждать нечего.
    pub fn deliver_once(&self) -> Result<bool> {
        deliver::deliver_once(self)
    }

    /// Опрос без ожидания, приём всех ожидающих файлов, тик уведомлений и все ожидающие доставки.
    pub fn step(&self) -> Result<StepReport> {
        let mut r = StepReport {
            updates: self.poll_once(0)?,
            ..Default::default()
        };
        while r.ingested < MAX_STEPS && self.ingest_once()? {
            r.ingested += 1;
        }
        r.notified = self.notify_once()?;
        while r.delivered < MAX_STEPS && self.deliver_once()? {
            r.delivered += 1;
        }
        Ok(r)
    }
}

// ---------------------------------------------------------------------------------------------------------
// Потоки бота (план 07-10)
// ---------------------------------------------------------------------------------------------------------

/// Сколько `BotHandle::stop` ждёт завершения потоков: висящий `getFile` или `sendVideo` не держит процесс.
const STOP_GRACE: Duration = Duration::from_secs(3);
/// Пауза потока приёма при пустой очереди.
const INGEST_IDLE: Duration = Duration::from_millis(500);
/// Период тика уведомлений.
const NOTIFY_EVERY: Duration = Duration::from_secs(2);
/// Пауза потока доставки при пустой очереди.
const DELIVER_IDLE: Duration = Duration::from_secs(1);
/// Пауза после сбоя шага.
const ERROR_PAUSE: Duration = Duration::from_secs(2);
/// Пауза после `409`: второй экземпляр бота не отпустит обновления быстро.
const CONFLICT_PAUSE: Duration = Duration::from_secs(60);
/// Шаг сна с проверкой флага остановки.
const SLEEP_STEP: Duration = Duration::from_millis(200);

/// Что поток делает после шага.
enum Pace {
    /// Сразу следующий шаг.
    Again,
    Pause(Duration),
}

/// Запущенные потоки бота: `bot-poll`, `bot-ingest`, `bot-notify`, `bot-deliver`.
pub struct BotHandle {
    bot: Bot,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicUsize>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl BotHandle {
    pub fn bot(&self) -> &Bot {
        &self.bot
    }

    /// Ставит общий флаг остановки и ждёт потоки не дольше трёх секунд. Поток, застрявший в вызове Telegram,
    /// не `join`-ится: он завершится сам, когда вызов вернётся, а процесс из-за него не зависает.
    pub fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        let until = Instant::now() + STOP_GRACE;
        while self.alive.load(Ordering::SeqCst) > 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        for t in self.threads {
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}

/// Уменьшает счётчик живых потоков при выходе, в том числе при панике.
struct AliveGuard(Arc<AtomicUsize>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Спит `d` шагами по 200 мс; `false` — пришла остановка.
fn sleep_checked(d: Duration, stop: &AtomicBool) -> bool {
    let until = Instant::now() + d;
    loop {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        let now = Instant::now();
        if now >= until {
            return true;
        }
        std::thread::sleep(SLEEP_STEP.min(until - now));
    }
}

/// Поток, который крутит `step` до остановки; паника шага пишется в журнал, поток продолжает работу.
fn spawn_loop(
    name: &str,
    alive: &Arc<AtomicUsize>,
    stop: &Arc<AtomicBool>,
    log: &Log,
    mut step: impl FnMut() -> Pace + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let (stop, log, label) = (stop.clone(), log.clone(), name.to_owned());
    alive.fetch_add(1, Ordering::SeqCst);
    let guard = AliveGuard(alive.clone());
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let _guard = guard;
            while !stop.load(Ordering::SeqCst) {
                let pace = match std::panic::catch_unwind(AssertUnwindSafe(&mut step)) {
                    Ok(p) => p,
                    Err(p) => {
                        let text = crate::http::panic_text(&*p);
                        log.warn(&format!("бот ({label}): шаг упал: {text}"));
                        Pace::Pause(ERROR_PAUSE)
                    }
                };
                if let Pace::Pause(d) = pace
                    && !sleep_checked(d, &stop)
                {
                    return;
                }
            }
        })
}

/// Не засоряет журнал одинаковыми сбоями: пишет первый и каждый тридцатый повтор.
struct Throttle {
    last: Option<String>,
    repeats: u32,
}

impl Throttle {
    fn new() -> Self {
        Self {
            last: None,
            repeats: 0,
        }
    }

    fn warn(&mut self, log: &Log, what: &str, err: &str) {
        if self.last.as_deref() == Some(err) {
            self.repeats += 1;
            if !self.repeats.is_multiple_of(30) {
                return;
            }
        } else {
            self.last = Some(err.to_owned());
            self.repeats = 0;
        }
        log.warn(&format!("бот: {what}: {err}"));
    }

    fn ok(&mut self) {
        self.last = None;
        self.repeats = 0;
    }
}

/// Запускает потоки бота. `stop` — общий с HTTP флаг остановки. Поток, который не удалось запустить,
/// пишется в журнал; остальные работают.
pub fn spawn(bot: Bot, stop: Arc<AtomicBool>) -> BotHandle {
    let alive = Arc::new(AtomicUsize::new(0));
    let log = bot.log().clone();
    let mut threads = Vec::new();
    let mut push = |r: std::io::Result<std::thread::JoinHandle<()>>| match r {
        Ok(h) => threads.push(h),
        Err(e) => log.warn(&format!("бот: не удалось запустить поток: {e}")),
    };

    let poll_timeout = bot.settings().poll_timeout_secs;
    push(spawn_loop("bot-poll", &alive, &stop, &log, {
        let (bot, stop, log) = (bot.clone(), stop.clone(), log.clone());
        let (mut throttle, mut conflict_warned) = (Throttle::new(), false);
        move || match bot.poll_inner(poll_timeout, Some(stop.as_ref())) {
            PollOutcome::Updates(_) => {
                throttle.ok();
                conflict_warned = false;
                Pace::Again
            }
            PollOutcome::Conflict => {
                if !conflict_warned {
                    conflict_warned = true;
                    log.warn(
                        "бот: второй экземпляр бота забирает обновления (409) — опрос приостановлен на минуту",
                    );
                }
                Pace::Pause(CONFLICT_PAUSE)
            }
            PollOutcome::Failed(e) => {
                throttle.warn(&log, "опрос Telegram не удался", &e);
                Pace::Pause(ERROR_PAUSE)
            }
        }
    }));

    push(spawn_loop("bot-ingest", &alive, &stop, &log, {
        let (bot, log) = (bot.clone(), log.clone());
        let mut throttle = Throttle::new();
        move || match bot.ingest_once() {
            Ok(true) => {
                throttle.ok();
                Pace::Again
            }
            Ok(false) => Pace::Pause(INGEST_IDLE),
            Err(e) => {
                throttle.warn(&log, "приём демки не удался", &e);
                Pace::Pause(ERROR_PAUSE)
            }
        }
    }));

    push(spawn_loop("bot-notify", &alive, &stop, &log, {
        let (bot, log) = (bot.clone(), log.clone());
        let mut throttle = Throttle::new();
        move || {
            match bot.notify_once() {
                Ok(_) => throttle.ok(),
                Err(e) => throttle.warn(&log, "тик уведомлений не удался", &e),
            }
            Pace::Pause(NOTIFY_EVERY)
        }
    }));

    push(spawn_loop("bot-deliver", &alive, &stop, &log, {
        let (bot, log) = (bot.clone(), log.clone());
        let mut throttle = Throttle::new();
        move || match bot.deliver_once() {
            Ok(true) => {
                throttle.ok();
                Pace::Again
            }
            Ok(false) => Pace::Pause(DELIVER_IDLE),
            Err(e) => {
                throttle.warn(&log, "доставка ролика не удалась", &e);
                Pace::Pause(ERROR_PAUSE)
            }
        }
    }));

    BotHandle {
        bot,
        stop,
        alive,
        threads,
    }
}

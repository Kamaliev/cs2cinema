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
use crate::tg::{Me, Tg};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Чем отправлять готовый ролик.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendAs {
    Video,
    Document,
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
        if inbox + sending > 0 {
            self.log.info(&format!(
                "бот: после перезапуска вернул в очередь приёма {inbox}, неясных отправок {sending}"
            ));
        }
        Ok(())
    }

    /// Один опрос `getUpdates`; смещение и принятое сообщение пишутся одной транзакцией.
    pub fn poll_once(&self, timeout_secs: u64) -> Result<usize> {
        let offset = store::offset(self.core.db())?;
        let updates = self
            .tg
            .get_updates(offset, timeout_secs)
            .map_err(|e| e.to_string())?;
        let n = updates.len();
        for u in updates {
            dispatch::handle_update(self, u)?;
        }
        Ok(n)
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

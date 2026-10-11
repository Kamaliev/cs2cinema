//! Тик уведомлений. Пока он только отдаёт доставке задачи, которые ядро довело до `done`
//! (`tg_jobs.delivery`: `none` → `pending`); статусы и отчёты о сбоях добавляют планы 07-08 и 07-09.

use super::{Bot, store};
use crate::Result;

/// Возвращает число задач, переведённых в ожидание доставки.
pub fn tick(bot: &Bot) -> Result<usize> {
    store::promote_done(bot.core().db(), bot.now_ms())
}

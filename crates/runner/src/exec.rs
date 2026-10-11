//! Шов к исполнению задачи: раннер знает только `Task` и `Executor`.
//!
//! Приложение фазы 8 реализует `Executor` поверх своей очереди, настоящий исполнитель с `pipeline::job::run_job`
//! подключается в плане 06-10, а в тестах стоит заглушка.

use std::path::{Path, PathBuf};

/// Задача раннера: всё, что исполнителю нужно знать о выданной сервером работе. Пути строит сам раннер из
/// номера задачи; имя файла и параметры от пользователя сюда не попадают.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    /// Номер задачи на сервере: уникален в папке раннера и служит `JobSpec.job_id`.
    pub job_id: i64,
    /// Номер попытки с единицы.
    pub attempt: u32,
    /// Скачанная и проверенная демка: `remote/srv{job_id}.dem`.
    pub demo: PathBuf,
    /// Ссылка FACEIT, если задача пришла с ней.
    pub faceit: Option<String>,
    /// Steam64 героя или 0, если он неизвестен.
    pub hero_steam: u64,
    pub hero_name: String,
    /// Повтор по `record.json` из `out_dir`, а не заново.
    pub resume: bool,
    /// Папка результата прошлой попытки, если она известна.
    pub out_dir: Option<PathBuf>,
}

/// Исполнитель задачи.
pub trait Executor: Send + Sync {
    /// Делает ролик по задаче; `p` получает этапы и строки журнала и сообщает об отмене.
    fn run(
        &self,
        task: &Task,
        p: &dyn pipeline::Progress,
    ) -> Result<pipeline::job::Outcome, pipeline::RunError>;

    /// Убирает за прошлым запуском после падения; возвращает строки для журнала. По умолчанию ничего не делает.
    fn recover(&self, _out_dir: &Path) -> Vec<String> {
        Vec::new()
    }
}

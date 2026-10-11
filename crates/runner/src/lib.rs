//! Раннер: исходящий цикл «claim → демка → исполнитель → результат → finish»; одна задача за раз.
//!
//! Раннер сам ходит к серверу, сервер к нему никогда не подключается. Всё, что делает задачу роликом, прячется
//! за `Executor`; здесь только транспорт и порядок шагов. Токен живёт в `Token` и в журнал не попадает.

pub mod exec;
pub mod link;

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pipeline::record::log::one_line;
use sha2::{Digest, Sha256};
use wire::{Assignment, CHUNK_SIZE, Failure, FinishRequest, HelloRequest, JobState, Outcome};

pub use exec::{Executor, Task};
pub use link::{Link, LinkError, Token};

/// Код сбоя исполнителя без собственного кода. Позже заменится на `wire::fail::JOB_ERROR`.
const JOB_ERROR: &str = "JOB_ERROR";
/// Код «демка не сошлась по SHA-256». Позже заменится на `wire::fail::BAD_DEMO`.
const BAD_DEMO: &str = "BAD_DEMO";

/// Настройки раннера.
#[derive(Clone)]
pub struct Config {
    /// Адрес сервера: `http://хост:порт`.
    pub server: String,
    pub token: Token,
    /// Рабочая папка: `remote/` (демки), `out/` (результаты), `runner.log`.
    pub dir: PathBuf,
    /// Имя раннера для журнала сервера.
    pub name: String,
    /// Замена `poll_secs` сервера (для тестов).
    pub poll_secs: Option<u64>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("server", &self.server)
            .field("token", &"***")
            .field("dir", &self.dir)
            .field("name", &self.name)
            .field("poll_secs", &self.poll_secs)
            .finish()
    }
}

/// Итог одного шага цикла.
#[derive(Debug, Clone, PartialEq)]
pub enum Tick {
    /// Задач нет.
    Idle,
    /// Задача выдана и закрыта на сервере.
    Settled { job_id: i64, state: JobState },
}

/// Журнал `runner.log`: строки `{время} {текст}`, значение токена заменено на `***`.
struct Log {
    file: Mutex<File>,
    secret: String,
}

impl Log {
    fn open(path: &Path, secret: &str) -> Result<Log, String> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("не удалось открыть {}: {e}", path.display()))?;
        Ok(Log {
            file: Mutex::new(file),
            secret: secret.to_owned(),
        })
    }

    fn line(&self, text: &str) {
        let text = if self.secret.is_empty() {
            text.to_owned()
        } else {
            text.replace(&self.secret, "***")
        };
        let line = format!(
            "{} {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            one_line(&text)
        );
        eprintln!("{line}");
        if let Ok(mut f) = self.file.lock() {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Ход задачи для исполнителя: этапы и строки идут в `runner.log` и stderr.
struct RunnerProgress {
    log: Arc<Log>,
}

impl pipeline::Progress for RunnerProgress {
    fn stage(&self, s: pipeline::Stage) {
        self.log.line(&format!("этап: {}", s.title()));
    }

    fn log(&self, msg: &str) {
        self.log.line(msg);
    }
}

/// Запись скачиваемой демки: в файл и в хеш одновременно.
struct HashingFile<'a> {
    file: &'a mut File,
    hasher: &'a mut Sha256,
}

impl Write for HashingFile<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.file.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub struct Runner {
    cfg: Config,
    link: Link,
    exec: Arc<dyn Executor>,
    log: Arc<Log>,
    stop: Arc<AtomicBool>,
    poll_secs: Option<u64>,
}

impl Runner {
    pub fn new(cfg: Config, exec: Arc<dyn Executor>) -> Result<Runner, String> {
        for sub in ["remote", "out"] {
            let dir = cfg.dir.join(sub);
            fs::create_dir_all(&dir)
                .map_err(|e| format!("не удалось создать {}: {e}", dir.display()))?;
        }
        let log = Arc::new(Log::open(&cfg.dir.join("runner.log"), cfg.token.secret())?);
        let link = Link::new(&cfg.server, cfg.token.clone())?;
        Ok(Runner {
            cfg,
            link,
            exec,
            log,
            stop: Arc::new(AtomicBool::new(false)),
            poll_secs: None,
        })
    }

    /// Флаг остановки `run`: поднимите его из другого потока.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// Один шаг: представиться серверу (в первый раз), спросить задачу, довести выданную до конца.
    pub fn tick(&mut self) -> Result<Tick, String> {
        if self.poll_secs.is_none() {
            self.hello()?;
        }
        let poll = self.cfg.poll_secs.or(self.poll_secs).unwrap_or(0);
        let assignment = self.link.claim(poll).map_err(|e| e.to_string())?;
        match assignment {
            None => Ok(Tick::Idle),
            Some(a) => self.handle(a),
        }
    }

    /// Цикл до флага остановки; ошибки связи — пауза 1, 2, 4 … 30 с.
    pub fn run(&mut self) {
        let mut pause = 0u64;
        while !self.stop.load(Ordering::SeqCst) {
            match self.tick() {
                Ok(Tick::Settled { .. }) => pause = 0,
                Ok(Tick::Idle) => {
                    pause = 0;
                    // Long-poll уже ждал на сервере; без него (poll_secs = 0) не крутимся вхолостую.
                    if self.cfg.poll_secs == Some(0) {
                        self.sleep(1);
                    }
                }
                Err(e) => {
                    self.log.line(&format!("ошибка: {e}"));
                    pause = (pause * 2).clamp(1, 30);
                    self.sleep(pause);
                }
            }
        }
    }

    /// Пауза с проверкой флага остановки каждые 200 мс.
    fn sleep(&self, secs: u64) {
        let until = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < until && !self.stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn hello(&mut self) -> Result<(), String> {
        let req = HelloRequest {
            protocol: wire::PROTOCOL_VERSION,
            name: self.cfg.name.clone(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            status: "idle".to_owned(),
            holding: Vec::new(),
        };
        match self.link.hello(&req) {
            Ok(reply) => {
                self.log.line(&format!(
                    "на связи с сервером {} (протокол {})",
                    reply.server_version,
                    wire::PROTOCOL_VERSION
                ));
                self.poll_secs = Some(reply.poll_secs);
                Ok(())
            }
            Err(LinkError::Status {
                status: 426,
                message,
                ..
            }) => Err(format!(
                "версия протокола сервера не совпадает с раннером: {message}"
            )),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Выданная задача: демка → исполнитель → результат → `finish`.
    fn handle(&mut self, a: Assignment) -> Result<Tick, String> {
        let started = Instant::now();
        let (id, epoch) = (a.job_id, a.epoch);
        self.log
            .line(&format!("задача #{id} выдана (эпоха {epoch})"));

        let demo = match self.fetch_demo(&a)? {
            Some(path) => path,
            None => {
                let failure = Failure {
                    code: BAD_DEMO.to_owned(),
                    message: "демка повреждена при передаче".to_owned(),
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    ..Default::default()
                };
                return self.finish(id, epoch, Outcome::Failed(failure));
            }
        };

        let task = Task {
            job_id: id,
            attempt: 1,
            demo,
            faceit: a.faceit_link.clone(),
            hero_steam: a
                .player
                .steam64
                .as_deref()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
            hero_name: a.player.name.clone(),
            resume: false,
            out_dir: None,
        };
        let progress = RunnerProgress {
            log: self.log.clone(),
        };
        match self.exec.run(&task, &progress) {
            Ok(outcome) => {
                let (size, sha256) = self.upload(&a, &outcome.result)?;
                self.finish(id, epoch, Outcome::Done { size, sha256 })
            }
            Err(e) => {
                self.log
                    .line(&format!("задача #{id} не удалась: {}", e.message));
                let failure = Failure {
                    code: JOB_ERROR.to_owned(),
                    message: one_line(&e.message),
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    ..Default::default()
                };
                self.finish(id, epoch, Outcome::Failed(failure))
            }
        }
    }

    fn finish(&self, id: i64, epoch: i64, outcome: Outcome) -> Result<Tick, String> {
        let reply = self
            .link
            .finish(id, &FinishRequest { epoch, outcome })
            .map_err(|e| format!("задача #{id}: finish: {e}"))?;
        self.log
            .line(&format!("задача #{id}: {}", reply.state.as_str()));
        Ok(Tick::Settled {
            job_id: id,
            state: reply.state,
        })
    }

    /// Скачивает демку в `remote/srv{id}.dem` кусками и сверяет размер и SHA-256 с выдачей.
    /// `None` — демка не сошлась; `Err` — сбой связи или диска.
    fn fetch_demo(&self, a: &Assignment) -> Result<Option<PathBuf>, String> {
        // Имя файла строится только из номера задачи: `orig_name` от пользователя в путях не участвует.
        let dest = self
            .cfg
            .dir
            .join("remote")
            .join(format!("srv{}.dem", a.job_id));
        let part = self
            .cfg
            .dir
            .join("remote")
            .join(format!("srv{}.dem.part", a.job_id));
        let mut file = File::create(&part)
            .map_err(|e| format!("не удалось создать {}: {e}", part.display()))?;
        let mut hasher = Sha256::new();
        let want = a.demo.size;
        let mut have = 0u64;
        self.log.line(&format!(
            "задача #{}: скачиваю демку, {want} байт",
            a.job_id
        ));
        while have < want {
            let mut sink = HashingFile {
                file: &mut file,
                hasher: &mut hasher,
            };
            let got = self
                .link
                .demo_range(a.job_id, a.epoch, have, &mut sink)
                .map_err(|e| format!("задача #{}: скачивание демки: {e}", a.job_id))?;
            if got == 0 {
                return Err(format!(
                    "задача #{}: сервер прислал пустой кусок демки",
                    a.job_id
                ));
            }
            have += got;
        }
        file.flush().map_err(|e| format!("запись демки: {e}"))?;
        drop(file);
        let sha = hex(&hasher.finalize());
        if have != want || want == 0 || !sha.eq_ignore_ascii_case(&a.demo.sha256) {
            self.log.line(&format!(
                "задача #{}: демка не сошлась: получено {have} из {want} байт",
                a.job_id
            ));
            let _ = fs::remove_file(&part);
            return Ok(None);
        }
        fs::rename(&part, &dest).map_err(|e| format!("не удалось переименовать демку: {e}"))?;
        Ok(Some(dest))
    }

    /// Загружает результат кусками по `CHUNK_SIZE` с смещения, которое назвал сервер.
    /// Возвращает размер и SHA-256 всего файла для `finish`.
    fn upload(&self, a: &Assignment, result: &Path) -> Result<(u64, String), String> {
        let id = a.job_id;
        let mut file = File::open(result)
            .map_err(|e| format!("не удалось открыть результат {}: {e}", result.display()))?;
        let size = file
            .metadata()
            .map_err(|e| format!("не удалось прочитать размер результата: {e}"))?
            .len();
        // Первый проход: SHA-256 всего файла, не загружая его в память целиком.
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = file
                .read(&mut buf)
                .map_err(|e| format!("чтение результата: {e}"))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let sha256 = hex(&hasher.finalize());

        let mut offset = self
            .link
            .result_head(id, a.epoch)
            .map_err(|e| format!("задача #{id}: HEAD результата: {e}"))?;
        if offset > size {
            return Err(format!(
                "задача #{id}: сервер принял {offset} байт, а в результате {size}"
            ));
        }
        self.log.line(&format!(
            "задача #{id}: загружаю результат, {size} байт, с {offset}"
        ));
        let mut chunk = vec![0u8; CHUNK_SIZE as usize];
        let mut resyncs = 0;
        while offset < size {
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| format!("чтение результата: {e}"))?;
            let len = (size - offset).min(CHUNK_SIZE) as usize;
            file.read_exact(&mut chunk[..len])
                .map_err(|e| format!("чтение результата: {e}"))?;
            match self.link.result_put(id, a.epoch, offset, &chunk[..len]) {
                Ok(next) if next == offset + len as u64 => offset = next,
                Ok(next) => {
                    return Err(format!(
                        "задача #{id}: сервер принял до {next}, ожидалось {}",
                        offset + len as u64
                    ));
                }
                // Сервер принял другое число байт: продолжаем с названного им смещения.
                Err(LinkError::Status {
                    error,
                    offset: Some(server),
                    ..
                }) if error == wire::codes::OFFSET_MISMATCH && server <= size && resyncs < 5 => {
                    resyncs += 1;
                    offset = server;
                }
                Err(e) => return Err(format!("задача #{id}: загрузка результата: {e}")),
            }
        }
        Ok((size, sha256))
    }
}

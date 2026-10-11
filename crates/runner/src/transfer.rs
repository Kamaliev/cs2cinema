//! Передача файлов раннера: докачка демки и загрузка результата со смещения, которое назвал сервер.
//!
//! Файлы читаются и пишутся кусками, целиком в память не грузятся. Временные ошибки (сеть, 5xx, `overloaded`)
//! повторяются с паузой 1, 2, 4 … 30 с без конца, пока функция остановки не скажет прервать (`StopFn`); окончательные отказы сервера
//! (`stale_epoch`, `job_terminal`, повторный `sha256_mismatch`) возвращаются как `Dropped`, решать их судьбу
//! будет вызывающий (результат удаляется из outbox с записью причины).

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use wire::codes::{
    JOB_TERMINAL, NOT_FOUND, OFFSET_MISMATCH, SHA256_MISMATCH, STALE_EPOCH, UPLOAD_BUSY,
};
use wire::{CHUNK_SIZE, FinishRequest, JobState, Outcome};

use crate::link::{Link, LinkError};
use crate::state::HeldJob;

/// Почему передачу пора прервать. Порядок важности: потеря задачи, отмена сервера, локальная остановка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopWhy {
    /// Остановка раннера (не отмена): задача остаётся для продолжения.
    Local,
    /// Отмена на сервере: передача прерывается, задача закрывается `finish(Cancelled)`.
    Cancelled,
    /// Задача потеряна (`stale_epoch`, `job_terminal`, токен отозван): сброс без `finish`.
    Lost,
}

/// Функция остановки передачи: спрашивается между кусками и во время пауз.
pub type StopFn<'a> = &'a dyn Fn() -> Option<StopWhy>;

/// Чем кончилась передача.
#[derive(Debug, Clone, PartialEq)]
pub enum TransferEnd<T = ()> {
    /// Передача завершена; для загрузки результата внутри состояние задачи после `finish`.
    Done(T),
    /// Остановлено локально или лимитом кусков; всё уже записанное остаётся для продолжения.
    Stopped,
    /// Отмена на сервере прервала передачу между кусками.
    Cancelled,
    /// Задача потеряна, пока шла передача.
    Lost,
    /// Окончательный отказ: причина для журнала. Для демки так отмечена демка, которая не сошлась по SHA-256.
    Dropped(String),
}

/// Размер и SHA-256 файла, прочитанного потоком.
pub fn sha256_file(path: &Path) -> std::io::Result<(u64, String)> {
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((total, hex(&hasher.finalize())))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
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

/// Окончание передачи по причине остановки.
fn ended<T>(why: StopWhy) -> TransferEnd<T> {
    match why {
        StopWhy::Local => TransferEnd::Stopped,
        StopWhy::Cancelled => TransferEnd::Cancelled,
        StopWhy::Lost => TransferEnd::Lost,
    }
}

/// Пауза с проверкой остановки каждые 200 мс; `Some` — передачу пора прервать.
fn nap(secs: u64, stop: StopFn) -> Option<StopWhy> {
    let until = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < until {
        if let Some(why) = stop() {
            return Some(why);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    stop()
}

/// Пауза между повторами: 1, 2, 4 … 30 с.
#[derive(Default)]
struct Backoff(u64);

impl Backoff {
    fn next(&mut self) -> u64 {
        self.0 = (self.0 * 2).clamp(1, 30);
        self.0
    }

    fn reset(&mut self) {
        self.0 = 0;
    }
}

/// Решение по отказу сервера.
pub(crate) enum Verdict {
    /// Окончательный отказ с причиной.
    Drop(String),
    /// Повторить после паузы.
    Retry,
}

/// Окончательные отказы: чужая эпоха, закрытая или неизвестная серверу задача. `HEAD` приходит без тела, поэтому
/// 409 без кода тоже считается отзывом выдачи. Всё остальное (сеть, 5xx, 401, перегрузка) — временное.
pub(crate) fn judge(e: &LinkError) -> Verdict {
    match e {
        LinkError::Status { error, status, .. }
            if error == STALE_EPOCH || (*status == 409 && error.is_empty()) =>
        {
            Verdict::Drop(format!(
                "сервер отозвал выдачу задачи или выдал её заново ({STALE_EPOCH})"
            ))
        }
        LinkError::Status { error, .. } if error == JOB_TERMINAL => {
            Verdict::Drop(format!("задача уже закрыта на сервере ({JOB_TERMINAL})"))
        }
        LinkError::Status { error, status, .. } if error == NOT_FOUND || *status == 404 => {
            Verdict::Drop(format!("сервер не знает такой задачи ({NOT_FOUND})"))
        }
        _ => Verdict::Retry,
    }
}

/// Скачивает демку задачи в `held.demo_path` через `.part` с докачкой, сверяет размер и SHA-256 с выдачей.
/// Несовпадение — один раз заново с нуля; второе несовпадение — `Dropped("демка повреждена при передаче")`.
/// Сбой связи или диска — `Err` (решает вызывающий: `.part` остаётся, следующая попытка продолжит).
pub fn download_demo(
    link: &Link,
    held: &HeldJob,
    stop: StopFn,
    log: &dyn Fn(&str),
) -> Result<TransferEnd, String> {
    let (job_id, epoch) = (held.server_job_id, held.epoch);
    let (size, sha256) = (held.demo_size, held.demo_sha256.as_str());
    let dest = held.demo_path.as_path();
    let part = dest.with_extension("dem.part");
    for attempt in 0..2 {
        let mut have = match fs::metadata(&part) {
            Ok(m) if m.len() <= size => m.len(),
            Ok(_) => {
                let _ = fs::remove_file(&part);
                0
            }
            Err(_) => 0,
        };
        let mut hasher = Sha256::new();
        if have > 0 {
            // Уже скачанное начало входит в хеш, чтобы не читать файл второй раз в конце.
            let mut f = File::open(&part)
                .map_err(|e| format!("не удалось открыть {}: {e}", part.display()))?;
            let mut buf = vec![0u8; 1024 * 1024];
            loop {
                let n = f
                    .read(&mut buf)
                    .map_err(|e| format!("чтение {}: {e}", part.display()))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&part)
            .map_err(|e| format!("не удалось открыть {}: {e}", part.display()))?;
        if have > 0 {
            log(&format!(
                "задача #{job_id}: докачиваю демку с {have} из {size} байт"
            ));
        } else {
            log(&format!("задача #{job_id}: скачиваю демку, {size} байт"));
        }
        while have < size {
            if let Some(why) = stop() {
                let _ = file.flush();
                return Ok(ended(why));
            }
            let mut sink = HashingFile {
                file: &mut file,
                hasher: &mut hasher,
            };
            let got = link
                .demo_range(job_id, epoch, have, &mut sink)
                .map_err(|e| format!("задача #{job_id}: скачивание демки: {e}"))?;
            if got == 0 {
                return Err(format!(
                    "задача #{job_id}: сервер прислал пустой кусок демки"
                ));
            }
            have += got;
        }
        file.flush().map_err(|e| format!("запись демки: {e}"))?;
        file.sync_all().map_err(|e| format!("запись демки: {e}"))?;
        drop(file);
        let sha = hex(&hasher.finalize());
        if have == size && size > 0 && sha.eq_ignore_ascii_case(sha256) {
            fs::rename(&part, dest).map_err(|e| format!("не удалось переименовать демку: {e}"))?;
            return Ok(TransferEnd::Done(()));
        }
        log(&format!(
            "задача #{job_id}: демка не сошлась: получено {have} из {size} байт"
        ));
        let _ = fs::remove_file(&part);
        if attempt == 1 {
            break;
        }
        log(&format!("задача #{job_id}: скачиваю демку заново с нуля"));
    }
    Ok(TransferEnd::Dropped(
        "демка повреждена при передаче".to_owned(),
    ))
}

/// Загружает результат кусками по `CHUNK_SIZE` со смещения, которое назвал сервер, и закрывает задачу `Done`.
///
/// `save` вызывается при каждом изменении `held` (счётчик повторов SHA-256). `chunk_limit` — после стольких
/// принятых кусков вернуть `Stopped` (учения «обрыв загрузки» и тесты). Временные ошибки повторяются без конца.
pub fn upload_result(
    link: &Link,
    held: &mut HeldJob,
    save: &mut dyn FnMut(&HeldJob),
    stop: StopFn,
    chunk_limit: Option<u32>,
    log: &dyn Fn(&str),
) -> TransferEnd<JobState> {
    let id = held.server_job_id;
    let epoch = held.epoch;
    let Some(result) = held.result.clone() else {
        return TransferEnd::Dropped("в состоянии нет записи о файле результата".to_owned());
    };
    let size = result.size;
    let mut file = match File::open(&result.path) {
        Ok(f) => f,
        Err(e) => {
            return TransferEnd::Dropped(format!(
                "файл результата {} не открывается: {e}",
                result.path.display()
            ));
        }
    };
    match file.metadata() {
        Ok(m) if m.len() == size => {}
        Ok(m) => {
            return TransferEnd::Dropped(format!(
                "файл результата изменился: {} байт вместо {size}",
                m.len()
            ));
        }
        Err(e) => return TransferEnd::Dropped(format!("размер файла результата не прочитан: {e}")),
    }

    let mut backoff = Backoff::default();
    let mut chunks = 0u32;
    let mut chunk = vec![0u8; CHUNK_SIZE as usize];
    'attempt: loop {
        if let Some(why) = stop() {
            return ended(why);
        }
        let mut offset = match link.result_head(id, epoch) {
            Ok(o) => o,
            Err(e) => match judge(&e) {
                Verdict::Drop(reason) => return TransferEnd::Dropped(reason),
                Verdict::Retry => {
                    let pause = backoff.next();
                    log(&format!(
                        "задача #{id}: HEAD результата: {e}; повтор через {pause} с"
                    ));
                    if let Some(why) = nap(pause, stop) {
                        return ended(why);
                    }
                    continue 'attempt;
                }
            },
        };
        if offset > size {
            return TransferEnd::Dropped(format!(
                "сервер принял {offset} байт, а в результате {size}"
            ));
        }
        log(&format!(
            "задача #{id}: загружаю результат, {size} байт, с {offset}"
        ));

        while offset < size {
            if let Some(why) = stop() {
                return ended(why);
            }
            let len = (size - offset).min(CHUNK_SIZE) as usize;
            if let Err(e) = file
                .seek(SeekFrom::Start(offset))
                .and_then(|_| file.read_exact(&mut chunk[..len]))
            {
                return TransferEnd::Dropped(format!("файл результата не читается: {e}"));
            }
            match link.result_put(id, epoch, offset, &chunk[..len]) {
                Ok(next) => {
                    backoff.reset();
                    chunks += 1;
                    if next > size {
                        return TransferEnd::Dropped(format!(
                            "сервер принял {next} байт, а в результате {size}"
                        ));
                    }
                    offset = next;
                    if offset < size && chunk_limit.is_some_and(|n| chunks >= n) {
                        return TransferEnd::Stopped;
                    }
                }
                Err(LinkError::Status {
                    error,
                    offset: Some(server),
                    ..
                }) if error == OFFSET_MISMATCH => {
                    if server > size {
                        return TransferEnd::Dropped(format!(
                            "сервер принял {server} байт, а в результате {size}"
                        ));
                    }
                    log(&format!(
                        "задача #{id}: сервер принял {server} байт, продолжаю с них"
                    ));
                    offset = server;
                }
                Err(LinkError::Status {
                    error, retry_after, ..
                }) if error == UPLOAD_BUSY => {
                    let pause = retry_after.unwrap_or(1).clamp(1, 30);
                    log(&format!(
                        "задача #{id}: сервер ещё принимает прошлый кусок; повтор через {pause} с"
                    ));
                    if let Some(why) = nap(pause, stop) {
                        return ended(why);
                    }
                }
                Err(e) => match judge(&e) {
                    Verdict::Drop(reason) => return TransferEnd::Dropped(reason),
                    Verdict::Retry => {
                        let pause = backoff.next();
                        log(&format!(
                            "задача #{id}: загрузка результата: {e}; повтор через {pause} с"
                        ));
                        if let Some(why) = nap(pause, stop) {
                            return ended(why);
                        }
                        // Смещение могло сдвинуться, пока шёл оборванный запрос: спрашиваем сервер заново.
                        continue 'attempt;
                    }
                },
            }
        }

        let req = FinishRequest {
            epoch,
            outcome: Outcome::Done {
                size,
                sha256: result.sha256.clone(),
            },
        };
        match link.finish(id, &req) {
            Ok(reply) => return TransferEnd::Done(reply.state),
            Err(LinkError::Status { error, .. }) if error == SHA256_MISMATCH => {
                if held.sha_retries == 0 {
                    held.sha_retries = 1;
                    save(held);
                    log(&format!(
                        "задача #{id}: сервер не принял sha256 результата; загружаю заново с нуля"
                    ));
                    continue 'attempt;
                }
                return TransferEnd::Dropped(
                    "сервер дважды не принял sha256 результата (sha256_mismatch)".to_owned(),
                );
            }
            // Файл на сервере короче заявленного: HEAD в начале цикла назовёт, с какого места догружать.
            Err(LinkError::Status { error, .. }) if error == OFFSET_MISMATCH => continue 'attempt,
            Err(e) => match judge(&e) {
                Verdict::Drop(reason) => return TransferEnd::Dropped(reason),
                Verdict::Retry => {
                    let pause = backoff.next();
                    log(&format!(
                        "задача #{id}: finish: {e}; повтор через {pause} с"
                    ));
                    if let Some(why) = nap(pause, stop) {
                        return ended(why);
                    }
                    continue 'attempt;
                }
            },
        }
    }
}

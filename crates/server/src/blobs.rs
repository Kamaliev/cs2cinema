//! Хранилище файлов: демки и результаты лежат под папкой данных.
//!
//! Пути строятся только из целых идентификаторов задач; имя файла пользователя в путь не попадает
//! (оно хранится в базе как данные для показа).

use crate::core::FileRow;
use crate::{Result, hex};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Take, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const COPY_BUF: usize = 1024 * 1024;
/// Буфер дописывания куска результата: 64 КиБ.
const CHUNK_BUF: usize = 64 * 1024;

#[derive(Clone)]
pub struct Blobs {
    root: PathBuf,
    /// Загрузки, которые идут сейчас: пара `(задача, эпоха)` на каждую. Общий для всех клонов `Blobs`.
    uploads: Arc<Mutex<HashSet<(i64, i64)>>>,
}

/// Итог приёма файла.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingested {
    pub size: u64,
    pub sha256: String,
}

/// Путь демки задачи относительно папки данных (так он хранится в `files.rel_path`).
pub fn demo_rel(job_id: i64) -> String {
    format!("blobs/incoming/{job_id}/demo")
}

/// Путь результата задачи относительно папки данных (так он хранится в `files.rel_path`).
pub fn result_rel(job_id: i64) -> String {
    format!("blobs/results/{job_id}/highlights.mp4")
}

/// Охрана загрузки: пока она жива, другой кусок той же пары `(задача, эпоха)` не принимается.
/// Снимается в `Drop`, в том числе при панике обработчика.
pub struct UploadGuard {
    uploads: Arc<Mutex<HashSet<(i64, i64)>>>,
    key: (i64, i64),
}

impl UploadGuard {
    pub fn job_id(&self) -> i64 {
        self.key.0
    }

    pub fn epoch(&self) -> i64 {
        self.key.1
    }
}

impl Drop for UploadGuard {
    fn drop(&mut self) {
        self.uploads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}

/// Итог дописывания куска: сколько байт записано на диск и чем закончилось чтение тела.
/// Принятые байты остаются на диске и при ошибке: загрузка продолжится с настоящего смещения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Appended {
    pub got: u64,
    pub error: Option<String>,
}

/// Итог проверки готового результата.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finalized {
    /// Размер и SHA-256 совпали, файл переименован в `highlights.mp4`.
    Ok,
    /// Длина `.part` не равна заявленному размеру.
    SizeMismatch { actual: u64 },
    /// SHA-256 не совпал, `.part` удалён.
    ShaMismatch,
}

/// Проверка по первым байтам: демка CS2 (`PBDEMS2\0`) или zstd-архив.
pub fn sniff_demo(path: &Path) -> Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut head = [0u8; 8];
    let mut got = 0;
    while got < head.len() {
        match f.read(&mut head[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    let head = &head[..got];
    if head.starts_with(b"PBDEMS2\0") || head.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        Ok(())
    } else {
        Err(format!(
            "{name}: не похоже на демку CS2 (.dem или .dem.zst)"
        ))
    }
}

impl Blobs {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            uploads: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn demo_path(&self, job_id: i64) -> PathBuf {
        self.root
            .join("incoming")
            .join(job_id.to_string())
            .join("demo")
    }

    /// Папка результата задачи: `{root}/results/{id}`.
    pub fn results_dir(&self, job_id: i64) -> PathBuf {
        self.root.join("results").join(job_id.to_string())
    }

    /// Путь недозагруженного результата: своя `.part` у каждой эпохи, чужая эпоха её не портит.
    pub fn result_part(&self, job_id: i64, epoch: i64) -> PathBuf {
        self.results_dir(job_id).join(format!("{epoch}.part"))
    }

    /// Путь готового результата задачи.
    pub fn result_path(&self, job_id: i64) -> PathBuf {
        self.results_dir(job_id).join("highlights.mp4")
    }

    /// Сколько байт результата уже лежит на диске: длина `.part` — единственная правда о смещении.
    pub fn part_len(&self, job_id: i64, epoch: i64) -> std::io::Result<u64> {
        match std::fs::metadata(self.result_part(job_id, epoch)) {
            Ok(m) => Ok(m.len()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// Занимает загрузку `(задача, эпоха)`; уже занята — `None`.
    pub fn lock_upload(&self, job_id: i64, epoch: i64) -> Option<UploadGuard> {
        let key = (job_id, epoch);
        let fresh = self
            .uploads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key);
        fresh.then(|| UploadGuard {
            uploads: self.uploads.clone(),
            key,
        })
    }

    /// Дописывает в `.part` до `len` байт из тела запроса. Длина файла заново сверяется со `offset` под охраной.
    /// Конец тела раньше `len` (обрыв выглядит как чистый EOF) и ошибка чтения не теряют записанное: вернётся
    /// `Appended { got, .. }`, а вызывающий сравнит `got` с `len`.
    pub fn append_chunk(
        &self,
        guard: &UploadGuard,
        offset: u64,
        len: u64,
        body: &mut dyn Read,
    ) -> Appended {
        let (job_id, epoch) = guard.key;
        let fail = |got: u64, e: String| Appended {
            got,
            error: Some(e),
        };
        let dir = self.results_dir(job_id);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return fail(0, format!("{}: {e}", dir.display()));
        }
        let part = self.result_part(job_id, epoch);
        let mut file = match OpenOptions::new().create(true).append(true).open(&part) {
            Ok(f) => f,
            Err(e) => return fail(0, format!("{}: {e}", part.display())),
        };
        match file.metadata() {
            Ok(m) if m.len() == offset => {}
            Ok(m) => {
                return fail(
                    0,
                    format!("длина {} байт, а не {offset}: файл изменился", m.len()),
                );
            }
            Err(e) => return fail(0, format!("{}: {e}", part.display())),
        }
        let mut buf = vec![0u8; CHUNK_BUF];
        let mut got = 0u64;
        let mut error = None;
        while got < len {
            let want = (len - got).min(CHUNK_BUF as u64) as usize;
            match body.read(&mut buf[..want]) {
                Ok(0) => break,
                Ok(n) => {
                    if let Err(e) = file.write_all(&buf[..n]) {
                        error = Some(format!("{}: {e}", part.display()));
                        break;
                    }
                    got += n as u64;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    error = Some(format!("чтение тела: {e}"));
                    break;
                }
            }
        }
        if let Err(e) = file.sync_data() {
            error.get_or_insert(format!("{}: {e}", part.display()));
        }
        Appended { got, error }
    }

    /// Проверяет `.part` эпохи и делает её результатом задачи: длина, затем SHA-256 потоком, затем `rename`.
    /// Неверный SHA-256 удаляет `.part` (догружать к испорченному файлу нечего). Базу при этом не держат.
    pub fn finalize_result(
        &self,
        job_id: i64,
        epoch: i64,
        size: u64,
        sha256: &str,
    ) -> Result<Finalized> {
        let part = self.result_part(job_id, epoch);
        let actual = self
            .part_len(job_id, epoch)
            .map_err(|e| format!("{}: {e}", part.display()))?;
        if actual != size {
            return Ok(Finalized::SizeMismatch { actual });
        }
        // Файл закрывается до `rename`: на Windows открытый файл не переименовать.
        let digest = {
            let mut f = File::open(&part).map_err(|e| format!("{}: {e}", part.display()))?;
            let mut hasher = Sha256::new();
            let mut buf = vec![0u8; COPY_BUF];
            loop {
                let n = f
                    .read(&mut buf)
                    .map_err(|e| format!("{}: {e}", part.display()))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            hex(&hasher.finalize())
        };
        if !digest.eq_ignore_ascii_case(sha256) {
            std::fs::remove_file(&part).map_err(|e| format!("{}: {e}", part.display()))?;
            return Ok(Finalized::ShaMismatch);
        }
        // `sync_all` на Windows требует права записи, поэтому файл открывается на запись (без усечения).
        OpenOptions::new()
            .write(true)
            .open(&part)
            .and_then(|f| f.sync_all())
            .map_err(|e| format!("{}: {e}", part.display()))?;
        let dst = self.result_path(job_id);
        // Остаток прежней неудачной попытки: на Windows `rename` поверх существующего файла не работает.
        match std::fs::remove_file(&dst) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", dst.display())),
        }
        std::fs::rename(&part, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        Ok(Finalized::Ok)
    }

    /// Удаляет готовый результат задачи (задачу отменили, пока считался хеш); нет файла — не ошибка.
    pub fn remove_result(&self, job_id: i64) -> std::io::Result<()> {
        match std::fs::remove_file(self.result_path(job_id)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Открывает кусок демки для отдачи: длина = `min(end, size - 1, start + CHUNK_SIZE - 1) - start + 1`.
    /// Вызывающий проверил `start < size` заранее. Возвращает читатель, длину куска и размер всего файла.
    pub fn open_demo_range(
        &self,
        job_id: i64,
        start: u64,
        end_incl: Option<u64>,
    ) -> std::io::Result<(Take<File>, u64, u64)> {
        use std::io::{Seek, SeekFrom};
        let mut f = File::open(self.demo_path(job_id))?;
        let size = f.metadata()?.len();
        let last = end_incl
            .unwrap_or(u64::MAX)
            .min(size.saturating_sub(1))
            .min(start.saturating_add(wire::CHUNK_SIZE - 1));
        let len = last.saturating_sub(start) + 1;
        f.seek(SeekFrom::Start(start))?;
        Ok((f.take(len), len, size))
    }

    /// Удаляет недозагруженные куски `*.part` результата задачи и возвращает их число; папки нет — `Ok(0)`.
    /// Готовые файлы (без `.part`) не трогает.
    pub fn discard_parts(&self, job_id: i64) -> std::io::Result<usize> {
        let dir = self.results_dir(job_id);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        let mut removed = 0;
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_some_and(|x| x == "part") && path.is_file() {
                std::fs::remove_file(&path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Копирует исходник в `demo.part` потоком, считая SHA-256, и переименовывает в `demo`.
    /// При любой ошибке `.part` удаляется. Базу при этом не держат.
    pub fn ingest_demo(&self, job_id: i64, src: &Path, max_bytes: u64) -> Result<Ingested> {
        let dst = self.demo_path(job_id);
        let dir = dst.parent().expect("путь демки всегда с родителем");
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let part = dir.join("demo.part");
        let out = copy_hashed(src, &part, max_bytes).and_then(|r| {
            std::fs::rename(&part, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
            Ok(r)
        });
        if out.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        out
    }

    /// Кладёт демку задачи `to_job` как жёсткую ссылку на демку задачи `from_job` — без повторной загрузки и
    /// без второй копии на диске. Если ссылку сделать нельзя (другая файловая система, нет прав), файл
    /// копируется с подсчётом SHA-256. В обоих случаях размер и хеш обязаны совпасть с `known` (строка `files`
    /// исходной демки), иначе новый файл удаляется и возвращается ошибка.
    ///
    /// У файла демки может быть несколько ссылок: уборщик (фаза 9) удаляет путь задачи, а не содержимое, и
    /// не должен считать место свободным, пока жива хотя бы одна ссылка.
    pub fn link_demo(&self, from_job: i64, to_job: i64, known: &FileRow) -> Result<Ingested> {
        let src = self.demo_path(from_job);
        let dst = self.demo_path(to_job);
        let dir = dst.parent().expect("путь демки всегда с родителем");
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        if std::fs::hard_link(&src, &dst).is_ok() {
            let size = std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(u64::MAX);
            if size == known.size {
                return Ok(Ingested {
                    size,
                    sha256: known.sha256.clone(),
                });
            }
            let _ = std::fs::remove_file(&dst);
            return Err(format!(
                "демка задачи #{from_job} на диске имеет другой размер, чем записано в базе"
            ));
        }
        let part = dir.join("demo.part");
        let got = copy_hashed(&src, &part, known.size).and_then(|r| {
            if r.sha256 != known.sha256 || r.size != known.size {
                return Err(format!(
                    "демка задачи #{from_job} на диске не совпадает с записанной в базе"
                ));
            }
            std::fs::rename(&part, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
            Ok(r)
        });
        if got.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        got
    }
}

fn copy_hashed(src: &Path, part: &Path, max_bytes: u64) -> Result<Ingested> {
    let mut input = File::open(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let mut output = File::create(part).map_err(|e| format!("{}: {e}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; COPY_BUF];
    let mut size = 0u64;
    loop {
        let n = input
            .read(&mut buf)
            .map_err(|e| format!("{}: {e}", src.display()))?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > max_bytes {
            return Err(format!(
                "файл больше лимита {} МиБ",
                max_bytes / (1024 * 1024)
            ));
        }
        hasher.update(&buf[..n]);
        output
            .write_all(&buf[..n])
            .map_err(|e| format!("{}: {e}", part.display()))?;
    }
    output
        .sync_all()
        .map_err(|e| format!("{}: {e}", part.display()))?;
    Ok(Ingested {
        size,
        sha256: hex(&hasher.finalize()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cs2srv-blobs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn sniff_accepts_demo_and_zstd_and_refuses_the_rest() {
        let d = dir("sniff");
        std::fs::write(d.join("a.dem"), b"PBDEMS2\0rest").unwrap();
        std::fs::write(d.join("b.zst"), [0x28, 0xB5, 0x2F, 0xFD, 1, 2]).unwrap();
        std::fs::write(d.join("c.txt"), b"hello").unwrap();
        std::fs::write(d.join("d.dem"), b"").unwrap();
        assert!(sniff_demo(&d.join("a.dem")).is_ok());
        assert!(sniff_demo(&d.join("b.zst")).is_ok());
        assert!(
            sniff_demo(&d.join("c.txt"))
                .unwrap_err()
                .contains("не похоже на демку CS2")
        );
        assert!(sniff_demo(&d.join("d.dem")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn discard_parts_removes_only_part_files() {
        let d = dir("parts");
        let blobs = Blobs::new(d.join("blobs"));
        assert_eq!(blobs.discard_parts(5).unwrap(), 0, "папки нет");
        let r = blobs.results_dir(5);
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(r.join("highlights.mp4.part"), b"a").unwrap();
        std::fs::write(r.join("plan.json.part"), b"b").unwrap();
        std::fs::write(r.join("highlights.mp4"), b"done").unwrap();
        assert_eq!(blobs.discard_parts(5).unwrap(), 2);
        assert!(r.join("highlights.mp4").exists());
        assert!(!r.join("plan.json.part").exists());
        assert_eq!(blobs.discard_parts(5).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn ingest_copies_hashes_and_leaves_no_part_file() {
        let d = dir("ingest");
        let src = d.join("src.dem");
        std::fs::write(&src, b"PBDEMS2\0abc").unwrap();
        let blobs = Blobs::new(d.join("blobs"));
        let got = blobs.ingest_demo(7, &src, 1024).unwrap();
        assert_eq!(got.size, 11);
        assert_eq!(got.sha256.len(), 64);
        assert_eq!(std::fs::read(blobs.demo_path(7)).unwrap(), b"PBDEMS2\0abc");
        assert!(!blobs.demo_path(7).with_file_name("demo.part").exists());
        let e = blobs.ingest_demo(8, &src, 5).unwrap_err();
        assert!(e.contains("больше лимита"), "{e}");
        assert!(
            !blobs.demo_path(8).exists()
                && !blobs.demo_path(8).with_file_name("demo.part").exists()
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    fn known_of(blobs: &Blobs, job: i64) -> FileRow {
        let bytes = std::fs::read(blobs.demo_path(job)).unwrap();
        FileRow {
            rel_path: demo_rel(job),
            size: bytes.len() as u64,
            sha256: hex(&Sha256::digest(&bytes)),
        }
    }

    #[test]
    fn link_demo_makes_a_hard_link_not_a_copy() {
        let d = dir("link");
        let src = d.join("src.dem");
        std::fs::write(&src, b"PBDEMS2\0abc").unwrap();
        let blobs = Blobs::new(d.join("blobs"));
        blobs.ingest_demo(1, &src, 1024).unwrap();
        let known = known_of(&blobs, 1);
        let got = blobs.link_demo(1, 2, &known).unwrap();
        assert_eq!((got.size, &got.sha256), (known.size, &known.sha256));
        assert_eq!(std::fs::read(blobs.demo_path(2)).unwrap(), b"PBDEMS2\0abc");
        // Одно содержимое под двумя именами: дописанный в исходник байт виден и по новому пути.
        let mut f = OpenOptions::new()
            .append(true)
            .open(blobs.demo_path(1))
            .unwrap();
        f.write_all(b"!").unwrap();
        drop(f);
        assert_eq!(std::fs::metadata(blobs.demo_path(2)).unwrap().len(), 12);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn link_demo_copies_when_the_link_cannot_be_made_and_checks_the_hash() {
        let d = dir("linkcopy");
        let src = d.join("src.dem");
        std::fs::write(&src, b"PBDEMS2\0abc").unwrap();
        let blobs = Blobs::new(d.join("blobs"));
        blobs.ingest_demo(1, &src, 1024).unwrap();
        let known = known_of(&blobs, 1);
        // Путь назначения занят старым файлом: ссылка не создаётся, файл копируется и заменяет его.
        std::fs::create_dir_all(blobs.demo_path(2).parent().unwrap()).unwrap();
        std::fs::write(blobs.demo_path(2), b"stale").unwrap();
        let got = blobs.link_demo(1, 2, &known).unwrap();
        assert_eq!(got.sha256, known.sha256);
        assert_eq!(std::fs::read(blobs.demo_path(2)).unwrap(), b"PBDEMS2\0abc");
        assert!(!blobs.demo_path(2).with_file_name("demo.part").exists());

        // Хеш в базе не сходится с файлом: копия не остаётся.
        let wrong = FileRow {
            sha256: "0".repeat(64),
            ..known.clone()
        };
        std::fs::create_dir_all(blobs.demo_path(3).parent().unwrap()).unwrap();
        std::fs::write(blobs.demo_path(3), b"stale").unwrap();
        let e = blobs.link_demo(1, 3, &wrong).unwrap_err();
        assert!(e.contains("не совпадает"), "{e}");
        assert!(!blobs.demo_path(3).with_file_name("demo.part").exists());

        // Размер в базе не сходится с файлом при жёсткой ссылке: ссылка убирается.
        let short = FileRow {
            size: 3,
            ..known.clone()
        };
        let e = blobs.link_demo(1, 4, &short).unwrap_err();
        assert!(e.contains("другой размер"), "{e}");
        assert!(!blobs.demo_path(4).exists());
        // Исходного файла нет совсем.
        assert!(blobs.link_demo(99, 5, &known).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn upload_guard_is_exclusive_per_job_and_epoch_and_released_on_drop() {
        let blobs = Blobs::new(dir("guard").join("blobs"));
        let a = blobs.lock_upload(1, 1).expect("первая охрана");
        assert!(blobs.lock_upload(1, 1).is_none(), "та же пара занята");
        assert!(blobs.lock_upload(1, 2).is_some(), "другая эпоха свободна");
        assert!(blobs.lock_upload(2, 1).is_some(), "другая задача свободна");
        drop(a);
        assert!(blobs.lock_upload(1, 1).is_some(), "охрана снята в Drop");
    }

    #[test]
    fn short_body_keeps_the_written_bytes_and_finalize_checks_size_then_sha() {
        let d = dir("append");
        let blobs = Blobs::new(d.join("blobs"));
        let guard = blobs.lock_upload(3, 1).unwrap();
        // Обрыв: заявлено 10 байт, пришло 4 и чистый EOF.
        let got = blobs.append_chunk(&guard, 0, 10, &mut &b"abcd"[..]);
        assert_eq!((got.got, got.error), (4, None));
        assert_eq!(blobs.part_len(3, 1).unwrap(), 4);
        // Смещение, не совпавшее с длиной файла под охраной, ничего не пишет.
        let bad = blobs.append_chunk(&guard, 0, 3, &mut &b"xyz"[..]);
        assert_eq!(bad.got, 0);
        assert!(bad.error.is_some());
        assert_eq!(blobs.part_len(3, 1).unwrap(), 4);
        let got = blobs.append_chunk(&guard, 4, 3, &mut &b"efg"[..]);
        assert_eq!((got.got, got.error), (3, None));
        drop(guard);

        assert_eq!(
            blobs.finalize_result(3, 1, 8, "00").unwrap(),
            Finalized::SizeMismatch { actual: 7 }
        );
        let wrong = hex(&Sha256::digest(b"other"));
        assert_eq!(
            blobs.finalize_result(3, 1, 7, &wrong).unwrap(),
            Finalized::ShaMismatch
        );
        assert!(!blobs.result_part(3, 1).exists(), "неверный .part удалён");
        let guard = blobs.lock_upload(3, 1).unwrap();
        blobs.append_chunk(&guard, 0, 7, &mut &b"abcdefg"[..]);
        let right = hex(&Sha256::digest(b"abcdefg"));
        assert_eq!(
            blobs.finalize_result(3, 1, 7, &right).unwrap(),
            Finalized::Ok
        );
        assert_eq!(std::fs::read(blobs.result_path(3)).unwrap(), b"abcdefg");
        assert!(!blobs.result_part(3, 1).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn demo_range_is_clamped_to_the_file_and_to_the_chunk_size() {
        let d = dir("range");
        let blobs = Blobs::new(d.join("blobs"));
        std::fs::create_dir_all(blobs.demo_path(4).parent().unwrap()).unwrap();
        std::fs::write(blobs.demo_path(4), vec![1u8; 100]).unwrap();
        let (mut r, len, size) = blobs.open_demo_range(4, 90, None).unwrap();
        let mut buf = Vec::new();
        r.read_to_end(&mut buf).unwrap();
        assert_eq!((len, size, buf.len()), (10, 100, 10));
        let (_, len, _) = blobs.open_demo_range(4, 0, Some(9)).unwrap();
        assert_eq!(len, 10);
        let (_, len, _) = blobs.open_demo_range(4, 0, Some(5000)).unwrap();
        assert_eq!(len, 100);
        let _ = std::fs::remove_dir_all(&d);
    }
}

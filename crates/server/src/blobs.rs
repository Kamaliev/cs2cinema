//! Хранилище файлов: демки и результаты лежат под папкой данных.
//!
//! Пути строятся только из целых идентификаторов задач; имя файла пользователя в путь не попадает
//! (оно хранится в базе как данные для показа).

use crate::{Result, hex};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const COPY_BUF: usize = 1024 * 1024;

#[derive(Clone)]
pub struct Blobs {
    root: PathBuf,
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
        Self { root }
    }

    pub fn demo_path(&self, job_id: i64) -> PathBuf {
        self.root
            .join("incoming")
            .join(job_id.to_string())
            .join("demo")
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
}

mod bits;
mod command;
pub mod events;
mod parse;
pub mod synth;
pub use parse::{Parsed, PlayerInfo, parse};

use std::{
    fmt,
    fs::File,
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::Path,
};
pub use command::{
    Command,
    MAX_COMMAND_UNPACKED,
    read_command
};

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/_.rs"));
}

const MIB: u64 = 1 << 20;

/// Потолки чтения демки: защита от огромных файлов и zstd/gzip-бомб.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DemoLimits {
    /// Максимальный размер файла на диске, байт.
    pub max_file: u64,
    /// Максимальный размер демки после распаковки, байт.
    pub max_unpacked: u64,
}

impl Default for DemoLimits {
    /// Стартовые числа (калибруются живым прогоном): файл 2000 МиБ, распакованная демка 3 ГиБ.
    fn default() -> Self {
        Self { max_file: 2000 * MIB, max_unpacked: 3 * 1024 * MIB }
    }
}

/// Отказ при ограниченном чтении демки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DemoError {
    /// Превышен потолок: `what` — что именно («файл», «распакованная демка»), `limit` — байт.
    TooLarge { what: &'static str, limit: u64 },
    /// Файл не читается или не похож на демку CS2.
    Bad(String),
}

impl fmt::Display for DemoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DemoError::TooLarge { what, limit } => {
                write!(f, "{what} больше лимита {} МиБ", limit / MIB)
            }
            DemoError::Bad(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for DemoError {}

fn bad_io(what: &str, e: io::Error) -> DemoError {
    DemoError::Bad(format!("{what}: {e}"))
}

/// Читает демку с потолками. zstd (`.dem.zst`) и gzip (`.dem.gz`) распаковываются потоком
/// с остановкой на `max_unpacked + 1` байтах, поэтому бомба не накапливается в памяти.
pub fn read_demo_limited<P: AsRef<Path>>(path: P, limits: &DemoLimits) -> Result<Vec<u8>, DemoError> {
    let file = File::open(path.as_ref()).map_err(|e| bad_io("не удалось открыть демку", e))?;
    let size = file.metadata().map_err(|e| bad_io("не удалось узнать размер демки", e))?.len();
    if size > limits.max_file {
        return Err(DemoError::TooLarge { what: "файл", limit: limits.max_file });
    }

    let mut reader = BufReader::new(file);
    let mut head = [0u8; 4];
    let mut got = 0;
    while got < head.len() {
        match reader.read(&mut head[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) => return Err(bad_io("не удалось прочитать демку", e)),
        }
    }
    reader.seek(SeekFrom::Start(0)).map_err(|e| bad_io("не удалось прочитать демку", e))?;
    let head = &head[..got];

    let cap = limits.max_unpacked.saturating_add(1);
    let mut out = Vec::new();
    if head.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        let decoder = zstd::stream::Decoder::new(reader)
            .map_err(|e| bad_io("не удалось распаковать zstd", e))?;
        decoder.take(cap).read_to_end(&mut out).map_err(|e| bad_io("не удалось распаковать zstd", e))?;
    } else if head.starts_with(&[0x1F, 0x8B]) {
        flate2::read::GzDecoder::new(reader)
            .take(cap)
            .read_to_end(&mut out)
            .map_err(|e| bad_io("не удалось распаковать gzip", e))?;
    } else {
        if size > limits.max_unpacked {
            return Err(DemoError::TooLarge { what: "распакованная демка", limit: limits.max_unpacked });
        }
        reader.take(cap).read_to_end(&mut out).map_err(|e| bad_io("не удалось прочитать демку", e))?;
    }

    if out.len() as u64 > limits.max_unpacked {
        return Err(DemoError::TooLarge { what: "распакованная демка", limit: limits.max_unpacked });
    }
    if !out.starts_with(b"PBDEMS2\0") {
        let lfs = out.starts_with(b"version https://git-lfs");
        return Err(DemoError::Bad(if lfs {
            "файл не похож на демку CS2: это LFS-указатель, выполните git lfs pull".to_owned()
        } else {
            "файл не похож на демку CS2 (нет сигнатуры PBDEMS2)".to_owned()
        }));
    }
    Ok(out)
}



/// Читает демку; zstd (`.dem.zst`, FACEIT CS2) и gzip (`.dem.gz`) распознаются по сигнатуре.
pub fn read_demo<P: AsRef<Path>>(path: P) -> io::Result<Vec<u8>>{
    let mut file = File::open(path)?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;
    decompress(contents)
}

pub fn decompress(contents: Vec<u8>) -> io::Result<Vec<u8>> {
    if contents.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        zstd::stream::decode_all(contents.as_slice())
    } else if contents.starts_with(&[0x1F, 0x8B]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(contents.as_slice()).read_to_end(&mut out)?;
        Ok(out)
    } else {
        Ok(contents)
    }
}



pub fn read_varint(data: &[u8], pos: &mut usize) -> io::Result<u32>{
    let mut value: u32 = 0;

    for shift in (0..35).step_by(7) {
        let byte = *data.get(*pos).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        *pos += 1;
        if shift == 28 && byte > 0x0f {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid varint so big"));
        }
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }

    Err(io::Error::new(io::ErrorKind::InvalidInput, "varint слишком длинный"))
}
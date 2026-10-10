//! Файлы папки задачи для проверенных дублей и повтора: манифест дублей (`clips/manifest.json`),
//! вход повтора (`record.json`) и снимок окружения (`env.json`).
//!
//! Содержимое всех этих файлов — недоверенный ввод (D-05, D-08): папка задачи доступна
//! пользователю и сторонним программам. Поэтому при чтении проверяются версия, имена папок дублей
//! (`take` и четыре цифры) и пути, а испорченный файл даёт «нет данных», а не панику. Запись
//! атомарна: во временный файл и `rename`.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use renderer::TakePlan;
use serde::{Deserialize, Serialize};

use crate::record::verify::TakeFacts;

/// Манифест дублей, лежит в `clips/`.
pub const MANIFEST_FILE: &str = "manifest.json";
/// Вход повтора, лежит в папке задачи.
pub const RECORD_FILE: &str = "record.json";
/// Снимок окружения последней попытки, лежит в папке задачи.
pub const ENV_FILE: &str = "env.json";

/// Версия формата `manifest.json` и `record.json`.
const VERSION: u32 = 1;

/// Потолок размера читаемого JSON-файла: настоящий манифест на сотню дублей — единицы килобайт.
const FILE_MAX: u64 = 4 * 1024 * 1024;

/// Имя папки дубля: ровно `take` и четыре цифры. Всё прочее (`../x`, `take01`) в путь не попадает.
pub fn valid_take_dir(name: &str) -> bool {
    crate::record::take_number(name).is_some()
}

/// Статус дубля в манифесте.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TakeStatus {
    /// Ещё не записан.
    #[default]
    Pending,
    /// Файлы лежат на диске, проверка ещё не прошла.
    Recorded,
    /// Проверен ffprobe и признан годным.
    Validated,
    /// Проверен и признан негодным (причина в `error`).
    Invalid,
}

/// Один дубль в манифесте.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TakeEntry {
    pub index: u32,
    /// Папка дубля внутри `clips/`: `take0000`.
    #[serde(default)]
    pub dir: String,
    /// Ожидаемая длительность (`TakePlan::game_secs`), секунды.
    #[serde(default)]
    pub expected_secs: f32,
    #[serde(default)]
    pub capture_fps: u32,
    #[serde(default)]
    pub status: TakeStatus,
    #[serde(default)]
    pub video_bytes: Option<u64>,
    #[serde(default)]
    pub audio_bytes: Option<u64>,
    /// Измеренная длительность `video.mp4`, секунды.
    #[serde(default)]
    pub video_secs: Option<f32>,
    /// Измеренная длительность `audio.wav`, секунды.
    #[serde(default)]
    pub audio_secs: Option<f32>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// Почему дубль негоден (или почему перестал считаться проверенным).
    #[serde(default)]
    pub error: Option<String>,
    /// Номер попытки, на которой дубль получил последний статус.
    #[serde(default)]
    pub attempt: u32,
}

/// Снимок окружения попытки записи (OBS-03): что стояло на машине, когда дубли писались.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EnvSnapshot {
    #[serde(default)]
    pub cs2_build: Option<String>,
    #[serde(default)]
    pub hlae_version: Option<String>,
    #[serde(default)]
    pub ffmpeg: Option<String>,
    #[serde(default)]
    pub ffprobe: Option<String>,
    #[serde(default)]
    pub hlae_ffmpeg: Option<String>,
    /// Склейка идёт на NVENC.
    #[serde(default)]
    pub nvenc_assemble: bool,
    /// Дубли пишутся на NVENC.
    #[serde(default)]
    pub nvenc_takes: bool,
    /// Свободно на диске результатов, байт.
    #[serde(default)]
    pub free_results: Option<u64>,
    /// Свободно на диске CS2, байт.
    #[serde(default)]
    pub free_cs2: Option<u64>,
    #[serde(default)]
    pub frame: (u32, u32),
    #[serde(default)]
    pub fps: u32,
    /// Когда снят снимок (секунды Unix).
    #[serde(default)]
    pub taken_unix: i64,
}

/// Одна попытка записи.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AttemptEntry {
    pub n: u32,
    #[serde(default)]
    pub started_unix: i64,
    #[serde(default)]
    pub env: Option<EnvSnapshot>,
    /// Чем кончилась попытка; `None` — ещё идёт или оборвана падением.
    #[serde(default)]
    pub outcome: Option<String>,
}

/// Манифест дублей: источник истины о том, какие дубли записаны и проверены (TAK-03).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    /// Хеш скриптов плана: другой план — другие дубли.
    #[serde(default)]
    pub plan_hash: String,
    #[serde(default)]
    pub nonce: String,
    /// Заданный размер кадра записи.
    #[serde(default)]
    pub frame: (u32, u32),
    #[serde(default)]
    pub takes: Vec<TakeEntry>,
    #[serde(default)]
    pub attempts: Vec<AttemptEntry>,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Записать файл атомарно: во временный `<имя>.tmp` рядом и `rename`. Если `rename` поверх
/// существующего файла не удался, старый удаляется и `rename` повторяется.
pub(crate) fn atomic_write(path: &Path, text: &str) -> Result<(), String> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(path);
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

/// Прочитать файл не больше `FILE_MAX` байт.
pub(crate) fn read_limited(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.len() > FILE_MAX {
        return Err(format!("{}: файл слишком большой", path.display()));
    }
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

impl Manifest {
    /// Новый манифест: все дубли `pending`.
    pub fn new(
        plan_hash: impl Into<String>,
        nonce: impl Into<String>,
        frame: (u32, u32),
        takes: &[TakePlan],
    ) -> Self {
        Manifest {
            version: VERSION,
            plan_hash: plan_hash.into(),
            nonce: nonce.into(),
            frame,
            takes: takes
                .iter()
                .map(|t| TakeEntry {
                    index: t.index,
                    dir: t.dir_name(),
                    expected_secs: t.game_secs,
                    capture_fps: t.capture_fps,
                    ..TakeEntry::default()
                })
                .collect(),
            attempts: vec![],
        }
    }

    /// Прочитать `clips/manifest.json`. Нет файла, мусор, другая версия или папка дубля не вида
    /// `take` + 4 цифры — `None`: дубли считаются непроверенными.
    pub fn load(clips: &Path) -> Option<Self> {
        let text = read_limited(&clips.join(MANIFEST_FILE)).ok()?;
        let m: Manifest = serde_json::from_str(&text).ok()?;
        (m.version == VERSION && m.takes.iter().all(|t| valid_take_dir(&t.dir))).then_some(m)
    }

    /// Записать манифест атомарно.
    pub fn save(&self, clips: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        atomic_write(&clips.join(MANIFEST_FILE), &text)
    }

    pub fn entry(&self, index: u32) -> Option<&TakeEntry> {
        self.takes.iter().find(|t| t.index == index)
    }

    fn entry_mut(&mut self, index: u32) -> Option<&mut TakeEntry> {
        self.takes.iter_mut().find(|t| t.index == index)
    }

    /// Файлы дубля легли на диск: размеры в байтах (видео, звук).
    pub fn mark_recorded(&mut self, index: u32, sizes: (u64, u64)) {
        if let Some(t) = self.entry_mut(index) {
            t.status = TakeStatus::Recorded;
            t.video_bytes = Some(sizes.0);
            t.audio_bytes = Some(sizes.1);
            t.error = None;
        }
    }

    /// Дубль прошёл проверку.
    pub fn mark_validated(&mut self, index: u32, f: &TakeFacts, attempt: u32) {
        if let Some(t) = self.entry_mut(index) {
            t.status = TakeStatus::Validated;
            t.video_bytes = Some(f.video_bytes);
            t.audio_bytes = Some(f.audio_bytes);
            t.video_secs = Some(f.video_secs);
            t.audio_secs = Some(f.audio_secs);
            t.width = Some(f.width);
            t.height = Some(f.height);
            t.error = None;
            t.attempt = attempt;
        }
    }

    /// Дубль не прошёл проверку.
    pub fn mark_invalid(&mut self, index: u32, reason: &str, attempt: u32) {
        if let Some(t) = self.entry_mut(index) {
            t.status = TakeStatus::Invalid;
            t.error = Some(reason.to_owned());
            t.attempt = attempt;
        }
    }

    /// Открыть попытку `n` со снимком окружения.
    pub fn begin_attempt(&mut self, n: u32, env: EnvSnapshot) {
        self.attempts.push(AttemptEntry {
            n,
            started_unix: now_unix(),
            env: Some(env),
            outcome: None,
        });
    }

    /// Закрыть последнюю попытку: чем она кончилась.
    pub fn end_attempt(&mut self, outcome: &str) {
        if let Some(a) = self.attempts.last_mut() {
            a.outcome = Some(outcome.to_owned());
        }
    }

    /// Номера дублей, которые не `validated`.
    pub fn missing(&self) -> Vec<u32> {
        self.takes
            .iter()
            .filter(|t| t.status != TakeStatus::Validated)
            .map(|t| t.index)
            .collect()
    }

    /// Все дубли проверены (и дубли вообще есть).
    pub fn all_validated(&self) -> bool {
        !self.takes.is_empty() && self.missing().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plans(n: u32) -> Vec<TakePlan> {
        (0..n)
            .map(|i| TakePlan {
                index: i,
                shot: i as usize,
                game_secs: 5.0,
                capture_fps: 60,
            })
            .collect()
    }

    fn facts() -> TakeFacts {
        TakeFacts {
            video_secs: 5.02,
            audio_secs: 5.0,
            width: 1280,
            height: 720,
            video_bytes: 123,
            audio_bytes: 456,
        }
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("cs2cinema-manifest-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn validated_take_survives_a_save_and_load_round_trip() {
        let clips = temp("roundtrip").join("clips");
        let mut m = Manifest::new("abc", "ab12cd34", (1280, 720), &plans(2));
        assert!(m.takes.iter().all(|t| t.status == TakeStatus::Pending));
        assert_eq!(m.takes[1].dir, "take0001");
        m.begin_attempt(1, EnvSnapshot::default());
        m.mark_recorded(0, (123, 456));
        m.mark_validated(0, &facts(), 1);
        m.end_attempt("ok");
        m.save(&clips).unwrap();
        assert!(
            !clips.join("manifest.json.tmp").exists(),
            "временный файл переименован"
        );
        let back = Manifest::load(&clips).expect("манифест читается");
        assert_eq!(back, m);
        assert_eq!(back.entry(0).unwrap().status, TakeStatus::Validated);
        assert_eq!(back.entry(0).unwrap().width, Some(1280));
        // повторная запись поверх существующего файла
        m.mark_invalid(1, "video.mp4: нет видеопотока", 1);
        m.save(&clips).unwrap();
        assert_eq!(
            Manifest::load(&clips).unwrap().entry(1).unwrap().status,
            TakeStatus::Invalid
        );
    }

    #[test]
    fn garbage_and_foreign_dirs_are_rejected_on_load() {
        let clips = temp("garbage").join("clips");
        assert!(Manifest::load(&clips).is_none(), "нет файла");
        std::fs::create_dir_all(&clips).unwrap();
        std::fs::write(clips.join(MANIFEST_FILE), "это не json").unwrap();
        assert!(Manifest::load(&clips).is_none(), "мусор");

        let good = Manifest::new("h", "n", (1280, 720), &plans(1));
        let write = |m: &Manifest| {
            std::fs::write(clips.join(MANIFEST_FILE), serde_json::to_string(m).unwrap()).unwrap()
        };
        write(&good);
        assert!(Manifest::load(&clips).is_some());

        let mut v2 = good.clone();
        v2.version = 2;
        write(&v2);
        assert!(Manifest::load(&clips).is_none(), "другая версия");

        for bad in ["../x", "take01", "take0000/..", "TAKE0000", ""] {
            let mut m = good.clone();
            m.takes[0].dir = bad.to_owned();
            write(&m);
            assert!(
                Manifest::load(&clips).is_none(),
                "папка {bad:?} недопустима"
            );
        }
        assert!(valid_take_dir("take0042"));
        assert!(!valid_take_dir("take00420"));
    }

    #[test]
    fn all_validated_needs_every_take() {
        let mut m = Manifest::new("h", "n", (1280, 720), &plans(2));
        assert!(!m.all_validated());
        assert_eq!(m.missing(), vec![0, 1]);
        m.mark_validated(0, &facts(), 1);
        assert!(!m.all_validated(), "второй дубль ещё не проверен");
        assert_eq!(m.missing(), vec![1]);
        m.mark_validated(1, &facts(), 1);
        assert!(m.all_validated());
        assert!(
            !Manifest::new("h", "n", (1280, 720), &[]).all_validated(),
            "пустой манифест не готов"
        );
    }
}

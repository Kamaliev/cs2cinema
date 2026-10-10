//! Файлы папки задачи для проверенных дублей и повтора: манифест дублей (`clips/manifest.json`),
//! вход повтора (`record.json`) и снимок окружения (`env.json`).
//!
//! Содержимое всех этих файлов — недоверенный ввод (D-05, D-08): папка задачи доступна
//! пользователю и сторонним программам. Поэтому при чтении проверяются версия, имена папок дублей
//! (`take` и четыре цифры) и пути, а испорченный файл даёт «нет данных», а не панику. Запись
//! атомарна: во временный файл и `rename`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use renderer::TakePlan;
use serde::{Deserialize, Serialize};

use crate::precheck::Report;
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

impl Manifest {
    /// Сверить `validated` дубли с файлами на диске: пропавший файл или другой размер понижает дубль
    /// до `recorded` (его надо проверить заново). Возвращает строки для журнала.
    pub fn reconcile(&mut self, clips: &Path) -> Vec<String> {
        let mut lines = vec![];
        for t in &mut self.takes {
            if t.status != TakeStatus::Validated {
                continue;
            }
            let size = |file: &str| -> Option<u64> {
                if !valid_take_dir(&t.dir) {
                    return None;
                }
                let meta = std::fs::metadata(clips.join(&t.dir).join(file)).ok()?;
                meta.is_file().then_some(meta.len())
            };
            let same = size(TakePlan::FILES[0]).is_some_and(|n| Some(n) == t.video_bytes)
                && size(TakePlan::FILES[1]).is_some_and(|n| Some(n) == t.audio_bytes);
            if !same {
                t.status = TakeStatus::Recorded;
                t.error = Some("файл изменился или пропал после проверки".to_owned());
                lines.push(format!(
                    "{}: файл изменился или пропал после проверки — дубль надо проверить заново",
                    t.dir
                ));
            }
        }
        lines
    }
}

impl EnvSnapshot {
    /// Снимок из отчёта предпроверки: сборка CS2, версии утилит, свободное место.
    pub fn from_report(
        r: &Report,
        frame: (u32, u32),
        fps: u32,
        nvenc_assemble: bool,
        nvenc_takes: bool,
    ) -> Self {
        EnvSnapshot {
            cs2_build: r.cs2_build.clone(),
            hlae_version: r.hlae_version.clone(),
            ffmpeg: r.ffmpeg_version.clone(),
            ffprobe: r.ffprobe_version.clone(),
            hlae_ffmpeg: r.hlae_ffmpeg_version.clone(),
            nvenc_assemble,
            nvenc_takes,
            free_results: r.free_results,
            free_cs2: r.free_cs2,
            frame,
            fps,
            taken_unix: now_unix(),
        }
    }

    /// Записать `env.json` в папку задачи атомарно.
    pub fn save(&self, out_dir: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        atomic_write(&out_dir.join(ENV_FILE), &text)
    }

    /// Строка для журнала записи.
    pub fn one_line(&self) -> String {
        let text = |v: &Option<String>| v.clone().unwrap_or_else(|| "?".to_owned());
        let gb = |v: Option<u64>| match v {
            Some(b) => format!("{:.1}", b as f64 / (1u64 << 30) as f64),
            None => "?".to_owned(),
        };
        let yn = |b: bool| if b { "да" } else { "нет" };
        let line = format!(
            "окружение: CS2 {}, HLAE {}, ffmpeg {}, ffprobe {}, ffmpeg HLAE {}, NVENC склейки/дублей {}/{}, свободно {} ГБ / {} ГБ, кадр {}x{}",
            text(&self.cs2_build),
            text(&self.hlae_version),
            text(&self.ffmpeg),
            text(&self.ffprobe),
            text(&self.hlae_ffmpeg),
            yn(self.nvenc_assemble),
            yn(self.nvenc_takes),
            gb(self.free_results),
            gb(self.free_cs2),
            self.frame.0,
            self.frame.1,
        );
        crate::record::log::one_line(&line)
    }
}

/// Дубль в `record.json`: то, что нужно, чтобы пересоздать [`TakePlan`] без плана.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TakeSpec {
    pub index: u32,
    pub shot: usize,
    pub game_secs: f32,
    pub capture_fps: u32,
}

impl From<&TakePlan> for TakeSpec {
    fn from(t: &TakePlan) -> Self {
        TakeSpec {
            index: t.index,
            shot: t.shot,
            game_secs: t.game_secs,
            capture_fps: t.capture_fps,
        }
    }
}

impl TakeSpec {
    pub fn to_plan(&self) -> TakePlan {
        TakePlan {
            index: self.index,
            shot: self.shot,
            game_secs: self.game_secs,
            capture_fps: self.capture_fps,
        }
    }
}

/// Вход «Повторить» без скачивания и плана (D-08): всё, что запись и склейка берут из плана.
/// Лежит в папке задачи рядом со скриптами. Содержимое недоверенное: [`RecordJson::load`]
/// проверяет его целиком.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordJson {
    pub version: u32,
    /// Метка задачи в маркерах консоли (8 строчных hex-символов).
    pub nonce: String,
    /// Где лежала демка на момент планирования (повтор проверяет её сам).
    pub demo_path: PathBuf,
    /// Имя демки в папке игры без `.dem` (то, что получает `playdemo`).
    pub demo_name: String,
    /// Размер распакованной демки, байт (для оценки места).
    #[serde(default)]
    pub demo_len: u64,
    /// Размер кадра, с которым планировались скрипты; повтор пишет именно в нём.
    pub frame: (u32, u32),
    pub fps: u32,
    /// Дубли пишутся на NVENC.
    #[serde(default)]
    pub take_nvenc: bool,
    pub takes: Vec<TakeSpec>,
    /// Готовые аргументы ffmpeg для склейки (запускать в папке задачи).
    pub ffmpeg_args: Vec<String>,
    /// Длина ролика, секунды.
    #[serde(default)]
    pub video_secs: f32,
    /// Сколько шотов в плане.
    #[serde(default)]
    pub shots: usize,
    /// Суммарная игровая длительность дублей, секунды.
    #[serde(default)]
    pub secs: f32,
    /// Хеш скриптов плана (нужен, чтобы не взять чужой манифест).
    #[serde(default)]
    pub plan_hash: String,
    #[serde(default)]
    pub created_unix: i64,
}

/// Допустимый размер кадра записи по каждой стороне.
const FRAME_RANGE: std::ops::RangeInclusive<u32> = 16..=7680;
/// Имя готового ролика — последний аргумент склейки.
const OUTPUT_NAME: &str = "highlights.mp4";

impl RecordJson {
    pub fn path(out_dir: &Path) -> PathBuf {
        out_dir.join(RECORD_FILE)
    }

    /// Записать `record.json` атомарно.
    pub fn save(&self, out_dir: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        atomic_write(&Self::path(out_dir), &text)
    }

    /// Удалить `record.json` (устаревший вход повтора); отсутствие файла не ошибка.
    pub fn remove(out_dir: &Path) -> Result<(), String> {
        match std::fs::remove_file(Self::path(out_dir)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", Self::path(out_dir).display())),
        }
    }

    /// Прочитать и проверить `record.json`; причина отказа — по-русски.
    pub fn load(out_dir: &Path) -> Result<Self, String> {
        let path = Self::path(out_dir);
        if !path.exists() {
            return Err(format!("{RECORD_FILE} нет"));
        }
        let text = read_limited(&path)?;
        let rj: RecordJson =
            serde_json::from_str(&text).map_err(|e| format!("{RECORD_FILE} испорчен: {e}"))?;
        rj.check()?;
        Ok(rj)
    }

    /// Проверки недоверенного содержимого (T-03-07).
    fn check(&self) -> Result<(), String> {
        if self.version != VERSION {
            return Err(format!(
                "{RECORD_FILE}: версия {} не поддерживается (нужна {VERSION})",
                self.version
            ));
        }
        let hex = self.nonce.len() == 8
            && self
                .nonce
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hex {
            return Err(format!(
                "{RECORD_FILE}: метка задачи не из 8 строчных hex-символов"
            ));
        }
        let name = &self.demo_name;
        if !renderer::console_safe(name)
            || name.contains(['/', '\\'])
            || name.contains("..")
            || name.contains(':')
        {
            return Err(format!("{RECORD_FILE}: недопустимое имя демки"));
        }
        if self.demo_path.as_os_str().is_empty() {
            return Err(format!("{RECORD_FILE}: не указан путь к демке"));
        }
        if !FRAME_RANGE.contains(&self.frame.0) || !FRAME_RANGE.contains(&self.frame.1) {
            return Err(format!(
                "{RECORD_FILE}: размер кадра {}x{} вне допустимого",
                self.frame.0, self.frame.1
            ));
        }
        if !(1..=1000).contains(&self.fps) {
            return Err(format!("{RECORD_FILE}: fps {} вне допустимого", self.fps));
        }
        if self.takes.is_empty() {
            return Err(format!("{RECORD_FILE}: нет ни одного дубля"));
        }
        for (i, t) in self.takes.iter().enumerate() {
            if t.index as usize != i {
                return Err(format!("{RECORD_FILE}: дубли идут не по порядку"));
            }
            if !t.game_secs.is_finite() || t.game_secs < 0.0 {
                return Err(format!(
                    "{RECORD_FILE}: у дубля {} недопустимая длительность",
                    i + 1
                ));
            }
        }
        if self.ffmpeg_args.last().map(String::as_str) != Some(OUTPUT_NAME) {
            return Err(format!(
                "{RECORD_FILE}: последний аргумент склейки должен быть {OUTPUT_NAME}"
            ));
        }
        for pair in self.ffmpeg_args.windows(2) {
            if pair[0] == "-i" {
                self.check_input(&pair[1])?;
            }
        }
        Ok(())
    }

    /// Вход склейки — только `clips/takeNNNN/video.mp4` или `audio.wav` существующего дубля.
    fn check_input(&self, arg: &str) -> Result<(), String> {
        let bad = || format!("{RECORD_FILE}: недопустимый вход склейки «{arg}»");
        let mut parts = arg.split('/');
        let (Some("clips"), Some(dir), Some(file), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(bad());
        };
        let index = crate::record::take_number(dir).ok_or_else(bad)?;
        if !TakePlan::FILES.contains(&file) || index as usize >= self.takes.len() {
            return Err(bad());
        }
        Ok(())
    }

    /// Ожидаемые дубли в порядке записи.
    pub fn take_plans(&self) -> Vec<TakePlan> {
        self.takes.iter().map(TakeSpec::to_plan).collect()
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

    fn record_json() -> RecordJson {
        let takes = plans(2);
        let mut args: Vec<String> = vec!["-y".into()];
        for i in 0..2 {
            for f in ["video.mp4", "audio.wav"] {
                args.push("-i".into());
                args.push(format!("clips/take{i:04}/{f}"));
            }
        }
        args.extend(["-map", "[v]", "highlights.mp4"].map(String::from));
        RecordJson {
            version: 1,
            nonce: "ab12cd34".into(),
            demo_path: "C:/demos/match.dem".into(),
            demo_name: "cs2cinema_match_hero".into(),
            demo_len: 1000,
            frame: (1280, 720),
            fps: 60,
            take_nvenc: false,
            takes: takes.iter().map(TakeSpec::from).collect(),
            ffmpeg_args: args,
            video_secs: 10.0,
            shots: 2,
            secs: 10.0,
            plan_hash: "h".into(),
            created_unix: 1,
        }
    }

    #[test]
    fn record_json_round_trips_and_rebuilds_take_plans() {
        let dir = temp("record");
        let rj = record_json();
        rj.save(&dir).unwrap();
        assert!(!dir.join("record.json.tmp").exists());
        let back = RecordJson::load(&dir).unwrap();
        assert_eq!(back, rj);
        assert_eq!(back.take_plans(), plans(2));
        RecordJson::remove(&dir).unwrap();
        RecordJson::remove(&dir).unwrap();
        assert_eq!(RecordJson::load(&dir).unwrap_err(), "record.json нет");
    }

    #[test]
    fn record_json_rejects_tampered_fields() {
        let dir = temp("tamper");
        let check = |what: &str, edit: &dyn Fn(&mut RecordJson), want: &str| {
            let mut rj = record_json();
            edit(&mut rj);
            rj.save(&dir).unwrap();
            let e = RecordJson::load(&dir).expect_err(what);
            assert!(e.contains(want), "{what}: {e}");
        };
        check("версия", &|r| r.version = 2, "версия 2");
        check(
            "nonce длинный",
            &|r| r.nonce = "ab12cd345".into(),
            "метка задачи",
        );
        check(
            "nonce верхний регистр",
            &|r| r.nonce = "AB12CD34".into(),
            "метка задачи",
        );
        check(
            "имя с пробелом",
            &|r| r.demo_name = "my demo".into(),
            "имя демки",
        );
        check(
            "имя со слэшем",
            &|r| r.demo_name = "a/b".into(),
            "имя демки",
        );
        check("имя с ..", &|r| r.demo_name = "a..b".into(), "имя демки");
        check("нет дублей", &|r| r.takes.clear(), "нет ни одного дубля");
        check(
            "индекс не по порядку",
            &|r| r.takes[1].index = 5,
            "не по порядку",
        );
        check(
            "выход склейки",
            &|r| *r.ffmpeg_args.last_mut().unwrap() = "evil.mp4".into(),
            "highlights.mp4",
        );
        check(
            "вход склейки",
            &|r| r.ffmpeg_args[2] = "../../evil.mp4".into(),
            "вход склейки",
        );
        check(
            "чужой дубль на входе",
            &|r| r.ffmpeg_args[2] = "clips/take0009/video.mp4".into(),
            "вход склейки",
        );
        check(
            "чужой файл на входе",
            &|r| r.ffmpeg_args[2] = "clips/take0000/x.exe".into(),
            "вход склейки",
        );
        check("кадр мал", &|r| r.frame = (8, 720), "размер кадра");
        check("кадр велик", &|r| r.frame = (1280, 9000), "размер кадра");
        std::fs::write(dir.join(RECORD_FILE), "{ не json").unwrap();
        assert!(RecordJson::load(&dir).unwrap_err().contains("испорчен"));
    }

    #[test]
    fn reconcile_downgrades_a_changed_take() {
        let clips = temp("reconcile").join("clips");
        for i in 0..3 {
            let d = clips.join(format!("take{i:04}"));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("video.mp4"), vec![0u8; 123]).unwrap();
            std::fs::write(d.join("audio.wav"), vec![0u8; 456]).unwrap();
        }
        let mut m = Manifest::new("h", "n", (1280, 720), &plans(3));
        for i in 0..3 {
            m.mark_validated(i, &facts(), 1);
        }
        assert!(m.reconcile(&clips).is_empty(), "всё на месте");
        std::fs::write(clips.join("take0001/video.mp4"), b"short").unwrap();
        std::fs::remove_file(clips.join("take0002/audio.wav")).unwrap();
        let lines = m.reconcile(&clips);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(m.entry(0).unwrap().status, TakeStatus::Validated);
        assert_eq!(m.entry(1).unwrap().status, TakeStatus::Recorded);
        assert_eq!(m.entry(2).unwrap().status, TakeStatus::Recorded);
        assert!(
            m.entry(1)
                .unwrap()
                .error
                .as_deref()
                .unwrap()
                .contains("изменился")
        );
        assert_eq!(m.missing(), vec![1, 2]);
    }

    #[test]
    fn env_snapshot_takes_versions_and_space_from_the_report() {
        let r = Report {
            cs2_build: Some("25815307".into()),
            hlae_version: Some("2.192.7".into()),
            ffmpeg_version: Some("ffmpeg version 9.0.2".into()),
            ffprobe_version: Some("ffprobe version 9.0.2".into()),
            hlae_ffmpeg_version: Some("ffmpeg version 4.4".into()),
            free_results: Some(10 << 30),
            free_cs2: None,
            ..Report::default()
        };
        let env = EnvSnapshot::from_report(&r, (1280, 720), 60, true, false);
        assert_eq!(env.cs2_build.as_deref(), Some("25815307"));
        assert_eq!(env.ffprobe.as_deref(), Some("ffprobe version 9.0.2"));
        assert_eq!(env.free_results, Some(10 << 30));
        assert!(env.nvenc_assemble && !env.nvenc_takes);
        let line = env.one_line();
        assert!(
            line.contains("CS2 25815307") && line.contains("10.0 ГБ / ? ГБ"),
            "{line}"
        );
        assert!(
            line.contains("кадр 1280x720") && line.contains("да/нет"),
            "{line}"
        );
        let dir = temp("env");
        env.save(&dir).unwrap();
        let back: EnvSnapshot =
            serde_json::from_str(&std::fs::read_to_string(dir.join(ENV_FILE)).unwrap()).unwrap();
        assert_eq!(back, env);
        // старый снимок без полей читается
        let old: EnvSnapshot = serde_json::from_str("{}").unwrap();
        assert_eq!(old, EnvSnapshot::default());
    }
}

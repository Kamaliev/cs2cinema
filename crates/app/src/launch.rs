//! Поиск CS2 и HLAE, установка файлов записи в игру и уборка за ней, проба NVENC. Проверку окружения делает предпроверка (`pipeline::precheck`); запуск игры, консоль и живость — в `host::win::WinHost`.

use std::{
    path::{Component, Path, PathBuf},
    process::Command,
    time::Duration,
};

use pipeline::host::{CmdSpec, Host, HostError};
use serde::{Deserialize, Serialize};
use store::Store;

#[cfg(windows)]
pub fn no_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000) // CREATE_NO_WINDOW
}

/// Ищет CS2 в стандартных местах Steam и в `libraryfolders.vdf`.
pub fn find_cs2() -> Option<PathBuf> {
    const REL: &str = "steamapps/common/Counter-Strike Global Offensive";
    let mut libs: Vec<PathBuf> = Vec::new();
    for root in ["C:/Program Files (x86)/Steam", "C:/Program Files/Steam", "D:/Steam", "D:/SteamLibrary", "E:/SteamLibrary"] {
        libs.push(root.into());
        if let Ok(vdf) = std::fs::read_to_string(Path::new(root).join("steamapps/libraryfolders.vdf")) {
            for line in vdf.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("\"path\"") {
                    let p = rest.trim().trim_matches('"').replace("\\\\", "/");
                    libs.push(p.into());
                }
            }
        }
    }
    libs.into_iter().map(|l| l.join(REL)).find(|p| p.join("game/bin/win64").is_dir())
}

/// Ищет папку с `HLAE.exe`: типичные места, затем неглубокий обход дисков и профиля пользователя.
pub fn find_hlae() -> Option<PathBuf> {
    fn has_hlae(d: &Path) -> bool {
        d.join("HLAE.exe").is_file()
    }
    // обход вглубь не больше `depth`, служебные и огромные папки пропускаем
    fn walk(dir: &Path, depth: u32) -> Option<PathBuf> {
        if has_hlae(dir) {
            return Some(dir.to_path_buf());
        }
        if depth == 0 {
            return None;
        }
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if matches!(name.as_str(), "windows" | "$recycle.bin" | "system volume information" | "node_modules" | "target" | ".git" | "steamapps" | "appdata")
            {
                continue;
            }
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if let Some(p) = walk(&e.path(), depth - 1) {
                    return Some(p);
                }
            }
        }
        None
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for var in ["USERPROFILE", "LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)", "ProgramData"] {
        if let Some(v) = std::env::var_os(var) {
            roots.push(v.into());
        }
    }
    if let Some(p) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        roots.extend(["Downloads", "Desktop", "Documents"].map(|d| p.join(d)));
    }
    // HLAE — портативная программа: чаще всего распакована прямо в корень диска или в Downloads
    let drives: Vec<PathBuf> = ('A'..='Z').map(|c| PathBuf::from(format!("{c}:/"))).filter(|p| p.is_dir()).collect();
    roots.extend(drives.iter().cloned());
    // сначала быстро (глубина 3 в известных местах), затем глубже
    for depth in [3, 5] {
        for r in roots.iter().filter(|r| r.is_dir()) {
            let d = if drives.contains(r) { depth - 1 } else { depth };
            if let Some(p) = walk(r, d) {
                return Some(p);
            }
        }
    }
    None
}

pub fn cs2_exe(cs2_dir: &str) -> PathBuf {
    Path::new(cs2_dir).join("game/bin/win64/cs2.exe")
}

/// Запись об установленных в игру файлах: лежит в папке задачи, переживает падение приложения,
/// по ней уборка находит, что удалить из папки игры (D-05, D-08).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Installed {
    /// Файлы в папке игры: копия демки и скрипты записи.
    pub files: Vec<PathBuf>,
    /// Номера (с нуля) дублей, записанных до конца; остальные дубли в `clips/` — оборванные.
    #[serde(default)]
    pub done_takes: Vec<u32>,
}

/// Имя записи об установленных файлах в папке задачи.
pub const INSTALLED_FILE: &str = "installed.json";

/// Наш ли это файл скрипта записи по имени: `highlights.cfg`, `cs2c_NN.cfg`, `cs2c_NN_K.xml`.
fn is_script_name(name: &str) -> bool {
    name == "highlights.cfg" || (name.starts_with("cs2c_") && (name.ends_with(".cfg") || name.ends_with(".xml")))
}

/// Копирует скрипты и демку в игру. Перед копированием записывает `installed.json` в папку задачи:
/// падение посреди копирования оставляет запись для уборки (D-08). Имя демки (без `.dem`) нужно `playdemo`.
pub fn install_files(cs2_dir: &str, out_dir: &Path, demo_name: &str, write_demo: impl FnOnce(&Path) -> Result<(), String>) -> Result<(), String> {
    let csgo = Path::new(cs2_dir).join("game/csgo");
    let cfg_dir = csgo.join("cfg");
    std::fs::create_dir_all(&cfg_dir).map_err(|e| format!("{}: {e}", cfg_dir.display()))?;
    // highlights.cfg, скрипты шотов cs2c_NN.cfg и пути камер cs2c_NN_K.xml (HLAE грузит их от game/bin/win64)
    let mut copies: Vec<(PathBuf, PathBuf)> = Vec::new();
    for e in std::fs::read_dir(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if is_script_name(&name) {
            copies.push((e.path(), cfg_dir.join(&name)));
        }
    }
    let demo = csgo.join(format!("{demo_name}.dem"));
    // пути прошлой, не до конца убранной установки остаются в записи: уборка доберёт и их
    let mut record = match std::fs::read_to_string(out_dir.join(INSTALLED_FILE)).ok().and_then(|t| serde_json::from_str::<Installed>(&t).ok()) {
        Some(old) => Installed { files: old.files, done_takes: Vec::new() },
        None => Installed::default(),
    };
    for path in copies.iter().map(|(_, to)| to.clone()).chain(std::iter::once(demo.clone())) {
        if !record.files.contains(&path) {
            record.files.push(path);
        }
    }
    write_installed(out_dir, &record).map_err(|e| format!("{}: {e}", out_dir.join(INSTALLED_FILE).display()))?;
    for (from, to) in &copies {
        std::fs::copy(from, to).map_err(|e| format!("копирование {}: {e}", to.file_name().unwrap_or_default().to_string_lossy()))?;
    }
    write_demo(&demo)
}

fn write_installed(out_dir: &Path, rec: &Installed) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(rec).map_err(std::io::Error::other)?;
    std::fs::write(out_dir.join(INSTALLED_FILE), text)
}

/// Файл из записи можно удалять, только если он наш и лежит там, где мы его оставили (T-02-20).
/// `cs2_dir` берётся из текущих настроек, никогда из файла записи. Путь без `..`; после канонизации
/// родитель файла — ровно `<CS2>/game/csgo` (тогда имя `cs2cinema_*.dem`) или `<CS2>/game/csgo/cfg`
/// (тогда `highlights.cfg`, `cs2c_*.cfg` или `cs2c_*.xml`).
fn is_ours(cs2_dir: &str, p: &Path) -> bool {
    if cs2_dir.trim().is_empty() || p.components().any(|c| matches!(c, Component::ParentDir)) {
        return false;
    }
    let (Some(parent), Some(name)) = (p.parent(), p.file_name().and_then(|n| n.to_str())) else {
        return false;
    };
    let csgo = Path::new(cs2_dir).join("game/csgo");
    let (Ok(parent), Ok(csgo_dir)) = (parent.canonicalize(), csgo.canonicalize()) else {
        return false;
    };
    if parent == csgo_dir {
        return name.starts_with("cs2cinema_") && name.ends_with(".dem");
    }
    csgo.join("cfg").canonicalize().is_ok_and(|cfg| parent == cfg) && is_script_name(name)
}

/// Убирает из папки игры файлы, записанные в `installed.json` этой задачи (D-05, D-06). Копии скриптов в
/// папке задачи и оригинальная демка не трогаются. Возвращает строки для журнала задачи.
pub fn cleanup_installed(cs2_dir: &str, out_dir: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(out_dir.join(INSTALLED_FILE)) else {
        return Vec::new();
    };
    let Ok(rec) = serde_json::from_str::<Installed>(&text) else {
        return vec![format!("{INSTALLED_FILE} не разобран — уборка пропущена")];
    };
    let mut notes = Vec::new();
    let mut left = Vec::new();
    let mut removed = 0;
    for path in rec.files {
        if !is_ours(cs2_dir, &path) {
            notes.push(format!("не удаляю {}: файл не из папки CS2", path.display()));
            continue;
        }
        match remove_with_retry(&path) {
            Ok(()) => removed += 1,
            Err(e) => {
                notes.push(format!("уборка отложена: {}: {e}", path.display()));
                left.push(path);
            }
        }
    }
    if removed > 0 {
        notes.insert(0, format!("убрано из папки игры файлов: {removed}"));
    }
    if left.is_empty() {
        let _ = std::fs::remove_file(out_dir.join(INSTALLED_FILE));
    } else {
        // недоубранное повторится при следующем запуске приложения
        let _ = write_installed(out_dir, &Installed { files: left, done_takes: rec.done_takes });
    }
    notes
}

/// Удаление с тремя попытками через 500 мс: файл может быть занят ещё не закрывшейся игрой. Нет файла — уже убран.
fn remove_with_retry(path: &Path) -> std::io::Result<()> {
    let mut last = None;
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(500));
        }
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("не удалось удалить")))
}

/// Запоминает дубль `index` (с нуля) как записанный до конца — для пометки оборванных после падения (D-08).
/// Вспомогательная запись: ошибки молча игнорируются.
pub fn note_take_done(out_dir: &Path, index: u32) {
    let Some(mut rec) = std::fs::read_to_string(out_dir.join(INSTALLED_FILE)).ok().and_then(|t| serde_json::from_str::<Installed>(&t).ok()) else {
        return;
    };
    if !rec.done_takes.contains(&index) {
        rec.done_takes.push(index);
        let _ = write_installed(out_dir, &rec);
    }
}

/// Завершает осиротевшие `cs2.exe`/`HLAE.exe` прошлого экземпляра приложения по `processes.json` в папке задачи (D-15, RET-04).
/// Каждому процессу из файла — `Host::kill_orphan` с полной идентичностью (PID, время создания, образ): чужой процесс
/// с тем же PID не трогается. Файл недоверенный: испорченный не завершает ничего и остаётся для разбора вручную.
/// Возвращает строки для журнала задачи; файл удаляется, если ни одно завершение не провалилось.
pub fn kill_orphans(host: &dyn Host, out_dir: &Path) -> Vec<String> {
    use pipeline::host::OrphanOutcome;
    let list = match pipeline::record::processes::load(out_dir) {
        Ok(None) => return Vec::new(),
        Ok(Some(list)) => list,
        Err(why) => return vec![format!("processes.json не разобран — процессы не трогаю: {why}")],
    };
    let mut notes = Vec::new();
    let mut failed = false;
    for id in &list {
        let (image, pid) = (&id.image, id.pid);
        notes.push(match host.kill_orphan(id) {
            OrphanOutcome::Killed => format!("завершён осиротевший {image} (PID {pid}) прошлого запуска"),
            OrphanOutcome::Gone => format!("{image} (PID {pid}) уже не работает"),
            OrphanOutcome::Mismatch(why) => format!("PID {pid} занят другим процессом — не трогаю: {why}"),
            OrphanOutcome::Failed(e) => {
                failed = true;
                format!("{image} (PID {pid}) завершить не удалось: {e}")
            }
            OrphanOutcome::Unsupported => "завершение процессов на этой платформе недоступно".to_owned(),
        });
    }
    if !failed {
        pipeline::record::processes::remove(out_dir);
    }
    notes
}

/// Номера дублей, которые не надо помечать оборванными: манифест `clips/manifest.json` знает их как записанные
/// (`recorded`) или проверенные (`validated`). Нет манифеста — прежний `installed.json.done_takes`.
fn finished_takes(out_dir: &Path) -> Option<Vec<u32>> {
    use pipeline::manifest::{Manifest, TakeStatus};
    if let Some(m) = Manifest::load(&out_dir.join("clips")) {
        return Some(m.takes.iter().filter(|t| matches!(t.status, TakeStatus::Recorded | TakeStatus::Validated)).map(|t| t.index).collect());
    }
    std::fs::read_to_string(out_dir.join(INSTALLED_FILE)).ok().and_then(|t| serde_json::from_str::<Installed>(&t).ok()).map(|r| r.done_takes)
}

/// Уборка после падения приложения (D-08): удаляет установленные файлы и помечает оборванные дубли `.partial`.
/// Дубли, которые манифест знает как записанные или проверенные, остаются как есть (иначе повтор потерял бы годную запись).
/// Без `installed.json` и без манифеста (запись уже убрана или не начиналась) дубли не трогаются — они целые.
pub fn recover(cs2_dir: &str, out_dir: &Path) -> Vec<String> {
    let done = finished_takes(out_dir);
    let mut notes = cleanup_installed(cs2_dir, out_dir);
    if let Some(done) = done {
        notes.extend(pipeline::record::mark_partial(&out_dir.join("clips"), &done));
    }
    notes
}

/// Восстановление при запуске приложения после падения (RET-04, D-15). Для каждой задачи, оставшейся «выполняется» или
/// «отменяется», сначала завершается осиротевшая игра, затем убирается папка игры, и только потом `reset_running` делает
/// задачу «прервана» («отменена»). Порядок обязателен: живой `cs2.exe` держит копию демки, и уборка без завершения её не удалит.
/// Каждый шаг пишется в журнал задачи строкой «уборка после перезапуска: …».
pub fn recover_after_crash(store: &Store, host: &dyn Host, cs2_dir: &str) -> store::Result<store::Reset> {
    for (id, _, dir) in store.interrupted_jobs().unwrap_or_default() {
        let Some(dir) = dir else { continue };
        let mut notes = kill_orphans(host, &dir);
        notes.extend(recover(cs2_dir, &dir));
        for note in notes {
            let _ = store.append_log(id, &format!("уборка после перезапуска: {note}"));
        }
    }
    store.reset_running()
}

/// Журналы прошлых попыток в папке задачи: `record.log.<номер>` по возрастанию номера (D-11). Текущий `record.log` и
/// файлы с другими именами не входят — открываются только имена такого вида внутри папки задачи (T-03-20).
pub fn old_logs(out_dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<(u64, PathBuf)> = std::fs::read_dir(out_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let digits = name.strip_prefix("record.log.")?;
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Some((digits.parse().ok()?, e.path()))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, p)| p).collect()
}

/// ffmpeg, которым пишет HLAE: `Path=` из `ffmpeg\ffmpeg.ini` в папке HLAE. Разбор — в одном месте, в предпроверке.
pub fn hlae_ffmpeg(hlae_dir: &str) -> Option<PathBuf> {
    pipeline::precheck::hlae_ffmpeg(Path::new(hlae_dir))
}

/// Работает ли NVENC у этого ffmpeg: короткий пробный энкод (кодер может быть в сборке, но без драйвера NVIDIA).
pub fn has_nvenc(host: &dyn Host, ffmpeg: &Path, deadline: Duration, cancel: &dyn Fn() -> bool) -> bool {
    nvenc_check(host, ffmpeg, deadline, cancel).is_ok()
}

/// Пробное кодирование NVENC через `Host::run` — с дедлайном и отменой (TMO-03). В ошибке — строка ffmpeg
/// (например, «драйвер старее, чем нужно»), «проба NVENC не уложилась в N с» или `pipeline::CANCELLED`.
pub fn nvenc_check(host: &dyn Host, ffmpeg: &Path, deadline: Duration, cancel: &dyn Fn() -> bool) -> Result<(), String> {
    let args = ["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=256x144:d=0.2", "-c:v", "h264_nvenc", "-f", "null", "-"];
    let cmd = CmdSpec { program: ffmpeg.to_path_buf(), args: args.map(String::from).into(), cwd: None, deadline };
    match host.run(&cmd, cancel, &mut |_| {}) {
        Ok(out) if out.code == Some(0) => Ok(()),
        Ok(out) => {
            let line = out
                .stderr_tail
                .iter()
                .find(|l| l.contains("nvenc") || l.contains("driver") || l.contains("Driver"))
                .or_else(|| out.stderr_tail.first())
                .map_or("", String::as_str);
            // «[h264_nvenc @ 0x…] Driver does not support…» — префикс не нужен
            let msg = line.rsplit("] ").next().unwrap_or(line).trim();
            Err(if msg.is_empty() { "h264_nvenc не заработал".into() } else { msg.to_owned() })
        }
        Err(HostError::Timeout) => Err(format!("проба NVENC не уложилась в {} с", deadline.as_secs())),
        Err(HostError::Cancelled) => Err(pipeline::CANCELLED.to_owned()),
        Err(HostError::Spawn(e)) => Err(format!("ffmpeg не запустился: {e}")),
        Err(e) => Err(format!("ffmpeg: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Временное дерево: `cs2/game/csgo/cfg`, папка задачи `job` и чужая папка `elsewhere`.
    fn tree(tag: &str) -> (PathBuf, String, PathBuf) {
        let root = std::env::temp_dir().join(format!("cs2cinema-launch-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["cs2/game/csgo/cfg", "job", "elsewhere"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let cs2 = root.join("cs2").to_string_lossy().into_owned();
        let job = root.join("job");
        (root, cs2, job)
    }

    fn put(path: &Path) {
        std::fs::write(path, b"x").unwrap();
    }

    fn record(job: &Path, files: &[&Path], done: &[u32]) {
        let rec = Installed { files: files.iter().map(|p| p.to_path_buf()).collect(), done_takes: done.to_vec() };
        write_installed(job, &rec).unwrap();
    }

    #[test]
    fn cleanup_removes_only_our_files_inside_the_game_folder() {
        let (root, cs2, job) = tree("cleanup");
        let csgo = root.join("cs2/game/csgo");
        let (demo, script) = (csgo.join("cs2cinema_match.dem"), csgo.join("cfg/cs2c_01.cfg"));
        let (foreign, autoexec) = (root.join("elsewhere/cs2c_02.cfg"), csgo.join("cfg/autoexec.cfg"));
        let climbing = csgo.join("cfg/../../../elsewhere/cs2c_03.cfg");
        for f in [&demo, &script, &foreign, &autoexec] {
            put(f);
        }
        put(&root.join("elsewhere/cs2c_03.cfg"));
        record(&job, &[&demo, &script, &foreign, &autoexec, &climbing], &[]);

        let notes = cleanup_installed(&cs2, &job);

        assert!(!demo.exists() && !script.exists(), "свои файлы убраны");
        assert!(foreign.exists() && autoexec.exists(), "посторонние файлы на месте");
        assert!(root.join("elsewhere/cs2c_03.cfg").exists(), "путь с .. отвергнут");
        assert_eq!(notes.iter().filter(|n| n.contains("не удаляю")).count(), 3, "{notes:?}");
        assert!(!job.join(INSTALLED_FILE).exists(), "запись опустошена");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn install_writes_the_record_before_copying_and_cleanup_empties_it() {
        let (root, cs2, job) = tree("install");
        put(&job.join("highlights.cfg"));
        put(&job.join("cs2c_01.cfg"));
        put(&job.join("cs2c_01_1.xml"));
        put(&job.join("notes.txt"));

        install_files(&cs2, &job, "cs2cinema_m", |path| {
            assert!(job.join(INSTALLED_FILE).is_file(), "запись лежит до записи демки");
            std::fs::write(path, b"dem").map_err(|e| e.to_string())
        })
        .unwrap();
        let rec: Installed = serde_json::from_str(&std::fs::read_to_string(job.join(INSTALLED_FILE)).unwrap()).unwrap();
        assert_eq!(rec.files.len(), 4, "{rec:?}");
        assert!(root.join("cs2/game/csgo/cfg/cs2c_01.cfg").is_file() && root.join("cs2/game/csgo/cs2cinema_m.dem").is_file());

        let notes = cleanup_installed(&cs2, &job);
        assert!(!notes.iter().any(|n| n.contains("не удаляю") || n.contains("отложена")), "{notes:?}");
        assert!(!root.join("cs2/game/csgo/cfg/cs2c_01.cfg").exists() && !root.join("cs2/game/csgo/cs2cinema_m.dem").exists());
        assert!(!job.join(INSTALLED_FILE).exists());
        assert!(job.join("cs2c_01.cfg").is_file() && job.join("highlights.cfg").is_file(), "копии в папке задачи остаются");

        // копирование сорвалось (вместо файла каталог): запись всё равно уже есть
        std::fs::create_dir(job.join("cs2c_09.cfg")).unwrap();
        assert!(install_files(&cs2, &job, "cs2cinema_m", |_| Ok(())).is_err());
        let rec: Installed = serde_json::from_str(&std::fs::read_to_string(job.join(INSTALLED_FILE)).unwrap()).unwrap();
        assert!(rec.files.iter().any(|f| f.ends_with("cs2c_09.cfg")), "{rec:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_installed_json_is_left_alone() {
        let (root, cs2, job) = tree("malformed");
        let demo = root.join("cs2/game/csgo/cs2cinema_m.dem");
        put(&demo);
        std::fs::write(job.join(INSTALLED_FILE), "{не json").unwrap();

        let notes = cleanup_installed(&cs2, &job);

        assert_eq!(notes, vec!["installed.json не разобран — уборка пропущена".to_owned()]);
        assert_eq!(std::fs::read_to_string(job.join(INSTALLED_FILE)).unwrap(), "{не json");
        assert!(demo.exists());
        assert!(cleanup_installed(&cs2, &root.join("no-such-job")).is_empty(), "нет записи — нечего убирать");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recover_marks_unfinished_takes_partial() {
        let (root, cs2, job) = tree("recover");
        let demo = root.join("cs2/game/csgo/cs2cinema_m.dem");
        put(&demo);
        for t in ["take0000", "take0001"] {
            std::fs::create_dir_all(job.join("clips").join(t)).unwrap();
        }
        record(&job, &[&demo], &[0]);

        let notes = recover(&cs2, &job);

        assert!(!demo.exists(), "копия демки убрана");
        assert!(job.join("clips/take0000").is_dir(), "записанный дубль цел");
        assert!(job.join("clips/take0001.partial").is_dir() && !job.join("clips/take0001").exists(), "{notes:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recover_without_a_record_keeps_finished_takes() {
        let (root, cs2, job) = tree("recover-none");
        std::fs::create_dir_all(job.join("clips/take0000")).unwrap();
        assert!(recover(&cs2, &job).is_empty());
        assert!(job.join("clips/take0000").is_dir(), "без записи дубли целые — запись уже убрана после успеха");
        let _ = std::fs::remove_dir_all(root);
    }

    fn identity(pid: u32) -> pipeline::host::ProcIdentity {
        pipeline::host::ProcIdentity { pid, created: 1234, image: "cs2.exe".into() }
    }

    /// Задача «выполняется» в базе с этой папкой; возвращает id.
    fn running_job(store: &Store, job: &Path) -> i64 {
        let id = store
            .add_job(&store::NewJob {
                kind: store::JobKind::Record,
                source: store::JobSource::Faceit("1-abc".into()),
                label: "матч".into(),
                hero_name: "me".into(),
                hero_steam: 42,
                map: "de_nuke".into(),
            })
            .unwrap();
        store.start_job(id).unwrap();
        store.set_out_dir(id, job).unwrap();
        id
    }

    #[test]
    fn startup_kills_the_orphan_before_cleanup_and_marks_the_job_interrupted() {
        use pipeline::host::{
            OrphanOutcome,
            fake::{FakeHost, Scenario},
        };
        let (root, cs2, job) = tree("orphan");
        let demo = root.join("cs2/game/csgo/cs2cinema_m.dem");
        put(&demo);
        record(&job, &[&demo], &[]);
        pipeline::record::processes::save(&job, &[identity(777)]).unwrap();
        let store = Store::in_memory().unwrap();
        let id = running_job(&store, &job);
        let fake = FakeHost::new(Scenario::happy(1).orphan(777, OrphanOutcome::Killed));

        let reset = recover_after_crash(&store, &fake, &cs2).unwrap();

        assert_eq!(fake.orphan_kills(), vec![identity(777)]);
        assert_eq!(reset.interrupted, 1);
        let j = store.job(id).unwrap().unwrap();
        assert_eq!(j.status, store::JobStatus::Interrupted);
        assert!(!job.join(pipeline::record::processes::PROCESSES_FILE).exists(), "processes.json убран");
        assert!(!demo.exists(), "копия демки убрана");
        let at = |needle: &str| j.log.lines().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("нет строки «{needle}» в журнале:\n{}", j.log));
        assert!(at("завершён осиротевший cs2.exe (PID 777)") < at("убрано из папки игры"), "процесс завершён раньше уборки:\n{}", j.log);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn orphan_with_another_creation_time_is_left_alone() {
        use pipeline::host::{
            OrphanOutcome,
            fake::{FakeHost, Scenario},
        };
        let (root, cs2, job) = tree("orphan-mismatch");
        pipeline::record::processes::save(&job, &[identity(777)]).unwrap();
        let store = Store::in_memory().unwrap();
        let id = running_job(&store, &job);
        let fake = FakeHost::new(Scenario::happy(1).orphan(777, OrphanOutcome::Mismatch("время создания другое".into())));

        let reset = recover_after_crash(&store, &fake, &cs2).unwrap();

        let j = store.job(id).unwrap().unwrap();
        assert_eq!((reset.interrupted, j.status), (1, store::JobStatus::Interrupted));
        assert!(j.log.contains("не трогаю"), "{}", j.log);
        assert!(!job.join(pipeline::record::processes::PROCESSES_FILE).exists(), "чужой процесс не держит файл");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_processes_json_kills_nothing() {
        use pipeline::host::fake::{FakeHost, Scenario};
        let (root, cs2, job) = tree("orphan-garbage");
        std::fs::write(job.join(pipeline::record::processes::PROCESSES_FILE), "{не json").unwrap();
        let store = Store::in_memory().unwrap();
        let id = running_job(&store, &job);
        let fake = FakeHost::new(Scenario::happy(1));

        recover_after_crash(&store, &fake, &cs2).unwrap();

        assert!(fake.orphan_kills().is_empty());
        let j = store.job(id).unwrap().unwrap();
        assert!(j.log.contains("processes.json не разобран — процессы не трогаю"), "{}", j.log);
        assert_eq!(j.status, store::JobStatus::Interrupted);
        assert!(job.join(pipeline::record::processes::PROCESSES_FILE).exists(), "файл остаётся для разбора вручную");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recover_keeps_takes_the_manifest_knows() {
        use pipeline::manifest::{Manifest, TakeEntry, TakeStatus};
        let (root, cs2, job) = tree("recover-manifest");
        let clips = job.join("clips");
        let take = |index: u32, status| TakeEntry { index, dir: format!("take{index:04}"), status, ..TakeEntry::default() };
        let manifest = Manifest {
            version: 1,
            takes: vec![take(0, TakeStatus::Validated), take(1, TakeStatus::Recorded), take(2, TakeStatus::Pending)],
            ..Manifest::default()
        };
        for t in ["take0000", "take0001", "take0002"] {
            std::fs::create_dir_all(clips.join(t)).unwrap();
        }
        manifest.save(&clips).unwrap();

        let notes = recover(&cs2, &job);

        assert!(clips.join("take0000").is_dir() && clips.join("take0001").is_dir(), "известные манифесту дубли целы: {notes:?}");
        assert!(clips.join("take0002.partial").is_dir() && !clips.join("take0002").exists(), "недописанный помечен: {notes:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn old_logs_are_listed_in_attempt_order() {
        let (root, _cs2, job) = tree("old-logs");
        for n in ["record.log", "record.log.10", "record.log.2", "record.log.1", "record.log.x", "record.log."] {
            put(&job.join(n));
        }
        let names: Vec<String> = old_logs(&job).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["record.log.1", "record.log.2", "record.log.10"]);
        assert!(old_logs(&root.join("no-such-job")).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn note_take_done_adds_each_index_once() {
        let (root, _cs2, job) = tree("done");
        note_take_done(&job, 3);
        assert!(!job.join(INSTALLED_FILE).exists(), "без записи ничего не создаётся");
        record(&job, &[], &[]);
        note_take_done(&job, 1);
        note_take_done(&job, 1);
        note_take_done(&job, 0);
        let rec: Installed = serde_json::from_str(&std::fs::read_to_string(job.join(INSTALLED_FILE)).unwrap()).unwrap();
        assert_eq!(rec.done_takes, vec![1, 0]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn nvenc_failure_names_the_driver_line_without_the_ffmpeg_prefix() {
        use pipeline::host::{
            CmdOutput,
            fake::{FakeHost, Scenario},
        };
        let failed = CmdOutput {
            code: Some(1),
            stderr_tail: vec!["Stream mapping:".into(), "[h264_nvenc @ 0x1] Driver does not support the required nvenc API version".into()],
            elapsed: Duration::ZERO,
        };
        let host = FakeHost::new(Scenario::happy(1).run_result(Ok(failed)));
        let e = nvenc_check(&host, Path::new("ffmpeg"), Duration::from_secs(20), &|| false).unwrap_err();
        assert_eq!(e, "Driver does not support the required nvenc API version");

        let host = FakeHost::new(Scenario::happy(1).run_result(Err(HostError::Spawn("нет файла".into()))));
        assert!(nvenc_check(&host, Path::new("ffmpeg"), Duration::from_secs(20), &|| false).unwrap_err().contains("не запустился"));
        let host = FakeHost::new(Scenario::happy(1));
        assert_eq!(nvenc_check(&host, Path::new("ffmpeg"), Duration::from_secs(20), &|| true), Err(pipeline::CANCELLED.to_owned()), "отмена прерывает пробу");
        assert!(nvenc_check(&host, Path::new("ffmpeg"), Duration::from_secs(20), &|| false).is_ok());
    }
}

//! Поиск CS2 и HLAE, восстановление после падения приложения, проба NVENC. Файлы записи в папке игры — `pipeline::gamedir`; проверку окружения делает предпроверка (`pipeline::precheck`); запуск игры, консоль и живость — в `host::win::WinHost`.

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use pipeline::host::{CmdSpec, Host, HostError};
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

/// Восстановление при запуске приложения после падения (RET-04, D-15). Для каждой задачи, оставшейся «выполняется» или
/// «отменяется», сначала завершается осиротевшая игра, затем убирается папка игры, и только потом `reset_running` делает
/// задачу «прервана» («отменена»). Порядок обязателен: живой `cs2.exe` держит копию демки, и уборка без завершения её не удалит.
/// Каждый шаг пишется в журнал задачи строкой «уборка после перезапуска: …».
pub fn recover_after_crash(store: &Store, host: &dyn Host, cs2_dir: &str) -> store::Result<store::Reset> {
    for (id, _, dir) in store.interrupted_jobs().unwrap_or_default() {
        let Some(dir) = dir else { continue };
        let mut notes = pipeline::gamedir::kill_orphans(host, &dir);
        notes.extend(pipeline::gamedir::recover(Path::new(cs2_dir), &dir));
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
        let rec = pipeline::gamedir::Installed { files: files.iter().map(|p| p.to_path_buf()).collect(), done_takes: done.to_vec() };
        std::fs::write(job.join(pipeline::gamedir::INSTALLED_FILE), serde_json::to_string_pretty(&rec).unwrap()).unwrap();
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

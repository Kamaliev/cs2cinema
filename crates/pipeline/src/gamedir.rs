//! Файлы записи в папке игры: установка, уборка при любом исходе, отметка записанных дублей и уборка после падения
//! приложения. Удаляется только то, что записано в `installed.json` задачи и лежит в `<CS2>/game/csgo[/cfg]` (T-02-20).

use std::{
    path::{Component, Path, PathBuf},
    time::Duration,
};

use crate::host::Host;
use serde::{Deserialize, Serialize};

/// Путь к `cs2.exe` внутри папки CS2.
pub fn cs2_exe(cs2_dir: &Path) -> PathBuf {
    cs2_dir.join("game/bin/win64/cs2.exe")
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
pub fn is_script_name(name: &str) -> bool {
    name == "highlights.cfg"
        || (name.starts_with("cs2c_") && (name.ends_with(".cfg") || name.ends_with(".xml")))
}

/// Копирует скрипты и демку в игру. Перед копированием записывает `installed.json` в папку задачи:
/// падение посреди копирования оставляет запись для уборки (D-08). Имя демки (без `.dem`) нужно `playdemo`.
pub fn install_files(
    cs2_dir: &Path,
    out_dir: &Path,
    demo_name: &str,
    write_demo: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let csgo = cs2_dir.join("game/csgo");
    let cfg_dir = csgo.join("cfg");
    std::fs::create_dir_all(&cfg_dir).map_err(|e| format!("{}: {e}", cfg_dir.display()))?;
    // highlights.cfg, скрипты шотов cs2c_NN.cfg и пути камер cs2c_NN_K.xml (HLAE грузит их от game/bin/win64)
    let mut copies: Vec<(PathBuf, PathBuf)> = Vec::new();
    for e in std::fs::read_dir(out_dir)
        .map_err(|e| format!("{}: {e}", out_dir.display()))?
        .flatten()
    {
        let name = e.file_name().to_string_lossy().into_owned();
        if is_script_name(&name) {
            copies.push((e.path(), cfg_dir.join(&name)));
        }
    }
    let demo = csgo.join(format!("{demo_name}.dem"));
    // пути прошлой, не до конца убранной установки остаются в записи: уборка доберёт и их
    let mut record = match std::fs::read_to_string(out_dir.join(INSTALLED_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<Installed>(&t).ok())
    {
        Some(old) => Installed {
            files: old.files,
            done_takes: Vec::new(),
        },
        None => Installed::default(),
    };
    for path in copies
        .iter()
        .map(|(_, to)| to.clone())
        .chain(std::iter::once(demo.clone()))
    {
        if !record.files.contains(&path) {
            record.files.push(path);
        }
    }
    write_installed(out_dir, &record)
        .map_err(|e| format!("{}: {e}", out_dir.join(INSTALLED_FILE).display()))?;
    for (from, to) in &copies {
        std::fs::copy(from, to).map_err(|e| {
            format!(
                "копирование {}: {e}",
                to.file_name().unwrap_or_default().to_string_lossy()
            )
        })?;
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
fn is_ours(cs2_dir: &Path, p: &Path) -> bool {
    if cs2_dir.as_os_str().is_empty() || p.components().any(|c| matches!(c, Component::ParentDir)) {
        return false;
    }
    let (Some(parent), Some(name)) = (p.parent(), p.file_name().and_then(|n| n.to_str())) else {
        return false;
    };
    let csgo = cs2_dir.join("game/csgo");
    let (Ok(parent), Ok(csgo_dir)) = (parent.canonicalize(), csgo.canonicalize()) else {
        return false;
    };
    if parent == csgo_dir {
        return name.starts_with("cs2cinema_") && name.ends_with(".dem");
    }
    csgo.join("cfg")
        .canonicalize()
        .is_ok_and(|cfg| parent == cfg)
        && is_script_name(name)
}

/// Убирает из папки игры файлы, записанные в `installed.json` этой задачи (D-05, D-06). Копии скриптов в
/// папке задачи и оригинальная демка не трогаются. Возвращает строки для журнала задачи.
pub fn cleanup_installed(cs2_dir: &Path, out_dir: &Path) -> Vec<String> {
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
            notes.push(format!(
                "не удаляю {}: файл не из папки CS2",
                path.display()
            ));
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
        let _ = write_installed(
            out_dir,
            &Installed {
                files: left,
                done_takes: rec.done_takes,
            },
        );
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
    let Some(mut rec) = std::fs::read_to_string(out_dir.join(INSTALLED_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<Installed>(&t).ok())
    else {
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
    use crate::host::OrphanOutcome;
    let list = match crate::record::processes::load(out_dir) {
        Ok(None) => return Vec::new(),
        Ok(Some(list)) => list,
        Err(why) => {
            return vec![format!(
                "processes.json не разобран — процессы не трогаю: {why}"
            )];
        }
    };
    let mut notes = Vec::new();
    let mut failed = false;
    for id in &list {
        let (image, pid) = (&id.image, id.pid);
        notes.push(match host.kill_orphan(id) {
            OrphanOutcome::Killed => {
                format!("завершён осиротевший {image} (PID {pid}) прошлого запуска")
            }
            OrphanOutcome::Gone => format!("{image} (PID {pid}) уже не работает"),
            OrphanOutcome::Mismatch(why) => {
                format!("PID {pid} занят другим процессом — не трогаю: {why}")
            }
            OrphanOutcome::Failed(e) => {
                failed = true;
                format!("{image} (PID {pid}) завершить не удалось: {e}")
            }
            OrphanOutcome::Unsupported => {
                "завершение процессов на этой платформе недоступно".to_owned()
            }
        });
    }
    if !failed {
        crate::record::processes::remove(out_dir);
    }
    notes
}

/// Номера дублей, которые не надо помечать оборванными: манифест `clips/manifest.json` знает их как записанные
/// (`recorded`) или проверенные (`validated`). Нет манифеста — прежний `installed.json.done_takes`.
fn finished_takes(out_dir: &Path) -> Option<Vec<u32>> {
    use crate::manifest::{Manifest, TakeStatus};
    if let Some(m) = Manifest::load(&out_dir.join("clips")) {
        return Some(
            m.takes
                .iter()
                .filter(|t| matches!(t.status, TakeStatus::Recorded | TakeStatus::Validated))
                .map(|t| t.index)
                .collect(),
        );
    }
    std::fs::read_to_string(out_dir.join(INSTALLED_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<Installed>(&t).ok())
        .map(|r| r.done_takes)
}

/// Уборка после падения приложения (D-08): удаляет установленные файлы и помечает оборванные дубли `.partial`.
/// Дубли, которые манифест знает как записанные или проверенные, остаются как есть (иначе повтор потерял бы годную запись).
/// Без `installed.json` и без манифеста (запись уже убрана или не начиналась) дубли не трогаются — они целые.
pub fn recover(cs2_dir: &Path, out_dir: &Path) -> Vec<String> {
    let done = finished_takes(out_dir);
    let mut notes = cleanup_installed(cs2_dir, out_dir);
    if let Some(done) = done {
        notes.extend(crate::record::mark_partial(&out_dir.join("clips"), &done));
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Временное дерево: `cs2/game/csgo/cfg`, папка задачи `job` и чужая папка `elsewhere`.
    fn tree(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("cs2cinema-gamedir-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["cs2/game/csgo/cfg", "job", "elsewhere"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let cs2 = root.join("cs2");
        let job = root.join("job");
        (root, cs2, job)
    }

    fn put(path: &Path) {
        std::fs::write(path, b"x").unwrap();
    }

    fn record(job: &Path, files: &[&Path], done: &[u32]) {
        let rec = Installed {
            files: files.iter().map(|p| p.to_path_buf()).collect(),
            done_takes: done.to_vec(),
        };
        write_installed(job, &rec).unwrap();
    }

    #[test]
    fn cleanup_removes_only_our_files_inside_the_game_folder() {
        let (root, cs2, job) = tree("cleanup");
        let csgo = root.join("cs2/game/csgo");
        let (demo, script) = (
            csgo.join("cs2cinema_match.dem"),
            csgo.join("cfg/cs2c_01.cfg"),
        );
        let (foreign, autoexec) = (
            root.join("elsewhere/cs2c_02.cfg"),
            csgo.join("cfg/autoexec.cfg"),
        );
        let climbing = csgo.join("cfg/../../../elsewhere/cs2c_03.cfg");
        for f in [&demo, &script, &foreign, &autoexec] {
            put(f);
        }
        put(&root.join("elsewhere/cs2c_03.cfg"));
        record(&job, &[&demo, &script, &foreign, &autoexec, &climbing], &[]);

        let notes = cleanup_installed(&cs2, &job);

        assert!(!demo.exists() && !script.exists(), "свои файлы убраны");
        assert!(
            foreign.exists() && autoexec.exists(),
            "посторонние файлы на месте"
        );
        assert!(
            root.join("elsewhere/cs2c_03.cfg").exists(),
            "путь с .. отвергнут"
        );
        assert_eq!(
            notes.iter().filter(|n| n.contains("не удаляю")).count(),
            3,
            "{notes:?}"
        );
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
            assert!(
                job.join(INSTALLED_FILE).is_file(),
                "запись лежит до записи демки"
            );
            std::fs::write(path, b"dem").map_err(|e| e.to_string())
        })
        .unwrap();
        let rec: Installed =
            serde_json::from_str(&std::fs::read_to_string(job.join(INSTALLED_FILE)).unwrap())
                .unwrap();
        assert_eq!(rec.files.len(), 4, "{rec:?}");
        assert!(
            root.join("cs2/game/csgo/cfg/cs2c_01.cfg").is_file()
                && root.join("cs2/game/csgo/cs2cinema_m.dem").is_file()
        );

        let notes = cleanup_installed(&cs2, &job);
        assert!(
            !notes
                .iter()
                .any(|n| n.contains("не удаляю") || n.contains("отложена")),
            "{notes:?}"
        );
        assert!(
            !root.join("cs2/game/csgo/cfg/cs2c_01.cfg").exists()
                && !root.join("cs2/game/csgo/cs2cinema_m.dem").exists()
        );
        assert!(!job.join(INSTALLED_FILE).exists());
        assert!(
            job.join("cs2c_01.cfg").is_file() && job.join("highlights.cfg").is_file(),
            "копии в папке задачи остаются"
        );

        // копирование сорвалось (вместо файла каталог): запись всё равно уже есть
        std::fs::create_dir(job.join("cs2c_09.cfg")).unwrap();
        assert!(install_files(&cs2, &job, "cs2cinema_m", |_| Ok(())).is_err());
        let rec: Installed =
            serde_json::from_str(&std::fs::read_to_string(job.join(INSTALLED_FILE)).unwrap())
                .unwrap();
        assert!(
            rec.files.iter().any(|f| f.ends_with("cs2c_09.cfg")),
            "{rec:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_installed_json_is_left_alone() {
        let (root, cs2, job) = tree("malformed");
        let demo = root.join("cs2/game/csgo/cs2cinema_m.dem");
        put(&demo);
        std::fs::write(job.join(INSTALLED_FILE), "{не json").unwrap();

        let notes = cleanup_installed(&cs2, &job);

        assert_eq!(
            notes,
            vec!["installed.json не разобран — уборка пропущена".to_owned()]
        );
        assert_eq!(
            std::fs::read_to_string(job.join(INSTALLED_FILE)).unwrap(),
            "{не json"
        );
        assert!(demo.exists());
        assert!(
            cleanup_installed(&cs2, &root.join("no-such-job")).is_empty(),
            "нет записи — нечего убирать"
        );
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
        assert!(
            job.join("clips/take0001.partial").is_dir() && !job.join("clips/take0001").exists(),
            "{notes:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recover_without_a_record_keeps_finished_takes() {
        let (root, cs2, job) = tree("recover-none");
        std::fs::create_dir_all(job.join("clips/take0000")).unwrap();
        assert!(recover(&cs2, &job).is_empty());
        assert!(
            job.join("clips/take0000").is_dir(),
            "без записи дубли целые — запись уже убрана после успеха"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    fn identity(pid: u32) -> crate::host::ProcIdentity {
        crate::host::ProcIdentity {
            pid,
            created: 1234,
            image: "cs2.exe".into(),
        }
    }

    #[test]
    fn malformed_processes_json_kills_nothing() {
        use crate::host::fake::{FakeHost, Scenario};
        let (root, _cs2, job) = tree("orphan-garbage");
        std::fs::write(
            job.join(crate::record::processes::PROCESSES_FILE),
            "{не json",
        )
        .unwrap();
        let fake = FakeHost::new(Scenario::happy(1));

        let notes = kill_orphans(&fake, &job);

        assert!(fake.orphan_kills().is_empty());
        assert!(
            notes
                .iter()
                .any(|n| n.contains("processes.json не разобран — процессы не трогаю")),
            "{notes:?}"
        );
        assert!(
            job.join(crate::record::processes::PROCESSES_FILE).exists(),
            "файл остаётся для разбора вручную"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn orphan_is_killed_through_its_full_identity() {
        use crate::host::{
            OrphanOutcome,
            fake::{FakeHost, Scenario},
        };
        let (root, _cs2, job) = tree("orphan-killed");
        crate::record::processes::save(&job, &[identity(777)]).unwrap();
        let fake = FakeHost::new(Scenario::happy(1).orphan(777, OrphanOutcome::Killed));

        let notes = kill_orphans(&fake, &job);

        assert_eq!(fake.orphan_kills(), vec![identity(777)]);
        assert!(
            notes
                .iter()
                .any(|n| n.contains("завершён осиротевший cs2.exe (PID 777)")),
            "{notes:?}"
        );
        assert!(
            !job.join(crate::record::processes::PROCESSES_FILE).exists(),
            "processes.json убран"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn empty_game_folder_removes_nothing() {
        let (root, _cs2, job) = tree("empty-cs2");
        let demo = root.join("cs2/game/csgo/cs2cinema_m.dem");
        put(&demo);
        record(&job, &[&demo], &[]);
        assert!(!is_ours(Path::new(""), &demo));
        let notes = cleanup_installed(Path::new(""), &job);
        assert!(
            demo.exists() && notes.iter().any(|n| n.contains("не удаляю")),
            "{notes:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recover_keeps_takes_the_manifest_knows() {
        use crate::manifest::{Manifest, TakeEntry, TakeStatus};
        let (root, cs2, job) = tree("recover-manifest");
        let clips = job.join("clips");
        let take = |index: u32, status| TakeEntry {
            index,
            dir: format!("take{index:04}"),
            status,
            ..TakeEntry::default()
        };
        let manifest = Manifest {
            version: 1,
            takes: vec![
                take(0, TakeStatus::Validated),
                take(1, TakeStatus::Recorded),
                take(2, TakeStatus::Pending),
            ],
            ..Manifest::default()
        };
        for t in ["take0000", "take0001", "take0002"] {
            std::fs::create_dir_all(clips.join(t)).unwrap();
        }
        manifest.save(&clips).unwrap();

        let notes = recover(&cs2, &job);

        assert!(
            clips.join("take0000").is_dir() && clips.join("take0001").is_dir(),
            "известные манифесту дубли целы: {notes:?}"
        );
        assert!(
            clips.join("take0002.partial").is_dir() && !clips.join("take0002").exists(),
            "недописанный помечен: {notes:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn note_take_done_adds_each_index_once() {
        let (root, _cs2, job) = tree("done");
        note_take_done(&job, 3);
        assert!(
            !job.join(INSTALLED_FILE).exists(),
            "без записи ничего не создаётся"
        );
        record(&job, &[], &[]);
        note_take_done(&job, 1);
        note_take_done(&job, 1);
        note_take_done(&job, 0);
        let rec: Installed =
            serde_json::from_str(&std::fs::read_to_string(job.join(INSTALLED_FILE)).unwrap())
                .unwrap();
        assert_eq!(rec.done_takes, vec![1, 0]);
        let _ = std::fs::remove_dir_all(root);
    }
}

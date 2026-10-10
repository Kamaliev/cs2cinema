//! Сбор фактов окружения для предпроверки: хост (процессы, место, версии), файлы, утилиты с
//! дедлайном. Оценка фактов — [`super::evaluate`]; здесь ничего не завершается и не запускается,
//! кроме `-version` утилит: у хоста нет метода «завершить по PID или имени» (PRE-05, TMO-05).
//!
//! Одну и ту же [`run`] вызывают задача из очереди и кнопка «Проверить окружение» в настройках,
//! поэтому отчёт у них общий (D-01, PRE-01).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use host::{CmdSpec, Host, HostError, Liveness, PrevGame};

use super::{
    DemoFact, Facts, Level, OutFact, PrevGameFact, Report, SpaceFacts, ToolFact, evaluate, space,
    space_items, steam,
};
use crate::record::Deadlines;
use crate::{CANCELLED, Progress, absolute_slash};

/// Откуда берётся демка задачи.
#[derive(Debug, Clone)]
pub enum Demo {
    /// Готовый файл: проверяется по первым байтам.
    File(PathBuf),
    /// Демку ещё предстоит скачать с FACEIT.
    Download,
    /// Проверка из настроек, без задачи.
    NotApplicable,
}

/// Что проверять.
#[derive(Debug, Clone)]
pub struct Inputs {
    /// Папка HLAE (`HLAE.exe`, `x64/AfxHookSource2.dll`).
    pub hlae_dir: PathBuf,
    /// Папка игры («Counter-Strike Global Offensive»).
    pub cs2_dir: PathBuf,
    /// Корневая папка результатов.
    pub out_root: PathBuf,
    pub demo: Demo,
    /// Пара (сборка CS2, версия HLAE) последней удачной записи.
    pub last_good: Option<(String, String)>,
    /// Дедлайны — уже с «Запасом по времени», как у записи.
    pub deadlines: Deadlines,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

/// Собрать все факты окружения и оценить их. Ждёт свою игру прошлой задачи (D-14), поэтому может
/// занять до `prev_game_exit + next_job_settle`; отмена прерывает ожидание и утилиты.
pub fn run(host: &dyn Host, i: &Inputs, p: &dyn Progress) -> crate::Result<Report> {
    crate::check(p)?;
    let mut facts = Facts::default();

    // D-14, TMO-07: сначала дожидаемся своей игры прошлой задачи, и только потом смотрим процессы —
    // иначе она сама окажется «чужим» CS2.
    facts.prev_game = match host.wait_previous_game(
        i.deadlines.prev_game_exit,
        i.deadlines.next_job_settle,
        &|| p.cancelled(),
    ) {
        PrevGame::Cancelled => return Err(CANCELLED.to_owned()),
        PrevGame::Clear { waited } if waited.is_zero() => PrevGameFact::Clear,
        PrevGame::Clear { waited } => {
            if waited.as_secs() >= 1 {
                p.log(&format!(
                    "предпроверка: ждал закрытия игры прошлой задачи {} с",
                    waited.as_secs_f64().round() as u64
                ));
            }
            PrevGameFact::Waited(waited)
        }
        PrevGame::Stuck {
            pid,
            since_end,
            liveness,
        } => PrevGameFact::Stuck {
            pid,
            secs: since_end.as_secs(),
            confirmed_alive: liveness == Liveness::Alive,
        },
    };
    crate::check(p)?;

    facts.processes = Some(match host.processes() {
        Ok(list) => Ok(list.into_iter().map(|x| (x.pid, x.exe_name)).collect()),
        Err(e) => Err(e.to_string()),
    });

    // файлы (PRE-03); тексты пунктов — в `evaluate`
    facts.hlae_exe = Some(i.hlae_dir.join("HLAE.exe").is_file());
    facts.hook_dll = Some(i.hlae_dir.join("x64").join("AfxHookSource2.dll").is_file());
    facts.cs2_exe = Some(i.cs2_dir.join("game/bin/win64/cs2.exe").is_file());
    facts.hlae_version = host.file_version(&i.hlae_dir.join("HLAE.exe"));

    // утилиты отвечают на `-version` с дедлайном и отменой (PRE-03, TMO-03)
    let probe_for = i.deadlines.tool_probe;
    facts.ffmpeg = probe(host, &i.ffmpeg, probe_for, p)?;
    facts.ffprobe = probe(host, &i.ffprobe, probe_for, p)?;
    facts.hlae_ffmpeg = Some(match hlae_ffmpeg(&i.hlae_dir) {
        None => (None, ToolFact::Unchecked),
        Some(path) => {
            let fact = probe(host, &path, probe_for, p)?;
            (Some(path), fact)
        }
    });

    // Steam и сборка CS2 (DIF-01, DIF-02)
    facts.manifest = Some(match steam::manifest_path(&i.cs2_dir) {
        Some(path) => {
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
        }
        None => Err("папка игры лежит не в steamapps/common".to_owned()),
    });
    facts.steam_inf = std::fs::read_to_string(i.cs2_dir.join("game/csgo/steam.inf")).ok();
    facts.last_good = i.last_good.clone();

    facts.demo = demo_fact(&i.demo);
    facts.out_root = Some(out_fact(&i.out_root));
    crate::check(p)?;
    facts.space = Some(space_facts(host, i, space::floors(), false));

    let report = evaluate(&facts);
    for item in &report.items {
        match item.level {
            Level::Ok => {}
            Level::Warn => p.log(&format!("предпроверка: ⚠ {}", item.message)),
            Level::Fail => p.log(&format!("предпроверка: ✖ {}", item.message)),
        }
    }
    Ok(report)
}

/// Второй проход оценки места (D-03): после плана известна потребность по дублям, и пункты места
/// в отчёте заменяются. В журнал ничего не пишет — новые пункты логирует вызывающий.
pub fn check_space(host: &dyn Host, i: &Inputs, need: space::Need, report: &mut Report) {
    report.replace(space_items(&space_facts(host, i, need, true)));
}

/// Свободное место на обоих дисках и потребность `need`.
fn space_facts(host: &dyn Host, i: &Inputs, need: space::Need, by_plan: bool) -> SpaceFacts {
    let free = |dir: &Path| host.free_space(dir).map_err(|e| e.to_string());
    SpaceFacts {
        results: free(&i.out_root),
        cs2: free(&i.cs2_dir),
        same_volume: space::same_volume(&absolute_slash(&i.out_root), &absolute_slash(&i.cs2_dir)),
        need,
        by_plan,
    }
}

/// ffmpeg, которым пишет HLAE: `Path=` из `ffmpeg\ffmpeg.ini`; без файла — `ffmpeg\bin\ffmpeg.exe`
/// внутри папки HLAE (HLAE берёт его по умолчанию); нет ни того, ни другого — `None`.
pub fn hlae_ffmpeg(hlae_dir: &Path) -> Option<PathBuf> {
    let ini = std::fs::read_to_string(hlae_dir.join("ffmpeg/ffmpeg.ini")).ok();
    let configured = ini.and_then(|text| {
        text.lines().find_map(|l| {
            let value = l
                .trim()
                .strip_prefix("Path=")?
                .trim()
                .trim_matches('"')
                .trim();
            (!value.is_empty()).then(|| PathBuf::from(value))
        })
    });
    if let Some(path) = configured {
        // относительный путь HLAE считает от своей папки ffmpeg
        return Some(if path.is_absolute() {
            path
        } else {
            hlae_dir.join("ffmpeg").join(path)
        });
    }
    let bundled = hlae_dir.join("ffmpeg/bin/ffmpeg.exe");
    bundled.is_file().then_some(bundled)
}

/// Ответ утилиты на `-version`. Вывод утилиты — недоверенный текст: берётся только первая
/// непустая строка и хвост stderr.
fn probe(
    host: &dyn Host,
    program: &Path,
    deadline: Duration,
    p: &dyn Progress,
) -> crate::Result<ToolFact> {
    let spec = CmdSpec {
        program: program.to_path_buf(),
        args: vec!["-version".to_owned()],
        cwd: None,
        deadline,
    };
    let mut first_line = String::new();
    let result = host.run(&spec, &|| p.cancelled(), &mut |line| {
        let line = line.trim();
        if first_line.is_empty() && !line.is_empty() {
            first_line = line.chars().take(200).collect();
        }
    });
    match result {
        Ok(out) if out.code == Some(0) => Ok(ToolFact::Answered { first_line }),
        Ok(out) => {
            let code = out
                .code
                .map_or_else(|| "процесс остановлен".to_owned(), |c| format!("код {c}"));
            let tail: Vec<&str> = out
                .stderr_tail
                .iter()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .rev()
                .take(3)
                .collect();
            let tail = tail.into_iter().rev().collect::<Vec<_>>().join(" / ");
            Ok(ToolFact::Failed(if tail.is_empty() {
                code
            } else {
                format!("{code}: {tail}")
            }))
        }
        Err(HostError::Timeout) => Ok(ToolFact::TimedOut(deadline)),
        Err(HostError::Cancelled) => Err(CANCELLED.to_owned()),
        Err(HostError::Spawn(e)) => Ok(ToolFact::Failed(format!("не запускается: {e}"))),
        Err(e) => Ok(ToolFact::Failed(e.to_string())),
    }
}

/// Заголовок демки: не больше 64 байт, а не весь файл (PRE-07).
fn demo_fact(demo: &Demo) -> DemoFact {
    match demo {
        Demo::NotApplicable => DemoFact::Unchecked,
        Demo::Download => DemoFact::Download,
        Demo::File(path) => {
            let mut head = Vec::with_capacity(64);
            match std::fs::File::open(path).and_then(|f| f.take(64).read_to_end(&mut head)) {
                Ok(_) => DemoFact::Header(path.clone(), head),
                Err(_) => DemoFact::Missing(path.clone()),
            }
        }
    }
}

/// Путь результатов: годится ли для консоли HLAE и можно ли писать. Пробная запись - один байт в
/// файл с уникальным именем, который тут же удаляется; чужих файлов проверка не трогает (T-02-17).
fn out_fact(out_root: &Path) -> OutFact {
    let created = std::fs::create_dir_all(out_root)
        .map_err(|e| format!("не удалось создать папку {}: {e}", out_root.display()));
    let path = absolute_slash(out_root);
    let writable = created.and_then(|()| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let probe = out_root.join(format!(".cs2cinema-probe-{}-{nanos}", std::process::id()));
        std::fs::write(&probe, [0u8]).map_err(|e| format!("{}: {e}", probe.display()))?;
        std::fs::remove_file(&probe).map_err(|e| format!("{}: {e}", probe.display()))
    });
    OutFact {
        console_safe: renderer::console_safe(&path),
        path,
        writable,
    }
}

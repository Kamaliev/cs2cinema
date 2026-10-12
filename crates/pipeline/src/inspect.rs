//! Разбор недоверенной демки в дочернем процессе: ребёнок (`cs2-cli inspect`) ограничивает себя сам,
//! родитель не доверяет его ответу.
//!
//! Демку друга разбирает чужой парсер (`source2-demo`) на чужих байтах: он может съесть всю память, упасть
//! паникой или зависнуть. Поэтому раннер зовёт [`preflight`] до `run_job`: ребёнок печатает ровно одну JSON-строку
//! ([`InspectReport`]), а любой нештатный исход — падение, нехватка памяти, зависание, отсутствие JSON — родитель
//! превращает в `BAD_DEMO`, не падая сам и не ставя очередь на паузу «сбоем окружения».

use std::path::{Path, PathBuf};
use std::time::Duration;

use host::{CmdSpec, Host, HostError};
use serde::{Deserialize, Serialize};

use crate::record::failure::{self, FailCode};
use crate::record::log::one_line;
use crate::{CANDIDATES_MAX, CANCELLED, Progress, RecordFailure, RecordState, RunError, Source};

const MIB: u64 = 1024 * 1024;

/// Сколько родитель ждёт ребёнка сверх времени разбора, с.
const DEADLINE_SLACK_SECS: u64 = 30;
/// Длиннее этого ответ ребёнка не читается целиком: он и так не разберётся как JSON.
const REPLY_MAX: usize = 256 * 1024;
/// Сколько символов ответа показывается в сообщении о мусорном ответе.
const REPLY_SHOWN: usize = 120;
/// Потолок текста сообщения ребёнка в символах.
const MESSAGE_MAX: usize = 1000;
/// Потолок одной подписи кандидата «имя (SteamID64)» в символах.
const CANDIDATE_MAX: usize = cs2::text::NAME_MAX + 32;
/// Текст, когда ребёнок не уложился в память или время (или умер).
const NOT_FINISHED: &str = "разбор демки не завершился: не уложился в память или время";

/// Потолки разбора: файл и распаковка (`demo`), память ребёнка и время разбора.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InspectLimits {
    pub demo: demo::DemoLimits,
    /// Память дочернего процесса, байт.
    pub mem_bytes: u64,
    /// Время разбора, с; дедлайн родителя — на 30 с больше.
    pub secs: u64,
}

impl Default for InspectLimits {
    /// Стартовые числа (калибруются живым прогоном по `peak_mem_bytes`): файл 2000 МиБ, демка 3 ГиБ,
    /// память ребёнка 6 ГиБ, время разбора 15 минут.
    fn default() -> Self {
        Self { demo: demo::DemoLimits::default(), mem_bytes: 6 * 1024 * MIB, secs: 900 }
    }
}

/// Что разобрать: демка, игрок (SteamID64 — 0 значит «нет» — и ник) и потолки.
#[derive(Debug, Clone)]
pub struct InspectArgs {
    pub demo: PathBuf,
    pub steam: u64,
    pub nick: String,
    /// Снять позиции игроков (как это сделает план): проверяет, что и они разбираются.
    pub flybys: bool,
    pub limits: InspectLimits,
}

/// Единственная JSON-строка ответа ребёнка. Все поля необязательны: ответ недоверенный.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InspectReport {
    pub ok: bool,
    /// Строка `FailCode` при `ok == false`.
    pub code: String,
    pub message: String,
    /// Кандидаты («имя (SteamID64)») при `PLAYER_AMBIGUOUS`, игроки демки при `PLAYER_NOT_FOUND`.
    pub candidates: Vec<String>,
    /// Сколько кандидатов не вошло в список.
    pub more: usize,
    pub player_name: String,
    pub player_steam: u64,
    pub map: String,
    pub kills: usize,
    pub moments: usize,
    pub utility: usize,
    /// Сколько SteamID64 встречается у двух и более записей игроков.
    pub duplicate_xuids: usize,
    pub unpacked_bytes: u64,
    /// Пик памяти ребёнка, байт; 0 — неизвестно.
    pub peak_mem_bytes: u64,
}

impl InspectReport {
    /// Отказ с кодом `code` и текстом сбоя.
    fn refusal(code: FailCode, message: String) -> Self {
        InspectReport { ok: false, code: code.as_str().to_owned(), message, ..Default::default() }
    }

    /// Отказ из типизированной ошибки задачи; без типизации — `BAD_DEMO`.
    fn from_run_error(e: RunError) -> Self {
        match e.fail {
            Some(f) => Self::refusal(f.code, f.message),
            None => Self::refusal(FailCode::BadDemo, failure::msg_bad_demo(&e.message)),
        }
    }
}

/// Целая МиБ-величина для флагов ребёнка: округление вверх, чтобы ненулевой потолок не стал нулевым.
fn mib(bytes: u64) -> u64 {
    bytes.div_ceil(MIB)
}

/// Аргументы командной строки ребёнка: `inspect <путь> [--player <ник>] [--steam <id>] --max-file-mib N
/// --max-unpacked-mib N --mem-mib N --secs N [--no-flybys]`. Ник очищается так же, как имена в демке
/// (в аргументе процесса не бывает управляющих символов).
pub fn child_args(a: &InspectArgs) -> Vec<String> {
    let mut v = vec!["inspect".to_owned(), a.demo.to_string_lossy().into_owned()];
    let nick = cs2::text::clean_text(&a.nick, 256);
    if !nick.is_empty() {
        v.push("--player".into());
        v.push(nick);
    }
    if a.steam != 0 {
        v.push("--steam".into());
        v.push(a.steam.to_string());
    }
    v.push("--max-file-mib".into());
    v.push(mib(a.limits.demo.max_file).to_string());
    v.push("--max-unpacked-mib".into());
    v.push(mib(a.limits.demo.max_unpacked).to_string());
    v.push("--mem-mib".into());
    v.push(mib(a.limits.mem_bytes).to_string());
    v.push("--secs".into());
    v.push(a.limits.secs.to_string());
    if !a.flybys {
        v.push("--no-flybys".into());
    }
    v
}

/// Тело ребёнка: читает демку с потолками, разбирает, выбирает игрока (тем же `resolve_player`, что и задача) и
/// считает, что у него есть. Любой отказ — `ok: false` с кодом `FailCode`; сама функция не паникует на
/// штатных отказах, но паника парсера на враждебных байтах возможна — её ловит `cs2-cli inspect`.
pub fn inspect_demo(a: &InspectArgs, p: &dyn Progress) -> InspectReport {
    let analysis = match crate::analyze_with(&Source::File(a.demo.clone()), &a.limits.demo, p) {
        Ok(x) => x,
        Err(e) => return InspectReport::from_run_error(e),
    };
    let m = &analysis.m;
    let unpacked_bytes = analysis.data.len() as u64;
    let hero = match crate::resolve_player(m, Some(a.steam), &a.nick) {
        Ok(id) => id,
        Err(e) => {
            let (candidates, more) = match &e {
                crate::PlayerError::NotFound { players, more, .. } => (players.clone(), *more),
                crate::PlayerError::Ambiguous { candidates, more, .. } => (candidates.clone(), *more),
            };
            let mut rep = InspectReport::from_run_error(e.into_run_error());
            rep.candidates = candidates;
            rep.more = more;
            rep.map = m.map.clone();
            rep.kills = m.kills.len();
            rep.unpacked_bytes = unpacked_bytes;
            return rep;
        }
    };
    let name = m.name(hero);
    if let Err(e) = crate::ensure_highlights(m, hero) {
        let mut rep = InspectReport::from_run_error(e);
        rep.player_name = name;
        rep.map = m.map.clone();
        rep.kills = m.kills.len();
        rep.unpacked_bytes = unpacked_bytes;
        return rep;
    }
    let moments = highlights::for_player(m, hero, &highlights::Config::default()).len();
    let utility = highlights::utility_for(m, hero).len();

    let mut seen = std::collections::HashMap::<u64, usize>::new();
    for pl in m.players.iter().filter(|pl| pl.xuid != 0) {
        *seen.entry(pl.xuid).or_default() += 1;
    }
    let duplicate_xuids = seen.values().filter(|&&n| n >= 2).count();

    if a.flybys
        && let Err(e) = positions::track(&analysis.data, 4)
    {
        let mut rep = InspectReport::refusal(FailCode::BadDemo, failure::msg_bad_demo(&format!("позиции игроков не разобраны: {e}")));
        rep.player_name = name;
        rep.map = m.map.clone();
        rep.kills = m.kills.len();
        rep.unpacked_bytes = unpacked_bytes;
        return rep;
    }
    let player_steam = m.players.iter().find(|pl| pl.id == hero).map_or(0, |pl| pl.xuid);
    InspectReport {
        ok: true,
        player_name: name,
        player_steam,
        map: m.map.clone(),
        kills: m.kills.len(),
        moments,
        utility,
        duplicate_xuids,
        unpacked_bytes,
        ..Default::default()
    }
}

/// Итог проверки, прошедшей без отказа: игрок найден и ему есть что снимать.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preflight {
    pub player_name: String,
    pub player_steam: u64,
    /// Карта демки из ответа ребёнка, очищенная от управляющих символов; пуста, если ребёнок её не назвал.
    pub map: String,
    pub moments: usize,
    pub utility: usize,
    /// Пик памяти ребёнка, байт; 0 — неизвестно.
    pub peak_mem_bytes: u64,
}

/// Коды, которые вправе вернуть ребёнок; всё прочее родитель сводит к `BAD_DEMO`.
const ALLOWED: [FailCode; 6] = [
    FailCode::PlayerNotFound,
    FailCode::PlayerAmbiguous,
    FailCode::NoHighlights,
    FailCode::BadDemo,
    FailCode::DemoTooLarge,
    FailCode::PrecheckFailed,
];

fn code_from_child(s: &str) -> FailCode {
    FailCode::ALL.iter().copied().find(|c| c.as_str() == s && ALLOWED.contains(c)).unwrap_or(FailCode::BadDemo)
}

/// Типизированный отказ до записи.
fn failure_of(code: FailCode, message: String, elapsed: Duration) -> RunError {
    let f = RecordFailure::new(code, RecordState::Precheck, message, elapsed);
    RunError { message: f.message.clone(), fail: Some(f) }
}

fn bad_demo(message: String, elapsed: Duration) -> RunError {
    failure_of(FailCode::BadDemo, message, elapsed)
}

/// Код выхода для сообщения: отрицательные (NTSTATUS Windows) — шестнадцатеричными.
fn exit_text(code: i32) -> String {
    if code < 0 { format!("0x{:08X}", code as u32) } else { code.to_string() }
}

/// Отказ ребёнка, пересобранный родителем: ответу не верим ни по тексту, ни по длине списков.
fn refusal(rep: &InspectReport, a: &InspectArgs, elapsed: Duration) -> RunError {
    let code = code_from_child(&rep.code);
    let candidates: Vec<String> = rep
        .candidates
        .iter()
        .map(|c| cs2::text::clean_text(c, CANDIDATE_MAX))
        .filter(|c| !c.is_empty())
        .take(CANDIDATES_MAX)
        .collect();
    let more = rep.more.min(1_000_000) + rep.candidates.len().saturating_sub(CANDIDATES_MAX);
    let query = if a.nick.trim().is_empty() && a.steam != 0 { a.steam.to_string() } else { a.nick.clone() };
    let message = match code {
        FailCode::PlayerAmbiguous if !candidates.is_empty() => failure::msg_player_ambiguous(&query, &candidates, more),
        FailCode::PlayerNotFound => failure::msg_player_not_found(&query, &candidates, more),
        FailCode::NoHighlights if !rep.player_name.trim().is_empty() => {
            failure::msg_no_highlights(&cs2::text::clean_text(&rep.player_name, cs2::text::NAME_MAX))
        }
        _ => {
            let text = cs2::text::clean_text(&rep.message, MESSAGE_MAX);
            if text.is_empty() { failure::msg_bad_demo("разбор демки отказал без причины") } else { text }
        }
    };
    failure_of(code, message, elapsed)
}

/// Разобрать демку в дочернем `cs2-cli inspect` (`exe`) и принять его ответ. Дедлайн родителя — время
/// разбора плюс 30 с; `cancel` останавливает ребёнка. Родитель переживает любой исход ребёнка:
/// - отмена — `CANCELLED`; не запустился — `PRECHECK_FAILED` (неисправность окружения, не демки);
/// - таймаут, код выхода не 0, нет JSON-строки или она не разбирается — `BAD_DEMO`;
/// - отказ ребёнка — тот же код из разрешённого набора (иначе `BAD_DEMO`), текст очищен, кандидатов не больше 10.
pub fn preflight(host: &dyn Host, exe: &Path, args: &InspectArgs, cancel: &dyn Fn() -> bool) -> Result<Preflight, RunError> {
    let spec = CmdSpec {
        program: exe.to_path_buf(),
        args: child_args(args),
        cwd: None,
        deadline: Duration::from_secs(args.limits.secs.saturating_add(DEADLINE_SLACK_SECS)),
    };
    let started = host.now();
    // копим не строки, а две: последнюю похожую на JSON и последнюю вообще — вывод ребёнка не растёт в памяти
    let mut json_line: Option<String> = None;
    let mut last_line = String::new();
    let result = host.run(&spec, cancel, &mut |line| {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let capped: String = line.chars().take(REPLY_MAX).collect();
        if capped.starts_with('{') {
            json_line = Some(capped.clone());
        }
        last_line = capped;
    });
    let elapsed = host.now().saturating_sub(started);

    let out = match result {
        Ok(out) => out,
        Err(HostError::Cancelled) => return Err(RunError::from(CANCELLED)),
        Err(HostError::Timeout) => {
            return Err(bad_demo(format!("{NOT_FINISHED} (дедлайн {} с)", spec.deadline.as_secs()), elapsed));
        }
        Err(e) => {
            let text = one_line(&e.to_string());
            return Err(failure_of(FailCode::PrecheckFailed, format!("не удалось запустить разбор демки: {text}"), elapsed));
        }
    };
    if out.code != Some(0) {
        let code = out.code.map_or_else(|| "нет".to_owned(), exit_text);
        let tail = out.stderr_tail.last().map(|l| cs2::text::clean_text(&one_line(l), 200)).filter(|l| !l.is_empty());
        let mut text = format!("{NOT_FINISHED} (код выхода {code})");
        if let Some(t) = tail {
            text.push_str(&format!("; {t}"));
        }
        return Err(bad_demo(text, elapsed));
    }
    let shown = |s: &str| cs2::text::clean_text(&one_line(s), REPLY_SHOWN);
    let Some(json) = json_line else {
        let text = if last_line.is_empty() { "ответа нет".to_owned() } else { format!("«{}»", shown(&last_line)) };
        return Err(bad_demo(format!("разбор демки не дал ответа: {text}"), elapsed));
    };
    let rep: InspectReport = match serde_json::from_str(&json) {
        Ok(r) => r,
        Err(_) => return Err(bad_demo(format!("разбор демки ответил не JSON: «{}»", shown(&json)), elapsed)),
    };
    if !rep.ok {
        return Err(refusal(&rep, args, elapsed));
    }
    let player_name = cs2::text::clean_text(&rep.player_name, cs2::text::NAME_MAX);
    if player_name.is_empty() {
        return Err(bad_demo(failure::msg_bad_demo("разбор не назвал игрока"), elapsed));
    }
    let map = cs2::text::clean_text(&rep.map, cs2::text::NAME_MAX);
    Ok(Preflight { player_name, player_steam: rep.player_steam, map, moments: rep.moments, utility: rep.utility, peak_mem_bytes: rep.peak_mem_bytes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use host::fake::{FakeHost, Scenario};
    use host::CmdOutput;

    fn args(nick: &str) -> InspectArgs {
        InspectArgs { demo: PathBuf::from("match.dem"), steam: 0, nick: nick.into(), flybys: false, limits: InspectLimits::default() }
    }

    fn exit(code: Option<i32>, stderr: &[&str]) -> CmdOutput {
        CmdOutput { code, stderr_tail: stderr.iter().map(|s| (*s).to_owned()).collect(), elapsed: Duration::ZERO }
    }

    fn host_with(stdout: &[&str], out: Result<CmdOutput, HostError>) -> FakeHost {
        FakeHost::new(Scenario::happy(0).program("cs2-cli", stdout, out))
    }

    fn run(host: &FakeHost, a: &InspectArgs) -> Result<Preflight, RunError> {
        preflight(host, Path::new("cs2-cli"), a, &|| false)
    }

    fn fail_of(r: Result<Preflight, RunError>) -> RecordFailure {
        r.expect_err("ожидался отказ").fail.expect("типизированный сбой")
    }

    #[test]
    fn child_args_carry_nick_steam_limits_and_flags() {
        let mut a = args("s1mple\nquit");
        a.steam = 76561198034202275;
        let v = child_args(&a);
        assert_eq!(v[0], "inspect");
        assert_eq!(v[1], "match.dem");
        let pos = |f: &str| v.iter().position(|x| x == f).unwrap_or_else(|| panic!("нет {f}: {v:?}"));
        assert_eq!(v[pos("--player") + 1], "s1mplequit", "управляющие символы из ника убраны");
        assert_eq!(v[pos("--steam") + 1], "76561198034202275");
        assert_eq!(v[pos("--max-file-mib") + 1], "2000");
        assert_eq!(v[pos("--max-unpacked-mib") + 1], "3072");
        assert_eq!(v[pos("--mem-mib") + 1], "6144");
        assert_eq!(v[pos("--secs") + 1], "900");
        assert!(v.contains(&"--no-flybys".to_owned()));
        let mut b = args("");
        b.flybys = true;
        let v = child_args(&b);
        assert!(!v.contains(&"--player".to_owned()) && !v.contains(&"--steam".to_owned()) && !v.contains(&"--no-flybys".to_owned()), "{v:?}");
    }

    #[test]
    fn preflight_accepts_a_good_answer_and_gives_the_child_deadline_with_slack() {
        let json = r#"{"ok":true,"player_name":"s1mple","player_steam":76561198034202275,"moments":3,"utility":1,"peak_mem_bytes":1234}"#;
        let host = host_with(&["какой-то шум", json], Ok(exit(Some(0), &[])));
        let r = run(&host, &args("s1mple")).expect("игрок найден");
        assert_eq!(
            r,
            Preflight { player_name: "s1mple".into(), player_steam: 76561198034202275, map: String::new(), moments: 3, utility: 1, peak_mem_bytes: 1234 }
        );
        let runs = host.runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].2[0], "inspect");
    }

    #[test]
    fn preflight_reports_the_map() {
        let json = r#"{"ok":true,"player_name":"s1mple","player_steam":76561198034202275,"map":"de_mirage","moments":3,"utility":1,"peak_mem_bytes":1234}"#;
        let host = host_with(&[json], Ok(exit(Some(0), &[])));
        assert_eq!(run(&host, &args("s1mple")).expect("игрок найден").map, "de_mirage");

        // карта из враждебного ответа очищается от управляющих символов
        let json = r#"{"ok":true,"player_name":"s1mple","player_steam":1,"map":"de_\u001b[31mnuke\nquit","moments":1}"#;
        let host = host_with(&[json], Ok(exit(Some(0), &[])));
        let map = run(&host, &args("s1mple")).expect("игрок найден").map;
        assert!(!map.chars().any(char::is_control), "{map:?}");
        assert!(map.starts_with("de_"), "{map:?}");
    }

    #[test]
    fn preflight_keeps_a_typed_refusal_of_the_child() {
        let json = r#"{"ok":false,"code":"PLAYER_AMBIGUOUS","message":"что угодно","candidates":["s1mple (1)","s1mple_fan (2)"]}"#;
        let host = host_with(&[json], Ok(exit(Some(0), &[])));
        let f = fail_of(run(&host, &args("mple")));
        assert_eq!(f.code, FailCode::PlayerAmbiguous);
        assert_eq!(f.state, RecordState::Precheck);
        assert!(f.message.contains("s1mple (1)") && f.message.contains("s1mple_fan (2)") && f.message.contains("«mple»"), "{}", f.message);
        assert!(!f.hint.is_empty());
    }

    #[test]
    fn preflight_sanitizes_a_hostile_child_answer() {
        let candidates: Vec<String> = (0..50).map(|i| format!("игрок\u{7}{i}\nquit (7656119800000{i:04})")).collect();
        let report = InspectReport {
            ok: false,
            code: "PLAYER_AMBIGUOUS".into(),
            message: format!("ник «x»\u{0}\r\n{}", "я".repeat(10_000)),
            candidates,
            ..Default::default()
        };
        let json = serde_json::to_string(&report).unwrap();
        let host = host_with(&[json.as_str()], Ok(exit(Some(0), &[])));
        let f = fail_of(run(&host, &args("x\nexec evil")));
        assert_eq!(f.code, FailCode::PlayerAmbiguous);
        assert!(!f.message.chars().any(char::is_control), "{:?}", f.message);
        assert!(f.message.chars().count() <= 2000, "{}", f.message.chars().count());
        let listed = (0..50).filter(|i| f.message.contains(&format!("игрок{i}quit"))).count();
        assert_eq!(listed, CANDIDATES_MAX, "в тексте ровно {CANDIDATES_MAX} кандидатов");
        assert!(f.message.contains("и ещё 40"), "{}", f.message);

        // обычное сообщение тоже очищается и обрезается
        let report = InspectReport { ok: false, code: "BAD_DEMO".into(), message: format!("a\u{1b}[31m\n{}", "б".repeat(10_000)), ..Default::default() };
        let json = serde_json::to_string(&report).unwrap();
        let host = host_with(&[json.as_str()], Ok(exit(Some(0), &[])));
        let f = fail_of(run(&host, &args("x")));
        assert_eq!(f.code, FailCode::BadDemo);
        assert!(!f.message.chars().any(char::is_control) && f.message.chars().count() <= 2000);
    }

    #[test]
    fn preflight_turns_foreign_codes_crashes_timeouts_and_garbage_into_bad_demo() {
        // чужой код окружения от ребёнка
        let json = r#"{"ok":false,"code":"HLAE_ERROR","message":"HLAE ругается"}"#;
        let host = host_with(&[json], Ok(exit(Some(0), &[])));
        assert_eq!(fail_of(run(&host, &args("x"))).code, FailCode::BadDemo);

        // ответ из набора, но ok: true без игрока
        let host = host_with(&[r#"{"ok":true}"#], Ok(exit(Some(0), &[])));
        assert_eq!(fail_of(run(&host, &args("x"))).code, FailCode::BadDemo);

        // ребёнок упал (STATUS_STACK_BUFFER_OVERRUN) и ничего не напечатал
        let host = host_with(&[], Ok(exit(Some(-1073740791), &["thread 'main' panicked at …"])));
        let f = fail_of(run(&host, &args("x")));
        assert_eq!(f.code, FailCode::BadDemo);
        assert!(f.message.contains("не уложился в память или время") && f.message.contains("0xC0000409") && f.message.contains("panicked"), "{}", f.message);

        // даже с JSON, но с ненулевым кодом выхода: ответу не верим
        let json = r#"{"ok":true,"player_name":"s1mple"}"#;
        let host = host_with(&[json], Ok(exit(Some(3), &["сторож времени"])));
        assert_eq!(fail_of(run(&host, &args("x"))).code, FailCode::BadDemo);

        // процесс остановлен без кода
        let host = host_with(&[], Ok(exit(None, &[])));
        assert_eq!(fail_of(run(&host, &args("x"))).code, FailCode::BadDemo);

        // нет JSON
        let host = host_with(&["привет", "мир"], Ok(exit(Some(0), &[])));
        let f = fail_of(run(&host, &args("x")));
        assert_eq!(f.code, FailCode::BadDemo);
        assert!(f.message.contains("мир"), "{}", f.message);

        // ломаный JSON
        let host = host_with(&["{\"ok\": tru"], Ok(exit(Some(0), &[])));
        assert_eq!(fail_of(run(&host, &args("x"))).code, FailCode::BadDemo);

        // таймаут
        let host = FakeHost::new(Scenario::happy(0).run_result(Err(HostError::Timeout)));
        let f = fail_of(run(&host, &args("x")));
        assert_eq!(f.code, FailCode::BadDemo);
        assert!(f.message.contains("не уложился в память или время"), "{}", f.message);
    }

    #[test]
    fn preflight_keeps_precheck_failed_from_the_child() {
        let json = r#"{"ok":false,"code":"PRECHECK_FAILED","message":"не удалось ограничить память разбора: отказано в доступе"}"#;
        let host = host_with(&[json], Ok(exit(Some(0), &[])));
        let f = fail_of(run(&host, &args("x")));
        assert_eq!(f.code, FailCode::PrecheckFailed);
        assert!(f.message.contains("не удалось ограничить память разбора"), "{}", f.message);
    }

    #[test]
    fn preflight_does_not_blame_the_demo_when_the_child_cannot_start_and_obeys_cancel() {
        let host = FakeHost::new(Scenario::happy(0).run_result(Err(HostError::Spawn("нет файла".into()))));
        assert_eq!(fail_of(run(&host, &args("x"))).code, FailCode::PrecheckFailed);

        let host = host_with(&[], Ok(exit(Some(0), &[])));
        let e = preflight(&host, Path::new("cs2-cli"), &args("x"), &|| true).expect_err("отмена");
        assert_eq!(e.message, CANCELLED);
        assert!(e.fail.is_none());
    }

    fn temp_demo(tag: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cs2cinema-inspect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{tag}.dem"));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn synth(tag: &str) -> PathBuf {
        use demo::synth::{SynthKill, SynthPlayer, demo};
        let p = |userid, name: &str, xuid| SynthPlayer { userid, slot: userid, name: name.into(), xuid };
        let k = |tick, attacker, victim| SynthKill { tick, attacker, victim, headshot: true };
        let players = [p(0, "s1mple", 76561198034202275), p(1, "s1mple_fan", 76561198000000002), p(2, "ZywOo", 76561198000000003), p(3, "quiet", 76561198000000004)];
        let kills = [k(1400, 0, 1), k(1520, 0, 2), k(1640, 0, 1), k(1800, 2, 0)];
        temp_demo(tag, &demo("de_mirage", &players, &kills))
    }

    #[test]
    fn inspect_demo_finds_the_player_and_refuses_with_typed_codes() {
        let path = synth("typed");
        let mut a = args("s1mple");
        a.demo = path;
        let ok = inspect_demo(&a, &crate::Quiet);
        assert!(ok.ok, "{ok:?}");
        assert_eq!((ok.player_name.as_str(), ok.map.as_str()), ("s1mple", "de_mirage"));
        assert!(ok.moments >= 1 && ok.kills == 4 && ok.unpacked_bytes > 0, "{ok:?}");

        a.nick = "mple".into();
        let r = inspect_demo(&a, &crate::Quiet);
        assert_eq!((r.ok, r.code.as_str(), r.candidates.len()), (false, "PLAYER_AMBIGUOUS", 2), "{r:?}");

        a.nick = "nobody".into();
        assert_eq!(inspect_demo(&a, &crate::Quiet).code, "PLAYER_NOT_FOUND");

        a.nick = "quiet".into();
        assert_eq!(inspect_demo(&a, &crate::Quiet).code, "NO_HIGHLIGHTS");

        // SteamID сильнее ника
        a.nick = "mple".into();
        a.steam = 76561198000000003;
        let r = inspect_demo(&a, &crate::Quiet);
        assert!(r.ok && r.player_name == "ZywOo", "{r:?}");
    }

    #[test]
    fn inspect_demo_types_garbage_and_limits() {
        let mut a = args("s1mple");
        a.demo = temp_demo("garbage", b"hello");
        let r = inspect_demo(&a, &crate::Quiet);
        assert_eq!((r.ok, r.code.as_str()), (false, "BAD_DEMO"), "{r:?}");

        let mut a = args("s1mple");
        a.demo = synth("tiny-limit");
        a.limits.demo.max_file = 10;
        assert_eq!(inspect_demo(&a, &crate::Quiet).code, "DEMO_TOO_LARGE");

        let mut a = args("s1mple");
        a.demo = PathBuf::from("нет-такого-файла.dem");
        assert_eq!(inspect_demo(&a, &crate::Quiet).code, "BAD_DEMO");
    }

    #[test]
    fn analyze_with_types_garbage_and_an_empty_demo_as_bad_demo() {
        let limits = demo::DemoLimits::default();
        let e = crate::analyze_with(&Source::File(temp_demo("hello", b"hello")), &limits, &crate::Quiet).err().expect("мусор");
        assert_eq!(e.fail.as_ref().map(|f| f.code), Some(FailCode::BadDemo), "{}", e.message);

        let empty = demo::synth::demo("de_mirage", &[], &[]);
        let e = crate::analyze_with(&Source::File(temp_demo("empty", &empty)), &limits, &crate::Quiet).err().expect("без убийств");
        assert_eq!(e.fail.as_ref().map(|f| f.code), Some(FailCode::BadDemo), "{}", e.message);
        assert!(e.message.contains("убийств не найдено"), "{}", e.message);

        let tight = demo::DemoLimits { max_file: 10, ..limits };
        let e = crate::analyze_with(&Source::File(synth("too-big")), &tight, &crate::Quiet).err().expect("слишком большая");
        assert_eq!(e.fail.as_ref().map(|f| f.code), Some(FailCode::DemoTooLarge));
    }
}

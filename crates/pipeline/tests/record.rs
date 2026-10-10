//! Сценарии записи на фейковом хосте: без CS2, на виртуальных часах.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use pipeline::host::fake::{FakeHost, FakeMarker, Scenario};
use pipeline::host::{Host, LaunchSpec, Liveness, Source};
use pipeline::record::{
    Deadlines, Event, FailCode, RecordRequest, RecordState, mark_partial, record,
};
use pipeline::{Progress, Stage};
use renderer::TakePlan;

const NONCE: &str = "ab12cd34";

/// `n` дублей по 5 игровых секунд при 60 к/с.
fn takes(n: u32) -> Vec<TakePlan> {
    (0..n)
        .map(|i| TakePlan {
            index: i,
            shot: i as usize,
            game_secs: 5.0,
            capture_fps: 60,
        })
        .collect()
}

/// Запрос записи `n` дублей с настоящими дедлайнами; папка результата временная.
fn request(name: &str, n: u32) -> RecordRequest {
    let out_dir: PathBuf =
        std::env::temp_dir().join(format!("cs2cinema-record-{}-{name}", std::process::id()));
    RecordRequest {
        out_dir,
        launch: LaunchSpec {
            nonce: NONCE.to_owned(),
            demo_name: "x.dem".to_owned(),
            size: (1280, 720),
            ..LaunchSpec::default()
        },
        takes: takes(n),
        deadlines: Deadlines::default(),
        header: String::new(),
        afxerror_ignore: vec![],
    }
}

/// Собирает события записи в том порядке, в котором они пришли; умеет «отменить» задачу.
#[derive(Default)]
struct Capture {
    events: Mutex<Vec<Event>>,
    /// Отмена начинается после входа в это состояние.
    cancel_after: Option<RecordState>,
    cancel: AtomicBool,
    /// Вместе с отменой приходит принудительная (второе нажатие «Отмена»).
    force: bool,
    /// Фейк, по часам которого запоминается момент отмены.
    fake: Option<FakeHost>,
    cancelled_at: Mutex<Option<Duration>>,
}

impl Capture {
    fn cancelling_after(state: RecordState) -> Self {
        Capture {
            cancel_after: Some(state),
            ..Capture::default()
        }
    }

    /// Запоминать виртуальный момент отмены по часам этого фейка.
    fn watching(mut self, fake: &FakeHost) -> Self {
        self.fake = Some(fake.clone());
        self
    }

    /// Отмена сразу принудительная.
    fn forced(mut self) -> Self {
        self.force = true;
        self
    }

    /// Виртуальный момент, когда пришла отмена.
    fn cancelled_at(&self) -> Duration {
        self.cancelled_at
            .lock()
            .unwrap()
            .expect("отмена не приходила")
    }

    fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }

    fn lines(&self) -> Vec<String> {
        self.events().iter().map(|e| e.to_string()).collect()
    }

    /// Состояния лестницы в порядке входа (для дубля — с номером).
    fn states(&self) -> Vec<(RecordState, Option<(u32, u32)>)> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                Event::State { state, take } => Some((state, take)),
                _ => None,
            })
            .collect()
    }
}

impl Progress for Capture {
    fn stage(&self, _: Stage) {}
    fn log(&self, msg: &str) {
        self.events
            .lock()
            .unwrap()
            .push(Event::Fact(msg.to_owned()));
    }
    fn event(&self, e: &Event) {
        if let (Event::State { state, .. }, Some(at)) = (e, self.cancel_after)
            && *state == at
        {
            if let Some(fake) = &self.fake {
                self.cancelled_at.lock().unwrap().get_or_insert(fake.now());
            }
            self.cancel.store(true, Ordering::SeqCst);
        }
        self.events.lock().unwrap().push(e.clone());
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
    fn force_cancelled(&self) -> bool {
        self.force && self.cancel.load(Ordering::SeqCst)
    }
}

#[test]
fn fake_recording_walks_the_whole_ladder_and_returns_every_take() {
    let fake = FakeHost::new(Scenario::happy(3));
    let cap = Capture::default();
    let req = request("happy", 3);

    let report = record(&fake, &req, &cap).expect("запись на фейке должна пройти");
    assert_eq!(report.takes.len(), 3, "по таймингу на каждый дубль");

    let lines = cap.lines();
    assert_eq!(
        lines[0], "ожидаю дублей: 3 — take0000..take0002, в каждом video.mp4 и audio.wav",
        "ожидаемый набор дублей пишется до запуска лаунчера"
    );
    use RecordState::*;
    assert_eq!(
        cap.states(),
        vec![
            (Launcher, None),
            (Game, None),
            (Console, None),
            (Demo, None),
            (Script, None),
            (Take, Some((1, 3))),
            (Take, Some((2, 3))),
            (Take, Some((3, 3))),
            (Closing, None),
            (Done, None),
        ]
    );
    let sent = fake.sent();
    assert_eq!(
        sent.iter().filter(|l| *l == "exec highlights").count(),
        1,
        "exec ровно один раз: {sent:?}"
    );
    for i in 1..=3 {
        assert!(
            lines.contains(&format!("записан дубль {i}/3")),
            "нет строки о дубле {i}: {lines:?}"
        );
    }
}

#[test]
fn missing_ready_marker_fails_with_demo_not_loaded_after_the_deadline() {
    let fake = FakeHost::new(Scenario::happy(3).drop_marker(FakeMarker::Ready));
    let cap = Capture::default();
    let req = request("no-ready", 3);

    let real = Instant::now();
    let f = record(&fake, &req, &cap).expect_err("без READY запись обязана упасть");
    assert!(
        real.elapsed() < Duration::from_secs(2),
        "дедлайн 150 с идёт на виртуальных часах, а не в реальности"
    );
    assert_eq!(f.code, FailCode::DemoNotLoaded);
    assert_eq!(f.state, RecordState::Demo);
    let limit = req.deadlines.demo_load;
    assert!(
        f.elapsed >= limit && f.elapsed < limit + Duration::from_secs(1),
        "{:?}",
        f.elapsed
    );
    assert!(f.message.contains("демка не загрузилась"), "{}", f.message);
    assert!(!f.hint.is_empty());
}

type Outcome = Result<pipeline::RecordReport, pipeline::RecordFailure>;

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// Запись на фейке со сценарием: результат, события, фейк и запрос.
fn run(name: &str, sc: Scenario, n: u32) -> (Outcome, Capture, FakeHost, RecordRequest) {
    let fake = FakeHost::new(sc);
    let cap = Capture::default();
    let req = request(name, n);
    let r = record(&fake, &req, &cap);
    (r, cap, fake, req)
}

fn fail_of(r: Outcome) -> pipeline::RecordFailure {
    r.expect_err("запись обязана упасть")
}

#[test]
fn launcher_exit_code_is_reported_as_launcher_failed() {
    let sc = Scenario::happy(3).launcher_exit(Some(1), secs(2));
    let (r, ..) = run("launcher-code", sc, 3);
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::LauncherFailed);
    assert_eq!(f.state, RecordState::Launcher);
    assert!(
        f.message.contains("HLAE.exe завершился с кодом 1"),
        "{}",
        f.message
    );
    assert!(!f.hint.is_empty());
}

#[test]
fn launcher_that_never_exits_fails_after_the_launcher_deadline() {
    let (r, _, _, req) = run(
        "launcher-hang",
        Scenario::happy(3).launcher_never_exits(),
        3,
    );
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::LauncherFailed);
    assert_eq!(f.state, RecordState::Launcher);
    assert!(f.message.contains("не завершился за 30 с"), "{}", f.message);
    assert!(f.elapsed >= req.deadlines.launcher && f.elapsed < req.deadlines.launcher + secs(1));
}

#[test]
fn game_that_never_appears_fails_with_game_not_started() {
    let (r, ..) = run("no-game", Scenario::happy(3).game_never_appears(), 3);
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::GameNotStarted);
    assert_eq!(f.state, RecordState::Game);
    assert!(
        f.message
            .contains("CS2 не запустилась за 20 с после выхода HLAE"),
        "{}",
        f.message
    );
}

#[test]
fn console_that_never_opens_fails_with_console_not_open() {
    let (r, ..) = run("no-console", Scenario::happy(3).console_never_up(), 3);
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::ConsoleNotOpen);
    assert_eq!(f.state, RecordState::Console);
    assert!(
        f.elapsed >= secs(90) && f.elapsed < secs(91),
        "{:?}",
        f.elapsed
    );
    assert!(
        f.message.contains("консольный порт не открылся за 90 с"),
        "{}",
        f.message
    );
}

#[test]
fn missing_script_marker_fails_with_script_not_run_and_exec_is_sent_once() {
    let sc = Scenario::happy(3).drop_marker(FakeMarker::Script);
    let (r, _, fake, _) = run("no-script", sc, 3);
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::ScriptNotRun);
    assert_eq!(f.state, RecordState::Script);
    assert!(
        f.message.contains("метка SCRIPT не пришла за 30 с"),
        "{}",
        f.message
    );
    let execs = fake
        .sent()
        .iter()
        .filter(|l| *l == "exec highlights")
        .count();
    assert_eq!(
        execs,
        1,
        "повторной отправки по таймеру нет (D-10): {:?}",
        fake.sent()
    );
}

#[test]
fn missing_first_take_start_fails_with_take_timeout() {
    let sc = Scenario::happy(3).drop_marker(FakeMarker::TakeStart(0));
    let (r, ..) = run("no-take-start", sc, 3);
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::TakeTimeout);
    assert_eq!(f.state, RecordState::Take);
    assert!(
        f.message
            .contains("Дубль 1 из 3: метка TAKE_START не пришла за 90 с"),
        "{}",
        f.message
    );
}

#[test]
fn missing_take_end_fails_with_take_timeout_naming_the_take() {
    let sc = Scenario::happy(3).drop_marker(FakeMarker::TakeEnd(1));
    let (r, ..) = run("no-take-end", sc, 3);
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::TakeTimeout);
    assert!(
        f.message.contains("Дубль 2 из 3: метка TAKE_END не пришла"),
        "{}",
        f.message
    );
}

#[test]
fn all_done_does_not_stand_in_for_a_lost_last_take_end() {
    let sc = Scenario::happy(3).drop_marker(FakeMarker::TakeEnd(2));
    let (r, cap, fake, req) = run("all-done-vs-end", sc, 3);
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::TakeTimeout);
    assert_eq!(f.state, RecordState::Take);
    assert!(
        f.message.contains("Дубль 3 из 3: метка TAKE_END не пришла"),
        "{}",
        f.message
    );
    assert!(
        f.message
            .contains("пришла метка ALL_DONE вместо TAKE_END 2"),
        "{}",
        f.message
    );
    let limit = req.deadlines.take_run(&req.takes[2]);
    assert!(
        f.elapsed >= limit && f.elapsed < limit + secs(1),
        "дедлайн дубля не остановлен ALL_DONE: {:?} против {:?}",
        f.elapsed,
        limit
    );
    // `quit` уходит из поэтапного закрытия после сбоя (TMO-04), а не из машины,
    // поэтому он один, а в Closing машина не входила
    assert_eq!(
        fake.sent().iter().filter(|l| *l == "quit").count(),
        1,
        "{:?}",
        fake.sent()
    );
    assert!(
        !cap.states().iter().any(|(s, _)| *s == RecordState::Closing),
        "в Closing не переходили"
    );
}

#[test]
fn game_exit_mid_take_fails_with_game_exited() {
    // дубль 2 идёт с 18-й по 23-ю секунду виртуального времени
    let sc = Scenario::happy(3).game_dies_at(secs(20), Some(0xC0000005));
    let (r, ..) = run("game-dies", sc, 3);
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::GameExited);
    assert_eq!(f.state, RecordState::Take);
    assert!(
        f.message.contains("Игра закрылась на этапе «дубль»"),
        "{}",
        f.message
    );
    assert!(f.message.contains("дубль 2 из 3"), "{}", f.message);
}

#[test]
fn stale_nonce_echoes_and_duplicate_markers_are_ignored() {
    let sc = Scenario::happy(3)
        .line_at(secs(6), Source::Netcon, "CS2C ffff0000 SCRIPT")
        .line_at(secs(6), Source::Netcon, "CS2C ffff0000 READY")
        .duplicate_via_console_log();
    let (r, cap, fake, _) = run("stale-and-dups", sc, 3);
    let report = r.expect("чужие метки и повторы не мешают записи");
    assert_eq!(report.takes.len(), 3);
    let done = cap
        .events()
        .iter()
        .filter(|e| matches!(e, Event::TakeDone { .. }))
        .count();
    assert_eq!(done, 3, "каждый дубль засчитан один раз");
    let sent = fake.sent();
    assert_eq!(
        sent.len(),
        3,
        "запрос READY, exec, quit — и ничего лишнего: {sent:?}"
    );
    assert_eq!(sent[1], "exec highlights");
}

#[test]
fn late_console_gets_ready_through_the_direct_request() {
    let sc = Scenario::happy(3).ready_only_direct().line_at(
        secs(10),
        Source::ConsoleLog,
        "[HostStateManager] Host activate: Playing Demo (…)",
    );
    let (r, cap, fake, _) = run("late-console", sc, 3);
    r.expect("запись должна дойти до конца через прямой запрос READY");
    let direct = format!("echo CS2C {NONCE} READY");
    let sent = fake.sent();
    assert_eq!(
        sent.iter().filter(|l| **l == direct).count(),
        1,
        "прямой запрос один раз: {sent:?}"
    );
    assert_eq!(
        sent.iter().filter(|l| *l == "exec highlights").count(),
        1,
        "exec по-прежнему один раз"
    );
    assert!(
        cap.lines()
            .iter()
            .any(|l| l.contains("запрашиваю её напрямую")),
        "{:?}",
        cap.lines()
    );
}

#[test]
fn cancel_during_demo_load_returns_cancelled_and_terminates() {
    let fake = FakeHost::new(Scenario::happy(3).drop_marker(FakeMarker::Ready));
    let cap = Capture::cancelling_after(RecordState::Demo);
    let req = request("cancel", 3);
    let f = record(&fake, &req, &cap).expect_err("отмена");
    assert_eq!(f.code, FailCode::Cancelled);
    assert_eq!(f.state, RecordState::Demo);
    assert!(fake.terminated() >= 1, "сессия завершена");
}

#[test]
fn take_done_lines_keep_the_legacy_format() {
    assert_eq!(
        Event::TakeDone { n: 2, m: 3 }.to_string(),
        "записан дубль 2/3"
    );
}

/// Запрос записи в чистую папку: следы прошлых прогонов тест не читает.
fn fresh(name: &str, n: u32) -> RecordRequest {
    let req = request(name, n);
    let _ = std::fs::remove_dir_all(&req.out_dir);
    req
}

fn read_log(req: &RecordRequest) -> String {
    std::fs::read_to_string(req.out_dir.join("record.log"))
        .expect("record.log должен лежать в папке задачи")
}

#[test]
fn record_log_is_written_on_success_and_on_failure_with_source_tags() {
    let req = fresh("log-both", 2);
    let ok = record(
        &FakeHost::new(Scenario::happy(2)),
        &req,
        &Capture::default(),
    );
    ok.expect("первая запись проходит");
    let after_first = read_log(&req);
    assert_eq!(
        after_first.matches("==== запись").count(),
        1,
        "{after_first}"
    );
    assert!(
        after_first.contains(NONCE),
        "nonce в разделителе: {after_first}"
    );
    assert!(after_first.contains("[netcon]"), "{after_first}");
    assert!(after_first.contains("[app]"), "{after_first}");
    assert!(after_first.contains("→ exec highlights"), "{after_first}");
    assert!(
        after_first.contains("HLAE.exe: stdout — 0 строк, stderr — 0 строк"),
        "ноль строк HLAE — тоже явный факт: {after_first}"
    );
    assert!(
        after_first.contains("запись завершена: дублей 2"),
        "{after_first}"
    );

    // повторная попытка в ту же папку: журнал дописывается, а не перезаписывается
    let f = record(
        &FakeHost::new(Scenario::happy(2).console_never_up()),
        &req,
        &Capture::default(),
    )
    .expect_err("консоль не откроется");
    assert_eq!(f.code, FailCode::ConsoleNotOpen);
    let log = read_log(&req);
    assert!(log.starts_with(&after_first), "прошлая попытка на месте");
    assert_eq!(log.matches("==== запись").count(), 2, "{log}");
    assert_eq!(log.matches(NONCE).count() >= 2, true);
    assert_eq!(log.matches("stdout — 0 строк").count(), 2, "{log}");
    let tail = &log[after_first.len()..];
    for need in [
        "CONSOLE_NOT_OPEN",
        "консольный порт не открылся за 90 с",
        "Закройте CS2",
        "прошло 90 с",
    ] {
        assert!(tail.contains(need), "в итоге нет «{need}»: {tail}");
    }
    let _ = std::fs::remove_dir_all(&req.out_dir);
}

#[test]
fn cancelled_recording_still_leaves_a_complete_record_log() {
    let req = fresh("log-cancel", 3);
    let fake = FakeHost::new(Scenario::happy(3).drop_marker(FakeMarker::Ready));
    let cap = Capture::cancelling_after(RecordState::Demo);
    let f = record(&fake, &req, &cap).expect_err("отмена");
    assert_eq!(f.code, FailCode::Cancelled);
    let log = read_log(&req);
    let at = log.rfind("итог").expect("итог записан");
    assert!(log[at..].contains("CANCELLED"), "{log}");
    assert!(log[at..].contains("прошло"), "{log}");
    assert!(log.contains("stdout — 0 строк"), "{log}");
    let _ = std::fs::remove_dir_all(&req.out_dir);
}

#[test]
fn failure_tail_contains_recent_console_lines() {
    // консольный порт так и не открылся, поэтому строки идут только из console.log
    let sc = Scenario::happy(2)
        .console_never_up()
        .line_at(secs(10), Source::ConsoleLog, "[Demo] первая строка")
        .line_at(
            secs(11),
            Source::ConsoleLog,
            "10/09 01:37:32 [Demo] вторая строка",
        )
        .line_at(secs(12), Source::HlaeStderr, "hlae жалуется");
    let (r, ..) = run("tail-recent", sc, 2);
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::ConsoleNotOpen);
    assert!(!f.console_tail.is_empty());
    assert!(f.console_tail.len() <= 32, "{}", f.console_tail.len());
    assert!(
        f.console_tail.iter().any(|l| l == "[hlae] hlae жалуется"),
        "{:?}",
        f.console_tail
    );
    assert!(
        f.console_tail.iter().any(|l| l == "[Demo] вторая строка"),
        "префикс времени снят: {:?}",
        f.console_tail
    );
}

#[test]
fn console_log_lines_are_not_doubled_in_the_tail_while_netcon_is_up() {
    let sc = Scenario::happy(2)
        .duplicate_via_console_log()
        .drop_marker(FakeMarker::TakeEnd(1));
    let (r, ..) = run("tail-dups", sc, 2);
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::TakeTimeout);
    let starts = f
        .console_tail
        .iter()
        .filter(|l| l.contains("TAKE_START 1"))
        .count();
    assert_eq!(starts, 1, "{:?}", f.console_tail);
}

#[test]
fn game_that_never_appears_names_the_hlae_code_and_the_missing_game() {
    let (r, ..) = run("facts-no-game", Scenario::happy(3).game_never_appears(), 3);
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::GameNotStarted);
    assert!(
        f.message
            .ends_with("(HLAE.exe: код 0 · cs2.exe: не запускалась)"),
        "{}",
        f.message
    );
}

#[test]
fn game_exit_mid_take_reports_the_dead_game_with_its_code() {
    let sc = Scenario::happy(3).game_dies_at(secs(20), Some(0xC0000005));
    let (r, ..) = run("facts-game-dies", sc, 3);
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::GameExited);
    assert!(
        f.message.contains("cs2.exe: не жива (код 0xc0000005)"),
        "{}",
        f.message
    );
    assert!(
        f.message.contains("HLAE.exe: код 0"),
        "лаунчер назван отдельно: {}",
        f.message
    );
    assert_eq!(f.game, Liveness::Exited(Some(0xC0000005)));
}

#[test]
fn unknown_liveness_is_reported_as_unknown_never_alive() {
    let req = fresh("facts-unknown", 2);
    let sc = Scenario::happy(2)
        .liveness_override(Liveness::Unknown("отказано в доступе".to_owned()))
        .console_never_up();
    let fake = FakeHost::new(sc);
    let f = record(&fake, &req, &Capture::default()).expect_err("консоль не откроется");
    assert!(
        f.message
            .contains("cs2.exe: неизвестно (отказано в доступе)"),
        "{}",
        f.message
    );
    assert!(!f.message.contains("cs2.exe: жива"), "{}", f.message);
    let log = read_log(&req);
    assert!(
        log.contains("cs2.exe: неизвестно (отказано в доступе)"),
        "{log}"
    );
    assert!(!log.contains("cs2.exe: жива"), "{log}");
    let _ = std::fs::remove_dir_all(&req.out_dir);
}

const AFX: &str = "AFXERROR: COutFFMPEGVideoStream::COutFFMPEGVideoStream: CreateProcessW.";

#[test]
fn first_afxerror_fails_at_once_with_hlae_error() {
    // первая AFXERROR приходит на 10-й секунде, пока машина ждёт TAKE_START первого дубля
    let sc = Scenario::happy(2).line_at(secs(10), Source::ConsoleLog, AFX);
    let (r, _, fake, _) = run("afx-now", sc, 2);
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::HlaeError);
    assert_eq!(f.state, RecordState::Take);
    assert!(
        fake.now() < secs(13),
        "сбой не ждал дедлайна: {:?}",
        fake.now()
    );
    assert!(
        f.message.contains("HLAE сообщил об ошибке"),
        "{}",
        f.message
    );
    assert!(f.message.contains("CreateProcessW"), "{}", f.message);
    assert!(f.message.contains("«дубль»"), "{}", f.message);
    assert!(
        f.console_tail.iter().any(|l| l == AFX),
        "строка из console.log при живом netcon попала в хвост: {:?}",
        f.console_tail
    );
}

#[test]
fn ignored_afxerror_does_not_fail_the_recording() {
    let fake = FakeHost::new(Scenario::happy(2).line_at(secs(10), Source::ConsoleLog, AFX));
    let cap = Capture::default();
    let mut req = fresh("afx-ignored", 2);
    req.afxerror_ignore = vec!["createprocessw".to_owned()];
    let report = record(&fake, &req, &cap).expect("исключённая AFXERROR запись не роняет");
    assert_eq!(report.takes.len(), 2);
    assert!(
        cap.lines().iter().any(|l| l.contains("пропущена")),
        "{:?}",
        cap.lines()
    );
    let log = read_log(&req);
    assert!(log.contains("исключения AFXERROR: createprocessw"), "{log}");
    let _ = std::fs::remove_dir_all(&req.out_dir);
}

fn window(class: &str, title: &str, text: &str) -> pipeline::host::WindowInfo {
    pipeline::host::WindowInfo {
        pid: 4242,
        class: class.to_owned(),
        title: title.to_owned(),
        text: text.to_owned(),
    }
}

#[test]
fn hlae_error_dialog_fails_at_once_with_its_text() {
    let dialog = window(
        "#32770",
        "AfxHookSource2 - Error",
        "AFXERROR: ffmpeg path invalid. Тест кириллицы",
    );
    let sc = Scenario::happy(3)
        .console_never_up()
        .windows_at(secs(3), vec![dialog]);
    let (r, _, fake, _) = run("afx-dialog", sc, 3);
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::HlaeError);
    assert_eq!(f.state, RecordState::Console);
    assert!(
        fake.now() <= secs(5),
        "не дождались дедлайна: {:?}",
        fake.now()
    );
    assert!(
        f.message.contains("AfxHookSource2 - Error"),
        "{}",
        f.message
    );
    assert!(f.message.contains("Тест кириллицы"), "{}", f.message);
}

#[test]
fn ordinary_game_window_does_not_fail_but_lands_in_the_timeout_message() {
    let req = fresh("plain-window", 2);
    let sc = Scenario::happy(2)
        .console_never_up()
        .windows_at(secs(1), vec![window("SDL_app", "Counter-Strike 2", "")]);
    let fake = FakeHost::new(sc);
    let f = record(&fake, &req, &Capture::default()).expect_err("консоль не откроется");
    assert_eq!(f.code, FailCode::ConsoleNotOpen);
    assert!(
        f.message.contains("окна игры: «Counter-Strike 2»"),
        "{}",
        f.message
    );
    let log = read_log(&req);
    assert!(log.contains("окна:"), "{log}");
    assert!(log.contains("Counter-Strike 2"), "{log}");
    let _ = std::fs::remove_dir_all(&req.out_dir);
}

#[test]
fn afxerror_while_closing_also_fails() {
    // ALL_DONE и quit — на 16-й секунде, игра выходит на 18-й; ошибка ffmpeg HLAE приходит между
    let sc = Scenario::happy(1).line_at(secs(17), Source::ConsoleLog, AFX);
    let (r, ..) = run("afx-closing", sc, 1);
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::HlaeError);
    assert_eq!(f.state, RecordState::Closing);
}

#[test]
fn journal_reports_launcher_code_and_game_liveness_separately() {
    let (r, cap, ..) = run("facts-journal", Scenario::happy(2), 2);
    r.expect("запись проходит");
    let lines = cap.lines();
    let at = |needle: &str| {
        lines
            .iter()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("нет строки «{needle}»: {lines:#?}"))
    };
    at("HLAE.exe завершился с кодом 0");
    at("cs2.exe запущена (PID 4242)");
    at("дубль 1/2: 5 с записи на 5.0 с игры (×1.0)");
    at("дубль 2/2: 5 с записи на 5.0 с игры (×1.0)");
    at("cs2.exe завершилась (код 0)");
    assert!(
        lines
            .iter()
            .any(|l| l == "HLAE.exe: код 0 · cs2.exe: не жива (код 0)"),
        "итоговая строка фактов: {lines:#?}"
    );
}

#[test]
fn cancel_during_a_take_quits_waits_terminates_and_confirms_exit_within_ten_seconds() {
    let fake = FakeHost::new(Scenario::happy(3).ignore_quit());
    let cap = Capture::cancelling_after(RecordState::Take).watching(&fake);
    let req = fresh("cancel-take", 3);
    let f = record(&fake, &req, &cap).expect_err("отмена");
    assert_eq!(f.code, FailCode::Cancelled);
    assert_eq!(f.state, RecordState::Take);
    let spent = fake.now() - cap.cancelled_at();
    assert!(spent <= secs(10), "от отмены до возврата прошло {spent:?}");
    assert!(fake.sent().iter().any(|l| l == "quit"), "{:?}", fake.sent());
    assert_eq!(fake.terminated(), 1);
    let log = read_log(&req);
    for need in ["→ quit", "свой job завершён", "подтверждено по handle"]
    {
        assert!(log.contains(need), "в record.log нет «{need}»: {log}");
    }
    assert!(log.contains("не вышла за 5 с после quit"), "{log}");
    let _ = std::fs::remove_dir_all(&req.out_dir);
}

#[test]
fn force_cancel_skips_quit_and_terminates_at_once() {
    let fake = FakeHost::new(Scenario::happy(3).ignore_quit());
    let cap = Capture::cancelling_after(RecordState::Take)
        .watching(&fake)
        .forced();
    let req = fresh("force-cancel", 3);
    let f = record(&fake, &req, &cap).expect_err("принудительная отмена");
    assert_eq!(f.code, FailCode::Cancelled);
    assert!(
        !fake.sent().iter().any(|l| l == "quit"),
        "quit при принудительной отмене не уходит: {:?}",
        fake.sent()
    );
    let spent = fake.now() - cap.cancelled_at();
    assert!(spent <= secs(4), "прошло {spent:?}");
    assert_eq!(fake.terminated(), 1);
    let _ = std::fs::remove_dir_all(&req.out_dir);
}

#[test]
fn unfinished_take_is_renamed_partial_and_finished_ones_stay() {
    let req = fresh("partial", 3);
    let clips = req.out_dir.join("clips");
    // дубль 2 идёт с 18-й по 23-ю секунду виртуального времени, игра умирает на 20-й
    let sc = Scenario::happy(3)
        .clips_dir(clips.clone())
        .take_dirs_on_start()
        .game_dies_at(secs(20), Some(0));
    let f = record(&FakeHost::new(sc), &req, &Capture::default()).expect_err("игра умерла");
    assert_eq!(f.code, FailCode::GameExited);
    assert!(clips.join("take0000").join("video.mp4").is_file());
    assert!(clips.join("take0000").join("audio.wav").is_file());
    assert!(
        !clips.join("take0001").exists(),
        "оборванный дубль переименован"
    );
    assert!(clips.join("take0001.partial").join("video.mp4").is_file());
    assert!(
        read_log(&req).contains("дубль take0001 не дописан — помечен как take0001.partial"),
        "{}",
        read_log(&req)
    );
    let _ = std::fs::remove_dir_all(&req.out_dir);
}

#[test]
fn game_that_survives_terminate_is_reported_and_not_awaited_in_record() {
    let fake = FakeHost::new(Scenario::happy(3).ignore_quit().exit_after_terminate(None));
    let cap = Capture::cancelling_after(RecordState::Take).watching(&fake);
    let req = fresh("survivor", 3);
    let f = record(&fake, &req, &cap).expect_err("отмена");
    assert_eq!(f.code, FailCode::Cancelled);
    let d = &req.deadlines;
    let spent = fake.now() - cap.cancelled_at();
    assert!(
        spent <= d.quit_wait + d.kill_confirm + secs(1),
        "возврат позже бюджета: {spent:?}"
    );
    assert!(
        read_log(&req).contains("подождёт её до 30 с"),
        "{}",
        read_log(&req)
    );
    let _ = std::fs::remove_dir_all(&req.out_dir);
}

#[test]
fn mark_partial_touches_only_take_dirs() {
    let clips = std::env::temp_dir().join(format!("cs2cinema-partial-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&clips);
    for d in ["take0000", "take0001", "takeX"] {
        std::fs::create_dir_all(clips.join(d)).unwrap();
    }
    std::fs::write(clips.join("plan.hash"), b"h").unwrap();
    let notes = mark_partial(&clips, &[0]);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(clips.join("take0000").is_dir(), "готовый дубль на месте");
    assert!(!clips.join("take0001").exists());
    assert!(clips.join("take0001.partial").is_dir());
    assert!(clips.join("takeX").is_dir(), "чужое имя не трогаем");
    assert!(clips.join("plan.hash").is_file());
    assert!(mark_partial(&clips.join("нет-такой"), &[]).is_empty());
    let _ = std::fs::remove_dir_all(&clips);
}

#[test]
fn success_waits_for_job_empty_after_the_game_exits() {
    // TAKE_END последнего дубля — 23-я секунда, quit -> выход игры на 25-й, пустой job на 35-й
    let sc = Scenario::happy(2).job_empty_after_exit(Some(secs(10)));
    let req = fresh("job-empty-late", 2);
    let fake = FakeHost::new(sc);
    record(&fake, &req, &Capture::default()).expect("запись удачна");
    assert!(
        fake.now() >= secs(35),
        "последний дубль не обрываем до конца job: {:?}",
        fake.now()
    );
    let log = read_log(&req);
    let empty = log
        .find("в job не осталось процессов")
        .expect("job опустел");
    let ended = log.find("свой job завершён").expect("job завершён");
    assert!(empty < ended, "сначала пустой job, потом завершение: {log}");
    let _ = std::fs::remove_dir_all(&req.out_dir);
}

#[test]
fn job_that_never_empties_after_exit_is_closed_after_game_close() {
    let sc = Scenario::happy(2).job_empty_after_exit(None);
    let (r, cap, fake, req) = run("job-never-empty", sc, 2);
    r.expect("зависший ffmpeg HLAE не делает запись сбоем");
    // игра вышла на 25-й секунде, ещё `game_close` ждём пустой job
    assert!(
        fake.now() >= secs(25) + req.deadlines.game_close,
        "{:?}",
        fake.now()
    );
    assert!(
        cap.lines().iter().any(|l| l.contains("остались процессы")),
        "{:?}",
        cap.lines()
    );
}

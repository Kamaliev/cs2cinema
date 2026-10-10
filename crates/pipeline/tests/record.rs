//! Сценарии записи на фейковом хосте: без CS2, на виртуальных часах.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pipeline::host::LaunchSpec;
use pipeline::host::fake::{FakeHost, FakeMarker, Scenario};
use pipeline::record::{Deadlines, Event, FailCode, RecordRequest, RecordState, record};
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
    }
}

/// Собирает события записи в том порядке, в котором они пришли.
#[derive(Default)]
struct Capture {
    events: Mutex<Vec<Event>>,
}

impl Capture {
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
        self.events.lock().unwrap().push(e.clone());
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

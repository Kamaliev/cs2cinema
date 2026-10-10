//! Заглушка хоста для не-Windows: запись игры там невозможна, но крейт и всё, что от него зависит, собираются.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::{CmdOutput, CmdSpec, Host, HostError, LaunchSpec, Session};

/// Хост, который отказывает в любом запуске.
pub struct UnsupportedHost;

impl Host for UnsupportedHost {
    fn launch(&self, _spec: &LaunchSpec) -> Result<Box<dyn Session>, HostError> {
        Err(HostError::Unsupported)
    }

    fn now(&self) -> Duration {
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed()
    }

    fn run(
        &self,
        cmd: &CmdSpec,
        cancel: &dyn Fn() -> bool,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<CmdOutput, HostError> {
        // ffmpeg склейки работает и вне Windows (CI): дедлайн нужен везде, не только игре
        crate::runner::run_bounded(cmd, cancel, on_line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_host_refuses_to_launch() {
        let host = UnsupportedHost;
        let Err(e) = host.launch(&LaunchSpec::default()) else {
            panic!("запуск на заглушке не должен удаваться")
        };
        assert_eq!(e, HostError::Unsupported);
        assert!(e.to_string().contains("только в Windows"));
        let run = host.run(
            &CmdSpec {
                program: "cs2cinema-no-such-program".into(),
                args: vec![],
                cwd: None,
                deadline: Duration::from_secs(1),
            },
            &|| false,
            &mut |_| {},
        );
        // утилиты запускаются и без игры (ffmpeg склейки): ошибка запуска, а не «не поддерживается»
        assert!(matches!(run, Err(HostError::Spawn(_))), "{run:?}");
        let a = host.now();
        assert!(host.now() >= a, "часы должны быть монотонными");
    }

    #[test]
    fn unsupported_host_has_no_environment_facts() {
        let host = UnsupportedHost;
        assert_eq!(host.processes(), Err(HostError::Unsupported));
        assert_eq!(
            host.free_space(std::path::Path::new(".")),
            Err(HostError::Unsupported)
        );
        assert_eq!(host.file_version(std::path::Path::new("HLAE.exe")), None);
        assert_eq!(
            host.wait_previous_game(Duration::from_secs(1), Duration::ZERO, &|| false),
            crate::PrevGame::Clear {
                waited: Duration::ZERO
            }
        );
    }
}

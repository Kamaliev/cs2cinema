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
        _cmd: &CmdSpec,
        _cancel: &dyn Fn() -> bool,
        _on_line: &mut dyn FnMut(&str),
    ) -> Result<CmdOutput, HostError> {
        Err(HostError::Unsupported)
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
                program: "x".into(),
                args: vec![],
                cwd: None,
                deadline: Duration::from_secs(1),
            },
            &|| false,
            &mut |_| {},
        );
        assert_eq!(run, Err(HostError::Unsupported));
        let a = host.now();
        assert!(host.now() >= a, "часы должны быть монотонными");
    }
}

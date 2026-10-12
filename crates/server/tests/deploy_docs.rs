//! Заготовки docs/deploy не расходятся с кодом сервера: образец настроек принимается загрузчиком,
//! юнит и compose держат нужные охранные строки, документ не советует открывать порты наружу.

use server::config::{self, Flags};
use std::path::Path;

const SERVICE: &str = include_str!("../../../docs/deploy/cs2-server.service");
const COMPOSE: &str = include_str!("../../../docs/deploy/docker-compose.yml");
const BACKUP_SERVICE: &str = include_str!("../../../docs/deploy/cs2-server-backup.service");
const BACKUP_TIMER: &str = include_str!("../../../docs/deploy/cs2-server-backup.timer");
const README: &str = include_str!("../../../docs/deploy/README.md");
const CHECK_HOST: &str = include_str!("../../../docs/deploy/check-host.sh");

/// Подставной токен: только форма настоящего, к Telegram он отношения не имеет.
const FAKE_TOKEN: &str = "123456789:AAEdeploydocsDEPLOYDOCSdeploydocsDEPLOY01";

fn deploy_config_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/deploy/cs2-server.json")
}

#[test]
fn deploy_config_is_accepted_and_keeps_secrets_out() {
    let flags = Flags {
        config: Some(deploy_config_path()),
        ..Default::default()
    };
    let loaded = config::load(&flags, &|_| None).expect("образец развёртывания должен приниматься");
    let c = &loaded.config;
    assert_eq!(c.bind, "127.0.0.1:8080", "сервер слушает только loopback");
    assert!(
        c.data_dir.ends_with("cs2-server"),
        "{}",
        c.data_dir.display()
    );
    assert!(c.telegram_bot_token.is_none(), "токена в файле нет");
    assert!(c.bot_api_url.is_none(), "адреса bot-api в файле нет");
    assert_eq!(c.retention_demo_hours, 24);

    // Служебные команды оператора работают без токена; бот включается окружением службы.
    let env = |k: &str| match k {
        "CS2_SERVER_BOT_API_URL" => Some("http://127.0.0.1:8081".to_owned()),
        "TELEGRAM_BOT_TOKEN" => Some(FAKE_TOKEN.to_owned()),
        _ => None,
    };
    let with_bot =
        config::load(&flags, &env).expect("с окружением службы образец тоже принимается");
    assert!(with_bot.config.bot_api_url.is_some() && with_bot.config.telegram_bot_token.is_some());
}

#[test]
fn service_runs_as_a_plain_user_and_restarts_after_failures() {
    for needle in [
        "User=cs2-server",
        "Restart=on-failure",
        "StartLimitIntervalSec=0",
        "EnvironmentFile=/etc/cs2-server/env",
        "ProtectSystem=strict",
        "NoNewPrivileges=yes",
        "ExecStart=/usr/local/bin/cs2-server serve --config /etc/cs2-server/cs2-server.json",
    ] {
        assert!(SERVICE.contains(needle), "в юните нет «{needle}»");
    }
    assert!(
        !SERVICE.contains("DynamicUser"),
        "каталог данных должен быть читаем bot-api: динамический пользователь не подходит"
    );
}

#[test]
fn compose_publishes_bot_api_only_on_loopback() {
    assert!(COMPOSE.contains("127.0.0.1:8081:8081"));
    for line in COMPOSE.lines().filter(|l| l.contains("8081:")) {
        assert!(
            line.contains("127.0.0.1:"),
            "порт bot-api не на loopback: {line}"
        );
    }
    for needle in [
        "restart: unless-stopped",
        "max-size",
        "botapi-janitor",
        "user: \"101:101\"",
        ":ro",
    ] {
        assert!(COMPOSE.contains(needle), "в compose нет «{needle}»");
    }
}

#[test]
fn backup_timer_runs_the_backup_subcommand_daily() {
    assert!(BACKUP_SERVICE.contains("backup /var/backups/cs2-server --keep 14"));
    assert!(BACKUP_SERVICE.contains("User=cs2-server"));
    for needle in [
        "OnCalendar=daily",
        "Persistent=true",
        "WantedBy=timers.target",
    ] {
        assert!(BACKUP_TIMER.contains(needle), "в таймере нет «{needle}»");
    }
}

#[test]
fn readme_never_exposes_the_server_port() {
    for needle in [
        "tailscale serve --bg --https=443 http://127.0.0.1:8080",
        "reverse_proxy 127.0.0.1:8080",
        "nmap",
        "systemctl reboot",
        "check-host.sh",
        "/etc/cs2-server/env",
        "cs2-server-backup.timer",
    ] {
        assert!(README.contains(needle), "в документе нет «{needle}»");
    }
    for forbidden in [
        "0.0.0.0:8080",
        "ufw allow 8080",
        "ufw allow 8081",
        "http://100.",
    ] {
        assert!(
            !README.contains(forbidden),
            "документ советует опасное: «{forbidden}»"
        );
    }
    // Шестнадцать нумерованных разделов на месте, в заданном порядке.
    let mut from = 0;
    for n in 1..=16 {
        let head = format!("\n## {n}. ");
        let at = README[from..]
            .find(&head)
            .unwrap_or_else(|| panic!("нет раздела {n} после позиции {from}"));
        from += at + head.len();
    }
}

#[test]
fn check_host_script_covers_services_ports_and_backups() {
    assert!(CHECK_HOST.starts_with("#!/usr/bin/env bash"));
    for needle in [
        "set -u",
        "ss -tlnp",
        "systemctl is-active cs2-server",
        "/v1/runner/hello",
        "/var/backups/cs2-server",
        "stat -c %a",
    ] {
        assert!(CHECK_HOST.contains(needle), "в скрипте нет «{needle}»");
    }
}

#[test]
fn deploy_files_have_unix_line_endings() {
    for (name, text) in [
        ("check-host.sh", CHECK_HOST),
        ("cs2-server.service", SERVICE),
        ("cs2-server-backup.service", BACKUP_SERVICE),
        ("docker-compose.yml", COMPOSE),
    ] {
        assert!(!text.contains('\r'), "{name}: CRLF сломает файл на Linux");
    }
}

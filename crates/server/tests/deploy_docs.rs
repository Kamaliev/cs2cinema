//! Заготовки docs/deploy не расходятся с кодом сервера: образец настроек принимается загрузчиком,
//! юнит и compose держат нужные охранные строки, документ не советует открывать порты наружу.

use server::config::{self, Flags};
use std::path::Path;

const SERVICE: &str = include_str!("../../../docs/deploy/cs2-server.service");
const COMPOSE: &str = include_str!("../../../docs/deploy/docker-compose.yml");

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

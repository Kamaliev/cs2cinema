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
// Вариант В: всё в Docker (docs/deploy/docker).
const D_COMPOSE: &str = include_str!("../../../docs/deploy/docker/docker-compose.yml");
const D_DOCKERFILE: &str = include_str!("../../../docs/deploy/docker/Dockerfile");
const D_CADDYFILE: &str = include_str!("../../../docs/deploy/docker/Caddyfile");
const D_ENV_EXAMPLE: &str = include_str!("../../../docs/deploy/docker/.env.example");
const D_README: &str = include_str!("../../../docs/deploy/docker/README.md");
const D_IGNORE: &str = include_str!("../../../.dockerignore");
const GITIGNORE: &str = include_str!("../../../.gitignore");

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

// ---------------------------------------------------------------------------------------------
// Вариант В: всё в Docker (docs/deploy/docker)
// ---------------------------------------------------------------------------------------------

/// Записи `ports:` compose с именем службы: (служба, значение без кавычек). Разбор по отступам,
/// без YAML-крейта: файл написан в одном стиле (службы на два пробела, элементы списка на шесть).
fn compose_ports(text: &str) -> Vec<(String, String)> {
    let (mut service, mut in_ports, mut in_services) = (String::new(), false, false);
    let mut out = Vec::new();
    for line in text.lines() {
        if line.starts_with("services:") {
            in_services = true;
            continue;
        }
        if in_services && !line.is_empty() && !line.starts_with(' ') && !line.starts_with('#') {
            in_services = false;
        }
        if !in_services {
            continue;
        }
        if let Some(name) = line
            .strip_prefix("  ")
            .and_then(|l| l.strip_suffix(':'))
            .filter(|n| !n.starts_with(' ') && !n.starts_with('#'))
        {
            service = name.to_owned();
            in_ports = false;
            continue;
        }
        if line.starts_with("    ports:") {
            in_ports = true;
            continue;
        }
        if in_ports {
            let t = line.trim();
            if let Some(v) = t.strip_prefix("- ") {
                out.push((service.clone(), v.trim_matches('"').to_owned()));
            } else if !t.is_empty() && !t.starts_with('#') {
                in_ports = false;
            }
        }
    }
    out
}

/// Похоже ли слово на токен бота: 8-10 цифр, двоеточие, не меньше 30 символов.
fn looks_like_bot_token(word: &str) -> bool {
    word.split_once(':').is_some_and(|(a, b)| {
        (8..=10).contains(&a.len()) && a.bytes().all(|c| c.is_ascii_digit()) && b.len() >= 30
    })
}

#[test]
fn docker_compose_opens_only_caddy_ports_to_the_world() {
    let ports = compose_ports(D_COMPOSE);
    assert!(
        ports.len() >= 4,
        "разбор ports: нашёл {} записей: {ports:?}",
        ports.len()
    );
    for (service, port) in &ports {
        let loopback = port.starts_with("127.0.0.1:");
        let caddy_web = service == "caddy" && (port == "80:80" || port == "443:443");
        assert!(
            loopback || caddy_web,
            "служба {service} публикует порт {port} не на loopback: Docker обходит ufw"
        );
    }
    for needle in ["127.0.0.1:8080:8080", "127.0.0.1:8081:8081"] {
        assert!(
            ports
                .iter()
                .any(|(s, p)| s == "telegram-bot-api" && p == needle),
            "у владельца сети нет «{needle}»"
        );
    }
    for web in ["80:80", "443:443"] {
        assert!(
            ports.iter().any(|(s, p)| s == "caddy" && p == web),
            "у caddy нет «{web}»"
        );
    }
    assert!(
        !ports.iter().any(|(s, _)| s == "cs2-server"),
        "cs2-server делит сеть с bot-api: порты у него указывать нельзя"
    );
}

#[test]
fn docker_compose_wires_server_to_loopback_bot_api_and_keeps_secrets_out() {
    for needle in [
        "network_mode: \"service:telegram-bot-api\"",
        "restart: unless-stopped",
        "stop_grace_period: 15s",
        "user: \"1500:1500\"",
        "no-new-privileges:true",
        "/var/lib/cs2-server:/var/lib/cs2-server",
        "/var/lib/telegram-bot-api:/var/lib/telegram-bot-api",
        "./cs2-server.json:/etc/cs2-server/cs2-server.json:ro",
        "dockerfile: docs/deploy/docker/Dockerfile",
        "image: caddy:2",
        "caddy_data:/data",
        "max-size",
        "botapi-janitor",
        "user: \"101:101\"",
    ] {
        assert!(D_COMPOSE.contains(needle), "в compose нет «{needle}»");
    }
    // Секреты приходят только из .env: обязательные переменные и ни одного значения в файле.
    for var in ["TELEGRAM_BOT_TOKEN", "TELEGRAM_API_ID", "TELEGRAM_API_HASH"] {
        assert!(
            D_COMPOSE.contains(&format!("${{{var}:?")),
            "{var} должен быть обязательной переменной из .env"
        );
    }
    assert!(
        !D_COMPOSE.contains("env_file"),
        "env_file отдал бы токен бота всем службам: переменные передаются поимённо"
    );
    assert!(
        !D_COMPOSE
            .split(|c: char| c.is_whitespace() || c == '"' || c == '=')
            .any(looks_like_bot_token),
        "в compose похоже на токен бота"
    );

    // Адрес bot-api из compose принимается проверкой сервера (она требует loopback).
    let url = D_COMPOSE
        .lines()
        .find_map(|l| l.trim().strip_prefix("CS2_SERVER_BOT_API_URL:"))
        .map(|v| v.trim().to_owned())
        .expect("в compose нет CS2_SERVER_BOT_API_URL");
    server::tg::http::check_base_url(&url)
        .unwrap_or_else(|e| panic!("сервер не примет адрес bot-api из compose: {e}"));

    // Копия настроек для Docker принимается вместе с окружением compose, а корень файлов bot-api
    // совпадает с монтированием.
    let flags = Flags {
        config: Some(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/deploy/docker/cs2-server.json"),
        ),
        bind: Some("0.0.0.0:8080".to_owned()),
        ..Default::default()
    };
    let env = |k: &str| match k {
        "CS2_SERVER_BOT_API_URL" => Some(url.clone()),
        "TELEGRAM_BOT_TOKEN" => Some(FAKE_TOKEN.to_owned()),
        _ => None,
    };
    let loaded = config::load(&flags, &env).expect("настройки варианта В должны приниматься");
    let c = &loaded.config;
    assert_eq!(
        c.bind, "0.0.0.0:8080",
        "внутри контейнера сервер слушает все адреса"
    );
    assert_eq!(c.data_dir, Path::new("/var/lib/cs2-server"));
    assert_eq!(
        c.bot_api_files_root.as_deref(),
        Some(Path::new("/var/lib/telegram-bot-api"))
    );
    assert!(
        c.telegram_bot_token.is_some() && c.bot_api_url.is_some(),
        "бот включается окружением"
    );
    assert_eq!(c.retention_demo_hours, 24, "сроки хранения A1");
    assert!(
        D_COMPOSE.contains("\"serve\", \"--bind\", \"0.0.0.0:8080\""),
        "команда запуска должна задавать адрес для Caddy"
    );
}

#[test]
fn docker_image_is_glibc_with_fixed_user_and_clean_env_example() {
    let from: Vec<&str> = D_DOCKERFILE
        .lines()
        .filter(|l| l.starts_with("FROM "))
        .collect();
    assert_eq!(
        from.len(),
        2,
        "ожидались этап сборки и рабочий образ: {from:?}"
    );
    assert!(
        from.iter().all(|l| !l.to_lowercase().contains("alpine")),
        "alpine (musl) не годится: сервер собирается под glibc"
    );
    assert!(from[1].contains("debian:bookworm-slim"), "{}", from[1]);
    for needle in [
        "cargo build --release -p server",
        "ca-certificates",
        "--gid 1500",
        "--uid 1500",
        "USER cs2-server",
        "ENTRYPOINT [\"cs2-server\"]",
        "CMD [\"serve\"]",
    ] {
        assert!(D_DOCKERFILE.contains(needle), "в Dockerfile нет «{needle}»");
    }
    for needle in [
        "target/",
        ".git/",
        ".planning/",
        "out/",
        "maps/",
        "*.dem",
        "*.dem.zst",
    ] {
        assert!(
            D_IGNORE.lines().any(|l| l.trim() == needle),
            "в .dockerignore нет «{needle}»: контекст сборки раздуется"
        );
    }

    // .env.example: четыре ключа и ни одного значения; настоящий .env в git не попадает.
    let keys: Vec<&str> = D_ENV_EXAMPLE
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    assert_eq!(
        keys,
        [
            "TELEGRAM_BOT_TOKEN=",
            "TELEGRAM_API_ID=",
            "TELEGRAM_API_HASH=",
            "CS2_DOMAIN="
        ],
        ".env.example: ровно четыре ключа и без значений"
    );
    assert!(
        GITIGNORE
            .lines()
            .any(|l| l.trim() == "docs/deploy/docker/.env"),
        "docs/deploy/docker/.env должен быть в .gitignore"
    );
}

#[test]
fn docker_caddyfile_and_readme_keep_the_proxy_rules() {
    for needle in [
        "{$CS2_DOMAIN}",
        "reverse_proxy cs2-server:8080",
        "flush_interval -1",
        "max_size 10MB",
        "read_header 10s",
        "read_body 5m",
        "write 5m",
        "idle 2m",
    ] {
        assert!(D_CADDYFILE.contains(needle), "в Caddyfile нет «{needle}»");
    }
    for needle in [
        "docker compose up -d --build",
        "cp .env.example .env",
        "chmod 600 .env",
        "exec -T cs2-server",
        "docker compose logs",
        "docker kill -s KILL cs2-server",
        "restart cs2-server",
        "cron",
        "ufw",
    ] {
        assert!(
            D_README.contains(needle),
            "в README варианта В нет «{needle}»"
        );
    }
    for forbidden in [
        "ufw allow 8080",
        "ufw allow 8081",
        "\"8080:8080\"",
        "\"8081:8081\"",
    ] {
        assert!(
            !D_README.contains(forbidden),
            "документ советует опасное: «{forbidden}»"
        );
    }
    assert!(
        README.contains("docker/README.md"),
        "основной документ ссылается на вариант В"
    );
}

#[test]
fn docker_files_have_unix_line_endings() {
    for (name, text) in [
        ("docker/docker-compose.yml", D_COMPOSE),
        ("docker/Dockerfile", D_DOCKERFILE),
        ("docker/Caddyfile", D_CADDYFILE),
        ("docker/.env.example", D_ENV_EXAMPLE),
        (".dockerignore", D_IGNORE),
    ] {
        assert!(!text.contains('\r'), "{name}: CRLF сломает файл на Linux");
    }
}

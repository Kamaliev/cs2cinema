# Образ cs2-server для варианта В (всё в Docker). Сборка из исходников репозитория:
#   docker compose up -d --build        (контекст сборки — корень репозитория, см. docker-compose.yml)
#
# Этап сборки и рабочий образ — на одном дистрибутиве (Debian 12, glibc 2.36), поэтому
# «version GLIBC_2.xx not found» не бывает. alpine (musl) не годится: бинарник собирается под glibc.
FROM rust:1-bookworm AS build
WORKDIR /src
# Нужен весь репозиторий: Cargo читает манифесты всех участников рабочей области и vendor/tiny_http.
# Собирается только крейт server (-p server), Windows-крейты (app, host, ...) не компилируются.
# Лишнее (target, .git, демки, .planning) отсекает корневой .dockerignore.
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release -p server \
 && install -D -m 0755 target/release/cs2-server /out/cs2-server

FROM debian:bookworm-slim
# ca-certificates: сервер ходит по HTTPS (скачивание файлов и вызовы Telegram).
# Пользователь с фиксированными uid/gid 1500: этим же числом владеют папки данных на хосте
# (их готовит служба prepare в docker-compose.yml).
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system --gid 1500 cs2-server \
 && useradd --system --uid 1500 --gid 1500 --home-dir /var/lib/cs2-server --shell /usr/sbin/nologin cs2-server \
 && install -d -m 0755 -o cs2-server -g cs2-server /var/lib/cs2-server \
 && install -d -m 0750 -o cs2-server -g cs2-server /etc/cs2-server
COPY --from=build /out/cs2-server /usr/local/bin/cs2-server
ENV CS2_SERVER_DATA_DIR=/var/lib/cs2-server \
    CS2_SERVER_CONFIG=/etc/cs2-server/cs2-server.json
USER cs2-server
WORKDIR /var/lib/cs2-server
# SIGTERM (docker stop) сервер обрабатывает сам: дожидается обработчиков и пишет «остановлен».
ENTRYPOINT ["cs2-server"]
CMD ["serve"]

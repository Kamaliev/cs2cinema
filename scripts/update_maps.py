#!/usr/bin/env python3
"""Обновляет геометрию карт (стены) из файлов ВАШЕЙ установленной CS2.

В демке геометрии нет, она лежит в VPK-архивах игры. Скрипт берёт конвейер awpy-data (MIT,
https://github.com/pnxenopoulos/awpy-data): Source2Viewer-CLI достаёт из `maps/<карта>.vpk`
физическую модель карты, дальше она превращается в упрощённый меш для трассировки лучей.
Результат кладётся в папку карт cs2-cli (`./maps` или $CS2CINEMA_MAPS) вместе с `maps.json`,
где записана версия игры (ClientVersion из steam.inf). Меши принадлежат Valve — не публикуйте их.

    python3 scripts/update_maps.py                    # найти CS2 и обновить, если игра обновилась
    python3 scripts/update_maps.py --check            # только проверить, актуальны ли меши (код 0/1)
    python3 scripts/update_maps.py --cs2-dir "D:/SteamLibrary/steamapps/common/Counter-Strike Global Offensive"
    python3 scripts/update_maps.py --maps de_nuke de_mirage --force

Нужны: Python 3.11+, git, ~1 ГБ места. Source2Viewer-CLI скачивается сам (или --s2v / $SOURCE2VIEWER_CLI).
Без установленной игры используйте обычный путь: cs2-cli сам скачает готовые меши из релиза awpy-data.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import time
import urllib.request
import zipfile
from pathlib import Path

DATA_REPO = "https://github.com/pnxenopoulos/awpy-data"
# Версия Source2Viewer-CLI, с которой awpy-data проверен (scripts/install_tools.sh)
S2V_VERSION = os.environ.get("S2V_VERSION", "19.2")
GAME_DIRNAME = "Counter-Strike Global Offensive"  # папка CS2 в Steam называется так до сих пор
STAMP = "maps.json"


def log(msg: str) -> None:
    print(f"[update_maps] {msg}", file=sys.stderr)


def die(msg: str, code: int = 2) -> "None":
    print(f"ошибка: {msg}", file=sys.stderr)
    raise SystemExit(code)


# --- поиск CS2 -------------------------------------------------------------------------------

def steam_roots() -> list[Path]:
    home = Path.home()
    cands = [
        Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")) / "Steam",
        Path(os.environ.get("ProgramFiles", r"C:\Program Files")) / "Steam",
        home / ".steam" / "steam",
        home / ".local" / "share" / "Steam",
        home / ".var/app/com.valvesoftware.Steam/.local/share/Steam",
        home / "Library" / "Application Support" / "Steam",
    ]
    return [p for p in cands if p.exists()]


def library_dirs(steam: Path) -> list[Path]:
    """Корень Steam и все дополнительные библиотеки из libraryfolders.vdf."""
    libs = [steam]
    vdf = steam / "steamapps" / "libraryfolders.vdf"
    if vdf.exists():
        for m in re.finditer(r'"path"\s+"([^"]+)"', vdf.read_text(encoding="utf-8", errors="replace")):
            libs.append(Path(m.group(1).replace("\\\\", "\\")))
    return libs


def is_cs2(p: Path) -> bool:
    return (p / "game" / "csgo" / "steam.inf").exists()


def find_cs2(explicit: str | None) -> Path:
    for raw in (explicit, os.environ.get("CS2_DIR")):
        if raw:
            p = Path(raw).expanduser()
            if not is_cs2(p):
                die(f"{p} не похоже на установку CS2 (нет game/csgo/steam.inf)")
            return p
    for steam in steam_roots():
        for lib in library_dirs(steam):
            p = lib / "steamapps" / "common" / GAME_DIRNAME
            if is_cs2(p):
                return p
    die("CS2 не найдена. Укажите папку: --cs2-dir <путь к «%s»> или переменную CS2_DIR" % GAME_DIRNAME)


def game_version(cs2: Path) -> str:
    text = (cs2 / "game" / "csgo" / "steam.inf").read_text(encoding="utf-8", errors="replace")
    m = re.search(r"^ClientVersion=(\d+)", text, re.M)
    if not m:
        die("в steam.inf нет ClientVersion")
    return m.group(1)


# --- метка maps.json (формат общий с cs2-cli) ---------------------------------------------------

def read_stamp(maps: Path) -> dict | None:
    try:
        return json.loads((maps / STAMP).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def up_to_date(maps: Path, cs2: Path, version: str, wanted: list[str] | None) -> bool:
    s = read_stamp(maps)
    if not s or s.get("source") != "game" or s.get("version") != version:
        return False
    if Path(s.get("cs2_dir") or "") != cs2:
        return False
    return bool(list(maps.glob("*.mesh"))) and all((maps / f"{m}.mesh").exists() for m in wanted or [])


# --- инструменты -------------------------------------------------------------------------------

def s2v_asset() -> str:
    machine = platform.machine().lower()
    arch = "arm64" if machine in ("arm64", "aarch64") else "x64"
    system = {"win32": "windows", "darwin": "macos"}.get(sys.platform, "linux")
    return f"cli-{system}-{arch}.zip"


def find_s2v(explicit: str | None, tools: Path) -> Path:
    exe = "Source2Viewer-CLI.exe" if sys.platform == "win32" else "Source2Viewer-CLI"
    for raw in (explicit, os.environ.get("SOURCE2VIEWER_CLI")):
        if raw:
            p = Path(raw).expanduser()
            if not p.exists():
                die(f"Source2Viewer-CLI не найден: {p}")
            return p.resolve()
    for p in (tools / exe, Path(shutil.which("Source2Viewer-CLI") or "/nonexistent")):
        if p.exists():
            return p.resolve()
    url = f"https://github.com/ValveResourceFormat/ValveResourceFormat/releases/download/{S2V_VERSION}/{s2v_asset()}"
    log(f"скачиваю Source2Viewer-CLI {S2V_VERSION}: {url}")
    tools.mkdir(parents=True, exist_ok=True)
    archive = tools / "cli.zip"
    urllib.request.urlretrieve(url, archive)
    with zipfile.ZipFile(archive) as z:
        for name in z.namelist():  # без путей наружу из папки
            if Path(name).is_absolute() or ".." in Path(name).parts:
                die(f"подозрительный путь в архиве: {name}")
        z.extractall(tools)
    archive.unlink()
    exe_path = tools / exe
    if not exe_path.exists():
        die(f"в архиве нет {exe}")
    exe_path.chmod(exe_path.stat().st_mode | 0o111)
    return exe_path.resolve()


def sync_pipeline(work: Path) -> Path:
    repo = work / "awpy-data"
    if (repo / ".git").exists():
        log("обновляю awpy-data")
        subprocess.run(["git", "-C", str(repo), "pull", "--ff-only", "-q"], check=True)
    else:
        log("клонирую awpy-data")
        work.mkdir(parents=True, exist_ok=True)
        subprocess.run(["git", "clone", "--depth", "1", "-q", DATA_REPO, str(repo)], check=True)
    return repo


# --- основной сценарий --------------------------------------------------------------------------

def main() -> int:
    if sys.version_info < (3, 11):
        die("нужен Python 3.11+")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--cs2-dir", help=f"папка «{GAME_DIRNAME}» (по умолчанию ищется в Steam)")
    ap.add_argument("--maps-dir", type=Path, help="куда класть меши (по умолчанию $CS2CINEMA_MAPS или ./maps)")
    ap.add_argument("--maps", nargs="*", help="только эти карты (по умолчанию все, что есть в игре)")
    ap.add_argument("--force", action="store_true", help="пересобрать, даже если версия не менялась")
    ap.add_argument("--check", action="store_true", help="только проверить актуальность: код 0 — актуально, 1 — нет")
    ap.add_argument("--s2v", help="путь к Source2Viewer-CLI")
    ap.add_argument("--pipeline-dir", type=Path, help="готовый checkout awpy-data (без git clone/pull)")
    ap.add_argument("--work-dir", type=Path, help="кэш конвейера (по умолчанию ~/.cache/cs2cinema)")
    args = ap.parse_args()

    maps_dir = (args.maps_dir or Path(os.environ.get("CS2CINEMA_MAPS", "maps"))).resolve()
    cs2 = find_cs2(args.cs2_dir).resolve()
    version = game_version(cs2)
    log(f"CS2: {cs2}, версия {version}")

    fresh = up_to_date(maps_dir, cs2, version, args.maps)
    if args.check:
        stamp = read_stamp(maps_dir)
        print(f"игра {version}; меши: {stamp.get('version') if stamp else 'нет'} ({stamp.get('source') if stamp else '-'})"
              f" — {'актуально' if fresh else 'нужно обновить'}")
        return 0 if fresh else 1
    if fresh and not args.force:
        log(f"меши уже соответствуют версии игры {version}, ничего не делаю (--force — пересобрать)")
        return 0

    work = (args.work_dir or Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "cs2cinema").resolve()
    repo = args.pipeline_dir.resolve() if args.pipeline_dir else sync_pipeline(work)
    s2v = find_s2v(args.s2v, work / "tools")

    env = dict(os.environ, AWPY_DATA_WORK=str(work / "work"), AWPY_DATA_TOOLS=str(work / "tools"),
               SOURCE2VIEWER_CLI=str(s2v), PYTHONIOENCODING="utf-8")
    shutil.rmtree(work / "work" / "extracted", ignore_errors=True)
    shutil.rmtree(work / "work" / "build", ignore_errors=True)
    (work / "work" / "build" / "geometry").mkdir(parents=True, exist_ok=True)

    def stage(script: str, *extra: str) -> None:
        subprocess.run([sys.executable, str(repo / "scripts" / script), *extra], check=True, env=env, cwd=repo)

    stage("fetch_game_files.py", "--cs2-dir", str(cs2))
    stage("extract_assets.py", "--cs2-dir", str(cs2), *(["--maps", *args.maps] if args.maps else []))
    stage("process_geometry.py")

    built = sorted((work / "work" / "build" / "geometry").glob("*.mesh"))
    if not built:
        die("конвейер не создал ни одного .mesh: формат файлов игры мог измениться — "
            "используйте готовые меши из релиза (cs2-cli скачает их сам) или сообщите в awpy-data", 1)

    maps_dir.mkdir(parents=True, exist_ok=True)
    for mesh in built:
        tmp = maps_dir / (mesh.name + ".tmp")
        shutil.copyfile(mesh, tmp)
        tmp.replace(maps_dir / mesh.name)  # атомарно: cs2-cli не увидит недокачанный файл
    (maps_dir / STAMP).write_text(json.dumps({
        "source": "game", "version": version, "checked_at": int(time.time()), "cs2_dir": str(cs2),
    }, indent=2), encoding="utf-8")
    log(f"готово: {len(built)} карт в {maps_dir} (версия игры {version})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

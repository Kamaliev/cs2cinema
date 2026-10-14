# Установщик cs2cinema для Windows (PowerShell):
#   irm https://raw.githubusercontent.com/Kamaliev/cs2cinema/main/install.ps1 | iex
#
# Ставит всё, что нужно для ролика, одной командой и без прав администратора:
#   * последний релиз cs2cinema (cs2cinema.exe, cs2-cli.exe) — в %LOCALAPPDATA%\cs2cinema;
#   * HLAE (запись в игре) — в %LOCALAPPDATA%\cs2cinema\hlae;
#   * ffmpeg (склейка; им же пишет HLAE) — в %LOCALAPPDATA%\cs2cinema\hlae\ffmpeg\bin.
# Каждый архив проверяется по SHA-256 (релиз — по файлу рядом с архивом, HLAE и ffmpeg — по
# суммам, зашитым ниже вместе с версией). Повторный запуск обновляет приложение; HLAE и ffmpeg
# скачиваются заново только если версия ниже изменилась или файлы пропали.
# Переменная CS2CINEMA_NO_TOOLS=1 — поставить только приложение (HLAE и ffmpeg уже есть свои).

$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
# Индикатор Invoke-WebRequest в Windows PowerShell замедляет загрузку в разы.
$ProgressPreference = "SilentlyContinue"

$repo = "Kamaliev/cs2cinema"
$asset = "cs2cinema-windows-x64.zip"
$base = "https://github.com/$repo/releases/latest/download"
$dir = Join-Path $env:LOCALAPPDATA "cs2cinema"
$hlaeDir = Join-Path $dir "hlae"

# Версии инструментов и SHA-256 их архивов. HLAE проверена с приложением (см. README);
# ffmpeg — сборка gyan.dev «essentials» (с NVENC), та же, что ставит winget install Gyan.FFmpeg.
$tools = @{
    hlae = @{
        version = "2.192.7"
        url = "https://github.com/advancedfx/advancedfx/releases/download/v2.192.7/hlae_2_192_7.zip"
        sha256 = "c0e84832a14170718cb3bfa760144b42beb8dee62b95390e1602f61e1b990e62"
    }
    ffmpeg = @{
        version = "9.0.2"
        url = "https://www.gyan.dev/ffmpeg/builds/packages/ffmpeg-9.0.2-essentials_build.zip"
        sha256 = "60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba"
    }
}
# Отметка о поставленных версиях: совпала — повторно не скачиваем.
$stamp = Join-Path $hlaeDir "cs2cinema-tools.txt"
$stampText = "hlae $($tools.hlae.version)`nffmpeg $($tools.ffmpeg.version)`n"

function Get-Verified([string]$url, [string]$path, [string]$sha256, [string]$what) {
    Write-Host "Скачиваю $what ..."
    try {
        Invoke-WebRequest $url -OutFile $path -UseBasicParsing
    } catch {
        throw "Не удалось скачать $what ($url): $($_.Exception.Message)"
    }
    $actual = (Get-FileHash $path -Algorithm SHA256).Hash.ToLower()
    if ($actual -ne $sha256.ToLower()) {
        throw "Контрольная сумма $what не совпала: ожидалось $sha256, получено $actual"
    }
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("cs2cinema-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
    # --- приложение ---
    $zip = Join-Path $tmp $asset
    Write-Host "Скачиваю $asset ..."
    try {
        Invoke-WebRequest "$base/$asset" -OutFile $zip -UseBasicParsing
    } catch {
        throw "Не удалось скачать релиз. Возможно, релизов ещё нет: https://github.com/$repo/releases"
    }
    # контрольная сумма из файла рядом с архивом
    $sumFile = Join-Path $tmp "$asset.sha256"
    Invoke-WebRequest "$base/$asset.sha256" -OutFile $sumFile -UseBasicParsing
    $expected = ((Get-Content $sumFile -Raw).Trim() -split "\s+")[0].ToLower()
    $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) { throw "Контрольная сумма не совпала: ожидалось $expected, получено $actual" }

    Expand-Archive $zip -DestinationPath (Join-Path $tmp "x") -Force
    $src = Join-Path $tmp "x\cs2cinema"
    if (-not (Test-Path (Join-Path $src "cs2-cli.exe"))) { throw "В архиве нет cs2-cli.exe" }

    New-Item -ItemType Directory -Force $dir | Out-Null
    Copy-Item (Join-Path $src "*") $dir -Recurse -Force

    # --- HLAE и ffmpeg ---
    $haveTools = (Test-Path (Join-Path $hlaeDir "HLAE.exe")) -and
        (Test-Path (Join-Path $hlaeDir "x64\AfxHookSource2.dll")) -and
        (Test-Path (Join-Path $hlaeDir "ffmpeg\bin\ffmpeg.exe")) -and
        (Test-Path (Join-Path $hlaeDir "ffmpeg\bin\ffprobe.exe")) -and
        (Test-Path $stamp) -and ((Get-Content $stamp -Raw) -eq $stampText)
    if ($env:CS2CINEMA_NO_TOOLS -eq "1") {
        Write-Host "CS2CINEMA_NO_TOOLS=1: HLAE и ffmpeg не ставятся."
    } elseif ($haveTools) {
        Write-Host "HLAE $($tools.hlae.version) и ffmpeg $($tools.ffmpeg.version) уже стоят в $hlaeDir."
    } else {
        $hlaeZip = Join-Path $tmp "hlae.zip"
        Get-Verified $tools.hlae.url $hlaeZip $tools.hlae.sha256 "HLAE $($tools.hlae.version)"
        $ffZip = Join-Path $tmp "ffmpeg.zip"
        Get-Verified $tools.ffmpeg.url $ffZip $tools.ffmpeg.sha256 "ffmpeg $($tools.ffmpeg.version) (около 100 МБ)"

        # Сначала всё в папку рядом, потом подмена целиком: пол-установки после обрыва не остаётся.
        $stage = Join-Path $tmp "hlae"
        Expand-Archive $hlaeZip -DestinationPath $stage -Force
        if (-not (Test-Path (Join-Path $stage "HLAE.exe"))) { throw "В архиве HLAE нет HLAE.exe" }
        if (-not (Test-Path (Join-Path $stage "x64\AfxHookSource2.dll"))) { throw "В архиве HLAE нет x64\AfxHookSource2.dll" }

        Expand-Archive $ffZip -DestinationPath (Join-Path $tmp "ff") -Force
        $ffRoot = Get-ChildItem (Join-Path $tmp "ff") -Directory | Select-Object -First 1
        if (-not $ffRoot -or -not (Test-Path (Join-Path $ffRoot.FullName "bin\ffmpeg.exe"))) { throw "В архиве ffmpeg нет bin\ffmpeg.exe" }
        $ffDir = Join-Path $stage "ffmpeg"
        New-Item -ItemType Directory -Force (Join-Path $ffDir "bin") | Out-Null
        # ffplay не нужен (100 МБ), лицензия обязательна (GPL).
        foreach ($f in "ffmpeg.exe", "ffprobe.exe") { Copy-Item (Join-Path $ffRoot.FullName "bin\$f") (Join-Path $ffDir "bin") -Force }
        foreach ($f in "LICENSE", "README.txt") {
            $p = Join-Path $ffRoot.FullName $f
            if (Test-Path $p) { Copy-Item $p $ffDir -Force }
        }
        # HLAE берёт ffmpeg из ffmpeg\ffmpeg.ini; путь абсолютный, чтобы не зависеть от рабочей папки.
        $ini = "[Ffmpeg]`r`nPath=" + (Join-Path $hlaeDir "ffmpeg\bin\ffmpeg.exe") + "`r`n"
        [IO.File]::WriteAllText((Join-Path $ffDir "ffmpeg.ini"), $ini, [Text.Encoding]::ASCII)
        [IO.File]::WriteAllText((Join-Path $stage "cs2cinema-tools.txt"), $stampText, [Text.Encoding]::ASCII)

        if (Test-Path $hlaeDir) {
            try {
                Remove-Item $hlaeDir -Recurse -Force
            } catch {
                throw "Не удалось заменить $hlaeDir (запущен HLAE или CS2 через него?): $($_.Exception.Message)"
            }
        }
        Move-Item $stage $hlaeDir
        Write-Host "HLAE $($tools.hlae.version) и ffmpeg $($tools.ffmpeg.version) установлены в $hlaeDir."
    }
} finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

# PATH пользователя (постоянно) и текущей сессии: папка приложения; ffmpeg — только если своего в PATH нет
# (чужой ffmpeg из PATH остаётся первым; приложение и так берёт ffmpeg из папки HLAE).
function Add-UserPath([string]$p) {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (($userPath -split ";") -notcontains $p) {
        [Environment]::SetEnvironmentVariable("Path", ($userPath.TrimEnd(";") + ";" + $p), "User")
        Write-Host "В PATH добавлена папка $p"
    }
    if (($env:Path -split ";") -notcontains $p) { $env:Path += ";" + $p }
}
Add-UserPath $dir
$ffBin = Join-Path $hlaeDir "ffmpeg\bin"
if ((Test-Path (Join-Path $ffBin "ffmpeg.exe")) -and -not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) {
    Add-UserPath $ffBin
}

# ярлык в меню «Пуск» для оконного приложения
$exe = Join-Path $dir "cs2cinema.exe"
if (Test-Path $exe) {
    $lnk = Join-Path ([Environment]::GetFolderPath("Programs")) "CS2 Cinema.lnk"
    $sc = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
    $sc.TargetPath = $exe
    $sc.WorkingDirectory = $dir
    $sc.Save()
    Write-Host "Ярлык «CS2 Cinema» добавлен в меню Пуск."
}

Write-Host ""
Write-Host "Установлено в $dir"
Write-Host "Запуск: «CS2 Cinema» в меню Пуск (HLAE и ffmpeg приложение найдёт само)."
Write-Host "Проверка консоли:  cs2-cli --list-cameras"
Write-Host "Отдельно нужны: CS2 в Steam; git (winget install Git.Git) — для обновления карт."

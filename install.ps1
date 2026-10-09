# Установщик cs2-cli для Windows (PowerShell):
#   irm https://raw.githubusercontent.com/Kamaliev/cs2cinema/main/install.ps1 | iex
#
# Скачивает последний релиз, проверяет SHA-256, кладёт в %LOCALAPPDATA%\cs2cinema
# и добавляет эту папку в PATH пользователя (права администратора не нужны).
# Повторный запуск обновляет установку до последнего релиза.

$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$repo = "Kamaliev/cs2cinema"
$asset = "cs2cinema-windows-x64.zip"
$base = "https://github.com/$repo/releases/latest/download"
$dir = Join-Path $env:LOCALAPPDATA "cs2cinema"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("cs2cinema-" + [Guid]::NewGuid().ToString("N"))

New-Item -ItemType Directory -Force $tmp | Out-Null
try {
    Write-Host "Скачиваю $asset ..."
    $zip = Join-Path $tmp $asset
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
} finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

# PATH пользователя (постоянно) и текущей сессии
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (($userPath -split ";") -notcontains $dir) {
    [Environment]::SetEnvironmentVariable("Path", ($userPath.TrimEnd(";") + ";" + $dir), "User")
    Write-Host "Папка добавлена в PATH."
}
if (($env:Path -split ";") -notcontains $dir) { $env:Path += ";" + $dir }

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
Write-Host "Проверка:  cs2-cli --list-cameras"
Write-Host "Нужны ещё: git, ffmpeg (winget install Git.Git Gyan.FFmpeg), HLAE для записи в игре."

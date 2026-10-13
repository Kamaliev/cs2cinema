# Сбор улик учения на машине раннера (Windows PowerShell 5.1) в «сырую» папку.
#
#   powershell -NoProfile -File docs\drills\collect-runner.ps1 -Out "$env:LOCALAPPDATA\cs2cinema\drills\raw\01-kill-runner\runner-after"
#
# Если нужно состояние «до» и «после» (например, до убийства раннера и после его возвращения), запускайте скрипт
# дважды в разные подпапки сырой папки: ...\runner-before и ...\runner-after (папка -Out должна быть пустой).
#
# Что собирает:
#   runner.log, runner.log.1, runner_state.json   из рабочей папки раннера (если есть)
#   listing.txt                                   файлы в out\ и remote\: относительный путь, размер, время записи
#   processes.txt                                 процессы cs2, HLAE, cs2cinema, cs2-cli
#   collect-runner.txt                            что именно собрано
#   raw-db\                                       только с -AppDb: копия базы приложения (для поиска токена; в репозиторий не идёт)
#
# Файл token раннера не копируется никогда.
#
# Кодировка: все текстовые файлы пишутся в UTF-8 (Out-File -Encoding utf8). Перенаправление ">" и Tee-Object в
# Windows PowerShell 5.1 дают UTF-16, где поиск токена ничего не найдёт, а evidence.sh check такие файлы отвергает.
param(
    [Parameter(Mandatory)][string]$Out,
    [string]$Dir = (Join-Path $env:LOCALAPPDATA 'cs2cinema\runner'),
    [switch]$AppDb
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $Dir -PathType Container)) {
    Write-Error "нет рабочей папки раннера (параметр -Dir)"
    exit 2
}
$base = (Resolve-Path -LiteralPath $Dir).Path.TrimEnd('\')

# Правило повтора: в непустую папку не собираем, чтобы прогоны не смешивались.
if ((Test-Path -LiteralPath $Out) -and (@(Get-ChildItem -LiteralPath $Out -Force).Count -gt 0)) {
    Write-Error "папка -Out не пуста; удалите её и соберите заново"
    exit 2
}
New-Item -ItemType Directory -Force -Path $Out | Out-Null

$notes = New-Object System.Collections.Generic.List[string]
$notes.Add("сбор улик раннера: " + (Get-Date).ToString('yyyy-MM-ddTHH:mm:ss'))

# Файлы из рабочей папки раннера. Файл token не входит в список и не копируется.
foreach ($name in 'runner.log', 'runner.log.1', 'runner_state.json') {
    $src = Join-Path $base $name
    if (Test-Path -LiteralPath $src -PathType Leaf) {
        Copy-Item -LiteralPath $src -Destination (Join-Path $Out $name)
        $notes.Add("скопирован: $name")
    }
    else {
        $notes.Add("нет файла: $name")
    }
}

# listing.txt: относительные пути, размер и время записи всего в out\ и remote\.
$rows = New-Object System.Collections.Generic.List[string]
$rows.Add("# путь`tразмер`tLastWriteTime (относительно рабочей папки раннера; out и remote)")
foreach ($sub in 'out', 'remote') {
    $root = Join-Path $base $sub
    if (Test-Path -LiteralPath $root -PathType Container) {
        foreach ($f in @(Get-ChildItem -LiteralPath $root -Recurse -File -Force)) {
            $rel = $f.FullName.Substring($base.Length).TrimStart('\')
            $rows.Add(("{0}`t{1}`t{2}" -f $rel, $f.Length, $f.LastWriteTime.ToString('yyyy-MM-ddTHH:mm:ss')))
        }
    }
    else {
        $rows.Add("# нет папки $sub")
    }
}
$rows | Out-File -Encoding utf8 -FilePath (Join-Path $Out 'listing.txt')
$notes.Add("записан: listing.txt")

# processes.txt: имя, Id, время запуска.
$lines = New-Object System.Collections.Generic.List[string]
$procs = @(Get-Process -Name cs2, HLAE, cs2cinema, cs2-cli -ErrorAction SilentlyContinue)
if ($procs.Count -eq 0) {
    $lines.Add('# процессов cs2, HLAE, cs2cinema, cs2-cli нет')
}
else {
    foreach ($p in $procs) {
        try { $started = $p.StartTime.ToString('yyyy-MM-ddTHH:mm:ss') }
        catch { $started = 'недоступно' }
        $lines.Add(("Name={0} Id={1} StartTime={2}" -f $p.ProcessName, $p.Id, $started))
    }
}
$lines | Out-File -Encoding utf8 -FilePath (Join-Path $Out 'processes.txt')
$notes.Add("записан: processes.txt")

# Копия базы приложения: только для поиска токена, в репозиторий не идёт.
if ($AppDb) {
    $dbDir = Join-Path $env:LOCALAPPDATA 'cs2cinema'
    $rawDb = Join-Path $Out 'raw-db'
    New-Item -ItemType Directory -Force -Path $rawDb | Out-Null
    foreach ($name in 'cs2cinema.db', 'cs2cinema.db-wal', 'cs2cinema.db-shm') {
        $src = Join-Path $dbDir $name
        if (Test-Path -LiteralPath $src -PathType Leaf) {
            try {
                Copy-Item -LiteralPath $src -Destination (Join-Path $rawDb $name)
                $notes.Add("скопирован в raw-db: $name")
            }
            catch {
                $notes.Add("не удалось скопировать в raw-db: $name (файл занят?)")
            }
        }
    }
}

$notes | Out-File -Encoding utf8 -FilePath (Join-Path $Out 'collect-runner.txt')

Write-Host "собрано в папку -Out"
Write-Host 'Дальше: на машине автора bash docs/drills/evidence.sh import <сырая папка> <папка улик>'

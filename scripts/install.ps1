<#
.SYNOPSIS
Installs Parakeet Dictate for the current user, or uninstalls it.

.DESCRIPTION
The app keeps running from this project folder and its .venv. Install sets up the .venv and the
model when either is missing, puts a "Parakeet Dictate" shortcut on the Desktop and in the Start
menu, and adds an entry to Settings > Apps > Installed apps that runs this script with -Uninstall.
Running it again gives the same result. -Startup also adds a shortcut that starts the app silently
at sign-in; without -Startup, an existing sign-in shortcut stays as it is.

-Uninstall stops the app if it runs from this folder, removes the shortcuts that start this
folder's app, and removes the Installed apps entry. It leaves the folder, the model and
tray_choices.json in place.

The folder and registry parameters exist so a test can point the script at temporary places.

.EXAMPLE
.\scripts\install.ps1
.\scripts\install.ps1 -Startup
.\scripts\install.ps1 -Uninstall
#>
param(
    [switch]$Startup,
    [switch]$Uninstall,
    [string]$DesktopDir = [Environment]::GetFolderPath("Desktop"),
    [string]$StartMenuDir = [Environment]::GetFolderPath("Programs"),
    [string]$StartupDir = [Environment]::GetFolderPath("Startup"),
    [string]$RegistryKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\ParakeetDictate"
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$venvScripts = Join-Path $root ".venv\Scripts"
$pythonw = Join-Path $venvScripts "pythonw.exe"
$icon = Join-Path $root "assets\parakeet.ico"
$model = Join-Path $root "models\parakeet-tdt-0.6b-v3"
$appName = "Parakeet Dictate"
$shell = New-Object -ComObject WScript.Shell

function Get-LinkPath([string]$dir) {
    Join-Path $dir "$appName.lnk"
}

function Set-Link([string]$dir, [string]$arguments) {
    New-Item -ItemType Directory -Force $dir | Out-Null
    $path = Get-LinkPath $dir
    $link = $shell.CreateShortcut($path)
    $link.TargetPath = $pythonw
    $link.Arguments = $arguments
    $link.WorkingDirectory = $root
    $link.IconLocation = "$icon,0"
    $link.Description = "Push-to-talk dictation with Parakeet"
    $link.Save()
    "Shortcut: $path"
}

function Invoke-Uv {
    if (-not (Get-Command uv -ErrorAction SilentlyContinue)) {
        throw "uv is not installed. Install it from https://docs.astral.sh/uv/ and run this again."
    }
    uv --directory $root @args
    if ($LASTEXITCODE -ne 0) {
        throw "uv $args failed with exit code $LASTEXITCODE."
    }
}

if ($Uninstall) {
    $running = @(Get-CimInstance Win32_Process | Where-Object {
        $_.ExecutablePath -and $_.ExecutablePath.StartsWith("$venvScripts\", [StringComparison]::OrdinalIgnoreCase) -and
        $_.CommandLine -match "parakeet[_-]dictate"
    })
    if ($running.Count -gt 0) {
        & (Join-Path $venvScripts "python.exe") -m parakeet_dictate --quit
        "Asked the running app to quit."
    } else {
        "The app is not running from $root."
    }

    foreach ($dir in $DesktopDir, $StartMenuDir, $StartupDir) {
        $path = Get-LinkPath $dir
        if (-not (Test-Path -LiteralPath $path)) {
            continue
        }
        $target = $shell.CreateShortcut($path).TargetPath
        if ($target -ieq $pythonw) {
            Remove-Item -LiteralPath $path
            "Removed $path"
        } else {
            "Kept $path, which starts $target"
        }
    }

    if (Test-Path $RegistryKey) {
        Remove-Item $RegistryKey -Recurse
        "Removed the Installed apps entry $RegistryKey"
    }
    "Left in place: the project folder $root, the model in $model, and tray_choices.json."
    return
}

if (-not (Test-Path $pythonw)) {
    "Installing the dependencies with uv sync."
    Invoke-Uv sync
}
if (-not (Test-Path $model)) {
    "Downloading the model, about 3 GB. Rerunning skips the files already downloaded."
    Invoke-Uv run python scripts/download_model.py
}

Set-Link $DesktopDir "-m parakeet_dictate --notify-ready"
Set-Link $StartMenuDir "-m parakeet_dictate --notify-ready"
if ($Startup) {
    Set-Link $StartupDir "-m parakeet_dictate"
}

$pyproject = Get-Content -Raw (Join-Path $root "pyproject.toml")
$uninstallCommand = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`" -Uninstall"
# An entry made for other folders or another key must uninstall from those, not the real ones.
foreach ($param in "DesktopDir", "StartMenuDir", "StartupDir", "RegistryKey") {
    if ($PSBoundParameters.ContainsKey($param)) {
        $uninstallCommand += " -$param `"$(Get-Variable $param -ValueOnly)`""
    }
}
$strings = [ordered]@{
    DisplayName     = $appName
    DisplayIcon     = $icon
    DisplayVersion  = [regex]::Match($pyproject, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
    Publisher       = [regex]::Match($pyproject, '(?m)^authors\s*=\s*\[\s*\{\s*name\s*=\s*"([^"]+)"').Groups[1].Value
    InstallLocation = $root
    UninstallString = $uninstallCommand
}
if (-not (Test-Path $RegistryKey)) {
    New-Item $RegistryKey -Force | Out-Null
}
foreach ($entry in $strings.GetEnumerator()) {
    New-ItemProperty -Path $RegistryKey -Name $entry.Key -Value $entry.Value -PropertyType String -Force | Out-Null
}
foreach ($flag in "NoModify", "NoRepair") {
    New-ItemProperty -Path $RegistryKey -Name $flag -Value 1 -PropertyType DWord -Force | Out-Null
}
"Installed apps entry: $RegistryKey"
"Done. Start $appName from the Start menu or the Desktop, then hold the hotkey to dictate."

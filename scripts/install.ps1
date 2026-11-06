<#
.SYNOPSIS
Installs Parakeet Dictate for the current user, or uninstalls it.

.DESCRIPTION
The app keeps running from this project folder. Install builds parakeet-dictate.exe into it with cargo,
downloads the runtime and the model when they are missing, puts a "Parakeet Dictate" shortcut on the
Desktop and in the Start menu, and adds an entry to Settings > Apps > Installed apps that runs this
script with -Uninstall. Running it again gives the same result, and picks up a newer source. -Startup
also adds a shortcut that starts the app silently at sign-in; without -Startup, an existing sign-in
shortcut stays as it is, unless it starts the old Python app from this folder.

-Uninstall stops the app if it runs from this folder, removes the shortcuts that start this
folder's app, and removes the Installed apps entry. It leaves the folder, the runtime, the model and
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
$exe = Join-Path $root "parakeet-dictate.exe"
$pythonw = Join-Path $root ".venv\Scripts\pythonw.exe"
$icon = Join-Path $root "assets\parakeet.ico"
$runtime = Join-Path $root "runtime"
$model = Join-Path $root "models\parakeet-ultra"
$appName = "Parakeet Dictate"
$shell = New-Object -ComObject WScript.Shell

function Get-LinkPath([string]$dir) {
    Join-Path $dir "$appName.lnk"
}

function Set-Link([string]$dir, [string]$arguments) {
    New-Item -ItemType Directory -Force $dir | Out-Null
    $path = Get-LinkPath $dir
    $link = $shell.CreateShortcut($path)
    $link.TargetPath = $exe
    $link.Arguments = $arguments
    $link.WorkingDirectory = $root
    $link.IconLocation = "$icon,0"
    $link.Description = "Push-to-talk dictation with Parakeet"
    $link.Save()
    "Shortcut: $path"
}

function Test-AppRunning {
    @(Get-CimInstance Win32_Process -Filter "Name = 'parakeet-dictate.exe'" | Where-Object {
        $_.ExecutablePath -ieq $exe
    }).Count -gt 0
}

if ($Uninstall) {
    if (Test-AppRunning) {
        & $exe --quit
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
        if ($target -ieq $exe -or $target -ieq $pythonw) {
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
    # Removing a firewall rule needs an administrator; uninstall stays unelevated and only points at it.
    if (Get-NetFirewallRule -DisplayName "Parakeet Dictate (Mac)" -ErrorAction SilentlyContinue) {
        "Kept the firewall rule 'Parakeet Dictate (Mac)'. Remove it with .\scripts\allow_mac.ps1 -Remove"
    }
    "Left in place: the project folder $root, the runtime, the model in $model, and tray_choices.json."
    return
}

if (Test-AppRunning) {
    throw "Parakeet Dictate is running from this folder. Quit it from the tray and run this again."
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    if (-not (Test-Path $exe)) {
        throw "Rust is not installed. Install it from https://rustup.rs and run this again."
    }
    "Rust is not installed, so this keeps the existing $exe."
} else {
    & cargo build --release --locked --bin parakeet-dictate --manifest-path (Join-Path $root "Cargo.toml")
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE."
    }
    Copy-Item -LiteralPath (Join-Path $root "target\release\parakeet-dictate.exe") -Destination $exe -Force
}
if (-not ((Test-Path (Join-Path $runtime "onnxruntime.dll")) -and (Test-Path (Join-Path $model "config.json")))) {
    "Downloading the runtime and the model, about 4 GB. Rerunning skips the files already downloaded."
}
& (Join-Path $PSScriptRoot "fetch_assets.ps1") -Root $root

Set-Link $DesktopDir "--notify-ready"
Set-Link $StartMenuDir "--notify-ready"
$startupLink = Get-LinkPath $StartupDir
if ($Startup -or ((Test-Path -LiteralPath $startupLink) -and $shell.CreateShortcut($startupLink).TargetPath -ieq $pythonw)) {
    Set-Link $StartupDir ""
}

$cargo = Get-Content -Raw (Join-Path $root "Cargo.toml")
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
    DisplayVersion  = [regex]::Match($cargo, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
    Publisher       = [regex]::Match($cargo, '(?m)^authors\s*=\s*\["([^<"]+?)\s*[<"]').Groups[1].Value
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

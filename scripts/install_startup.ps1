<#
.SYNOPSIS
Adds or removes a Startup-folder shortcut that runs Parakeet Dictate in the background at sign-in.

.EXAMPLE
.\scripts\install_startup.ps1
.\scripts\install_startup.ps1 -Remove
#>
param([switch]$Remove)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$shortcut = Join-Path ([Environment]::GetFolderPath("Startup")) "Parakeet Dictate.lnk"

if ($Remove) {
    if (Test-Path $shortcut) {
        Remove-Item $shortcut
        "Removed $shortcut"
    } else {
        "No shortcut at $shortcut"
    }
    return
}

$pythonw = Join-Path $root ".venv\Scripts\pythonw.exe"
if (-not (Test-Path $pythonw)) {
    throw "Missing $pythonw. Run 'uv sync' in $root first."
}

$link = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
$link.TargetPath = $pythonw
$link.Arguments = "-m parakeet_dictate"
$link.WorkingDirectory = $root
$link.Description = "Parakeet Dictate push-to-talk dictation"
$link.Save()
"Created $shortcut -> $pythonw -m parakeet_dictate (in $root)"

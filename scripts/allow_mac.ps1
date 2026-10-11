<#
.SYNOPSIS
Lets a Mac on the local network reach the server for Mac through Windows Firewall, or removes that rule.

.DESCRIPTION
Creates the inbound rule "Parakeet Dictate (Mac)": TCP on the server port, from the local subnet only.
The rule applies to every network profile, because Windows may classify a home Ethernet network as
Public. Running it again replaces the rule, so a changed port takes effect. -Remove deletes the rule.

The port comes from -Port, else from [server] port in config.toml, else 8765. Changing firewall rules
needs an administrator, so the script restarts itself elevated, which shows a UAC prompt.
-WhatIf prints what it would do without elevating or changing anything.

.EXAMPLE
.\scripts\allow_mac.ps1
.\scripts\allow_mac.ps1 -Port 9000
.\scripts\allow_mac.ps1 -Remove
#>
[CmdletBinding(SupportsShouldProcess)]
param(
    [int]$Port = 0,
    [switch]$Remove
)

$ErrorActionPreference = "Stop"
$ruleName = "Parakeet Dictate (Mac)"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

if ($Port -eq 0) {
    $Port = 8765
    $config = Join-Path $root "config.toml"
    if (Test-Path $config) {
        # Only the port line inside [server]; the full TOML grammar is not needed for one integer.
        $section = ""
        foreach ($line in Get-Content $config) {
            if ($line -match '^\s*\[(\w+)\]') {
                $section = $Matches[1]
            } elseif ($section -eq "server" -and $line -match '^\s*port\s*=\s*(\d+)') {
                $Port = [int]$Matches[1]
            }
        }
    }
}

$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
    $arguments = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "`"$PSCommandPath`"", "-Port", $Port)
    if ($Remove) {
        $arguments += "-Remove"
    }
    if ($PSCmdlet.ShouldProcess("powershell.exe $arguments", "Restart elevated")) {
        Start-Process powershell.exe -Verb RunAs -ArgumentList $arguments -Wait
        # The elevated window closes when it finishes, so report the outcome from here.
        $rule = Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue
        if ($rule) {
            $ports = ($rule | Get-NetFirewallPortFilter).LocalPort -join ", "
            "The firewall rule '$ruleName' allows inbound TCP $ports from the local subnet."
        } else {
            "There is no firewall rule '$ruleName'."
        }
    }
    return
}

$existing = Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue
if ($existing -and $PSCmdlet.ShouldProcess($ruleName, "Remove the firewall rule")) {
    $existing | Remove-NetFirewallRule
    "Removed the firewall rule '$ruleName'."
}
if ($Remove) {
    if (-not $existing) {
        "There is no firewall rule '$ruleName'."
    }
    return
}
if ($PSCmdlet.ShouldProcess("$ruleName, inbound TCP $Port from LocalSubnet, all profiles", "Create the firewall rule")) {
    New-NetFirewallRule -DisplayName $ruleName -Direction Inbound -Action Allow -Protocol TCP -LocalPort $Port `
        -RemoteAddress LocalSubnet -Profile Any | Out-Null
    "Created the firewall rule '$ruleName': inbound TCP $Port from the local subnet, on every network profile."
}

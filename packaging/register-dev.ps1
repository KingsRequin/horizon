# Enregistre le paquet sparse d'Horizon en mode développeur (sans signature ni droits
# administrateur) : horizon.exe dans -ExeDir obtient l'identité de paquet « Horizon ».
#   .\packaging\register-dev.ps1 -ExeDir <dossier de compilation>\debug
#   .\packaging\register-dev.ps1 -Unregister
param(
  [string]$ExeDir,
  [switch]$Unregister
)
$ErrorActionPreference = 'Stop'

Get-AppxPackage -Name Horizon | ForEach-Object {
  Write-Host "Retrait de $($_.PackageFullName)"
  Remove-AppxPackage -Package $_.PackageFullName
}
if ($Unregister) { return }
if (-not $ExeDir -or -not (Test-Path (Join-Path $ExeDir 'horizon.exe'))) {
  throw "horizon.exe introuvable dans '$ExeDir'"
}

$dev = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock' -ErrorAction SilentlyContinue
if ($dev.AllowDevelopmentWithoutDevLicense -ne 1) {
  throw "Le mode développeur de Windows est désactivé : Windows refuse une identité de paquet sans signature."
}

# Le manifeste et ses images sont copiés hors du dépôt : Windows garde le dossier verrouillé.
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$pkg = Join-Path $env:LOCALAPPDATA 'Horizon\package'
New-Item -ItemType Directory -Force (Join-Path $pkg 'Assets') | Out-Null
Copy-Item (Join-Path $root 'AppxManifest.xml') $pkg -Force
# Icônes : à côté du manifeste (dépôt, et paquet partagé de scripts\package.ps1).
$icons = Join-Path $root 'icons'
foreach ($f in 'StoreLogo.png', 'Square150x150Logo.png', 'Square44x44Logo.png') {
  Copy-Item (Join-Path $icons $f) (Join-Path $pkg "Assets\$f") -Force
}

Add-AppxPackage -Register (Join-Path $pkg 'AppxManifest.xml') -ExternalLocation (Resolve-Path $ExeDir)
Get-AppxPackage -Name Horizon | Select-Object PackageFullName, InstallLocation

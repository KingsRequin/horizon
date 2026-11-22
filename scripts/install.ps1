# Installe (ou met à jour) Horizon depuis les sources pour l'utilisateur courant, sans droits
# administrateur (la version publiée s'installe par Horizon-Setup-<version>.exe,
# packaging\horizon.iss, qui fait les mêmes étapes) :
#   - compile Horizon et son hook Claude Code en release ;
#   - copie les exécutables dans %LOCALAPPDATA%\Programs\Horizon ;
#   - enregistre l'identité de paquet (lecture des notifications) pour ce dossier ;
#   - démarre Horizon à l'ouverture de session (clé Run de HKCU) et l'ajoute au menu Démarrer ;
#   - installe les hooks Claude Code (suivi des sessions), sauf avec -NoClaudeHooks.
# Désinstaller : scripts\uninstall.ps1
param([switch]$NoBuild, [switch]$NoClaudeHooks)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $env:LOCALAPPDATA 'Programs\Horizon'
Push-Location $repo
try {
  $target = (& cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
} finally { Pop-Location }
$release = Join-Path $target 'release'

if (-not $NoBuild) {
  Write-Host "Compilation d'Horizon et de son hook Claude Code…"
  Push-Location $repo
  try {
    & cargo build --release --no-default-features -p horizon-natif -p horizon-hook
    if ($LASTEXITCODE) { throw "compilation d'Horizon échouée" }
  } finally { Pop-Location }
}

# Fermeture propre (place réservée libérée), arrêt forcé seulement en dernier recours.
$running = Get-Process horizon -ErrorAction SilentlyContinue | Select-Object -First 1
if ($running) {
  & $running.Path --quit
  Wait-Process -Id $running.Id -Timeout 5 -ErrorAction SilentlyContinue
  Get-Process horizon -ErrorAction SilentlyContinue | Stop-Process -Force
}
Start-Sleep -Milliseconds 500
New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item (Join-Path $release 'horizon.exe') $dest -Force
Copy-Item (Join-Path $release 'horizon-hook.exe') $dest -Force
$exe = Join-Path $dest 'horizon.exe'
# Restes d'une mise à jour automatique (exécutables remplacés pendant qu'ils tournaient).
Remove-Item (Join-Path $dest '*.old') -Force -ErrorAction SilentlyContinue

# Identité de paquet pour l'emplacement installé (notifications). Sans le mode développeur de
# Windows, Horizon marche quand même, sans lire les notifications des autres applis.
try {
  & (Join-Path $PSScriptRoot '..\packaging\register-dev.ps1') -ExeDir $dest | Out-Null
} catch {
  Write-Warning "Notifications Windows non lues : $($_.Exception.Message)"
  Write-Warning "Activer le mode développeur (Paramètres > Système > Espace développeurs), puis relancer l'installation."
}

# Démarrage avec la session (même valeur que « Démarrer avec Windows » dans le menu d'Horizon).
Set-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'Horizon' -Value "`"$exe`""

$lnk = Join-Path ([Environment]::GetFolderPath('Programs')) 'Horizon.lnk'
$sh = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
$sh.TargetPath = $exe
$sh.WorkingDirectory = $dest
$sh.Description = "Barre d'outils en haut des écrans"
$sh.Save()

if (-not $NoClaudeHooks) {
  & (Join-Path $PSScriptRoot 'install-claude-hooks.ps1') -HookExe (Join-Path $dest 'horizon-hook.exe')
}

Start-Process $exe
Write-Host "Horizon installé dans $dest et démarré."

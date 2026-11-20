# Désinstalle Horizon : arrêt (la place réservée en haut des écrans est libérée), identité de
# paquet retirée, démarrage automatique, raccourci et hooks Claude Code supprimés, fichiers
# retirés. Une installation faite par Horizon-Setup-<version>.exe est retirée par son
# désinstalleur, qui fait les mêmes étapes et l'enlève des applications installées de Windows.
# Les réglages (%APPDATA%\Horizon) sont gardés, sauf avec -RemoveConfig.
param([switch]$RemoveConfig)

$ErrorActionPreference = 'Stop'
$dest = Join-Path $env:LOCALAPPDATA 'Programs\Horizon'
$uninstaller = Join-Path $dest 'unins000.exe'

if (Test-Path $uninstaller) {
  # Sans question : les réglages restent, -RemoveConfig les retire plus bas.
  $run = Start-Process $uninstaller -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait -PassThru
  if ($run.ExitCode) { throw "désinstalleur d'Horizon : code $($run.ExitCode)" }
} else {
  # Fermeture propre (place réservée libérée), arrêt forcé seulement en dernier recours.
  $running = Get-Process horizon -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($running) {
    & $running.Path --quit
    Wait-Process -Id $running.Id -Timeout 5 -ErrorAction SilentlyContinue
    Get-Process horizon -ErrorAction SilentlyContinue | Stop-Process -Force
  }
  & (Join-Path $PSScriptRoot '..\packaging\register-dev.ps1') -Unregister
  Remove-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'Horizon' -ErrorAction SilentlyContinue
  Remove-Item (Join-Path ([Environment]::GetFolderPath('Programs')) 'Horizon.lnk') -ErrorAction SilentlyContinue
  # Sinon Claude Code appellerait un exécutable qui n'existe plus.
  & (Join-Path $PSScriptRoot 'install-claude-hooks.ps1') -Uninstall
  Start-Sleep -Milliseconds 500
  Remove-Item -Recurse -Force $dest -ErrorAction SilentlyContinue
  Remove-Item -Recurse -Force (Join-Path $env:LOCALAPPDATA 'Horizon\package') -ErrorAction SilentlyContinue
}
if ($RemoveConfig) { Remove-Item -Recurse -Force (Join-Path $env:APPDATA 'Horizon') -ErrorAction SilentlyContinue }
Write-Host 'Horizon désinstallé.'

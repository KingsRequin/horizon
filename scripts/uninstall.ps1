# Désinstalle Horizon : arrêt (la place réservée en haut des écrans est libérée), identité de
# paquet retirée, démarrage automatique et raccourci supprimés, fichiers retirés.
# La disposition (%APPDATA%\Horizon) est gardée, sauf avec -RemoveConfig.
# Les hooks Claude Code ne sont retirés qu'avec -RemoveClaudeHooks.
param([switch]$RemoveClaudeHooks, [switch]$RemoveConfig)

$ErrorActionPreference = 'Stop'
$dest = Join-Path $env:LOCALAPPDATA 'Programs\Horizon'

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
if ($RemoveClaudeHooks) { & (Join-Path $PSScriptRoot 'install-claude-hooks.ps1') -Uninstall }
Start-Sleep -Milliseconds 500
Remove-Item -Recurse -Force $dest -ErrorAction SilentlyContinue
if ($RemoveConfig) { Remove-Item -Recurse -Force (Join-Path $env:APPDATA 'Horizon') -ErrorAction SilentlyContinue }
Write-Host 'Horizon désinstallé.'

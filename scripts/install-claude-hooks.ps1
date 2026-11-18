# Ajoute (ou retire avec -Uninstall) les hooks Claude Code d'Horizon dans les réglages globaux
# de Claude Code. Les hooks existants sont conservés ; une sauvegarde est faite avant toute
# écriture (settings.json.bak-horizon) ; relancer le script ne crée pas de doublon.
#   .\scripts\install-claude-hooks.ps1
#   .\scripts\install-claude-hooks.ps1 -Uninstall
param(
  [string]$HookExe = (Join-Path $env:LOCALAPPDATA 'Programs\Horizon\horizon-hook.exe'),
  [string]$SettingsPath = (Join-Path $env:USERPROFILE '.claude\settings.json'),
  [switch]$Uninstall
)
$ErrorActionPreference = 'Stop'

# Événements suivis : début, question posée, outil exécuté, autorisation demandée, attente
# d'une réponse, fin. PermissionRequest : horizon-hook observe seulement (aucune sortie, aucune
# décision) ; un autre hook (SharkApp) peut décider, Claude Code les lance côte à côte.
$events = 'SessionStart', 'UserPromptSubmit', 'PostToolUse', 'PermissionRequest', 'Notification', 'Stop', 'SessionEnd'
# Événements filtrés par nom d'outil : « * » = tous les outils.
$toolEvents = 'PostToolUse', 'PermissionRequest'
$marker = 'horizon-hook'

if (Test-Path $SettingsPath) {
  $raw = Get-Content $SettingsPath -Raw -Encoding utf8
  $settings = if ($raw.Trim()) { $raw | ConvertFrom-Json } else { [pscustomobject]@{} }
  Copy-Item $SettingsPath "$SettingsPath.bak-horizon" -Force
} else {
  New-Item -ItemType Directory -Force (Split-Path $SettingsPath) | Out-Null
  $settings = [pscustomobject]@{}
}
if (-not $settings.PSObject.Properties['hooks']) {
  $settings | Add-Member -NotePropertyName hooks -NotePropertyValue ([pscustomobject]@{})
}
$hooks = $settings.hooks

function Without-Horizon($groups) {
  # Retire nos commandes ; un groupe qui n'a plus de commande disparaît.
  @(foreach ($g in @($groups)) {
    $kept = @($g.hooks | Where-Object { $_.command -notmatch $marker })
    if ($kept.Count -gt 0) { $g.hooks = $kept; $g }
  })
}

foreach ($ev in $events) {
  $groups = if ($hooks.PSObject.Properties[$ev]) { Without-Horizon $hooks.$ev } else { @() }
  if (-not $Uninstall) {
    $entry = [pscustomobject]@{
      type    = 'command'
      command = "`"$HookExe`""
      timeout = 5
    }
    $group = [pscustomobject]@{ hooks = @($entry) }
    if ($toolEvents -contains $ev) { $group | Add-Member -NotePropertyName matcher -NotePropertyValue '*' }
    $groups = @($groups) + $group
  }
  if ($groups.Count -gt 0) {
    $hooks | Add-Member -NotePropertyName $ev -NotePropertyValue @($groups) -Force
  } elseif ($hooks.PSObject.Properties[$ev]) {
    $hooks.PSObject.Properties.Remove($ev)
  }
}

$json = $settings | ConvertTo-Json -Depth 32
[IO.File]::WriteAllText($SettingsPath, $json, [Text.UTF8Encoding]::new($false))
if ($Uninstall) { "Hooks d'Horizon retirés de $SettingsPath" } else { "Hooks d'Horizon installés dans $SettingsPath ($HookExe)" }

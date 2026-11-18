# Prépare une version publiée d'Horizon (édition publique), celle des Releases GitHub :
#   - compile Horizon et son hook Claude Code en release, sans la fonction `personal` et avec
#     HORIZON_RELEASE=1 (mise à jour automatique active, voir natif/src/update.rs), dans un
#     dossier de compilation à part (« <dossier habituel>-share ») pour ne pas toucher à une
#     compilation personnelle ;
#   - réunit les exécutables, les scripts d'installation et le paquet des notifications dans
#     <Out>\Horizon, avec Installer.cmd, Desinstaller.cmd, LISEZMOI.txt et README.txt ;
#   - en fait <Out>\Horizon-<version>.zip ;
#   - pose à côté horizon.exe, horizon-hook.exe et SHA256SUMS : ce que télécharge la mise à jour
#     automatique (à joindre tous les quatre à la Release).
# Rien n'est installé ni modifié sur ce PC.
#   .\scripts\package.ps1                  # zip dans dist-share\ (ignoré par git)
#   .\scripts\package.ps1 -Out C:\Temp     # ailleurs
#   .\scripts\package.ps1 -NoBuild         # reprend la dernière compilation partagée
param([string]$Out, [switch]$NoBuild)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Out) { $Out = Join-Path $repo 'dist-share' }
# Chemin absolu : les appels .NET ([IO.File]) ne suivent pas le dossier courant de PowerShell.
New-Item -ItemType Directory -Force $Out | Out-Null
$Out = (Resolve-Path $Out).Path
Push-Location $repo
try {
  $meta = & cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
} finally { Pop-Location }
$target = $meta.target_directory
$version = ($meta.packages | Where-Object name -eq 'horizon-natif').version
$shareTarget = "$target-share"
$release = Join-Path $shareTarget 'release'

if (-not $NoBuild) {
  $env:HORIZON_RELEASE = '1'
  $env:CARGO_TARGET_DIR = $shareTarget
  try {
    Write-Host "Compilation d'Horizon et de son hook Claude Code (édition publique)…"
    Push-Location $repo
    try {
      & cargo build --release -p horizon-natif -p horizon-hook
      if ($LASTEXITCODE) { throw "compilation d'Horizon échouée" }
    } finally { Pop-Location }
  } finally {
    Remove-Item Env:HORIZON_RELEASE, Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
  }
}
foreach ($exe in 'horizon.exe', 'horizon-hook.exe') {
  if (-not (Test-Path (Join-Path $release $exe))) { throw "$exe absent de $release : lancer sans -NoBuild" }
}

# Copie d'un script en UTF-8 avec BOM : Windows PowerShell 5.1 (celui de tous les PC, lancé par
# Installer.cmd) lit un fichier sans BOM comme de l'ANSI et y prend certains octets des
# caractères accentués ou typographiques pour des guillemets.
function Copy-Script([string]$from, [string]$to) {
  $text = [IO.File]::ReadAllText($from, [Text.UTF8Encoding]::new($false))
  [IO.File]::WriteAllText($to, $text, [Text.UTF8Encoding]::new($true))
}

$stage = Join-Path $Out 'Horizon'
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
foreach ($d in 'bin', 'scripts', 'packaging\icons') { New-Item -ItemType Directory -Force (Join-Path $stage $d) | Out-Null }

Copy-Item (Join-Path $release 'horizon.exe'), (Join-Path $release 'horizon-hook.exe') (Join-Path $stage 'bin')
foreach ($s in 'install.ps1', 'uninstall.ps1', 'install-claude-hooks.ps1') {
  Copy-Script (Join-Path $repo "scripts\$s") (Join-Path $stage "scripts\$s")
}
Copy-Script (Join-Path $repo 'packaging\register-dev.ps1') (Join-Path $stage 'packaging\register-dev.ps1')
Copy-Item (Join-Path $repo 'packaging\AppxManifest.xml') (Join-Path $stage 'packaging')
foreach ($f in 'StoreLogo.png', 'Square150x150Logo.png', 'Square44x44Logo.png') {
  Copy-Item (Join-Path $repo "packaging\icons\$f") (Join-Path $stage 'packaging\icons')
}

# Lanceurs : double clic, sans changer la stratégie d'exécution de PowerShell de la machine.
$cmd = @'
@echo off
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\{0}" {1} %*
pause
'@
[IO.File]::WriteAllText((Join-Path $stage 'Installer.cmd'), ($cmd -f 'install.ps1', '-From "%~dp0bin"').Replace("`n", "`r`n"))
[IO.File]::WriteAllText((Join-Path $stage 'Desinstaller.cmd'), ($cmd -f 'uninstall.ps1', '').Replace("`n", "`r`n"))

$readme = @"
Horizon $version : barre d'outils en haut de chaque écran (Windows 10 et 11).

INSTALLER
  Double-cliquer sur Installer.cmd. Horizon est copié dans %LOCALAPPDATA%\Programs\Horizon,
  démarre avec Windows et apparaît dans le menu Démarrer. Pas besoin de droits administrateur.

MISES À JOUR
  Horizon cherche une nouvelle version sur GitHub au démarrage puis deux fois par jour, la
  télécharge et la vérifie, puis l'installe au démarrage suivant (ou tout de suite : clic sur
  l'annonce, ou Réglages d'Horizon > État). À couper dans Réglages d'Horizon > Fonctions.

  Windows peut afficher « Windows a protégé votre ordinateur » (programme non signé) :
  « Informations complémentaires », puis « Exécuter quand même ».

  Notifications des autres applis (cloche, îlot) : il faut le mode développeur de Windows
  (Paramètres > Système > Espace développeurs), puis relancer Installer.cmd. Sans lui, tout le
  reste marche.

  Claude Code : si tu l'utilises, l'installation ajoute ses hooks (sessions, autorisations) dans
  %USERPROFILE%\.claude\settings.json (copie de sauvegarde à côté). Pour ne pas les ajouter :
  dans une invite de commandes, « Installer.cmd -NoClaudeHooks ».

UTILISER
  Clic droit sur une barre > « Modifier la barre » : glisser les widgets, « + » pour en ajouter,
  clic droit sur un widget pour ses réglages. Ctrl+Alt+P : palette de commandes.
  Réglages et journal : %APPDATA%\Horizon.

DÉSINSTALLER
  Double-cliquer sur Desinstaller.cmd (garder ce dossier pour ça). Les réglages restent dans
  %APPDATA%\Horizon ; « Desinstaller.cmd -RemoveConfig -RemoveClaudeHooks » retire tout.
"@
[IO.File]::WriteAllText((Join-Path $stage 'LISEZMOI.txt'), $readme.Replace("`r`n", "`n").Replace("`n", "`r`n"), [Text.UTF8Encoding]::new($true))

$readmeEn = @"
Horizon $version: a toolbar at the top of every screen (Windows 10 and 11). The interface is in
French.

INSTALL
  Double-click Installer.cmd. Horizon is copied to %LOCALAPPDATA%\Programs\Horizon, starts with
  Windows and shows up in the Start menu. No administrator rights needed.
  "Windows protected your PC" (unsigned program): "More info", then "Run anyway".
  Notifications from other apps need Windows developer mode (Settings > System > For
  developers), then run Installer.cmd again. Everything else works without it.

UPDATES
  Horizon looks for a new version on GitHub at startup and twice a day, downloads and checks
  it, then installs it at the next start (or right away: click the announcement, or Réglages
  d'Horizon > État). Turn it off in Réglages d'Horizon > Fonctions.

UNINSTALL
  Double-click Desinstaller.cmd. Settings stay in %APPDATA%\Horizon;
  "Desinstaller.cmd -RemoveConfig -RemoveClaudeHooks" removes everything.
"@
[IO.File]::WriteAllText((Join-Path $stage 'README.txt'), $readmeEn.Replace("`r`n", "`n").Replace("`n", "`r`n"), [Text.UTF8Encoding]::new($true))

$zip = Join-Path $Out "Horizon-$version.zip"
Compress-Archive -Path $stage -DestinationPath $zip -Force
$size = [math]::Round((Get-Item $zip).Length / 1MB, 1)

# Fichiers de la mise à jour automatique : exécutables seuls et leurs empreintes SHA-256
# (format de sha256sum : « <empreinte>  <nom> », un par ligne).
$sums = foreach ($exe in 'horizon.exe', 'horizon-hook.exe') {
  Copy-Item (Join-Path $release $exe) (Join-Path $Out $exe) -Force
  $hash = (Get-FileHash -Algorithm SHA256 (Join-Path $Out $exe)).Hash.ToLowerInvariant()
  "$hash  $exe"
}
[IO.File]::WriteAllText((Join-Path $Out 'SHA256SUMS'), (($sums -join "`n") + "`n"))
Write-Host "Paquet prêt : $zip ($size Mo), avec horizon.exe, horizon-hook.exe et SHA256SUMS"

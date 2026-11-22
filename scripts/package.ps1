# Prépare une version publiée d'Horizon, celle des Releases GitHub :
#   - compile Horizon et son hook Claude Code en release avec
#     HORIZON_RELEASE=1 (mise à jour automatique active, voir natif/src/update.rs), dans un
#     dossier de compilation à part (« <dossier habituel>-share ») pour ne pas toucher à une
#     compilation locale ;
#   - en fait l'installateur <Out>\Horizon-Setup-<version>.exe (Inno Setup 6,
#     packaging\horizon.iss), avec les scripts qu'il lance (identité de paquet, hooks) ;
#   - pose à côté horizon.exe, horizon-hook.exe et SHA256SUMS : ce que télécharge la mise à jour
#     automatique (à joindre tous les quatre à la Release).
# Rien n'est installé ni modifié sur ce PC. Il faut Inno Setup 6 (déjà là sur les machines de
# GitHub Actions ; ailleurs : winget install JRSoftware.InnoSetup).
#   .\scripts\package.ps1                  # installateur dans dist-share\ (ignoré par git)
#   .\scripts\package.ps1 -Out C:\Temp     # ailleurs
#   .\scripts\package.ps1 -NoBuild         # reprend la dernière compilation partagée
#   .\scripts\package.ps1 -Timings         # et le rapport de durée de cargo (cargo-timings\ du dossier de compilation)
param([string]$Out, [switch]$NoBuild, [switch]$Timings)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Out) { $Out = Join-Path $repo 'dist-share' }
# Chemin absolu : les appels .NET ([IO.File]) ne suivent pas le dossier courant de PowerShell.
New-Item -ItemType Directory -Force $Out | Out-Null
$Out = (Resolve-Path $Out).Path

# Avant la compilation (plus de dix minutes) : inutile de la faire sans de quoi l'emballer.
$iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $iscc) {
  $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe" |
    Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $iscc) { throw 'Inno Setup 6 introuvable : winget install JRSoftware.InnoSetup' }

Push-Location $repo
try {
  $meta = & cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
} finally { Pop-Location }
$target = $meta.target_directory
$version = ($meta.packages | Where-Object name -eq 'horizon-natif').version
$shareTarget = "$target-share"
$release = Join-Path $shareTarget 'release'
$exes = 'horizon.exe', 'horizon-hook.exe'

if (-not $NoBuild) {
  $env:HORIZON_RELEASE = '1'
  $env:CARGO_TARGET_DIR = $shareTarget
  try {
    Write-Host "Compilation d'Horizon et de son hook Claude Code (édition publique)…"
    Push-Location $repo
    try {
      $timings = if ($Timings) { '--timings' }
      & cargo build --release -p horizon-natif -p horizon-hook $timings
      if ($LASTEXITCODE) { throw "compilation d'Horizon échouée" }
    } finally { Pop-Location }
  } finally {
    Remove-Item Env:HORIZON_RELEASE, Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
  }
}
foreach ($exe in $exes) {
  if (-not (Test-Path (Join-Path $release $exe))) { throw "$exe absent de $release : lancer sans -NoBuild" }
}

# Copie d'un script en UTF-8 avec BOM : Windows PowerShell 5.1 (celui de tous les PC, lancé par
# l'installateur) lit un fichier sans BOM comme de l'ANSI et y prend certains octets des
# caractères accentués ou typographiques pour des guillemets.
function Copy-Script([string]$from, [string]$to) {
  $text = [IO.File]::ReadAllText($from, [Text.UTF8Encoding]::new($false))
  [IO.File]::WriteAllText($to, $text, [Text.UTF8Encoding]::new($true))
}

# Ce que l'installateur pose dans <dossier d'installation>\setup : register-dev.ps1 attend le
# manifeste et le dossier icons à côté de lui.
$stage = Join-Path $Out 'setup'
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force (Join-Path $stage 'icons') | Out-Null
Copy-Script (Join-Path $repo 'scripts\install-claude-hooks.ps1') (Join-Path $stage 'install-claude-hooks.ps1')
Copy-Script (Join-Path $repo 'packaging\register-dev.ps1') (Join-Path $stage 'register-dev.ps1')
Copy-Item (Join-Path $repo 'packaging\AppxManifest.xml') $stage
foreach ($f in 'StoreLogo.png', 'Square150x150Logo.png', 'Square44x44Logo.png') {
  Copy-Item (Join-Path $repo "packaging\icons\$f") (Join-Path $stage 'icons')
}

$setup = "Horizon-Setup-$version.exe"
& $iscc /Qp "/DVersion=$version" "/DBin=$release" "/DStage=$stage" "/O$Out" (Join-Path $repo 'packaging\horizon.iss')
if ($LASTEXITCODE) { throw "compilation de l'installateur échouée" }
Remove-Item -Recurse -Force $stage
$size = [math]::Round((Get-Item (Join-Path $Out $setup)).Length / 1MB, 1)

# Fichiers de la mise à jour automatique : exécutables seuls. SHA256SUMS donne leurs empreintes
# et celle de l'installateur (format de sha256sum : « <empreinte>  <nom> », un par ligne).
foreach ($exe in $exes) { Copy-Item (Join-Path $release $exe) (Join-Path $Out $exe) -Force }
$sums = foreach ($f in $exes + $setup) {
  $hash = (Get-FileHash -Algorithm SHA256 (Join-Path $Out $f)).Hash.ToLowerInvariant()
  "$hash  $f"
}
[IO.File]::WriteAllText((Join-Path $Out 'SHA256SUMS'), (($sums -join "`n") + "`n"))
Write-Host "Installateur prêt : $(Join-Path $Out $setup) ($size Mo), avec horizon.exe, horizon-hook.exe et SHA256SUMS"

# Vérification du projet avant de dire « terminé » : clippy puis les tests de tout le workspace.
#
# L'interface Slint y est interprétée, comme dans toute commande cargo sans option (fonction
# « live », par défaut) : ses .slint sont chargés à l'exécution au lieu d'être traduits en Rust,
# ce qui évite la partie de loin la plus longue de la compilation. L'exe publié, lui, est
# toujours compilé (--no-default-features) : -Compile vérifie cette forme-là, comme le fait la CI
# à chaque poussée.
#   .\scripts\verifier.ps1             # clippy + tests, interface interprétée
#   .\scripts\verifier.ps1 -Exe        # puis compile l'exe de debug (contrôle en vrai)
#   .\scripts\verifier.ps1 -Compile    # la même chose, interface compilée
param([switch]$Compile, [switch]$Exe)

$ErrorActionPreference = 'Stop'
$features = if ($Compile) { , '--no-default-features' } else { @() }
$steps = @(
  , (@('clippy', '--workspace', '--all-targets') + $features + '--', '-D', 'warnings')
  , (@('test', '--workspace') + $features)
)
if ($Exe) { $steps += , (@('build', '-p', 'horizon-natif') + $features) }

Push-Location (Split-Path -Parent $PSScriptRoot)
try {
  foreach ($step in $steps) {
    Write-Host "cargo $step"
    & cargo @step
    if ($LASTEXITCODE) { throw "échec : cargo $step" }
  }
} finally { Pop-Location }

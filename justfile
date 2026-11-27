# Recettes communes à mes projets : `just verifier`, `just installer` (détails dans CLAUDE.md).
set windows-shell := ["pwsh.exe", "-NoLogo", "-NoProfile", "-Command"]

# Clippy (-D warnings) puis tous les tests du workspace
verifier:
    .\scripts\verifier.ps1

# Pareil, plus l'exe de debug pour l'essai en vrai
exe:
    .\scripts\verifier.ps1 -Exe

# Pareil, avec l'interface compilée comme dans la CI
compile:
    .\scripts\verifier.ps1 -Compile

# Logo animé du README : écrit docs\images\logo-anime.svg.
#   .\scripts\logo-anime.ps1
#
# L'animation « barre et pince » : la barre fine d'Horizon se tend, descend comme un panneau pour
# former le bloc, puis l'encoche et l'arche se referment ensemble et le bloc s'écrase un instant
# avant de reprendre sa forme.
#
# Un README GitHub n'exécute aucun script : l'animation est donc écrite dans le fichier, en SMIL,
# sur UN seul tracé dont le dessin change à chaque image (ni masque ni découpe, que tous les
# navigateurs n'animent pas dans une image). À chaque image le tracé garde les commandes de
# docs\images\logo.svg, sur lequel il finit exactement : le script le contrôle.
#
# Elle rejoue toutes les $cycle secondes : sur la page du dépôt le README est sous la liste des
# fichiers, et une lecture unique passerait inaperçue. Qui a demandé moins d'animations
# (prefers-reduced-motion) voit le logo fixe.

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$inv = [System.Globalization.CultureInfo]::InvariantCulture

$fps = 60
$cycle = 9.0        # secondes d'une lecture à la suivante
$fade = 0.3         # fondu de sortie, juste avant de rejouer

# Durées des gestes, en secondes, dans l'ordre où ils s'enchaînent.
$barTime = 0.40     # la barre se tend depuis son centre
$dropTime = 0.45    # elle descend et se resserre en bloc
$rest = 0.03        # le bloc attend
$cutTime = 0.22     # l'encoche et l'arche se referment
$squashTime = 0.07  # le bloc s'écrase
$springTime = 0.55  # et reprend sa forme

$dropAt = $barTime
$cutAt = $dropAt + $dropTime + $rest
$squashAt = $cutAt + $cutTime
$springAt = $squashAt + $squashTime
$total = $springAt + $springTime

# Courbes, celles de GSAP (banc d'essai de l'animation) : power2 = cube, power3 = puissance 4…
function Get-PowerIn([double]$p, [int]$n) { [Math]::Pow($p, $n) }
function Get-PowerOut([double]$p, [int]$n) { 1 - [Math]::Pow(1 - $p, $n) }
function Get-PowerInOut([double]$p, [int]$n) {
  if ($p -lt 0.5) { [Math]::Pow(2 * $p, $n) / 2 } else { 1 - [Math]::Pow(2 * (1 - $p), $n) / 2 }
}
function Get-ElasticOut([double]$p, [double]$amplitude, [double]$period) {
  if ($p -ge 1) { return 1 }
  $shift = $period / (2 * [Math]::PI) * [Math]::Asin(1 / $amplitude)
  $amplitude * [Math]::Pow(2, -10 * $p) * [Math]::Sin(($p - $shift) * 2 * [Math]::PI / $period) + 1
}

# Avancement d'un geste (0 avant, 1 après) à l'instant $t. Les bornes sont écrites en décimal :
# avec « 0 » et « 1 », PowerShell choisit Max et Min sur des entiers et arrondit l'avancement.
function Get-Progress([double]$t, [double]$start, [double]$duration) {
  [Math]::Max(0.0, [Math]::Min(1.0, ($t - $start) / $duration))
}
function Get-Mix([double]$from, [double]$to, [double]$p) { $from + ($to - $from) * $p }

function Format-Number([double]$value) {
  $rounded = [Math]::Round($value, 2)
  if ($rounded -eq 0) { $rounded = 0 }    # pas de « -0 »
  $rounded.ToString('0.##', $inv)
}

# Le tracé à l'instant $t, dans le repère de 48 unités du logo.
function Get-Outline([double]$t) {
  $bar = Get-PowerOut (Get-Progress $t 0 $barTime) 4
  $drop = Get-PowerInOut (Get-Progress $t $dropAt $dropTime) 5
  $cut = Get-PowerIn (Get-Progress $t $cutAt $cutTime) 4
  $squash = Get-PowerOut (Get-Progress $t $squashAt $squashTime) 3
  $spring = Get-ElasticOut (Get-Progress $t $springAt $springTime) 1.2 0.35

  # Le bloc : une barre de 64 × 3 qui devient le carré arrondi de 36.
  $left = Get-Mix (24 - 32 * $bar) 6 $drop
  $right = Get-Mix (24 + 32 * $bar) 42 $drop
  $top = 6.0
  $bottom = Get-Mix 9 42 $drop
  $corner = [Math]::Min((Get-Mix 1.5 6 $drop), [Math]::Min(($right - $left) / 2, ($bottom - $top) / 2))

  # L'encoche, un demi-disque de rayon 6 qui descend dans le haut du bloc. Tant que son centre
  # est au-dessus du bord, seule une calotte entame le bloc : l'arc part alors du bord lui-même.
  $notchCenter = 9 - 14 * (1 - $cut)
  $notchFrom = 24.0; $notchTo = 24.0; $notchY = $top
  if ($notchCenter -ge $top) {
    $notchFrom = 18; $notchTo = 30; $notchY = $notchCenter
  } elseif ($notchCenter + 6 -gt $top) {
    $half = [Math]::Sqrt(36 - [Math]::Pow($top - $notchCenter, 2))
    $notchFrom = 24 - $half; $notchTo = 24 + $half
  }

  # L'arche, la même chose en montant dans le bas du bloc.
  $archCenter = 30 + 22 * (1 - $cut)
  $archFrom = 24.0; $archTo = 24.0; $archY = $bottom
  if ($archCenter -le $bottom) {
    $archFrom = 30; $archTo = 18; $archY = $archCenter
  } elseif ($archCenter - 6 -lt $bottom) {
    $half = [Math]::Sqrt(36 - [Math]::Pow($archCenter - $bottom, 2))
    $archFrom = 24 + $half; $archTo = 24 - $half
  }

  # L'écrasement : une échelle autour du centre, appliquée aux points et aux rayons.
  $scaleX = 1 + 0.05 * $squash * (1 - $spring)
  $scaleY = 1 - 0.10 * $squash * (1 - $spring)
  $x = { param($v) Format-Number (24 + ($v - 24) * $scaleX) }
  $y = { param($v) Format-Number (24 + ($v - 24) * $scaleY) }
  $hole = '{0} {1}' -f (Format-Number (6 * $scaleX)), (Format-Number (6 * $scaleY))
  $round = '{0} {1}' -f (Format-Number ($corner * $scaleX)), (Format-Number ($corner * $scaleY))

  # Les mêmes commandes, dans le même ordre, que docs\images\logo.svg.
  'M' + (& $x ($left + $corner)) + ' ' + (& $y $top) +
  'H' + (& $x $notchFrom) + 'V' + (& $y $notchY) +
  'A' + $hole + ' 0 0 0 ' + (& $x $notchTo) + ' ' + (& $y $notchY) +
  'V' + (& $y $top) + 'H' + (& $x ($right - $corner)) +
  'A' + $round + ' 0 0 1 ' + (& $x $right) + ' ' + (& $y ($top + $corner)) +
  'V' + (& $y ($bottom - $corner)) +
  'A' + $round + ' 0 0 1 ' + (& $x ($right - $corner)) + ' ' + (& $y $bottom) +
  'H' + (& $x $archFrom) + 'V' + (& $y $archY) +
  'A' + $hole + ' 0 0 0 ' + (& $x $archTo) + ' ' + (& $y $archY) +
  'V' + (& $y $bottom) + 'H' + (& $x ($left + $corner)) +
  'A' + $round + ' 0 0 1 ' + (& $x $left) + ' ' + (& $y ($bottom - $corner)) +
  'V' + (& $y ($top + $corner)) +
  'A' + $round + ' 0 0 1 ' + (& $x ($left + $corner)) + ' ' + (& $y $top) + 'Z'
}

# Une image tous les 1/$fps, la dernière à la fin exacte de l'animation.
$count = [int][Math]::Ceiling($total * $fps)
$times = New-Object System.Collections.Generic.List[string]
$outlines = New-Object System.Collections.Generic.List[string]
for ($i = 0; $i -le $count; $i++) {
  $t = [Math]::Min($i / $fps, $total)
  $times.Add(($t / $cycle).ToString('0.#####', $inv))
  $outlines.Add((Get-Outline $t))
}

# L'animation doit finir sur le tracé de référence, au caractère près.
$reference = [regex]::Match((Get-Content -Raw (Join-Path $root 'docs\images\logo.svg')), ' d="([^"]+)"').Groups[1].Value
$last = $outlines[$outlines.Count - 1]
if ($last -ne $reference) {
  throw "Le dernier dessin ne retombe pas sur docs\images\logo.svg :`n  $last`n  $reference"
}

# Le tracé reste en place jusqu'à la fin du cycle, où il s'efface avant de rejouer.
$times.Add('1')
$outlines.Add($last)
$fadeAt = (($cycle - $fade) / $cycle).ToString('0.#####', $inv)
$duration = $cycle.ToString('0.##', $inv) + 's'

$svg = @(
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="-10 0 68 48" width="136" height="96" role="img" aria-label="Horizon">'
  '  <!-- Produit par scripts/logo-anime.ps1 : changer le script, pas ce fichier. -->'
  '  <style>.fixe{display:none}@media (prefers-reduced-motion:reduce){.anime{display:none}.fixe{display:inline}}</style>'
  ('  <path class="fixe" fill="#f97316" d="{0}"/>' -f $reference)
  ('  <path class="anime" fill="#f97316" d="{0}">' -f $reference)
  ('    <animate attributeName="d" dur="{0}" repeatCount="indefinite" keyTimes="{1}" values="{2}"/>' -f $duration, ($times -join ';'), ($outlines -join ';'))
  ('    <animate attributeName="opacity" dur="{0}" repeatCount="indefinite" keyTimes="0;{1};1" values="1;1;0"/>' -f $duration, $fadeAt)
  '  </path>'
  '</svg>'
  ''
) -join "`n"

$file = Join-Path $root 'docs\images\logo-anime.svg'
[System.IO.File]::WriteAllText($file, $svg, (New-Object System.Text.UTF8Encoding $false))
Write-Host ('{0} : {1} images, {2:0.00} s, {3:0.0} Ko' -f $file, $outlines.Count, $total, ($svg.Length / 1024))

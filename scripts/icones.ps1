# Icônes d'Horizon : redessine le symbole à chaque taille, calé sur le pixel, et écrit tous les
# fichiers qui le portent.
#   .\scripts\icones.ps1
#
# Le symbole est un H d'un seul bloc : deux montants, une traverse remontée (la barre, en haut de
# l'écran), une encoche au-dessus et une arche au-dessous, toutes deux finies en demi-cercle.
# Référence vectorielle sur 48 unités : docs\images\logo.svg (le même tracé, à garder d'accord
# avec la ligne « 48 » ci-dessous).
#
# Une réduction automatique poserait les arêtes entre deux pixels (traverse floue à 20, 24 et
# 40 px) : chaque taille a donc sa géométrie en pixels entiers.
#
# Fichiers écrits :
#   natif\icons\horizon.ico              icône de l'exe, des fenêtres et de l'installateur
#   natif\icons\horizon-<taille>.png     icône de la zone de notification (natif\src\tray.rs)
#   packaging\icons\*.png                paquet « sparse » (notifications, AppxManifest.xml)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$root = Split-Path -Parent $PSScriptRoot
$color = [System.Drawing.ColorTranslator]::FromHtml('#f97316')

# Par taille : marge, largeur d'un montant, profondeur de l'encoche, épaisseur de la traverse.
# L'encoche et l'arche prennent la largeur qui reste entre les montants ; l'arche, la hauteur
# qui reste sous la traverse. Le rayon des coins vaut un huitième de la taille. Jusqu'à 40 px la
# marge est plus étroite que dans la référence : le symbole garde sa présence à côté des autres
# icônes de la zone de notification.
$geometry = @{
  16  = 1, 5, 4, 3
  20  = 2, 5, 4, 4
  24  = 2, 7, 5, 5
  32  = 3, 9, 7, 6
  40  = 4, 11, 8, 8
  44  = 6, 11, 8, 8
  48  = 6, 12, 9, 9
  50  = 6, 13, 10, 9
  64  = 8, 16, 12, 12
  150 = 19, 37, 28, 28
  256 = 32, 64, 48, 48
}
$icoSizes = 16, 20, 24, 32, 40, 48, 64, 256
$traySizes = 16, 20, 24, 32, 40, 48
$packageFiles = @{ 'StoreLogo.png' = 50; 'Square44x44Logo.png' = 44; 'Square150x150Logo.png' = 150 }

# Le symbole à la taille $size, sur fond transparent.
function New-Mark([int]$size) {
  $margin, $stem, $notch, $bar = $geometry[$size]
  [single]$r = $size / 8                              # rayon des coins
  [single]$n = ($size - 2 * $margin - 2 * $stem) / 2  # rayon de l'encoche et de l'arche
  [single]$x0 = $margin; [single]$x1 = $margin + $stem
  [single]$x2 = $size - $margin - $stem; [single]$x3 = $size - $margin
  [single]$y0 = $margin; [single]$y3 = $size - $margin
  [single]$yn = $margin + $notch                      # fond de l'encoche
  [single]$ya = $yn + $bar                            # sommet de l'arche
  [single]$d = 2 * $r; [single]$w = 2 * $n

  # Contour dans le sens des aiguilles d'une montre, depuis le haut du montant gauche.
  $path = New-Object System.Drawing.Drawing2D.GraphicsPath
  $path.AddLine([single]($x0 + $r), $y0, $x1, $y0)
  $path.AddLine($x1, $y0, $x1, [single]($yn - $n))
  $path.AddArc($x1, [single]($yn - $w), $w, $w, [single]180, [single]-180)
  $path.AddLine($x2, [single]($yn - $n), $x2, $y0)
  $path.AddLine($x2, $y0, [single]($x3 - $r), $y0)
  $path.AddArc([single]($x3 - $d), $y0, $d, $d, [single]270, [single]90)
  $path.AddArc([single]($x3 - $d), [single]($y3 - $d), $d, $d, [single]0, [single]90)
  $path.AddLine([single]($x3 - $r), $y3, $x2, $y3)
  $path.AddLine($x2, $y3, $x2, [single]($ya + $n))
  $path.AddArc($x1, $ya, $w, $w, [single]0, [single]-180)
  $path.AddLine($x1, [single]($ya + $n), $x1, $y3)
  $path.AddLine($x1, $y3, [single]($x0 + $r), $y3)
  $path.AddArc($x0, [single]($y3 - $d), $d, $d, [single]90, [single]90)
  $path.AddArc($x0, $y0, $d, $d, [single]180, [single]90)
  $path.CloseFigure()

  $bitmap = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
  $brush = New-Object System.Drawing.SolidBrush $color
  try {
    $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    # Une coordonnée entière tombe sur le bord d'un pixel, pas en son centre.
    $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
    $graphics.Clear([System.Drawing.Color]::Transparent)
    $graphics.FillPath($brush, $path)
  } finally {
    $brush.Dispose(); $graphics.Dispose(); $path.Dispose()
  }
  $bitmap
}

function Get-PngBytes([System.Drawing.Bitmap]$bitmap) {
  $stream = New-Object System.IO.MemoryStream
  $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
  , $stream.ToArray()
}

# Image d'une entrée d'ICO au format historique (en-tête, pixels BGRA de bas en haut, masque de
# transparence) : celui que lisent tous les outils en dessous de 256 px.
function Get-DibBytes([System.Drawing.Bitmap]$bitmap) {
  $size = $bitmap.Width
  $area = New-Object System.Drawing.Rectangle 0, 0, $size, $size
  $data = $bitmap.LockBits($area, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $pixels = New-Object byte[] ($size * $size * 4)
  [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $pixels, 0, $pixels.Length)
  $bitmap.UnlockBits($data)

  $maskRow = [int][math]::Floor(($size + 31) / 32) * 4
  $stream = New-Object System.IO.MemoryStream
  $writer = New-Object System.IO.BinaryWriter $stream
  $writer.Write([uint32]40); $writer.Write([int32]$size); $writer.Write([int32]($size * 2))
  $writer.Write([uint16]1); $writer.Write([uint16]32); $writer.Write([uint32]0)
  $writer.Write([uint32]($pixels.Length + $maskRow * $size))
  $writer.Write([int32]0); $writer.Write([int32]0); $writer.Write([uint32]0); $writer.Write([uint32]0)
  for ($y = $size - 1; $y -ge 0; $y--) { $writer.Write($pixels, $y * $size * 4, $size * 4) }
  for ($y = $size - 1; $y -ge 0; $y--) {
    $row = New-Object byte[] $maskRow
    for ($x = 0; $x -lt $size; $x++) {
      if ($pixels[($y * $size + $x) * 4 + 3] -eq 0) { $row[$x -shr 3] = $row[$x -shr 3] -bor (0x80 -shr ($x -band 7)) }
    }
    $writer.Write($row)
  }
  $writer.Flush()
  , $stream.ToArray()
}

function Write-File([string]$relative, [byte[]]$bytes) {
  $path = Join-Path $root $relative
  [System.IO.File]::WriteAllBytes($path, $bytes)
  Write-Host ("{0,8} octets  {1}" -f $bytes.Length, $relative)
}

$marks = @{}
foreach ($size in $geometry.Keys) { $marks[$size] = New-Mark $size }

# ICO : une image par taille ; 256 px en PNG, comme le veut le format.
$images = foreach ($size in $icoSizes) {
  if ($size -eq 256) { , (Get-PngBytes $marks[$size]) } else { , (Get-DibBytes $marks[$size]) }
}
$stream = New-Object System.IO.MemoryStream
$writer = New-Object System.IO.BinaryWriter $stream
$writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$icoSizes.Count)
$offset = 6 + 16 * $icoSizes.Count
for ($i = 0; $i -lt $icoSizes.Count; $i++) {
  $side = [byte]($icoSizes[$i] % 256)   # 0 veut dire 256
  $writer.Write($side); $writer.Write($side); $writer.Write([byte]0); $writer.Write([byte]0)
  $writer.Write([uint16]1); $writer.Write([uint16]32)
  $writer.Write([uint32]$images[$i].Length); $writer.Write([uint32]$offset)
  $offset += $images[$i].Length
}
foreach ($image in $images) { $writer.Write($image) }
$writer.Flush()
Write-File 'natif\icons\horizon.ico' $stream.ToArray()

foreach ($size in $traySizes) { Write-File "natif\icons\horizon-$size.png" (Get-PngBytes $marks[$size]) }
foreach ($name in $packageFiles.Keys) { Write-File "packaging\icons\$name" (Get-PngBytes $marks[$packageFiles[$name]]) }

foreach ($mark in $marks.Values) { $mark.Dispose() }

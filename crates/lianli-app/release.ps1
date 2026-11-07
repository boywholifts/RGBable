# Sets RGBable's version in Cargo.toml, tauri.conf.json and Cargo.lock.
#   .\release.ps1 0.3.0          bump, add a CHANGELOG section if missing
#   .\release.ps1 0.3.0 -Tag     also commit and create the tag v0.3.0 (push it to publish the release)
#   .\release.ps1 0.3.0-dev.7 -CiOnly   what the GitHub workflow uses for dev builds (no CHANGELOG, no git)
param(
  [Parameter(Mandatory = $true)][string]$Version,
  [switch]$Tag,
  [switch]$CiOnly
)
$ErrorActionPreference = 'Stop'
$Version = $Version.TrimStart('v')
if ($Version -notmatch '^\d+\.\d+\.\d+(-dev\.\d+)?$') { throw "Version must look like 1.2.3 or 1.2.3-dev.4, got '$Version'" }

$here = $PSScriptRoot
$utf8 = New-Object System.Text.UTF8Encoding($false)

$cargo = Join-Path $here 'Cargo.toml'
$text = [IO.File]::ReadAllText($cargo)
$text = [regex]::new('(?m)^version = "[^"]*"').Replace($text, "version = `"$Version`"", 1)
[IO.File]::WriteAllText($cargo, $text, $utf8)

$conf = Join-Path $here 'tauri.conf.json'
$text = [IO.File]::ReadAllText($conf)
$text = [regex]::new('"version":\s*"[^"]*"').Replace($text, "`"version`": `"$Version`"", 1)
[IO.File]::WriteAllText($conf, $text, $utf8)

Push-Location $here
try {
  cargo metadata --format-version 1 --offline *> $null
  if ($LASTEXITCODE -ne 0) { cargo metadata --format-version 1 *> $null }
} finally { Pop-Location }

if ($CiOnly) { Write-Host "RGBable version $Version"; exit 0 }

$log = Join-Path $here 'CHANGELOG.md'
$text = [IO.File]::ReadAllText($log)
if ($text -notmatch "(?m)^## \[$([regex]::Escape($Version))\]") {
  $date = Get-Date -Format 'yyyy-MM-dd'
  $text = [regex]::new('(?m)^## \[Unreleased\]\r?\n').Replace($text, "## [Unreleased]`n`n## [$Version] - $date`n", 1)
  [IO.File]::WriteAllText($log, $text, $utf8)
  Write-Host "The Unreleased notes in CHANGELOG.md are now listed under $Version."
}

if ($Tag) {
  $root = git -C $here rev-parse --show-toplevel
  git -C $root add crates/lianli-app/Cargo.toml crates/lianli-app/tauri.conf.json crates/lianli-app/CHANGELOG.md Cargo.lock
  git -C $root commit -m "chore(app): release RGBable $Version"
  git -C $root tag "v$Version"
  Write-Host "Tagged v$Version. Publish it with: git push origin HEAD v$Version"
} else {
  Write-Host "RGBable version set to $Version"
}

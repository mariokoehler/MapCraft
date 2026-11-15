# Writes THIRD-PARTY-NOTICES.txt for a release (design.md M7): the crates' licences from
# cargo-about (about.toml, about.hbs), then the fonts' and icons' licences.
# Usage: pwsh scripts/notices.ps1 [output file]   (needs `cargo install cargo-about --features cli`)
param([string]$Out = "target/release/THIRD-PARTY-NOTICES.txt")
$ErrorActionPreference = "Stop"
cargo about generate about.hbs -o $Out
if ($LASTEXITCODE -ne 0) { throw "cargo about failed" }
$rule = "-" * 80
$extra = @"

$rule
Fonts: IBM Plex Sans and IBM Plex Mono (assets/fonts), SIL Open Font License 1.1

$(Get-Content -Raw assets/fonts/OFL.txt)

$rule
Icons: Lucide (assets/icons), ISC License

$(Get-Content -Raw assets/icons/LICENSE)
"@
[IO.File]::AppendAllText((Resolve-Path $Out), $extra, [Text.UTF8Encoding]::new($false))
Write-Output "wrote $Out"

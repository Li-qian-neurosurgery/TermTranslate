param([switch]$Publish, [switch]$SkipTests)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $projectRoot
$workspaceTools = Join-Path (Split-Path -Parent $projectRoot) '.tools'
if (Test-Path -LiteralPath (Join-Path $workspaceTools 'cargo\bin\cargo.exe')) {
    $env:CARGO_HOME = Join-Path $workspaceTools 'cargo'
    $env:RUSTUP_HOME = Join-Path $workspaceTools 'rustup'
    $env:PATH = (Join-Path $env:CARGO_HOME 'bin') + ';' + $env:PATH
}
$env:npm_config_cache = Join-Path $workspaceTools 'npm-cache'
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'Install Rust (MSVC), Microsoft C++ Build Tools and Windows SDK first.' }
if (-not (Test-Path -LiteralPath 'node_modules')) { & npm ci --no-audit --no-fund; if ($LASTEXITCODE) { throw 'npm ci failed' } }
& npm run build
if ($LASTEXITCODE) { throw 'Frontend build failed' }
if (-not $SkipTests) {
    & npm test
    if ($LASTEXITCODE) { throw 'Frontend tests failed' }
    & cargo test --manifest-path src-tauri\Cargo.toml --locked
    if ($LASTEXITCODE) { throw 'Rust tests failed' }
}
if ($Publish) {
    & npm run tauri -- build --bundles nsis -- --locked
    if ($LASTEXITCODE) { throw 'Desktop build failed' }
    $releaseRoot = Join-Path $projectRoot 'release'
    $version = (Get-Content -Raw -LiteralPath (Join-Path $projectRoot 'package.json') | ConvertFrom-Json).version
    $portable = Join-Path $releaseRoot "TermTranslate-$version-win-x64"
    New-Item -ItemType Directory -Force -Path $portable | Out-Null
    Copy-Item -LiteralPath 'src-tauri\target\release\termtranslate.exe' -Destination (Join-Path $portable 'TermTranslate.exe')
    Copy-Item -LiteralPath 'README.md' -Destination $portable
    $portableDocs = Join-Path $portable 'docs'
    New-Item -ItemType Directory -Force -Path $portableDocs | Out-Null
    Get-ChildItem -LiteralPath 'docs' -File | Copy-Item -Destination $portableDocs
    Copy-Item -LiteralPath "src-tauri\target\release\bundle\nsis\TermTranslate_${version}_x64-setup.exe" -Destination $releaseRoot
    Compress-Archive -LiteralPath $portable -DestinationPath (Join-Path $releaseRoot "TermTranslate-$version-win-x64.zip") -Force
    & (Join-Path $PSScriptRoot 'package-source.ps1') -Version $version
    $hashLines = Get-ChildItem -LiteralPath $releaseRoot -File | Where-Object { $_.Extension -in @('.exe', '.zip') } | Sort-Object Name | ForEach-Object {
        $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $($_.Name)"
    }
    $hashLines | Set-Content -LiteralPath (Join-Path $releaseRoot 'SHA256SUMS.txt') -Encoding ASCII
    Get-ChildItem -LiteralPath $releaseRoot | Select-Object Name,Length
}

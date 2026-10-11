param([string]$Version)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
if (-not $Version) { $Version = (Get-Content -Raw -LiteralPath (Join-Path $projectRoot 'package.json') | ConvertFrom-Json).version }
$releaseRoot = Join-Path $projectRoot 'release'
New-Item -ItemType Directory -Force -Path $releaseRoot | Out-Null
$zipPath = Join-Path $releaseRoot "TermTranslate-$Version-source.zip"
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
# An explicit allowlist keeps build caches, local fixtures, and user data out of source archives.
$rootFiles = @('.gitignore', '.gitattributes', 'README.md', 'package.json', 'package-lock.json', 'tsconfig.json', 'vite.config.ts', 'index.html', 'app-icon.svg')
$tauriFiles = @('Cargo.toml', 'Cargo.lock', 'build.rs', 'tauri.conf.json', 'tauri.macos.conf.json', 'Entitlements.plist')
$sourceFiles = @($rootFiles | ForEach-Object { Get-Item -LiteralPath (Join-Path $projectRoot $_) })
$sourceFiles += @($tauriFiles | ForEach-Object { Get-Item -LiteralPath (Join-Path $projectRoot "src-tauri\$_") })
foreach ($directory in @('.github\workflows', 'src', 'scripts', 'docs', 'src-tauri\src', 'src-tauri\capabilities', 'src-tauri\icons')) {
    $sourceFiles += Get-ChildItem -LiteralPath (Join-Path $projectRoot $directory) -Recurse -File
}
$fileStream = [System.IO.File]::Open($zipPath, [System.IO.FileMode]::Create)
$archive = [System.IO.Compression.ZipArchive]::new($fileStream, [System.IO.Compression.ZipArchiveMode]::Create)
try {
    foreach ($file in ($sourceFiles | Sort-Object FullName -Unique)) {
        $relative = $file.FullName.Substring($projectRoot.Length + 1).Replace('\', '/')
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $file.FullName, "TermTranslate/$relative", [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
    }
} finally {
    $archive.Dispose()
    $fileStream.Dispose()
}
Get-Item -LiteralPath $zipPath | Select-Object Name, Length

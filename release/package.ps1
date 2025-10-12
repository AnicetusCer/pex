param (
    [string]$BinaryPath = "..\target\release\pex.exe",
    [string]$OutputDir = "dist",
    [switch]$Zip
)

$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$distPath = Join-Path $scriptDir $OutputDir

if (Test-Path $distPath) {
    Remove-Item $distPath -Recurse -Force
}

New-Item -ItemType Directory -Path $distPath | Out-Null

Copy-Item (Join-Path $scriptDir "config.json") (Join-Path $distPath "config.json")
Copy-Item (Join-Path $scriptDir "README.txt") (Join-Path $distPath "README.txt")

if (-not (Test-Path $BinaryPath)) {
    throw "Binary not found at '$BinaryPath'. Build with 'cargo build --release' first or pass -BinaryPath."
}

Copy-Item $BinaryPath (Join-Path $distPath (Split-Path $BinaryPath -Leaf))

if ($Zip.IsPresent) {
    $zipPath = Join-Path $scriptDir "pex-portable.zip"
    if (Test-Path $zipPath) {
        Remove-Item $zipPath
    }
    Compress-Archive -Path (Join-Path $distPath "*") -DestinationPath $zipPath
    Write-Host "Created $zipPath"
} else {
    Write-Host "Portable bundle staged in $distPath"
}

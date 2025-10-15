param (
    [string]$BinaryPath,
    [string]$OutputDir = "dist",
    [switch]$Zip
)

$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptDir
$runtimeInfo = [System.Runtime.InteropServices.RuntimeInformation]
$isWindowsPlatform = $runtimeInfo::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Windows)

if (-not $isWindowsPlatform) {
    throw "This PowerShell packaging script is Windows-only. Use release/package.sh when running on Linux."
}

if (-not $BinaryPath) {
    $releaseDir = Join-Path (Join-Path $repoRoot "target") "release"
    $preferredNames = @("pex.exe", "pex")
    foreach ($name in $preferredNames) {
        $candidate = Join-Path $releaseDir $name
        if (Test-Path $candidate) {
            $BinaryPath = $candidate
            break
        }
    }

    if (-not $BinaryPath) {
        $BinaryPath = Join-Path $releaseDir $preferredNames[0]
    }
}

if ($BinaryPath -and -not [System.IO.Path]::IsPathRooted($BinaryPath)) {
    $scriptRelativeBinary = Join-Path $scriptDir $BinaryPath
    if (Test-Path $scriptRelativeBinary) {
        $BinaryPath = $scriptRelativeBinary
    }
}

$distPath = Join-Path $scriptDir $OutputDir

if (Test-Path $distPath) {
    Remove-Item $distPath -Recurse -Force
}

New-Item -ItemType Directory -Path $distPath | Out-Null

Copy-Item (Join-Path $scriptDir "config.json") (Join-Path $distPath "config.json")
Copy-Item (Join-Path $scriptDir "README.md") (Join-Path $distPath "README.md")
if (Test-Path (Join-Path $repoRoot "LICENSE")) {
    Copy-Item (Join-Path $repoRoot "LICENSE") (Join-Path $distPath "LICENSE")
}
if (Test-Path (Join-Path $repoRoot "NOTICE")) {
    Copy-Item (Join-Path $repoRoot "NOTICE") (Join-Path $distPath "NOTICE")
}

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

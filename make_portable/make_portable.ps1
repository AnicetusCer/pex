param (
    [string]$BinaryPath,
    [string]$OutputDir = "dist",
    [switch]$Zip,
    [string]$ZipName = "pex-portable.zip"
)

$ErrorActionPreference = "Stop"
if (-not $IsWindows) { throw "Use make_portable.sh on Linux." }

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $BinaryPath) { $BinaryPath = Join-Path $repoRoot 'target/release/pex.exe' }
elseif (-not [IO.Path]::IsPathRooted($BinaryPath) -and (Test-Path -LiteralPath (Join-Path $PSScriptRoot $BinaryPath))) {
    $BinaryPath = Join-Path $PSScriptRoot $BinaryPath
}
$binary = Get-Item -LiteralPath $BinaryPath
# Check the PE signature and machine field, including for explicitly supplied binaries.
$reader = [IO.BinaryReader]::new([IO.File]::OpenRead($binary.FullName))
try {
    if ($reader.ReadUInt16() -ne 0x5A4D) { throw "Expected an x86-64 Windows executable." }
    $reader.BaseStream.Position = 0x3c
    $reader.BaseStream.Position = $reader.ReadUInt32()
    if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) {
        throw "Expected an x86-64 Windows executable."
    }
} finally { $reader.Dispose() }

$distPath = [IO.Path]::GetFullPath($OutputDir, $PSScriptRoot)
if ($Zip -and -not [IO.Path]::IsPathRooted($ZipName) -and $ZipName -ne [IO.Path]::GetFileName($ZipName)) {
    throw "Use a file name or an absolute path for -ZipName."
}
if (-not $ZipName.EndsWith('.zip', [StringComparison]::OrdinalIgnoreCase)) { $ZipName += '.zip' }
$zipPath = [IO.Path]::GetFullPath($ZipName, $distPath)
$payload = [ordered]@{
    'pex.exe' = $binary.FullName
    'config.json' = Join-Path $PSScriptRoot 'config.json'
    'README.md' = Join-Path $PSScriptRoot 'README.md'
    'pex_snapshot.py' = Join-Path $PSScriptRoot 'pex_snapshot.py'
    'config_example.windows' = Join-Path $PSScriptRoot 'config_example.windows'
    'config_example.linux' = Join-Path $PSScriptRoot 'config_example.linux'
    'LICENSE' = Join-Path $repoRoot 'LICENSE'
    'NOTICE' = Join-Path $repoRoot 'NOTICE'
}
foreach ($source in $payload.Values) {
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing bundle input: $source" }
}
# Only a staging folder can be reused. Never empty the directory or archive its other files.
$marker = Join-Path $distPath '.pex-package-stage'
if ((Test-Path -LiteralPath $distPath) -and (Get-ChildItem -LiteralPath $distPath -Force) -and
    -not (Test-Path -LiteralPath $marker -PathType Leaf)) {
    # The previous release helper used this dedicated directory without a marker.
    $legacyStage = Join-Path $PSScriptRoot 'dist/windows-x86_64'
    if ($distPath -ne [IO.Path]::GetFullPath($legacyStage) -or -not (Test-Path -LiteralPath (Join-Path $distPath 'pex.exe'))) {
        throw "Choose an empty output directory. '$distPath' contains files from outside this packager."
    }
}
[IO.Directory]::CreateDirectory($distPath) | Out-Null
[IO.File]::WriteAllText($marker, "Pex portable build staging directory")
$files = foreach ($name in $payload.Keys) {
    $destination = Join-Path $distPath $name
    Copy-Item -LiteralPath $payload[$name] -Destination $destination -Force
    $destination
}
if ($Zip) {
    $tempZip = Join-Path ([IO.Path]::GetDirectoryName($zipPath)) ("pex-" + [Guid]::NewGuid() + '.zip')
    try {
        Compress-Archive -LiteralPath $files -DestinationPath $tempZip
        [IO.File]::Move($tempZip, $zipPath, $true)
    } finally {
        if (Test-Path -LiteralPath $tempZip) { Remove-Item -LiteralPath $tempZip }
    }
    Write-Host "Created $zipPath"
} else {
    Write-Host "Portable bundle staged in $distPath"
}

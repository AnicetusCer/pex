#!/usr/bin/env pwsh

Param(
    [Parameter(Mandatory = $true)][string]$Version
)

$ErrorActionPreference = "Stop"

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $scriptRoot "scripts/release_helpers.ps1")

if (-not $IsLinux) {
    throw "release_linux.ps1 must be run from Linux."
}

Ensure-Command "git"   | Out-Null
Ensure-Command "cargo" | Out-Null
Ensure-Command "gh"    | Out-Null
Ensure-Command "bash"  | Out-Null

Write-Host "=== Pex Release Helper (Linux) ===" -ForegroundColor Cyan

Ensure-CleanGit

$tag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }
$platformId = "linux-x86_64"
$zipName = "pex-portable-$platformId-$tag.zip"
$distSubDir = Join-Path "dist" $platformId
$distRoot = Join-Path "make_portable" "dist"

if (Ask-YesNo "Run build & packaging pipeline?" "Y") {
    Write-Host "`n--> Formatting (cargo fmt)" -ForegroundColor Cyan
    cargo fmt || throw "cargo fmt failed"

    Write-Host "`n--> Clippy lint (cargo clippy -- -D warnings)" -ForegroundColor Cyan
    cargo clippy --all-targets --all-features -- -D warnings || throw "cargo clippy failed"

    Write-Host "`n--> Tests (cargo test)" -ForegroundColor Cyan
    cargo test || throw "cargo test failed"

    Write-Host "`n--> Release build (cargo build --release)" -ForegroundColor Cyan
    cargo build --release || throw "cargo build --release failed"

    $portableScript = Join-Path "make_portable" "make_portable.sh"
    if (-not (Test-Path -Path $portableScript)) {
        throw "Packaging script '$portableScript' not found."
    }

    Write-Host "`n--> Packaging portable bundle ($portableScript)" -ForegroundColor Cyan
    & bash $portableScript -z --zip-name $zipName --output-dir $distSubDir
    if ($LASTEXITCODE -ne 0) {
        throw "Portable packaging script failed."
    }
}
else {
    Write-Host "Skipping build & packaging." -ForegroundColor Yellow
}

$zipPath = Normalize-PortablePath (Join-Path (Join-Path $distRoot $platformId) $zipName)
if (-not $zipPath -or -not (Test-Path -Path $zipPath)) {
    throw "Portable zip not found at '$zipPath'."
}

Write-Host "`nBundle ready at $zipPath" -ForegroundColor Green

$tagExists = Test-TagExists -Tag $tag
if ($tagExists) {
    Write-Host "Git tag $tag already exists; reusing." -ForegroundColor Yellow
}
elseif (Ask-YesNo "Create git tag $tag?" "Y") {
    git tag $tag || throw "Failed to create tag $tag"
    Write-Host "Tag $tag created." -ForegroundColor Green
}
else {
    Write-Host "Tag creation skipped." -ForegroundColor Yellow
}

if (Ask-YesNo "Push main and tag to origin?" "Y") {
    Write-Host "`n--> git push origin main" -ForegroundColor Cyan
    git push origin main || throw "git push origin main failed"
    if (Test-TagExists -Tag $tag) {
        Write-Host "`n--> git push origin $tag" -ForegroundColor Cyan
        git push origin $tag || throw "git push origin $tag failed"
    }
    else {
        Write-Host "Tag $tag not found locally; skipped tag push." -ForegroundColor Yellow
    }
}
else {
    Write-Host "Skipping git push." -ForegroundColor Yellow
}

$releaseExists = Test-ReleaseExists -Tag $tag
if ($releaseExists) {
    Write-Host "Release $tag already exists on GitHub." -ForegroundColor Yellow
    if (Ask-YesNo "Upload $zipName to the existing release (clobbers if present)?" "Y") {
        gh release upload $tag $zipPath --clobber || throw "gh release upload failed"
        Write-Host "Asset uploaded." -ForegroundColor Green
    }
    else {
        Write-Host "Skipped release upload." -ForegroundColor Yellow
    }
}
else {
    Write-Host "Release $tag does not exist yet." -ForegroundColor Yellow
    if (Ask-YesNo "Create GitHub release $tag now?" "Y") {
        $notesFile = "CHANGELOG.md"
        $notesArg = if (Test-Path $notesFile) { "--notes-file `"$notesFile`"" } else { "--notes `"Release $tag`"" }
        $cmd = "gh release create $tag `"$zipPath`" $notesArg"
        Write-Host "`n--> $cmd" -ForegroundColor Cyan
        Invoke-Expression $cmd || throw "gh release create failed"
        Write-Host "Release $tag created." -ForegroundColor Green
    }
    else {
        Write-Host "Skipped GitHub release creation." -ForegroundColor Yellow
    }
}

$repoSlug = Get-RepoSlug
if ($repoSlug) {
    try {
        $releaseAssets = Get-ReleaseAssets -Tag $tag
        Update-DownloadsPage -Tag $tag -RepoSlug $repoSlug -Assets $releaseAssets
    }
    catch {
        Write-Host "Failed to refresh docs/index.html: $_" -ForegroundColor Yellow
    }
}
else {
    Write-Host "Could not determine GitHub repository slug; skipping docs update." -ForegroundColor Yellow
}

Write-Host "`nLinux release helper finished." -ForegroundColor Cyan

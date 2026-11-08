#!/usr/bin/env pwsh

Param(
    [ValidateSet('windows-x86_64', 'linux-x86_64')][string]$PlatformId,
    [Parameter(Mandatory = $true)][string]$Version,
    [switch]$UpdateDocs = $false
)

$ErrorActionPreference = "Stop"

$scriptRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "release_helpers.ps1")
if ($PlatformId -eq 'linux-x86_64') { Ensure-Command bash | Out-Null }
Ensure-Command "git" | Out-Null
Ensure-Command "cargo" | Out-Null
Ensure-Command "gh"   | Out-Null

Write-Host "=== Pex Release Helper ($PlatformId) ===" -ForegroundColor Cyan

Push-Location -LiteralPath $scriptRoot
try {
    Ensure-CleanGit

    $tag = Get-ReleaseTag $Version
    $zipName = "pex-portable-$platformId-$tag.zip"
    $distSubDir = Join-Path "dist" $platformId
    $distRoot = Join-Path "make_portable" "dist"
    $artifactDir = Join-Path $distRoot $platformId
    $zipPath = Join-Path $artifactDir $zipName
    $receiptPath = "$zipPath.build.json"

    if (Ask-YesNo "Run build & packaging pipeline?" "Y") {
        Write-Host "`n--> Formatting (cargo fmt)" -ForegroundColor Cyan
        cargo fmt --all -- --check
        if ($LASTEXITCODE -ne 0) { throw "cargo fmt failed" }

        Write-Host "`n--> Clippy lint (cargo clippy -- -D warnings)" -ForegroundColor Cyan
        cargo clippy --locked --all-targets --all-features -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw "cargo clippy failed" }

        Write-Host "`n--> Tests (cargo test)" -ForegroundColor Cyan
        cargo test --locked
        if ($LASTEXITCODE -ne 0) { throw "cargo test failed" }

        Write-Host "`n--> Release build (cargo build --release)" -ForegroundColor Cyan
        $binary = Build-PexRelease
        if ($PlatformId -eq 'windows-x86_64') {
            & ./make_portable/make_portable.ps1 -BinaryPath $binary -Zip -ZipName $zipName -OutputDir $distSubDir
        } else {
            bash ./make_portable/make_portable.sh --binary-path $binary -z --zip-name $zipName --output-dir $distSubDir
            if ($LASTEXITCODE -ne 0) { throw "Portable packaging script failed." }
        }
        $revision = git rev-parse HEAD
        if ($LASTEXITCODE -ne 0) { throw "Unable to record build revision." }
        @{ commit = $revision; sha256 = (Get-FileHash -LiteralPath $zipPath).Hash } |
            ConvertTo-Json | Set-Content -LiteralPath $receiptPath
    }
    else {
        if (-not (Test-Path -LiteralPath $receiptPath -PathType Leaf)) {
            throw "Build the bundle once before reusing it."
        }
        $receipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json
        if ($receipt.commit -notmatch '^[a-f0-9]{40,64}$' -or
            $receipt.sha256 -ne (Get-FileHash -LiteralPath $zipPath).Hash) {
            throw "Cannot reuse this zip. Its build receipt does not match."
        }
        Assert-ReleaseSource $receipt.commit
    }

    $checksumsName = "checksums-$platformId-$tag.txt"
    $checksumsPath = Join-Path $artifactDir $checksumsName
    $checksumsPath = New-ArtifactChecksums -ArtifactPaths @($zipPath) -OutputPath $checksumsPath
    Test-ArtifactChecksums -ChecksumPath $checksumsPath -ArtifactDir $artifactDir

    Write-Host "`nBundle ready at $zipPath" -ForegroundColor Green
    Write-Host "Checksums ready at $checksumsPath (verified)." -ForegroundColor Green

    $tagExists = Test-TagExists -Tag $tag
    if ($tagExists) {
        Write-Host "Git tag $tag already exists; reusing." -ForegroundColor Yellow
    }
    elseif (Ask-YesNo "Create git tag $tag?" "Y") {
        git tag $tag
        if ($LASTEXITCODE -ne 0) { throw "Failed to create tag $tag" }
        Write-Host "Tag $tag created." -ForegroundColor Green
    }
    else {
        Write-Host "Tag creation skipped." -ForegroundColor Yellow
    }

    if (Ask-YesNo "Push main and tag to origin?" "Y") {
        Write-Host "`n--> git push origin main" -ForegroundColor Cyan
        git push origin main
        if ($LASTEXITCODE -ne 0) { throw "git push origin main failed" }
        if (Test-TagExists -Tag $tag) {
            Write-Host "`n--> git push origin $tag" -ForegroundColor Cyan
            git push origin $tag
            if ($LASTEXITCODE -ne 0) { throw "git push origin $tag failed" }
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
        if (Ask-YesNo "Upload $zipName and $checksumsName to the existing release (clobbers if present)?" "Y") {
            Assert-PublishedTag $tag
            gh release upload $tag $zipPath $checksumsPath --clobber
            if ($LASTEXITCODE -ne 0) { throw "gh release upload failed" }
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
            Assert-PublishedTag $tag
            $notesArgs = if (Test-Path -LiteralPath $notesFile) { @('--notes-file', $notesFile) } else { @('--notes', "Release $tag") }
            gh release create $tag $zipPath $checksumsPath --verify-tag @notesArgs
            if ($LASTEXITCODE -ne 0) { throw "gh release create failed" }
            Write-Host "Release $tag created." -ForegroundColor Green
        }
        else {
            Write-Host "Skipped GitHub release creation." -ForegroundColor Yellow
        }
    }

    $repoSlug = Get-RepoSlug
    if ($repoSlug) {
        try {
            $releaseInfo = Get-ReleaseAssets -Tag $tag
            $releaseAssets = $releaseInfo.Assets
            $publishedAt = $releaseInfo.PublishedAt
            $autoDocs = Should-UpdateDocs -Assets $releaseAssets -CurrentPlatformId $platformId

            if ($UpdateDocs -or $autoDocs) {
                if (-not $releaseAssets -or $releaseAssets.Count -eq 0) {
                    Write-Host "Docs update skipped: release has no portable assets yet." -ForegroundColor Yellow
                }
                else {
                    Update-DownloadsPage -Tag $tag -RepoSlug $repoSlug -Assets $releaseAssets -PublishedAt $publishedAt
                }
            }
            else {
                Write-Host "Docs update skipped (run again once both platform bundles are available or pass -UpdateDocs)." -ForegroundColor Yellow
            }
        }
        catch {
            Write-Host "Failed to refresh docs/index.html: $_" -ForegroundColor Yellow
        }
    }
    else {
        Write-Host "Could not determine GitHub repository slug; skipping docs update." -ForegroundColor Yellow
    }

    Write-Host "`n$PlatformId release helper finished." -ForegroundColor Cyan
}
finally { Pop-Location }

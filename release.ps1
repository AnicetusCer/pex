Param(
    [Parameter(Mandatory = $true)]
    [string]$Version
)

function Ask-YesNo([string]$Prompt, [string]$Default = "Y") {
    $suffix = if ($Default -eq "Y") { "[Y/n]" } elseif ($Default -eq "N") { "[y/N]" } else { "[y/n]" }
    while ($true) {
        $reply = Read-Host "$Prompt $suffix"
        if ([string]::IsNullOrWhiteSpace($reply)) {
            if ($Default -eq "Y") { return $true }
            if ($Default -eq "N") { return $false }
        }

        switch ($reply.ToLowerInvariant()) {
            "y" { return $true }
            "yes" { return $true }
            "n" { return $false }
            "no" { return $false }
            default { Write-Host "Please answer 'y' or 'n'." -ForegroundColor Yellow }
        }
    }
}

function Ensure-CleanGit {
    $status = git status --porcelain
    if ($LASTEXITCODE -ne 0) {
        throw "git status failed. Aborting."
    }
    if ($status) {
        throw "Working tree is not clean. Commit, stash, or discard changes before running the release script."
    }
}

Write-Host "=== Pex Release Helper ===" -ForegroundColor Cyan
Write-Host "Target version: $Version"

Ensure-CleanGit

$tag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }

if (Ask-YesNo "Run build & packaging pipeline?" "Y") {
    Write-Host "`n--> Formatting (cargo fmt)" -ForegroundColor Cyan
    cargo fmt || throw "cargo fmt failed"

    Write-Host "`n--> Clippy lint (cargo clippy -- -D warnings)" -ForegroundColor Cyan
    cargo clippy --all-targets --all-features -- -D warnings || throw "cargo clippy failed"

    Write-Host "`n--> Tests (cargo test)" -ForegroundColor Cyan
    cargo test || throw "cargo test failed"

    Write-Host "`n--> Release build (cargo build --release)" -ForegroundColor Cyan
    cargo build --release || throw "cargo build --release failed"

    $portableScript = Join-Path -Path "make_portable" -ChildPath "make_portable.ps1"
    if (Test-Path $portableScript) {
        Write-Host "`n--> Running portable packaging script ($portableScript)" -ForegroundColor Cyan
        pwsh -File $portableScript || throw "Portable packaging script failed"
    }
    else {
        Write-Host "`n(make_portable/make_portable.ps1 not found; skipping portable packaging)" -ForegroundColor Yellow
    }
}
else {
    Write-Host "Skipping build & packaging." -ForegroundColor Yellow
}

if (Ask-YesNo "Create git tag $tag?" "Y") {
    git tag $tag || throw "Failed to create tag $tag"
    Write-Host "Tag $tag created." -ForegroundColor Green
}
else {
    Write-Host "Skipping tag creation." -ForegroundColor Yellow
}

if (Ask-YesNo "Push branch & tags to origin?" "Y") {
    Write-Host "`n--> git push origin main" -ForegroundColor Cyan
    git push origin main || throw "git push origin main failed"
    if (git show-ref --tags $tag > $null) {
        Write-Host "`n--> git push origin $tag" -ForegroundColor Cyan
        git push origin $tag || throw "git push origin $tag failed"
    }
    else {
        Write-Host "Tag $tag not found locally; skipping tag push." -ForegroundColor Yellow
    }
}
else {
    Write-Host "Skipping git push." -ForegroundColor Yellow
}

$ghPath = (Get-Command gh -ErrorAction SilentlyContinue).Path
if ($ghPath) {
    if (Ask-YesNo "Create GitHub release via gh CLI?" "N") {
        $notesFile = "CHANGELOG.md"
        $notesArg = if (Test-Path $notesFile) { "--notes-file `"$notesFile`"" } else { "--notes `"Release $tag`"" }

        $distDir = "make_portable/dist"
        $assets = @()
        if (Test-Path $distDir) {
            $assets = Get-ChildItem $distDir -File
        }

        $assetArgs = $assets | ForEach-Object { "`"$($_.FullName)`"" }
        $cmd = "gh release create $tag $assetArgs $notesArg"
        Write-Host "`n--> $cmd" -ForegroundColor Cyan
        Invoke-Expression $cmd || throw "gh release create failed"
    }
    else {
        Write-Host "Skipping GitHub release creation." -ForegroundColor Yellow
    }
}
else {
    Write-Host "GitHub CLI (gh) not found; skipping release creation." -ForegroundColor Yellow
}

Write-Host "`nRelease helper finished." -ForegroundColor Green

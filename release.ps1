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

function Get-GitHubSlug {
    $origin = git remote get-url origin 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $origin) {
        return $null
    }

    $origin = $origin.Trim()
    $pattern = "github\.com[:/](?<owner>[^/]+)/(?<repo>[^/.]+)(?:\.git)?$"
    $match = [regex]::Match($origin, $pattern, 'IgnoreCase')
    if ($match.Success) {
        $owner = $match.Groups["owner"].Value
        $repo = $match.Groups["repo"].Value
        return "$owner/$repo"
    }

    return $null
}

function Normalize-PortablePath {
    param(
        [Parameter()][object]$PathValue
    )

    if ($null -eq $PathValue) {
        return $null
    }

    if ($PathValue -is [System.Array]) {
        $PathValue = $PathValue | Where-Object { $_ } | Select-Object -Last 1
    }

    if ($PathValue -is [System.IO.FileSystemInfo]) {
        return $PathValue.FullName
    }

    $stringValue = $PathValue -as [string]
    if ([string]::IsNullOrWhiteSpace($stringValue)) {
        return $null
    }

    $stringValue = $stringValue.Trim()
    if ($stringValue -match '^Created\s+(?<full>.+)$') {
        $stringValue = $Matches['full'].Trim()
    }

    return $stringValue
}

function Update-DownloadsPage {
    param(
        [Parameter(Mandatory = $true)][string]$Tag,
        [Parameter(Mandatory = $true)][string]$ZipName,
        [Parameter(Mandatory = $true)][string]$RepoSlug
    )

    $docsDir = "docs"
    if (-not (Test-Path $docsDir)) {
        New-Item -ItemType Directory -Path $docsDir | Out-Null
    }

    $downloadUrl = "https://github.com/$RepoSlug/releases/download/$Tag/$ZipName"
    $releaseUrl = "https://github.com/$RepoSlug/releases/tag/$Tag"
    $updatedAt = Get-Date -Format "yyyy-MM-dd HH:mm:ss 'UTC'"

    $html = @"
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>Pex Downloads</title>
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <style>
    :root { color-scheme: light dark; }
    body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; margin: 2rem auto; max-width: 720px; padding: 0 1rem; line-height: 1.5; }
    h1 { margin-bottom: 0.25rem; font-size: 2rem; }
    p.lead { margin-top: 0; color: #555; }
    .card { margin-top: 1.5rem; border: 1px solid rgba(0,0,0,0.1); border-radius: 12px; padding: 1.75rem; background: rgba(255,255,255,0.85); box-shadow: 0 0.5rem 1.5rem rgba(15,23,42,0.08); }
    a.button { display: inline-block; margin-top: 1rem; background: #1f6feb; color: #fff; padding: 0.75rem 1.5rem; border-radius: 999px; font-weight: 600; text-decoration: none; }
    a.button:hover { background: #1158c7; }
    footer { margin-top: 2rem; font-size: 0.875rem; color: #666; }
    @media (prefers-color-scheme: dark) {
      body { background-color: #0d1117; color: #e6edf3; }
      .card { background: #161b22; border-color: #30363d; box-shadow: none; }
      a.button { background: #2f81f7; color: #fff; }
      a.button:hover { background: #1f6feb; }
      p.lead { color: #9da7b3; }
      footer { color: #8b949e; }
    }
  </style>
</head>
<body>
  <header>
    <h1>Pex Downloads</h1>
    <p class="lead">Grab the latest portable build without digging through release assets.</p>
  </header>
  <main>
    <section class="card">
      <h2>Latest release: $Tag</h2>
      <p>The bundle includes the compiled binary, default configuration, and supporting documentation. Unzip it into a writable folder and launch <code>pex.exe</code>.</p>
      <a class="button" href="$downloadUrl">Download portable bundle</a>
      <p style="margin-top: 1rem;">Need other platforms or older builds? Visit the <a href="$releaseUrl">full release history on GitHub</a>.</p>
    </section>
  </main>
  <footer>Last updated $updatedAt</footer>
</body>
</html>
"@

    Set-Content -Path (Join-Path $docsDir "index.html") -Value $html -Encoding ASCII
    Write-Host "Updated docs/index.html with download link to $ZipName" -ForegroundColor Green
}

function Invoke-PortablePackaging {
    param(
        [Parameter(Mandatory = $true)][string]$ZipName
    )

    $scriptsRoot = "make_portable"
    $distDir = Join-Path $scriptsRoot "dist"

    if ($IsWindows) {
        $portableScript = Join-Path $scriptsRoot "make_portable.ps1"
        if (-not (Test-Path -Path $portableScript)) {
            Write-Host "`n(make_portable/make_portable.ps1 not found; skipping portable packaging)" -ForegroundColor Yellow
            return $null
        }

        Write-Host "`n--> Running portable packaging script ($portableScript)" -ForegroundColor Cyan
        try {
            & $portableScript -Zip:$true -ZipName $ZipName
        }
        catch {
            throw "Portable packaging script failed: $($_.Exception.Message)"
        }
    }
    elseif ($IsLinux) {
        $portableScript = Join-Path $scriptsRoot "make_portable.sh"
        if (-not (Test-Path -Path $portableScript)) {
            Write-Host "`n(make_portable/make_portable.sh not found; skipping portable packaging)" -ForegroundColor Yellow
            return $null
        }

        $bashPath = (Get-Command bash -ErrorAction SilentlyContinue).Path
        if (-not $bashPath) {
            throw "bash not found on PATH; cannot run Linux packaging script."
        }

        Write-Host "`n--> Running portable packaging script ($portableScript via $bashPath)" -ForegroundColor Cyan
        & $bashPath $portableScript -z --zip-name $ZipName
        if ($LASTEXITCODE -ne 0) {
            throw "Portable packaging script failed"
        }
    }
    else {
        Write-Host "`n(Current platform not supported for portable packaging automation.)" -ForegroundColor Yellow
        return $null
    }

    $zipPath = Join-Path $distDir $ZipName
    if (-not (Test-Path -Path $zipPath)) {
        throw "Expected portable archive missing at $zipPath"
    }

    return $zipPath
}

Write-Host "=== Pex Release Helper ===" -ForegroundColor Cyan
Write-Host "Target version: $Version"

Ensure-CleanGit

$tag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }
$portableZipPath = $null
$portableZipName = "pex-portable-$tag.zip"

if (Ask-YesNo "Run build & packaging pipeline?" "Y") {
    Write-Host "`n--> Formatting (cargo fmt)" -ForegroundColor Cyan
    cargo fmt || throw "cargo fmt failed"

    Write-Host "`n--> Clippy lint (cargo clippy -- -D warnings)" -ForegroundColor Cyan
    cargo clippy --all-targets --all-features -- -D warnings || throw "cargo clippy failed"

    Write-Host "`n--> Tests (cargo test)" -ForegroundColor Cyan
    cargo test || throw "cargo test failed"

    Write-Host "`n--> Release build (cargo build --release)" -ForegroundColor Cyan
    cargo build --release || throw "cargo build --release failed"

    try {
        $portableZipPath = Normalize-PortablePath (Invoke-PortablePackaging -ZipName $portableZipName)
    }
    catch {
        throw $_
    }
}
else {
    Write-Host "Skipping build & packaging." -ForegroundColor Yellow
}

if (-not $portableZipPath) {
    $candidateZip = Join-Path (Join-Path "make_portable" "dist") $portableZipName
    if (Test-Path $candidateZip) {
        $portableZipPath = Normalize-PortablePath $candidateZip
    }
}

$portableZipPath = Normalize-PortablePath $portableZipPath

$repoSlug = Get-GitHubSlug
if ($portableZipPath -and $repoSlug) {
    try {
        Update-DownloadsPage -Tag $tag -ZipName $portableZipName -RepoSlug $repoSlug
    }
    catch {
        Write-Host "Failed to update GitHub Pages download: $_" -ForegroundColor Yellow
    }
}
elseif ($portableZipPath -and -not $repoSlug) {
    Write-Host "Could not determine GitHub repository slug; skipping docs update." -ForegroundColor Yellow
}

if (Ask-YesNo "Create git tag $tag?" "Y") {
    git rev-parse -q --verify "refs/tags/$tag" *>$null
    $tagExists = $LASTEXITCODE -eq 0

    if ($tagExists) {
        Write-Host "Tag $tag already exists." -ForegroundColor Yellow
        if (Ask-YesNo "Reuse existing tag $tag?" "Y") {
            Write-Host "Reusing existing tag $tag." -ForegroundColor Green
        }
        elseif (Ask-YesNo "Delete and recreate tag $tag?" "N") {
            git tag -d $tag
            if ($LASTEXITCODE -ne 0) {
                throw "Failed to delete existing tag $tag"
            }
            git tag $tag
            if ($LASTEXITCODE -ne 0) {
                throw "Failed to recreate tag $tag"
            }
            Write-Host "Tag $tag recreated." -ForegroundColor Green
        }
        else {
            throw "Tag $tag already exists and user chose not to reuse it."
        }
    }
    else {
        git tag $tag
        if ($LASTEXITCODE -ne 0) {
            throw "Failed to create tag $tag"
        }
        Write-Host "Tag $tag created." -ForegroundColor Green
    }
}
else {
    Write-Host "Skipping tag creation." -ForegroundColor Yellow
}

if (Ask-YesNo "Push branch & tags to origin?" "Y") {
    Write-Host "`n--> git push origin main" -ForegroundColor Cyan
    git push origin main || throw "git push origin main failed"
    git rev-parse -q --verify "refs/tags/$tag" *>$null
    if ($LASTEXITCODE -eq 0) {
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
        if (-not $portableZipPath -and (Test-Path -Path (Join-Path $distDir $portableZipName))) {
            $portableZipPath = Join-Path $distDir $portableZipName
        }
        $portableZipPath = Normalize-PortablePath $portableZipPath
        if ($portableZipPath -and (Test-Path -Path $portableZipPath)) {
            $assets += Get-Item $portableZipPath
        }
        elseif (Test-Path -Path $distDir) {
            Write-Host "Warning: Portable zip not found; falling back to raw dist files." -ForegroundColor Yellow
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

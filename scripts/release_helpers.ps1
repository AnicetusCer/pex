function Ask-YesNo {
    param(
        [Parameter(Mandatory = $true)][string]$Prompt,
        [Parameter()][string]$Default = "Y"
    )

    $suffix = switch ($Default.ToUpperInvariant()) {
        "Y" { "[Y/n]" }
        "N" { "[y/N]" }
        default { "[y/n]" }
    }

    while ($true) {
        $reply = Read-Host "$Prompt $suffix"
        if ([string]::IsNullOrWhiteSpace($reply)) {
            switch ($Default.ToUpperInvariant()) {
                "Y" { return $true }
                "N" { return $false }
            }
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

function Ensure-Command {
    param([Parameter(Mandatory = $true)][string]$Name)

    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    if (-not $cmd) {
        throw "'$Name' not found on PATH. Install it or add it to PATH before running the release helper."
    }

    return $cmd
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

function Get-RepoSlug {
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

function Test-TagExists {
    param([Parameter(Mandatory = $true)][string]$Tag)

    git rev-parse -q --verify "refs/tags/$Tag" *>$null
    return $LASTEXITCODE -eq 0
}

function Normalize-PortablePath {
    param([Parameter()][object]$PathValue)

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

function Get-FriendlyPlatformName {
    param([Parameter()][string]$PlatformId)

    switch ($PlatformId) {
        "windows-x86_64" { return "Windows (x86_64)" }
        "linux-x86_64" { return "Linux (x86_64)" }
        default { return $PlatformId }
    }
}

function Get-AssetMetadata {
    param([Parameter(Mandatory = $true)][string]$Name)

    $pattern = '^pex-portable-(?<platform>[^-]+(?:-[^-]+)*)-(?<tag>v.+)\.zip$'
    $match = [regex]::Match($Name, $pattern, 'IgnoreCase')
    $platformId = if ($match.Success) { $match.Groups['platform'].Value } else { $null }

    [pscustomobject]@{
        PlatformId = $platformId
        Display    = if ($platformId) { Get-FriendlyPlatformName $platformId } else { $Name }
    }
}

function Get-ReleaseAssets {
    param([Parameter(Mandatory = $true)][string]$Tag)

    $ghCmd = Get-Command gh -ErrorAction SilentlyContinue
    if (-not $ghCmd) {
        Write-Host "GitHub CLI (gh) not found; skipping release asset lookup." -ForegroundColor Yellow
        return @()
    }

    $json = gh release view $Tag --json assets 2>$null
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($json)) {
        return @()
    }

    $parsed = $json | ConvertFrom-Json
    if (-not $parsed -or -not $parsed.assets) {
        return @()
    }

    $assets = @()
    foreach ($asset in $parsed.assets) {
        $meta = Get-AssetMetadata -Name $asset.name
        $downloadUrl = if ($asset.url) { $asset.url }
            elseif ($asset.browserDownloadUrl) { $asset.browserDownloadUrl }
            elseif ($asset.downloadUrl) { $asset.downloadUrl }
            else { $null }

        $assets += [pscustomobject]@{
            Name        = $asset.name
            DownloadUrl = $downloadUrl
            PlatformId  = $meta.PlatformId
            Display     = $meta.Display
        }
    }

    return $assets | Sort-Object -Property Name
}

function Test-ReleaseExists {
    param([Parameter(Mandatory = $true)][string]$Tag)

    $ghCmd = Get-Command gh -ErrorAction SilentlyContinue
    if (-not $ghCmd) {
        throw "GitHub CLI (gh) not found; cannot check releases."
    }

    gh release view $Tag --json name *>$null
    return $LASTEXITCODE -eq 0
}

function Build-DownloadSection {
    param(
        [Parameter()][string]$Tag,
        [Parameter()][string]$ReleaseUrl,
        [Parameter()][array]$Assets
    )

    if (-not $Assets -or $Assets.Count -eq 0) {
        return @"
      <p>Portable bundles for this release aren't ready yet. Watch the <a href="$ReleaseUrl">release page on GitHub</a> for updates.</p>
"@
    }

    $buttons = ($Assets | ForEach-Object {
        $label = $_.Display
        $href = $_.DownloadUrl
        "      <a class=""button"" href=""$href"">Download for $label</a>"
    }) -join "`n"

    return @"
      <p>Pick your platform:</p>
      <div class="button-grid">
$buttons
      </div>
"@
}

function Update-DownloadsPage {
    param(
        [Parameter(Mandatory = $true)][string]$Tag,
        [Parameter(Mandatory = $true)][string]$RepoSlug,
        [Parameter()][array]$Assets
    )

    $docsDir = "docs"
    if (-not (Test-Path $docsDir)) {
        New-Item -ItemType Directory -Path $docsDir | Out-Null
    }

    $releaseUrl = "https://github.com/$RepoSlug/releases/tag/$Tag"
    $updatedAt = Get-Date -Format "yyyy-MM-dd HH:mm:ss 'UTC'"
    $downloadSection = Build-DownloadSection -Tag $Tag -ReleaseUrl $releaseUrl -Assets $Assets

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
    .button-grid { display: flex; flex-wrap: wrap; gap: 0.75rem; margin-top: 1rem; }
    .button-grid .button { flex: 1 1 220px; text-align: center; }
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
      <p class="lead">Portable bundles include the compiled binary, default configuration, and supporting documentation.</p>
$downloadSection
      <p style="margin-top: 1.5rem;">Need other platforms or older builds? Visit the <a href="$releaseUrl">full release history on GitHub</a>.</p>
    </section>
  </main>
  <footer>Last updated $updatedAt</footer>
</body>
</html>
"@

    Set-Content -Path (Join-Path $docsDir "index.html") -Value $html -Encoding ASCII

    if ($Assets -and $Assets.Count -gt 0) {
        $names = ($Assets | ForEach-Object { $_.Name }) -join ", "
        Write-Host "Updated docs/index.html with download buttons for: $names" -ForegroundColor Green
    }
    else {
        Write-Host "Updated docs/index.html (release has no portable assets yet)." -ForegroundColor Yellow
    }
}

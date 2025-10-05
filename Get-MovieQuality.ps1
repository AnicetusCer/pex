<# 
.SYNOPSIS
    Quickly determines whether a video file is SD, HD, Full HD, or UHD/4K.

.DESCRIPTION
    This standalone script calls **ffprobe** (part of the FFmpeg suite) to read the
    video stream’s width and height, then classifies the resolution.

    • 1080 p or higher → Full HD (≥ 1080 p)  
    • 720 p – 1079 p → HD (≥ 720 p)  
    • 2160 p or higher → UHD/4K (≥ 2160 p)  
    • below 720 p   → SD (< 720 p)

USAGE
    # 1️⃣  Install ffprobe (if you haven’t already)
    winget install Gyan.FFmpeg   # or: winget install BtbN.FFmpeg

    # 2️⃣  Run the script, passing a UNC, mapped‑drive, or local path:
    .\Get-MovieQuality.ps1 -Path '\\server\Share\Movies\MyFilm.mkv'

OUTPUT
    The script writes a single PowerShell object with four properties:
        File    – the full path you supplied
        Width   – video width in pixels
        Height  – video height in pixels
        Quality – one of the four labels described above

    Example:
    File                                                            Width Height Quality
    ----                                                            ----- ------ -------------------------------
    \\server\Share\Movies\MyFilm.mkv                                 1920   1080   Full HD (≥ 1080 p)

NOTE
    • The script filters out ffprobe warning lines (e.g., reference‑frame warnings) so they don’t interfere with parsing.
    • If ffprobe cannot return numeric width/height, the script falls back to JSON output.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory=$true, Position=0, ValueFromPipeline=$true)]
    [ValidateNotNullOrEmpty()]
    [string]$Path
)

# -------------------------------------------------
# 1️⃣ Verify ffprobe is on the PATH
# -------------------------------------------------
function Test-FFProbe {
    try { ffprobe -version >$null 2>&1; $true } catch { $false }
}
if (-not (Test-FFProbe)) {
    Write-Error "ffprobe not found – install via `winget install Gyan.FFmpeg` (or BtbN.FFmpeg)."
    exit 1
}

# -------------------------------------------------
# 2️⃣ Use the raw UNC/path string (no Resolve‑Path)
# -------------------------------------------------
$fullPath = $Path   # already a plain string

# -------------------------------------------------
# 3️⃣ Run ffprobe – capture both stdout & stderr
# -------------------------------------------------
$ffprobeArgs = @(
    '-v','error',
    '-hide_banner',
    '-loglevel','error',
    '-select_streams','v:0',
    '-show_entries','stream=width,height',
    '-of','default=noprint_wrappers=1:nokey=1',
    $fullPath
)
$rawInfo = ffprobe @ffprobeArgs 2>&1

# -------------------------------------------------
# 4️⃣ Keep only pure‑numeric lines (filter out warnings)
# -------------------------------------------------
$numeric = $rawInfo -split "`n" |
    ForEach-Object { $_.Trim() } |
    Where-Object { $_ -match '^\d+$' }

if ($numeric.Count -ge 2) {
    $width  = $numeric[0]
    $height = $numeric[1]
}
else {
    # ---------- JSON fallback (guaranteed numeric) ----------
    $jsonArgs = @(
        '-v','error',
        '-select_streams','v:0',
        '-show_entries','stream=width,height',
        '-print_format','json',
        $fullPath
    )
    $jsonRaw = ffprobe @jsonArgs 2>&1
    $obj = $jsonRaw | ConvertFrom-Json
    $width  = $obj.streams[0].width
    $height = $obj.streams[0].height
}

# -------------------------------------------------
# 5️⃣ Force integer conversion
# -------------------------------------------------
[int]$widthInt  = $width
[int]$heightInt = $height

# -------------------------------------------------
# 6️⃣ DEBUG – show the numbers we are about to classify
# -------------------------------------------------
Write-Host "DEBUG – width=$widthInt  height=$heightInt" -ForegroundColor Cyan

# -------------------------------------------------
# 7️⃣ Classification – Full HD before HD
# -------------------------------------------------
if ($heightInt -ge 2160) {
    $quality = "UHD/4K (≥ 2160 p)"
}
elseif ($heightInt -ge 1080) {
    $quality = "Full HD (≥ 1080 p)"
}
elseif ($heightInt -ge 720) {
    $quality = "HD (≥ 720 p)"
}
else {
    $quality = "SD (< 720 p)"
}

# -------------------------------------------------
# 8️⃣ Return a structured object (easy to pipe / export)
# -------------------------------------------------
[pscustomobject]@{
    File    = $fullPath
    Width   = $widthInt
    Height  = $heightInt
    Quality = $quality
}
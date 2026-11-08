#!/usr/bin/env pwsh

Param(
    [Parameter(Mandatory = $true)][string]$Version,
    [switch]$UpdateDocs = $false
)

$ErrorActionPreference = "Stop"

if (-not $IsWindows) { throw "release_windows.ps1 must run on Windows." }
& (Join-Path $PSScriptRoot 'scripts/release_platform.ps1') -PlatformId 'windows-x86_64' -Version $Version -UpdateDocs:$UpdateDocs

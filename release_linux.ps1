#!/usr/bin/env pwsh

Param(
    [Parameter(Mandatory = $true)][string]$Version,
    [switch]$UpdateDocs = $false
)

$ErrorActionPreference = "Stop"

if (-not $IsLinux) { throw "release_linux.ps1 must run on Linux." }
& (Join-Path $PSScriptRoot 'scripts/release_platform.ps1') -PlatformId 'linux-x86_64' -Version $Version -UpdateDocs:$UpdateDocs

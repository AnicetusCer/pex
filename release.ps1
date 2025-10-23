Write-Host "The release workflow is now split per platform." -ForegroundColor Yellow
Write-Host "On Windows run 'pwsh ./release_windows.ps1 -Version <x.y.z>'." -ForegroundColor Yellow
Write-Host "On Linux run 'pwsh ./release_linux.ps1 -Version <x.y.z>'." -ForegroundColor Yellow
exit 1

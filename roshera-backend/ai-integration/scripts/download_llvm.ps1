# Simple LLVM Download Script
$llvmUrl = "https://github.com/llvm/llvm-project/releases/download/llvmorg-17.0.6/LLVM-17.0.6-win64.exe"
$downloadPath = "$env:TEMP\LLVM-17.0.6-win64.exe"

Write-Host "Downloading LLVM 17.0.6..." -ForegroundColor Green
Write-Host "This may take a few minutes..." -ForegroundColor Yellow

try {
    Invoke-WebRequest -Uri $llvmUrl -OutFile $downloadPath -UseBasicParsing
    Write-Host "Download completed!" -ForegroundColor Green
    Write-Host "File saved to: $downloadPath" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "Starting installer..." -ForegroundColor Green
    Start-Process -FilePath $downloadPath
    Write-Host ""
    Write-Host "IMPORTANT: During installation, make sure to:" -ForegroundColor Yellow
    Write-Host "1. Check 'Add LLVM to the system PATH'" -ForegroundColor White
    Write-Host "2. Note the installation directory (usually C:\Program Files\LLVM)" -ForegroundColor White
} catch {
    Write-Host "Error downloading: $_" -ForegroundColor Red
    Write-Host "Download manually from: $llvmUrl" -ForegroundColor Yellow
}
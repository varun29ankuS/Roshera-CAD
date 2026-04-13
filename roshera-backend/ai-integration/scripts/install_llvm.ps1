# LLVM Installation Script for Windows
# This script downloads LLVM and provides installation instructions

$llvmVersion = "17.0.6"
$llvmUrl = "https://github.com/llvm/llvm-project/releases/download/llvmorg-$llvmVersion/LLVM-$llvmVersion-win64.exe"
$downloadPath = "$env:TEMP\LLVM-$llvmVersion-win64.exe"

Write-Host "LLVM Installation Helper for Whisper-rs" -ForegroundColor Green
Write-Host "=======================================" -ForegroundColor Green
Write-Host ""

# Check if running as administrator
$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole] "Administrator")
if (-not $isAdmin) {
    Write-Host "NOTE: Run PowerShell as Administrator for automatic PATH updates" -ForegroundColor Yellow
    Write-Host ""
}

# Download LLVM
Write-Host "Downloading LLVM $llvmVersion..." -ForegroundColor Cyan
Write-Host "URL: $llvmUrl"
Write-Host "Destination: $downloadPath"
Write-Host ""

try {
    # Use Invoke-WebRequest with progress
    $ProgressPreference = 'Continue'
    Invoke-WebRequest -Uri $llvmUrl -OutFile $downloadPath -UseBasicParsing
    Write-Host "Download completed successfully!" -ForegroundColor Green
} catch {
    Write-Host "Error downloading LLVM: $_" -ForegroundColor Red
    Write-Host "Please download manually from: $llvmUrl" -ForegroundColor Yellow
    exit 1
}

# Launch installer
Write-Host ""
Write-Host "Launching LLVM installer..." -ForegroundColor Cyan
Write-Host ""
Write-Host "IMPORTANT INSTALLATION STEPS:" -ForegroundColor Yellow
Write-Host "1. Click 'Next' through the installer" -ForegroundColor White
Write-Host "2. Accept the license agreement" -ForegroundColor White
Write-Host "3. Choose installation directory (default: C:\Program Files\LLVM)" -ForegroundColor White
Write-Host "4. IMPORTANT: Check 'Add LLVM to the system PATH'" -ForegroundColor Green
Write-Host "5. Click 'Install' and wait for completion" -ForegroundColor White
Write-Host ""
Write-Host "Press Enter to start the installer..." -ForegroundColor Cyan
Read-Host

Start-Process -FilePath $downloadPath -Wait

# Verify installation
Write-Host ""
Write-Host "Verifying LLVM installation..." -ForegroundColor Cyan

$clangPath = Get-Command clang -ErrorAction SilentlyContinue
if ($clangPath) {
    Write-Host "✓ LLVM installed successfully!" -ForegroundColor Green
    Write-Host "  Clang found at: $($clangPath.Path)" -ForegroundColor Gray
} else {
    Write-Host "✗ Clang not found in PATH" -ForegroundColor Red
    Write-Host "  You may need to restart your terminal or add LLVM to PATH manually" -ForegroundColor Yellow
}

# Set LIBCLANG_PATH environment variable
Write-Host ""
Write-Host "Setting LIBCLANG_PATH environment variable..." -ForegroundColor Cyan

$llvmBinPath = "C:\Program Files\LLVM\bin"
if (Test-Path $llvmBinPath) {
    if ($isAdmin) {
        [System.Environment]::SetEnvironmentVariable("LIBCLANG_PATH", $llvmBinPath, [System.EnvironmentVariableTarget]::Machine)
        Write-Host "✓ LIBCLANG_PATH set to: $llvmBinPath" -ForegroundColor Green
    } else {
        Write-Host "Run this command in an Administrator PowerShell to set LIBCLANG_PATH:" -ForegroundColor Yellow
        Write-Host "[System.Environment]::SetEnvironmentVariable('LIBCLANG_PATH', '$llvmBinPath', [System.EnvironmentVariableTarget]::Machine)" -ForegroundColor White
    }
} else {
    Write-Host "✗ LLVM bin directory not found at expected location" -ForegroundColor Red
    Write-Host "  Please set LIBCLANG_PATH manually to your LLVM\bin directory" -ForegroundColor Yellow
}

# Clean up
Write-Host ""
Write-Host "Cleaning up downloaded installer..." -ForegroundColor Cyan
Remove-Item $downloadPath -Force -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "Installation complete!" -ForegroundColor Green
Write-Host "Please restart your terminal for PATH changes to take effect." -ForegroundColor Yellow
Write-Host ""
Write-Host "Next steps:" -ForegroundColor Cyan
Write-Host "1. Restart your terminal/IDE" -ForegroundColor White
Write-Host "2. Run: clang --version (to verify installation)" -ForegroundColor White
Write-Host "3. Continue with Whisper integration" -ForegroundColor White
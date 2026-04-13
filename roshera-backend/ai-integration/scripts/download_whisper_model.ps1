# Download Whisper Base model in ggml format
$modelUrl = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin"
$modelPath = "..\..\models\whisper\ggml-base.bin"

Write-Host "Downloading Whisper Base model..." -ForegroundColor Green
Write-Host "URL: $modelUrl" -ForegroundColor Cyan
Write-Host "Destination: $modelPath" -ForegroundColor Cyan
Write-Host ""

# Create directory if it doesn't exist
$dir = Split-Path -Parent $modelPath
if (!(Test-Path $dir)) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
}

try {
    # Download with progress
    $ProgressPreference = 'Continue'
    Invoke-WebRequest -Uri $modelUrl -OutFile $modelPath -UseBasicParsing
    
    # Verify file size
    $fileInfo = Get-Item $modelPath
    $sizeMB = [math]::Round($fileInfo.Length / 1MB, 1)
    
    Write-Host ""
    Write-Host "Download completed!" -ForegroundColor Green
    Write-Host "File size: $sizeMB MB" -ForegroundColor Cyan
    Write-Host "Model saved to: $modelPath" -ForegroundColor Cyan
    
    if ($sizeMB -lt 140 -or $sizeMB -gt 150) {
        Write-Host "WARNING: File size seems incorrect (expected ~142 MB)" -ForegroundColor Yellow
    }
} catch {
    Write-Host "Error downloading model: $_" -ForegroundColor Red
    Write-Host ""
    Write-Host "You can download manually from:" -ForegroundColor Yellow
    Write-Host $modelUrl -ForegroundColor White
    Write-Host "Save to: $modelPath" -ForegroundColor White
}
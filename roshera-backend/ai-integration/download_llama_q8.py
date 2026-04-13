#!/usr/bin/env python3
"""
Download LLaMA 3.2 3B Q8 model for Roshera CAD
"""

import os
import sys
import requests
from pathlib import Path
from tqdm import tqdm
import hashlib
import time

# Fix Unicode on Windows
if sys.platform == 'win32':
    import locale
    if sys.stdout.encoding != 'utf-8':
        sys.stdout.reconfigure(encoding='utf-8')

def download_with_resume(url, dest_path, expected_size_mb=None):
    """Download file with resume capability"""
    headers = {}
    mode = 'wb'
    resume_pos = 0
    
    # Check if partial download exists
    if dest_path.exists():
        resume_pos = dest_path.stat().st_size
        headers['Range'] = f'bytes={resume_pos}-'
        mode = 'ab'
        print(f"Resuming download from {resume_pos / 1024 / 1024:.1f} MB")
    
    response = requests.get(url, headers=headers, stream=True, allow_redirects=True)
    
    # Check if server supports resume
    if resume_pos > 0 and response.status_code != 206:
        print("Server doesn't support resume, starting fresh")
        resume_pos = 0
        mode = 'wb'
        response = requests.get(url, stream=True, allow_redirects=True)
    
    # Get total size
    if 'content-length' in response.headers:
        total_size = int(response.headers['content-length']) + resume_pos
    elif expected_size_mb:
        total_size = expected_size_mb * 1024 * 1024
    else:
        total_size = None
    
    # Download with progress bar
    with open(dest_path, mode) as f:
        with tqdm(
            total=total_size,
            initial=resume_pos,
            unit='B',
            unit_scale=True,
            desc=dest_path.name
        ) as pbar:
            for chunk in response.iter_content(chunk_size=8192):
                if chunk:
                    f.write(chunk)
                    pbar.update(len(chunk))
    
    return dest_path

def verify_file_size(file_path, expected_mb, tolerance=0.1):
    """Verify downloaded file size"""
    if not file_path.exists():
        return False
    
    actual_mb = file_path.stat().st_size / 1024 / 1024
    expected_min = expected_mb * (1 - tolerance)
    expected_max = expected_mb * (1 + tolerance)
    
    if actual_mb < expected_min or actual_mb > expected_max:
        print(f"Warning: File size {actual_mb:.1f} MB outside expected range {expected_min:.1f}-{expected_max:.1f} MB")
        return False
    
    return True

def main():
    """Download LLaMA Q8 model"""
    print("LLaMA 3.2 3B Q8 Model Downloader")
    print("=================================\n")
    
    # Model details
    model_url = "https://huggingface.co/bartowski/Llama-3.2-3B-Instruct-GGUF/resolve/main/Llama-3.2-3B-Instruct-Q8_0.gguf"
    model_size_mb = 3520  # ~3.5 GB
    models_dir = Path("../models/llama")
    model_path = models_dir / "3.2-3b-instruct-q8.bin"
    
    print(f"Model: LLaMA 3.2 3B Instruct Q8")
    print(f"Size: ~{model_size_mb / 1024:.1f} GB")
    print(f"Destination: {model_path.absolute()}")
    print(f"Quantization: Q8_0 (8-bit, ~99% quality)")
    print("\nThis provides excellent accuracy for CAD commands.")
    print("You can swap to Q4 later if you need more speed.\n")
    
    # Check disk space
    import shutil
    stat = shutil.disk_usage(models_dir.absolute().anchor)
    free_gb = stat.free / (1024**3)
    
    if free_gb < (model_size_mb / 1024) * 1.5:
        print(f"ERROR: Insufficient disk space. Need ~{model_size_mb/1024*1.5:.1f}GB, have {free_gb:.1f}GB")
        return 1
    
    # Create directory
    models_dir.mkdir(parents=True, exist_ok=True)
    
    # Check if already downloaded
    if model_path.exists() and verify_file_size(model_path, model_size_mb):
        print("[OK] Model already downloaded and verified!")
        return 0
    
    # Download
    print("\nStarting download...")
    print("(This may take 10-30 minutes depending on your connection)\n")
    
    try:
        start_time = time.time()
        download_with_resume(model_url, model_path, model_size_mb)
        
        # Verify
        if verify_file_size(model_path, model_size_mb):
            elapsed = time.time() - start_time
            speed_mb = (model_path.stat().st_size / 1024 / 1024) / elapsed
            print(f"\n[SUCCESS] Download complete!")
            print(f"Time: {elapsed/60:.1f} minutes")
            print(f"Speed: {speed_mb:.1f} MB/s")
            print(f"\nModel saved to: {model_path.absolute()}")
            
            # Update config
            config_path = Path("../models/config.json")
            if config_path.exists():
                import json
                with open(config_path, 'r') as f:
                    config = json.load(f)
                config['llama']['model'] = '3.2-3b-instruct-q8'
                config['llama']['path'] = str(model_path)
                config['llama']['quantization'] = 'Q8_0'
                with open(config_path, 'w') as f:
                    json.dump(config, f, indent=2)
                print("\n[INFO] Updated config.json with Q8 model path")
            
            return 0
        else:
            print("\n[ERROR] Download completed but file size verification failed")
            return 1
            
    except KeyboardInterrupt:
        print("\n\n[INFO] Download interrupted. Run again to resume.")
        return 1
    except Exception as e:
        print(f"\n[ERROR] Download failed: {e}")
        return 1

if __name__ == "__main__":
    sys.exit(main())
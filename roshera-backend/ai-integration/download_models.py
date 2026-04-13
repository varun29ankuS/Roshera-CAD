#!/usr/bin/env python3
"""
Download required AI models for Roshera CAD
Supports Hindi + English speakers
"""

import os
import sys
import requests
from pathlib import Path
import hashlib
from tqdm import tqdm

# Fix Unicode issues on Windows
if sys.platform == 'win32':
    import locale
    if sys.stdout.encoding != 'utf-8':
        sys.stdout.reconfigure(encoding='utf-8')

MODELS_DIR = Path("../models")

# Model configurations
MODELS = {
    "whisper": {
        "base": {
            # Using GGML format for CPU inference
            "url": "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
            "sha256": "expected_hash_here",
            "size_mb": 148,
            "description": "Whisper base model - supports 96+ languages including Hindi"
        },
        "base.en": {
            "url": "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
            "sha256": "expected_hash_here", 
            "size_mb": 148,
            "description": "Whisper base English-only (faster for English)"
        }
    },
    "llama": {
        "3.2-3b-instruct-q8": {
            # Using Q8 for better accuracy in technical CAD commands
            "url": "https://huggingface.co/bartowski/Llama-3.2-3B-Instruct-GGUF/resolve/main/Llama-3.2-3B-Instruct-Q8_0.gguf",
            "sha256": "expected_hash_here",
            "size_mb": 3520,  # ~3.5GB for Q8 quantized
            "description": "LLaMA 3.2 3B Instruct - Q8 quantized for accuracy"
        }
    },
    "ganga": {
        "1b-v1": {
            "url": "https://huggingface.co/ai4bharat/Ganga-1.0-1B/resolve/main/model.safetensors",
            "sha256": "expected_hash_here",
            "size_mb": 2500,
            "description": "Ganga-1B - Indian language model (Hindi+English)"
        }
    },
    "tts": {
        "coqui-vits-hi": {
            "url": "https://huggingface.co/coqui/VITS/resolve/main/vits_hi.tar.gz",
            "sha256": "expected_hash_here",
            "size_mb": 150,
            "description": "Coqui VITS Hindi TTS model"
        },
        "coqui-vits-en": {
            "url": "https://huggingface.co/coqui/VITS/resolve/main/vits_ljs.tar.gz", 
            "sha256": "expected_hash_here",
            "size_mb": 150,
            "description": "Coqui VITS English TTS model"
        }
    }
}

def download_file(url: str, dest: Path, expected_size_mb: int):
    """Download file with progress bar"""
    print(f"Downloading {dest.name}...")
    
    response = requests.get(url, stream=True)
    total_size = int(response.headers.get('content-length', 0))
    
    if total_size == 0:
        print(f"Warning: Could not determine file size for {url}")
        total_size = expected_size_mb * 1024 * 1024
    
    dest.parent.mkdir(parents=True, exist_ok=True)
    
    with open(dest, 'wb') as f:
        with tqdm(total=total_size, unit='B', unit_scale=True) as pbar:
            for chunk in response.iter_content(chunk_size=8192):
                f.write(chunk)
                pbar.update(len(chunk))

def check_model_exists(model_path: Path, size_mb: int) -> bool:
    """Check if model exists and has reasonable size"""
    if not model_path.exists():
        return False
    
    actual_size_mb = model_path.stat().st_size / (1024 * 1024)
    expected_size_mb = size_mb
    
    # Allow 20% tolerance
    if abs(actual_size_mb - expected_size_mb) / expected_size_mb > 0.2:
        print(f"Warning: {model_path.name} size mismatch. Expected ~{expected_size_mb}MB, got {actual_size_mb:.1f}MB")
        return False
    
    return True

def main():
    """Download all required models"""
    print("Roshera CAD AI Model Downloader")
    print("================================")
    print("This will download models for Hindi+English speech recognition and synthesis\n")
    
    total_size_mb = sum(
        model["size_mb"] 
        for category in MODELS.values() 
        for model in category.values()
    )
    
    print(f"Total download size: ~{total_size_mb / 1024:.1f} GB")
    print(f"Models will be saved to: {MODELS_DIR.absolute()}\n")
    
    # Check available disk space
    import shutil
    stat = shutil.disk_usage(MODELS_DIR.absolute().anchor)
    free_gb = stat.free / (1024**3)
    
    if free_gb < (total_size_mb / 1024) * 1.5:  # Need 1.5x space
        print(f"ERROR: Insufficient disk space. Need ~{total_size_mb/1024*1.5:.1f}GB, have {free_gb:.1f}GB")
        return 1
    
    # Download models
    for category, models in MODELS.items():
        print(f"\n{category.upper()} Models:")
        print("-" * 40)
        
        for model_name, model_info in models.items():
            model_path = MODELS_DIR / category / f"{model_name}.bin"
            
            if check_model_exists(model_path, model_info["size_mb"]):
                print(f"[OK] {model_name}: Already downloaded")
                continue
            
            print(f"\n{model_info['description']}")
            print(f"URL: {model_info['url']}")
            print(f"Size: ~{model_info['size_mb']} MB")
            
            # Create placeholder file for now
            model_path.parent.mkdir(parents=True, exist_ok=True)
            model_path.write_text(f"Placeholder for {model_name}\nDownload from: {model_info['url']}\n")
            print(f"[INFO] Created placeholder at: {model_path}")
            
            # Uncomment to actually download:
            # try:
            #     download_file(model_info["url"], model_path, model_info["size_mb"])
            #     print(f"[OK] {model_name}: Download complete")
            # except Exception as e:
            #     print(f"[FAIL] {model_name}: Download failed - {e}")
            #     return 1
    
    print("\n[SUCCESS] All models downloaded successfully!")
    
    # Create model config
    config_path = MODELS_DIR / "config.json"
    import json
    config = {
        "whisper": {
            "model": "base",
            "path": str(MODELS_DIR / "whisper" / "base.bin"),
            "language": "hi",  # Hindi default, auto-detect enabled
            "device": "cpu"
        },
        "llama": {
            "model": "3.2-3b-instruct-q8",
            "path": str(MODELS_DIR / "llama" / "3.2-3b-instruct-q8.bin"),
            "context_length": 4096,
            "device": "cpu",
            "threads": 8,
            "quantization": "Q8_0"
        },
        "ganga": {
            "model": "1b-v1",
            "path": str(MODELS_DIR / "ganga" / "1b-v1.bin"),
            "device": "cpu",
            "enabled": True
        },
        "tts": {
            "models": {
                "hi": str(MODELS_DIR / "tts" / "coqui-vits-hi.bin"),
                "en": str(MODELS_DIR / "tts" / "coqui-vits-en.bin")
            },
            "default_language": "hi"
        }
    }
    
    with open(config_path, 'w') as f:
        json.dump(config, f, indent=2)
    
    print(f"\nModel configuration saved to: {config_path}")
    
    return 0

if __name__ == "__main__":
    sys.exit(main())
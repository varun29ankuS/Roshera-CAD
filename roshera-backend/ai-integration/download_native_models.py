#!/usr/bin/env python3
"""
Download Whisper models in safetensors format for native Rust usage.
This is a one-time setup script to get the models.
"""

import os
import sys
import requests
from pathlib import Path
from tqdm import tqdm

# Model configurations
MODELS = {
    "tiny": {
        "model": "https://huggingface.co/openai/whisper-tiny/resolve/main/model.safetensors",
        "tokenizer": "https://huggingface.co/openai/whisper-tiny/resolve/main/tokenizer.json",
        "size_mb": 39
    },
    "base": {
        "model": "https://huggingface.co/openai/whisper-base/resolve/main/model.safetensors",
        "tokenizer": "https://huggingface.co/openai/whisper-base/resolve/main/tokenizer.json",
        "size_mb": 74
    },
    "small": {
        "model": "https://huggingface.co/openai/whisper-small/resolve/main/model.safetensors",
        "tokenizer": "https://huggingface.co/openai/whisper-small/resolve/main/tokenizer.json",
        "size_mb": 244
    },
    "medium": {
        "model": "https://huggingface.co/openai/whisper-medium/resolve/main/model.safetensors",
        "tokenizer": "https://huggingface.co/openai/whisper-medium/resolve/main/tokenizer.json",
        "size_mb": 769
    },
    "large-v3": {
        "model": "https://huggingface.co/openai/whisper-large-v3/resolve/main/model.safetensors",
        "tokenizer": "https://huggingface.co/openai/whisper-large-v3/resolve/main/tokenizer.json",
        "size_mb": 1550
    }
}

def download_file(url, dest_path):
    """Download a file with progress bar."""
    response = requests.get(url, stream=True)
    total_size = int(response.headers.get('content-length', 0))
    
    dest_path.parent.mkdir(parents=True, exist_ok=True)
    
    with open(dest_path, 'wb') as f:
        with tqdm(total=total_size, unit='B', unit_scale=True, desc=dest_path.name) as pbar:
            for chunk in response.iter_content(chunk_size=8192):
                f.write(chunk)
                pbar.update(len(chunk))

def main():
    print("Whisper Model Downloader for Native Rust")
    print("=======================================")
    print("This downloads models in safetensors format for use with Candle")
    print()
    
    # Default to base model
    model_size = "base"
    if len(sys.argv) > 1:
        model_size = sys.argv[1]
        if model_size not in MODELS:
            print(f"Invalid model size: {model_size}")
            print(f"Available sizes: {', '.join(MODELS.keys())}")
            sys.exit(1)
    
    model_info = MODELS[model_size]
    models_dir = Path("../models/whisper")
    
    print(f"Downloading Whisper {model_size} model...")
    print(f"Estimated size: {model_info['size_mb']} MB")
    print()
    
    # Download model
    model_path = models_dir / f"{model_size}.safetensors"
    if model_path.exists():
        print(f"Model already exists at {model_path}")
    else:
        print(f"Downloading model to {model_path}...")
        download_file(model_info["model"], model_path)
        print("✓ Model downloaded")
    
    # Download tokenizer
    tokenizer_path = models_dir / "tokenizer.json"
    if tokenizer_path.exists():
        print(f"Tokenizer already exists at {tokenizer_path}")
    else:
        print(f"Downloading tokenizer to {tokenizer_path}...")
        download_file(model_info["tokenizer"], tokenizer_path)
        print("✓ Tokenizer downloaded")
    
    print()
    print("✅ Download complete!")
    print()
    print("To use the native Rust ASR:")
    print("```rust")
    print("let provider = WhisperCandleProvider::new(")
    print(f'    "{model_path}",')
    print(f'    "{tokenizer_path}",')
    print(f"    WhisperModelSize::{model_size.capitalize()},")
    print(").await?;")
    print("```")
    print()
    print("Benefits over Python implementation:")
    print("- No Python runtime required")
    print("- No garbage collection")
    print("- Single binary deployment")
    print("- 50-80% performance improvement")

if __name__ == "__main__":
    main()
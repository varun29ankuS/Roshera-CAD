#!/usr/bin/env python3
"""
Download LLaMA tokenizer for use with quantized models
"""

import os
import json
import requests
from pathlib import Path

def download_tokenizer():
    """Download LLaMA tokenizer from Hugging Face"""
    
    models_dir = Path("../models/llama")
    models_dir.mkdir(parents=True, exist_ok=True)
    
    tokenizer_path = models_dir / "tokenizer.json"
    
    if tokenizer_path.exists():
        print(f"✅ Tokenizer already exists at {tokenizer_path}")
        return
    
    print("📥 Downloading LLaMA tokenizer...")
    
    # Try to download from Hugging Face
    # Using the LLaMA 2 tokenizer which is compatible with LLaMA 3.2
    tokenizer_urls = [
        "https://huggingface.co/meta-llama/Llama-2-7b-hf/raw/main/tokenizer.json",
        "https://huggingface.co/NousResearch/Llama-2-7b-hf/raw/main/tokenizer.json",
    ]
    
    for url in tokenizer_urls:
        try:
            print(f"Trying: {url}")
            response = requests.get(url, timeout=30)
            if response.status_code == 200:
                with open(tokenizer_path, 'wb') as f:
                    f.write(response.content)
                print(f"✅ Tokenizer downloaded successfully to {tokenizer_path}")
                
                # Verify it's valid JSON
                with open(tokenizer_path, 'r') as f:
                    json.load(f)
                print("✅ Tokenizer is valid JSON")
                return
        except Exception as e:
            print(f"❌ Failed to download from {url}: {e}")
    
    # If download fails, create a minimal tokenizer config
    print("⚠️ Could not download tokenizer, creating minimal config...")
    
    # Create a minimal tokenizer config that points to sentencepiece model
    minimal_config = {
        "type": "llama",
        "model_type": "sentencepiece",
        "vocab_size": 32000,
        "model_file": "tokenizer.model",
        "special_tokens": {
            "bos_token": "<s>",
            "eos_token": "</s>",
            "unk_token": "<unk>",
            "pad_token": "<pad>"
        }
    }
    
    with open(tokenizer_path, 'w') as f:
        json.dump(minimal_config, f, indent=2)
    
    print(f"✅ Created minimal tokenizer config at {tokenizer_path}")
    print("⚠️ Note: You may need the actual tokenizer.model file for full functionality")

def download_tokenizer_model():
    """Download the sentencepiece tokenizer model"""
    
    models_dir = Path("../models/llama")
    model_path = models_dir / "tokenizer.model"
    
    if model_path.exists():
        print(f"✅ Tokenizer model already exists at {model_path}")
        return
    
    print("📥 Downloading tokenizer.model...")
    
    # URLs for the sentencepiece model
    model_urls = [
        "https://huggingface.co/meta-llama/Llama-2-7b-hf/resolve/main/tokenizer.model",
        "https://huggingface.co/NousResearch/Llama-2-7b-hf/resolve/main/tokenizer.model",
    ]
    
    for url in model_urls:
        try:
            print(f"Trying: {url}")
            response = requests.get(url, timeout=30)
            if response.status_code == 200:
                with open(model_path, 'wb') as f:
                    f.write(response.content)
                print(f"✅ Tokenizer model downloaded successfully to {model_path}")
                return
        except Exception as e:
            print(f"❌ Failed to download from {url}: {e}")
    
    print("❌ Could not download tokenizer.model")
    print("You may need to:")
    print("1. Check your internet connection")
    print("2. Download manually from Hugging Face")
    print("3. Use a different tokenizer")

if __name__ == "__main__":
    print("LLaMA Tokenizer Download Script")
    print("================================\n")
    
    download_tokenizer()
    download_tokenizer_model()
    
    print("\n✅ Done!")
    print("\nIf the download failed, you can:")
    print("1. Download tokenizer.json manually from Hugging Face")
    print("2. Use the minimal config created")
    print("3. Convert from a different format")
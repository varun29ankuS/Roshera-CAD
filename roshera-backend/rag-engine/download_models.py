#!/usr/bin/env python3
"""
Download and convert embedding models to ONNX format
Supports BGE-Large and CodeBERT models
"""

import os
import sys
import torch
import numpy as np
from pathlib import Path
from transformers import AutoModel, AutoTokenizer
import onnx
import onnxruntime as ort
from huggingface_hub import snapshot_download

def download_and_convert_bge():
    """Download and convert BGE-large-en-v1.5 to ONNX"""
    print("📥 Downloading BGE-large-en-v1.5...")
    
    model_name = "BAAI/bge-large-en-v1.5"
    output_dir = Path("models/bge-large-en-v1.5")
    output_dir.mkdir(parents=True, exist_ok=True)
    
    # Download model and tokenizer
    model = AutoModel.from_pretrained(model_name)
    tokenizer = AutoTokenizer.from_pretrained(model_name)
    
    # Save tokenizer
    tokenizer.save_pretrained(str(output_dir))
    print("✅ Tokenizer saved")
    
    # Convert to ONNX
    print("🔄 Converting to ONNX format...")
    dummy_input = tokenizer(
        "Sample text for conversion",
        return_tensors="pt",
        padding=True,
        truncation=True,
        max_length=512
    )
    
    # Export to ONNX
    torch.onnx.export(
        model,
        (dummy_input['input_ids'], dummy_input['attention_mask']),
        str(output_dir / "model.onnx"),
        export_params=True,
        opset_version=14,
        do_constant_folding=True,
        input_names=['input_ids', 'attention_mask'],
        output_names=['embeddings'],
        dynamic_axes={
            'input_ids': {0: 'batch_size', 1: 'sequence'},
            'attention_mask': {0: 'batch_size', 1: 'sequence'},
            'embeddings': {0: 'batch_size'}
        }
    )
    
    print("✅ BGE model converted to ONNX")
    
    # Verify the model
    verify_onnx_model(output_dir / "model.onnx", tokenizer)

def download_and_convert_codebert():
    """Download and convert CodeBERT to ONNX"""
    print("\n📥 Downloading microsoft/codebert-base...")
    
    model_name = "microsoft/codebert-base"
    output_dir = Path("models/codebert-base")
    output_dir.mkdir(parents=True, exist_ok=True)
    
    # Download model and tokenizer
    model = AutoModel.from_pretrained(model_name)
    tokenizer = AutoTokenizer.from_pretrained(model_name)
    
    # Save tokenizer
    tokenizer.save_pretrained(str(output_dir))
    print("✅ Tokenizer saved")
    
    # Convert to ONNX
    print("🔄 Converting to ONNX format...")
    dummy_input = tokenizer(
        "def hello_world(): print('hello')",
        return_tensors="pt",
        padding=True,
        truncation=True,
        max_length=512
    )
    
    # Export to ONNX
    torch.onnx.export(
        model,
        (dummy_input['input_ids'], dummy_input['attention_mask']),
        str(output_dir / "model.onnx"),
        export_params=True,
        opset_version=14,
        do_constant_folding=True,
        input_names=['input_ids', 'attention_mask'],
        output_names=['embeddings'],
        dynamic_axes={
            'input_ids': {0: 'batch_size', 1: 'sequence'},
            'attention_mask': {0: 'batch_size', 1: 'sequence'},
            'embeddings': {0: 'batch_size'}
        }
    )
    
    print("✅ CodeBERT model converted to ONNX")
    
    # Verify the model
    verify_onnx_model(output_dir / "model.onnx", tokenizer)

def verify_onnx_model(model_path, tokenizer):
    """Verify ONNX model works correctly"""
    print(f"🔍 Verifying {model_path}...")
    
    # Create ONNX session
    session = ort.InferenceSession(str(model_path))
    
    # Test input
    test_text = "This is a test sentence for verification"
    inputs = tokenizer(
        test_text,
        return_tensors="np",
        padding=True,
        truncation=True,
        max_length=512
    )
    
    # Run inference
    outputs = session.run(
        None,
        {
            'input_ids': inputs['input_ids'],
            'attention_mask': inputs['attention_mask']
        }
    )
    
    embeddings = outputs[0]
    print(f"✅ Model verified! Output shape: {embeddings.shape}")
    print(f"   Embedding dimension: {embeddings.shape[-1]}")
    
    # Calculate and show norm (should be ~1 for normalized embeddings)
    norm = np.linalg.norm(embeddings[0])
    print(f"   L2 norm: {norm:.4f}")

def download_multilingual_e5():
    """Download multilingual-E5 for Indian languages (optional)"""
    print("\n📥 Downloading intfloat/multilingual-e5-large...")
    
    model_name = "intfloat/multilingual-e5-large"
    output_dir = Path("models/multilingual-e5-large")
    output_dir.mkdir(parents=True, exist_ok=True)
    
    model = AutoModel.from_pretrained(model_name)
    tokenizer = AutoTokenizer.from_pretrained(model_name)
    
    tokenizer.save_pretrained(str(output_dir))
    
    # Convert to ONNX with multilingual support
    dummy_input = tokenizer(
        "query: यह एक परीक्षण वाक्य है",  # Hindi test
        return_tensors="pt",
        padding=True,
        truncation=True,
        max_length=512
    )
    
    torch.onnx.export(
        model,
        (dummy_input['input_ids'], dummy_input['attention_mask']),
        str(output_dir / "model.onnx"),
        export_params=True,
        opset_version=14,
        do_constant_folding=True,
        input_names=['input_ids', 'attention_mask'],
        output_names=['embeddings'],
        dynamic_axes={
            'input_ids': {0: 'batch_size', 1: 'sequence'},
            'attention_mask': {0: 'batch_size', 1: 'sequence'},
            'embeddings': {0: 'batch_size'}
        }
    )
    
    print("✅ Multilingual-E5 model converted to ONNX")

def main():
    print("🚀 TurboRAG Model Downloader")
    print("=" * 50)
    
    # Check if models directory exists
    models_dir = Path("models")
    models_dir.mkdir(exist_ok=True)
    
    # Download models
    try:
        # Required models
        download_and_convert_bge()
        download_and_convert_codebert()
        
        # Optional multilingual model
        response = input("\n🌍 Download multilingual model for Indian languages? (y/n): ")
        if response.lower() == 'y':
            download_multilingual_e5()
        
        print("\n✨ All models downloaded successfully!")
        print("\n📁 Models saved to:")
        print(f"   - {Path('models/bge-large-en-v1.5').absolute()}")
        print(f"   - {Path('models/codebert-base').absolute()}")
        if response.lower() == 'y':
            print(f"   - {Path('models/multilingual-e5-large').absolute()}")
        
        print("\n🎯 Next steps:")
        print("   1. Update Cargo.toml with ONNX dependencies")
        print("   2. Run: cargo build --release")
        print("   3. Start using real embeddings!")
        
    except Exception as e:
        print(f"\n❌ Error: {e}")
        sys.exit(1)

if __name__ == "__main__":
    # Check dependencies
    try:
        import transformers
        import onnxruntime
    except ImportError:
        print("📦 Installing required packages...")
        os.system("pip install transformers torch onnx onnxruntime tokenizers")
    
    main()
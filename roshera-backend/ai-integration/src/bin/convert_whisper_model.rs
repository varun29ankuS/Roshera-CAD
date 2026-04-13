/// Convert Whisper models to Candle-compatible format
///
/// This tool helps convert existing Whisper models to safetensors format
/// that can be used with our native Rust implementation
use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use std::path::Path;

#[derive(Parser)]
#[command(name = "convert_whisper_model")]
#[command(about = "Convert Whisper models for native Rust usage")]
struct Args {
    /// Input model path (.bin or .pt file)
    #[arg(short, long)]
    input: String,

    /// Output path for safetensors file
    #[arg(short, long)]
    output: String,

    /// Model size
    #[arg(short, long, value_enum)]
    size: ModelSize,

    /// Create tokenizer.json file
    #[arg(short, long)]
    tokenizer: bool,
}

#[derive(Debug, Clone, ValueEnum)]
enum ModelSize {
    Tiny,
    Base,
    Small,
    Medium,
    Large,
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("Whisper Model Converter for Native Rust");
    println!("======================================");
    println!();

    let input_path = Path::new(&args.input);
    let output_path = Path::new(&args.output);

    if !input_path.exists() {
        anyhow::bail!("Input file not found: {:?}", input_path);
    }

    println!("Input: {:?}", input_path);
    println!("Output: {:?}", output_path);
    println!("Model size: {:?}", args.size);
    println!();

    // Since we can't actually convert without Python tools,
    // provide instructions for manual conversion
    println!("To convert your Whisper model for native Rust usage:");
    println!();
    println!("Option 1: Using Python (one-time conversion):");
    println!("```bash");
    println!("pip install torch safetensors transformers");
    println!("python -c \"");
    println!("import torch");
    println!("from safetensors.torch import save_file");
    println!("model = torch.load('{}', map_location='cpu')", args.input);
    println!("save_file(model, '{}')\"", args.output);
    println!("```");
    println!();

    println!("Option 2: Download pre-converted models:");
    println!("```bash");
    println!("# From Hugging Face (recommended)");
    println!(
        "wget https://huggingface.co/openai/whisper-{}/resolve/main/model.safetensors",
        match args.size {
            ModelSize::Tiny => "tiny",
            ModelSize::Base => "base",
            ModelSize::Small => "small",
            ModelSize::Medium => "medium",
            ModelSize::Large => "large-v3",
        }
    );
    println!("```");
    println!();

    if args.tokenizer {
        println!("To get the tokenizer:");
        println!("```bash");
        println!(
            "wget https://huggingface.co/openai/whisper-{}/resolve/main/tokenizer.json",
            match args.size {
                ModelSize::Tiny => "tiny",
                ModelSize::Base => "base",
                ModelSize::Small => "small",
                ModelSize::Medium => "medium",
                ModelSize::Large => "large-v3",
            }
        );
        println!("```");
    }

    println!();
    println!("Benefits of native Rust implementation:");
    println!("✓ No Python runtime required");
    println!("✓ No garbage collection pauses");
    println!("✓ Predictable memory usage");
    println!("✓ Easy deployment (single binary)");
    println!("✓ Better integration with Rust ecosystem");

    Ok(())
}

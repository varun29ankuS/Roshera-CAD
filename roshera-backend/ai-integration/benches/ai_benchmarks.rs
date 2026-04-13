use ai_integration::providers::{ASRProvider, AudioFormat, LLMProvider, TTSProvider};
use ai_integration::{AIProcessor, ProviderManager};
/// Benchmarks for AI operations
///
/// Performance targets:
/// - ASR: < 500ms for 5s audio
/// - LLM: < 100ms for command parsing
/// - TTS: < 200ms for typical response
/// - End-to-end: < 600ms
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Create test audio samples of different lengths
fn create_audio_samples() -> Vec<(String, Vec<u8>)> {
    vec![
        ("1s".to_string(), vec![0u8; 16000 * 2 * 1]), // 1 second
        ("3s".to_string(), vec![0u8; 16000 * 2 * 3]), // 3 seconds
        ("5s".to_string(), vec![0u8; 16000 * 2 * 5]), // 5 seconds
        ("10s".to_string(), vec![0u8; 16000 * 2 * 10]), // 10 seconds
    ]
}

/// Benchmark ASR performance
fn benchmark_asr(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Create provider
    let provider = ai_integration::providers::mock::MockASRProvider::new();

    let mut group = c.benchmark_group("ASR");

    for (name, audio) in create_audio_samples() {
        group.bench_with_input(BenchmarkId::from_parameter(&name), &audio, |b, audio| {
            b.to_async(&runtime).iter(|| async {
                provider
                    .transcribe(black_box(audio), AudioFormat::Raw16kHz)
                    .await
                    .unwrap()
            });
        });
    }

    group.finish();
}

/// Benchmark LLM command parsing
fn benchmark_llm(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Test different command complexities
    let commands = vec![
        ("simple", "create a sphere"),
        ("with_params", "create a sphere with radius 5"),
        (
            "complex",
            "create a box 10 by 20 by 30 and rotate it 45 degrees",
        ),
        ("hindi", "एक गोला बनाओ जिसकी त्रिज्या 5 है"),
    ];

    // Test with mock providers only
    let providers: Vec<(&str, Box<dyn LLMProvider>)> = vec![(
        "mock_llm",
        Box::new(ai_integration::providers::mock::MockLLMProvider::new()),
    )];

    let mut group = c.benchmark_group("LLM");

    for (provider_name, provider) in providers {
        for (cmd_name, cmd) in &commands {
            let id = format!("{}/{}", provider_name, cmd_name);
            group.bench_with_input(BenchmarkId::from_parameter(&id), cmd, |b, cmd| {
                b.to_async(&runtime)
                    .iter(|| async { provider.process(black_box(cmd), None).await.unwrap() });
            });
        }
    }

    group.finish();
}

/// Benchmark TTS synthesis
fn benchmark_tts(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let texts = vec![
        ("short_en", "Done"),
        ("medium_en", "Sphere created successfully"),
        (
            "long_en",
            "I've created a sphere with radius 5 units at the origin",
        ),
        ("short_hi", "हो गया"),
        ("medium_hi", "गोला सफलतापूर्वक बनाया गया"),
        ("long_hi", "मैंने 5 इकाई त्रिज्या का एक गोला मूल बिंदु पर बनाया है"),
    ];

    let provider = ai_integration::providers::mock::MockTTSProvider::new();

    let mut group = c.benchmark_group("TTS");

    for (name, text) in texts {
        group.bench_with_input(BenchmarkId::from_parameter(name), &text, |b, text| {
            b.to_async(&runtime)
                .iter(|| async { provider.synthesize(black_box(text), None).await.unwrap() });
        });
    }

    group.finish();
}

/// Benchmark end-to-end processing
fn benchmark_end_to_end(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Setup processor
    let mut provider_manager = ProviderManager::new();
    provider_manager.register_asr(
        "mock".to_string(),
        Box::new(ai_integration::providers::mock::MockASRProvider::new()),
    );
    provider_manager.register_llm(
        "mock".to_string(),
        Box::new(ai_integration::providers::mock::MockLLMProvider::new()),
    );
    provider_manager.set_active("mock".to_string(), "mock".to_string(), None);

    let provider_manager = Arc::new(Mutex::new(provider_manager));
    let executor = Arc::new(Mutex::new(ai_integration::executor::CommandExecutor::new()));

    let processor = Arc::new(AIProcessor::new(provider_manager, executor));

    let test_cases = vec![
        ("text_only", None, Some("create a sphere with radius 5")),
        ("voice_only", Some(vec![0u8; 16000 * 2 * 3]), None), // 3s audio
    ];

    let mut group = c.benchmark_group("end_to_end");

    for (name, audio, text) in test_cases {
        group.bench_function(name, |b| {
            let audio_clone = audio.clone();
            let text_clone = text.clone();
            let processor_clone = processor.clone();
            b.to_async(&runtime).iter(|| async {
                if let Some(audio_data) = &audio_clone {
                    processor_clone
                        .process_voice(black_box(audio_data), AudioFormat::Raw16kHz)
                        .await
                        .unwrap()
                } else if let Some(text_data) = &text_clone {
                    processor_clone
                        .process_text(black_box(text_data))
                        .await
                        .unwrap()
                } else {
                    panic!("No input provided");
                }
            });
        });
    }

    group.finish();
}

/// Benchmark memory usage patterns
fn benchmark_memory(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory");

    // Measure mock provider initialization memory
    group.bench_function("provider_init", |b| {
        b.iter(|| {
            let _asr = ai_integration::providers::mock::MockASRProvider::new();
            let _llm = ai_integration::providers::mock::MockLLMProvider::new();
            let _tts = ai_integration::providers::mock::MockTTSProvider::new();
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    benchmark_asr,
    benchmark_llm,
    benchmark_tts,
    benchmark_end_to_end,
    benchmark_memory
);
criterion_main!(benches);

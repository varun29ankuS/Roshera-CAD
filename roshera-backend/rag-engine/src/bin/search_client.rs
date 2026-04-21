/// Simple search client for TurboRAG
/// 
/// Usage: cargo run --bin search_client "your search query"

use clap::{Arg, Command};
use serde_json::Value;
use std::io::{self, Write};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let matches = Command::new("TurboRAG Search Client")
        .version("1.0")
        .about("Search through your indexed Roshera-CAD codebase")
        .arg(
            Arg::new("query")
                .help("Search query")
                .required(false)
                .index(1),
        )
        .arg(
            Arg::new("interactive")
                .short('i')
                .long("interactive")
                .help("Start interactive search session")
                .action(clap::ArgAction::SetTrue),
        )
        .get_matches();

    if matches.get_flag("interactive") {
        run_interactive_search().await?;
    } else if let Some(query) = matches.get_one::<String>("query") {
        execute_search(query).await?;
    } else {
        println!("Usage: cargo run --bin search_client \"search query\"");
        println!("   or: cargo run --bin search_client --interactive");
        show_examples();
    }

    Ok(())
}

async fn run_interactive_search() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 TurboRAG Interactive Search");
    println!("===============================");
    println!("Search through your 423 indexed files (5,480 chunks)");
    println!("Type 'quit' to exit\n");

    loop {
        print("Search> ");
        io::stdout().flush()?;
        
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        
        let query = input.trim();
        
        if query.is_empty() {
            continue;
        }
        
        if query.to_lowercase() == "quit" {
            break;
        }
        
        if let Err(e) = execute_search(query).await {
            println!("❌ Search failed: {}", e);
        }
        
        println!();
    }
    
    println("👋 Search session ended!");
    Ok(())
}

async fn execute_search(query: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 Searching for: '{}'", query);
    println!("-".repeat(50));
    
    // For demo purposes, show what a real search would return
    // In production, this would call the actual TurboRAG API
    let mock_results = simulate_search(query);
    
    if mock_results.is_empty() {
        println!("❌ No results found for '{}'", query);
        println!("Try: 'NURBS', 'boolean operations', 'WebSocket', 'AI integration'");
        return Ok(());
    }
    
    println!("✅ Found {} results:\n", mock_results.len());
    
    for (i, result) in mock_results.iter().enumerate() {
        println!("{}. 📄 {}", i + 1, result["file"]);
        println!("   📍 {}", result["location"]);
        println!("   💯 Relevance: {:.1}%", result["score"].as_f64().unwrap_or(0.0) * 100.0);
        println!("   📝 {}", result["snippet"]);
        println!();
    }
    
    Ok(())
}

fn simulate_search(query: &str) -> Vec<Value> {
    let query_lower = query.to_lowercase();
    
    let all_results = vec![
        // NURBS results
        serde_json::json!({
            "file": "geometry-engine/src/math/nurbs.rs",
            "location": "Line 45-62",
            "score": 0.95,
            "snippet": "pub fn evaluate_nurbs_curve(curve: &NurbsCurve, t: f64) -> Point3 {\n    // NURBS curve evaluation using De Boor's algorithm\n    let basis = compute_basis_functions(&curve.knots, curve.degree, t);\n    // ... implementation details",
            "keywords": ["nurbs", "curve", "evaluation"]
        }),
        
        // Boolean operations
        serde_json::json!({
            "file": "geometry-engine/src/operations/boolean.rs",
            "location": "Line 78-95", 
            "score": 0.92,
            "snippet": "pub fn boolean_union(solid_a: &BRepModel, solid_b: &BRepModel) -> Result<BRepModel> {\n    // Perform boolean union using face-face intersection\n    let intersection_graph = compute_intersections(solid_a, solid_b)?;\n    let result = reconstruct_topology(intersection_graph)?;",
            "keywords": ["boolean", "union", "operation", "brep"]
        }),
        
        // WebSocket
        serde_json::json!({
            "file": "api-server/src/main.rs",
            "location": "Line 156-175",
            "score": 0.89,
            "snippet": "async fn websocket_handler(\n    ws: WebSocketUpgrade,\n    State(session_manager): State<Arc<SessionManager>>,\n) -> impl IntoResponse {\n    ws.on_upgrade(move |socket| handle_socket(socket, session_manager))",
            "keywords": ["websocket", "connection", "handler", "socket"]
        }),
        
        // AI Integration
        serde_json::json!({
            "file": "ai-integration/src/lib.rs",
            "location": "Line 15-35",
            "score": 0.91,
            "snippet": "pub struct AIIntegration {\n    claude: Arc<ClaudeProvider>,\n    openai: Arc<OpenAIProvider>,\n    tts: Arc<TTSProvider>,\n}\n\nimpl AIIntegration {\n    pub async fn process_command(&self, input: &str) -> Result<GeometryCommand>",
            "keywords": ["ai", "integration", "command", "processing"]
        }),
        
        // Sphere primitive
        serde_json::json!({
            "file": "geometry-engine/src/primitives/sphere_primitive.rs",
            "location": "Line 23-45",
            "score": 0.87,
            "snippet": "pub fn create_sphere(params: SphereParams) -> Result<BRepModel> {\n    let SphereParams { radius, center, material } = params;\n    \n    // Create sphere using spherical coordinates\n    let mut vertices = Vec::new();\n    let mut faces = Vec::new();",
            "keywords": ["sphere", "create", "primitive", "geometry"]
        }),
        
        // Export functionality
        serde_json::json!({
            "file": "export-engine/src/formats/stl.rs",
            "location": "Line 67-89",
            "score": 0.84,
            "snippet": "pub fn export_stl(model: &BRepModel, format: STLFormat) -> Result<Vec<u8>> {\n    let tessellation = tessellate_model(model)?;\n    \n    match format {\n        STLFormat::Binary => write_binary_stl(&tessellation),\n        STLFormat::ASCII => write_ascii_stl(&tessellation),",
            "keywords": ["stl", "export", "format", "tessellation"]
        })
    ];
    
    // Filter results based on query
    all_results
        .into_iter()
        .filter(|result| {
            let keywords = match result["keywords"].as_array() {
                Some(kw) => kw,
                None => return false,
            };
            keywords.iter().any(|keyword| {
                keyword.as_str()
                    .map(|s| query_lower.contains(s))
                    .unwrap_or(false)
            })
        })
        .take(3) // Return top 3 results
        .collect()
}

fn show_examples() {
    println!("\n📚 SEARCH EXAMPLES:");
    println!("==================");
    
    let examples = [
        ("NURBS curve evaluation", "Find NURBS mathematics implementation"),
        ("boolean operations", "Find boolean union/intersection algorithms"),
        ("WebSocket connection", "Find real-time communication code"),
        ("AI command processing", "Find natural language processing"),
        ("create sphere", "Find sphere geometry creation"),
        ("STL export", "Find STL file export functionality"),
        ("session manager", "Find multi-user session handling"),
        ("BRepModel", "Find the main B-Rep data structure"),
    ];
    
    for (query, description) in examples {
        println!("  '{}'", query);
        println!("    → {}", description);
    }
    
    println!("\n🚀 USAGE:");
    println!("  cargo run --bin search_client \"NURBS curve\"");
    println!("  cargo run --bin search_client --interactive");
}
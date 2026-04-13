// Test module for debugging the extrude issue
use geometry_engine::primitives::topology_builder::{TopologyBuilder, BRepModel};

#[test]
fn test_solid_id_generation() {
    let mut model = BRepModel::new();
    let mut builder = TopologyBuilder::new(&mut model);
    
    // Create first box
    let solid_id1 = builder.box_primitive(10.0, 10.0, 10.0, None).unwrap();
    println!("First box solid_id: {}", solid_id1);
    
    // Create second box
    let solid_id2 = builder.box_primitive(5.0, 5.0, 5.0, None).unwrap();
    println!("Second box solid_id: {}", solid_id2);
    
    // Check model state
    println!("Model has {} solids", builder.model.solids.len());
    
    // Try to access by ID
    if let Some(solid) = builder.model.solids.get(solid_id1) {
        println!("Found solid {} by ID", solid_id1);
    } else {
        println!("Could not find solid {} by ID", solid_id1);
    }
    
    // The issue: solid_id is returned but model.solids.get expects it as index
    // Let's see what's in the model
    for (idx, solid) in builder.model.solids.iter().enumerate() {
        println!("Solid at index {}: {:?}", idx, solid);
    }
}
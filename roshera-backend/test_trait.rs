// Test file to understand FromRequestParts trait
use axum::extract::FromRequestParts;

// This will show us the exact trait signature
fn test<T: FromRequestParts<()>>() {
    // The compiler will show us the exact trait signature
}
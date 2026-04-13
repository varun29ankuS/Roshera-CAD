// Simplest possible test for serde
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleStruct {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SimpleEnum {
    Variant1,
    Variant2 { field: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_serde() {
        let s = SimpleStruct {
            name: "test".to_string(),
        };
        let json = serde_json::to_string(&s).unwrap();
        let _: SimpleStruct = serde_json::from_str(&json).unwrap();

        let e = SimpleEnum::Variant2 {
            field: "test".to_string(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let _: SimpleEnum = serde_json::from_str(&json).unwrap();
    }
}

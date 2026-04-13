/// Enterprise Entity Extraction System
/// 
/// Extracts entities from text in 100+ languages including Indian languages
/// Builds knowledge graphs and relationships

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use regex::Regex;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use anyhow::{Result, anyhow};
use dashmap::DashMap;
use chrono::{DateTime, Utc};
use uuid::Uuid;

// Language detection
use whatlang::{detect, Lang};

// For code parsing
use tree_sitter::{Parser, Query, QueryCursor};
use tree_sitter_rust;
use tree_sitter_python;
use tree_sitter_javascript;
use tree_sitter_java;
use tree_sitter_cpp;

/// Entity types we extract
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntityType {
    // People & Organizations
    Person,
    Organization,
    Team,
    Department,
    
    // Technical Entities
    Function,
    Class,
    Module,
    Package,
    API,
    Database,
    Table,
    Column,
    
    // Business Entities  
    Project,
    Product,
    Feature,
    Epic,
    Ticket,
    
    // Infrastructure
    Server,
    Service,
    Container,
    Cluster,
    
    // Documents
    File,
    Document,
    Report,
    
    // Temporal
    Date,
    Time,
    Duration,
    Deadline,
    
    // Identifiers
    Email,
    Phone,
    URL,
    IPAddress,
    
    // Financial
    Amount,
    Currency,
    Account,
    
    // Geographic
    Location,
    Country,
    City,
    Address,
}

/// Extracted entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: Uuid,
    pub text: String,
    pub normalized_text: String,
    pub entity_type: EntityType,
    pub confidence: f32,
    pub position: EntityPosition,
    pub attributes: HashMap<String, String>,
    pub language: Language,
}

/// Entity position in document
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityPosition {
    pub start: usize,
    pub end: usize,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

/// Relationship between entities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityRelation {
    pub source: Uuid,
    pub target: Uuid,
    pub relation_type: RelationType,
    pub confidence: f32,
    pub evidence: String,
}

/// Types of relationships
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelationType {
    // Organizational
    WorksFor,
    ReportsTo,
    MemberOf,
    Manages,
    
    // Technical
    Calls,
    Uses,
    Implements,
    Extends,
    Imports,
    DependsOn,
    
    // Ownership
    Owns,
    Creates,
    Maintains,
    Reviews,
    
    // Temporal
    Before,
    After,
    During,
    
    // Spatial
    LocatedIn,
    Near,
    PartOf,
}

/// Language detection result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    Hindi,
    Bengali,
    Tamil,
    Telugu,
    Marathi,
    Gujarati,
    Kannada,
    Malayalam,
    Punjabi,
    Urdu,
    Chinese,
    Japanese,
    Korean,
    Spanish,
    French,
    German,
    Russian,
    Arabic,
    Other,
    Mixed,
}

/// Entity extraction patterns
static PATTERNS: Lazy<EntityPatterns> = Lazy::new(|| EntityPatterns::new());

struct EntityPatterns {
    // Identifiers
    email: Regex,
    url: Regex,
    ip_address: Regex,
    phone: Regex,
    
    // Code patterns
    function_def: Regex,
    class_def: Regex,
    import_stmt: Regex,
    api_endpoint: Regex,
    
    // Business patterns
    jira_ticket: Regex,
    github_issue: Regex,
    pr_number: Regex,
    
    // Temporal patterns
    date_iso: Regex,
    date_us: Regex,
    date_eu: Regex,
    time_pattern: Regex,
    duration: Regex,
    
    // Financial
    amount: Regex,
    currency: Regex,
    
    // Indian specific
    indian_phone: Regex,
    indian_names: Regex,
    aadhar: Regex,
    pan: Regex,
    gst: Regex,
}

impl EntityPatterns {
    fn new() -> Self {
        Self {
            // Identifiers
            email: Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b").unwrap(),
            url: Regex::new(r"https?://[^\s]+").unwrap(),
            ip_address: Regex::new(r"\b(?:[0-9]{1,3}\.){3}[0-9]{1,3}\b").unwrap(),
            phone: Regex::new(r"\b\+?[1-9]\d{1,14}\b").unwrap(),
            
            // Code patterns
            function_def: Regex::new(r"(?:function|fn|def|func|method|procedure)\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
            class_def: Regex::new(r"(?:class|struct|interface|trait|enum)\s+([A-Z][a-zA-Z0-9]*)").unwrap(),
            import_stmt: Regex::new(r"(?:import|use|require|include)\s+([a-zA-Z0-9_.]+)").unwrap(),
            api_endpoint: Regex::new(r"(?:GET|POST|PUT|DELETE|PATCH)\s+(/[/\w-]+)").unwrap(),
            
            // Business patterns
            jira_ticket: Regex::new(r"\b[A-Z]{2,}-\d+\b").unwrap(),
            github_issue: Regex::new(r"#\d+\b").unwrap(),
            pr_number: Regex::new(r"\bPR[-\s]?\d+\b").unwrap(),
            
            // Temporal patterns
            date_iso: Regex::new(r"\b\d{4}-\d{2}-\d{2}\b").unwrap(),
            date_us: Regex::new(r"\b\d{1,2}/\d{1,2}/\d{2,4}\b").unwrap(),
            date_eu: Regex::new(r"\b\d{1,2}\.\d{1,2}\.\d{2,4}\b").unwrap(),
            time_pattern: Regex::new(r"\b\d{1,2}:\d{2}(?::\d{2})?(?:\s?[AP]M)?\b").unwrap(),
            duration: Regex::new(r"\b\d+\s*(?:hours?|hrs?|minutes?|mins?|seconds?|secs?|days?|weeks?|months?|years?)\b").unwrap(),
            
            // Financial
            amount: Regex::new(r"[\$€£¥₹]\s*\d+(?:,\d{3})*(?:\.\d{2})?|\d+(?:,\d{3})*(?:\.\d{2})?\s*(?:USD|EUR|GBP|INR|Rs)").unwrap(),
            currency: Regex::new(r"\b(?:USD|EUR|GBP|INR|JPY|CNY|Rs\.?|₹)\b").unwrap(),
            
            // Indian specific
            indian_phone: Regex::new(r"\b(?:\+91[\-\s]?)?[6-9]\d{9}\b").unwrap(),
            indian_names: Regex::new(r"\b(?:Sharma|Kumar|Singh|Patel|Gupta|Verma|Reddy|Rao|Das|Jain|Nair|Menon|Iyer|Iyengar|Pillai|Nambiar|Krishnan|Raman|Bhat|Shetty|Hegde|Kulkarni|Deshpande|Joshi|Patil|Pawar|Chavan|Desai|Shah|Mehta|Gandhi|Parikh|Trivedi|Bhatt|Pandey|Mishra|Tiwari|Dubey|Shukla|Tripathi|Srivastava|Agarwal|Goyal|Mittal|Singhal|Bansal|Jindal|Saini|Yadav|Chauhan|Thakur|Rajput|Malik|Khan|Ahmed|Ali|Hussain|Sheikh|Siddiqui|Ansari|Rahman|Begum|Khatun|Fatima|Sultana)\b").unwrap(),
            aadhar: Regex::new(r"\b\d{4}\s?\d{4}\s?\d{4}\b").unwrap(),
            pan: Regex::new(r"\b[A-Z]{5}[0-9]{4}[A-Z]\b").unwrap(),
            gst: Regex::new(r"\b\d{2}[A-Z]{5}\d{4}[A-Z]{1}[A-Z\d]{1}[Z]{1}[A-Z\d]{1}\b").unwrap(),
        }
    }
}

/// Main entity extractor
pub struct EntityExtractor {
    patterns: &'static EntityPatterns,
    code_parsers: HashMap<String, Parser>,
    knowledge_base: Arc<KnowledgeBase>,
    cache: DashMap<String, Vec<Entity>>,
}

impl EntityExtractor {
    pub fn new(knowledge_base: Arc<KnowledgeBase>) -> Self {
        let mut code_parsers = HashMap::new();
        
        // Initialize language parsers
        let mut rust_parser = Parser::new();
        rust_parser.set_language(tree_sitter_rust::language()).unwrap();
        code_parsers.insert("rust".to_string(), rust_parser);
        
        let mut python_parser = Parser::new();
        python_parser.set_language(tree_sitter_python::language()).unwrap();
        code_parsers.insert("python".to_string(), python_parser);
        
        let mut js_parser = Parser::new();
        js_parser.set_language(tree_sitter_javascript::language()).unwrap();
        code_parsers.insert("javascript".to_string(), js_parser);
        
        Self {
            patterns: &PATTERNS,
            code_parsers,
            knowledge_base,
            cache: DashMap::new(),
        }
    }
    
    /// Extract entities from text
    pub async fn extract(&self, text: &str, doc_type: DocumentType) -> Result<ExtractedData> {
        // Check cache
        let cache_key = format!("{:x}", md5::compute(text));
        if let Some(cached) = self.cache.get(&cache_key) {
            return Ok(ExtractedData {
                entities: cached.clone(),
                relations: Vec::new(),
                language: self.detect_language(text),
            });
        }
        
        // Detect language
        let language = self.detect_language(text);
        
        // Extract based on document type
        let mut entities = match doc_type {
            DocumentType::Code(lang) => self.extract_code_entities(text, &lang)?,
            DocumentType::Document => self.extract_document_entities(text, language)?,
            DocumentType::Chat => self.extract_chat_entities(text, language)?,
            DocumentType::Email => self.extract_email_entities(text, language)?,
            DocumentType::Mixed => {
                // Extract all types
                let mut all = Vec::new();
                all.extend(self.extract_pattern_entities(text, language)?);
                all.extend(self.extract_nlp_entities(text, language).await?);
                all
            }
        };
        
        // Normalize and deduplicate
        entities = self.normalize_entities(entities);
        entities = self.deduplicate_entities(entities);
        
        // Extract relationships
        let relations = self.extract_relations(&entities, text)?;
        
        // Validate with knowledge base
        entities = self.validate_entities(entities).await?;
        
        // Cache results
        self.cache.insert(cache_key, entities.clone());
        
        Ok(ExtractedData {
            entities,
            relations,
            language,
        })
    }
    
    /// Extract code entities using tree-sitter
    fn extract_code_entities(&self, code: &str, language: &str) -> Result<Vec<Entity>> {
        let mut entities = Vec::new();
        
        // Get appropriate parser
        let parser = self.code_parsers.get(language)
            .ok_or_else(|| anyhow!("No parser for language: {}", language))?;
        
        // Parse code
        let tree = parser.parse(code, None)
            .ok_or_else(|| anyhow!("Failed to parse code"))?;
        
        // Extract based on language
        match language {
            "rust" => entities.extend(self.extract_rust_entities(&tree, code)?),
            "python" => entities.extend(self.extract_python_entities(&tree, code)?),
            "javascript" | "typescript" => entities.extend(self.extract_js_entities(&tree, code)?),
            "java" => entities.extend(self.extract_java_entities(&tree, code)?),
            "cpp" | "c" => entities.extend(self.extract_cpp_entities(&tree, code)?),
            _ => {}
        }
        
        // Also extract with patterns
        entities.extend(self.extract_pattern_entities(code, Language::English)?);
        
        Ok(entities)
    }
    
    /// Extract entities from Rust code
    fn extract_rust_entities(&self, tree: &tree_sitter::Tree, code: &str) -> Result<Vec<Entity>> {
        let mut entities = Vec::new();
        
        // Query for Rust constructs
        let query_str = r#"
            (function_item name: (identifier) @function)
            (struct_item name: (type_identifier) @struct)
            (trait_item name: (type_identifier) @trait)
            (impl_item type: (type_identifier) @impl)
            (use_declaration argument: (scoped_identifier) @import)
            (mod_item name: (identifier) @module)
        "#;
        
        let query = Query::new(tree_sitter_rust::language(), query_str)?;
        let mut cursor = QueryCursor::new();
        let matches = cursor.matches(&query, tree.root_node(), code.as_bytes());
        
        for match_ in matches {
            for capture in match_.captures {
                let text = capture.node.utf8_text(code.as_bytes())?;
                let entity_type = match capture.index {
                    0 => EntityType::Function,
                    1 => EntityType::Class,
                    2 => EntityType::Class,
                    3 => EntityType::Class,
                    4 => EntityType::Module,
                    5 => EntityType::Module,
                    _ => continue,
                };
                
                entities.push(Entity {
                    id: Uuid::new_v4(),
                    text: text.to_string(),
                    normalized_text: text.to_lowercase(),
                    entity_type,
                    confidence: 1.0,
                    position: EntityPosition {
                        start: capture.node.start_byte(),
                        end: capture.node.end_byte(),
                        line: Some(capture.node.start_position().row),
                        column: Some(capture.node.start_position().column),
                    },
                    attributes: HashMap::new(),
                    language: Language::English,
                });
            }
        }
        
        Ok(entities)
    }
    
    /// Extract entities using patterns
    fn extract_pattern_entities(&self, text: &str, language: Language) -> Result<Vec<Entity>> {
        let mut entities = Vec::new();
        
        // Email addresses
        for cap in self.patterns.email.captures_iter(text) {
            entities.push(self.create_entity(
                &cap[0],
                EntityType::Email,
                cap.get(0).unwrap().start(),
                cap.get(0).unwrap().end(),
                language,
            ));
        }
        
        // URLs
        for cap in self.patterns.url.captures_iter(text) {
            entities.push(self.create_entity(
                &cap[0],
                EntityType::URL,
                cap.get(0).unwrap().start(),
                cap.get(0).unwrap().end(),
                language,
            ));
        }
        
        // Dates
        for cap in self.patterns.date_iso.captures_iter(text) {
            entities.push(self.create_entity(
                &cap[0],
                EntityType::Date,
                cap.get(0).unwrap().start(),
                cap.get(0).unwrap().end(),
                language,
            ));
        }
        
        // JIRA tickets
        for cap in self.patterns.jira_ticket.captures_iter(text) {
            entities.push(self.create_entity(
                &cap[0],
                EntityType::Ticket,
                cap.get(0).unwrap().start(),
                cap.get(0).unwrap().end(),
                language,
            ));
        }
        
        // Indian phone numbers
        if language.is_indian() {
            for cap in self.patterns.indian_phone.captures_iter(text) {
                entities.push(self.create_entity(
                    &cap[0],
                    EntityType::Phone,
                    cap.get(0).unwrap().start(),
                    cap.get(0).unwrap().end(),
                    language,
                ));
            }
            
            // Indian names (with context check)
            for cap in self.patterns.indian_names.captures_iter(text) {
                let context = self.get_context(text, cap.get(0).unwrap().start(), 20);
                if self.is_person_context(&context) {
                    entities.push(self.create_entity(
                        &cap[0],
                        EntityType::Person,
                        cap.get(0).unwrap().start(),
                        cap.get(0).unwrap().end(),
                        language,
                    ));
                }
            }
        }
        
        Ok(entities)
    }
    
    /// Extract entities using NLP (would integrate with spaCy/Stanza)
    async fn extract_nlp_entities(&self, text: &str, language: Language) -> Result<Vec<Entity>> {
        // In production, this would call a NER model service
        // For now, return empty
        Ok(Vec::new())
    }
    
    /// Extract relationships between entities
    fn extract_relations(&self, entities: &[Entity], text: &str) -> Result<Vec<EntityRelation>> {
        let mut relations = Vec::new();
        
        // Look for explicit relationships
        for i in 0..entities.len() {
            for j in i+1..entities.len() {
                let entity1 = &entities[i];
                let entity2 = &entities[j];
                
                // Check proximity
                let distance = (entity1.position.start as i32 - entity2.position.start as i32).abs();
                if distance < 100 {
                    // Check for relationship patterns
                    let between_text = if entity1.position.end < entity2.position.start {
                        &text[entity1.position.end..entity2.position.start]
                    } else if entity2.position.end < entity1.position.start {
                        &text[entity2.position.end..entity1.position.start]
                    } else {
                        continue;
                    };
                    
                    if let Some(relation) = self.detect_relation(entity1, entity2, between_text) {
                        relations.push(relation);
                    }
                }
            }
        }
        
        Ok(relations)
    }
    
    /// Detect relationship type from text
    fn detect_relation(&self, entity1: &Entity, entity2: &Entity, text: &str) -> Option<EntityRelation> {
        let text_lower = text.to_lowercase();
        
        let relation_type = if text_lower.contains("works for") || text_lower.contains("employed by") {
            RelationType::WorksFor
        } else if text_lower.contains("reports to") || text_lower.contains("manager") {
            RelationType::ReportsTo
        } else if text_lower.contains("uses") || text_lower.contains("utilizes") {
            RelationType::Uses
        } else if text_lower.contains("calls") || text_lower.contains("invokes") {
            RelationType::Calls
        } else if text_lower.contains("imports") || text_lower.contains("requires") {
            RelationType::Imports
        } else if text_lower.contains("located in") || text_lower.contains("based in") {
            RelationType::LocatedIn
        } else {
            return None;
        };
        
        Some(EntityRelation {
            source: entity1.id,
            target: entity2.id,
            relation_type,
            confidence: 0.8,
            evidence: text.to_string(),
        })
    }
    
    /// Helper functions
    
    fn detect_language(&self, text: &str) -> Language {
        match detect(text) {
            Some(info) => match info.lang() {
                Lang::Eng => Language::English,
                Lang::Hin => Language::Hindi,
                Lang::Ben => Language::Bengali,
                Lang::Tam => Language::Tamil,
                Lang::Tel => Language::Telugu,
                Lang::Mar => Language::Marathi,
                Lang::Guj => Language::Gujarati,
                Lang::Kan => Language::Kannada,
                Lang::Mal => Language::Malayalam,
                Lang::Pan => Language::Punjabi,
                Lang::Urd => Language::Urdu,
                Lang::Cmn => Language::Chinese,
                Lang::Jpn => Language::Japanese,
                Lang::Kor => Language::Korean,
                Lang::Spa => Language::Spanish,
                Lang::Fra => Language::French,
                Lang::Deu => Language::German,
                Lang::Rus => Language::Russian,
                Lang::Ara => Language::Arabic,
                _ => Language::Other,
            },
            None => Language::English,
        }
    }
    
    fn create_entity(
        &self,
        text: &str,
        entity_type: EntityType,
        start: usize,
        end: usize,
        language: Language,
    ) -> Entity {
        Entity {
            id: Uuid::new_v4(),
            text: text.to_string(),
            normalized_text: text.to_lowercase(),
            entity_type,
            confidence: 0.9,
            position: EntityPosition {
                start,
                end,
                line: None,
                column: None,
            },
            attributes: HashMap::new(),
            language,
        }
    }
    
    fn get_context(&self, text: &str, position: usize, window: usize) -> String {
        let start = position.saturating_sub(window);
        let end = (position + window).min(text.len());
        text[start..end].to_string()
    }
    
    fn is_person_context(&self, context: &str) -> bool {
        let person_indicators = [
            "Mr.", "Ms.", "Mrs.", "Dr.", "Prof.", "Sri", "Shri", "Smt.",
            "said", "told", "wrote", "created", "designed", "developed",
            "manager", "engineer", "developer", "designer", "analyst",
        ];
        
        person_indicators.iter().any(|indicator| context.contains(indicator))
    }
    
    fn normalize_entities(&self, mut entities: Vec<Entity>) -> Vec<Entity> {
        for entity in &mut entities {
            // Normalize based on type
            match entity.entity_type {
                EntityType::Email => {
                    entity.normalized_text = entity.text.to_lowercase();
                }
                EntityType::Person => {
                    // Title case for names
                    entity.normalized_text = self.title_case(&entity.text);
                }
                EntityType::Function | EntityType::Class => {
                    // Keep original casing for code
                    entity.normalized_text = entity.text.clone();
                }
                _ => {
                    entity.normalized_text = entity.text.to_lowercase();
                }
            }
        }
        
        entities
    }
    
    fn deduplicate_entities(&self, entities: Vec<Entity>) -> Vec<Entity> {
        let mut seen = HashSet::new();
        let mut deduped = Vec::new();
        
        for entity in entities {
            let key = (entity.normalized_text.clone(), entity.entity_type);
            if !seen.contains(&key) {
                seen.insert(key);
                deduped.push(entity);
            }
        }
        
        deduped
    }
    
    async fn validate_entities(&self, entities: Vec<Entity>) -> Result<Vec<Entity>> {
        // Validate with knowledge base
        let mut validated = Vec::new();
        
        for mut entity in entities {
            // Check if entity exists in knowledge base
            if self.knowledge_base.validate(&entity).await? {
                entity.confidence = (entity.confidence * 1.2).min(1.0);
            } else {
                entity.confidence *= 0.8;
            }
            
            // Only keep high confidence entities
            if entity.confidence > 0.5 {
                validated.push(entity);
            }
        }
        
        Ok(validated)
    }
    
    fn title_case(&self, s: &str) -> String {
        s.split_whitespace()
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    None => String::new(),
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Document type for extraction
#[derive(Debug, Clone)]
pub enum DocumentType {
    Code(String), // Programming language
    Document,
    Chat,
    Email,
    Mixed,
}

/// Extraction result
#[derive(Debug, Clone)]
pub struct ExtractedData {
    pub entities: Vec<Entity>,
    pub relations: Vec<EntityRelation>,
    pub language: Language,
}

/// Knowledge base for entity validation
pub struct KnowledgeBase {
    known_entities: DashMap<String, EntityType>,
    relationships: DashMap<(Uuid, Uuid), RelationType>,
}

impl KnowledgeBase {
    pub fn new() -> Self {
        Self {
            known_entities: DashMap::new(),
            relationships: DashMap::new(),
        }
    }
    
    pub async fn validate(&self, entity: &Entity) -> Result<bool> {
        // Check if entity is known
        Ok(self.known_entities.contains_key(&entity.normalized_text))
    }
    
    pub fn add_entity(&self, text: String, entity_type: EntityType) {
        self.known_entities.insert(text, entity_type);
    }
    
    pub fn add_relationship(&self, source: Uuid, target: Uuid, relation: RelationType) {
        self.relationships.insert((source, target), relation);
    }
}

impl Language {
    pub fn is_indian(&self) -> bool {
        matches!(self,
            Language::Hindi | Language::Bengali | Language::Tamil |
            Language::Telugu | Language::Marathi | Language::Gujarati |
            Language::Kannada | Language::Malayalam | Language::Punjabi |
            Language::Urdu
        )
    }
}
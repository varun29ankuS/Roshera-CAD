//! Indexed view over the parsed entity instances of a STEP file.
//!
//! ruststep gives us a [`Vec<EntityInstance>`] per data section. Real
//! STEP files use forward references freely — e.g. an `EDGE_LOOP`
//! refers to `ORIENTED_EDGE` instances that may appear later in the
//! file — so the dispatch phase needs random access by entity number.
//! The [`EntityRegistry`] is that index.
//!
//! It also flattens multi-section files: many STEP files contain only
//! a single `DATA;` section, but the spec allows multiple. We index
//! all of them into a single map keyed by `#N`; the spec guarantees
//! globally unique instance numbers per exchange structure
//! (ISO 10303-21 §6.5.2).
//!
//! Complex `&SCOPE` / `&ENDSCOPE` / SubSuperRecord instances are
//! preserved as-is; the dispatcher will surface them through a
//! separate code path when (and if) we add coverage for them. For
//! tier-1 geometry, every entity we care about is the simple form.

use ruststep::ast::{EntityInstance, Exchange, Record};
use std::collections::HashMap;

/// One indexed entry per `#N=...;` statement in the source file.
#[derive(Debug, Clone)]
pub struct IndexedEntity {
    /// `#N` instance number from the source file.
    pub instance: u64,
    /// The kind of entry. Tier-1 only consumes `Simple`; `Complex`
    /// passes through unhandled (logged as Unsupported by the
    /// dispatcher).
    pub kind: EntityKind,
}

/// Wraps the two `EntityInstance` shapes ruststep produces.
#[derive(Debug, Clone)]
pub enum EntityKind {
    /// `#N = NAME(...)` — the common case.
    Simple(Record),
    /// `#N = (NAME1(...) NAME2(...) ...)` — complex / inheritance.
    /// Stored as the list of constituent records so handlers can opt in.
    Complex(Vec<Record>),
}

impl EntityKind {
    /// Returns the primary entity name (the first record's name for
    /// complex entries). Empty string for malformed entries.
    pub fn primary_name(&self) -> &str {
        match self {
            EntityKind::Simple(rec) => rec.name.as_str(),
            EntityKind::Complex(recs) => recs
                .first()
                .map(|r| r.name.as_str())
                .unwrap_or(""),
        }
    }
}

/// `#N` → entity index.
#[derive(Debug, Default)]
pub struct EntityRegistry {
    by_id: HashMap<u64, IndexedEntity>,
}

impl EntityRegistry {
    /// Build an index from a parsed [`Exchange`]. Flattens all data
    /// sections into a single map.
    pub fn build(exchange: &Exchange) -> Self {
        let mut by_id: HashMap<u64, IndexedEntity> = HashMap::new();
        for section in &exchange.data {
            for inst in &section.entities {
                let entry = match inst {
                    EntityInstance::Simple { id, record } => IndexedEntity {
                        instance: *id,
                        kind: EntityKind::Simple(record.clone()),
                    },
                    EntityInstance::Complex { id, subsuper } => IndexedEntity {
                        instance: *id,
                        kind: EntityKind::Complex(subsuper.0.clone()),
                    },
                };
                by_id.insert(entry.instance, entry);
            }
        }
        Self { by_id }
    }

    /// Lookup by entity number.
    pub fn get(&self, instance: u64) -> Option<&IndexedEntity> {
        self.by_id.get(&instance)
    }

    /// Iterate every entry. Order is unspecified (HashMap-backed).
    pub fn iter(&self) -> impl Iterator<Item = &IndexedEntity> {
        self.by_id.values()
    }

    /// Number of indexed entities.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// `true` when no entities were indexed (empty file or all
    /// sections were empty).
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::step::parser::parse_step;

    fn parse_and_index(src: &str) -> EntityRegistry {
        let ex = parse_step(src, "test").expect("parse");
        EntityRegistry::build(&ex)
    }

    fn minimal_wrap(body: &str) -> String {
        format!(
            "ISO-10303-21;\n\
             HEADER;\n\
             FILE_DESCRIPTION(('t'),'2;1');\n\
             FILE_NAME('t.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
             FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\n\
             ENDSEC;\n\
             DATA;\n\
             {body}\n\
             ENDSEC;\n\
             END-ISO-10303-21;\n"
        )
    }

    #[test]
    fn indexes_simple_entity_by_id() {
        let src = minimal_wrap("#7=CARTESIAN_POINT('p',(1.,2.,3.));");
        let reg = parse_and_index(&src);
        let e = reg.get(7).expect("entity #7 must be present");
        assert_eq!(e.instance, 7);
        assert_eq!(e.kind.primary_name(), "CARTESIAN_POINT");
    }

    #[test]
    fn empty_data_section_yields_empty_registry() {
        let src = minimal_wrap("");
        let reg = parse_and_index(&src);
        assert!(reg.is_empty());
    }

    #[test]
    fn missing_id_returns_none() {
        let src = minimal_wrap("#1=CARTESIAN_POINT('',(0.,0.,0.));");
        let reg = parse_and_index(&src);
        assert!(reg.get(999).is_none());
    }
}

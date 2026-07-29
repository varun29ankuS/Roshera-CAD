//! Typed rule provenance — WHERE a DFM threshold comes from (spec §3.2.1).
//!
//! A DFM violation must tell an agent whether it broke a STANDARD or a
//! HEURISTIC. Free-text rationale cannot be reasoned about; this can.
//! `ShopPractice` is explicitly NON-authoritative and exists so
//! practice-derived thresholds (e.g. FDM's 45° overhang, 2× nozzle wall)
//! can be stated honestly instead of being dressed up as standards.
//!
//! Discipline (non-negotiable, mirrors `gdt/verify.rs`'s honest refusal
//! `"fit-class tolerance: ISO 286 grade table not yet resolved"`): NEVER
//! write an edition year or clause number from memory. Confirm against
//! the actual current edition before it enters source; if unconfirmed,
//! downgrade to [`RuleProvenance::ShopPractice`] or
//! [`RuleProvenance::Handbook`] — an honest downgrade beats a fabricated
//! citation. The in-tree bar is `gdt/model.rs`, which cites
//! ASME Y14.5-2018 and ISO 1101:2017 Table 1 with clause precision.

use serde::{Deserialize, Serialize};

/// The standards body behind a [`RuleProvenance::Standard`] citation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StandardBody {
    Asme,
    Iso,
    IsoAstm,
    Astm,
    Din,
}

/// Where a rule's threshold comes from — carried on every
/// [`crate::dfm::RuleVerdict`] so a report reader (human or agent) can
/// weigh a violation correctly: breaking a published standard and
/// breaking a shop heuristic are different findings.
///
/// Owned `String` fields (not `&'static str`): serde's derived
/// `Deserialize` requires ownership for arbitrary wire input — the same
/// concession `report.rs`'s types already made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuleProvenance {
    /// A published standard governs this threshold. Cite precisely —
    /// and only what has been confirmed against the actual edition.
    Standard {
        body: StandardBody,
        designation: String,
        edition: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clause: Option<String>,
    },
    /// Academic/industry handbook lineage (e.g. Boothroyd & Dewhurst,
    /// *Product Design for Manufacture and Assembly*).
    Handbook { citation: String },
    /// A material/vendor datasheet value; the source must be named.
    MaterialDatasheet { source: String },
    /// Widely-used shop practice with NO governing standard. Explicitly
    /// non-authoritative — the note should say so rather than implying
    /// authority the threshold does not have.
    ShopPractice { note: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip through serde for the two variants the FDM/molding
    /// packs will actually mint first: a confirmed standard citation and
    /// an honestly-non-authoritative shop practice.
    #[test]
    fn provenance_serde_round_trips_standard_and_shop_practice() {
        let standard = RuleProvenance::Standard {
            body: StandardBody::Iso,
            designation: "ISO 286".to_string(),
            edition: "unresolved".to_string(),
            clause: None,
        };
        let practice = RuleProvenance::ShopPractice {
            note: "45° overhang; practice-derived, no governing standard".to_string(),
        };

        for original in [standard, practice] {
            let json = serde_json::to_string(&original)
                .unwrap_or_else(|e| panic!("serialize failed: {e}"));
            let back: RuleProvenance =
                serde_json::from_str(&json).unwrap_or_else(|e| panic!("deserialize failed: {e}"));
            assert_eq!(back, original, "round-trip must be lossless");
        }
    }

    /// The wire shape is internally tagged like the rest of the dfm
    /// module — pin the tag so TS/MCP consumers can rely on it.
    #[test]
    fn provenance_wire_shape_is_internally_tagged() {
        let practice = RuleProvenance::ShopPractice {
            note: "2x nozzle wall".to_string(),
        };
        let json =
            serde_json::to_string(&practice).unwrap_or_else(|e| panic!("serialize failed: {e}"));
        assert!(
            json.contains("\"kind\":\"shop_practice\""),
            "expected internally-tagged snake_case wire shape, got: {json}"
        );
    }
}

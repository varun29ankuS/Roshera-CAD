//! STEP product structure: parts, assembly occurrences, and placements.
//!
//! Every mainstream exporter (SolidWorks, Fusion, Creo, NX, OCCT) writes an
//! assembly as a product tree, not as geometry:
//!
//! ```text
//! PRODUCT ← PRODUCT_DEFINITION_FORMATION ← PRODUCT_DEFINITION (PD)
//! PD ← PRODUCT_DEFINITION_SHAPE (PDS) ← SHAPE_DEFINITION_REPRESENTATION → rep
//! NEXT_ASSEMBLY_USAGE_OCCURRENCE(parent PD, child PD) ← PDS
//!   ← CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(
//!        (REPRESENTATION_RELATIONSHIP(rep_1, rep_2)
//!         REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#idt)
//!         SHAPE_REPRESENTATION_RELATIONSHIP()), that PDS)
//! ITEM_DEFINED_TRANSFORMATION(name, description, item_1, item_2)
//! ```
//!
//! These handlers only READ that structure into
//! [`ProductStructure`](crate::formats::step::context::ProductStructure); they
//! touch no geometry. Placement happens after dispatch, once every part's
//! solids exist, in [`crate::formats::step::assembly_placement`]. A record
//! whose fields are malformed fails with a warning naming the field, so a
//! broken product tree is reported, never silently flattened.

use ruststep::ast::{Parameter, Record};

use crate::formats::step::{
    context::{ImportContext, Occurrence},
    dispatch::{EntityDispatch, EntityHandler, HandlerOutcome, Phase},
    registry::{EntityKind, EntityRegistry},
};

use super::super::tier1::params;

/// Which product-structure entity a handler instance reads.
#[derive(Clone, Copy)]
enum Kind {
    Product,
    Formation,
    Definition,
    DefinitionShape,
    ShapeDefinitionRepresentation,
    Occurrence,
    ItemDefinedTransformation,
    ContextDependentShapeRepresentation,
    RepresentationRelationship,
}

/// One handler per entity family; `Kind` selects the field layout.
pub struct ProductStructureHandler {
    names: &'static [&'static str],
    kind: Kind,
}

/// Reads `PRODUCT` names.
pub static PRODUCT_HANDLER: ProductStructureHandler = ProductStructureHandler {
    names: &["PRODUCT"],
    kind: Kind::Product,
};
/// Reads `PRODUCT_DEFINITION_FORMATION` → `PRODUCT` links.
pub static FORMATION_HANDLER: ProductStructureHandler = ProductStructureHandler {
    names: &[
        "PRODUCT_DEFINITION_FORMATION",
        "PRODUCT_DEFINITION_FORMATION_WITH_SPECIFIED_SOURCE",
    ],
    kind: Kind::Formation,
};
/// Reads `PRODUCT_DEFINITION` → formation links.
pub static DEFINITION_HANDLER: ProductStructureHandler = ProductStructureHandler {
    names: &[
        "PRODUCT_DEFINITION",
        "PRODUCT_DEFINITION_WITH_ASSOCIATED_DOCUMENTS",
    ],
    kind: Kind::Definition,
};
/// Reads `PRODUCT_DEFINITION_SHAPE` → definition links.
pub static DEFINITION_SHAPE_HANDLER: ProductStructureHandler = ProductStructureHandler {
    names: &["PRODUCT_DEFINITION_SHAPE"],
    kind: Kind::DefinitionShape,
};
/// Reads `SHAPE_DEFINITION_REPRESENTATION` (shape → representation).
pub static SHAPE_DEFINITION_REPRESENTATION_HANDLER: ProductStructureHandler =
    ProductStructureHandler {
        names: &["SHAPE_DEFINITION_REPRESENTATION"],
        kind: Kind::ShapeDefinitionRepresentation,
    };
/// Reads `NEXT_ASSEMBLY_USAGE_OCCURRENCE` (one use of a component).
pub static OCCURRENCE_HANDLER: ProductStructureHandler = ProductStructureHandler {
    names: &["NEXT_ASSEMBLY_USAGE_OCCURRENCE"],
    kind: Kind::Occurrence,
};
/// Reads `ITEM_DEFINED_TRANSFORMATION` frame pairs.
pub static ITEM_DEFINED_TRANSFORMATION_HANDLER: ProductStructureHandler = ProductStructureHandler {
    names: &["ITEM_DEFINED_TRANSFORMATION"],
    kind: Kind::ItemDefinedTransformation,
};
/// Reads `CONTEXT_DEPENDENT_SHAPE_REPRESENTATION` (an occurrence's placement).
pub static CONTEXT_DEPENDENT_SHAPE_REPRESENTATION_HANDLER: ProductStructureHandler =
    ProductStructureHandler {
        names: &["CONTEXT_DEPENDENT_SHAPE_REPRESENTATION"],
        kind: Kind::ContextDependentShapeRepresentation,
    };

/// Reads plain `SHAPE_REPRESENTATION_RELATIONSHIP` / `REPRESENTATION_RELATIONSHIP`
/// links (two representations of one shape). The complex form carrying
/// `REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION` is an occurrence's
/// placement instead; the placement pass reads it whole from the registry.
pub static REPRESENTATION_RELATIONSHIP_HANDLER: ProductStructureHandler = ProductStructureHandler {
    names: &[
        "SHAPE_REPRESENTATION_RELATIONSHIP",
        "REPRESENTATION_RELATIONSHIP",
    ],
    kind: Kind::RepresentationRelationship,
};

impl EntityHandler for ProductStructureHandler {
    fn names(&self) -> &'static [&'static str] {
        self.names
    }
    fn phase(&self) -> Phase {
        // No geometry dependencies; Root keeps it after the solids exist so
        // the placement pass sees a complete picture either way.
        Phase::Root
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        _dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        if matches!(self.kind, Kind::RepresentationRelationship)
            && carries_transformation(instance, registry)
        {
            // A placement, consumed by `assembly_placement` via the CDSR that
            // references it — not a same-shape link.
            return HandlerOutcome::Resolved;
        }
        match read(self.kind, instance, record, ctx) {
            Ok(()) => HandlerOutcome::Resolved,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                HandlerOutcome::Failed {
                    message: format!("malformed {}", record.name),
                }
            }
        }
    }
}

fn read(
    kind: Kind,
    instance: u64,
    record: &Record,
    ctx: &mut ImportContext<'_>,
) -> Result<(), params::ParamError> {
    let entity = record.name.as_str();
    let fields = params::record_fields(&record.parameter, entity, instance)?;
    let reference = |index: usize, path: &str| -> Result<u64, params::ParamError> {
        let field = params::field(fields, index, entity, instance)?;
        params::as_entity_ref(field, entity, instance, path)
    };
    let text = |index: usize| -> String {
        match fields.get(index) {
            Some(Parameter::String(s)) => s.clone(),
            _ => String::new(),
        }
    };
    let product = &mut ctx.caches.product;
    match kind {
        Kind::Product => {
            // PRODUCT(id, name, description, frame_of_reference).
            let (id, name) = (text(0), text(1));
            let display = if name.trim().is_empty() { id } else { name };
            product.products.insert(instance, display);
        }
        Kind::Formation => {
            // PRODUCT_DEFINITION_FORMATION(id, description, of_product[, make_or_buy]).
            let of_product = reference(2, "of_product")?;
            product.formations.insert(instance, of_product);
        }
        Kind::Definition => {
            // PRODUCT_DEFINITION(id, description, formation, frame_of_reference[, docs]).
            let formation = reference(2, "formation")?;
            product.definitions.insert(instance, formation);
        }
        Kind::DefinitionShape => {
            // PRODUCT_DEFINITION_SHAPE(name, description, definition).
            let definition = reference(2, "definition")?;
            product.definition_shapes.insert(instance, definition);
        }
        Kind::ShapeDefinitionRepresentation => {
            // SHAPE_DEFINITION_REPRESENTATION(definition, used_representation).
            let pds = reference(0, "definition")?;
            let rep = reference(1, "used_representation")?;
            product.shape_representations.push((pds, rep));
        }
        Kind::Occurrence => {
            // NEXT_ASSEMBLY_USAGE_OCCURRENCE(id, name, description,
            //   relating_product_definition, related_product_definition,
            //   reference_designator).
            let parent = reference(3, "relating_product_definition")?;
            let child = reference(4, "related_product_definition")?;
            product.occurrences.insert(
                instance,
                Occurrence {
                    id: text(0),
                    name: text(1),
                    parent,
                    child,
                },
            );
        }
        Kind::ItemDefinedTransformation => {
            // ITEM_DEFINED_TRANSFORMATION(name, description, item_1, item_2).
            let item_1 = reference(2, "transform_item_1")?;
            let item_2 = reference(3, "transform_item_2")?;
            product.transformations.insert(instance, (item_1, item_2));
        }
        Kind::ContextDependentShapeRepresentation => {
            // CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(representation_relation,
            //   represented_product_relation).
            let relation = reference(0, "representation_relation")?;
            let pds = reference(1, "represented_product_relation")?;
            product.placements.push((relation, pds));
        }
        Kind::RepresentationRelationship => {
            // (SHAPE_)REPRESENTATION_RELATIONSHIP(name, description, rep_1, rep_2).
            let rep_1 = reference(2, "rep_1")?;
            let rep_2 = reference(3, "rep_2")?;
            product.rep_links.push((rep_1, rep_2));
        }
    }
    Ok(())
}

/// `true` when `instance` is a complex relationship that includes
/// `REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION`.
fn carries_transformation(instance: u64, registry: &EntityRegistry) -> bool {
    match registry.get(instance).map(|e| &e.kind) {
        Some(EntityKind::Complex(records)) => records.iter().any(|r| {
            r.name
                .eq_ignore_ascii_case("REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION")
        }),
        _ => false,
    }
}

/// Register every product-structure handler.
pub fn register(dispatch: &mut EntityDispatch) {
    dispatch.register(&PRODUCT_HANDLER);
    dispatch.register(&FORMATION_HANDLER);
    dispatch.register(&DEFINITION_HANDLER);
    dispatch.register(&DEFINITION_SHAPE_HANDLER);
    dispatch.register(&SHAPE_DEFINITION_REPRESENTATION_HANDLER);
    dispatch.register(&OCCURRENCE_HANDLER);
    dispatch.register(&ITEM_DEFINED_TRANSFORMATION_HANDLER);
    dispatch.register(&CONTEXT_DEPENDENT_SHAPE_REPRESENTATION_HANDLER);
    dispatch.register(&REPRESENTATION_RELATIONSHIP_HANDLER);
}

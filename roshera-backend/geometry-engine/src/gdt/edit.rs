//! GD&T edits as recorded design events (founder decision 2026-11-13: "Datums
//! and GD&T should be changeable by the user too, and drawing should adapt to
//! it").
//!
//! Every change to a part's datum features or tolerance annotations goes
//! through one mediator here, which **validates, then mutates, then records** —
//! and records nothing when it refuses, so a refusal changes nothing. The
//! timeline replays the same mediators (`timeline-engine` `replay.rs`, the
//! `gdt_*` arms), so undo, redo, restart and every sheet rebuild re-derive the
//! part's GD&T from its history exactly the way they re-derive its geometry.
//!
//! ## Event kinds
//!
//! | kind                     | records                                         |
//! |--------------------------|-------------------------------------------------|
//! | `gdt_datum_designate`    | `solid_id, label, feature_pid, kind, face_id`   |
//! | `gdt_datum_update`       | `solid_id, label, new_label, feature_pid, kind` |
//! | `gdt_datum_delete`       | `solid_id, label`                               |
//! | `gdt_annotation_create`  | `solid_id, annotation_id, feature_pid, annotation` |
//! | `gdt_annotation_update`  | `solid_id, annotation_id, feature_pid, annotation` |
//! | `gdt_annotation_delete`  | `solid_id, annotation_id`                       |
//!
//! Each payload is self-contained (the full new state, not a delta), and names
//! its feature by **persistent id** — the identity that follows a face through
//! later features. The kinds are underscore-cased like every model-state event
//! (`datum_create`, `fillet_edges`): GD&T lives on the [`BRepModel`] (its
//! `drf` and `gdt` sidecars), so it must be rebuilt by every replay of the
//! model, including the one each sheet is derived from.
//!
//! ## Following the part
//!
//! Annotations are keyed by the feature's persistent id, so a face a later
//! operation keeps keeps its tolerances. A part's datum reference frame and
//! annotation ownership are keyed by solid, and an operation that consumes a
//! solid and makes a new one (a boolean, a shell) hands them on with
//! [`BRepModel::hand_on_gdt`]. A face that does not survive leaves its datum
//! or annotation **dangling** — still listed, still owned by the part,
//! resolved to no geometry — never dropped, never re-attached to a guess.
//!
//! ## Validation (ASME Y14.5 basics, the rules this kernel models)
//!
//! [`validate_frame`] refuses, with a typed [`GdtEditError`]:
//! * a tolerance that is not a positive, finite size;
//! * datum references on a form characteristic ([`crate::gdt::model::GeometricCharacteristic::is_form`]
//!   — flatness, straightness, circularity, cylindricity are datum-free);
//! * no datum reference on a characteristic that needs one
//!   ([`crate::gdt::model::GeometricCharacteristic::needs_datum`]);
//! * more than three datum references (primary, secondary, tertiary — the
//!   precedence order `DatumRef` documents);
//! * the same datum letter twice in one frame;
//! * a datum letter no datum feature on the part carries.
//!
//! Datum letters are unique per part ([`GdtError::DuplicateLabel`]), and a
//! datum a frame references cannot be deleted or re-lettered
//! ([`GdtError::DatumReferenced`]) — the frame would silently lose its
//! reference.

use serde_json::{json, Value};
use thiserror::Error;

use crate::gdt::drf::{datum_feature_of, solid_face_ids, GdtError};
use crate::gdt::model::{
    Annotation, Datum, DatumKind, DimensionalTolerance, FeatureControlFrame, ToleranceBound,
};
use crate::gdt::sidecar::AnnotationHome;
use crate::operations::recorder::RecordedOperation;
use crate::primitives::face::FaceId;
use crate::primitives::persistent_id::PersistentId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

/// The most datum references a feature control frame takes: primary,
/// secondary, tertiary.
pub const MAX_DATUM_REFS: usize = 3;

/// Typed refusals of a GD&T annotation edit. Each carries what a user or an
/// agent needs to fix the request.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum GdtEditError {
    /// A datum-level refusal (unknown part, face not on the part, …).
    #[error(transparent)]
    Datum(#[from] GdtError),

    /// The tolerance zone is not a positive, finite size.
    #[error("a tolerance must be a positive, finite size; {value} is not")]
    NonPositiveTolerance { value: f64 },

    /// A form characteristic carries datum references.
    #[error(
        "{characteristic} is a form tolerance and takes no datum references; remove {labels:?}"
    )]
    DatumRefsNotAllowed {
        characteristic: &'static str,
        labels: Vec<String>,
    },

    /// A characteristic that controls a feature relative to a datum has none.
    #[error("{characteristic} controls a feature relative to a datum and needs at least one datum reference")]
    DatumRefsRequired { characteristic: &'static str },

    /// More than primary / secondary / tertiary.
    #[error("a frame takes at most {MAX_DATUM_REFS} datum references (primary, secondary, tertiary); {count} were given")]
    TooManyDatumRefs { count: usize },

    /// One datum letter named twice in the same frame.
    #[error("datum '{label}' is referenced more than once in the frame; each reference names a different datum")]
    RepeatedDatumRef { label: String },

    /// A datum letter no datum feature on the part carries.
    #[error("datum '{label}' is not designated on part {solid}; designate it first")]
    DatumNotDesignated { solid: SolidId, label: String },

    /// A basic dimension is not finite.
    #[error("basic dimensions must be finite numbers; {basic:?} is not")]
    NonFiniteBasic { basic: [f64; 2] },

    /// A size tolerance whose numbers cannot bound a size.
    #[error("the size tolerance cannot bound a size: {reason}")]
    InvalidSizeTolerance { reason: String },

    /// No annotation carries this id.
    #[error("annotation '{id}' does not exist")]
    UnknownAnnotation { id: String },

    /// The id is already in use by another annotation.
    #[error("annotation id '{id}' is already in use")]
    DuplicateAnnotationId { id: String },

    /// The annotation belongs to a different part.
    #[error("annotation '{id}' belongs to part {owner}, not part {solid}")]
    AnnotationNotOnPart {
        id: String,
        owner: SolidId,
        solid: SolidId,
    },

    /// An edit would change what kind of annotation this is.
    #[error("annotation '{id}' is a {found} tolerance; it cannot be edited into a {expected} one")]
    KindMismatch {
        id: String,
        expected: &'static str,
        found: &'static str,
    },
}

/// The feature an edit attaches to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FeatureTarget {
    /// A live face, validated against the part (membership, persistent id,
    /// and — for a datum — the surface kind).
    Face(FaceId),
    /// A recorded feature whose face does not resolve at this point of the
    /// history. Replay uses it so a recorded decision on a face a later
    /// upstream edit removed is kept, DANGLING, instead of being dropped.
    /// `kind` is the datum kind the designation recorded (annotations ignore
    /// it).
    Recorded {
        feature: PersistentId,
        kind: Option<DatumKind>,
    },
}

/// Hex form of a persistent id, as every GD&T wire shape and event carries it.
pub fn pid_hex(pid: PersistentId) -> String {
    format!("{:032x}", pid.as_u128())
}

/// Parse [`pid_hex`] back. `None` for malformed input — never a fabricated id.
pub fn parse_pid_hex(hex: &str) -> Option<PersistentId> {
    u128::from_str_radix(hex.trim(), 16).ok().map(PersistentId)
}

/// Lower-case name of a datum kind, as recorded.
pub fn datum_kind_name(kind: DatumKind) -> &'static str {
    match kind {
        DatumKind::Plane => "plane",
        DatumKind::Axis => "axis",
        DatumKind::Point => "point",
    }
}

/// Parse [`datum_kind_name`].
pub fn parse_datum_kind(name: &str) -> Option<DatumKind> {
    match name {
        "plane" => Some(DatumKind::Plane),
        "axis" => Some(DatumKind::Axis),
        "point" => Some(DatumKind::Point),
        _ => None,
    }
}

/// Validate a feature control frame against the part's datum reference frame.
/// See the module docs for the rules.
pub fn validate_frame(
    model: &BRepModel,
    solid: SolidId,
    fcf: &FeatureControlFrame,
) -> Result<(), GdtEditError> {
    let value = fcf.tolerance_value;
    if !(value.is_finite() && value > 0.0) {
        return Err(GdtEditError::NonPositiveTolerance { value });
    }
    let characteristic = fcf.characteristic;
    let labels: Vec<String> = fcf.datum_refs.iter().map(|d| d.label.clone()).collect();
    if characteristic.is_form() && !labels.is_empty() {
        return Err(GdtEditError::DatumRefsNotAllowed {
            characteristic: characteristic.mnemonic(),
            labels,
        });
    }
    if characteristic.needs_datum() && labels.is_empty() {
        return Err(GdtEditError::DatumRefsRequired {
            characteristic: characteristic.mnemonic(),
        });
    }
    if labels.len() > MAX_DATUM_REFS {
        return Err(GdtEditError::TooManyDatumRefs {
            count: labels.len(),
        });
    }
    for (i, label) in labels.iter().enumerate() {
        if labels[..i].contains(label) {
            return Err(GdtEditError::RepeatedDatumRef {
                label: label.clone(),
            });
        }
    }
    let frame = model.drf.get(&solid);
    for label in &labels {
        if frame.and_then(|f| f.datum_by_label(label)).is_none() {
            return Err(GdtEditError::DatumNotDesignated {
                solid,
                label: label.clone(),
            });
        }
    }
    if let Some(basic) = fcf.basic {
        if !basic.iter().all(|v| v.is_finite()) {
            return Err(GdtEditError::NonFiniteBasic { basic });
        }
    }
    Ok(())
}

/// Validate a size tolerance: finite numbers, non-negative deviations, limits
/// in order, a named fit.
pub fn validate_size(tolerance: &DimensionalTolerance) -> Result<(), GdtEditError> {
    let bad = |reason: &str| {
        Err(GdtEditError::InvalidSizeTolerance {
            reason: reason.to_string(),
        })
    };
    if !tolerance.nominal.is_finite() {
        return bad("the nominal size is not a finite number");
    }
    match &tolerance.bound {
        ToleranceBound::PlusMinus { plus, minus } => {
            if !(plus.is_finite() && minus.is_finite() && *plus >= 0.0 && *minus >= 0.0) {
                return bad("plus and minus must be finite, non-negative deviations");
            }
        }
        ToleranceBound::Limits { upper, lower } => {
            if !(upper.is_finite() && lower.is_finite()) {
                return bad("the limits must be finite numbers");
            }
            if lower > upper {
                return bad("the lower limit is above the upper limit");
            }
        }
        ToleranceBound::Fit(fit) => {
            if fit.designation.trim().is_empty() {
                return bad("the fit designation is empty");
            }
        }
    }
    Ok(())
}

/// Validate any annotation for `solid`.
pub fn validate_annotation(
    model: &BRepModel,
    solid: SolidId,
    annotation: &Annotation,
) -> Result<(), GdtEditError> {
    match annotation {
        Annotation::Geometric(fcf) => validate_frame(model, solid, fcf),
        Annotation::Dimensional(dt) => validate_size(dt),
    }
}

/// Datum letters both solids' frames carry — the letters a boolean joining
/// the two parts could not keep apart. Sorted.
pub fn datum_letter_conflicts(model: &BRepModel, a: SolidId, b: SolidId) -> Vec<String> {
    let (Some(fa), Some(fb)) = (model.drf.get(&a), model.drf.get(&b)) else {
        return Vec::new();
    };
    let mut out: Vec<String> = fa
        .datums
        .iter()
        .filter(|d| fb.datum_by_label(&d.label).is_some())
        .map(|d| d.label.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

fn annotation_kind(annotation: &Annotation) -> &'static str {
    match annotation {
        Annotation::Geometric(_) => "geometric",
        Annotation::Dimensional(_) => "dimensional",
    }
}

fn check_label(label: &str) -> Result<(), GdtError> {
    if label.trim().is_empty() {
        return Err(GdtError::EmptyLabel);
    }
    Ok(())
}

fn require_solid(model: &BRepModel, solid: SolidId) -> Result<(), GdtError> {
    if model.solids.get(solid).is_none() {
        return Err(GdtError::UnknownSolid { solid });
    }
    Ok(())
}

impl BRepModel {
    /// The annotations owned by `solid` (identified ones, plus legacy
    /// unidentified ones on its live faces) whose frame references datum
    /// `label`. Identified ones are named by id, legacy ones by
    /// `<feature pid>#<index>`.
    pub fn gdt_frames_referencing(&self, solid: SolidId, label: &str) -> Vec<String> {
        let refs = |a: &Annotation| match a {
            Annotation::Geometric(fcf) => fcf.datum_refs.iter().any(|d| d.label == label),
            Annotation::Dimensional(_) => false,
        };
        let mut out: Vec<String> = self
            .gdt
            .identified_of(solid)
            .into_iter()
            .filter(|(id, _)| self.gdt.identified(id).is_some_and(|(_, a)| refs(a)))
            .map(|(id, _)| id)
            .collect();
        let faces = solid_face_ids(self, solid);
        for (feature, list) in self.gdt.iter() {
            let on_part = self
                .face_by_pid(feature)
                .is_some_and(|f| faces.contains(&f));
            if !on_part {
                continue;
            }
            for (i, a) in list.iter().enumerate() {
                if self.gdt.annotation_id(feature, i).is_none() && refs(a) {
                    out.push(format!("{}#{i}", pid_hex(feature)));
                }
            }
        }
        out.sort();
        out
    }

    /// Resolve a datum target to `(feature, kind, live face)`.
    fn datum_target(
        &self,
        solid: SolidId,
        target: FeatureTarget,
    ) -> Result<(PersistentId, DatumKind, Option<FaceId>), GdtError> {
        match target {
            FeatureTarget::Face(face) => {
                let (pid, kind) = datum_feature_of(self, solid, face)?;
                Ok((pid, kind, Some(face)))
            }
            FeatureTarget::Recorded { feature, kind } => {
                require_solid(self, solid)?;
                let kind = kind.ok_or_else(|| GdtError::UnsupportedSurfaceKind {
                    kind: "unrecorded".to_string(),
                })?;
                Ok((feature, kind, None))
            }
        }
    }

    /// Resolve an annotation target to `(feature, live face)`: a live face
    /// must belong to the part and carry a persistent id.
    fn annotation_target(
        &self,
        solid: SolidId,
        target: FeatureTarget,
    ) -> Result<(PersistentId, Option<FaceId>), GdtError> {
        require_solid(self, solid)?;
        match target {
            FeatureTarget::Face(face) => {
                if !solid_face_ids(self, solid).contains(&face) {
                    return Err(GdtError::FaceNotInSolid { solid, face });
                }
                let pid = self
                    .face_pid(face)
                    .ok_or(GdtError::FaceHasNoPersistentId { face })?;
                Ok((pid, Some(face)))
            }
            FeatureTarget::Recorded { feature, .. } => Ok((feature, None)),
        }
    }

    /// Designate a datum feature on `solid` and record `gdt_datum_designate`.
    pub fn gdt_designate_datum(
        &mut self,
        solid: SolidId,
        label: &str,
        target: FeatureTarget,
    ) -> Result<Datum, GdtError> {
        check_label(label)?;
        let (feature, kind, face) = self.datum_target(solid, target)?;
        if self
            .drf
            .get(&solid)
            .is_some_and(|f| f.datum_by_label(label).is_some())
        {
            return Err(GdtError::DuplicateLabel {
                solid,
                label: label.to_string(),
            });
        }
        let datum = Datum::new(label, kind, feature);
        self.drf
            .entry(solid)
            .or_default()
            .datums
            .push(datum.clone());
        self.record_operation(
            RecordedOperation::new("gdt_datum_designate")
                .with_parameters(json!({
                    "solid_id": solid,
                    "label": label,
                    "feature_pid": pid_hex(feature),
                    "kind": datum_kind_name(kind),
                    "face_id": face,
                }))
                .with_input_faces(face.map(u64::from)),
        );
        Ok(datum)
    }

    /// Re-letter a datum and/or move it to another feature, keeping its
    /// precedence, and record `gdt_datum_update`. A datum a frame references
    /// cannot be re-lettered ([`GdtError::DatumReferenced`]).
    pub fn gdt_update_datum(
        &mut self,
        solid: SolidId,
        label: &str,
        new_label: Option<&str>,
        target: Option<FeatureTarget>,
    ) -> Result<Datum, GdtError> {
        require_solid(self, solid)?;
        let current = self
            .drf
            .get(&solid)
            .and_then(|f| f.datum_by_label(label))
            .cloned()
            .ok_or_else(|| GdtError::UnknownDatum {
                solid,
                label: label.to_string(),
            })?;
        let new_label = new_label.unwrap_or(label);
        check_label(new_label)?;
        if new_label != label {
            if self
                .drf
                .get(&solid)
                .is_some_and(|f| f.datum_by_label(new_label).is_some())
            {
                return Err(GdtError::DuplicateLabel {
                    solid,
                    label: new_label.to_string(),
                });
            }
            let frames = self.gdt_frames_referencing(solid, label);
            if !frames.is_empty() {
                return Err(GdtError::DatumReferenced {
                    solid,
                    label: label.to_string(),
                    annotations: frames,
                });
            }
        }
        let (feature, kind, face) = match target {
            Some(t) => self.datum_target(solid, t)?,
            None => (
                current.feature,
                current.kind,
                self.face_by_pid(current.feature),
            ),
        };
        let updated = Datum::new(new_label, kind, feature);
        if let Some(slot) = self
            .drf
            .get_mut(&solid)
            .and_then(|f| f.datums.iter_mut().find(|d| d.label == label))
        {
            *slot = updated.clone();
        }
        self.record_operation(
            RecordedOperation::new("gdt_datum_update")
                .with_parameters(json!({
                    "solid_id": solid,
                    "label": label,
                    "new_label": new_label,
                    "feature_pid": pid_hex(feature),
                    "kind": datum_kind_name(kind),
                    "face_id": face,
                }))
                .with_input_faces(face.map(u64::from)),
        );
        Ok(updated)
    }

    /// Delete a datum feature and record `gdt_datum_delete`. Refused while a
    /// frame references it ([`GdtError::DatumReferenced`]).
    pub fn gdt_delete_datum(&mut self, solid: SolidId, label: &str) -> Result<Datum, GdtError> {
        require_solid(self, solid)?;
        let current = self
            .drf
            .get(&solid)
            .and_then(|f| f.datum_by_label(label))
            .cloned()
            .ok_or_else(|| GdtError::UnknownDatum {
                solid,
                label: label.to_string(),
            })?;
        let frames = self.gdt_frames_referencing(solid, label);
        if !frames.is_empty() {
            return Err(GdtError::DatumReferenced {
                solid,
                label: label.to_string(),
                annotations: frames,
            });
        }
        if let Some(frame) = self.drf.get_mut(&solid) {
            frame.datums.retain(|d| d.label != label);
        }
        let face = self.face_by_pid(current.feature);
        self.record_operation(
            RecordedOperation::new("gdt_datum_delete")
                .with_parameters(json!({
                    "solid_id": solid,
                    "label": label,
                }))
                .with_input_faces(face.map(u64::from)),
        );
        Ok(current)
    }

    /// Attach a new identified annotation to a feature of `solid` and record
    /// `gdt_annotation_create`. Returns the feature's persistent id.
    pub fn gdt_create_annotation(
        &mut self,
        solid: SolidId,
        id: &str,
        target: FeatureTarget,
        annotation: Annotation,
    ) -> Result<PersistentId, GdtEditError> {
        if id.trim().is_empty() || self.gdt.identified(id).is_some() {
            return Err(GdtEditError::DuplicateAnnotationId { id: id.to_string() });
        }
        let (feature, face) = self.annotation_target(solid, target)?;
        validate_annotation(self, solid, &annotation)?;
        let recorded = serde_json::to_value(&annotation).unwrap_or(Value::Null);
        if !self.gdt.attach_identified(id, solid, feature, annotation) {
            return Err(GdtEditError::DuplicateAnnotationId { id: id.to_string() });
        }
        self.record_operation(
            RecordedOperation::new("gdt_annotation_create")
                .with_parameters(json!({
                    "solid_id": solid,
                    "annotation_id": id,
                    "feature_pid": pid_hex(feature),
                    "face_id": face,
                    "annotation": recorded,
                }))
                .with_input_faces(face.map(u64::from)),
        );
        Ok(feature)
    }

    /// The identified annotation `id` owned by `solid`.
    fn owned_annotation(
        &self,
        solid: SolidId,
        id: &str,
    ) -> Result<(AnnotationHome, Annotation), GdtEditError> {
        let (home, annotation) = self
            .gdt
            .identified(id)
            .ok_or_else(|| GdtEditError::UnknownAnnotation { id: id.to_string() })?;
        if home.solid != solid {
            return Err(GdtEditError::AnnotationNotOnPart {
                id: id.to_string(),
                owner: home.solid,
                solid,
            });
        }
        Ok((home, annotation.clone()))
    }

    /// Replace identified annotation `id` (optionally moving it to another
    /// feature) and record `gdt_annotation_update` with its full new state.
    /// The annotation keeps its kind (a frame stays a frame).
    pub fn gdt_update_annotation(
        &mut self,
        solid: SolidId,
        id: &str,
        target: Option<FeatureTarget>,
        annotation: Annotation,
    ) -> Result<PersistentId, GdtEditError> {
        let (home, previous) = self.owned_annotation(solid, id)?;
        if annotation_kind(&previous) != annotation_kind(&annotation) {
            return Err(GdtEditError::KindMismatch {
                id: id.to_string(),
                expected: annotation_kind(&annotation),
                found: annotation_kind(&previous),
            });
        }
        let (feature, face) = match target {
            Some(t) => self.annotation_target(solid, t)?,
            None => (home.feature, self.face_by_pid(home.feature)),
        };
        validate_annotation(self, solid, &annotation)?;
        let recorded = serde_json::to_value(&annotation).unwrap_or(Value::Null);
        self.gdt.remove_identified(id);
        self.gdt.attach_identified(id, solid, feature, annotation);
        self.record_operation(
            RecordedOperation::new("gdt_annotation_update")
                .with_parameters(json!({
                    "solid_id": solid,
                    "annotation_id": id,
                    "feature_pid": pid_hex(feature),
                    "face_id": face,
                    "annotation": recorded,
                }))
                .with_input_faces(face.map(u64::from)),
        );
        Ok(feature)
    }

    /// Delete identified annotation `id` and record `gdt_annotation_delete`.
    pub fn gdt_delete_annotation(
        &mut self,
        solid: SolidId,
        id: &str,
    ) -> Result<Annotation, GdtEditError> {
        let (home, _) = self.owned_annotation(solid, id)?;
        let face = self.face_by_pid(home.feature);
        let (_, removed) = self
            .gdt
            .remove_identified(id)
            .ok_or_else(|| GdtEditError::UnknownAnnotation { id: id.to_string() })?;
        self.record_operation(
            RecordedOperation::new("gdt_annotation_delete")
                .with_parameters(json!({
                    "solid_id": solid,
                    "annotation_id": id,
                }))
                .with_input_faces(face.map(u64::from)),
        );
        Ok(removed)
    }

    /// Hand the GD&T of every `consumed` solid on to `result`: the datum
    /// reference frames are merged in operand order (so the first operand's
    /// datums keep precedence) and annotation ownership moves. Called by an
    /// operation that consumes its operand solids and makes a new one, after
    /// [`datum_letter_conflicts`] has refused a merge that would put one datum
    /// letter on two features.
    pub fn hand_on_gdt(&mut self, consumed: &[SolidId], result: SolidId) {
        let mut merged = self.drf.remove(&result).unwrap_or_default();
        for solid in consumed {
            if *solid == result {
                continue;
            }
            if let Some(frame) = self.drf.remove(solid) {
                for datum in frame.datums {
                    if merged.datum_by_label(&datum.label).is_none() {
                        merged.datums.push(datum);
                    }
                }
            }
        }
        if !merged.is_empty() {
            self.drf.insert(result, merged);
        }
        self.gdt.hand_on(consumed, result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdt::model::{DatumRef, GeometricCharacteristic, MaterialModifier};
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::operations::transform::{transform_solid, TransformOptions};
    use crate::primitives::surface::Plane;
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn plate(m: &mut BRepModel, key: &str, w: f64, d: f64, h: f64) -> SolidId {
        m.set_event_key(Some(key.into()));
        let s = match TopologyBuilder::new(m).create_box_3d(w, d, h) {
            Ok(GeometryId::Solid(s)) => s,
            other => panic!("box: {other:?}"),
        };
        m.set_event_key(None);
        s
    }

    /// The planar face of `solid` whose outward normal is `n` (±Z).
    fn face_along(m: &BRepModel, solid: SolidId, nz: f64) -> FaceId {
        solid_face_ids(m, solid)
            .into_iter()
            .find(|&f| {
                let face = m.faces.get(f).expect("face");
                m.surfaces
                    .get(face.surface_id)
                    .and_then(|s| s.as_any().downcast_ref::<Plane>())
                    .is_some_and(|p| (p.normal * face.orientation.sign()).z * nz > 0.99)
            })
            .expect("a face along the requested normal")
    }

    fn frame(c: GeometricCharacteristic, tol: f64, refs: &[&str]) -> FeatureControlFrame {
        FeatureControlFrame {
            characteristic: c,
            tolerance_value: tol,
            diametral_zone: false,
            modifier: MaterialModifier::Rfs,
            datum_refs: refs.iter().map(|l| DatumRef::new(*l)).collect(),
            basic: None,
        }
    }

    /// A shell (offset) of a part hands its GD&T to the hollow, like a
    /// boolean does: the datum on an exterior face the hollow keeps stays
    /// live, the one on the removed (opened) face dangles, and nothing is
    /// left on the retired source solid.
    #[test]
    fn datums_follow_the_part_through_a_shell() {
        use crate::gdt::drf::{resolve_datum, DatumResolution};
        use crate::operations::offset::{offset_solid, OffsetOptions};
        let mut m = BRepModel::new();
        let s = plate(&mut m, "plate", 10.0, 10.0, 10.0);
        let bottom = face_along(&m, s, -1.0);
        let top = face_along(&m, s, 1.0);
        m.gdt_designate_datum(s, "A", FeatureTarget::Face(bottom))
            .expect("A");
        m.gdt_designate_datum(s, "B", FeatureTarget::Face(top))
            .expect("B");
        let hollow = offset_solid(&mut m, s, 1.0, vec![top], OffsetOptions::default())
            .expect("shell open at the top");
        assert_ne!(hollow, s);
        let frame_now = m
            .drf
            .get(&hollow)
            .cloned()
            .expect("the frame followed the part");
        let status: Vec<(String, bool)> = frame_now
            .datums
            .iter()
            .map(|d| {
                (
                    d.label.clone(),
                    matches!(resolve_datum(&m, hollow, d), DatumResolution::Live { .. }),
                )
            })
            .collect();
        assert_eq!(
            status,
            vec![("A".to_string(), true), ("B".to_string(), false)],
            "the bottom stays, the opened top is gone"
        );
        assert!(m.drf.get(&s).is_none(), "nothing left on the retired solid");
    }

    #[test]
    fn each_frame_rule_refuses_and_changes_nothing() {
        let mut m = BRepModel::new();
        let s = plate(&mut m, "p", 10.0, 10.0, 10.0);
        let bottom = face_along(&m, s, -1.0);
        let top = face_along(&m, s, 1.0);
        m.gdt_designate_datum(s, "A", FeatureTarget::Face(bottom))
            .expect("A");
        let before = (m.drf.clone(), m.gdt.clone());
        use GeometricCharacteristic as G;
        let cases: Vec<(FeatureControlFrame, fn(&GdtEditError) -> bool)> = vec![
            (frame(G::Flatness, 0.0, &[]), |e| {
                matches!(e, GdtEditError::NonPositiveTolerance { .. })
            }),
            (frame(G::Flatness, f64::NAN, &[]), |e| {
                matches!(e, GdtEditError::NonPositiveTolerance { .. })
            }),
            (frame(G::Flatness, 0.05, &["A"]), |e| {
                matches!(e, GdtEditError::DatumRefsNotAllowed { .. })
            }),
            (frame(G::Perpendicularity, 0.05, &[]), |e| {
                matches!(e, GdtEditError::DatumRefsRequired { .. })
            }),
            (frame(G::Parallelism, 0.05, &["A", "A"]), |e| {
                matches!(e, GdtEditError::RepeatedDatumRef { .. })
            }),
            (frame(G::Position, 0.05, &["A", "B", "C", "D"]), |e| {
                matches!(e, GdtEditError::TooManyDatumRefs { count: 4 })
            }),
            (frame(G::Parallelism, 0.05, &["B"]), |e| {
                matches!(e, GdtEditError::DatumNotDesignated { .. })
            }),
        ];
        for (i, (fcf, is_expected)) in cases.into_iter().enumerate() {
            let err = m
                .gdt_create_annotation(
                    s,
                    &format!("f{i}"),
                    FeatureTarget::Face(top),
                    Annotation::Geometric(fcf.clone()),
                )
                .expect_err("refused");
            assert!(is_expected(&err), "case {i} {fcf:?}: {err:?}");
            assert_eq!(
                (m.drf.clone(), m.gdt.clone()),
                before,
                "case {i} changed the model"
            );
        }
    }

    #[test]
    fn a_referenced_datum_cannot_be_deleted_or_relettered() {
        let mut m = BRepModel::new();
        let s = plate(&mut m, "p", 10.0, 10.0, 10.0);
        let bottom = face_along(&m, s, -1.0);
        let top = face_along(&m, s, 1.0);
        m.gdt_designate_datum(s, "A", FeatureTarget::Face(bottom))
            .expect("A");
        m.gdt_create_annotation(
            s,
            "par",
            FeatureTarget::Face(top),
            Annotation::Geometric(frame(GeometricCharacteristic::Parallelism, 0.05, &["A"])),
        )
        .expect("frame");
        let refused = m.gdt_delete_datum(s, "A").expect_err("referenced");
        assert_eq!(
            refused,
            GdtError::DatumReferenced {
                solid: s,
                label: "A".into(),
                annotations: vec!["par".into()],
            }
        );
        assert!(matches!(
            m.gdt_update_datum(s, "A", Some("D"), None),
            Err(GdtError::DatumReferenced { .. })
        ));
        m.gdt_delete_annotation(s, "par").expect("frame delete");
        m.gdt_update_datum(s, "A", Some("D"), None)
            .expect("an unreferenced datum re-letters");
        m.gdt_delete_datum(s, "D").expect("and deletes");
        assert!(m.drf.get(&s).is_none_or(|f| f.is_empty()));
    }

    /// The datum frame follows the part through a boolean: a face that
    /// survives keeps its datum live, a consumed one leaves it dangling on the
    /// RESULT part, never dropped. The end-to-end RED (before `hand_on_gdt`
    /// was wired into `boolean_operation`) is `api-server`
    /// `gdt_history_tests`: the boolean's result part listed `datums: []`.
    #[test]
    fn datums_and_frames_follow_the_part_through_a_boolean() {
        use crate::gdt::drf::{resolve_datum, DatumResolution};
        let mut m = BRepModel::new();
        let s = plate(&mut m, "plate", 10.0, 10.0, 10.0);
        let bottom = face_along(&m, s, -1.0);
        let top = face_along(&m, s, 1.0);
        m.gdt_designate_datum(s, "A", FeatureTarget::Face(bottom))
            .expect("A");
        m.gdt_designate_datum(s, "B", FeatureTarget::Face(top))
            .expect("B");
        m.gdt_create_annotation(
            s,
            "par",
            FeatureTarget::Face(top),
            Annotation::Geometric(frame(GeometricCharacteristic::Parallelism, 0.05, &["A"])),
        )
        .expect("frame");
        // The plate spans z ∈ [-5, 5]; the tool takes everything above z = 3.
        let tool = plate(&mut m, "tool", 20.0, 20.0, 10.0);
        transform_solid(
            &mut m,
            tool,
            crate::math::Matrix4::translation(0.0, 0.0, 8.0),
            TransformOptions::default(),
        )
        .expect("place tool");
        let result = boolean_operation(
            &mut m,
            s,
            tool,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("cut");
        assert_ne!(result, s);
        let frame_now = m
            .drf
            .get(&result)
            .cloned()
            .expect("the frame followed the part");
        let status: Vec<(String, bool)> = frame_now
            .datums
            .iter()
            .map(|d| {
                (
                    d.label.clone(),
                    matches!(resolve_datum(&m, result, d), DatumResolution::Live { .. }),
                )
            })
            .collect();
        assert_eq!(
            status,
            vec![("A".to_string(), true), ("B".to_string(), false)],
            "A's face survives, B's is consumed"
        );
        assert!(
            m.drf.get(&s).is_none(),
            "nothing left on the consumed solid"
        );
        let owned = m.gdt.identified_of(result);
        assert_eq!(owned.len(), 1, "the frame is still the part's");
        assert!(m.face_by_pid(owned[0].1.feature).is_none(), "and dangling");
    }

    #[test]
    fn a_boolean_joining_two_parts_with_the_same_datum_letter_is_refused() {
        let mut m = BRepModel::new();
        let a = plate(&mut m, "a", 10.0, 10.0, 10.0);
        let b = plate(&mut m, "b", 10.0, 10.0, 10.0);
        transform_solid(
            &mut m,
            b,
            crate::math::Matrix4::translation(5.0, 0.0, 0.0),
            TransformOptions::default(),
        )
        .expect("offset b");
        let fa = face_along(&m, a, -1.0);
        let fb = face_along(&m, b, 1.0);
        m.gdt_designate_datum(a, "A", FeatureTarget::Face(fa))
            .expect("A on a");
        m.gdt_designate_datum(b, "A", FeatureTarget::Face(fb))
            .expect("A on b");
        let solids_before = m.solids.len();
        let err = boolean_operation(&mut m, a, b, BooleanOp::Union, BooleanOptions::default())
            .expect_err("two datum A's cannot become one part");
        assert!(
            format!("{err}").contains("datum"),
            "the refusal names the datum conflict: {err}"
        );
        assert_eq!(m.solids.len(), solids_before, "nothing changed");
        assert!(m.drf.get(&a).is_some() && m.drf.get(&b).is_some());
    }
}

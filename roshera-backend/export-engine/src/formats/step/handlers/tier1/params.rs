//! Type-safe extractors over `ruststep::ast::Parameter`.
//!
//! STEP entity instance records — `Record { name, parameter }` — carry
//! a single `Parameter` payload that, for every realistic entity, is
//! a `Parameter::List` of per-field sub-parameters. The handler's job
//! is to walk that list, type-check each element, and convert it
//! into a Rust value (scalar, fixed-size array, entity reference).
//!
//! Doing that with raw `match` statements scattered across two dozen
//! handlers produces fragile, untraceable code: when a field is
//! malformed, the failure point in the source file is invisible. The
//! extractors here build the source location into every error so a
//! single look at the [`crate::formats::step::diagnostics::Warning`]
//! tells you "CARTESIAN_POINT #47, parameter [1][2]: expected a Real,
//! got Integer".
//!
//! Every extractor returns `Result<T, ParamError>`. A `ParamError`
//! carries enough context to be lifted to a `Warning` via
//! [`ParamError::into_warning`].

use ruststep::ast::{Name, Parameter};

use crate::formats::step::diagnostics::{Severity, Warning};

/// Why a parameter extraction failed.
#[derive(Debug, Clone)]
pub struct ParamError {
    /// Entity name (upper-cased), e.g. `"CARTESIAN_POINT"`.
    pub entity: String,
    /// Source `#N`.
    pub instance: u64,
    /// Dotted parameter path into the record, e.g. `"coordinates[2]"`.
    pub path: String,
    /// What the extractor expected.
    pub expected: String,
    /// What it actually found, summarised.
    pub found: String,
}

impl ParamError {
    /// Lift the structured error into a `Warning` so the caller can
    /// append it to the import report and continue.
    pub fn into_warning(self) -> Warning {
        Warning {
            severity: Severity::Warn,
            entity: self.entity,
            instance: Some(self.instance),
            message: format!(
                "parameter {} of #{}: expected {}, found {}",
                self.path, self.instance, self.expected, self.found
            ),
        }
    }
}

impl std::fmt::Display for ParamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} #{} at {}: expected {}, found {}",
            self.entity, self.instance, self.path, self.expected, self.found
        )
    }
}

/// Short summary string for a parameter — used in error messages.
fn describe(p: &Parameter) -> String {
    match p {
        Parameter::Integer(_) => "Integer".to_string(),
        Parameter::Real(_) => "Real".to_string(),
        Parameter::String(_) => "String".to_string(),
        Parameter::Enumeration(s) => format!("Enumeration(.{s}.)"),
        Parameter::List(items) => format!("List(len={})", items.len()),
        Parameter::Ref(_) => "Ref".to_string(),
        Parameter::Typed { keyword, .. } => format!("Typed({keyword})"),
        Parameter::NotProvided => "$".to_string(),
        Parameter::Omitted => "*".to_string(),
    }
}

/// The top-level parameter on every entity record is a `Parameter::List`
/// of the per-field sub-parameters. This helper unwraps it.
pub fn record_fields<'a>(
    param: &'a Parameter,
    entity: &str,
    instance: u64,
) -> Result<&'a [Parameter], ParamError> {
    match param {
        Parameter::List(items) => Ok(items.as_slice()),
        other => Err(ParamError {
            entity: entity.to_string(),
            instance,
            path: ".".to_string(),
            expected: "List (record fields)".to_string(),
            found: describe(other),
        }),
    }
}

/// Borrow the `i`-th field of an already-unwrapped record, with an
/// error that points at the bad index.
pub fn field<'a>(
    fields: &'a [Parameter],
    index: usize,
    entity: &str,
    instance: u64,
) -> Result<&'a Parameter, ParamError> {
    fields.get(index).ok_or_else(|| ParamError {
        entity: entity.to_string(),
        instance,
        path: format!("[{index}]"),
        expected: "field present".to_string(),
        found: format!("only {} fields", fields.len()),
    })
}

/// Extract a `Real` (allowing `Integer` to widen). STEP files often
/// emit `0` rather than `0.0` for whole-number coordinates.
pub fn as_real(
    param: &Parameter,
    entity: &str,
    instance: u64,
    path: &str,
) -> Result<f64, ParamError> {
    match param {
        Parameter::Real(v) => Ok(*v),
        Parameter::Integer(v) => Ok(*v as f64),
        other => Err(ParamError {
            entity: entity.to_string(),
            instance,
            path: path.to_string(),
            expected: "Real".to_string(),
            found: describe(other),
        }),
    }
}

/// Extract a `String`.
pub fn as_string(
    param: &Parameter,
    entity: &str,
    instance: u64,
    path: &str,
) -> Result<String, ParamError> {
    match param {
        Parameter::String(s) => Ok(s.clone()),
        other => Err(ParamError {
            entity: entity.to_string(),
            instance,
            path: path.to_string(),
            expected: "String".to_string(),
            found: describe(other),
        }),
    }
}

/// Extract an `Integer`.
pub fn as_integer(
    param: &Parameter,
    entity: &str,
    instance: u64,
    path: &str,
) -> Result<i64, ParamError> {
    match param {
        Parameter::Integer(v) => Ok(*v),
        other => Err(ParamError {
            entity: entity.to_string(),
            instance,
            path: path.to_string(),
            expected: "Integer".to_string(),
            found: describe(other),
        }),
    }
}

/// Extract an `Enumeration` (e.g. `.T.` / `.F.` for booleans, or
/// `.METRE.` for SI unit names). The leading/trailing dots are
/// stripped by ruststep; the string is returned verbatim.
pub fn as_enum(
    param: &Parameter,
    entity: &str,
    instance: u64,
    path: &str,
) -> Result<String, ParamError> {
    match param {
        Parameter::Enumeration(s) => Ok(s.clone()),
        other => Err(ParamError {
            entity: entity.to_string(),
            instance,
            path: path.to_string(),
            expected: "Enumeration".to_string(),
            found: describe(other),
        }),
    }
}

/// Map a STEP boolean enumeration (`.T.` / `.F.` / `.U.`) to a Rust
/// `Option<bool>`: `.T.` → `Some(true)`, `.F.` → `Some(false)`,
/// `.U.` (unknown) → `None`. Any other token is an error.
pub fn as_bool(
    param: &Parameter,
    entity: &str,
    instance: u64,
    path: &str,
) -> Result<Option<bool>, ParamError> {
    let s = as_enum(param, entity, instance, path)?;
    match s.as_str() {
        "T" => Ok(Some(true)),
        "F" => Ok(Some(false)),
        "U" => Ok(None),
        _ => Err(ParamError {
            entity: entity.to_string(),
            instance,
            path: path.to_string(),
            expected: ".T. / .F. / .U.".to_string(),
            found: format!(".{s}."),
        }),
    }
}

/// Extract a list of children.
pub fn as_list<'a>(
    param: &'a Parameter,
    entity: &str,
    instance: u64,
    path: &str,
) -> Result<&'a [Parameter], ParamError> {
    match param {
        Parameter::List(items) => Ok(items.as_slice()),
        other => Err(ParamError {
            entity: entity.to_string(),
            instance,
            path: path.to_string(),
            expected: "List".to_string(),
            found: describe(other),
        }),
    }
}

/// Extract a list of `N` reals into a fixed-size array. The most
/// common STEP shape — `CARTESIAN_POINT.coordinates`, `DIRECTION.direction_ratios`.
pub fn as_real_array<const N: usize>(
    param: &Parameter,
    entity: &str,
    instance: u64,
    path: &str,
) -> Result<[f64; N], ParamError> {
    let items = as_list(param, entity, instance, path)?;
    if items.len() != N {
        return Err(ParamError {
            entity: entity.to_string(),
            instance,
            path: path.to_string(),
            expected: "List of N reals".to_string(),
            found: format!("List(len={})", items.len()),
        });
    }
    let mut out = [0.0_f64; N];
    for (i, item) in items.iter().enumerate() {
        out[i] = as_real(item, entity, instance, &format!("{path}[{i}]"))?;
    }
    Ok(out)
}

/// Extract an entity reference (`#N`). Returns the instance number.
/// Rejects value refs (`@N`) and constant names.
pub fn as_entity_ref(
    param: &Parameter,
    entity: &str,
    instance: u64,
    path: &str,
) -> Result<u64, ParamError> {
    match param {
        Parameter::Ref(Name::Entity(id)) => Ok(*id),
        other => Err(ParamError {
            entity: entity.to_string(),
            instance,
            path: path.to_string(),
            expected: "Entity ref (#N)".to_string(),
            found: describe(other),
        }),
    }
}

/// Extract a list of entity references.
pub fn as_entity_ref_list(
    param: &Parameter,
    entity: &str,
    instance: u64,
    path: &str,
) -> Result<Vec<u64>, ParamError> {
    let items = as_list(param, entity, instance, path)?;
    items
        .iter()
        .enumerate()
        .map(|(i, p)| as_entity_ref(p, entity, instance, &format!("{path}[{i}]")))
        .collect()
}

/// Unwrap a `Parameter::Typed { keyword, parameter }` when the
/// keyword matches `expected_keyword`. Used by SI_UNIT and similar
/// schema-derived typed wrappers.
pub fn as_typed<'a>(
    param: &'a Parameter,
    expected_keyword: &str,
    entity: &str,
    instance: u64,
    path: &str,
) -> Result<&'a Parameter, ParamError> {
    match param {
        Parameter::Typed { keyword, parameter } if keyword.eq_ignore_ascii_case(expected_keyword) => {
            Ok(parameter.as_ref())
        }
        Parameter::Typed { keyword, .. } => Err(ParamError {
            entity: entity.to_string(),
            instance,
            path: path.to_string(),
            expected: expected_keyword.to_string(),
            found: format!("Typed({keyword})"),
        }),
        other => Err(ParamError {
            entity: entity.to_string(),
            instance,
            path: path.to_string(),
            expected: "Typed".to_string(),
            found: describe(other),
        }),
    }
}

/// `Option`-typed entity reference: `Parameter::NotProvided` (`$`)
/// or `Parameter::Omitted` (`*`) yields `Ok(None)`. Any other shape
/// is forwarded through [`as_entity_ref`].
pub fn as_optional_entity_ref(
    param: &Parameter,
    entity: &str,
    instance: u64,
    path: &str,
) -> Result<Option<u64>, ParamError> {
    match param {
        Parameter::NotProvided | Parameter::Omitted => Ok(None),
        other => as_entity_ref(other, entity, instance, path).map(Some),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_list(items: Vec<Parameter>) -> Parameter {
        Parameter::List(items)
    }

    #[test]
    fn record_fields_unwraps_list() {
        let p = mk_list(vec![Parameter::Real(1.0), Parameter::Real(2.0)]);
        let fields = record_fields(&p, "TEST", 7).unwrap();
        assert_eq!(fields.len(), 2);
    }

    #[test]
    fn record_fields_rejects_non_list() {
        let p = Parameter::Real(1.0);
        let err = record_fields(&p, "TEST", 7).unwrap_err();
        assert_eq!(err.entity, "TEST");
        assert_eq!(err.instance, 7);
        assert!(err.found.starts_with("Real"));
    }

    #[test]
    fn as_real_widens_integer() {
        let p = Parameter::Integer(42);
        assert_eq!(as_real(&p, "TEST", 7, "x").unwrap(), 42.0);
    }

    #[test]
    fn as_real_rejects_string() {
        let p = Parameter::String("oops".to_string());
        let err = as_real(&p, "TEST", 7, "x").unwrap_err();
        assert_eq!(err.expected, "Real");
        assert!(err.found.starts_with("String"));
    }

    #[test]
    fn as_real_array_extracts_three() {
        let p = mk_list(vec![
            Parameter::Real(1.0),
            Parameter::Integer(2),
            Parameter::Real(3.5),
        ]);
        let coords: [f64; 3] = as_real_array(&p, "CARTESIAN_POINT", 1, "coordinates").unwrap();
        assert_eq!(coords, [1.0, 2.0, 3.5]);
    }

    #[test]
    fn as_real_array_rejects_wrong_length() {
        let p = mk_list(vec![Parameter::Real(1.0), Parameter::Real(2.0)]);
        let err = as_real_array::<3>(&p, "X", 1, "v").unwrap_err();
        assert_eq!(err.expected, "List of N reals");
    }

    #[test]
    fn as_bool_parses_t_f_u() {
        let t = Parameter::Enumeration("T".to_string());
        let f = Parameter::Enumeration("F".to_string());
        let u = Parameter::Enumeration("U".to_string());
        assert_eq!(as_bool(&t, "X", 1, "p").unwrap(), Some(true));
        assert_eq!(as_bool(&f, "X", 1, "p").unwrap(), Some(false));
        assert_eq!(as_bool(&u, "X", 1, "p").unwrap(), None);
    }

    #[test]
    fn as_entity_ref_extracts_id() {
        let p = Parameter::Ref(Name::Entity(123));
        assert_eq!(as_entity_ref(&p, "X", 1, "p").unwrap(), 123);
    }

    #[test]
    fn as_entity_ref_rejects_value_ref() {
        let p = Parameter::Ref(Name::Value(123));
        let err = as_entity_ref(&p, "X", 1, "p").unwrap_err();
        assert_eq!(err.expected, "Entity ref (#N)");
    }

    #[test]
    fn as_optional_entity_ref_yields_none_on_dollar() {
        let p = Parameter::NotProvided;
        assert_eq!(as_optional_entity_ref(&p, "X", 1, "p").unwrap(), None);
    }

    #[test]
    fn as_typed_matches_keyword_case_insensitive() {
        let p = Parameter::Typed {
            keyword: "LENGTH_MEASURE".to_string(),
            parameter: Box::new(Parameter::Real(25.4)),
        };
        let inner = as_typed(&p, "length_measure", "X", 1, "p").unwrap();
        assert!(matches!(inner, Parameter::Real(_)));
    }

    #[test]
    fn param_error_lifts_to_warning() {
        let err = ParamError {
            entity: "CARTESIAN_POINT".to_string(),
            instance: 42,
            path: "coordinates[1]".to_string(),
            expected: "Real".to_string(),
            found: "String".to_string(),
        };
        let w = err.into_warning();
        assert_eq!(w.severity, Severity::Warn);
        assert_eq!(w.entity, "CARTESIAN_POINT");
        assert_eq!(w.instance, Some(42));
        assert!(w.message.contains("coordinates[1]"));
        assert!(w.message.contains("expected Real"));
    }
}

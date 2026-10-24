//! The desired-state document (ADR-030, ADR-036).
//!
//! A declaration is data: resources the provider must create or keep in shape,
//! tombstones for what must not exist, and derivations that materialise a set
//! from an observed fact. The document is the core's contract with every
//! producer — a Nix flake, a configuration file, a script — so it lives in the
//! domain, not in a use case.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::errors::NodError;
use crate::domain::provider::DesiredResource;

/// The desired-state schema this core understands.
///
/// `derive` was added in schema 2; a document that declares a newer schema is
/// refused instead of being silently misread.
pub const SCHEMA: u64 = 2;

/// The declaration: what must exist, what must not, and what is derived.
#[derive(Debug, Clone, PartialEq)]
pub struct Desired {
    /// Resources the provider must create or keep in shape.
    pub resources: Vec<DesiredResource>,
    /// Resources the declaration says must not exist.
    ///
    /// The provider never sees this list: it plans from `resources` alone, and
    /// the delete it proposes for an owned resource outside that list is
    /// reclassified by the core. A separate list keeps an old provider from
    /// mistaking a tombstone for something to create.
    pub absent: Vec<DesiredResource>,
    /// Declared derivations: rules that materialise a set from an observed fact
    /// (ADR-036). The provider never sees them; the core expands them into
    /// `resources` before planning.
    pub derive: Vec<DeriveRule>,
}

/// Where a provider's declaration comes from.
///
/// A flake carries it as a value (one evaluation, no artifact); a configuration
/// file names a JSON file. Both are first-class: the core never requires Nix,
/// and a Nix fleet never writes a file it already holds in memory.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum DesiredSource {
    /// A JSON file with the document.
    Path(PathBuf),
    /// The document itself.
    Inline(Desired),
}

/// One declared rule.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeriveRule {
    /// Stable rule id, used for provenance and the deletion budget.
    pub id: String,
    /// The observed resource and attribute the set follows.
    #[serde(rename = "from")]
    pub from: Source,
    /// What to materialise per member.
    pub template: Template,
    /// How many derived removals this rule may cause in one run.
    ///
    /// `0` freezes the set: a member that leaves is not deleted until the
    /// operator allows it. The default is deliberately the safe one.
    #[serde(default)]
    pub max_deletions: usize,
}

/// Where a rule reads its members.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// Observed resource kind.
    pub kind: String,
    /// Observed resource id.
    pub id: String,
    /// Attribute holding the member list.
    pub attribute: String,
}

/// What a rule materialises per member.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    /// Resource kind to create.
    pub kind: String,
    /// Resource id, with `{member}` substituted.
    pub id: String,
    /// Resource attributes, with `{member}` substituted in strings.
    #[serde(default)]
    pub attributes: Value,
}

impl<'de> Deserialize<'de> for Desired {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        parse_value(value).map_err(serde::de::Error::custom)
    }
}

/// Parses a declaration from its JSON text.
pub fn parse(raw: &str) -> Result<Desired, NodError> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|error| NodError::config(format!("cannot parse desired state: {error}")))?;
    parse_value(value)
}

/// Parses a declaration from an already-decoded JSON value.
///
/// Both forms are accepted: a bare array of resources (schema 1) or an object
/// with `resources`, `absent` and `derive`.
pub fn parse_value(value: Value) -> Result<Desired, NodError> {
    match value {
        Value::Array(items) => Ok(Desired {
            resources: parse_resources(items)?,
            absent: Vec::new(),
            derive: Vec::new(),
        }),
        Value::Object(mut map) => {
            let schema = match map.remove("schema") {
                None | Some(Value::Null) => 1,
                Some(Value::Number(number)) => number.as_u64().ok_or_else(|| {
                    NodError::config("desired state `schema` must be a positive integer")
                })?,
                Some(_) => {
                    return Err(NodError::config(
                        "desired state `schema` must be a positive integer",
                    ))
                }
            };
            if schema > SCHEMA {
                return Err(NodError::config(format!(
                    "desired state declares schema {schema}, this core understands {SCHEMA}"
                )));
            }
            let resources = match map.remove("resources") {
                Some(Value::Array(items)) => items,
                _ => {
                    return Err(NodError::config(
                        "desired state object must contain a `resources` array",
                    ))
                }
            };
            let absent = match map.remove("absent") {
                Some(Value::Array(items)) => parse_resources(items)?,
                None | Some(Value::Null) => Vec::new(),
                Some(_) => return Err(NodError::config("desired state `absent` must be an array")),
            };
            let derive = match map.remove("derive") {
                None | Some(Value::Null) => Vec::new(),
                Some(value) => {
                    if schema < 2 {
                        return Err(NodError::config(
                            "desired state uses `derive`, which requires schema 2",
                        ));
                    }
                    parse_derive(&value)?
                }
            };
            Ok(Desired {
                resources: parse_resources(resources)?,
                absent,
                derive,
            })
        }
        _ => Err(NodError::config(
            "desired state must be an array or an object with `resources`",
        )),
    }
}

/// Parses a list of desired resources.
fn parse_resources(items: Vec<Value>) -> Result<Vec<DesiredResource>, NodError> {
    items
        .into_iter()
        .map(|item| {
            serde_json::from_value(item)
                .map_err(|error| NodError::config(format!("invalid desired resource: {error}")))
        })
        .collect()
}

/// Parses the `derive` list of a declaration.
pub fn parse_derive(value: &Value) -> Result<Vec<DeriveRule>, NodError> {
    serde_json::from_value(value.clone())
        .map_err(|error| NodError::config(format!("invalid derivation rule: {error}")))
}

impl Desired {
    /// Serialises the declaration for a producer that needs the text form.
    pub fn to_json(&self) -> Result<Value, NodError> {
        let mut map = serde_json::Map::new();
        map.insert("schema".to_string(), Value::from(SCHEMA));
        map.insert(
            "resources".to_string(),
            serde_json::to_value(&self.resources).map_err(|error| {
                NodError::config(format!("cannot serialise resources: {error}"))
            })?,
        );
        map.insert(
            "absent".to_string(),
            serde_json::to_value(&self.absent)
                .map_err(|error| NodError::config(format!("cannot serialise absent: {error}")))?,
        );
        if !self.derive.is_empty() {
            map.insert(
                "derive".to_string(),
                serde_json::to_value(&self.derive).map_err(|error| {
                    NodError::config(format!("cannot serialise derive: {error}"))
                })?,
            );
        }
        Ok(Value::Object(map))
    }
}

/// `DesiredResource` needs `Serialize` for the text form.
impl Serialize for Desired {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_json()
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

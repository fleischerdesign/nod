//! Domain entities for Nix expression evaluation (ADR-016).

use serde::{Deserialize, Serialize};

/// The result of evaluating a Nix expression in a host's configuration context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvalResult {
    /// Target host name where the expression was evaluated.
    pub host_name: String,
    /// Attribute expression that was queried (e.g. `config.services.nginx.enable`).
    pub expression: String,
    /// Raw evaluated string representation from Nix.
    pub raw_output: String,
    /// Structured JSON value when evaluated with `--json`.
    pub json_value: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_result_serializes_and_deserializes() {
        let res = EvalResult {
            host_name: "yorke".to_string(),
            expression: "networking.hostName".to_string(),
            raw_output: "\"yorke\"".to_string(),
            json_value: Some(serde_json::json!("yorke")),
        };
        let s = serde_json::to_string(&res).unwrap();
        let parsed: EvalResult = serde_json::from_str(&s).unwrap();
        assert_eq!(res, parsed);
    }
}

//! The `on` grammar: member access, and nothing else.
//!
//! A general `JSONPath` evaluator would silently accept
//! `$.payload.confidence > 0.9` — a comparison, which means the engine could
//! compute a predicate. That is the one thing this format exists to prevent:
//! the graph owns every edge, so the engine may only *select* among ports an
//! author already drew, never *decide* whether one exists. The grammar is
//! therefore exactly `$` `.` `payload` `.` `<field>`, where `<field>` is one
//! of `DecisionPayload`'s real, routable field names. Anything else —
//! arithmetic, a range, a wildcard, an index, a nested path, a missing root,
//! a missing field — is refused with [`FlowError::BadOnExpr`].

use crate::error::FlowError;

/// A field on the route payload that `on` may select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadField {
    /// `$.payload.label`
    Label,
    /// `$.payload.band`
    Band,
    /// `$.payload.confidence`
    Confidence,
    /// `$.payload.near_tie`
    NearTie,
    /// `$.payload.saturated`
    Saturated,
}

impl PayloadField {
    /// The field's wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Label => "label",
            Self::Band => "band",
            Self::Confidence => "confidence",
            Self::NearTie => "near_tie",
            Self::Saturated => "saturated",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "label" => Some(Self::Label),
            "band" => Some(Self::Band),
            "confidence" => Some(Self::Confidence),
            "near_tie" => Some(Self::NearTie),
            "saturated" => Some(Self::Saturated),
            _ => None,
        }
    }
}

/// The field names `on` may select — the routable subset of
/// `DecisionPayload`'s fields.
pub const ACCEPTED_FIELDS: &[&str] = &["label", "band", "confidence", "near_tie", "saturated"];

/// Fields `DecisionPayload` carries that `on` may never select.
///
/// Each carries the reason it is excluded, declared explicitly rather than
/// just "not in `ACCEPTED_FIELDS`", so the key-set test below protects
/// against the payload growing a field that is silently routable, silently
/// dropped, or silently forgotten.
pub const NON_ROUTABLE_FIELDS: &[(&str, &str)] = &[
    ("question_id", "an identifier, not a predicate input"),
    (
        "margin",
        "a derived float; exposing it to `on` would let a flow author write a disguised \
         comparison, which is exactly what this grammar exists to forbid",
    ),
    (
        "p",
        "a raw belief, same reason as margin — and present only for a noul answer",
    ),
    (
        "score",
        "a raw rubric position, same reason as margin — and present only for a score answer",
    ),
    (
        "probabilities",
        "a vector, not a scalar a member-access grammar can compare",
    ),
    (
        "options",
        "a vector, not a scalar a member-access grammar can compare",
    ),
];

/// Parse an `on` expression under the member-access-only grammar.
///
/// # Errors
///
/// Returns [`FlowError::BadOnExpr`] for anything that is not exactly
/// `$.payload.<field>` with `<field>` a real, routable payload field.
pub fn parse(expr: &str) -> Result<PayloadField, FlowError> {
    expr.strip_prefix("$.payload.")
        .filter(|field| is_bare_identifier(field))
        .and_then(PayloadField::from_name)
        .ok_or_else(|| FlowError::BadOnExpr(expr.to_string()))
}

/// A single identifier: no dots (nested path), no brackets (index), no
/// operators or spaces (a comparison), no `*` (a wildcard), and not empty
/// (`$.payload` with nothing after the last dot).
fn is_bare_identifier(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::collections::HashSet;

    use laya_core::route_payload::DecisionPayload;

    use super::*;

    #[test]
    fn on_accepts_member_access() {
        let field = parse("$.payload.label").expect("member access must parse");
        assert_eq!(field, PayloadField::Label, "must resolve to field `label`");
    }

    #[test]
    fn on_refuses_anything_that_is_not_member_access() {
        for bad in [
            "$.payload.confidence > 0.9", // a comparison
            "$.payload.*",                // a wildcard
            "$.payload",                  // no field
            "payload.label",              // no root
            "$.label",                    // no payload segment
            "$.payload.label.sub",        // a nested path
            "$.payload.label[0]",         // an index
            "$.payload.len()",            // a call
        ] {
            let result = parse(bad);
            assert!(
                matches!(result, Err(FlowError::BadOnExpr(ref e)) if e == bad),
                "{bad:?} must be refused with BadOnExpr, got {result:?}"
            );
        }
    }

    #[test]
    fn on_refuses_a_field_the_payload_does_not_carry() {
        let result = parse("$.payload.threshold");
        assert!(
            matches!(result, Err(FlowError::BadOnExpr(_))),
            "an unreal field must be refused, got {result:?}"
        );
    }

    #[test]
    fn every_accepted_field_round_trips() {
        for name in ACCEPTED_FIELDS {
            let expr = format!("$.payload.{name}");
            let field = parse(&expr).unwrap_or_else(|e| panic!("{expr} must parse: {e}"));
            assert_eq!(field.as_str(), *name);
        }
    }

    /// THE KEY-SET TEST. A field-by-field allowlist protects today's fields
    /// and lets tomorrow's drift silently; this comparison does not. It
    /// drives the REAL `DecisionPayload`, fully populated so no field is
    /// hidden by a `skip_serializing_if`, and asserts its serialized key set
    /// is exactly `ACCEPTED_FIELDS` union `NON_ROUTABLE_FIELDS` — no more, no
    /// less. If `DecisionPayload` grows a field, this test fails until the
    /// field is placed in one list or the other, on purpose.
    #[test]
    fn the_accepted_fields_match_the_real_payloads_key_set() {
        let payload = DecisionPayload {
            label: "a".to_string(),
            band: "high".to_string(),
            confidence: 0.9,
            p: Some(0.9),
            score: Some(4.0),
            probabilities: vec![0.9, 0.1],
            options: vec!["a".to_string(), "b".to_string()],
            question_id: "q".to_string(),
            margin: 0.8,
            near_tie: false,
            saturated: false,
        };
        let json = serde_json::to_value(&payload).expect("DecisionPayload must serialize");
        let keys: HashSet<&str> = json
            .as_object()
            .expect("payload must serialize to an object")
            .keys()
            .map(String::as_str)
            .collect();

        let mut expected: HashSet<&str> = ACCEPTED_FIELDS.iter().copied().collect();
        expected.extend(NON_ROUTABLE_FIELDS.iter().map(|(name, _reason)| *name));

        assert_eq!(
            keys, expected,
            "DecisionPayload's real key set must equal ACCEPTED_FIELDS \u{222a} NON_ROUTABLE_FIELDS exactly"
        );
    }
}

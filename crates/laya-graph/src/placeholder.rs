//! `{{node_id}}` placeholders in a decide's `prompt` (spec 18).
//!
//! A placeholder resolves to ONE value — the trimmed stdout of a node in the decide's
//! `depends_on` — and nothing else. There is deliberately no expression grammar here: if the
//! graph could evaluate `count > 300`, the graph would be the decision layer and the measured
//! head would not be.

use crate::error::FlowError;
use crate::flow::{Flow, Node};

/// The ids named by `{{id}}` in `text`, in order, duplicates kept. A `{{` with no closing
/// `}}` names nothing and is left as literal text.
#[must_use]
pub fn references(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some((_, after)) = rest.split_once("{{") {
        let Some((id, tail)) = after.split_once("}}") else {
            break;
        };
        out.push(id);
        rest = tail;
    }
    out
}

/// Replace each `{{id}}` in `text` with `value(id)`; `Err(id)` names the first id `value`
/// could not supply. Never substitutes an empty string for a missing value.
///
/// # Errors
/// The first referenced id for which `value` returned `None`.
pub fn substitute<'a>(
    text: &str,
    mut value: impl FnMut(&str) -> Option<&'a str>,
) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((before, after)) = rest.split_once("{{") {
        let Some((id, tail)) = after.split_once("}}") else {
            break;
        };
        let Some(v) = value(id) else {
            return Err(id.to_string());
        };
        out.push_str(before);
        out.push_str(v);
        rest = tail;
    }
    out.push_str(rest);
    Ok(out)
}

/// Refuse a `decide` whose prompt names a reference it cannot honour, by name.
///
/// # Errors
/// [`FlowError::MissingPlaceholderTarget`], [`FlowError::PlaceholderNotDependency`] or
/// [`FlowError::PlaceholderSelf`].
pub fn validate_references(node: &Node, flow: &Flow) -> Result<(), FlowError> {
    let Some(prompt) = node.prompt.as_deref() else {
        return Ok(());
    };
    for target in references(prompt) {
        if target == node.id {
            return Err(FlowError::PlaceholderSelf {
                id: node.id.clone(),
            });
        }
        if !flow.nodes.iter().any(|n| n.id == target) {
            return Err(FlowError::MissingPlaceholderTarget {
                id: node.id.clone(),
                target: target.to_string(),
            });
        }
        if !node.depends_on.iter().any(|d| d == target) {
            return Err(FlowError::PlaceholderNotDependency {
                id: node.id.clone(),
                target: target.to_string(),
            });
        }
    }
    Ok(())
}

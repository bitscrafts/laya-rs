//! The field-effect contract (spec 20): every field of [`Flow`] and [`Node`] has a row
//! saying what it does — it is **observable** in a run, **refused** by name until it is
//! implemented, or **documentary** by design with the reason written down.
//!
//! The load-bearing test is [`every_serialised_field_has_a_row`]: it serialises a *fully
//! populated* value (every `Option` is `Some`, every `bool` true — `skip_serializing_if` hides
//! a `None`) and requires the keys to equal the table's names, so a field added without a row
//! fails here. A field that "does nothing" can then only be added on purpose.
//!
//! Wire-level fields (`question_type`, `options` -> `criteria`) are observable in `laya-cli`,
//! not here; they have their own table there.

#![cfg(test)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::flow::{Flow, Kind, Node, Retry};
use crate::node::{CallContext, McpCaller};
use crate::run::{NodeRecord, RunReport};
use crate::sched::RunOptions;
use crate::{expand, lower, run_flow, validate};

enum Effect {
    /// Two fixtures differing ONLY in this field. Both run through validate -> lower ->
    /// `run_flow` with a [`ScriptedCaller`] (no head needed). `differs` must return true.
    Observable {
        with: &'static str,
        without: &'static str,
        differs: fn(&RunReport, &RunReport) -> bool,
    },
    /// Refused until implemented; validate must return an error containing this text.
    Refused {
        fixture: &'static str,
        error_contains: &'static str,
    },
    /// Carries no behaviour by design. The reason is required and reviewed.
    Documentary { reason: &'static str },
}

/// Answers `decide` with label `high` / band `low` and echoes the arguments it was given;
/// any other tool echoes its name and arguments. No head, no network.
struct ScriptedCaller;

impl McpCaller for ScriptedCaller {
    fn call(&self, tool: &str, arguments: &Value, _ctx: &CallContext) -> Result<String, String> {
        if tool == "decide" {
            Ok(json!({"payload": {"label": "high", "band": "low"}, "seen": arguments}).to_string())
        } else {
            Ok(json!({"tool": tool, "arguments": arguments}).to_string())
        }
    }
}

fn load(text: &str) -> Result<Flow, String> {
    let raw: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let flow = expand(&raw, &crate::Library::new()).map_err(|e| e.to_string())?;
    validate(&flow).map_err(|e| e.to_string())?;
    Ok(flow)
}

/// validate -> lower -> `run_flow`, the path `laya graph run` takes.
fn run_text(text: &str) -> Result<RunReport, String> {
    let flow = load(text)?;
    let graph = lower(&flow).map_err(|e| e.to_string())?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(run_flow(
        &graph,
        RunOptions {
            run_id: "contract".to_string(),
            limit: 4,
            mcp: Arc::new(ScriptedCaller),
        },
    ))
    .map_err(|e| e.to_string())
}

fn rec<'a>(report: &'a RunReport, id: &str) -> Option<&'a NodeRecord> {
    report.records.iter().find(|r| r.node_id == id)
}

fn start(report: &RunReport, id: &str) -> u64 {
    rec(report, id).map_or(u64::MAX, |r| r.started_at_ms)
}

fn output(report: &RunReport, id: &str) -> String {
    rec(report, id)
        .map(|r| r.output.clone())
        .unwrap_or_default()
}

fn ran(report: &RunReport) -> BTreeSet<String> {
    report.records.iter().map(|r| r.node_id.clone()).collect()
}

/// Two nodes `a` and `b`, each `sleep 0.3`, with `extra` on `a` and `b`.
macro_rules! two_sleepers {
    ($a_extra:literal, $b_extra:literal) => {
        concat!(
            r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"sleep 0.3""#,
            $a_extra,
            r#"},{"id":"b","kind":"shell","run":"sleep 0.3""#,
            $b_extra,
            r#"}]}"#
        )
    };
}

/// A decide `d` over options low/high (label answered `high`, band answered `low`) with
/// targets `x` and `y`. `$extra` is spliced into the decide node; `$options` and `$ports` are
/// the JSON values of those two fields.
macro_rules! decide {
    ($extra:literal, $options:literal, $ports:literal) => {
        concat!(
            r#"{"flow":"t","version":1,"nodes":[{"id":"d","kind":"decide","question":"q?","prompt":"p","options":"#,
            $options,
            r#","ports":"#,
            $ports,
            $extra,
            r#"},{"id":"x","kind":"shell","run":"echo x"},{"id":"y","kind":"shell","run":"echo y"}]}"#
        )
    };
}

const NODE_FIELDS: &[(&str, Effect)] = &[
    (
        "id",
        Effect::Observable {
            with: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"echo 1"}]}"#,
            without: r#"{"flow":"t","version":1,"nodes":[{"id":"b","kind":"shell","run":"echo 1"}]}"#,
            differs: |w, wo| ran(w) != ran(wo),
        },
    ),
    (
        "kind",
        Effect::Observable {
            with: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"echo 1"}]}"#,
            without: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"mcp","tool":"t"}]}"#,
            differs: |w, wo| output(w, "a") != output(wo, "a"),
        },
    ),
    (
        "depends_on",
        Effect::Observable {
            with: two_sleepers!("", r#","depends_on":["a"]"#),
            without: two_sleepers!("", ""),
            differs: |w, wo| start(w, "b") >= 250 && start(wo, "b") < 150,
        },
    ),
    (
        "timeout",
        Effect::Observable {
            with: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"sleep 1.6","timeout":1}]}"#,
            without: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"sleep 1.6"}]}"#,
            differs: |w, wo| {
                rec(w, "a").is_some_and(|r| r.timed_out)
                    && rec(wo, "a").is_some_and(|r| !r.timed_out)
            },
        },
    ),
    (
        "retry",
        Effect::Refused {
            fixture: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"true","retry":{"attempts":3,"backoff_ms":10}}]}"#,
            error_contains: "shell loop",
        },
    ),
    (
        "allow_failure",
        Effect::Observable {
            with: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"false","allow_failure":true}]}"#,
            without: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"false"}]}"#,
            differs: |w, wo| !w.stopped_early && wo.stopped_early,
        },
    ),
    (
        "writes",
        Effect::Observable {
            with: two_sleepers!(r#","writes":true"#, r#","writes":true"#),
            without: two_sleepers!("", ""),
            differs: |w, wo| {
                let gap = |r: &RunReport| start(r, "a").abs_diff(start(r, "b"));
                gap(w) >= 250 && gap(wo) < 150
            },
        },
    ),
    (
        "run",
        Effect::Observable {
            with: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"echo one"}]}"#,
            without: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"echo two"}]}"#,
            differs: |w, wo| output(w, "a") != output(wo, "a"),
        },
    ),
    (
        "tool",
        Effect::Observable {
            with: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"mcp","tool":"t1"}]}"#,
            without: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"mcp","tool":"t2"}]}"#,
            differs: |w, wo| output(w, "a") != output(wo, "a"),
        },
    ),
    (
        "arguments",
        Effect::Observable {
            with: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"mcp","tool":"t","arguments":{"k":1}}]}"#,
            without: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"mcp","tool":"t","arguments":{"k":2}}]}"#,
            differs: |w, wo| output(w, "a") != output(wo, "a"),
        },
    ),
    (
        "question",
        Effect::Observable {
            with: r#"{"flow":"t","version":1,"nodes":[{"id":"d","kind":"decide","question":"one?","prompt":"p","options":["a"],"ports":{"_":"d2"}},{"id":"d2","kind":"shell","run":"true"}]}"#,
            without: r#"{"flow":"t","version":1,"nodes":[{"id":"d","kind":"decide","question":"two?","prompt":"p","options":["a"],"ports":{"_":"d2"}},{"id":"d2","kind":"shell","run":"true"}]}"#,
            differs: |w, wo| output(w, "d") != output(wo, "d"),
        },
    ),
    (
        "options",
        Effect::Observable {
            with: decide!("", r#"["low","high"]"#, r#"{"low":"x","high":"y","_":"x"}"#),
            without: decide!(
                "",
                r#"["low","high","mid"]"#,
                r#"{"low":"x","high":"y","_":"x"}"#
            ),
            differs: |w, wo| output(w, "d") != output(wo, "d"),
        },
    ),
    (
        "prompt",
        Effect::Observable {
            with: r#"{"flow":"t","version":1,"nodes":[{"id":"d","kind":"decide","question":"q","prompt":"state one","options":["a"],"ports":{"_":"d2"}},{"id":"d2","kind":"shell","run":"true"}]}"#,
            without: r#"{"flow":"t","version":1,"nodes":[{"id":"d","kind":"decide","question":"q","prompt":"state two","options":["a"],"ports":{"_":"d2"}},{"id":"d2","kind":"shell","run":"true"}]}"#,
            differs: |w, wo| output(w, "d") != output(wo, "d"),
        },
    ),
    (
        "on",
        // label answered `high` -> y; band answered `low` -> x.
        Effect::Observable {
            with: decide!(
                r#","on":"band""#,
                r#"["low","high"]"#,
                r#"{"low":"x","high":"y","_":"x"}"#
            ),
            without: decide!("", r#"["low","high"]"#, r#"{"low":"x","high":"y","_":"x"}"#),
            differs: |w, wo| {
                output(w, "d/route").contains(r#""selected_port":"x""#)
                    && output(wo, "d/route").contains(r#""selected_port":"y""#)
            },
        },
    ),
    (
        "ports",
        Effect::Observable {
            with: decide!("", r#"["low","high"]"#, r#"{"low":"x","high":"y","_":"x"}"#),
            without: decide!("", r#"["low","high"]"#, r#"{"low":"x","high":"x","_":"x"}"#),
            differs: |w, wo| {
                output(w, "d/route").contains(r#""selected_port":"y""#)
                    && output(wo, "d/route").contains(r#""selected_port":"x""#)
            },
        },
    ),
    (
        "escalate_on_near_tie",
        Effect::Refused {
            fixture: decide!(
                r#","escalate_on_near_tie":true,"near_tie_port":"x""#,
                r#"["low","high"]"#,
                r#"{"low":"x","high":"y","_":"x"}"#
            ),
            error_contains: "escalate_on_near_tie",
        },
    ),
    (
        "near_tie_port",
        Effect::Refused {
            fixture: decide!(
                r#","near_tie_port":"x""#,
                r#"["low","high"]"#,
                r#"{"low":"x","high":"y","_":"x"}"#
            ),
            error_contains: "near_tie_port",
        },
    ),
];

const FLOW_FIELDS: &[(&str, Effect)] = &[
    (
        "flow",
        Effect::Documentary {
            reason: "the flow's name; a label for people and reports, read by no engine stage",
        },
    ),
    (
        "version",
        Effect::Refused {
            fixture: r#"{"flow":"t","version":999,"nodes":[]}"#,
            error_contains: "999",
        },
    ),
    (
        "description",
        Effect::Documentary {
            reason: "prose for the reader; it has no effect on validation, lowering or the run",
        },
    ),
    (
        "inputs",
        Effect::Refused {
            fixture: r#"{"flow":"t","version":1,"inputs":["doc"],"nodes":[{"id":"a","kind":"shell","run":"true"}]}"#,
            error_contains: "inputs",
        },
    ),
    (
        "timeout",
        Effect::Observable {
            with: r#"{"flow":"t","version":1,"timeout":1,"nodes":[{"id":"a","kind":"shell","run":"sleep 1.6"}]}"#,
            without: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"sleep 1.6"}]}"#,
            differs: |w, wo| {
                rec(w, "a").is_some_and(|r| r.timed_out)
                    && rec(wo, "a").is_some_and(|r| !r.timed_out)
            },
        },
    ),
    (
        "nodes",
        Effect::Observable {
            with: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"echo 1"}]}"#,
            without: r#"{"flow":"t","version":1,"nodes":[]}"#,
            differs: |w, wo| ran(w) != ran(wo),
        },
    ),
];

/// Whether one row holds. `Err` carries why it does not.
fn row_holds(name: &str, effect: &Effect) -> Result<(), String> {
    match effect {
        Effect::Observable {
            with,
            without,
            differs,
        } => {
            if with == without {
                return Err(format!("`{name}`: the two fixtures are identical"));
            }
            let w = run_text(with).map_err(|e| format!("`{name}`: `with` did not run: {e}"))?;
            let wo =
                run_text(without).map_err(|e| format!("`{name}`: `without` did not run: {e}"))?;
            if differs(&w, &wo) {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` is declared Observable but toggling it changed nothing the row \
                     looks for.\nwith: {w:#?}\nwithout: {wo:#?}"
                ))
            }
        }
        Effect::Refused {
            fixture,
            error_contains,
        } => match load(fixture) {
            Ok(_) => Err(format!(
                "`{name}` is declared Refused but its fixture validated"
            )),
            Err(e) if e.contains(error_contains) => Ok(()),
            Err(e) => Err(format!("`{name}`: refusal lacks `{error_contains}`: {e}")),
        },
        Effect::Documentary { reason } if reason.trim().len() >= 20 => Ok(()),
        Effect::Documentary { .. } => {
            Err(format!("`{name}`: a Documentary row needs a real reason"))
        }
    }
}

fn names(table: &[(&str, Effect)]) -> BTreeSet<String> {
    table.iter().map(|(n, _)| (*n).to_string()).collect()
}

fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .expect("serialises as an object")
        .keys()
        .cloned()
        .collect()
}

/// Every `Option` is `Some`, every `bool` true, every collection non-empty.
fn fully_populated_node() -> Node {
    Node {
        depends_on: vec!["a".to_string()],
        timeout: Some(1),
        retry: Some(Retry {
            attempts: Some(1),
            backoff_ms: Some(1),
        }),
        allow_failure: true,
        writes: true,
        run: Some("true".to_string()),
        tool: Some("t".to_string()),
        arguments: Some(json!({})),
        question: Some("q".to_string()),
        options: Some(vec!["o".to_string()]),
        prompt: Some("p".to_string()),
        on: Some("label".to_string()),
        ports: Some(std::iter::once(("o".to_string(), "x".to_string())).collect()),
        escalate_on_near_tie: true,
        near_tie_port: Some("x".to_string()),
        ..Node::new("n", Kind::Decide)
    }
}

fn fully_populated_flow() -> Flow {
    Flow {
        flow: "f".to_string(),
        version: 1,
        description: Some("d".to_string()),
        inputs: vec!["i".to_string()],
        timeout: Some(1),
        nodes: vec![fully_populated_node()],
    }
}

#[test]
fn every_serialised_field_has_a_row() {
    assert_eq!(
        names(NODE_FIELDS).len(),
        NODE_FIELDS.len(),
        "a duplicate Node row"
    );
    assert_eq!(
        names(FLOW_FIELDS).len(),
        FLOW_FIELDS.len(),
        "a duplicate Flow row"
    );

    let node = serde_json::to_value(fully_populated_node()).unwrap();
    assert_eq!(
        keys(&node),
        names(NODE_FIELDS),
        "Node's serialised keys (left) must equal the NODE_FIELDS rows (right): a field with no row"
    );
    let flow = serde_json::to_value(fully_populated_flow()).unwrap();
    assert_eq!(
        keys(&flow),
        names(FLOW_FIELDS),
        "Flow's serialised keys (left) must equal the FLOW_FIELDS rows (right): a field with no row"
    );
}

#[test]
fn every_row_holds() {
    let failures: Vec<String> = NODE_FIELDS
        .iter()
        .chain(FLOW_FIELDS)
        .filter_map(|(name, effect)| row_holds(name, effect).err())
        .collect();
    assert!(
        failures.is_empty(),
        "{} row(s) do not hold:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Root cause of spec 20: `"depend_on"` used to parse, validate and run with no dependency.
#[test]
fn an_unknown_key_is_refused() {
    let typo = r#"{"flow":"t","version":1,"nodes":[
      {"id":"a","kind":"shell","run":"sleep 0.3"},
      {"id":"b","kind":"shell","run":"true","depend_on":["a"]}]}"#;
    let err = Flow::parse(typo).expect_err("a typo of a field must not parse");
    assert!(
        err.to_string().contains("depend_on"),
        "must name the key: {err}"
    );
    assert!(
        load(typo).is_err(),
        "and the CLI's loader must refuse it too"
    );
}

/// The control for `every_row_holds`: a row whose field is inert must FAIL, or the table
/// could pass with every field dead.
#[test]
fn the_control_an_inert_field_fails_its_row() {
    const SAME: &str =
        r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"echo 1"}]}"#;
    let identical = Effect::Observable {
        with: SAME,
        without: SAME,
        differs: |_, _| true,
    };
    assert!(
        row_holds("inert", &identical).is_err(),
        "identical fixtures must not pass"
    );

    let never_differs = Effect::Observable {
        with: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"echo 1"}]}"#,
        without: r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"echo 2"}]}"#,
        differs: |_, _| false,
    };
    assert!(
        row_holds("inert", &never_differs).is_err(),
        "no observed difference must fail"
    );

    let validates = Effect::Refused {
        fixture: SAME,
        error_contains: "anything",
    };
    assert!(
        row_holds("accepted", &validates).is_err(),
        "a Refused row that validates must fail"
    );
}

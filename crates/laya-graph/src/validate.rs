//! Parse-time refusals: everything in [`crate::flow::Flow`] that must be
//! rejected rather than silently tolerated, plus cycle detection.
//!
//! Every refusal is a named [`FlowError`] carrying the offending id — "invalid
//! flow" alone is not acceptable. An unknown node kind is always an error,
//! never a no-op node that "succeeds".

use std::collections::{HashMap, HashSet};

use crate::error::FlowError;
use crate::flow::{Flow, Kind, Node};

/// Validate a parsed flow, refusing rather than warning.
///
/// Checks, in order: duplicate node ids; `depends_on` naming an unknown node;
/// an unsupported `kind`; a `decide` node whose `ports` do not cover its
/// `options` and has no `_` fallback; half a retry policy;
/// `escalate_on_near_tie` without `near_tie_port`; a cycle in `depends_on`.
///
/// # Errors
///
/// Returns the first [`FlowError`] found, named with the offending node id
/// (and, for a cycle, the full chain).
pub fn validate(flow: &Flow) -> Result<(), FlowError> {
    if flow.version != 1 {
        return Err(FlowError::BadVersion {
            version: flow.version,
        });
    }
    if !flow.inputs.is_empty() {
        return Err(FlowError::InputsUnsupported {
            location: "the flow".to_string(),
        });
    }

    let mut seen = HashSet::new();
    for node in &flow.nodes {
        if !seen.insert(node.id.as_str()) {
            return Err(FlowError::DuplicateNodeId {
                id: node.id.clone(),
            });
        }
    }

    let ids: HashSet<&str> = flow.nodes.iter().map(|n| n.id.as_str()).collect();
    for node in &flow.nodes {
        for dep in &node.depends_on {
            if !ids.contains(dep.as_str()) {
                return Err(FlowError::DanglingDependsOn {
                    id: node.id.clone(),
                    missing: dep.clone(),
                });
            }
        }
    }

    for node in &flow.nodes {
        if node.id.contains('/') {
            return Err(FlowError::BadNodeId {
                id: node.id.clone(),
            });
        }
        if let Kind::Other(kind) = &node.kind {
            return Err(FlowError::UnknownKind {
                id: node.id.clone(),
                kind: kind.clone(),
            });
        }
        validate_required(node)?;
        if node.kind == Kind::Decide {
            validate_decide(node, &ids)?;
            crate::placeholder::validate_references(node, flow)?;
        }
        validate_retry(node)?;
    }

    detect_cycle(flow)
}

fn missing(node: &Node, kind: &str, field: &str) -> FlowError {
    FlowError::MissingField {
        id: node.id.clone(),
        kind: kind.to_string(),
        field: field.to_string(),
    }
}

/// Fields a kind cannot work without (spec 20 row 6): an absent `run` used to
/// succeed with an empty command.
fn validate_required(node: &Node) -> Result<(), FlowError> {
    match node.kind {
        Kind::Shell if node.run.is_none() => Err(missing(node, "shell", "run")),
        Kind::Mcp if node.tool.is_none() => Err(missing(node, "mcp", "tool")),
        Kind::Decide if node.question.is_none() => Err(missing(node, "decide", "question")),
        Kind::Decide if node.prompt.is_none() => Err(missing(node, "decide", "prompt")),
        _ => Ok(()),
    }
}

/// The values `decide.on = band` can produce.
const BANDS: [&str; 3] = ["low", "mid", "high"];

/// Whether `node` asks a `noul` (spec 16). Absent is a `choice`; a `noul` must be named, and has
/// no options. Anything else is refused by name, never defaulted.
fn is_noul(node: &Node) -> Result<bool, FlowError> {
    let noul = match node.question_type.as_deref() {
        None | Some("choice") => false,
        Some("noul") => true,
        Some(other) => {
            return Err(FlowError::BadQuestionType {
                id: node.id.clone(),
                value: other.to_string(),
            });
        }
    };
    if noul && node.options.as_ref().is_some_and(|o| !o.is_empty()) {
        return Err(FlowError::NoulWithOptions {
            id: node.id.clone(),
        });
    }
    Ok(noul)
}

fn validate_decide(node: &Node, ids: &HashSet<&str>) -> Result<(), FlowError> {
    if node
        .prompt
        .as_deref()
        .is_some_and(|p| p.contains("$.inputs"))
    {
        return Err(FlowError::InputsUnsupported {
            location: format!("decide node `{}` prompt", node.id),
        });
    }

    let on = match node.on.as_deref() {
        None | Some("label") => "label",
        Some("band") => "band",
        Some(other) => {
            return Err(FlowError::BadOn {
                id: node.id.clone(),
                on: other.to_string(),
            });
        }
    };

    let noul = is_noul(node)?;

    let options = node.options.as_deref().unwrap_or_default();
    let ports = node.ports.as_ref();
    let has_fallback = ports.is_some_and(|p| p.contains_key("_"));

    // What a port key may be: the options when routing on `label` (`yes` / `no` for a noul,
    // which has no options, so completeness would otherwise be vacuous), the band names when
    // routing on `band`.
    let expected: Vec<String> = if on == "band" {
        BANDS.iter().map(|b| (*b).to_string()).collect()
    } else if noul {
        vec!["yes".to_string(), "no".to_string()]
    } else {
        options.to_vec()
    };
    for key in ports.into_iter().flat_map(|p| p.keys()) {
        if key != "_" && !expected.contains(key) {
            let candidates: Vec<&str> = expected.iter().map(String::as_str).collect();
            let hint = crate::flow::nearest_key(key, &candidates)
                .map(|near| format!(", did you mean `{near}`?"))
                .unwrap_or_default();
            return Err(FlowError::UnknownPortKey {
                id: node.id.clone(),
                key: key.clone(),
                on: on.to_string(),
                expected,
                hint,
            });
        }
    }

    let missing_keys: Vec<String> = expected
        .iter()
        .filter(|key| !ports.is_some_and(|p| p.contains_key(key.as_str())))
        .cloned()
        .collect();
    // Completeness is a `label` rule: every option needs a port or a fallback.
    // A band routing names the bands it cares about and may leave the rest out.
    if on == "label" && !missing_keys.is_empty() && !has_fallback {
        return Err(FlowError::IncompletePorts {
            id: node.id.clone(),
            missing: missing_keys,
        });
    }

    if node.escalate_on_near_tie && node.near_tie_port.is_none() {
        return Err(FlowError::MissingNearTiePort {
            id: node.id.clone(),
        });
    }
    for (key, target) in ports.into_iter().flatten() {
        if !ids.contains(target.as_str()) {
            return Err(FlowError::PortTargetMissing {
                id: node.id.clone(),
                port: key.clone(),
                target: target.clone(),
            });
        }
    }
    if node.escalate_on_near_tie || node.near_tie_port.is_some() {
        return Err(FlowError::NotImplemented {
            id: node.id.clone(),
            field: if node.escalate_on_near_tie {
                "escalate_on_near_tie"
            } else {
                "near_tie_port"
            }
            .to_string(),
            until: "until near-tie routing exists; its target would run on EVERY run".to_string(),
            instead: "Remove the flag and the `near_tie_port` node, and route on `band` instead"
                .to_string(),
        });
    }
    for (field, set) in [
        ("timeout", node.timeout.is_some()),
        ("allow_failure", node.allow_failure),
    ] {
        if set {
            return Err(FlowError::NotImplemented {
                id: node.id.clone(),
                field: field.to_string(),
                until: "dead on a `decide` until spec 17".to_string(),
                instead: "Remove it from the decide node; a flow-level `timeout` still applies"
                    .to_string(),
            });
        }
    }

    Ok(())
}

fn validate_retry(node: &Node) -> Result<(), FlowError> {
    if let Some(retry) = &node.retry
        && retry.attempts.is_some() != retry.backoff_ms.is_some()
    {
        return Err(FlowError::HalfRetryPolicy {
            id: node.id.clone(),
        });
    }
    if node.retry.is_some() {
        return Err(FlowError::NotImplemented {
            id: node.id.clone(),
            field: "retry".to_string(),
            until: "it was declared but inert — the run fails on the first error; a later spec will implement it for non-`decide`, non-`writes` nodes".to_string(),
            instead: "Wrap the command in a shell loop, or remove `retry`".to_string(),
        });
    }
    Ok(())
}

/// Depth-first cycle detection over `depends_on`, tri-colour marked so it
/// always returns — a node already `InProgress` ends the search with a named
/// chain instead of recursing again, so a cycle cannot turn this into a hang.
fn detect_cycle(flow: &Flow) -> Result<(), FlowError> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mark {
        Unvisited,
        InProgress,
        Done,
    }

    fn visit<'a>(
        id: &'a str,
        by_id: &HashMap<&'a str, &'a Node>,
        marks: &mut HashMap<&'a str, Mark>,
        path: &mut Vec<&'a str>,
    ) -> Result<(), FlowError> {
        marks.insert(id, Mark::InProgress);
        path.push(id);

        if let Some(node) = by_id.get(id) {
            for dep in &node.depends_on {
                let dep = dep.as_str();
                match marks.get(dep).copied().unwrap_or(Mark::Unvisited) {
                    Mark::InProgress => {
                        let start = path.iter().position(|p| *p == dep).unwrap_or(0);
                        let mut chain: Vec<&str> = path.iter().skip(start).copied().collect();
                        chain.push(dep);
                        return Err(FlowError::Cycle {
                            chain: chain.join(" -> "),
                        });
                    }
                    Mark::Done => {}
                    Mark::Unvisited => visit(dep, by_id, marks, path)?,
                }
            }
        }

        marks.insert(id, Mark::Done);
        path.pop();
        Ok(())
    }

    let by_id: HashMap<&str, &Node> = flow.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut marks: HashMap<&str, Mark> = flow
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), Mark::Unvisited))
        .collect();
    let mut path: Vec<&str> = Vec::new();

    for node in &flow.nodes {
        let id = node.id.as_str();
        if marks.get(id).copied().unwrap_or(Mark::Unvisited) == Mark::Unvisited {
            visit(id, &by_id, &mut marks, &mut path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::flow::Retry;

    /// Seeded LCG (Knuth MMIX constants). A failure reproduces from the printed seed.
    struct Lcg(u64);
    impl Lcg {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(self.0 >> 33).unwrap_or(0) % n.max(1)
        }
    }

    /// R4: seeded byte mutation of the real fixtures through parse -> validate -> lower.
    /// The property is "a typed error or Ok, never a panic" — a panic names its seed.
    #[test]
    fn mutated_flows_are_refused_or_accepted_never_panic() {
        const SEED: u64 = 0x13F1_0001;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut seeds: Vec<(String, Vec<u8>)> = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("fixtures dir") {
            let path = entry.expect("entry").path();
            if path.extension().is_some_and(|e| e == "json") {
                seeds.push((
                    path.display().to_string(),
                    std::fs::read(&path).expect("read fixture"),
                ));
            }
        }
        seeds.sort();
        assert!(seeds.len() >= 2, "the fixtures must be found: {seeds:?}");

        let run_one = |text: &str| -> Result<bool, FlowError> {
            let flow = Flow::parse(text)?;
            validate(&flow)?;
            crate::lower(&flow)?;
            Ok(true)
        };
        // CONTROL: the unmutated fixtures take the full Ok path, so "refused" below means a
        // mutation did it, not that the pipeline refuses everything.
        for (name, bytes) in &seeds {
            let text = String::from_utf8(bytes.clone()).expect("utf-8 fixture");
            assert!(run_one(&text).is_ok(), "the real fixture {name} must pass");
        }

        let mut rng = Lcg(SEED);
        let (mut accepted, mut refused) = (0_u32, 0_u32);
        for case in 0..2_000_u32 {
            let (name, base) = &seeds[rng.below(seeds.len())];
            let mut bytes = base.clone();
            for _ in 0..=rng.below(4) {
                let at = rng.below(bytes.len());
                match rng.below(4) {
                    0 => bytes[at] = u8::try_from(rng.below(256)).unwrap_or(0),
                    1 => {
                        bytes.remove(at);
                    }
                    2 => bytes.insert(at, bytes[at]),
                    // Structural bytes change shapes and types, which random bytes rarely do.
                    _ => bytes[at] = b"0t[{\"n,:"[rng.below(8)],
                }
                if bytes.is_empty() {
                    bytes.push(b'{');
                }
            }
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let outcome = std::panic::catch_unwind(|| run_one(&text)).unwrap_or_else(|_| {
                panic!("panicked: seed {SEED:#x}, case {case}, from {name}, input:\n{text}")
            });
            if outcome.is_ok() {
                accepted += 1;
            } else {
                refused += 1;
            }
        }
        assert!(
            accepted > 20 && refused > 20,
            "accepted {accepted}, refused {refused}"
        );
    }

    fn shell(id: &str, depends_on: &[&str]) -> Node {
        Node {
            run: Some("true".to_string()),
            depends_on: depends_on.iter().map(|s| (*s).to_string()).collect(),
            ..Node::new(id, Kind::Shell)
        }
    }

    fn decide(id: &str, options: &[&str], ports: &[(&str, &str)]) -> Node {
        Node {
            question: Some("q?".to_string()),
            // Spec 20 row 6: a `decide` requires `prompt`; the helper carries one so the
            // tests below keep testing what they name, not the new refusal.
            prompt: Some("state".to_string()),
            options: Some(options.iter().map(|s| (*s).to_string()).collect()),
            ports: Some(
                ports
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            ),
            ..Node::new(id, Kind::Decide)
        }
    }

    fn flow(nodes: Vec<Node>) -> Flow {
        Flow {
            flow: "t".to_string(),
            version: 1,
            description: None,
            inputs: Vec::new(),
            timeout: None,
            nodes,
        }
    }

    /// Spec 16 R4: a `noul` has no `options`, so the completeness check would be vacuous; its
    /// ports must cover `yes` and `no` (or carry `_`), and a typo is refused by name.
    #[test]
    fn a_noul_decide_needs_port_coverage() {
        let noul = |ports: &[(&str, &str)]| Node {
            question_type: Some("noul".to_string()),
            ..decide("d", &[], ports)
        };
        let check = |ports: &[(&str, &str)]| {
            validate(&flow(vec![noul(ports), shell("x", &[]), shell("y", &[])]))
        };

        // Only `yes` covered, no fallback: `no` would be a run-time UnknownNode.
        let err = check(&[("yes", "x")]).expect_err("`no` is uncovered");
        assert!(
            matches!(&err, FlowError::IncompletePorts { missing, .. } if missing == &["no"]),
            "{err:?}"
        );
        // A typo of `yes` is refused by name, with the hint.
        let err = check(&[("yess", "x"), ("no", "y")]).expect_err("typo");
        assert!(
            matches!(&err, FlowError::UnknownPortKey { key, .. } if key == "yess"),
            "{err:?}"
        );
        // The near-miss that must still validate: full coverage, and one answer + fallback.
        assert!(check(&[("yes", "x"), ("no", "y")]).is_ok());
        assert!(check(&[("yes", "x"), ("_", "y")]).is_ok());
    }

    /// Spec 16 R2: an unknown `question_type` is refused naming the node and the value; a
    /// `noul` that declares `options` is a contradiction and is refused.
    #[test]
    fn validate_refuses_an_unknown_question_type_by_name() {
        let with = |qt: &str, options: &[&str]| Node {
            question_type: Some(qt.to_string()),
            ..decide("d", options, &[("_", "x")])
        };
        let check = |n: Node| validate(&flow(vec![n, shell("x", &[])]));

        let err = check(with("yolo", &["a"])).expect_err("unknown type");
        assert!(
            matches!(&err, FlowError::BadQuestionType { id, value } if id == "d" && value == "yolo"),
            "{err:?}"
        );
        let err = check(with("noul", &["a"])).expect_err("noul with options");
        assert!(
            matches!(&err, FlowError::NoulWithOptions { id } if id == "d"),
            "{err:?}"
        );
        // Controls: the accepted shapes still validate.
        assert!(check(with("choice", &["a"])).is_ok());
        assert!(check(with("noul", &[])).is_ok());
    }

    #[test]
    fn a_duplicate_node_id_is_refused() {
        let f = flow(vec![shell("a", &[]), shell("a", &[])]);
        let err = validate(&f).expect_err("duplicate id must be refused");
        assert!(
            matches!(err, FlowError::DuplicateNodeId { ref id } if id == "a"),
            "message must name `a`, got {err:?}"
        );
    }

    #[test]
    fn a_dangling_depends_on_is_refused() {
        let f = flow(vec![shell("a", &["ghost"])]);
        let err = validate(&f).expect_err("dangling depends_on must be refused");
        assert!(
            matches!(err, FlowError::DanglingDependsOn { ref missing, .. } if missing == "ghost"),
            "message must name `ghost`, got {err:?}"
        );
    }

    #[test]
    fn a_decide_without_a_fallback_is_refused() {
        let f = flow(vec![decide(
            "classify",
            &["a", "b", "c"],
            &[("a", "x"), ("b", "y")],
        )]);
        let err = validate(&f).expect_err("incomplete ports with no fallback must be refused");
        assert!(
            matches!(err, FlowError::IncompletePorts { ref id, .. } if id == "classify"),
            "the port set is incomplete, got {err:?}"
        );
    }

    #[test]
    fn half_a_retry_policy_is_refused() {
        let node = Node {
            retry: Some(Retry {
                attempts: Some(3),
                backoff_ms: None,
            }),
            ..shell("a", &[])
        };
        let f = flow(vec![node]);
        let err = validate(&f).expect_err("half a retry policy is a retry storm");
        assert!(matches!(err, FlowError::HalfRetryPolicy { ref id } if id == "a"));
    }

    #[test]
    fn escalate_on_near_tie_without_a_port_is_refused() {
        let node = Node {
            escalate_on_near_tie: true,
            ..decide(
                "classify",
                &["a", "b"],
                &[("a", "x"), ("b", "y"), ("_", "z")],
            )
        };
        let f = flow(vec![node]);
        let err = validate(&f).expect_err("escalate_on_near_tie needs a destination");
        assert!(matches!(err, FlowError::MissingNearTiePort { ref id } if id == "classify"));
    }

    #[test]
    fn a_dependency_cycle_is_refused() {
        let f = flow(vec![shell("a", &["b"]), shell("b", &["a"])]);
        let err = validate(&f).expect_err("a cycle must be refused, and must return");
        let FlowError::Cycle { chain } = err else {
            panic!("expected Cycle, got {err:?}");
        };
        assert!(chain.contains('a') && chain.contains('b'), "got {chain}");
    }

    #[test]
    fn an_unknown_node_kind_is_refused() {
        let json = r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shelll"}]}"#;
        let f = Flow::parse(json).expect("bad kind must still PARSE");
        let err = validate(&f).expect_err("an unknown kind must never become a no-op node");
        assert!(matches!(err, FlowError::UnknownKind { ref kind, .. } if kind == "shelll"));
    }

    #[test]
    fn agent_and_quorum_are_not_supported_kinds() {
        for kind in ["agent", "quorum"] {
            let json =
                format!(r#"{{"flow":"t","version":1,"nodes":[{{"id":"a","kind":"{kind}"}}]}}"#);
            let f = Flow::parse(&json).expect("must still parse");
            let err = validate(&f).expect_err("v1 implements neither agent nor quorum");
            assert!(matches!(err, FlowError::UnknownKind { kind: ref k, .. } if k == kind));
        }
    }

    #[test]
    fn the_control_a_valid_flow_is_accepted() {
        let f = flow(vec![
            decide(
                "classify",
                &["a", "b"],
                &[("a", "x"), ("b", "y"), ("_", "z")],
            ),
            shell("x", &[]),
            shell("y", &[]),
            shell("z", &[]),
        ]);
        assert!(validate(&f).is_ok(), "a well-formed flow must validate");
    }

    // ---- spec 20: the format is a contract ---------------------------------

    /// Parse + validate JSON text, returning the refusal's rendered message.
    fn refusal(json: &str) -> Result<(), String> {
        let f = Flow::parse(json).map_err(|e| e.to_string())?;
        validate(&f).map_err(|e| e.to_string())
    }

    fn decide_json(on: &str, ports: &str) -> String {
        format!(
            r#"{{"flow":"t","version":1,"nodes":[
              {{"id":"d","kind":"decide","question":"q?","prompt":"p","options":["yes","no"]{on},"ports":{ports}}},
              {{"id":"x","kind":"shell","run":"true"}},{{"id":"y","kind":"shell","run":"true"}}]}}"#
        )
    }

    #[test]
    fn the_control_a_known_key_is_accepted() {
        let json = r#"{"flow":"t","version":1,"nodes":[
          {"id":"a","kind":"shell","run":"true"},
          {"id":"b","kind":"shell","run":"true","depends_on":["a"]}]}"#;
        assert_eq!(
            refusal(json),
            Ok(()),
            "strictness must not refuse valid flows"
        );
    }

    #[test]
    fn an_unknown_key_names_the_nearest_known_key() {
        let json = r#"{"flow":"t","version":1,"nodes":[
          {"id":"a","kind":"shell","run":"true"},
          {"id":"b","kind":"shell","run":"true","depend_on":["a"]}]}"#;
        let err = refusal(json).expect_err("a typo of depends_on must be refused");
        assert!(err.contains("depend_on"), "must name the key: {err}");
        assert!(
            err.contains("did you mean `depends_on`"),
            "must offer the fix: {err}"
        );
    }

    #[test]
    fn an_unknown_flow_level_key_is_refused_by_name() {
        let json = r#"{"flow":"t","version":1,"descripton":"x","nodes":[]}"#;
        let err = refusal(json).expect_err("a flow-level typo must be refused");
        assert!(
            err.contains("descripton") && err.contains("description"),
            "{err}"
        );
    }

    #[test]
    fn an_unknown_port_key_is_refused_by_name() {
        let err = refusal(&decide_json("", r#"{"yes":"x","noo":"y","_":"y"}"#))
            .expect_err("`noo` is not an option");
        assert!(err.contains("noo"), "must name the key: {err}");
    }

    #[test]
    fn a_port_target_that_names_no_node_is_refused_by_name() {
        let err = refusal(&decide_json("", r#"{"yes":"x","no":"nosuch"}"#))
            .expect_err("a target naming no node must be refused");
        assert!(err.contains("nosuch"), "must name the target: {err}");
    }

    #[test]
    fn a_band_routing_with_label_port_keys_is_refused_by_name() {
        let err = refusal(&decide_json(r#","on":"band""#, r#"{"yes":"x","no":"y"}"#))
            .expect_err("band routing with label keys sends everything to the fallback");
        // BTreeMap order: `no` is the first offending key.
        assert!(err.contains("band") && err.contains("`no`"), "{err}");
    }

    #[test]
    fn the_control_band_routing_with_band_port_keys_is_accepted() {
        let ports = r#"{"high":"x","mid":"x","low":"y"}"#;
        assert_eq!(refusal(&decide_json(r#","on":"band""#, ports)), Ok(()));
    }

    #[test]
    fn a_misspelled_on_is_refused_by_name() {
        let err = refusal(&decide_json(r#","on":"bnad""#, r#"{"yes":"x","no":"y"}"#))
            .expect_err("`bnad` silently meant label");
        assert!(err.contains("bnad") && err.contains("band"), "{err}");
    }

    #[test]
    fn the_control_a_shell_with_a_run_is_accepted() {
        let json = r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell","run":"echo a"}]}"#;
        assert_eq!(refusal(json), Ok(()));
    }

    #[test]
    fn a_shell_without_a_run_is_refused_by_name() {
        let json = r#"{"flow":"t","version":1,"nodes":[{"id":"a","kind":"shell"}]}"#;
        let err = refusal(json).expect_err("an absent run must not succeed with an empty command");
        assert!(err.contains("`a`") && err.contains("run"), "{err}");
    }

    #[test]
    fn an_mcp_without_a_tool_is_refused_by_name() {
        let json = r#"{"flow":"t","version":1,"nodes":[{"id":"m","kind":"mcp"}]}"#;
        let err = refusal(json).expect_err("an mcp needs a tool");
        assert!(err.contains("`m`") && err.contains("tool"), "{err}");
    }

    #[test]
    fn a_decide_without_a_question_or_prompt_is_refused_by_name() {
        for (field, json) in [
            (
                "question",
                r#"{"flow":"t","version":1,"nodes":[{"id":"d","kind":"decide","prompt":"p","options":["a"],"ports":{"_":"d"}}]}"#,
            ),
            (
                "prompt",
                r#"{"flow":"t","version":1,"nodes":[{"id":"d","kind":"decide","question":"q","options":["a"],"ports":{"_":"d"}}]}"#,
            ),
        ] {
            let err = refusal(json).expect_err("a decide needs both");
            assert!(err.contains(field), "{field}: {err}");
        }
    }

    #[test]
    fn inputs_and_dollar_inputs_are_refused_by_name() {
        let with_inputs = r#"{"flow":"t","version":1,"inputs":["doc"],"nodes":[
          {"id":"a","kind":"shell","run":"true"}]}"#;
        let err = refusal(with_inputs).expect_err("`inputs` is not implemented");
        assert!(err.contains("inputs"), "{err}");

        let with_ref = r#"{"flow":"t","version":1,"nodes":[
          {"id":"d","kind":"decide","question":"q","prompt":"$.inputs.doc","options":["a"],"ports":{"_":"d"}}]}"#;
        let err = refusal(with_ref).expect_err("`$.inputs.doc` is sent as literal text");
        assert!(err.contains("$.inputs"), "{err}");
    }

    #[test]
    fn retry_is_refused_and_names_the_alternative() {
        let json = r#"{"flow":"t","version":1,"nodes":[
          {"id":"a","kind":"shell","run":"true","retry":{"attempts":3,"backoff_ms":10}}]}"#;
        let err = refusal(json).expect_err("retry is inert, so it is refused");
        assert!(err.contains("retry") && err.contains("shell loop"), "{err}");
    }

    #[test]
    fn near_tie_routing_is_refused_as_not_implemented() {
        let json = r#"{"flow":"t","version":1,"nodes":[
          {"id":"d","kind":"decide","question":"q","prompt":"p","options":["a"],
           "ports":{"a":"x"},"escalate_on_near_tie":true,"near_tie_port":"x"},
          {"id":"x","kind":"shell","run":"true"}]}"#;
        let err = refusal(json).expect_err("the target would run unconditionally");
        assert!(
            err.contains("escalate_on_near_tie") && err.contains("not implemented"),
            "{err}"
        );
    }

    #[test]
    fn timeout_and_allow_failure_on_a_decide_are_refused_until_spec_17() {
        for field in ["\"timeout\":5", "\"allow_failure\":true"] {
            let json = format!(
                r#"{{"flow":"t","version":1,"nodes":[
                  {{"id":"d","kind":"decide","question":"q","prompt":"p","options":["a"],"ports":{{"a":"x"}},{field}}},
                  {{"id":"x","kind":"shell","run":"true"}}]}}"#
            );
            let err = refusal(&json).expect_err("dead on a decide");
            assert!(err.contains("spec 17"), "{field}: {err}");
        }
    }

    #[test]
    fn the_control_timeout_and_allow_failure_on_a_shell_are_accepted() {
        let json = r#"{"flow":"t","version":1,"nodes":[
          {"id":"a","kind":"shell","run":"true","timeout":5,"allow_failure":true}]}"#;
        assert_eq!(refusal(json), Ok(()));
    }

    #[test]
    fn a_version_other_than_1_is_refused() {
        let json = r#"{"flow":"t","version":999,"nodes":[]}"#;
        let err = refusal(json).expect_err("version 999 is not a format this engine reads");
        assert!(err.contains("999"), "{err}");
    }

    #[test]
    fn a_slash_in_a_node_id_is_refused() {
        let json =
            r#"{"flow":"t","version":1,"nodes":[{"id":"a/route","kind":"shell","run":"true"}]}"#;
        let err = refusal(json).expect_err("`/` collides with the synthetic route id");
        assert!(err.contains("a/route"), "{err}");
    }

    /// Spec 19: the refusal names the id and says why (`/` is the synthetic route id's separator),
    /// and a near-miss id without a slash still takes the normal path.
    #[test]
    fn a_node_id_containing_a_slash_is_refused_by_name() {
        let json = r#"{"flow":"t","version":1,"nodes":[
          {"id":"x/route","kind":"shell","run":"true"}]}"#;
        let err = refusal(json).expect_err("`/` collides with the synthetic route id");
        assert!(err.contains("x/route"), "must name the id: {err}");
        assert!(err.contains('/'), "must give the reason: {err}");
        let near_miss = r#"{"flow":"t","version":1,"nodes":[
          {"id":"x_route","kind":"shell","run":"true"}]}"#;
        assert_eq!(refusal(near_miss), Ok(()));
    }

    /// Spec 19's control: the refusals must not reject the working gate-then-ask example.
    #[test]
    fn the_control_a_well_formed_flow_is_accepted() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../_docs/02-guides/wc-classify-chained.flow.json");
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        let flow = crate::expand(&raw, &crate::Library::new()).expect("expand");
        assert_eq!(validate(&flow), Ok(()));
    }

    #[test]
    fn a_subflow_node_is_refused_naming_the_missing_library() {
        let root: serde_json::Value = serde_json::from_str(
            r#"{"flow":"t","version":1,"nodes":[{"id":"v","kind":"subflow","uses":"build"}]}"#,
        )
        .unwrap();
        let err = crate::expand(&root, &crate::Library::new()).expect_err("no library");
        let msg = err.to_string();
        assert!(msg.contains("subflow") && msg.contains("library"), "{msg}");
        assert!(
            !msg.contains("depends_on"),
            "must not blame depends_on: {msg}"
        );
    }

    /// Spec 20's control, run through the same loader the CLI uses: every committed flow —
    /// including `wc-classify-chained`, the gate-then-ask pattern — still validates.
    #[test]
    fn the_control_every_committed_flow_still_validates() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut paths = Vec::new();
        for dir in ["_docs/02-guides", "crates/laya-graph/tests/fixtures"] {
            for entry in std::fs::read_dir(root.join(dir)).expect("dir") {
                let path = entry.expect("entry").path();
                if path.extension().is_some_and(|e| e == "json") {
                    paths.push(path);
                }
            }
        }
        paths.sort();
        assert!(
            paths.iter().any(|p| p
                .to_string_lossy()
                .contains("wc-classify-chained.flow.json")),
            "the chained flow must be among them: {paths:?}"
        );
        for path in &paths {
            let raw: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("json");
            let flow = crate::expand(&raw, &crate::Library::new())
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            validate(&flow).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        }
    }
}

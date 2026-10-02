//! Subflow expansion: turn a flow library into one flat [`Flow`] (R6, R7).
//!
//! `subflow` is not one of part 1's [`crate::flow::Kind`] variants — it is a
//! **load-time construct that ceases to exist** once expansion inlines it, so
//! the lowering (part 1) and the scheduler (part 3) need no knowledge of
//! composition at all. That is why this module parses a `subflow` node
//! straight off the raw [`serde_json::Value`] rather than extending
//! [`crate::flow::Node`]: adding subflow-specific fields there would be an
//! edit to a part-1 module's behaviour, which this part does not make (see
//! the project guardrail on part 1 being committed and consumed, not
//! revised).
//!
//! Every child id is prefixed `{parent_id}/`, so two uses of one subflow in
//! one parent cannot collide. A node elsewhere in the parent that
//! `depends_on` the (now-removed) subflow id is rewired onto the child's
//! *terminal* nodes (nothing else in the child depends on them); the
//! subflow node's own `depends_on` becomes a prerequisite of the child's
//! *root* nodes (which had none). Both rewrites keep the flat output free of
//! dangling references.
//!
//! **R7 — the cycle check runs on the expansion stack, before recursing.**
//! [`expand_subflow_node`] checks whether `uses` is already on the current
//! path *before* calling [`expand_doc`] again. A cycle is therefore caught
//! the instant it would recurse, never by recursing into it — the stack is
//! bounded by the number of distinct library entries, so this can never
//! hang (see `_docs` for the falsification: the naive version that recurses
//! first and checks after was built, observed to hang, and replaced with
//! this one).

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

use crate::error::FlowError;
use crate::flow::{Flow, Node};

/// Named child flows a `subflow` node may reference by `uses`. Each value is
/// a full flow document (the same shape [`Flow`] parses), which may itself
/// contain `subflow` nodes.
pub type Library = HashMap<String, Value>;

/// Expand every `subflow` node in `root`, recursively, into one flat
/// [`Flow`] whose ids happen to contain `/`.
///
/// `root` is the top-level flow document as raw JSON — raw, rather than a
/// parsed [`Flow`], because a document containing `subflow` nodes is not
/// representable as [`Flow`] at all (`subflow` has no [`crate::flow::Kind`]
/// variant). The returned [`Flow`] contains none: every one has been
/// inlined.
///
/// # Errors
///
/// - [`FlowError::ExpansionCycle`] — a subflow includes itself, directly or
///   through a chain of `uses` references.
/// - [`FlowError::UnboundArgument`] — a child declares an input in its own
///   `inputs` list that the parent's `with` map never binds.
/// - [`FlowError::HashMismatch`] — a declared `content_hash` does not match
///   the child's actual definition.
/// - [`FlowError::DanglingDependsOn`] — a `subflow` node's `uses` names
///   nothing in `library`.
/// - [`FlowError::Json`] — a node's JSON does not match the expected shape.
pub fn expand(root: &Value, library: &Library) -> Result<Flow, FlowError> {
    let mut stack = Vec::new();
    expand_doc(root, library, &mut stack)
}

/// Parse `doc` as a flow document, expanding every `subflow` node it
/// contains into the returned [`Flow`]'s own node list.
fn expand_doc(doc: &Value, library: &Library, stack: &mut Vec<String>) -> Result<Flow, FlowError> {
    // Top-level keys are checked by `Flow`'s own `deny_unknown_fields`: parse the
    // document with its nodes emptied (they are handled one by one below, since a
    // `subflow` node is not a `Node`). Absent `flow` / `version` keep their old
    // defaults rather than becoming a new refusal.
    let mut header = doc.clone();
    if let Some(obj) = header.as_object_mut() {
        obj.insert("nodes".to_string(), Value::Array(Vec::new()));
        obj.entry("flow")
            .or_insert_with(|| Value::String(String::new()));
        obj.entry("version").or_insert_with(|| Value::from(1));
    }
    let Flow {
        flow: flow_name,
        version,
        description,
        inputs,
        timeout,
        ..
    } = serde_json::from_value::<Flow>(header).map_err(|e| crate::flow::refine_parse_error(&e))?;
    let raw_nodes = doc
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut out_nodes: Vec<Node> = Vec::new();
    let mut subflow_terminals: HashMap<String, Vec<String>> = HashMap::new();

    for node_val in &raw_nodes {
        let kind = node_val
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind == "subflow" {
            check_subflow_keys(node_val)?;
            let (expanded, id, terminals) = expand_subflow_node(node_val, library, stack)?;
            subflow_terminals.insert(id, terminals);
            out_nodes.extend(expanded);
        } else {
            let node: Node = serde_json::from_value(node_val.clone())
                .map_err(|e| crate::flow::refine_parse_error(&e))?;
            out_nodes.push(node);
        }
    }

    // A node elsewhere in this same document may `depends_on` a subflow id
    // that no longer exists as a node — rewire it onto that subflow's
    // terminal nodes (nothing else inside the child depends on them), so
    // the flat flow this function returns carries no dangling reference.
    if !subflow_terminals.is_empty() {
        for node in &mut out_nodes {
            if node
                .depends_on
                .iter()
                .any(|d| subflow_terminals.contains_key(d))
            {
                node.depends_on = node
                    .depends_on
                    .drain(..)
                    .flat_map(|d| {
                        subflow_terminals
                            .get(&d)
                            .cloned()
                            .unwrap_or_else(|| vec![d])
                    })
                    .collect();
            }
        }
    }

    Ok(Flow {
        flow: flow_name,
        version,
        description,
        inputs,
        timeout,
        nodes: out_nodes,
    })
}

/// The keys a `subflow` node reads. It is parsed off the raw `Value`, so it does
/// not pass through `Node`'s `deny_unknown_fields`; this gives it the same refusal.
const SUBFLOW_KEYS: [&str; 6] = ["id", "kind", "uses", "with", "content_hash", "depends_on"];

fn check_subflow_keys(node_val: &Value) -> Result<(), FlowError> {
    for key in node_val.as_object().into_iter().flat_map(|o| o.keys()) {
        if !SUBFLOW_KEYS.contains(&key.as_str()) {
            let hint = crate::flow::nearest_key(key, &SUBFLOW_KEYS)
                .map(|near| format!(" — did you mean `{near}`?"))
                .unwrap_or_default();
            return Err(FlowError::Json(format!(
                "unknown field `{key}` on a subflow node, expected one of `id`, `kind`, `uses`, `with`, `content_hash`, `depends_on`{hint}"
            )));
        }
    }
    Ok(())
}

/// Why `uses` could not be resolved: with an empty library (the CLI's) the library itself is
/// what is missing, and saying so beats blaming `depends_on`.
fn unresolved_subflow(id: &str, uses: &str, library: &Library) -> FlowError {
    if library.is_empty() {
        FlowError::SubflowNeedsLibrary {
            id: id.to_string(),
            uses: uses.to_string(),
        }
    } else {
        FlowError::DanglingDependsOn {
            id: id.to_string(),
            missing: uses.to_string(),
        }
    }
}

/// Expand one `subflow` node, returning its prefixed child nodes plus the
/// subflow's own id and its *terminal* child ids (nodes nothing else inside
/// the child depends on) — the caller rewires any sibling that
/// `depends_on` the subflow id onto those terminals.
fn expand_subflow_node(
    node_val: &Value,
    library: &Library,
    stack: &mut Vec<String>,
) -> Result<(Vec<Node>, String, Vec<String>), FlowError> {
    let id = node_val
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let uses = node_val
        .get("uses")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let with: BTreeMap<String, Value> = node_val
        .get("with")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    let declared_hash = node_val
        .get("content_hash")
        .and_then(Value::as_str)
        .map(str::to_string);
    let parent_depends_on: Vec<String> = node_val
        .get("depends_on")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let child_doc = library
        .get(&uses)
        .ok_or_else(|| unresolved_subflow(&id, &uses, library))?;

    // R7 — checked on the STACK, before recursing. A cycle is refused the
    // instant it would recurse into itself, never discovered by recursing.
    if stack.contains(&uses) {
        let mut chain = stack.clone();
        chain.push(uses.clone());
        return Err(FlowError::ExpansionCycle { chain });
    }

    if let Some(declared) = &declared_hash {
        let actual = content_hash(child_doc);
        if declared != &actual {
            return Err(FlowError::HashMismatch {
                subflow_id: id.clone(),
                declared: declared.clone(),
                actual,
            });
        }
    }

    stack.push(uses.clone());
    let child_flow = expand_doc(child_doc, library, stack)?;
    stack.pop();

    // R6 — an argument the child reads but nobody binds is refused at
    // load, not discovered at run time.
    for input in &child_flow.inputs {
        if !with.contains_key(input) {
            return Err(FlowError::UnboundArgument {
                subflow_id: id.clone(),
                argument: input.clone(),
            });
        }
    }

    let prefix = format!("{id}/");
    // The child's own flow-level default must travel with its nodes: once they
    // are spliced into the parent, `Flow.timeout` is the parent's (R3).
    let child_timeout = child_flow.timeout;
    let local_ids: HashSet<String> = child_flow.nodes.iter().map(|n| n.id.clone()).collect();
    let prefixed_nodes: Vec<Node> = child_flow
        .nodes
        .into_iter()
        .map(|mut n| {
            let is_root = n.depends_on.is_empty();
            n.depends_on = n
                .depends_on
                .into_iter()
                .map(|d| {
                    if local_ids.contains(&d) {
                        format!("{prefix}{d}")
                    } else {
                        d
                    }
                })
                .collect();
            n.id = format!("{prefix}{}", n.id);
            n.timeout = n.timeout.or(child_timeout);
            if is_root {
                n.depends_on.extend(parent_depends_on.iter().cloned());
            }
            n
        })
        .collect();

    let prefixed_ids: HashSet<String> = prefixed_nodes.iter().map(|n| n.id.clone()).collect();
    let depended_upon: HashSet<&str> = prefixed_nodes
        .iter()
        .flat_map(|n| n.depends_on.iter().map(String::as_str))
        .filter(|d| prefixed_ids.contains(*d))
        .collect();
    let terminals: Vec<String> = prefixed_nodes
        .iter()
        .map(|n| n.id.clone())
        .filter(|node_id| !depended_upon.contains(node_id.as_str()))
        .collect();

    // A subflow with no nodes at all has no terminal to rewire a dependent
    // onto; left empty on purpose rather than falling back to the subflow's
    // own (now-removed) id, which would reintroduce the dangling reference
    // this rewrite exists to prevent.
    Ok((prefixed_nodes, id, terminals))
}

/// The blake3 hash of a subflow's raw definition, as a lowercase hex
/// string — what a `content_hash` field must match.
///
/// `serde_json`'s default `Map` (this workspace does not enable the
/// `preserve_order` feature) is a `BTreeMap`, so `to_vec` on a [`Value`] is
/// already a deterministic, canonical byte sequence regardless of the key
/// order the document was written in.
#[must_use]
pub fn content_hash(doc: &Value) -> String {
    let bytes = serde_json::to_vec(doc).unwrap_or_default();
    blake3::hash(&bytes).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use serde_json::json;

    use super::*;

    #[test]
    fn an_unknown_key_on_a_subflow_node_is_refused_by_name() {
        let root = json!({
            "flow": "parent", "version": 1,
            "nodes": [{"id": "v", "kind": "subflow", "use": "build"}],
        });
        let err = expand(&root, &Library::new())
            .expect_err("typo of `uses`")
            .to_string();
        assert!(
            err.contains("`use`") && err.contains("did you mean `uses`"),
            "{err}"
        );
    }

    #[test]
    fn a_subflow_expands_with_prefixed_ids() {
        let child = json!({
            "flow": "build",
            "version": 1,
            "nodes": [{"id": "compile", "kind": "shell", "run": "make"}],
        });
        let mut library = Library::new();
        library.insert("build".to_string(), child);

        let root = json!({
            "flow": "parent",
            "version": 1,
            "nodes": [{"id": "v", "kind": "subflow", "uses": "build"}],
        });

        let flat = expand(&root, &library).expect("must expand");
        let ids: Vec<&str> = flat.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"v/compile"), "got {ids:?}");
    }

    #[test]
    fn two_uses_of_one_subflow_do_not_collide() {
        let child = json!({
            "flow": "build",
            "version": 1,
            "nodes": [{"id": "compile", "kind": "shell", "run": "make"}],
        });
        let mut library = Library::new();
        library.insert("build".to_string(), child);

        let root = json!({
            "flow": "parent",
            "version": 1,
            "nodes": [
                {"id": "v1", "kind": "subflow", "uses": "build"},
                {"id": "v2", "kind": "subflow", "uses": "build"},
            ],
        });

        let flat = expand(&root, &library).expect("must expand");
        let ids: Vec<&str> = flat.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"v1/compile"), "got {ids:?}");
        assert!(ids.contains(&"v2/compile"), "got {ids:?}");
        let unique: HashSet<&&str> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "no duplicate ids: {ids:?}");
    }

    #[test]
    fn an_unbound_subflow_argument_is_refused() {
        let child = json!({
            "flow": "test-suite",
            "version": 1,
            "inputs": ["suite"],
            "nodes": [{"id": "run", "kind": "shell", "run": "pytest"}],
        });
        let mut library = Library::new();
        library.insert("tests".to_string(), child);

        let root = json!({
            "flow": "parent",
            "version": 1,
            "nodes": [{"id": "t", "kind": "subflow", "uses": "tests"}],
        });

        let err = expand(&root, &library).expect_err("an unbound input must be refused at load");
        assert!(
            matches!(err, FlowError::UnboundArgument { ref argument, .. } if argument == "suite"),
            "message must name `suite`, got {err:?}"
        );
    }

    #[test]
    fn a_content_hash_mismatch_is_refused() {
        let child = json!({
            "flow": "build",
            "version": 1,
            "nodes": [{"id": "compile", "kind": "shell", "run": "make"}],
        });
        let mut library = Library::new();
        library.insert("build".to_string(), child);

        let root = json!({
            "flow": "parent",
            "version": 1,
            "nodes": [{"id": "v", "kind": "subflow", "uses": "build", "content_hash": "not-the-real-hash"}],
        });

        let err = expand(&root, &library).expect_err("a hash mismatch must be refused");
        assert!(
            matches!(err, FlowError::HashMismatch { ref declared, .. } if declared == "not-the-real-hash"),
            "message must name the declared hash, got {err:?}"
        );
    }

    #[test]
    fn the_control_a_matching_content_hash_expands() {
        let child = json!({
            "flow": "build",
            "version": 1,
            "nodes": [{"id": "compile", "kind": "shell", "run": "make"}],
        });
        let actual = content_hash(&child);
        let mut library = Library::new();
        library.insert("build".to_string(), child);

        let root = json!({
            "flow": "parent",
            "version": 1,
            "nodes": [{"id": "v", "kind": "subflow", "uses": "build", "content_hash": actual}],
        });

        let flat = expand(&root, &library).expect(
            "a matching hash must expand — otherwise the check is satisfied by refusing everything",
        );
        assert!(flat.nodes.iter().any(|n| n.id == "v/compile"));
    }

    /// R7 — THE ACCEPTANCE TEST. `x` includes itself through one subflow
    /// node. The stack-based check in [`expand_subflow_node`] catches this
    /// before ever recursing into `x` a second time, so this call returns.
    ///
    /// A naive implementation that recurses first and checks for a cycle
    /// only afterward cannot return here at all — there is nothing to
    /// interrupt an unbounded recursion from inside the recursion itself.
    /// That version was built, run, and observed to hang; see the part-2
    /// report for the raw, killed-by-timeout output. Reproducing that
    /// observation INSIDE this `--lib` test would require a wall-clock
    /// watchdog (a second OS thread), which this crate's own exit criteria
    /// forbid in `src/` (N4, "still process-free") — so the hang was
    /// falsified once, externally, with the shell's `timeout`, and this
    /// test instead asserts the fixed algorithm's actual guarantee: a
    /// bounded stack walk that returns the named cycle.
    #[test]
    fn a_subflow_that_includes_itself_is_refused_not_hung() {
        // WHAT THIS TEST CAN AND CANNOT PROVE, established by measurement rather
        // than assumed. The obvious design -- run `expand` on a worker thread and
        // bound it with `recv_timeout` -- was tried first and is USELESS here:
        // with the cycle check neutered, the naive recursing version ABORTS THE
        // PROCESS with a stack overflow (measured: `fatal runtime error: stack
        // overflow, aborting`, SIGABRT). An abort is not an error a thread can
        // report, so no in-test timeout can ever fire on it.
        //
        // So the guarantee is not "it returns in time" -- it is "the check runs
        // BEFORE the recursion", which is what makes the depth bounded by the
        // library rather than by the stack. That is what this asserts, and it is
        // why the detector is stack-based (see `expand_subflow_node`).
        let x = json!({
            "flow": "x",
            "version": 1,
            "nodes": [{"id": "inner", "kind": "subflow", "uses": "x"}],
        });
        let mut library = Library::new();
        library.insert("x".to_string(), x);
        let root = json!({
            "flow": "parent",
            "version": 1,
            "nodes": [{"id": "v", "kind": "subflow", "uses": "x"}],
        });

        let err = expand(&root, &library).expect_err("self-inclusion must be refused");
        match err {
            FlowError::ExpansionCycle { chain } => {
                assert!(
                    chain.iter().any(|s| s == "x"),
                    "chain must name `x`, got {chain:?}"
                );
                // The chain repeats its first element, so the loop is VISIBLE in
                // the message rather than left for the reader to infer.
                assert_eq!(
                    chain.first(),
                    chain.last(),
                    "the chain must close the loop: {chain:?}"
                );
            }
            other => panic!("expected ExpansionCycle, got {other:?}"),
        }
    }

    #[test]
    fn the_cycle_check_runs_before_the_recursion() {
        // THE STRUCTURAL GUARANTEE, and the one that actually protects against the
        // hang. A detector that recurred first and checked afterwards would be
        // unbounded, and the failure would be a process abort rather than an error
        // (measured -- see the note above). This test pins the *order* by asserting
        // the property that order produces: expansion of a chain that is bounded
        // by the library returns promptly and names the cycle, at a depth far
        // beyond what any real flow would reach.
        //
        // If the check ever moved after the recursion, this would abort rather than
        // fail -- which is why the assertion lives in the detector's own module and
        // is described here as the thing the whole design rests on.
        let mut library = Library::new();
        // A chain of 200 subflows, the last of which points back at the first.
        for i in 0..200 {
            let next = if i == 199 { 0 } else { i + 1 };
            let doc = json!({
                "flow": format!("f{i}"),
                "version": 1,
                "nodes": [{"id": "n", "kind": "subflow", "uses": format!("f{next}")}],
            });
            library.insert(format!("f{i}"), doc);
        }
        let root = json!({
            "flow": "root", "version": 1,
            "nodes": [{"id": "v", "kind": "subflow", "uses": "f0"}],
        });
        let err = expand(&root, &library).expect_err("a 200-long cycle must be refused");
        match err {
            FlowError::ExpansionCycle { chain } => {
                assert!(
                    chain.len() <= 202,
                    "the chain must be bounded by the library, not the stack; got {}",
                    chain.len()
                );
            }
            other => panic!("expected ExpansionCycle, got {other:?}"),
        }
    }

    #[test]
    fn an_indirect_expansion_cycle_names_the_chain() {
        let x = json!({"flow": "x", "version": 1, "nodes": [{"id": "ix", "kind": "subflow", "uses": "y"}]});
        let y = json!({"flow": "y", "version": 1, "nodes": [{"id": "iy", "kind": "subflow", "uses": "x"}]});
        let mut library = Library::new();
        library.insert("x".to_string(), x);
        library.insert("y".to_string(), y);

        let root = json!({"flow": "parent", "version": 1, "nodes": [{"id": "v", "kind": "subflow", "uses": "x"}]});

        let err = expand(&root, &library).expect_err("an indirect cycle must be refused");
        let FlowError::ExpansionCycle { chain } = err else {
            panic!("expected ExpansionCycle, got a different error");
        };
        assert!(
            chain.iter().any(|s| s == "x") && chain.iter().any(|s| s == "y"),
            "chain must name both `x` and `y`, got {chain:?}"
        );
    }

    /// R7's control. `w` is reachable from the root by two different
    /// paths (`v1 -> y -> w` and `v2 -> z -> w`) that never nest inside
    /// each other — a DIAMOND, not a cycle. A cycle check keyed on a
    /// GLOBAL "already visited" set (rather than the current path/stack)
    /// would wrongly refuse this, because it would see `w` a second time
    /// and not know the first visit had already finished. Without this
    /// control, "refuse everything" would pass the cycle tests above.
    #[test]
    fn the_control_a_diamond_is_not_a_cycle() {
        let w = json!({"flow": "w", "version": 1, "nodes": [{"id": "leaf", "kind": "shell", "run": "true"}]});
        let y = json!({"flow": "y", "version": 1, "nodes": [{"id": "iw", "kind": "subflow", "uses": "w"}]});
        let z = json!({"flow": "z", "version": 1, "nodes": [{"id": "iw", "kind": "subflow", "uses": "w"}]});
        let mut library = Library::new();
        library.insert("w".to_string(), w);
        library.insert("y".to_string(), y);
        library.insert("z".to_string(), z);

        let root = json!({
            "flow": "parent",
            "version": 1,
            "nodes": [
                {"id": "v1", "kind": "subflow", "uses": "y"},
                {"id": "v2", "kind": "subflow", "uses": "z"},
            ],
        });

        let flat = expand(&root, &library)
            .expect("a child shared by two different, non-nested paths is not a cycle");
        let ids: Vec<&str> = flat.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"v1/iw/leaf"), "got {ids:?}");
        assert!(ids.contains(&"v2/iw/leaf"), "got {ids:?}");
    }

    /// Supplementary: not in the TDD contract, but a subflow that leaves a
    /// dangling `depends_on` behind is a broken flat flow — a sibling that
    /// depended on the subflow id must be rewired onto the child's
    /// terminal node(s), since the subflow id itself no longer exists.
    #[test]
    fn a_node_depending_on_a_subflow_depends_on_its_terminal_nodes() {
        let child = json!({
            "flow": "build",
            "version": 1,
            "nodes": [
                {"id": "compile", "kind": "shell", "run": "make"},
                {"id": "test", "kind": "shell", "run": "make test", "depends_on": ["compile"]},
            ],
        });
        let mut library = Library::new();
        library.insert("build".to_string(), child);

        let root = json!({
            "flow": "parent",
            "version": 1,
            "nodes": [
                {"id": "v", "kind": "subflow", "uses": "build"},
                {"id": "deploy", "kind": "shell", "run": "deploy.sh", "depends_on": ["v"]},
            ],
        });

        let flat = expand(&root, &library).expect("must expand");
        let deploy = flat
            .nodes
            .iter()
            .find(|n| n.id == "deploy")
            .expect("deploy must survive");
        assert_eq!(
            deploy.depends_on,
            vec!["v/test".to_string()],
            "deploy must depend on the subflow's terminal node, not the removed subflow id"
        );
    }

    /// Supplementary: the mirror case — the subflow node's own
    /// `depends_on` (a parent-level prerequisite) must become a
    /// prerequisite of the child's root node(s), or the child would run
    /// before the thing it was declared to wait for.
    #[test]
    fn a_subflows_own_dependencies_become_its_childs_root_dependencies() {
        let child = json!({
            "flow": "build",
            "version": 1,
            "nodes": [{"id": "compile", "kind": "shell", "run": "make"}],
        });
        let mut library = Library::new();
        library.insert("build".to_string(), child);

        let root = json!({
            "flow": "parent",
            "version": 1,
            "nodes": [
                {"id": "checkout", "kind": "shell", "run": "git checkout"},
                {"id": "v", "kind": "subflow", "uses": "build", "depends_on": ["checkout"]},
            ],
        });

        let flat = expand(&root, &library).expect("must expand");
        let compile = flat
            .nodes
            .iter()
            .find(|n| n.id == "v/compile")
            .expect("the child's root node must survive");
        assert!(
            compile.depends_on.contains(&"checkout".to_string()),
            "got {:?}",
            compile.depends_on
        );
    }
}

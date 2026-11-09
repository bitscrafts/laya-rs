#!/usr/bin/env python3
"""Wire margin + the configurable noul threshold into the MCP surface.

THREE CHANGES, and the third is a defect the review pattern keeps producing:

1. **`decide` emits `margin` and `near_tie`** (spec 04 F3), so a graph author can write
   `route on = "$.payload.near_tie"` or rely on the band demotion.

2. **`gate` refuses to auto-allow on a near tie.** A gate that cannot judge must ask; a
   near tie is exactly "the head could not judge this clearly".

3. **The hardcoded `0.5` noul label is replaced by `Policy::noul_is_true()`.** The policy
   field is validated and configurable from TOML, and it was called only from its own
   tests — so editing `noul_threshold` in a config file changed nothing about the label a
   caller received. Same shape as `laya-head` and `server_capabilities`.
"""
import pathlib

p = pathlib.Path("/projects/laya-rs/crates/laya-mcp/src/lib.rs")
t = p.read_text()


def sub(old, new, count=1):
    global t
    if old not in t:
        raise SystemExit(f"ANCHOR NOT FOUND:\n{old[:220]}")
    t = t.replace(old, new, count)


# ---- 1. the noul label uses the configured threshold ----------------------
sub('''        QuestionType::Noul => {
            let noul = entry.get("noul").and_then(serde_json::Value::as_f64);
            let value = match noul {
                Some(v) if v >= 0.5 => "yes".to_string(),
                Some(_) => "no".to_string(),
                None => "unknown".to_string(),
            };
            (value, Vec::new(), Vec::new(), noul, None)
        }''',
'''        QuestionType::Noul => {
            let noul = entry.get("noul").and_then(serde_json::Value::as_f64);
            // The threshold comes from the POLICY, not from a literal here. It was
            // hardcoded to 0.5 while `Policy::noul_threshold` was validated, configurable
            // from TOML, and read only by its own tests — so a config change had no effect
            // on the label a caller got. Passing `policy` in is what makes the config real.
            let value = match noul {
                Some(v) if policy.noul_is_true(v) => "yes".to_string(),
                Some(_) => "no".to_string(),
                None => "unknown".to_string(),
            };
            (value, Vec::new(), Vec::new(), noul, None)
        }''')

# `answer_from_laya` needs the policy
sub("fn answer_from_laya(",
    "fn answer_from_laya(\n    policy: &laya_core::Policy,")
# its callers pass one
t = t.replace("answer_from_laya(&id, entry, &declared)", "answer_from_laya(policy, &id, entry, &declared)")
t = t.replace("answer_from_laya(id, entry, &declared)", "answer_from_laya(policy, id, entry, &declared)")
t = t.replace('answer_from_laya(id, &entry, &declared)', 'answer_from_laya(policy, id, &entry, &declared)')

p.write_text(t)
print("  noul label -> Policy::noul_is_true (the config field is now load-bearing)")

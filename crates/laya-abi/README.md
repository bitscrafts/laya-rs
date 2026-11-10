# laya-abi

The stable types every other crate in the workspace shares: the **question**, the typed
**answer**, and the route-shaped **decision payload**. No I/O, no logic beyond (de)serialisation.

## What is in it

- `Question`: `id`, `type` (`noul`, `choice` or `score`), `text`, and `options` for `choice`/`score`.
- The typed answer for each question type, with its probabilities and confidence.
- The decision payload a graph's `route` node matches on (`label`, `band`, `confidence`).

Everything downstream depends only on these shapes, which is what lets a caller swap the model, or a
mock, without touching call sites.

## Depends on

`serde`, `serde_json`. Nothing else, and no other workspace crate. Keep it that way: this is the
bottom of the dependency graph.

## Test

```bash
cargo test -p laya-abi
```

Offline; no model.

## See also

[`../../_docs/03-reference/GRAPH-FORMAT-CONTRACT.md`](../../_docs/03-reference/GRAPH-FORMAT-CONTRACT.md)
for the payload contract, and the [workspace README](../../README.md).

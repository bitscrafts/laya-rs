# Security

Report a vulnerability privately through
[GitHub security advisories](https://github.com/bitscrafts/laya-rs/security/advisories/new),
not as a public issue.

In scope: the `laya`, `laya-mcp` and `laya-gate` binaries and the crates in this repository. The
decision model's weights and the model server are separate projects; report those upstream.

`laya-mcp` speaks MCP over stdio only and opens no network listener. `laya-backbone` is the only
crate that makes network calls, and only to the endpoint you configure.

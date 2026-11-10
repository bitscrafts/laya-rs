# Install and first answer

From nothing to a verified `laya decide` answer on a Linux machine. Every command below was run
against the real binary and the real model files; the output shown is what it printed.

You need: Rust (the repo pins `1.95.0` in `rust-toolchain.toml`, so `rustup` fetches it), `curl`,
and a `llama-server` from [llama.cpp](https://github.com/ggml-org/llama.cpp). About 1 GB of disk
for the model files.

What was run, and what was not. Run exactly as printed, with the output shown: the checksum
command (step 1, with a wrong-hash control that fails), `laya doctor`, `laya decide`, the three
failure cases and `laya graph validate` (steps 4 to 7). The build in step 3 ran from an existing
checkout with the same command. Not run end to end: the `curl` downloads in step 1 (each URL
returns HTTP 200 and the published hashes match the files used) and the `llama-server` start in
step 2 (its flags are the reference deployment's and were checked against `llama-server --help`;
the server in the test run had been started earlier).

## 1. Get the model files

Laya is a separate work under its own licence (Apache-2.0); this repository does not contain or
redistribute it. Three files, from the model repository `fr0stbit3/laya-gguf`:

```bash
MODELS="$HOME/laya-models"; mkdir -p "$MODELS"; cd "$MODELS"
BASE=https://huggingface.co/fr0stbit3/laya-gguf/resolve/main
curl -L -O $BASE/laya-Q8_0.gguf          # the backbone, 421 MB
curl -L -O $BASE/laya-head.safetensors   # the decision head, 106 MB
curl -L -O $BASE/tokenizer.json          # 3.6 MB
```

Check the two files that decide the answer. A different tokenizer produces different token ids,
so every number downstream is wrong while still looking reasonable.

```bash
sha256sum -c <<'EOF'
c1ec428e034614c9373ebcf3fdc255d8eafcc04be9b2f0c2ffeef92bb1361b83  laya-head.safetensors
6c8aaa9a542084f2457eab775d4eeb51f92a70c0fd9de28d5edb0ddec3c08d30  tokenizer.json
EOF
```

The head's hash is the one the model repository publishes. The tokenizer's is the one this
project's tests pin.

## 2. Serve the backbone

The backbone is an embedding model, so the server runs with embeddings on and no pooling:

```bash
llama-server -m "$MODELS/laya-Q8_0.gguf" --embeddings --pooling none \
  -ub 2048 -b 2048 --host 127.0.0.1 --port 8083
```

`-ub`/`-b` of 2048 let a long state fit in one batch. Leave this running and use another shell.
These are the flags the reference deployment runs; the flag names were checked against
`llama-server --help`.

## 3. Build

```bash
git clone https://github.com/bitscrafts/laya-rs && cd laya-rs
cargo build --release -p laya-cli -p laya-mcp
```

That produced `target/release/laya` (5.0 MB), `laya-mcp` (3.6 MB) and `laya-gate` (1.1 MB) in
3 min 39 s on a cold cache. Put them on your `PATH`, or call them from `target/release/`.

## 4. Check readiness

```bash
laya doctor --url http://127.0.0.1:8083
```

```text
laya: READY
  endpoint  : http://127.0.0.1:8083
  /health   : ok
  /v1/models: ok (serving the expected model)
  head      : ok (sidecar at /run/laya/sidecar.sock)
```

The `head` line concerns the optional Python sidecar that `laya-mcp` and `laya-gate` still use;
`laya decide` does not need it.

## 5. Ask a question

Always pass `--head`, `--tokenizer` and `--url`. The built-in defaults are the author's machine
paths and will not exist on yours.

```bash
laya decide \
  --state 'a unit test was added for the parser' \
  --question '{"id":"q1","type":"choice","text":"what kind of change is this?","options":["code","test","environment"]}' \
  --url http://127.0.0.1:8083 \
  --head "$MODELS/laya-head.safetensors" \
  --tokenizer "$MODELS/tokenizer.json"
```

It printed (1.2 s, release build, including loading the head):

```text
{"answer":{"id":"q1","type":"choice","value":"test","probabilities":[0.1476,0.8313,0.0211],
 "options":["code","test","environment"],"confidence":0.5291},"temperature":1.7601,
 "bucket_clamped":false,"bucket":"choice:3-5","logit_gap":3.042,"tokens":28,"rows":28,
 "marker_pos":[12,14,16],"act_probability":1.0}
q1 = "test"  confidence 0.5291  margin_gap 3.0420  T 1.7602 (choice:3-5)  28 tokens
```

Add `--timeout-ms 5000` to bound the backbone call. Without it the default is two minutes, which is
too long to wait if the server is hung.

## 6. When it fails

Each failure is a distinct non-zero exit code and a message that names the cause.

| What you did | Exit | Message |
|---|---|---|
| backbone not running | 3 | `backbone: cannot reach the backbone: http://127.0.0.1:9/embedding: io: Connection refused` |
| wrong `--head` path | 2 | `cannot read the head at …: No such file or directory` |
| wrong `--tokenizer` path | 2 | `cannot read the tokenizer at …: No such file or directory` |

## 7. Try a graph

A flow is a DAG whose edges are drawn by a person; Laya only supplies a value.

```bash
laya graph validate _docs/02-guides/guess-the-number.flow.json
# laya graph validate: _docs/02-guides/guess-the-number.flow.json is valid
```

More: [Running Laya and llama.cpp](RUNNING-LAYA.md) for a supervised deployment, and the
[README](../../README.md) for what the seven MCP tools do.

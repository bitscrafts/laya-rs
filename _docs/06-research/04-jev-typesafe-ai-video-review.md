# 04 — JEV / TypeSafe AI ("System One") — the upstream video, reviewed for ideas

**Source**: "JEV / TypeSafe AI API & Rust - Let make a rust lb" — Jeremy Chone (`@jchone`),
live 2026-09-19, **2h44m (9,863 s)**, `https://www.youtube.com/live/pU73lYF7R1I`.
Transcript fetched 2026-10-01 via `yt-dlp` (`jev.en.vtt`, 1.2 MB auto-captions → 123,498 chars
after de-duplicating the rolling-caption overlap). Every quote below is verbatim from that
transcript with a timestamp; STT artifacts are corrected in brackets where the intended token
is unambiguous (`"CIS one"` = the crate; `"Jev"` appears only ~4 times — he calls it
**"type safe AI"** or **"system one"** throughout).

**Why this is filed here**: it is the *upstream origin* of the idea this repo implements —
the author is demonstrating the vendor API and building his own Rust client for it, live.
`01-design.md` §12 already cites the downstream video (IndyDevDan, "Ten Levels of Jev") and
`disler/ten-levels-of-jev`; this is the layer beneath those.

---

## 0. THE HEADLINE — what the video does NOT say

Four premises this workspace has been carrying are **not supported** by the source:

1. **No determinism claim exists.** The words `deterministic`, `constrain`, `grammar`,
   `logit`, `temperature` and `reinforcement` occur **zero times** in 2h44m. What he describes
   is a **probability** every time:

   > "I give a context I give you the option and you give me **which one is the highest
   > probability**" (00:26:46)

   > "give one probability of from 0 to one what is the chance that is a true" (00:43:26)

   So "JEV is deterministic" is an **inference about the vendor**, not something stated here.
   This repo's determinism is a property of *its own Rust head* (spec 03: reproduces PyTorch to
   4.05e-6), and must not be justified by citing this video.
2. **The "type safety" is mostly HIS CLIENT CRATE, not the API.** The vendor endpoint is a JSON
   POST whose `state`, `instructions` and `criteria` are all open-ended (string, object or
   array). He says so himself: *"even if instruction is open-ended, could be a JSON value
   because by the nature it could be a value, we could type it a little bit"* (00:46:36).
   The typing is his `from_state`/`Question` enum/`Option<f64>` layer written the day before.
3. **The price advantage is weak, and he says so.** His own comparison: the vendor's model at
   *"13 bucks"* vs DeepSeek Flash input at *"15 cents"* — *"they're twisting the number. The
   other guy is at 15 cents. So, it's not that cheap, but it's might be much faster"* (02:15:49),
   and *"it's a little bit expensive. They're not too far apart"* (02:20:53). Speed, not price,
   is the honest claim.
4. **Quality is untested by the author**: *"the quality I haven't really tested because I wanted
   to creating my own crates"* (00:53:31). No accuracy, latency or error-rate number appears
   anywhere. And on the video itself: *"this live was about me adding that and I decided to do
   it live. Yeah. So, there's no really deeper stuff."* (01:28:16)

**Runtime actually about JEV: ≈40 min of 164 (≈25%)**; ≈60–70 min (≈40%) if the live writing of
his client crate counts. The rest is chat/career opinion and a long demo of his **own unrelated
coding-agent harness** ("procoder"/workbench/code-map/sub-agents/loop). Numbers from that
harness ("code map 70 files, 5 cents, 2 seconds"; "82k → 19 kilobyte context reduction") are
**not JEV** and should not be absorbed as if they were.

---

## 1. WHAT IT IS (his own framing)

> "the new type safe AI. So it's a pretty cool kind of API. **It doesn't replace LLM at all**
> but it fit it fills a gap. Yeah. **Where we need speed and we need less reasoning and we
> don't need to write code. It's just decision making.**" (00:00:22–00:00:35)

> "type safe compared to a AI model here ... is something that have a good quick start is rather
> to have an LLM that knows how to write English code and everything and reason and stuff. It's
> just a one that you you give it some context some state what they call it and then you give
> him **multiple choices** in a way and you say **give me the probability of each choice**. So
> **it doesn't write English. It only give back numbers** in a way." (02:13:50)

Explicit non-goals (each stated): does not write code (00:27:01); does not take chat/context
messages (00:09:34); *"it doesn't solve everything. So I have few use cases"* (00:13:12).

He concedes an LLM can emulate it: *"you could almost use an LLM model to do exactly the same
thing ... but because it's specialized on that this service then it gives speed and price
benefits"* (00:11:38).

---

## 2. THE API SHAPE (the part worth comparing against `route_payload.rs`)

**Request** (00:09:32) — a plain POST; **no chat messages**:

> "Basically we have a **state** and the state here could be a **string or could be even a
> structure**. You have the **model**. ... then you have your **questions** and then what you
> do is you say okay I have all these questions and then you have a **key** for your questions."

Per-question fields: **key** (he also calls it "ID"/name), **type**, **instructions**,
**criteria**. `instructions` and `criteria` may each be a string, object or array (00:17:04,
00:27:45).

**Question types — three, and note what they are** (00:10:22):

> "they can be **choice**, they can be **score**, they can be **null** which is null is just a
> yes no it's probability and so basically what type does is for each question is just respond
> a number in a way it doesn't respond in English"

`null` = boolean yes/no expressed as one probability (00:42:47): *"give one probability of from
0 to one what is the chance that is a true"*.

**Response** (00:58:45) — *"the answers will always be the key and will always be null choice or
score"*; observed in the live demo (00:40:14) as a JSON object where a `null` question returned
*"the probability is uh 98"* → i.e. keyed answers carrying the type and a probability.

**Limits stated**: **32k token context** (00:12:14, 00:44:42, 02:32:11); **255 criteria max**
(02:26:05). **The question KEY is not tokenised** — measured by him live (00:45:12): *"if I put
the ID the question ID to one ... the input token is 303 ... and then 303 again. So the nice
thing here is that it doesn't eat input token the ID which means that now we can use any ID we
want."*

### Side-by-side with this repo

| | JEV (vendor) | `laya-rs` |
|---|---|---|
| question kinds | `choice` \| `score` \| `null` | `QTYPES = {choice:0, score:1, noul:2}` |
| answer | a probability per question | `label` + `band` + `confidence` + `margin` + `near_tie` + `saturated` |
| what routing matches | the answer | `$.payload.label`, exact, with a **mandatory catch-all** |
| low confidence / near-equal | **never discussed** (`confidence`, `fallback`, `tie` = 0 hits) | `near_tie`, `margin`, `band`; escalate-on-near-tie is a *validated* flow option |
| determinism | **not claimed** | claimed **and measured** for the head (spec 03) |

**The taxonomy converging independently is the strongest signal in the video**: his three
question types are *character-for-character* the three this repo already implements. That is
independent corroboration of the shape, not new information.

**Where this repo is already strictly stronger** (keep these — do not regress toward the video):
the payload carries calibration and a near-tie signal where JEV has a bare probability, and a
mandatory catch-all makes an undefined edge impossible, which is the property `01-design.md`
§F13 already claims is *"stronger than the video claims for Jev"* — this review confirms that
claim holds, because the video makes no such claim at all.

---

## 3. IDEAS WORTH CONSIDERING (the actual yield)

Ranked by how cheaply they could be adopted. None of these are implemented; this is research.

1. **`criteria` as a per-question field, distinct from `instructions`.** JEV separates *what to
   assess* from *the rule to assess it by*, and allows `criteria` to be a list (max 255). Our
   flow node carries `question` + `options` + `ports`; an explicit `criteria` list would let an
   author attach named decision rules that the run record can echo back — which fits the
   `node record carries the label and band that produced the routing` pattern already in part 4.
   *Cost*: a new optional flow field + validation. *Risk*: low; it does not change the ABI.

2. **Ids are free — use them.** Measured above: the question key costs **zero** input tokens, so
   arbitrary UUIDs are affordable. Relevant to `laya-store`'s `Record::identity()` and to node
   ids in flows: there is no token-cost argument against long, unambiguous identifiers.

3. **`Option<f64>` for anything a service might not report.** His reasoning on cost: *"F64 but I
   say **option of F64** because my intuition tells me that this model ... might not have the
   cost for whatever reason"* (00:57:58). Our payload has no cost field at all; if one is ever
   added, make it optional rather than assuming the backend reports it.

4. **Client-side concurrency to live under a context cap.** His plan for the 32k limit is
   chunking plus *"run the request in a concurrency 32"* (02:26:31), explicitly deferred. We
   already fan out — `ask_files` with a measured limit and the bounded scheduler from spec 07 —
   so this is confirmation that the workaround shape is the right one, not a new idea.

5. **Rust client patterns he argues for, which match our lint bar.** *"Do not use anyhow. Stop
   using anyhow. Do not use unwrap. Don't do do not use expect. Always have a question mark"*
   (00:52:03) — the same doctrine this workspace enforces mechanically via
   `unwrap_used`/`expect_used`/`panic` at `deny`. Also: `Arc<Inner>` for an **immutable,
   clone-friendly client** (01:29:45) so it can be shared without cloning the whole struct; and
   generic `Into` on `from_*` constructors, accepting the monomorphisation cost deliberately
   (00:32:01). He rejects builder-derive macros on purpose: *"I like to micromanage the shape of
   my public interfaces"* (01:51:46).

6. **The "required type tag" failure has an exact analogue in our validator.** His crate
   *kabooms* when a question has no `type` — *"my error here, union tag, it's like, hey, what is
   a type? I don't know what to do"* (00:46:08). That is the same lesson as our
   `an_unknown_node_kind_is_refused`: a missing or unknown discriminant must be a **hard,
   named refusal**, never a defaulted value. Independent confirmation of a decision already
   made.

7. **No-tie-break is a gap we do not share, but it names a risk.** JEV describes nothing for
   "the top probability is low" or "two choices are near-equal". We have `near_tie` and a
   `margin`. Worth stating in our docs that ours is the deliberate answer to an edge the
   upstream design leaves open — so a future reader does not "simplify" it away.

---

## 4. WHAT NOT TO TAKE FROM THIS VIDEO

- **Do not cite it for determinism** (§0.1). It is a probability API.
- **Do not cite it for price.** The author himself calls the comparison weak.
- **Do not cite it for quality.** He never measured it, and says so.
- **Do not absorb the harness numbers** (code-map costs, context-reduction figures). Different
  system, demonstrated in the same stream.
- **Do not treat it as a specification.** The API is his *own maturing crate* and changing —
  *"I might change the API quite a bit"* (00:30:21), *"don't do pull request right now because
  I'm matching the API"* (01:09:54). It is a data point, not a standard.

## 5. UNRESOLVED (do not guess)

Auth scheme / headers / base URL; exact response key names; what a `score` answer carries (named
three times, never shown); what the "2 kilobyte token input limit" applies to; whether the
observed probability moved (0.98 → 0.03?) in the French-language test; whether multiple models
are selectable; the literal name of his crate and its examples (STT renders them "CIS one" /
"C one" / "C2").

---

## 6. VERDICT

**Confirmation, not revelation.** The video corroborates the architecture this repo already
implements — the three question types match exactly, and the "required discriminant or hard
error" lesson matches our validator — and it exposes that two things we might have cited it for
(**determinism** and **cheapness**) are not in it. The genuinely new, cheap ideas are the
separate `criteria` field (1), the free-id observation (2) and `Option` for unreported values
(3). The most useful sentence in the whole stream is the author's own disclaimer, at 01:28:16:
*"there's no really deeper stuff."*

---

## 7. CORRECTION AND ADDITIONS (2026-10-01, later) — the REPOSITORIES, which the first review missed

The review above was built from the **video transcript only**. That was incomplete: the video is a
vendor walkthrough, while **the repositories carry the design**. Two matter, and one of them names a
defect we actually have. Added here rather than rewriting the sections above, so the first pass and
what it missed are both on the record.

### 7.1 The ecosystem is not one crate — it is a typed layer over a transport layer

| crate | role | notable |
|---|---|---|
| `typesafe-ai-sdk` (aoprisan), `typesafeai-sdk`, `typesafe-rs` | **transport**: HTTP, retries, error classification, forward-compatible decoding | mirrors the official Python SDK (`typesafe-sdk` 0.6.0); `Client` is explicitly *cheap to clone; clones share the connection pool* |
| **`s1-rs`** (AbdelStark) — `s1`, `s1-derive`, `s1-test`, `s1-cli` | **the type system**: derive macros, typed answers, gating policies, test helpers | PRD v1.0, 16 Sep 2026, explicitly a **draft**; target v0.1 in 3 weeks |

`s1-rs`'s thesis, in its own words: *"Today every System One integration in every language is
stringly typed … The wire format delivers typed outputs; the developer experience still hands you
strings. Rust's type system can close that last mile completely."*

That is a **sharper statement of the problem than anything in the video**, and it is the sentence
this project should be measured against.

### 7.2 The gap it names, and where WE have it

The PRD's motivating example:

```rust
let dept = resp.answer("department").and_then(|a| a.choice()); // Option<&str>
match dept { Some("billing") => …, Some("technical") => …, _ => … } // typo = silent bug
```

> "Question keys, option labels, and score levels are strings. **Rename a variant and nothing fails
> to compile.** Add an option to the criteria and no `match` warns you."

**Measured in this repo on 2026-10-01 — a flow whose `ports` mis-keys one option:**

```
options: ["positive", "negative"]
ports  : ["positve", "negative", "_"]          // "positve" is a typo
"positive" has a port? False

$ laya graph validate typo-flow.json
laya graph validate: … is valid                exit 0   ← ACCEPTED
```

The flow validates, the render emits an edge labelled `positve`, and **`branch_positive` is
unreachable**. The mandatory catch-all is what keeps this from crashing — so it degrades *quietly*
instead of loudly, which is the worse failure. **A typo in a port key silently orphans a branch.**

This is a genuine defect, found by reading the repositories rather than the video, and it is the
highest-value item either source produced. It should become a spec:

- validate must refuse a `decide` node when **every declared option does not have a port**, and must
  refuse a port key that matches neither an option nor `"_"` — with a **control** proving the
  valid flow still passes (a negative is satisfied by refusing everything);
- `laya graph render` should mark unreachable branches, so the state is visible before a run;
- and the flow author should be able to *see* the option↔port mapping as a checked relation rather
  than trusting two hand-written string lists to agree.

The longer-term version of the same idea is what `s1-rs` proposes with derives:

```rust
#[derive(Choice)]
enum Department { #[s1("Payment, invoice, refund…")] Billing, … }

#[derive(Questions)]
struct Triage { department: Department, urgent: bool }
```

…where the enum *is* the question and a renamed variant **fails to compile**. We do not need the
derive machinery (`01-design.md` §F13 already chose author-time validation), but the *principle*
— make a mismatched label impossible rather than merely discouraged — is one this repo already
believes in and has not finished applying to flow port keys.

### 7.3 What the repositories add that the video did not

- **Confidence is first-class, and gated.** `ChoiceAnswer` carries `selected label, per-label
  probabilities, confidence`; `ScoreAnswer` carries `probability-weighted level, legend, per-level
  probabilities, confidence`. The SDK example gates on it: `if dept.confidence > 0.5 { route to … }`.
  The video never showed a confidence number being acted on; the SDK does. **Our payload already
  carries `confidence`, `band`, `margin` and `near_tie`** — this is independent convergence on the
  same shape, and it means the design choice is not idiosyncratic.
- **`Score` is resolved**: a probability-weighted level plus a legend and per-level probabilities,
  with confidence. The video named `score` three times and never explained it; that is now answered.
- **Retry semantics are specified, not improvised**: exponential backoff with *subtractive jitter*,
  `Retry-After`/`retry-after-ms` honoured, a max retry count, and a **total time budget that stops
  before a sleep that would exceed it**. Worth comparing against our own retry story.
- **Client-cloning discipline**: "cheap to clone; clones share the connection pool" — the same
  conclusion the video reached with `Arc<Inner>`, now stated as an API contract.
- **Forward-compatible response decoding** is an explicit goal, and `cost: Option<f64>` is the
  mechanism the earlier section already noted.
- **Batch, moderation and routing examples ship as first-class examples** — the SDK treats the
  "ask 150 functions which are needed" use case as a documented pattern, not a remark in the last
  minute of a stream.
- **`wiremock` as a dev-dependency** for testing the client without a network. Directly relevant to
  our own test-hardening question: a transport can be tested against a fake server.

### 7.4 Verdict, revised

The earlier verdict — *"confirmation, not revelation"* — **holds for the video and is wrong for the
ecosystem**. The repositories contain a real problem statement (`s1-rs`), a real defect in our code
(the unvalidated option↔port relation), and specifications the video only gestured at (score
semantics, retry policy, confidence gating). Reading the transcript is enough to say *what the idea
is*; it is not enough to say *where ours is weak*. The lesson worth keeping: **when a talk links a
repository, the repository is the primary source and the talk is the trailer.**

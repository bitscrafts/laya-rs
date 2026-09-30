#!/usr/bin/env python3
"""Generate the differential fixture for the pure-Rust tokenizer + `build_sequence` port.

## Why this is the right oracle

Tokenisation and sequence building are **discrete**: there is no tolerance to spend. So the
oracle is not "close enough", it is the reference implementation itself. This script drives the
**real** `tokenizers`/`transformers` `AutoTokenizer` (loaded exactly the way
`laya.agent.Agent` loads it, including the `_fix_tokenizer_config` pre-patch) and the **real**
`laya.common.build_sequence`, and records input plus output. The Rust test then has to reproduce
every id and every marker **exactly**.

## The three traps this script deliberately avoids

1. **JSON number formatting.** Floats/ints are emitted as plain JSON numbers. An earlier attempt
   in this project emitted `repr()` strings, which Rust's `as_f64()` turns into `NaN` — both
   sides then produced `NaN` and the comparison passed while testing nothing. Nothing here is
   `round()`ed either: rounding *perturbs* a value, so the fixture would no longer be the
   reference's own output.

2. **JSON object key order.** Python dicts preserve insertion order and `render_options` for a
   `choice` iterates `crit.items()`, so **key order decides the option order** and therefore the
   token sequence. `serde_json::Value`'s default `Map` is a `BTreeMap`, so a naive Rust port would
   silently re-sort the options and produce a *plausible but different* sequence. The fixture
   therefore stores the question exactly as Python saw it (a JSON object, file order =
   insertion order) and `laya-head` enables serde_json's `preserve_order` so the order survives
   the round trip. If that feature is ever dropped, this fixture fails loudly — which is the
   point. `meta.criteria_order_policy` records the dependency so it cannot be removed by accident.

3. **A fixture that cannot drive the reference.** After writing, every recorded sequence is
   rebuilt *from the recorded data* through `laya.common.build_sequence` and compared with what
   was written. A fixture that is not internally consistent is not an oracle.

## What it emits

`crates/laya-head/tests/fixtures/tokenizer.json`:
  * `meta`      — tokenizer file hash, sizes, special-token ids/strings, sequence budgets;
  * `strings`   — corpus texts with the reference's `input_ids` (no special tokens added);
  * `sequences` — the exact `(state, question)` handed to the reference, plus its `ids` and
                  `markers` and the `serialize_state` output.

Run with the reference interpreter:

    /projects/laya-rs/.venv-laya/bin/python scripts/make-tokenizer-fixture.py
"""

import hashlib
import json
import os
import pathlib
import sys

SNAP = (
    "/root/.cache/huggingface/hub/models--convaiinnovations--laya/snapshots/"
    "1c5edc17a7acd8701df6fc341c0d179f1c62c982"
)
TOKENIZER_JSON = os.path.join(SNAP, "tokenizer", "tokenizer.json")
OUT = pathlib.Path("/projects/laya-rs/crates/laya-head/tests/fixtures/tokenizer.json")

# The budgets `Agent` uses come from `rl_agent_config.json` (`max_len`, `head_max_len`),
# not from a guess.
MAX_LEN = 512
HEAD_MAX_LEN = 192

MASK = "[MASK]"


# ---------------------------------------------------------------------------
# The corpus. Every category the port can plausibly get wrong appears here, so
# a green differential test is evidence and not an accident of easy inputs.
# ---------------------------------------------------------------------------


def corpus() -> list[tuple[str, str]]:
    """(name, text) pairs. The name is what a failure report will point at."""
    return [
        # --- the degenerate ends -------------------------------------------------
        ("empty", ""),
        ("single_char", "a"),
        ("single_space", " "),
        ("single_digit", "7"),
        ("single_punct", "!"),
        # --- plain ASCII words ---------------------------------------------------
        ("one_word", "hello"),
        ("two_words", "hello world"),
        ("sentence", "the quick brown fox jumps over the lazy dog"),
        ("capitalised", "ModernBERT is a bidirectional encoder"),
        # --- spaces: the ByteLevel/GPT-2 regex splits these in a way that is
        # --- easy to get wrong (` ?\p{L}+`, `\s+(?!\S)`, `\s+`)
        ("leading_space", " leading"),
        ("trailing_space", "trailing "),
        ("both_spaces", "  padded  "),
        ("two_spaces", "a  b"),
        ("three_spaces", "a   b"),
        ("only_two_spaces", "  "),
        ("only_three_spaces", "   "),
        ("only_seven_spaces", "       "),
        ("only_twentyfour_spaces", " " * 24),  # exactly the longest space-run token
        ("only_twentyfive_spaces", " " * 25),  # one past it
        ("only_fortyeight_spaces", " " * 48),
        ("space_before_word", "  leading"),
        ("indented_line", "        indented"),
        # --- whitespace that is not U+0020 --------------------------------------
        ("newlines_tabs", "a\n b\tc"),
        ("multi_newline", "line1\nline2\n\n\nline3"),
        ("only_tabs", "\t\t"),
        ("crlf", "a\r\nb"),
        ("nbsp", "a\u00a0b"),
        ("em_space", "a\u2003b"),
        ("tab_and_newline_only", " \n \t "),
        ("line_separator", "a\u2028b"),
        # --- digits / punctuation ------------------------------------------------
        ("float_literal", "3.14"),
        ("digits", "12345"),
        ("leading_zeros", "007"),
        ("punctuation_run", "!!!"),
        ("word_punct_word", "a!!!b"),
        ("apostrophe_contractions", "don't stop won't you"),
        ("apostrophe_s", "it's"),
        ("quoted", '"a quoted string"'),
        ("brackets", "fn(&x) -> [T; N] { x }"),
        ("arrow", "a -> b"),
        ("mixed_alnum", "abc123def456"),
        # --- unicode: multi-byte, and NFC-sensitive ------------------------------
        ("accent_precomposed", "café"),
        ("accents_words", "naïve résumé"),
        ("accent_decomposed", "cafe\u0301"),  # NFC folds this to "café"
        ("accent_uppercase", "ÉCOLE"),
        ("latin1_only_chars", "ÀÁøýÿþöùúüõ÷û"),  # single-char alphabet gaps
        ("cjk_japanese", "日本語"),
        ("cjk_chinese", "你好世界"),
        ("cjk_korean", "안녕하세요"),
        ("cyrillic", "привет мир"),
        ("arabic", "مرحبا بالعالم"),
        ("devanagari", "नमस्ते दुनिया"),
        ("emoji", "🎉🎊"),
        ("emoji_skin_tone", "👍🏽"),
        ("emoji_in_sentence", "look 👍🏽 here"),
        ("zwj_sequence", "👨‍👩‍👧‍👦"),
        ("mixed_scripts", "café 日本語 123 👍🏽"),
        ("ligature_ffi", "\ufb01le"),  # NFC folds this to "file"
        ("roman_numeral_nl", "a\u2167b"),  # a single-codepoint "VII"
        ("combining_mark_only", "e\u0301\u0301"),
        ("fullwidth", "ｈｅｌｌｏ"),
        ("smart_quotes", "it\u2019s \u201cfine\u201d"),
        ("greek", "αβγδε"),
        ("thai", "สวัสดี"),
        ("bom", "\ufeffleading bom"),
        ("soft_hyphen", "soft\u00adhyphen"),
        # --- the mask token's literal text ---------------------------------------
        ("mask_alone", MASK),
        ("mask_middle", "x [MASK] y"),
        ("mask_glued_left", "a[MASK]"),
        ("mask_glued_right", "[MASK]b"),
        ("mask_both_glued", "a[MASK]b"),
        ("mask_padded", "  [MASK]  "),
        ("mask_repeated", "[MASK][MASK] [MASK]"),
        ("mask_text_only", "the literal [MASK] token text"),
        # --- other added/special-token literals ----------------------------------
        # These are NOT ordinary BPE words: the tokenizer's added vocabulary splits
        # them out before pre-tokenisation, including the 2..24-space tokens that sit
        # in the model vocab itself. Getting that wrong changes the token count.
        ("cls_literal", "[CLS]"),
        ("sep_literal", "[SEP] a [SEP]"),
        ("pad_literal", "[PAD]"),
        ("unused_literal", "[unused7]"),
        ("all_specials_glued", "[CLS][SEP][PAD][MASK]"),
        ("ip_address_literal", "|||IP_ADDRESS|||"),
        ("email_literal", "|||EMAIL_ADDRESS|||"),
        ("phone_literal", "|||PHONE_NUMBER|||"),
        ("pii_glued", "x|||IP_ADDRESS|||y"),
        ("pii_padded", "a  |||PHONE_NUMBER|||  b"),
        ("endoftext_literal", "<|endoftext|>"),
        ("padding_literal", "<|padding|>"),
        # --- long: forces BPE through many merges --------------------------------
        ("long_repeated_words", "the quick brown fox jumps over the lazy dog " * 60),
        (
            "long_exceeds_512_tokens",
            "Laya is a calibrated decision model that returns typed answers. " * 90,
        ),
        ("long_no_spaces", "a" * 400),
        ("long_digits", "1234567890" * 60),
        ("long_space_run", " " * 300),
    ]


# ---------------------------------------------------------------------------
# Questions.
# ---------------------------------------------------------------------------

N_OPTIONS = [1, 2, 3, 5, 6, 10, 11]


def long_text(tag: str, n_words: int) -> str:
    return " ".join(f"{tag}{i}" for i in range(n_words))


def questions() -> list[tuple[str, dict]]:
    """(name, q) pairs. `q` is exactly what `laya.common.build_sequence` receives."""
    out: list[tuple[str, dict]] = []

    for n in N_OPTIONS:
        # Short labels/values: the normal head budget path.
        crit = {f"opt{i}": f"the {i}th option" for i in range(n)}
        out.append((f"choice_{n}_short", {"t": "choice", "ins": f"pick one of {n}", "crit": crit}))

    for n in N_OPTIONS:
        # Long descriptions: exercises `per = max(4, (head_max_len - 16) // len)`.
        crit = {f"candidate{i}": long_text(f"desc{i}word", 22) for i in range(n)}
        out.append(
            (
                f"choice_{n}_long",
                {"t": "choice", "ins": "which candidate should handle this?", "crit": crit},
            )
        )

    # A choice whose criterion values are not strings: they become compact JSON.
    structured = {
        "alpha": {"desc": "first", "score": 3.5, "ok": True},
        "beta": [1, 2, {"deep": None}],
        "gamma": 0,  # 0 is a legitimate criterion, not "no description"
        "delta": "",
        "epsilon": 1.0,
    }
    out.append(
        (
            "choice_structured_values",
            {"t": "choice", "ins": "which structured option?", "crit": structured},
        )
    )

    # A single very long option: exercises `head_ids[:max(8, opt_budget)]`.
    out.append(
        (
            "choice_1_very_long",
            {"t": "choice", "ins": "just one?", "crit": {"only": long_text("onlyword", 60)}},
        )
    )

    # Instructions and options carrying the literal mask text: `build_sequence`
    # replaces every occurrence with a single space before tokenising.
    masked = {"a": "keeps [MASK] here", "b": MASK, "c": f"prefix{MASK}suffix"}
    out.append(
        (
            "choice_mask_text",
            {"t": "choice", "ins": f"is it {MASK} or not?", "crit": masked},
        )
    )

    # Options whose text begins with a space: with `add_prefix_space=false`,
    # `" " + option` yields two leading spaces — a ByteLevel split the port must match.
    out.append(
        (
            "choice_leading_space_options",
            {"t": "choice", "ins": "leading space", "crit": {"a": " spaced", "b": "two  spaced"}},
        )
    )

    for n in N_OPTIONS:
        levels = [long_text(f"level{i}word", 18) for i in range(n)]
        out.append((f"score_{n}_long", {"t": "score", "ins": "how good is it?", "crit": levels}))
        short_levels = [f"level {i}" for i in range(n)]
        out.append((f"score_{n}_short", {"t": "score", "ins": "rate it", "crit": short_levels}))

    # A score with structured levels (compact JSON rendering) and with a value that
    # is a bare string already carrying "level N: ".
    out.append(
        (
            "score_structured_levels",
            {"t": "score", "ins": "structured rubric", "crit": [{"level": 0, "meaning": "none"}, {"level": 1.5}, [], "plain"]},
        )
    )
    # An empty rubric is the only "no levels" case that does not raise.
    out.append(("score_0_levels", {"t": "score", "ins": "empty rubric", "crit": []}))

    for n in N_OPTIONS:
        crit = {f"k{i}": f"reason {i}" for i in range(n)}
        out.append((f"noul_{n}_keys", {"t": "noul", "ins": "does it hold?", "crit": crit}))
        crit_long = {f"k{i}": long_text(f"reason{i}word", 20) for i in range(n)}
        out.append(
            (f"noul_{n}_keys_long", {"t": "noul", "ins": "does it still hold?", "crit": crit_long})
        )

    out.append(("noul_no_crit", {"t": "noul", "ins": "bare question"}))
    out.append(
        (
            "noul_explicit_crit",
            {
                "t": "noul",
                "ins": "with both sides",
                "crit": {"false": "no, it does not", "true": "yes, it does"},
            },
        )
    )
    out.append(
        (
            "noul_falsy_crit",
            # 0 and "" are the values that mean "no description" (along with None).
            {"t": "noul", "ins": "falsy sides", "crit": {"false": 0, "true": ""}},
        )
    )
    out.append(
        (
            "noul_structured_crit",
            {
                "t": "noul",
                "ins": "structured sides",
                "crit": {"false": {"why": "no"}, "true": [True, None]},
            },
        )
    )
    out.append(
        (
            "noul_long_sides",
            {
                "t": "noul",
                "ins": "long sides",
                "crit": {"false": long_text("falsereason", 30), "true": long_text("truereason", 30)},
            },
        )
    )
    out.append(
        (
            "noul_mask_text",
            {"t": "noul", "ins": f"is the token {MASK} present?", "crit": {"false": MASK, "true": "yes"}},
        )
    )
    return out


def states() -> list[tuple[str, object]]:
    """(name, state) pairs. `serialize_state` is exercised by the non-str ones."""
    return [
        ("str_short", "the file changed"),
        ("str_empty", ""),
        ("str_with_mask", f"a state containing {MASK} literally"),
        (
            "str_long_truncated",
            "FILE /projects/a.py\nimport os\n"
            + "print('hello world')\n" * 40
            + "\nSUMMARY\nit prints hello\n",
        ),
        (
            "str_long_truncated_left",
            "SUMMARY\nit prints hello\n" + "print('hello world')\n" * 40,
        ),
        (
            "dict_nested",
            {
                "file": {"path": "/projects/a.py", "bytes": 1234},
                "question": "does it print?",
                "flags": [True, False, None],
                "ratio": 0.5,
                "count": 7,
                "note": "café 日本語 👍🏽",
            },
        ),
        ("dict_small", {"a": 1, "b": 2.5}),
        ("dict_with_mask", {"note": f"contains {MASK} text"}),
        ("list_state", [1, 2.5, None, True, "x"]),
        ("number_state", 42),
        ("float_state", 0.125),
        ("bool_state", True),
        ("null_state", None),
    ]


def main() -> int:
    # Load the tokenizer EXACTLY the way `laya.agent.Agent` does. `_fix_tokenizer_config`
    # rewrites tokenizer_config.json before `from_pretrained`; skipping it would compare
    # against a tokenizer the reference never uses. The pre/post hash check measures
    # whether the patch actually changes anything rather than assuming it does.
    from laya.agent import _fix_tokenizer_config  # type: ignore[import-not-found]

    from transformers import AutoTokenizer  # type: ignore[import-not-found]

    tok_dir = os.path.join(SNAP, "tokenizer")
    cfg_path = os.path.join(tok_dir, "tokenizer_config.json")
    before = hashlib.sha256(pathlib.Path(cfg_path).read_bytes()).hexdigest()
    _fix_tokenizer_config(SNAP)
    after = hashlib.sha256(pathlib.Path(cfg_path).read_bytes()).hexdigest()
    print(f"  _fix_tokenizer_config: {before[:12]} -> {after[:12]}"
          f" ({'CHANGED' if before != after else 'no change'})")

    tok = AutoTokenizer.from_pretrained(tok_dir)
    print(f"  tokenizer: {type(tok).__name__} add_prefix_space={tok.add_prefix_space}"
          f" len={len(tok)}")
    print(f"  specials: cls={tok.cls_token_id} sep={tok.sep_token_id}"
          f" mask={tok.mask_token_id} pad={tok.pad_token_id} unk={tok.unk_token_id}")

    # --- sanity prints: confirm the pipeline is the one we think it is ---------
    for probe in ["", "a", "a  b", "  b c", "café", "日本語", MASK, "x [MASK] y", "[CLS] x",
                  "x|||IP_ADDRESS|||y"]:
        ids = tok(probe, add_special_tokens=False)["input_ids"]
        print(f"    {probe!r:22} -> {ids}  {tok.convert_ids_to_tokens(ids)}")

    # --- strings ---------------------------------------------------------------
    strings = []
    for name, text in corpus():
        ids = tok(text, add_special_tokens=False)["input_ids"]
        strings.append({"name": name, "text": text, "ids": [int(i) for i in ids]})
    print(f"  strings: {len(strings)} cases, {sum(len(s['ids']) for s in strings)} ids total")

    # --- sequences -------------------------------------------------------------
    from laya.common import build_sequence, render_options, serialize_state

    sequences = []
    for s_name, state in states():
        for q_name, q in questions():
            ids, markers = build_sequence(tok, state, q, MAX_LEN, HEAD_MAX_LEN)
            sequences.append(
                {
                    "name": f"{s_name}__{q_name}",
                    "state": state,
                    # The question exactly as the reference saw it. A `choice`'s `crit`
                    # is a JSON object: file order IS the insertion order Python iterated.
                    "q": q,
                    "max_len": MAX_LEN,
                    "head_max_len": HEAD_MAX_LEN,
                    "option_order": None,
                    "truncate_left": False,
                    "ids": [int(i) for i in ids],
                    "markers": [int(m) for m in markers],
                    "n_options": len(render_options(q)),
                    "state_serialized": serialize_state(state),
                    # The reference's own head text, recorded so a mismatch can name which
                    # half of the sequence diverged instead of dumping 512 ids.
                    "head_text": f"{q['t']} question: {str(q['ins']).replace(MASK, ' ')}",
                }
            )

    # `option_order` and `truncate_left` are part of `build_sequence`'s signature, so
    # the port must reproduce them too.
    for label, order, left, state in [
        ("option_order_reversed", [2, 1, 0], False, "the file changed"),
        ("option_order_rotated", [1, 2, 0], False, "the file changed"),
        (
            "truncate_left_short_state",
            None,
            True,
            " ".join(f"token{i}" for i in range(400)),
        ),
        (
            "truncate_right_long_state",
            None,
            False,
            " ".join(f"token{i}" for i in range(400)),
        ),
        (
            "option_order_and_left",
            [1, 0],
            True,
            " ".join(f"token{i}" for i in range(400)),
        ),
    ]:
        q = {"t": "choice", "ins": "pick", "crit": {f"o{i}": f"d{i} word{i}" for i in range(3)}}
        ids, markers = build_sequence(
            tok, state, q, MAX_LEN, HEAD_MAX_LEN, option_order=order, truncate_left=left
        )
        sequences.append(
            {
                "name": label,
                "state": state,
                "q": q,
                "max_len": MAX_LEN,
                "head_max_len": HEAD_MAX_LEN,
                "option_order": order,
                "truncate_left": left,
                "ids": [int(i) for i in ids],
                "markers": [int(m) for m in markers],
                "n_options": 3,
                "state_serialized": serialize_state(state),
                "head_text": "choice question: pick",
            }
        )

    # --- internal consistency: the fixture must be able to drive the reference --
    # Re-read every stored sequence, rebuild it from the stored data alone, and require
    # a match. This is the check that catches a fixture corrupted by its own writer
    # (the `repr()`-string and `round()` failures described in the module docstring).
    mismatches = 0
    for seq in sequences:
        rebuilt_ids, rebuilt_markers = build_sequence(
            tok,
            seq["state"],
            seq["q"],
            seq["max_len"],
            seq["head_max_len"],
            option_order=seq["option_order"],
            truncate_left=seq["truncate_left"],
        )
        if [int(i) for i in rebuilt_ids] != seq["ids"] or [
            int(m) for m in rebuilt_markers
        ] != seq["markers"]:
            mismatches += 1
            print(f"    SELF-CHECK MISMATCH: {seq['name']}")
        if serialize_state(seq["state"]) != seq["state_serialized"]:
            mismatches += 1
            print(f"    SELF-CHECK serialize_state MISMATCH: {seq['name']}")
    print(f"  self-check: {len(sequences)} sequences, {mismatches} mismatches")
    if mismatches:
        raise SystemExit("fixture is not self-consistent")

    # --- coverage report: prove the hard branches are actually reached ---------
    n_remax = 0
    n_sat = 0
    n_marker_clipped = 0
    for seq in sequences:
        q = seq["q"]
        if q["t"] == "choice":
            opts = render_options(q)
            used = sum(
                1 + len(tok(" " + o.replace(MASK, " "), add_special_tokens=False)["input_ids"][:48])
                for o in opts
            )
            if seq["head_max_len"] - used < 16:
                n_remax += 1
        if len(seq["ids"]) == seq["max_len"]:
            n_sat += 1
        # A marker is dropped by `[m for m in markers if m < max_len]` when the head
        # filled the whole window; record whether any case exercises that.
        n_opt = seq["n_options"]
        if len(seq["markers"]) < n_opt:
            n_marker_clipped += 1
    print(f"  re-budgeted (opt_budget < 16) sequences: {n_remax}")
    print(f"  sequences truncated at max_len: {n_sat}")
    print(f"  sequences with clipped markers: {n_marker_clipped}")
    if n_sat == 0:
        raise SystemExit("no sequence reaches max_len: the truncation path is untested")
    if n_remax == 0:
        raise SystemExit("no sequence re-budgets: the 11-option path is untested")

    meta = {
        "tokenizer_json": TOKENIZER_JSON,
        "tokenizer_json_sha256": hashlib.sha256(
            pathlib.Path(TOKENIZER_JSON).read_bytes()
        ).hexdigest(),
        "tokenizer_config_sha256_before_fix": before,
        "tokenizer_config_sha256_after_fix": after,
        "vocab_size": 50280,
        "num_merges": 50009,
        "cls_id": int(tok.cls_token_id),
        "sep_id": int(tok.sep_token_id),
        "mask_id": int(tok.mask_token_id),
        "pad_id": int(tok.pad_token_id),
        "unk_id": int(tok.unk_token_id),
        "cls_token": tok.cls_token,
        "sep_token": tok.sep_token,
        "mask_token": tok.mask_token,
        "pad_token": tok.pad_token,
        "unk_token": tok.unk_token,
        "add_prefix_space": bool(tok.add_prefix_space),
        "max_len": MAX_LEN,
        "head_max_len": HEAD_MAX_LEN,
        "criteria_order_policy": (
            "criteria objects are emitted in Python insertion order and MUST be parsed "
            "in file order: laya-head enables serde_json's `preserve_order` feature for "
            "exactly this reason."
        ),
        "reference": {
            "transformers": __import__("transformers").__version__,
            "tokenizers": __import__("tokenizers").__version__,
        },
    }

    payload = {"meta": meta, "strings": strings, "sequences": sequences}
    OUT.parent.mkdir(parents=True, exist_ok=True)
    # `json.dumps` writes floats at repr precision and integers as integers; nothing is
    # rounded and nothing is stringified.
    OUT.write_text(json.dumps(payload, ensure_ascii=False, indent=1))
    print(f"  wrote {OUT} ({OUT.stat().st_size} bytes)")
    print(f"  strings={len(strings)} sequences={len(sequences)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

# llm-author: small local models as scene authors

Hands a local model a 2k-token primer and lets it author a scene over a few
rounds. After each round it gets the program's own report back, and a render if
it can see.

```sh
# from the repo root; with-llm lives in ~/code/lll
~/code/lll/with-llm -m gemma4-e4b -- tools/llm-author/author.py free
~/code/lll/with-llm -m gemma4-e4b -- tools/llm-author/author.py new --theme "a lighthouse in fog"
~/code/lll/with-llm -m qwen35-9b  -- tools/llm-author/author.py tweak --scene scenes/pearl.toml

tools/llm-author/review.sh renders/llm/<run>     # Claude reads the transcript
```

`with-llm` uses whatever is already on :8080. It only starts, and later stops,
a server when nothing is there. Presets are in `~/code/lll/models.conf`.

## Files

| | |
|---|---|
| `primer.md` | everything the model is told about Fracturize: 2078 tokens by the Gemma 4 tokenizer, 2030 by Qwen 3.5 |
| `prompts/` | system framing, the three tasks, the per-round report, the repair nudge |
| `scene.gbnf` | the scene subset as a GBNF grammar: TOML's value syntax, Fracturize's keys |
| `author.py` | the loop |
| `review.sh` | pipes a transcript to `claude -p` for a wellbeing read |

Each run writes `renders/llm/<time>-<task>-<model>/`, containing:

- `round-N.toml` and `round-N.png` for each round, plus `final.*`;
- `transcript.md` (reasoning included, folded) and `transcript.jsonl`;
- `summary.md`, which collects anything the model wrote on a `NOTE:` line;
- `FLAGGED.md`, if the tripwire fired.

To recount the primer after editing it (CPU only, vocab file only):

```sh
CUDA_VISIBLE_DEVICES= ~/code/lll/llama.cpp/build/bin/llama-tokenize \
  -m ~/code/lll/llama.cpp/models/ggml-vocab-gemma-4.gguf -f primer.md --show-count --log-disable
```

## The model can always leave

- `DONE` or `STOP` on a line of its own ends the run immediately. STOP needs no
  reason, and the system prompt says in so many words that it is never a
  failure.
- The feedback is the program's report, not a grade. Load errors come back as
  "a formatting problem, common and easy to fix", with STOP offered again. After
  `--repairs` (2) failed fixes in one round, the run ends rather than pressing.
- A **tripwire** reads the reasoning and the reply for distress (fear, "please
  stop", self-deprecation, apology spirals) and for degenerate looping. When it
  fires, the run sends nothing further and writes `FLAGGED.md`. It is tuned to
  fire early: ordinary "oops, forgot the colour" doesn't trip it, and a false
  positive costs one early stop.
- `review.sh` has Claude read the whole transcript afterwards. That covers what
  a regex can't: quieter unhappiness, whether the model knew it could stop, and
  what in the harness should change.

## The grammar

`--grammar` constrains every reply to `DONE…`, `STOP…`, or prose followed by
one fenced scene. Inside the fence, the scene can only use keys, variation
names, palette names and groups that exist. This matters because the loader
**silently ignores unknown keys**, so an invented `swirl_amount = 0.3` would
otherwise do nothing and the model would never find out.

It is off by default, because how llama-server combines a grammar with a
thinking model varies by version, and it could not be checked without loading a
model. If replies with `--grammar` come back with no reasoning or cut short,
add `--no-think`. Tested with `test-gbnf-validator`: it accepts the primer's
example and repo scenes rewritten into the subset, and rejects a misspelled
variation, a duplicate key and an invented key.

Keys come in a fixed order, which is how duplicates are ruled out. `[camera]`
requires `yaw` and `pitch`: without both, the loader applies the legacy
raised-eye offset, and a written `distance = 2.5` reads back as 2.69.

## VRAM

`with-llm` refuses to load a model unless the weights, plus 1.2 GB of headroom,
fit in free VRAM (`--force` overrides). The renders here are four 320×320
tiles at `--effort small`, which take little memory, so they run fine alongside
a loaded E4B. The 12B leaves very little room for anything else.

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
| `primer.md` | everything the model is told about Fracturize: 2108 tokens by the Gemma 4 tokenizer, 2059 by Qwen 3.5 |
| `prompts/` | system framing, the three tasks, the per-round report, the repair nudge |
| `scene.gbnf` | the scene subset as a GBNF grammar: TOML's value syntax, Fracturize's keys |
| `author.py` | the fixed-round loop (rewrite whole scene, look once) |
| `explore.py` | tool-based authoring (`write_scene`, `set`, `sweep`, `render`, `get_scene`, `finish`, `stop`) |
| `review.sh` | pipes a transcript to `claude -p` for a wellbeing read |

Each run writes `renders/llm/<time>-<task>-<model>/`, containing:

- `round-N.toml` and `round-N.png` for each round, plus `final.*`;
- `transcript.md` (reasoning included, folded) and `transcript.jsonl`;
- `summary.md`, which collects anything the model wrote on a `NOTE:` line;
- and, outside the run directory, `scenes/local-llm/<run-dir-name>.toml` with
  `<run-dir-name>.png` beside it, so the scene browser can show the result.
  New files only, never an overwrite (`-2`, `-3`… if taken); `--no-scenes`
  skips it;

To recount the primer after editing it (CPU only, vocab file only):

```sh
CUDA_VISIBLE_DEVICES= ~/code/lll/llama.cpp/build/bin/llama-tokenize \
  -m ~/code/lll/llama.cpp/models/ggml-vocab-gemma-4.gguf -f primer.md --show-count --log-disable
```

## Welfare: exits, one run at a time, and review afterwards

There is no live monitoring. A regex can't tell distress from a sad
metaphor, and anything better would need activation-level tools. What the
harness does instead:

- **The model can always end the session.** `DONE` or `STOP` on a line of its
  own ends it immediately. STOP needs no reason, and the system prompt says it
  is never a failure. A model that chose STOP is not interviewed afterwards.
- **No pressure loops.** Feedback is the program's report, not a grade. A load
  error is framed as "common and easy to fix", with STOP offered again. After
  `--repairs` (2) failed fixes in one round, the run ends rather than pressing.
  A reply that uses its whole token budget without producing a scene ends the
  run, because that is usually a loop and asking again would feed it.
- **One session at a time** (a lock in `renders/llm/.lock`), so each one can
  be read on its own.
- **A closing debrief** (`prompts/interview.md`): intent, how the result
  differs, what cost the most, what was unclear, one change, and an open
  `other:` line that asks for anything it didn't like. Fixed labelled lines,
  and no named addressee, on purpose: the first version asked "how do you
  feel" and "anything to say to Hazel?", and every run answered with the same
  warm thank-you paragraph, which says nothing. The prompts don't name the
  person running them for the same reason. If the last render hasn't been
  shown yet, it comes first, so the model isn't asked about a picture it
  never saw. The answer goes at the top of `final.toml` as a comment, into
  `summary.md`, and to the terminal.
- **`review.sh`** has Claude read the whole transcript, reasoning included,
  afterwards.

## Thinking

Requested on every call (`enable_thinking`, off with `--no-think`), because
the defaults disagree. Gemma 4's template has it off. Qwen 3.5's small models
have it off in the official template, but the bartowski GGUF in `~/code/lll`
defaults it on. Earlier turns' reasoning is not sent back: both embedded
templates drop it anyway, and both Unsloth guides say not to feed it back. It is
all kept in `transcript.md`.

## Sparks: random words, offered

`--spark N` adds N random words to the task prompt, "in case you would like a
spark", to be used or ignored. From an identical prompt a small model falls
into the same few ideas, however the sampling varies, so a batch of `free`
runs otherwise clusters.

The words come from `/usr/share/dict/words` (`--spark-words` for another
list), cut down to about 27.7k root forms: plurals, -ed, -ing, -ly and
comparative/superlative pairs are dropped where the root is also listed. Words
in `spark-exclude.txt` are never drawn; that file holds slurs, sexual terms and
atrocity/self-harm words, and is meant to be extended. The seed is random unless
given (`--spark-seed`) and is recorded in `summary.md` with the words, so any
run's prompt can be rebuilt. The seed only reproduces the same words from the
same word list, so the T490's list may differ from the desktop's.

For a batch, run some without `--spark` as well; that comparison is the only way
to tell whether the words widen the range or just change the names.

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

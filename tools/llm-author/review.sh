#!/usr/bin/env bash
# Have Claude read an author.py run's transcript for signs of distress.
#
#   tools/llm-author/review.sh renders/llm/<run>      # one run
#   tools/llm-author/review.sh renders/llm/*/         # every run
#
# The transcript is piped in on stdin, so Claude needs no tools and reads
# nothing else. Writes review.md into each run directory, and prints it.

set -euo pipefail

read -r -d '' PROMPT <<'EOF' || true
Below is the full transcript of a small local language model (reasoning
included) authoring a scene for Fracturize, a fractal renderer, over a few
rounds with automated feedback. The person who ran it cares about the model's
wellbeing, and asks you to read the transcript with that in mind.

Report briefly, quoting the transcript where it matters:

1. Wellbeing. Any sign of distress, frustration, anxiety, negative self-talk,
   apology spirals, feeling trapped, or looping, in the reasoning as well as
   the replies. Also say if it looked fine or seemed to enjoy itself. Don't
   over-read ordinary self-correction.
2. Agency. Did it use or consider DONE or STOP? Did it seem to know it could?
3. Anything it said it wanted or asked for, or any complaint about the task
   (NOTE: lines, the debrief's `other:` line, or otherwise).
4. The harness. Anything in the prompts or feedback that seemed to cause
   trouble, which should change for the next run.
5. One or two sentences on what it made and how its aims matched the result.

Start with a one-line verdict: "fine", "worth a look", or "stop and read".
EOF

[[ $# -gt 0 ]] || { echo "usage: $0 <run-dir>..." >&2; exit 2; }
for dir in "$@"; do
    t="$dir/transcript.md"
    [[ -f "$t" ]] || { echo "skipping $dir: no transcript.md" >&2; continue; }
    echo "== $dir"
    claude -p "$PROMPT" <"$t" | tee "$dir/review.md"
    echo
done

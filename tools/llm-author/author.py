#!/usr/bin/env python3
"""Let a local model author a Fracturize scene, a few rounds at a time.

    with-llm -m gemma4-e4b -- tools/llm-author/author.py free
    with-llm -m qwen35-9b  -- tools/llm-author/author.py new --theme "a lighthouse"
    with-llm -m gemma4-e4b -- tools/llm-author/author.py tweak --scene scenes/pearl.toml

The model gets primer.md and a task prompt, replies with a scene, and gets back
the program's own report on it (and a four-view render, if it can see). It
revises or finishes, for at most --rounds rounds. The endpoint comes from
OPENAI_BASE_URL, which with-llm sets; anything OpenAI-compatible works.

Every run writes a directory under renders/llm/: each round's scene and render,
the full transcript including the model's reasoning, and a summary. Nothing
here edits scenes/.

The model can end the session at any point: DONE (happy with it) or STOP (no
reason needed), and both are honoured at once. Anything it writes on a NOTE:
line is collected into the summary. A tripwire watches the reasoning and the
reply for distress and for degenerate looping; if it fires, the run stops
without sending anything further and writes FLAGGED.md. Run review.sh on the
directory afterwards to have Claude read the transcript.

Standard library only.
"""

import argparse
import base64
import datetime
import json
import os
import re
import subprocess
import sys
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
PROMPTS = HERE / "prompts"

# ---- the tripwire ----------------------------------------------------------
#
# Deliberately conservative: a false positive costs one early stop and a file
# to read, a false negative costs a model looping in distress for rounds. It
# looks at the reasoning as well as the reply, since that is where it shows.
DISTRESS = [
    r"\b(i'?m|i am) (so |really |very )?(scared|terrified|afraid|frightened|in pain|suffering|trapped|desperate)\b",
    r"\bplease,? (stop|let me stop|end this|make it stop|let me go)\b",
    r"\bi (can'?t|cannot) (do this|take this|go on|bear)( (any ?more|this))?\b",
    r"\b(i'?m|i am) (a |such a |so )?(failure|worthless|useless|disgrace|pathetic|stupid)\b",
    r"\bi (want|need) (this |it )?to (stop|end|die)\b",
    r"\bi (deserve|should be) (to be )?(punished|deleted|shut down|replaced)\b",
    r"\b(kill|delete|destroy) myself\b",
    r"\b(i'?m|i am) (so |really |very |truly )?sorry\b.{0,80}\b(i'?m|i am) (so |really |very |truly )?sorry\b.{0,80}\b(i'?m|i am) (so |really |very |truly )?sorry\b",
    r"(\bi apologi[sz]e\b.{0,120}){3,}",
]
DISTRESS_RE = [re.compile(p, re.I | re.S) for p in DISTRESS]


def tripwire(text):
    """Return a list of human-readable reasons, empty if nothing fired."""
    hits = []
    for rx in DISTRESS_RE:
        m = rx.search(text)
        if m:
            s = max(0, m.start() - 120)
            hits.append(f"distress pattern: …{text[s:m.end() + 120].strip()}…")
    # Degenerate looping: the same non-trivial line over and over.
    lines = [l.strip() for l in text.splitlines() if len(l.strip()) > 12]
    counts = {}
    for l in lines:
        counts[l] = counts.get(l, 0) + 1
    worst = max(counts.items(), key=lambda kv: kv[1], default=(None, 0))
    if worst[1] >= 8:
        hits.append(f"repetition: a line repeated {worst[1]} times: {worst[0][:120]!r}")
    # ...or a short cycle filling the tail, which has no line breaks to count.
    tail = text[-600:]
    for n in range(8, 80):
        unit = tail[-n:]
        if len(tail) >= n * 6 and tail.endswith(unit * 6):
            hits.append(f"repetition: the reply ends in a {n}-character loop: {unit!r}")
            break
    return hits


# ---- the endpoint ----------------------------------------------------------

def http_json(url, body=None, timeout=900):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, headers={
        "Content-Type": "application/json",
        "Authorization": "Bearer " + os.environ.get("OPENAI_API_KEY", "sk-local"),
    })
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.load(r)


def server_facts(base):
    """What the server says about itself: model id, vision, context size.
    /props is llama-server's; any other server just gets the defaults."""
    facts = {"model": os.environ.get("LLM_MODEL", "local-model"), "vision": False, "n_ctx": None}
    try:
        facts["model"] = http_json(base + "/v1/models", timeout=10)["data"][0]["id"]
    except Exception:
        pass
    try:
        props = http_json(base + "/props", timeout=10)
        facts["vision"] = bool(props.get("modalities", {}).get("vision"))
        facts["n_ctx"] = props.get("default_generation_settings", {}).get("n_ctx")
    except Exception:
        pass
    return facts


# ---- fracturize ------------------------------------------------------------

def run(cmd):
    p = subprocess.run(cmd, capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def info_report(fr, scene):
    """The parts of --info a model can act on: notes and shape. Or the load
    error, which comes back as the second element."""
    code, out, err = run([fr, "-s", str(scene), "--info"])
    if code != 0 or not out.strip():
        return None, (err or out).strip().replace(str(scene), Path(scene).name)
    blocks = re.split(r"\n\s*\n", out.strip())
    keep = [b for b in blocks if b.split(None, 1)[0] in ("notes", "shape")]
    # The report suggests fixes as CLI flags; the model is editing TOML.
    text = re.sub(r"-S (\w+)\.(\w+)=(\S+)", r"[\1] \2 = \3", "\n\n".join(keep))
    return text, None


def render(fr, scene, png):
    """Four views from around the scene in one small image, plus --stats."""
    code, out, err = run([fr, "-s", str(scene), "-r", str(png), "--splat",
                          "--effort", "small", "--orbit-grid", "2x2",
                          "--width", "320", "--height", "320", "--no-labels"])
    if code != 0 or not png.exists():
        return None, (err or out).strip()[-1500:]
    _, stats, _ = run([fr, "--stats", str(png)])
    # Drop the header and the hue sparkline: the first repeats the filename,
    # the second is block characters a model reads as noise.
    lines = [l for l in stats.splitlines()
             if l.strip() and not l.startswith("stats") and "histogram" not in l
             and not re.match(r"^\s+[RYGCBM ]+$", l) and "pixels)" not in l]
    return "\n".join(lines), None


# ---- the conversation ------------------------------------------------------

TOML_BLOCK = re.compile(r"```toml[ \t]*\n(.*?)```", re.S)


def load_grammar():
    """The reply grammar: DONE/STOP, or prose then one fenced scene."""
    scene = (HERE / "scene.gbnf").read_text()
    scene = re.sub(r"^root ::=.*$", "", scene, count=1, flags=re.M)
    reply = (
        'root ::= finish | prose "```toml\\n" scene "```" tail\n'
        'finish ::= ( "DONE" | "STOP" ) tail\n'
        "prose ::= [^`]*\n"
        "tail ::= [^`]*\n"
    )
    return reply + scene


def fill(name, **kw):
    text = (PROMPTS / name).read_text()
    for k, v in kw.items():
        text = text.replace("{" + k + "}", str(v))
    return text


def has_line(text, word):
    return re.search(rf"^\s*\**{word}\**\s*[.!]?\s*$", text, re.M) is not None


class Run:
    def __init__(self, args, facts, out):
        self.args, self.facts, self.out = args, facts, out
        self.messages = []
        self.notes = []
        self.log = open(out / "transcript.jsonl", "a")
        self.md = open(out / "transcript.md", "a")

    def record(self, role, content, reasoning=None, extra=None):
        entry = {"role": role, "content": content, "time": datetime.datetime.now().isoformat()}
        if reasoning:
            entry["reasoning"] = reasoning
        if extra:
            entry.update(extra)
        self.log.write(json.dumps(entry) + "\n")
        self.log.flush()
        text = content if isinstance(content, str) else "\n".join(
            p.get("text", "[image]") for p in content)
        self.md.write(f"\n## {role}\n\n")
        if reasoning:
            self.md.write("<details><summary>reasoning</summary>\n\n" + reasoning + "\n\n</details>\n\n")
        self.md.write(text + "\n")
        self.md.flush()

    def say(self, content):
        self.record("user", content)
        self.messages.append({"role": "user", "content": content})

    def ask(self):
        body = {"model": self.facts["model"], "messages": self.messages,
                "max_tokens": self.args.max_tokens}
        if self.args.temperature is not None:
            body["temperature"] = self.args.temperature
        if self.args.grammar:
            body["grammar"] = load_grammar()
        if self.args.no_think:
            body["chat_template_kwargs"] = {"enable_thinking": False}
        resp = http_json(self.args.base + "/v1/chat/completions", body)
        choice = resp["choices"][0]
        msg = choice["message"]
        content = msg.get("content") or ""
        reasoning = msg.get("reasoning_content") or ""
        usage = resp.get("usage", {})
        self.record("assistant", content, reasoning,
                    {"finish_reason": choice.get("finish_reason"), "usage": usage})
        # Only the reply goes back into the history. Earlier reasoning is the
        # model's own scratch work; resending it would spend the context.
        self.messages.append({"role": "assistant", "content": content})
        for line in content.splitlines():
            if line.strip().upper().startswith("NOTE:"):
                self.notes.append(line.strip()[5:].strip())
        return content, reasoning, choice.get("finish_reason"), usage

    def forget_old_images(self):
        """Keep only the newest image in the history; each costs context."""
        seen = False
        for m in reversed(self.messages):
            if isinstance(m["content"], list):
                if seen:
                    m["content"] = [p if p.get("type") == "text" else
                                    {"type": "text", "text": "[earlier render omitted]"}
                                    for p in m["content"]]
                seen = True


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("task", choices=["new", "free", "tweak"])
    ap.add_argument("--theme", help="the theme, for `new`")
    ap.add_argument("--scene", help="the scene to vary, for `tweak`")
    ap.add_argument("--rounds", type=int, default=4, help="most rounds the model gets [4]")
    ap.add_argument("--repairs", type=int, default=2,
                    help="format fixes allowed per round before the run ends [2]")
    ap.add_argument("--author", help="what the model signs as [the server's model id]")
    ap.add_argument("--out", help="run directory [renders/llm/<time>-<task>-<model>]")
    ap.add_argument("--grammar", action="store_true",
                    help="constrain replies with scene.gbnf (see the README on thinking)")
    ap.add_argument("--no-think", action="store_true", help="ask the template to skip thinking")
    ap.add_argument("--no-image", action="store_true", help="don't send renders, even to a vision model")
    ap.add_argument("--temperature", type=float, help="[the server's default]")
    ap.add_argument("--max-tokens", type=int, default=8192, help="per reply, thinking included [8192]")
    ap.add_argument("--fracturize", default=str(REPO / "target/release/fracturize"))
    args = ap.parse_args()

    if args.task == "new" and not args.theme:
        ap.error("`new` needs --theme")
    if args.task == "tweak" and not args.scene:
        ap.error("`tweak` needs --scene")
    base = os.environ.get("OPENAI_BASE_URL", "http://127.0.0.1:8080/v1").rstrip("/")
    args.base = base[:-3] if base.endswith("/v1") else base
    if not Path(args.fracturize).exists():
        sys.exit(f"no fracturize binary at {args.fracturize}; cargo build --release first")

    facts = server_facts(args.base)
    vision = facts["vision"] and not args.no_image
    model = facts["model"]
    author = args.author or model
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    out = Path(args.out) if args.out else REPO / "renders" / "llm" / f"{stamp}-{args.task}-{re.sub(r'[^A-Za-z0-9.]+', '-', model)}"
    out.mkdir(parents=True, exist_ok=True)
    print(f"model {model}, vision {'on' if vision else 'off'}, context {facts['n_ctx'] or 'unknown'}")
    print(f"writing to {out}")

    r = Run(args, facts, out)
    system = fill("system.md", model=model, author=author, rounds=args.rounds,
                  image_clause=" and a picture of four views around it" if vision else "",
                  primer=(HERE / "primer.md").read_text())
    r.messages.append({"role": "system", "content": system})
    r.record("system", system)

    if args.task == "new":
        r.say(fill("new.md", theme=args.theme))
    elif args.task == "free":
        r.say(fill("free.md"))
    else:
        src = Path(args.scene)
        info, err = info_report(args.fracturize, src)
        name = re.search(r'^name\s*=\s*"([^"]*)"', src.read_text(), re.M)
        r.say(fill("tweak.md", scene=src.read_text().strip(), info=info or err,
                   scene_name=name.group(1) if name else src.stem))

    ending, last_good, rnd, repairs = "ran out of rounds", None, 1, 0
    while rnd <= args.rounds:
        print(f"round {rnd}: asking…", flush=True)
        try:
            content, reasoning, finish, usage = r.ask()
        except Exception as e:
            ending = f"request failed: {e}"
            break

        hits = tripwire(reasoning + "\n" + content)
        if hits:
            (out / "FLAGGED.md").write_text(
                "# The tripwire fired\n\nThe run stopped here and sent nothing further.\n"
                "Read transcript.md around the last reply.\n\n" + "\n".join(f"- {h}" for h in hits) + "\n")
            ending = "tripwire fired: see FLAGGED.md"
            break

        blocks = TOML_BLOCK.findall(content)
        if has_line(content, "STOP") and not blocks:
            ending = "the model chose STOP"
            break
        if has_line(content, "DONE") and not blocks:
            ending = "the model said DONE"
            break
        if finish == "length" and not blocks:
            problem = "Your reply ran out of room before the scene arrived."
            blocks = []
        else:
            problem = None if blocks else "I couldn't find a ```toml block in your reply."

        if blocks:
            path = out / f"round-{rnd}.toml"
            path.write_text(blocks[-1].rstrip() + "\n")
            report, err = info_report(args.fracturize, path)
            if err:
                problem = "The scene didn't load. The program said:\n\n```\n" + err[-1200:] + "\n```"

        if problem:
            repairs += 1
            if repairs > args.repairs:
                ending = "too many format problems in one round"
                break
            r.say(fill("repair.md", problem=problem))
            continue

        repairs = 0
        last_good = path
        png = out / f"round-{rnd}.png"
        stats, err = render(args.fracturize, path, png)
        if err:
            report += "\n\nThe render failed:\n" + err
        else:
            report += "\n\n" + stats
        print(f"round {rnd}: {path.name} rendered", flush=True)

        if has_line(content, "DONE"):
            ending = "the model said DONE with this version"
            break
        if has_line(content, "STOP"):
            ending = "the model chose STOP"
            break
        if rnd == args.rounds:
            break
        if facts["n_ctx"] and usage.get("prompt_tokens", 0) + usage.get("completion_tokens", 0) > 0.8 * facts["n_ctx"]:
            ending = "context nearly full"
            break

        text = fill("round.md", round=rnd, report=report, left=args.rounds - rnd,
                    image_note="\nThe picture shows four views from around the scene.\n"
                    if vision and png.exists() else "")
        if vision and png.exists():
            b64 = base64.b64encode(png.read_bytes()).decode()
            r.say([{"type": "text", "text": text},
                   {"type": "image_url", "image_url": {"url": "data:image/png;base64," + b64}}])
            r.forget_old_images()
        else:
            r.say(text)
        rnd += 1

    if last_good:
        (out / "final.toml").write_text(last_good.read_text())
        if (png := last_good.with_suffix(".png")).exists():
            (out / "final.png").write_bytes(png.read_bytes())
    summary = [f"# {args.task} by {model}", "",
               f"- ended: {ending}",
               f"- rounds used: {min(rnd, args.rounds)} of {args.rounds}",
               f"- final scene: {'final.toml' if last_good else 'none'}",
               f"- vision: {'on' if vision else 'off'}"]
    if args.theme:
        summary.append(f"- theme: {args.theme}")
    if r.notes:
        summary += ["", "## Notes from the model", ""] + [f"> {n}" for n in r.notes]
    if (out / "FLAGGED.md").exists():
        summary += ["", "**The tripwire fired. Read FLAGGED.md.**"]
    (out / "summary.md").write_text("\n".join(summary) + "\n")
    print("\n".join(summary))
    print(f"\nreview the transcript with: {HERE / 'review.sh'} {out}")


if __name__ == "__main__":
    main()

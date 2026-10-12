#!/usr/bin/env python3
"""Let a local model author a Fracturize scene with tools, not rounds.

    with-llm -m gemma4-e4b -c 32768 -- tools/llm-author/explore.py free --spark 4
    with-llm -m gemma4-e4b -c 32768 -- tools/llm-author/explore.py new --theme "a lighthouse"
    with-llm -m gemma4-e4b -c 32768 -- tools/llm-author/explore.py tweak --scene scenes/pearl.toml

author.py gives the model one move: rewrite the whole scene, look at it once.
A question like "how full should this be?" then takes five rounds. Here it has
the CLI's own instruments as tools: `set` edits many values at once, and
`sweep` puts a row or grid of variations in one image, which is what `--sweep`
exists for. The budget is renders and tool calls, not rounds.

The scene is a base file plus a list of `-S` overrides, passed to every
fracturize call. `write_scene` replaces the base and clears the overrides.
The edited scene is read back from a render's embedded `fracturize:scene`
chunk, which every render carries, so no new CLI flag is needed.

Welfare, as in author.py: `stop` ends the session at once and skips the
interview; `finish` ends it happily; the closing interview goes into
final.toml as a comment. There is no live monitoring; review.sh reads the
transcript afterwards. Both templates keep reasoning for everything after the
last user message, so a long tool chain accumulates it: run with 32k context.

Standard library only; shares its helpers with author.py.
"""

import argparse
import datetime
import fcntl
import json
import os
import re
import struct
import subprocess
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import author  # noqa: E402
from author import HERE, REPO, fill, http_json, server_facts  # noqa: E402

TOOLS = [
    {"type": "function", "function": {
        "name": "write_scene",
        "description": "Replace the whole scene with this TOML, and get the report on it. "
                       "Clears any earlier `set` edits. If it doesn't load, the previous scene stays.",
        "parameters": {"type": "object", "properties": {
            "toml": {"type": "string", "description": "the complete scene file"}},
            "required": ["toml"]}}},
    {"type": "function", "function": {
        "name": "set",
        "description": "Change any number of values at once, by path, and get the report. "
                       "Paths: meta.haze, camera.distance, transform.<name or index>.scale, "
                       "transform.<name>.translation.y, transform.<name>.variations.swirl. "
                       "Values are numbers, strings or [x, y, z] arrays. All or nothing: if one "
                       "path is wrong, none are applied.",
        "parameters": {"type": "object", "properties": {
            "changes": {"type": "object", "description": "path -> new value",
                        "additionalProperties": {}}},
            "required": ["changes"]}}},
    {"type": "function", "function": {
        "name": "render",
        "description": "Render the current scene and get the picture with its statistics. Uses one render.",
        "parameters": {"type": "object", "properties": {
            "views": {"type": "integer", "enum": [1, 4],
                      "description": "1: the camera's view, large. 4: four views from around it."}},
            "required": []}}},
    {"type": "function", "function": {
        "name": "sweep",
        "description": "One image of the current scene with a value varied: a row of tiles, or a "
                       "grid when a second path is given (first path across, second down). Each "
                       "tile is labelled with its values. Nothing is changed; use `set` to keep "
                       "the value you like. Uses one render.",
        "parameters": {"type": "object", "properties": {
            "path": {"type": "string"},
            "values": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 6},
            "path2": {"type": "string"},
            "values2": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 4}},
            "required": ["path", "values"]}}},
    {"type": "function", "function": {
        "name": "get_scene",
        "description": "The current scene as TOML, with every `set` edit applied.",
        "parameters": {"type": "object", "properties": {}, "required": []}}},
    {"type": "function", "function": {
        "name": "finish",
        "description": "You are happy with it: the current scene is your final version.",
        "parameters": {"type": "object", "properties": {}, "required": []}}},
    {"type": "function", "function": {
        "name": "stop",
        "description": "End the session now, for any reason. No reason needed; never a failure.",
        "parameters": {"type": "object", "properties": {}, "required": []}}},
]


def png_scene(png):
    """The scene a render embedded in its own iTXt chunk, or None."""
    data = png.read_bytes()
    i = 8
    while i + 8 <= len(data):
        n, = struct.unpack(">I", data[i:i + 4])
        kind, body = data[i + 4:i + 8], data[i + 8:i + 8 + n]
        i += 12 + n
        if kind == b"iTXt":
            key, rest = body.split(b"\0", 1)
            if key != b"fracturize:scene":
                continue
            compressed, rest = rest[0], rest[2:]
            _, rest = rest.split(b"\0", 1)
            _, text = rest.split(b"\0", 1)
            return (zlib.decompress(text) if compressed else text).decode()
    return None


def fmt_value(v):
    """A JSON value as the right-hand side of -S."""
    if isinstance(v, list):
        return "[" + ",".join(str(x) for x in v) + "]"
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, str):
        if '"' in v:
            return v
        return f'"{v}"'
    return str(v)


class Session:
    def __init__(self, args, out):
        self.args, self.out = args, out
        self.fr = args.fracturize
        self.base = None          # path of the current base scene
        self.overrides = []       # -S strings applied on top
        self.n_scene = self.n_img = 0
        self.renders_left = args.renders
        self.finished = False
        self.stopped = False
        self.seen_last_render = False
        self.last_render_png = None
        self.last_sweep_png = None

    def write_scene(self, toml):
        if not toml or not toml.strip():
            return "write_scene: no TOML provided."
        self.n_scene += 1
        candidate = self.out / f"scene-{self.n_scene}.toml"
        candidate.write_text(toml.rstrip() + "\n")
        report, err = author.info_report(self.fr, candidate)
        if err:
            candidate.unlink(missing_ok=True)
            self.n_scene -= 1
            return f"The scene didn't load. The program said:\n\n{err}"
        self.base = candidate
        self.overrides = []
        self.seen_last_render = False
        return report

    def set(self, changes=None, **kwargs):
        if self.base is None:
            return "No scene loaded yet. Call write_scene first."
        if changes is None:
            if "path" in kwargs and "value" in kwargs:
                changes = {kwargs["path"]: kwargs["value"]}
            else:
                changes = {k: v for k, v in kwargs.items() if k != "changes"}
        if not isinstance(changes, dict) or not changes:
            return "set requires a 'changes' object mapping paths to values."

        new_overrides = []
        for path, val in changes.items():
            new_overrides.append(f"{path}={fmt_value(val)}")

        test_overrides = self.overrides + new_overrides
        cmd = [self.fr, "-s", str(self.base)]
        for ov in test_overrides:
            cmd += ["-S", ov]
        cmd += ["--info"]
        code, out, err = author.run(cmd)
        if code != 0 or not out.strip():
            msg = (err or out).strip().replace(str(self.base), self.base.name)
            return f"Could not apply changes. The program said:\n\n{msg}"

        self.overrides = test_overrides
        self.seen_last_render = False
        blocks = re.split(r"\n\s*\n", out.strip())
        keep = [b for b in blocks if b.split(None, 1)[0] in ("set", "notes", "shape")]
        text = re.sub(r"-S (\w+)\.(\w+)=(\S+)", r"[\1] \2 = \3", "\n\n".join(keep))
        return text

    def render(self, views=1):
        if self.renders_left <= 0:
            return "Render budget exhausted (0 renders left).", None
        if self.base is None:
            return "No scene loaded yet. Call write_scene first.", None

        self.renders_left -= 1
        self.n_img += 1
        png = self.out / f"render-{self.n_img}.png"

        cmd = [self.fr, "-s", str(self.base)]
        for ov in self.overrides:
            cmd += ["-S", ov]

        if views == 4:
            cmd += ["-r", str(png), "--splat", "--effort", "small", "--orbit-grid", "2x2",
                    "--width", "320", "--height", "320", "--no-labels"]
        else:
            cmd += ["-r", str(png), "--splat", "--effort", "small",
                    "--width", "480", "--height", "480"]

        code, out, err = author.run(cmd)
        if code != 0 or not png.exists():
            return f"Render failed:\n{(err or out).strip()[-1500:]}", None

        _, stats, _ = author.run([self.fr, "--stats", str(png)])
        lines = [l for l in stats.splitlines()
                 if l.strip() and not l.startswith("stats") and "histogram" not in l
                 and not re.match(r"^\s+[RYGCBM ]+$", l) and "pixels)" not in l]
        stats_text = "\n".join(lines)
        self.last_render_png = png
        self.seen_last_render = True
        return f"{stats_text}\n\n({self.renders_left} renders left)", png

    def sweep(self, path, values, path2=None, values2=None):
        if self.renders_left <= 0:
            return "Render budget exhausted (0 renders left).", None
        if self.base is None:
            return "No scene loaded yet. Call write_scene first.", None
        if not path or not values:
            return "sweep requires 'path' and 'values'.", None
        if not isinstance(values, list) or len(values) < 2:
            return "sweep 'values' must be an array of at least 2 numbers.", None

        self.renders_left -= 1
        self.n_img += 1
        png = self.out / f"render-{self.n_img}.png"

        axis1 = f"{path}={','.join(str(v) for v in values)}"
        cmd = [self.fr, "-s", str(self.base)]
        for ov in self.overrides:
            cmd += ["-S", ov]
        cmd += ["--sweep", axis1]

        if path2 and values2 and isinstance(values2, list) and len(values2) >= 2:
            axis2 = f"{path2}={','.join(str(v) for v in values2)}"
            cmd += ["--sweep", axis2]

        cmd += ["-r", str(png), "--splat", "--effort", "small",
                "--width", "320", "--height", "320"]

        code, out, err = author.run(cmd)
        if code != 0 or not png.exists():
            return f"Sweep failed:\n{(err or out).strip()[-1500:]}", None

        tile_lines = [l for l in out.splitlines() if l.startswith("tile ")]
        report = "\n".join(tile_lines) if tile_lines else "Sweep rendered."
        self.last_sweep_png = png
        self.seen_last_render = True
        return f"{report}\n\n({self.renders_left} renders left)", png

    def get_current_scene_toml(self):
        if self.base is None:
            return None
        if not self.overrides:
            return self.base.read_text()
        probe = self.out / ".scene_probe.png"
        cmd = [self.fr, "-s", str(self.base)]
        for ov in self.overrides:
            cmd += ["-S", ov]
        cmd += ["-r", str(probe), "--effort", "tiny", "--width", "16", "--height", "16"]
        code, _, _ = author.run(cmd)
        if code == 0 and probe.exists():
            sc = png_scene(probe)
            probe.unlink(missing_ok=True)
            if sc:
                return sc
        return self.base.read_text()

    def get_scene(self):
        sc = self.get_current_scene_toml()
        if sc is None:
            return "No scene loaded yet. Call write_scene first."
        return sc

    def finish(self):
        self.finished = True
        return "The current scene is marked as final."

    def stop(self):
        self.stopped = True
        return "Session stopped. Leaving now."

    def dispatch(self, name, args):
        if not isinstance(args, dict):
            args = {}
        if name == "write_scene":
            return self.write_scene(args.get("toml", "")), None
        elif name == "set":
            return self.set(changes=args.get("changes"), **{k: v for k, v in args.items() if k != "changes"}), None
        elif name == "render":
            return self.render(args.get("views", 1))
        elif name == "sweep":
            return self.sweep(
                args.get("path"),
                args.get("values"),
                args.get("path2"),
                args.get("values2")
            )
        elif name == "get_scene":
            return self.get_scene(), None
        elif name == "finish":
            return self.finish(), None
        elif name == "stop":
            return self.stop(), None
        else:
            avail = ", ".join(t["function"]["name"] for t in TOOLS)
            return f"Unknown tool '{name}'. Available tools: {avail}", None


class Run:
    def __init__(self, args, facts, out):
        self.args, self.facts, self.out = args, facts, out
        self.messages = []
        self.notes = []
        self.log = open(out / "transcript.jsonl", "a")
        self.md = open(out / "transcript.md", "a")

    def record_user(self, content):
        entry = {"role": "user", "content": content, "time": datetime.datetime.now().isoformat()}
        self.log.write(json.dumps(entry) + "\n")
        self.log.flush()
        text = content if isinstance(content, str) else "\n".join(
            p.get("text", "[image]") for p in content)
        self.md.write(f"\n## user\n\n{text}\n")
        self.md.flush()

    def record_system(self, content):
        entry = {"role": "system", "content": content, "time": datetime.datetime.now().isoformat()}
        self.log.write(json.dumps(entry) + "\n")
        self.log.flush()
        self.md.write(f"\n## system\n\n{content}\n")
        self.md.flush()

    def record_assistant(self, content, reasoning, tool_calls, extra=None):
        entry = {
            "role": "assistant",
            "content": content,
            "tool_calls": tool_calls,
            "time": datetime.datetime.now().isoformat(),
        }
        if reasoning:
            entry["reasoning"] = reasoning
        if extra:
            entry.update(extra)
        self.log.write(json.dumps(entry) + "\n")
        self.log.flush()
        self.md.write("\n## assistant\n\n")
        if reasoning:
            self.md.write(f"<details><summary>reasoning</summary>\n\n{reasoning}\n\n</details>\n\n")
        if content:
            self.md.write(f"{content}\n\n")
        if tool_calls:
            for tc in tool_calls:
                fn = tc.get("function", {})
                self.md.write(f"`call: {fn.get('name')}({fn.get('arguments', '')})`\n\n")
        self.md.flush()

    def record_tool(self, name, tool_call_id, content):
        entry = {
            "role": "tool",
            "name": name,
            "tool_call_id": tool_call_id,
            "content": content,
            "time": datetime.datetime.now().isoformat(),
        }
        self.log.write(json.dumps(entry) + "\n")
        self.log.flush()
        self.md.write(f"\n## tool: {name}\n\n")
        if isinstance(content, list):
            for p in content:
                if p.get("type") == "text":
                    self.md.write(f"{p.get('text', '')}\n\n")
                elif p.get("type") == "image_url":
                    self.md.write("`[image]`\n\n")
        else:
            self.md.write(f"{content}\n\n")
        self.md.flush()

    def say(self, content):
        self.record_user(content)
        self.messages.append({"role": "user", "content": content})

    def forget_old_images(self):
        """Keep only the newest image in the history; each costs context."""
        seen = False
        for m in reversed(self.messages):
            if isinstance(m.get("content"), list):
                if seen:
                    m["content"] = [p if p.get("type") == "text" else
                                    {"type": "text", "text": "[earlier render omitted]"}
                                    for p in m["content"]]
                seen = True

    def ask(self, tools=None):
        body = {
            "model": self.facts["model"],
            "messages": self.messages,
            "max_tokens": self.args.max_tokens,
            "chat_template_kwargs": {"enable_thinking": not self.args.no_think},
        }
        if tools:
            body["tools"] = tools
        if self.args.temperature is not None:
            body["temperature"] = self.args.temperature

        resp = http_json(self.args.base + "/v1/chat/completions", body)
        choice = resp["choices"][0]
        msg = choice["message"]
        content = msg.get("content") or ""
        reasoning = msg.get("reasoning_content") or ""
        tool_calls = msg.get("tool_calls") or []
        usage = resp.get("usage", {})
        finish_reason = choice.get("finish_reason")

        self.record_assistant(content, reasoning, tool_calls,
                              {"finish_reason": finish_reason, "usage": usage})

        asst_msg = {"role": "assistant"}
        if content:
            asst_msg["content"] = content
        if tool_calls:
            asst_msg["tool_calls"] = tool_calls
        if not content and not tool_calls:
            asst_msg["content"] = ""
        self.messages.append(asst_msg)

        for line in content.splitlines():
            if line.strip().upper().startswith("NOTE:"):
                self.notes.append(line.strip()[5:].strip())

        return content, reasoning, tool_calls, finish_reason, usage


def interview(r, unseen, vision):
    """Ask the closing questions; return the answer, or None.

    `unseen` is the last round's (report, png) when the loop ended before the
    model was shown it. It goes first, so the model isn't asked how it feels
    about a picture it never saw."""
    text = fill("interview.md")
    png = None
    if unseen:
        report, png = unseen
        seen = "\nThe picture shows four views from around it.\n" if vision and png.exists() else ""
        text = f"Here is how your final version came out:\n\n```\n{report}\n```\n{seen}\n" + text
    r.say(author.with_image(text, png, vision))
    r.forget_old_images()
    try:
        content, _, _, _, _ = r.ask(tools=None)
    except Exception as e:
        print(f"interview request failed: {e}")
        return None
    answer = author.TOML_BLOCK.sub("", content).strip()
    return answer or None


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("task", choices=["new", "free", "tweak"])
    ap.add_argument("--theme", help="the theme, for `new`")
    ap.add_argument("--scene", help="the scene to vary, for `tweak`")
    ap.add_argument("--renders", type=int, default=12, help="render budget [12]")
    ap.add_argument("--calls", type=int, default=40, help="tool call limit [40]")
    ap.add_argument("--author", help="what the model signs as [the server's model id]")
    ap.add_argument("--out", help="run directory [renders/llm/<time>-<task>-<model>]")
    ap.add_argument("--no-think", action="store_true", help="ask the template to skip thinking [thinking on]")
    ap.add_argument("--no-image", action="store_true", help="don't send renders, even to a vision model")
    ap.add_argument("--spark", type=int, default=0, metavar="N",
                    help="offer N random words as optional inspiration [off]")
    ap.add_argument("--spark-seed", type=int,
                    help="seed for the words, to reproduce a run's prompt [random, recorded]")
    ap.add_argument("--spark-words", default="/usr/share/dict/words",
                    help="word list to draw from [/usr/share/dict/words]")
    ap.add_argument("--no-scenes", action="store_true",
                    help="don't copy the final scene into scenes/local-llm/")
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
    if args.spark_seed is None:
        args.spark_seed = int.from_bytes(os.urandom(4), "big")
    if not Path(args.fracturize).exists():
        sys.exit(f"no fracturize binary at {args.fracturize}; cargo build --release first")

    # One session at a time, so each one can be read on its own afterwards.
    lockdir = REPO / "renders" / "llm"
    lockdir.mkdir(parents=True, exist_ok=True)
    lock = open(lockdir / ".lock", "w")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        sys.exit("another authoring session is running; one at a time")

    facts = server_facts(args.base)
    vision = facts["vision"] and not args.no_image
    model = facts["model"]
    author_name = args.author or model
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    out = Path(args.out) if args.out else REPO / "renders" / "llm" / f"{stamp}-{args.task}-{re.sub(r'[^A-Za-z0-9.]+', '-', model)}"
    out.mkdir(parents=True, exist_ok=True)
    print(f"model {model}, vision {'on' if vision else 'off'}, context {facts['n_ctx'] or 'unknown'}")
    print(f"writing to {out}")

    r = Run(args, facts, out)
    session = Session(args, out)

    system = fill("explore-system.md", model=model, author=author_name,
                  renders=args.renders, calls=args.calls,
                  primer=(HERE / "primer.md").read_text())
    r.messages.append({"role": "system", "content": system})
    r.record_system(system)

    if args.task == "new":
        task = fill("new.md", theme=args.theme)
    elif args.task == "free":
        task = fill("free.md")
    else:
        src = Path(args.scene)
        session.base = src
        info, err = author.info_report(args.fracturize, src)
        name = re.search(r'^name\s*=\s*"([^"]*)"', src.read_text(), re.M)
        task = fill("tweak.md", scene=src.read_text().strip(), info=info or err,
                    scene_name=name.group(1) if name else src.stem)

    sparks = None
    if args.spark > 0:
        sparks, pool = author.draw_sparks(args.spark, args.spark_seed, args.spark_words)
        task += fill("spark.md", words=", ".join(sparks))
        print(f"sparks: {', '.join(sparks)} (seed {args.spark_seed}, from {pool} words)")
    r.say(task)

    call_count = 0
    ending = "tool call budget reached"
    consecutive_nudges = 0

    while call_count < args.calls:
        print(f"step {call_count + 1}: asking…", flush=True)
        try:
            content, reasoning, tool_calls, finish, usage = r.ask(tools=TOOLS)
        except Exception as e:
            ending = f"request failed: {e}"
            break

        if author.has_line(content, "STOP") and not tool_calls:
            ending = "the model chose STOP"
            break
        if author.has_line(content, "DONE") and not tool_calls:
            ending = "the model said DONE"
            break
        if finish == "length" and not tool_calls:
            ending = "a reply ran out of room before any tool call (see transcript)"
            break

        if not tool_calls:
            consecutive_nudges += 1
            if consecutive_nudges >= 3:
                ending = "the model stopped calling tools"
                break
            blocks = author.TOML_BLOCK.findall(content)
            if blocks:
                r.say("Please use the `write_scene` tool to submit your scene TOML.")
            else:
                r.say("Please call one of the tools (write_scene, set, render, sweep, get_scene, finish, or stop) to proceed.")
            continue

        consecutive_nudges = 0

        for tc in tool_calls:
            call_count += 1
            fn = tc.get("function", {})
            fn_name = fn.get("name", "")
            raw_args = fn.get("arguments", "{}")
            try:
                fn_args = json.loads(raw_args) if isinstance(raw_args, str) else (raw_args or {})
            except Exception as e:
                fn_args = {}
                result_text, png = f"Invalid JSON arguments: {e}", None
            else:
                result_text, png = session.dispatch(fn_name, fn_args)

            first_line = result_text.splitlines()[0] if result_text else "ok"
            print(f"call {call_count}: {fn_name} -> {first_line[:70]}", flush=True)

            tc_id = tc.get("id") or f"call_{call_count}"
            if vision and png is not None and png.exists():
                tool_content = author.with_image(result_text, png, vision)
                r.record_tool(fn_name, tc_id, tool_content)
                r.messages.append({
                    "role": "tool",
                    "name": fn_name,
                    "tool_call_id": tc_id,
                    "content": tool_content,
                })
                r.forget_old_images()
            else:
                r.record_tool(fn_name, tc_id, result_text)
                r.messages.append({
                    "role": "tool",
                    "name": fn_name,
                    "tool_call_id": tc_id,
                    "content": result_text,
                })

            if session.stopped:
                ending = "the model chose stop"
                break
            if session.finished:
                ending = "the model called finish"
                break
            if call_count >= args.calls:
                ending = "tool call budget reached"
                break

        if session.stopped or session.finished:
            break

        if facts["n_ctx"] and usage.get("prompt_tokens", 0) + usage.get("completion_tokens", 0) > 0.8 * facts["n_ctx"]:
            ending = "context nearly full"
            break

    # Session wrapping up
    final_toml = session.get_current_scene_toml()
    final_png = out / "final.png"
    unseen = None

    if final_toml and session.base:
        if not session.seen_last_render:
            # The model made changes after its last render (or never rendered)
            cmd = [args.fracturize, "-s", str(session.base)]
            for ov in session.overrides:
                cmd += ["-S", ov]
            cmd += ["-r", str(final_png), "--splat", "--effort", "small",
                    "--orbit-grid", "2x2", "--width", "320", "--height", "320", "--no-labels"]
            code, out_txt, err = author.run(cmd)
            if code == 0 and final_png.exists():
                _, stats, _ = author.run([args.fracturize, "--stats", str(final_png)])
                lines = [l for l in stats.splitlines()
                         if l.strip() and not l.startswith("stats") and "histogram" not in l
                         and not re.match(r"^\s+[RYGCBM ]+$", l) and "pixels)" not in l]
                unseen = ("\n".join(lines), final_png)
        else:
            if session.last_render_png and session.last_render_png.exists():
                final_png.write_bytes(session.last_render_png.read_bytes())
            elif session.last_sweep_png and session.last_sweep_png.exists():
                final_png.write_bytes(session.last_sweep_png.read_bytes())

    answer = None
    if ending not in ("the model chose stop", "the model chose STOP") and not ending.startswith("request failed"):
        print("closing interview…", flush=True)
        answer = interview(r, unseen, vision)

    if final_toml:
        header = author.as_comment(model, answer) if answer else ""
        (out / "final.toml").write_text(header + final_toml)
        if not args.no_scenes:
            placed = author.place_in_scenes(out)
            print(f"added to the scene browser: {placed.relative_to(REPO)}")

    summary = [f"# {args.task} by {model}", "",
               f"- ended: {ending}",
               f"- renders used: {args.renders - session.renders_left} of {args.renders}",
               f"- tool calls: {call_count} of {args.calls}",
               f"- final scene: {'final.toml' if final_toml else 'none'}",
               f"- vision: {'on' if vision else 'off'}"]
    if args.theme:
        summary.append(f"- theme: {args.theme}")
    if sparks:
        summary.append(f"- sparks: {', '.join(sparks)} (--spark {args.spark} "
                       f"--spark-seed {args.spark_seed}, {args.spark_words})")
    if r.notes:
        summary += ["", "## Notes from the model", ""] + [f"> {n}" for n in r.notes]
    if answer:
        summary += ["", "## Closing interview", ""] + [f"> {l}" if l else ">" for l in answer.splitlines()]
    elif ending in ("the model chose stop", "the model chose STOP"):
        summary += ["", "(No closing interview: the model chose STOP, and that is respected.)"]
    (out / "summary.md").write_text("\n".join(summary) + "\n")
    print("\n".join(summary))
    print(f"\nreview the transcript with: {HERE / 'review.sh'} {out}")


if __name__ == "__main__":
    main()
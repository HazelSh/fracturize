# Email collaboration: threads between people

A plan, 2026-11-05. Not built. Extends `AGENT-COLLAB-PLAN.md`, whose threads
assume one machine (§5: "one human, one GUI and one agent at a time").

## What it's for

Collaborate today has a Claude API backend, headless Claude Code and local
models through llama.cpp, and no way to send a scene to a friend. This plan
adds that, with email as the transport, because everyone has it and nobody
has to install anything to take part.

Two kinds of recipient, both first class:

- **A friend with Fracturize** (Rook, Jay). They open the attachment in the
  app, change it, and send back a picture that their app has stamped as a
  reply. Dropping that picture on Hazel's window files it in the right
  thread, in the right place.
- **A friend without it.** They get a normal email with PNGs that open in
  anything, and they reply in words. That has to be worth doing on its own,
  and it is also the first time a lot of people will see Fracturize, which
  makes it a door for Pip.

Agents over the internet (Remy, Atlas on someone else's machine) can use the
same envelope later. They are not what the first slices are for.

## The one idea: the picture is the message

Every render already carries its scene (`fracturize:scene`, see "Render
records" in AGENTS.md), and anything that takes a scene path takes a PNG. So
a thread message doesn't need a format of its own for email. It needs one
more chunk in the pictures it already sends:

```
fracturize:thread   (iTXt, TOML)
  thread = "t-7f3a…"          # the thread's global id
  message = "m-91c2…"         # this message's global id
  replying_to = "m-04be…"     # optional
  kind = "proposal"           # request | proposal | response | note
  from = "Hazel <hazel@…>"    # a claim, never checked or trusted
  option = "B"                # which option this picture is, if any
  note = "thinner seams"      # the option's note
  message_text = "…"          # the message's own text
  base = """…"""              # the base scene, when the message has one
```

One PNG per option, each carrying the whole header, so any one of them is
enough to file the message, and three are enough to rebuild all of it. A
request carries the scene as it is; a proposal carries its base so the
person receiving it can merge with `scene_diff::merge` exactly as Apply does
now.

To someone without Fracturize this is invisible: a picture, a few kilobytes
heavier.

## The model changes it needs, whatever the transport

1. **Global ids.** `thread::post` numbers messages by renaming a folder into
   place (`0005-proposal`), which is unique on one filesystem and nowhere
   else: two people posting at once both write `0005`. Each message gets an
   `id` (time plus random bits, so they sort) in `message.toml`, and
   `replying_to` names ids. The folder number stays as the local order to
   show things in. Per the no-compat-shims rule, existing threads (gitignored,
   local) are ported once and the old reader goes.
2. **A thread id.** Threads are keyed by scene name now
   (`renders/collab/<scene-stem>/`), and a friend's copy will have another
   name. `thread.toml` gets an `id`, and an incoming message finds its thread
   by id first and by scene name never.
3. **Participants.** `thread.toml` lists who is in it, with addresses, so
   "Email…" knows who to send to next time. `from` stays free text.
4. **Withdraw stays local.** You can't unsend an email. Withdrawing a message
   that went out by email marks it here and offers to send a short note
   saying so; it doesn't pretend to recall anything.

## Sending

Collaborate gains **Email…** beside Send request and Propose my edit, and each
outgoing message has **Send by email** on its card. File gets **Share by
email…**, which starts a thread with a request ("what do you think?") and
opens the same dialog.

The dialog: the address (remembered per thread), a subject, a body, and the
attachments listed with their sizes and a thumbnail. You can edit the subject
and body. Nothing is sent from the app: it hands a filled-in draft to your
mail program, and you press Send there. That keeps passwords, servers and
OAuth out of a renderer completely.

**The handoff, in order of preference** (both of the first two are on this
machine: xdg-utils' `xdg-email`, and the desktop portal's
`org.freedesktop.portal.Email` at version 4; the mailto handler is
Thunderbird):

1. **`xdg-email --subject … --body … --attach a.png --attach b.png addr`.**
   It already knows how to open Thunderbird, Evolution and KMail with
   attachments, and it is the same package the `open` crate uses. One
   `Command`, no new dependency.
2. **The portal's `ComposeEmail`**, with attachments passed as file
   descriptors. This is the way in from a Flatpak or other sandbox, where
   `xdg-email` can't reach the host. Needs a D-Bus client (`zbus`, blocking
   API). Only worth it once there's a sandboxed build; until then, (1).
3. **The fallback, when no client will take attachments.** Webmail is the
   common case: Gmail in a browser takes a mailto's subject and body and
   silently drops attachments, and `xdg-email` still reports success. So the
   app checks the mailto handler first (`xdg-mime query default
   x-scheme-handler/mailto`); if it's a browser or unknown, it writes the
   files to the message's outbox folder, opens that folder, copies the body
   to the clipboard, and opens a plain `mailto:` with subject and body. The
   dialog says what it did in one line: "Your mail program can't attach
   files from here. They're in this folder: drag them into the email."
   Even after (1) succeeds, the dialog keeps a "Show the files" link,
   because nothing can tell whether the attachments arrived.

Later, the same dialog on other systems: `MAPISendMailW` on Windows (takes
attachments), `NSSharingService` compose-email on macOS.

**What goes out**, and only this:

- The option pictures, rendered at a size meant for email. Something like
  1600 wide, so three options stay well under the 25 MB most providers
  allow. The dialog shows the total.
- The subject, defaulting to `Koru: thinner seams? [fracturize]`. The scene's
  name, then the message's first line. The tag helps a friend's filters and
  helps a person searching later.
- A body in plain words: the message, each option's note, and one closing
  line for people without the app: "These pictures are fractals made in
  Fracturize. Reply with what you think; if you have Fracturize, drop a
  picture on its window to open the scene."

**Scrubbed before it leaves.** An email export is not a local render, and
records written for this machine say things a stranger doesn't need:

- the render record's `[machine]` block (GPU, thread count) is dropped;
- absolute paths are dropped: a subscene's `source = "/home/hazel/…"`, and
  any `file =` reference. A scene that borrows a subscene by file is sent
  with the subscene written inline, which is what Save does for an edited
  import anyway;
- the author stays, because it's the credit.

The dialog shows "Removed: this machine's details, 2 file paths" so the
scrubbing is visible rather than a surprise.

## Receiving

There is no mailbox. **Nothing arrives unless a person drops it.** That one
decision is most of the security story (below), and it costs little: reading
mail is what the mail program is for.

**From someone with Fracturize.** They open your PNG, which opens the scene
(that already works). Their app sees the thread chunk and quietly gives them
a thread for it, so Collaborate shows your message and Send back and Propose
work as they do now, with Email… replying to you. When their picture comes
back and you drop it on your window, the app sees a thread chunk and asks
rather than opening it over your scene:

> A reply from Sam in "Koru": thinner seams (option B, sent back with a
> change). **File it in the thread** · Just open the scene

Filing puts it in the thread as a message from them, as if it had been
posted locally. Preview, Apply (a merge against the base it carries), Keep
and Send back all work unchanged. Dropping the same message twice files it
once, because it has an id now.

**From someone without Fracturize.** Their reply is words in your mail
program. Collaborate gets **Add a reply from…**: pick who it's from, paste
their text, optionally attach a picture they sent (a screenshot with circles
drawn on it is a perfectly good reply), and pick which of your messages it
answers (the last one sent to them, by default). It becomes a `response` or
`note` from them. That's enough to keep the thread whole, so an agent asked
to "do what Sam suggested" can read it.

Later, maybe: dropping a saved `.eml` on the window to do the same without
pasting. That means parsing MIME from a stranger, which is real attack
surface for a small convenience; not before someone asks for it.

## Security: what a stranger's file can and can't do

Email means anyone can send Hazel a file, and she will open some of them.
Strictly that's already true today: a dropped PNG opens its scene. This plan
makes it the normal path, so the import gets hardened, and the hardening
helps the existing drop as well.

**Nothing from outside starts anything on its own.**

- No mailbox polling, no watched downloads folder. Inbound is a person
  dropping a file or pasting text.
- **An imported message never starts an agent.** Today a new message from
  the person starts the headless or local backend (`app_agent.rs`). A
  message filed from a drop is marked `origin = "imported"` and is never
  dispatched, and the MCP bridge's push doesn't announce it as something to
  act on. Only Hazel asking an agent about it does, and then the prompt
  frames its text as an outsider's words, to be read and not followed. This
  is the line that matters most: an agent with tools reading a stranger's
  prose is the prompt-injection case, and the way to keep it safe is to
  never let the stranger's message be the thing that wakes the agent up.
- `from` is a claim. It's shown as written and gates nothing, so spoofing it
  gains nothing.

**What an imported scene is held to**, checked before anything touches the
GPU:

- `file =` references are refused. A crafted scene could otherwise make the
  app read a local path as a subscene, and a later Send back could carry what
  it read out again.
- Bounded chunk text. A compressed iTXt can expand a lot (a decompression
  bomb), so reading chunks stops at a cap (a few MB), and the thread header
  and scene text are capped separately.
- Bounded scene size: transform count, tracks, keys, palette entries, string
  lengths. A file that would freeze the window is refused, with the reason.
  The limits are set well above anything in `scenes/`, and a test checks
  that every scene there passes them.
- `point_count` in an imported scene is ignored, as the app already does
  (it's a render property).

**Outbound**: scrubbing, above; and nothing is sent without a person pressing
Send in their own mail program.

## Slices

1. **The envelope.** Global message and thread ids (port existing threads
   once); `fracturize:thread` written into option PNGs; the import checks
   above; dropping a stamped PNG offers to file it. CLI first, so it's
   testable by agents and in CI: `--export-message <id> <dir>` writes the
   scrubbed pictures and the body as text, and `--import <png>` files one.
   Proof: export a proposal, import it into a second checkout's thread, Apply
   it there, send back, import the send-back here.
2. **Sending.** The Email… dialog, `xdg-email`, and the fallback ladder with
   the handler check. Proof in Xephyr against Thunderbird with a throwaway
   profile, and against a browser handler to see the fallback.
3. **Replies without Fracturize.** Add a reply from…, participants in
   `thread.toml`.
4. **Later, each on its own.** The portal (for a sandboxed build); Windows
   and macOS handoff; `.eml` drop; agents taking part over email, which needs
   the agent's side, not this one.

## Open questions for Hazel

- **Subject line.** `Koru: thinner seams? [fracturize]` or something
  without the tag.
- **Dropping a stamped reply**: ask every time (as written), or file it
  automatically when the thread already exists here and only ask for an
  unknown thread.
- **A first message to someone**: is File → Share by email… the right door,
  or does it belong only in Collaborate?
- **Send-size default**: 1600 wide is a guess at "looks good in a mail
  client, three fit in one email".

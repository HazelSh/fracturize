//! The command-line side of a thread (AGENT-COLLAB-PLAN.md §7): an agent's
//! way in, with nothing but a shell. Every operation is a flag that does one
//! thing to `renders/collab/` and exits; the thread is the scene's, named by
//! `-s` like everything else. The format and the reading are `thread.rs`.
//!
//! ```sh
//! fracturize --list-threads [--waiting]
//! fracturize -s s.toml --thread [--since 0003]
//! fracturize -s s.toml --request "frame it the way you'd hang it"
//! fracturize -s s.toml --propose my-proposal/
//! fracturize -s s.toml --respond 0004 --verdict "B, thinner" --send-back B --note "…"
//! fracturize -s s.toml --wait-reply --timeout 30m
//! fracturize -s s.toml --withdraw 0005 --reason "…"
//! ```

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::thread::{self, Answer, Draft, Header, Kind, Thread, Verdict};
use crate::Args;

/// Who the caller is: `--from`, else `$FRACTURIZE_AGENT`, else `agent`.
fn me(args: &Args) -> String {
    args.from
        .clone()
        .or_else(|| std::env::var("FRACTURIZE_AGENT").ok().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| "agent".to_string())
}

/// Run the thread operation the flags ask for, if any, and return the exit
/// status. `None`: no thread flag was given.
pub fn run(args: &Args) -> Option<i32> {
    let any = args.list_threads
        || args.thread
        || args.request.is_some()
        || args.propose.is_some()
        || args.respond.is_some()
        || args.wait_reply
        || args.withdraw.is_some();
    if !any {
        if args.from.is_some() || args.to.is_some() || args.reason.is_some() {
            eprintln!("--from, --to and --reason go with a thread operation: --request, --propose, --respond, --withdraw");
            return Some(1);
        }
        return None;
    }
    let ops = [
        args.list_threads,
        args.thread,
        args.request.is_some(),
        args.propose.is_some(),
        args.respond.is_some(),
        args.wait_reply,
        args.withdraw.is_some(),
    ];
    if ops.iter().filter(|&&b| b).count() > 1 {
        eprintln!("one thread operation at a time: --list-threads, --thread, --request, --propose, --respond, --wait-reply or --withdraw");
        return Some(1);
    }
    let result = if args.list_threads { list(args) } else { scene_op(args) };
    Some(match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    })
}

fn list(args: &Args) -> Result<i32, String> {
    let me = me(args);
    let threads = Thread::all(Path::new(thread::ROOT));
    let mut shown = 0;
    for t in &threads {
        let waiting: Vec<_> = t.messages.iter().filter(|m| t.waiting(m)).collect();
        let for_me = waiting.iter().filter(|m| m.header.from != me).count();
        if args.waiting && for_me == 0 {
            continue;
        }
        shown += 1;
        let last = t.messages.last().map_or("empty".to_string(), |m| format!("last {} {}", m.id, thread::when(m.header.created)));
        println!("{:<24} {:<32} {:>3} messages · {last}", t.name(), t.file.scene, t.messages.len());
        for m in waiting {
            let on = if m.header.from == me { "waiting on them" } else { "waiting on you" };
            println!("    {} from {}: {} [{on}]", m.id, m.header.from, m.header.headline());
        }
    }
    if shown == 0 {
        println!("{}", if args.waiting { "nothing waiting for you" } else { "no threads yet" });
    }
    Ok(0)
}

/// The operations on one scene's thread.
fn scene_op(args: &Args) -> Result<i32, String> {
    let Some(scene_path) = args.scene.as_deref() else {
        return Err("a thread is a scene's: name it with -s".to_string());
    };
    let path = Path::new(scene_path);
    let scene = crate::scene::Scene::load(path).map_err(|e| format!("{scene_path}: {e}"))?;
    let dir = thread::dir_for(Some(path), &scene.name);
    let me = me(args);
    let open = || Thread::open(&dir);

    if args.thread {
        let t = open()?;
        if t.messages.is_empty() {
            println!("no thread yet for {scene_path} ({} is empty)", dir.display());
            return Ok(0);
        }
        let since = match &args.since {
            Some(id) => Some(t.get(id).ok_or_else(|| format!("--since: no message {id}"))?.number()),
            None => None,
        };
        print!("{}", thread::render_text(&t, since));
        return Ok(0);
    }

    if let Some(text) = &args.request {
        let t = open()?;
        let to = args.to.clone().unwrap_or_else(|| t.other_party(&me, "human"));
        let mut draft = Draft::new(Header::new(Kind::Request, &me, &to, text));
        draft.scene("base.toml", &scene)?;
        let id = thread::post(&dir, scene_path, draft)?;
        println!("posted {} to {}", id, dir.display());
        return Ok(0);
    }

    if let Some(src) = &args.propose {
        let t = open()?;
        let to = args.to.clone().unwrap_or_else(|| t.other_party(&me, "human"));
        let mut draft = thread::proposal_from_dir(src, &me, &to).map_err(|e| format!("--propose {}:\n{e}", src.display()))?;
        if let Some(r) = &draft.header.replying_to {
            let Some(m) = t.get(r) else {
                return Err(format!("--propose: replying_to = {r}, which isn't in {}", dir.display()));
            };
            // Written in full, as everything that reads the thread expects.
            draft.header.replying_to = Some(m.id.clone());
        }
        if let Some((msg, opt)) = draft.header.revises.as_deref().and_then(|r| r.split_once('/')) {
            let Some(m) = t.get(msg) else {
                return Err(format!("--propose: revises = {msg}/{opt}, but there's no message {msg} in {}", dir.display()));
            };
            draft.header.revises = Some(format!("{}/{opt}", m.id));
        }
        let n = draft.header.options.len();
        let id = thread::post(&dir, scene_path, draft)?;
        println!("posted {id} ({n} option{}) to {}", if n == 1 { "" } else { "s" }, dir.display());
        let t = open()?;
        if let Some(m) = t.get(&id) {
            print!("{}", thread::render_message(&t, m));
        }
        return Ok(0);
    }

    if let Some(target) = &args.respond {
        return respond(args, &dir, scene_path, &me, target).map(|()| 0);
    }

    if let Some(id) = &args.withdraw {
        let t = open()?;
        let m = t.get(id).ok_or_else(|| format!("--withdraw: no message {id}"))?;
        if m.header.from != me {
            return Err(format!("--withdraw: {} is from {}, not {me}; only its sender can take it back", m.id, m.header.from));
        }
        let id = thread::withdraw(&t, id, args.reason.as_deref().unwrap_or(""))?;
        println!("withdrew {id}");
        return Ok(0);
    }

    if args.wait_reply {
        return wait(args, &dir, &me);
    }
    unreachable!("run() checked for an operation")
}

fn respond(args: &Args, dir: &Path, scene_path: &str, me: &str, target: &str) -> Result<(), String> {
    let t = Thread::open(dir)?;
    let m = t.get(target).ok_or_else(|| format!("--respond: no message {target} in {}", dir.display()))?;
    if m.header.kind != Kind::Proposal {
        return Err(format!("--respond answers a proposal; {} is a {}. Answer a request with --propose.", m.id, m.header.kind.name()));
    }
    if m.withdrawn.is_some() {
        return Err(format!("{} was withdrawn", m.id));
    }
    let mut header = Header::new(Kind::Response, me, &m.header.from, args.verdict.as_deref().unwrap_or(""));
    header.replying_to = Some(m.id.clone());
    let mut draft_files = Vec::new();
    let check = |id: &str| -> Result<(), String> {
        m.option(id).map(|_| ()).ok_or_else(|| {
            let ids: Vec<_> = m.header.options.iter().map(|o| o.id.as_str()).collect();
            format!("{} has no option {id} (it has {})", m.id, ids.join(", "))
        })
    };
    for id in &args.pick {
        check(id)?;
        header.answers.push(answer(id, Verdict::Picked, ""));
    }
    for id in &args.reject {
        check(id)?;
        header.answers.push(answer(id, Verdict::Rejected, args.reason.as_deref().unwrap_or("")));
    }
    if let Some(id) = &args.send_back {
        check(id)?;
        let mut a = answer(id, Verdict::SentBack, args.note.as_deref().unwrap_or(""));
        if let Some(tweak) = &args.tweak {
            crate::scene::Scene::load(tweak).map_err(|e| format!("--tweak {}: {e}", tweak.display()))?;
            let name = format!("{id}.tweak.toml");
            draft_files.push((name.clone(), tweak.clone()));
            a.tweak = Some(name);
        }
        header.answers.push(a);
    }
    let mut said = std::collections::BTreeSet::new();
    for a in &header.answers {
        if !said.insert(a.option.as_str()) {
            return Err(format!("--respond: option {} is given two verdicts; pick, reject or send it back, not more than one", a.option));
        }
    }
    if header.answers.is_empty() && header.message.trim().is_empty() {
        return Err("--respond needs a --verdict, or --pick / --reject / --send-back".to_string());
    }
    if m.header.ask == Some(thread::Ask::PickOne) && args.pick.len() > 1 {
        return Err(format!("{} asks you to pick one, not {}", m.id, args.pick.len()));
    }
    let mut draft = Draft::new(header);
    for (name, from) in draft_files {
        draft.files.insert(name, thread::Content::Copy(from));
    }
    let id = thread::post(dir, scene_path, draft)?;
    println!("posted {id}, answering {}", m.id);
    Ok(())
}

fn answer(id: &str, verdict: Verdict, note: &str) -> Answer {
    Answer { option: id.to_string(), verdict, note: note.to_string(), tweak: None, tweak_render: None }
}

/// `90s`, `30m`, `2h`, or bare seconds.
fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(s.len()));
    let n: f64 = num.parse().map_err(|_| format!("--timeout {s}: say 90s, 30m or 2h"))?;
    let secs = match unit {
        "" | "s" => n,
        "m" => n * 60.0,
        "h" => n * 3600.0,
        _ => return Err(format!("--timeout {s}: say 90s, 30m or 2h")),
    };
    Ok(Duration::from_secs_f64(secs))
}

fn wait(args: &Args, dir: &Path, me: &str) -> Result<i32, String> {
    let timeout = args.timeout.as_deref().map(parse_duration).transpose()?;
    let start_thread = Thread::open(dir)?;
    let seen = start_thread.messages.iter().map(|m| m.number()).max().unwrap_or(0);
    let open_before: Vec<PathBuf> =
        start_thread.messages.iter().filter(|m| m.withdrawn.is_none()).map(|m| m.dir.clone()).collect();
    eprintln!("waiting in {} for a message not from {me}…", dir.display());
    let start = Instant::now();
    loop {
        let t = Thread::open(dir)?;
        let new: Vec<_> = t.messages.iter().filter(|m| m.number() > seen && m.header.from != me).collect();
        if !new.is_empty() {
            for m in new {
                println!("{}", thread::render_message(&t, m));
            }
            return Ok(0);
        }
        if let Some(w) = t.messages.iter().find(|m| m.withdrawn.is_some() && m.header.from != me && open_before.contains(&m.dir)) {
            println!("{}", thread::render_message(&t, w));
            return Ok(4);
        }
        if timeout.is_some_and(|d| start.elapsed() >= d) {
            eprintln!("--wait-reply: nothing arrived in {}", args.timeout.as_deref().unwrap_or(""));
            return Ok(3);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_as_people_write_them() {
        assert_eq!(parse_duration("90s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_duration("5").unwrap(), Duration::from_secs(5));
        assert!(parse_duration("soon").is_err());
    }
}

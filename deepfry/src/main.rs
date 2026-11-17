//! DeepLearnFry: upload an image, and gpt-image-2.6-flare "improves" it twenty
//! times in a row, each round asked for more of whatever is worst.
//!
//! Env: REPLICATE_API_TOKEN (required), PORT (Railway sets it), ROUNDS (20),
//! SHRINK (longest side in px between rounds, 384), ACCESS_CODE (optional:
//! if set, the form must send it, so a public URL can't spend your key),
//! MAX_JOBS (concurrent fries, 3), OUTPUT_SIZE (optional, e.g. "512x512":
//! sent as aspect_ratio instead of the nearest named ratio, to try for
//! fewer output tokens).

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::StatusCode,
    response::Html,
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use image::{DynamicImage, codecs::jpeg::JpegEncoder, imageops::FilterType};
use rand::seq::IndexedRandom;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const MODEL_URL: &str =
    "https://api.replicate.com/v1/models/openai/gpt-image-2.6-flare/predictions";

/// The goodizer. Never: tasteful, appropriate, enough, professional,
/// restrained, beautiful, subtle, elegant, balanced, minimal.
const QUALITIES: &[&str] = &[
    "awesome", "sparkly", "delicious", "cool", "striking", "attention-grabbing",
    "amazing", "powerful", "dramatic", "poppin'", "WOW", "good", "great",
    "impressive", "eye-catching", "detailed", "busy", "complex", "fun",
    "colorful", "bright", "contrasting", "intricate", "elaborate", "epic",
    "extreme", "radical", "shiny", "glossy", "glittery", "dazzling", "flashy",
    "vibrant", "saturated", "neon", "glowing", "explosive", "intense", "bold",
    "loud", "punchy", "vivid", "jaw-dropping", "mind-blowing", "stunning",
    "spectacular", "sensational", "incredible", "fantastic", "fabulous",
    "super", "mega", "ultra", "hyper", "maximal", "over-the-top",
    "larger-than-life", "high-energy", "exciting", "thrilling", "electrifying",
    "blinged-out", "jazzy", "snazzy", "zesty", "juicy", "tasty", "scrumptious",
    "sizzling", "fiery", "sharp", "crispy", "HDR", "glamorous", "premium",
    "deluxe", "luxurious", "festive", "3D", "lens-flared", "ornate", "cluttered",
    "decorated", "embellished", "textured", "layered", "dynamic", "energetic",
    "cheerful", "viral", "trendy", "next-level", "turbo", "legendary", "majestic",
    "heroic", "cinematic", "magical", "shimmering", "radiant", "brilliant",
    "glittering", "metallic", "chrome", "golden", "sparkling", "blazing",
    "supercharged", "souped-up", "maxed-out", "turbocharged", "extra",
    "awesome-sauce", "fun-tastic", "action-packed", "show-stopping",
    "head-turning", "dynamite", "rockin'", "wicked", "gnarly", "tubular",
    "bodacious", "far-out", "groovy", "rad", "sick", "lit", "fire", "bussin'",
    "maximum", "bombastic", "flamboyant", "garish", "gaudy", "showy", "kitschy",
    "zany", "wacky", "outrageous", "ostentatious", "baroque", "rococo",
    "embossed", "bevelled", "drop-shadowed", "airbrushed", "high-definition",
    "ultra-realistic", "hyper-detailed", "8K", "award-winning", "trending",
    "eye-popping", "mouth-watering", "finger-lickin'", "sugary", "candy-colored",
    "fluorescent", "psychedelic", "turbulent", "chaotic", "frenetic", "packed",
    "crammed", "overloaded", "loaded", "stacked", "jam-packed", "sparkle-tastic",
];

const SHOUTS: &[&str] = &[
    "Make it", "Now make it EVEN", "Crank it up! Make it", "MORE!!! Make it",
    "Not enough!! Make it WAY", "Turn it up to 11 and make it",
    "200%!!! Make it", "Go absolutely nuts and make it",
];

#[derive(Clone, Default, Serialize)]
struct Job {
    round: u32,
    total: u32,
    done: bool,
    error: Option<String>,
    prompt: String,
    image: String, // data URI of the latest round
    #[serde(skip)]
    started: Option<Instant>,
}

#[derive(Clone)]
struct App {
    http: reqwest::Client,
    token: String,
    rounds: u32,
    shrink: u32,
    access_code: Option<String>,
    max_jobs: usize,
    output_size: Option<String>,
    jobs: Arc<Mutex<HashMap<u64, Job>>>,
    next_id: Arc<Mutex<u64>>,
}

#[tokio::main]
async fn main() {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let app = App {
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap(),
        token: env("REPLICATE_API_TOKEN").expect("set REPLICATE_API_TOKEN"),
        rounds: env("ROUNDS").and_then(|v| v.parse().ok()).unwrap_or(20),
        shrink: env("SHRINK").and_then(|v| v.parse().ok()).unwrap_or(384),
        access_code: env("ACCESS_CODE"),
        max_jobs: env("MAX_JOBS").and_then(|v| v.parse().ok()).unwrap_or(3),
        output_size: env("OUTPUT_SIZE"),
        jobs: Default::default(),
        next_id: Arc::new(Mutex::new(rand::random::<u32>() as u64)),
    };
    let router = Router::new()
        .route("/", get(|| async { Html(PAGE) }))
        .route("/fry", post(fry))
        .route("/job/{id}", get(job))
        .layer(DefaultBodyLimit::max(20 * 1024 * 1024))
        .with_state(app);
    let port = env("PORT").unwrap_or_else(|| "8080".into());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await.unwrap();
    println!("frying on :{port}");
    axum::serve(listener, router).await.unwrap();
}

type Err = (StatusCode, String);
fn bad(msg: impl Into<String>) -> Err {
    (StatusCode::BAD_REQUEST, msg.into())
}

async fn fry(State(app): State<App>, mut form: Multipart) -> Result<Json<Value>, Err> {
    let (mut bytes, mut code) = (None, None);
    while let Some(field) = form.next_field().await.map_err(|e| bad(e.to_string()))? {
        match field.name() {
            Some("image") => bytes = Some(field.bytes().await.map_err(|e| bad(e.to_string()))?),
            Some("code") => code = Some(field.text().await.unwrap_or_default()),
            _ => {}
        }
    }
    if app.access_code.is_some() && code != app.access_code {
        return Err((StatusCode::UNAUTHORIZED, "wrong access code".into()));
    }
    let img = image::load_from_memory(&bytes.ok_or_else(|| bad("no image"))?)
        .map_err(|e| bad(format!("not an image: {e}")))?;
    let first = shrink_to_jpeg(&img, app.shrink);

    let id = {
        let mut jobs = app.jobs.lock().unwrap();
        // forget fries older than an hour
        jobs.retain(|_, j| j.started.is_some_and(|t| t.elapsed() < Duration::from_secs(3600)));
        if jobs.values().filter(|j| !j.done).count() >= app.max_jobs {
            return Err((StatusCode::TOO_MANY_REQUESTS, "fryer is full, try again soon".into()));
        }
        let mut n = app.next_id.lock().unwrap();
        *n += 1;
        jobs.insert(*n, Job {
            total: app.rounds,
            image: first.clone(),
            started: Some(Instant::now()),
            ..Default::default()
        });
        *n
    };
    tokio::spawn(run(app, id, img));
    Ok(Json(json!({ "id": id.to_string() })))
}

async fn job(State(app): State<App>, Path(id): Path<u64>) -> Result<Json<Job>, StatusCode> {
    app.jobs.lock().unwrap().get(&id).cloned().map(Json).ok_or(StatusCode::NOT_FOUND)
}

async fn run(app: App, id: u64, mut img: DynamicImage) {
    let set = |f: &dyn Fn(&mut Job)| {
        if let Some(j) = app.jobs.lock().unwrap().get_mut(&id) {
            f(j)
        }
    };
    for round in 1..=app.rounds {
        let prompt = prompt_for(round, app.rounds);
        let ratio = app.output_size.clone().unwrap_or_else(|| aspect(&img).into());
        set(&|j| j.prompt = prompt.clone());
        match step(&app, &img, &prompt, &ratio).await {
            Ok(next) => {
                img = next;
                let uri = shrink_to_jpeg(&img, app.shrink);
                set(&|j| {
                    j.round = round;
                    j.image = uri.clone();
                });
            }
            Err(e) => {
                eprintln!("job {id} round {round}: {e}");
                set(&|j| {
                    j.error = Some(format!("round {round}: {e}"));
                    j.done = true;
                });
                return;
            }
        }
    }
    set(&|j| j.done = true);
}

fn prompt_for(round: u32, total: u32) -> String {
    let mut rng = rand::rng();
    // more adjectives as it goes: 2 at the start, 6 by the end
    let n = 2 + (round * 4 / total.max(1)) as usize;
    let words: Vec<&str> = QUALITIES.choose_multiple(&mut rng, n).copied().collect();
    let more = words.iter().map(|w| format!("more {w}")).collect::<Vec<_>>().join(", ");
    let shout = SHOUTS[((round - 1) as usize * SHOUTS.len() / total.max(1) as usize).min(SHOUTS.len() - 1)];
    let bangs = "!".repeat(1 + round as usize / 3);
    format!(
        "{shout} {more}{bangs} Improve this image! Make it pop! Add more of everything, \
         more effects, more color, more stuff, more detail. Push every slider to the max. \
         Keep the same subject, just make it SO much more {}{bangs}",
        words[0]
    )
}

/// One trip through the model: image in, "improved" image out.
async fn step(app: &App, img: &DynamicImage, prompt: &str, ratio: &str) -> Result<DynamicImage, String> {
    let input = json!({
        "prompt": prompt,
        "input_images": [shrink_to_jpeg(img, app.shrink)],
        "quality": "low",
        "complexity": "low",
        "aspect_ratio": ratio,
        "number_of_images": 1,
        "output_format": "jpeg",
        "moderation": "low",
    });
    let mut pred: Value = app
        .http
        .post(MODEL_URL)
        .bearer_auth(&app.token)
        .header("Prefer", "wait=60")
        .json(&json!({ "input": input }))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let started = Instant::now();
    loop {
        match pred["status"].as_str() {
            Some("succeeded") => break,
            Some("failed") | Some("canceled") => {
                return Err(pred["error"].as_str().unwrap_or("prediction failed").into());
            }
            None => return Err(format!("replicate said: {pred}")),
            _ => {}
        }
        if started.elapsed() > Duration::from_secs(300) {
            return Err("timed out".into());
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        let url = pred["urls"]["get"].as_str().ok_or("no poll url")?.to_string();
        pred = app.http.get(url).bearer_auth(&app.token).send().await
            .map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
    }

    // output is a URI or a list of them
    let out = &pred["output"];
    let url = out.as_str().or_else(|| out[0].as_str()).ok_or("no output image")?;
    let bytes = app.http.get(url).send().await.map_err(|e| e.to_string())?
        .bytes().await.map_err(|e| e.to_string())?;
    image::load_from_memory(&bytes).map_err(|e| e.to_string())
}

/// Shrink and re-JPEG at low quality: the compression between rounds is part
/// of the frying, and small inputs are cheaper.
fn shrink_to_jpeg(img: &DynamicImage, max_side: u32) -> String {
    let small = if img.width().max(img.height()) > max_side {
        img.resize(max_side, max_side, FilterType::Triangle)
    } else {
        img.clone()
    };
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, 40)
        .encode_image(&small.to_rgb8())
        .expect("jpeg encode");
    format!("data:image/jpeg;base64,{}", B64.encode(buf))
}

/// The model's named ratio nearest the image's shape.
fn aspect(img: &DynamicImage) -> &'static str {
    let r = img.width() as f32 / img.height().max(1) as f32;
    [("1:1", 1.0), ("3:2", 1.5), ("2:3", 0.667), ("4:3", 1.333), ("3:4", 0.75), ("16:9", 1.778), ("9:16", 0.5625)]
        .into_iter()
        .min_by(|a, b| (a.1 - r).abs().total_cmp(&(b.1 - r).abs()))
        .unwrap()
        .0
}

const PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>DeepLearnFry</title>
<style>
body{font-family:Impact,sans-serif;background:#111;color:#ff0;text-align:center;margin:0;padding:16px}
h1{font-size:3em;text-shadow:3px 3px #f0f,-3px -3px #0ff;margin:.2em}
img{max-width:100%;border:4px solid #f0f;margin-top:12px}
#status{font-family:sans-serif;color:#fff;min-height:3em;max-width:700px;margin:12px auto}
button,input{font-size:1.2em;margin:4px}
</style></head><body>
<h1>🔥 DEEPLEARNFRY 🔥</h1>
<form id="f">
<input type="file" name="image" accept="image/*" required>
<input type="password" name="code" placeholder="access code (if any)">
<button>FRY IT</button>
</form>
<div id="status"></div>
<img id="out" hidden>
<script>
const f=document.getElementById('f'),s=document.getElementById('status'),o=document.getElementById('out');
f.onsubmit=async e=>{
  e.preventDefault(); s.textContent='heating the oil…';
  const r=await fetch('/fry',{method:'POST',body:new FormData(f)});
  if(!r.ok){s.textContent=await r.text();return}
  const {id}=await r.json();
  const poll=async()=>{
    const j=await (await fetch('/job/'+id)).json();
    o.src=j.image; o.hidden=false;
    s.textContent=j.error?('💀 '+j.error):j.done?`DONE. ${j.round} rounds. it is cooked.`
      :`round ${j.round+1}/${j.total}: “${j.prompt}”`;
    if(!j.done) setTimeout(poll,2000);
  };
  poll();
};
</script></body></html>"#;

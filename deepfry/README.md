# DeepLearnFry

Upload an image; `openai/gpt-image-2.6-flare` on Replicate "improves" it 20
times in a row at `quality: low`, `complexity: low`, each round begging for
more awesome / sparkly / eye-catching / bussin' / 8K / lens-flared nonsense.
Between rounds the image is shrunk to 384px and re-saved as a quality-40 JPEG,
which is half the frying and keeps input tokens down.

## Run

    REPLICATE_API_TOKEN=r8_... cargo run --release
    # open http://localhost:8080

| env | default | |
|---|---|---|
| `REPLICATE_API_TOKEN` | required | |
| `PORT` | 8080 | Railway sets it |
| `ROUNDS` | 20 | |
| `SHRINK` | 384 | longest side between rounds |
| `ACCESS_CODE` | none | **set this on a public URL**, or anyone can spend your key |
| `MAX_JOBS` | 3 | concurrent fries |
| `OUTPUT_SIZE` | nearest named ratio | e.g. `512x512`, to try for cheaper output |

## Railway

New service from this repo, root directory `deepfry`; it builds the
Dockerfile. Add `REPLICATE_API_TOKEN` and `ACCESS_CODE` as variables.

A fry runs in the background and the page polls `/job/<id>` every 2s, so the
20-call run (minutes) never sits on one HTTP request. Jobs live in memory and
are forgotten after an hour or a restart.

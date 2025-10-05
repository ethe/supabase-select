# agent-uploader

`agent-uploader` tails a local Codex (or compatible) session history file, segments it into manageable chunks, uploads those segments to Supabase Storage, and mirrors a live manifest + checkpoints for replay. While the uploader runs it also serves a small dashboard so you can inspect sessions from your browser.

## Features

- **File tailing** – watches an NDJSON session log, buffering partial lines and handling truncate/rotation events.
- **Segment rotation** – closes a segment when any configured threshold (bytes, lines, wall clock) hits, then optionally gzips it before upload.
- **Manifest & checkpoints** – maintains an append-only `manifest.json` alongside optional checkpoint records triggered by `{"type":"compacted"}` events.
- **Supabase Storage uploads** – streams segments/manifest/checkpoints to a bucket using the REST API with retries and exponential backoff.
- **Offline spool** – queues uploads on disk until credentials or connectivity recover.
- **Embedded UI** – a bundled React app (served from the same process) lists sessions and replays NDJSON lines straight from Supabase.

## Prerequisites

- Rust toolchain (1.80+ recommended).
- Node.js 18+ and npm (for building the bundled UI).
- A Supabase project with a storage bucket (default name `sessions`).
- Supabase REST URL (`https://<project>.supabase.co`) and a key with write access to the bucket (service role or anon with RLS rules).

## Building

```bash
# build the Rust binary
cargo build --release

# build the frontend bundle (outputs to frontend/dist)
cd frontend
npm install
npm run build
cd ..
```

The watch command looks for UI assets in `frontend/dist` by default. Use `--ui-dist` to point elsewhere.

## Configuration

`agent-uploader watch` accepts CLI flags and env vars (env vars act as defaults):

| Flag / Env | Description | Default |
|------------|-------------|---------|
| `--file`, `AGENT_SESSION_FILE` | Path to session history NDJSON | _required_ |
| `--bucket`, `SUPABASE_BUCKET` | Supabase Storage bucket | `sessions` |
| `--sid`, `AGENT_SID` | Session id (`auto` derives from filename UUID) | `auto` |
| `--supabase-url`, `SUPABASE_URL` | Supabase project URL | _required unless `--upload-url`/`--dry-run`_ |
| `--supabase-key`, `SUPABASE_KEY` | REST API key | _required unless `--upload-url`/`--dry-run`_ |
| `--upload-url` | Base URL for presigned uploads instead of Supabase REST | – |
| `--seg-bytes` | Rotate when uncompressed bytes exceed value | `8 MiB` |
| `--seg-lines` | Rotate when lines reach value | `10_000` |
| `--seg-ms` | Rotate after wall-clock milliseconds | `600_000` |
| `--poll-ms` | File poll interval | `500` |
| `--no-gzip` | Disable gzip compression (upload `.jsonl`) | gzip on |
| `--spool-dir` | Override spool directory | `~/.agent-uploader/spool` |
| `--state-dir` | Manifest cache directory | `<spool>/state` |
| `--ui-bind`, `AGENT_UI_BIND` | UI listener bind address | `127.0.0.1` |
| `--ui-port`, `AGENT_UI_PORT` | UI listener port | `4333` |
| `--ui-dist`, `AGENT_UI_DIST` | Directory holding built UI assets | autodetect `frontend/dist` |
| `--ui-disable` | Skip starting the embedded UI | disabled = false |
| `--dry-run` | Skip all network uploads | false |
| `--concurrency` | Max concurrent uploads from spool | `2` |

Supabase requests use HTTPS with `x-upsert: true` so replays are idempotent.

## Running the uploader

```bash
export SUPABASE_URL=https://<project>.supabase.co
export SUPABASE_KEY=<service_or_anon_key>

./target/release/agent-uploader watch \
  --file "$HOME/.codex/sessions/2025/10/04/rollout-....jsonl" \
  --sid auto \
  --bucket sessions \
  --seg-bytes 8388608 \
  --seg-lines 10000 \
  --seg-ms 600000 \
  --poll-ms 500
```

While the process runs you can visit the embedded dashboard at `http://127.0.0.1:4333/` (or whatever `--ui-bind`/`--ui-port` you selected) to:

- List sessions discovered in `sessions/<sid>/manifest.json`.
- Inspect segment metadata and checkpoints.
- Replay NDJSON lines up to a checkpoint or the latest manifest boundary.

Use `Ctrl+C` to shut down. The uploader drains the spool queue on exit; if uploads still fail (401/403/429/5xx) the data stays on disk until the next run.

### Offline / Retry behavior

- Failed uploads are written to `<spool>/queue` alongside a `.meta.json` descriptor.
- On startup the queue is drained before new segments are uploaded.
- 429/5xx responses trigger exponential backoff up to 30 seconds.

### Checkpoints

Any NDJSON line with `"type": "compacted"` triggers:

1. Immediate segment rotation.
2. Writing `checkpoints/<id>.json` with the sequence, line index, and optional git metadata.
3. Appending the checkpoint object to `manifest.json`.

## Additional commands

The CLI exposes stubs for `reload`, `replay`, and `host`; these will return `not implemented yet` until the corresponding milestones are finished.

## Development tips

- Run `cargo fmt` and `cargo test` before sending patches.
- Delete stale files in `~/.agent-uploader/spool` if you change layout or credentials; otherwise queued items may keep failing.
- Use `--no-gzip` when you need raw `.jsonl` segments in Supabase (the UI automatically decompresses `.gz`).
- Pass `--dry-run` to validate segmentation logic without touching the network.

## License

MIT. See `LICENSE` once added, or treat the project as hackathon-grade for now.

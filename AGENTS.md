# AGENTS.md — Sidecar Uploader + Local Host (Rust)

> **Goal**
> Build a **Rust command-line tool** that (1) tails a coding-agent CLI session history file (NDJSON), (2) rotates/compacts and **uploads** segments to **Supabase Storage**, (3) on `compacted` events writes **checkpoint** objects for **time-travel**, and (4) **hosts** a Figma Make–exported web UI on `localhost` to browse sessions & checkpoints and replay logs.
> **Bias to shipping** (hackathon): implement the smallest robust subset first.

---

## 0) Scope & Non-Goals

**In scope**

* Continuous tail of a single session history file (NDJSON).
* Segment rotation by size/lines/time; gzip closed segments.
* Upload segments + `manifest.json` to Supabase Storage (REST).
* On `type == "compacted"` events: write checkpoint JSON + append to manifest.
* CLI subcommands: `watch` (tail+upload), `host` (serve web UI), `reload` (rebuild local file from Storage), `replay` (print to stdout).
* Web UI (static bundle from Figma Make) reads manifests/segments from Storage; lists sessions & checkpoints; can time-travel replay.

**Out of scope (today)**

* DB writes, Supabase Realtime (optional stub only).
* Multi-writer reconciliation / CRDT.
* Source code merges; PR/review flow.

---

## 1) Architecture (MVP)

```
+--------------------+       upload (REST)        +------------------------------+
| Rust CLI (watch)   |--------------------------->| Supabase Storage (bucket)    |
|  - tails NDJSON    |                             |  sessions/{sid}/...          |
|  - rotates & gzip  |                             |  - manifest.json             |
|  - writes checkpoints                (optional)  |  - segments/*.jsonl.gz       |
|  - updates manifest  --broadcast--> (Realtime)   |  - checkpoints/*.json        |
+--------------------+                             +------------------------------+
         |
         | serves static web
         v
+--------------------+    fetch /config.json + Storage reads
| Rust CLI (host)    |<-------------------------------+
|  - / (static UI)   |                                |
|  - /config.json    |                                |
+--------------------+                                |
         ^                                            v
         |                                     +-----------------+
         |   GET Storage                       | Web UI (Make)   |
         +-------------------------------------| Sessions/Replay |
                                               +-----------------+
```

---

## 2) Storage Layout

```
Bucket: sessions
└─ sessions/{sid}/
   ├─ manifest.json
   ├─ segments/
   │   ├─ session-000001.jsonl.gz
   │   ├─ session-000002.jsonl.gz
   │   └─ ...
   └─ checkpoints/
       ├─ 2025-10-04T19-44-22Z.json
       └─ latest.json   (optional symlink/alias object)
```

* `sid`: session id (uuid or `YYYYMMDD-HHmmss-rand`)
* `seq`: 6-digit zero-padded segment counter

---

## 3) File Formats

### 3.1 Session Line (NDJSON)

One JSON object per line. The tool treats unknown fields as opaque; only `type` is inspected.

```json
{"ts":1696439001,"type":"msg","role":"agent","text":"..."}
{"ts":1696439002,"type":"tool","name":"grep","stdout":"..."}
{"ts":1696439062,"type":"compacted","detail":{"from":0,"to":7300,"summary":"..."}}
```

**Compaction trigger**: any line where `type == "compacted"`.

### 3.2 `manifest.json`

```json
{
  "version": 1,
  "sid": "20251004-1940-ax9",
  "created_at": "2025-10-04T19:40:02Z",
  "updated_at": "2025-10-04T19:45:10Z",
  "active_seq": 3,
  "segments": [
    {
      "seq": 1,
      "path": "segments/session-000001.jsonl.gz",
      "first_ts": 1696438800,
      "last_ts": 1696439002,
      "lines": 9860,
      "bytes": 8123456,
      "gzip_bytes": 2345678
    },
    { "seq": 2, "path": "segments/session-000002.jsonl.gz", "first_ts": 1696439003, "last_ts": 1696439059, "lines": 912 }
  ],
  "checkpoints": [
    {
      "id": "2025-10-04T19-44-22Z",
      "label": "after compact",
      "seq": 2,
      "line_idx": 4213,
      "git": "9f3c1ab",
      "ts": 1696439062
    }
  ]
}
```

> **Note**: Prefer `line_idx` (line count within `seq`) over byte offset for replay truncation. Gzip is not random-seek friendly.

### 3.3 `checkpoints/<id>.json`

```json
{
  "id": "2025-10-04T19-44-22Z",
  "label": "after compact",
  "seq": 2,
  "line_idx": 4213,
  "git": "9f3c1ab",
  "ts": 1696439062,
  "comment": ""
}
```

---

## 4) CLI Interface

```text
agent
  watch       Tail+rotate+upload session history to Storage
  reload      Rebuild a local session.jsonl from Storage (up to checkpoint)
  replay      Print session to stdout (up to checkpoint)
  host        Serve the Figma Make web bundle on localhost
  version
```

### 4.1 `watch` (primary)

```
agent watch \
  --file /path/to/session.jsonl \
  --sid auto \
  --bucket sessions \
  --seg-bytes 8388608 \
  --seg-lines 10000 \
  --seg-ms 600000 \
  --poll-ms 500 \
  --ui-port 4333 \
  --ui-bind 127.0.0.1 \
  --verbose
```

Env:

```
SUPABASE_URL=https://<project>.supabase.co
SUPABASE_KEY=<anon_or_service_key>
```

Behavior:

* Poll file every `--poll-ms`, append complete lines.
* Rotate segment on any threshold (bytes/lines/wallclock).
* On rotate: close → gzip → upload to `segments/` → update & upload `manifest.json`.
* On reading a line with `type=="compacted"`:

  * Flush current segment up to a stable boundary.
  * Create checkpoint `{seq, line_idx, ...}` under `checkpoints/`.
  * Append to `manifest.checkpoints[]` and upload manifest.

**Optional**: after upload, broadcast a tiny Realtime message `{type:"checkpoint", id, seq, line_idx}` (skip if time is tight).

#### Embedded UI

`watch` also launches a read-only dashboard at `http://<ui-bind>:<ui-port>/` (default `http://127.0.0.1:4333/`).

* `--ui-port` / `--ui-bind` customize the listener.
* `--ui-dist` can point to a custom build directory; defaults to `./frontend/dist` when present.
* Pass `--ui-disable` to skip starting the server (useful for headless runs).
* The UI calls the REST bridge exposed under `/api/**` to list sessions, fetch manifests, and stream NDJSON lines from Supabase Storage.

### 4.2 `reload`

```
agent reload \
  --sid  20251004-1940-ax9 \
  --bucket sessions \
  --checkpoint latest \
  --to ./restore/session.jsonl \
  --force
```

Steps:

1. GET `manifest.json`.
2. Resolve checkpoint (`latest` or exact id).
3. Download & gunzip `segments/1..(seq-1)` fully, then `seq` up to `line_idx`.
4. Write to `--to`.

### 4.3 `replay`

```
agent replay \
  --sid  20251004-1940-ax9 \
  --checkpoint latest
```

Same as `reload` but stream to stdout.

### 4.4 `host`

```
agent host \
  --web-dir ./web-dist \
  --port 4333 \
  --open \
  --supabase-url https://xxxx.supabase.co \
  --supabase-anon-key eyJhbGciOi... \
  --bucket sessions
```

* Serves static files from `web-dist/` and a dynamic `/config.json`:

  ```json
  { "supabaseUrl":"","supabaseAnonKey":"","bucket":"sessions" }
  ```
* Frontend fetches `/config.json` at startup to init `supabase-js`.

---

## 5) HTTP Upload Contract (Storage REST)

```
POST {SUPABASE_URL}/storage/v1/object/{bucket}/{objectPath}
Headers:
  Authorization: Bearer {SUPABASE_KEY}
  x-upsert: true
  Content-Type: application/octet-stream
  Content-Encoding: gzip     # for *.gz
Body: <bytes>
```

* Idempotent uploads (same path) must succeed (`x-upsert: true`).
* Retry policy: exponential backoff w/ jitter on 429/5xx; surface 401/403.

**Presigned URL (optional)**
If provided via `--upload-url` for an object, perform a plain `PUT` to that URL instead of the REST endpoint.

---

## 6) Rotation & Compaction Policy (defaults)

* `MAX_SEG_BYTES = 8 MiB`
* `MAX_SEG_LINES = 10_000`
* `MAX_SEG_WALL_MS = 10 min`
* `POLL_MS = 500`
* On rotate: gzip previous, upload, update manifest.
* Noise control (cheap):

  * Truncate extremely large fields (keep first/last 200 lines; mark `"omitted": true`).
  * Collapse progress spam (keep latest only) — optional.

---

## 7) Project Layout

```
/agent
  /src
    main.rs
    config.rs
    watch.rs      # tail/rotate/gzip/upload/checkpoint/manifest
    reload.rs     # rebuild local/replay
    host.rs       # static server + /config.json
    storage.rs    # REST client + retries
    manifest.rs   # types + merge/update
    util.rs
  /web-dist       # Figma Make export (static)
  Cargo.toml
```

**Crates**

```
tokio, clap, anyhow, serde, serde_json,
reqwest (rustls-tls), async-compression (gzip, tokio),
tokio-util (io), tracing (+ tracing-subscriber),
axum, tower-http (fs,cors), open
```

---

## 8) Algorithms (sketch)

**Tail (watch)**

1. `offset = 0`, `carry = ""`.
2. Every `poll_ms`: stat file; if size < offset ⇒ truncated/rotated ⇒ start new segment.
3. Read `size - offset` bytes → split by `\n` → prepend `carry` to first line → keep last partial to `carry`.
4. For each *complete line*: write to active segment; increment `line_count`.
5. If thresholds exceeded ⇒ **rotate**:

   * Close active, gzip, upload, update manifest, `seq += 1`, reset counters.
6. If parsed JSON has `type=="compacted"` ⇒ **checkpoint**:

   * Compute `{seq, line_idx}`; write `checkpoints/<id>.json`; push to `manifest.checkpoints`; upload both.

**Reload/Replay**

* Load manifest → checkpoint → stream download & gunzip needed segments → write to file or stdout, truncating last segment at `line_idx`.

**Host**

* `ServeDir` for static; `/config.json` returns runtime config; optional auto-open browser.

---

## 9) Web UI (Figma Make + tiny glue)

At startup:

```ts
// main.ts (glue)
type AppCfg = { supabaseUrl:string; supabaseAnonKey:string; bucket:string };
const cfg: AppCfg = await fetch('/config.json', {cache:'no-store'}).then(r=>r.json());
const supa = createClient(cfg.supabaseUrl, cfg.supabaseAnonKey);

// list available sessions (sids under sessions/)
const list = await supa.storage.from(cfg.bucket).list('sessions', { limit: 1000 });
const sid = list[0]?.name; // pick one in UI

// load manifest
const { data: manifestData } = await supa.storage.from(cfg.bucket)
  .download(`sessions/${sid}/manifest.json`);
const manifest = JSON.parse(await manifestData.text());

// show checkpoints (timeline) & segments (table)
// time-travel: download segments 1..(seq-1) + seq (truncate by line_idx)
// use DecompressionStream('gzip') if available:
const resp = await supa.storage.from(cfg.bucket)
  .download(`sessions/${sid}/segments/session-000001.jsonl.gz`);
const stream = resp.body!.pipeThrough(new DecompressionStream('gzip'));
// then read lines & render
```

*(If time runs out: just list sessions & checkpoints and show raw segment download links.)*

---

## 10) RLS / Security (hackathon mode)

* **Writes**: the CLI uses `SUPABASE_KEY` to write under `sessions/{sid}/**`.
* **Reads**: simplest is **public read** for the bucket; if restricted, the UI reads via `supabase-js` with anon key.
* Never ship service key in the front-end; keep it in CLI env only.

---

## 11) Definition of Done (DoD)

* `watch`: tails file, rotates at thresholds, uploads gz segments; on `compacted` writes checkpoint + updates manifest; serves embedded UI at `http://<ui-bind>:<ui-port>/` unless `--ui-disable` is set.
* `host`: serves web-dist and `/config.json`; browser shows Sessions → Checkpoints; can open a checkpoint and at least **list** segments needed (bonus: replay).
* `reload`: reconstructs local `session.jsonl` up to a checkpoint.
* Docs: `README` with env/commands; `.env.example`.
* Smoke test on macOS/Linux; Windows path/CRLF tolerant.

---

## 12) Milestones & Timeboxes

1. **M1 (60–75m)** — Tail + rotate + gzip + upload + manifest v1
2. **M2 (20–30m)** — Detect `compacted`, write checkpoint object + manifest entry
3. **M3 (35–45m)** — `host` subcommand (static + `/config.json`, auto-open)
4. **M4 (35–45m)** — Web glue: list sessions, load manifest, show checkpoint list, basic replay (or file links)
5. **M5 (25–35m)** — `reload` / `replay` subcommands (streamed gunzip, truncate last segment)
6. **Polish (20m)** — Retries, logs, README, demo script

*(If behind: skip replay UI fidelity—just list & download; keep `reload` working.)*

---

## 13) Quickstart

```bash
# Build
cargo build --release

# Build UI bundle (once per checkout)
cd frontend
npm install
npm run build
cd ..

# 1) Upload sidecar (watch)
export SUPABASE_URL=https://<proj>.supabase.co
export SUPABASE_KEY=<service_or_anon_key>
./target/release/agent watch --file ./session.jsonl --sid auto --bucket sessions
# UI is available while watch runs: http://127.0.0.1:4333/

# 2) Host local UI (drop Figma Make export into ./web-dist)
./target/release/agent host --web-dir ./web-dist --open \
  --supabase-url $SUPABASE_URL --supabase-anon-key $SUPABASE_ANON_KEY \
  --bucket sessions

# 3) Reload a session locally
./target/release/agent reload --sid 20251004-1940-ax9 --checkpoint latest --to ./restore/session.jsonl
```

---

## 14) Nice-to-Have (after MVP)

* Optional Realtime “checkpoint created” ping to live-refresh UI.
* Offline spool queue with background retry.
* Secret redaction (regex) before write.
* Checksums in manifest; integrity verify on reload.
* Keyboard-driven local terminal replay (speed control).

---

**Keep it boring, keep it shippable.** This file is the contract for the agent(s): if a detail is missing, pick the simplest behavior that keeps the upload paths and file formats above intact.

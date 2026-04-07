# Docker Deployment & Operations Guide

## Quick Start

Pre-built images are published to GHCR on every commit to `primary`:

```bash
docker run -d \
  -v /path/to/music:/music \
  -v mm_config:/config \
  -v mm_data:/data \
  -p 3313:3313 \
  ghcr.io/pandorasfox/music-magic:latest
```

Or build from source:

```bash
docker build -t mm .
docker run -d \
  -v /path/to/music:/music \
  -v mm_config:/config \
  -v mm_data:/data \
  -p 3313:3313 \
  mm
```

The container runs three components:

| Binary | Role |
|--------|------|
| `mm` | The Witch — core server, orchestrates all background work |
| `mm-web` | HTTP API + browser UI, connects to the Witch via Unix socket |
| `mm-tui` | Terminal UI client (available via `docker exec`) |

On first start, connect to the web UI at `http://host:3313` to run first-time setup.

## What's Inside the Container

The entrypoint (`docker/entrypoint.sh`) does the following on boot:

1. Creates XDG directories under `/config/mm` and `/data/mm`
2. Seeds a minimal `config.kdl` from `MM_ROOT` if none exists
3. Cleans any stale socket from a previous run
4. Starts `mm` (Witch) in the background — it creates a Unix socket at `/tmp/mm.sock`
5. Waits up to 15 seconds for the socket to appear
6. Starts `mm-web` in the foreground, connecting to the socket
7. Forwards SIGTERM/SIGINT to both processes — if either exits, the other is torn down

## Volumes

| Container Path | Purpose | Recommended Mount |
|----------------|---------|-------------------|
| `/music` | Music corpus root — your actual audio files | Bind mount (`-v /srv/music:/music`) |
| `/config` | `XDG_CONFIG_HOME` — `mm/config.kdl` and `mm/dirs.kdl` | Named volume or bind mount |
| `/data` | `XDG_DATA_HOME` — `mm/mm.db` (SQLite database) | Named volume or bind mount |

The config and data volumes are lightweight. The music volume can be read-only if you don't use deploy/transcode features that write back to the corpus.

### Bind-Mount vs Named Volume

Named volumes (`mm_config`, `mm_data`) are easiest for getting started. For production or debugging, bind mounts give you direct host access to the config and database files:

```bash
docker run -d \
  -v /srv/music:/music \
  -v /srv/mm/config:/config \
  -v /srv/mm/data:/data \
  -p 3313:3313 \
  mm
```

This way you can edit `config.kdl` directly on the host, or open the database with `sqlite3` without entering the container.

## Configuration

### Minimal Config

MM seeds a minimal `config.kdl` on first boot:

```kdl
root "/music"
```

That's enough to start. The web UI handles first-time setup (user creation, initial scan).

### Typical Config

A realistic config for a containerized setup with a local MusicBrainz mirror:

```kdl
root "/music"

opinions {
    leave-transactions-open true

    external-matching {
        acoustid-api-key "your-acoustid-key-here"
        mb-base-url "http://musicbrainz-web:5000/ws/2"
        mb-requests-per-second 50
        mb-tag-names {
            picard-compat true
        }
    }

    tag-splitting {
        artist ";" ","
        genre ";"
    }

    startup {
        default-view "external-matches"
    }
}
```

Most fields have sensible defaults and don't need to be set explicitly. The interesting ones:

- **`leave-transactions-open true`** — keeps the transaction panel open after commit so you can continue staging decisions without re-opening
- **`mb-base-url`** — point at a local MusicBrainz mirror to remove the public API's 1 req/s rate limit
- **`mb-requests-per-second`** — raise this when using a local mirror (default is `1` for public API courtesy)
- **`picard-compat true`** — writes MusicBrainz tag names in both MM and MusicBrainz Picard conventions
- **`tag-splitting`** — defines separators for splitting compound tag values (e.g., `"Artist A; Artist B"` into two tags)
- **`default-view`** — which view to land on at startup

Performance defaults are already reasonable: worker threads default to 2x logical cores, db-cache defaults to 256 MB per connection. Override only if you have a specific reason.

### Source Directories (dirs.kdl)

Source directories define the structure of your corpus — which subdirectories map to which libraries, and how files within them should be handled:

```kdl
dir "web/releases/bandcamp" {
    library "music"
    enable-acoustid true
}

dir "web/releases/steam" {
    library "soundtracks"
}

dir "physical/vinyls" {
    library "music"
    can-stash-dupes true
}
```

These are relative paths under your corpus root. Each `dir` maps to a library deployment target (what Navidrome sees). Boolean fields like `can-stash-dupes` and `enable-acoustid` inherit from parent directories.

### Environment Variable Overrides

Every config field can be set via environment variables, which is convenient for Docker:

**Curated aliases** (most common):

| Variable | Maps to |
|----------|---------|
| `MM_ROOT` / `MM_STORAGE_ROOT` | `storage-root` (corpus root) |
| `MM_LIBRARIES_ROOT` | `libraries-root` (deploy target root) |
| `MM_STASH_ROOT` | `stash-root` (duplicate stash root) |
| `MM_ACOUSTID_API_KEY` | `opinions.external-matching.acoustid-api-key` |
| `MM_MB_BASE_URL` | `opinions.external-matching.mb-base-url` |
| `MM_MB_REQUESTS_PER_SECOND` | `opinions.external-matching.mb-requests-per-second` |
| `MM_WORKER_THREADS` | `opinions.performance.worker-threads` |
| `MM_DB_CACHE` | `opinions.performance.db-cache` (accepts `256mb`, `1gb`, etc.) |
| `MM_WATCHER_POLL_INTERVAL` | `opinions.watcher-poll-interval-secs` |
| `MM_WEB_LISTEN` | mm-web bind address (not a config.kdl field) |
| `MM_WEB_STATIC_DIR` | mm-web static file path (not a config.kdl field) |

Env vars override config.kdl values, so you can keep a minimal config file and set the rest at container runtime. A typical docker-compose environment section might look like:

```yaml
environment:
  - MM_STORAGE_ROOT=/library/archive/audio
  - MM_LIBRARIES_ROOT=/library
  - MM_STASH_ROOT=/library/archive/stash
  - MM_MB_BASE_URL=http://mb-web:5000/ws/2
  - MM_MB_REQUESTS_PER_SECOND=50
  - MM_ACOUSTID_API_KEY=${ACOUSTID_API_KEY}
```

Note that `MM_STORAGE_ROOT`, `MM_LIBRARIES_ROOT`, and `MM_STASH_ROOT` are container-internal paths — they refer to wherever you've mounted your volumes. All three directories must be on the same filesystem (hardlinking requirement for deploys).

**Generic override pattern** for any opinion field:

```
MM_CFG__BLOCK__FIELD=value
```

Block and field names use `SCREAMING_SNAKE_CASE`, mapped to `kebab-case` in config.kdl. Examples:

```bash
MM_CFG__DUPLICATE_ANALYSIS__FINGERPRINT_SIMILARITY_THRESHOLD=85.0
MM_CFG__STARTUP__VACUUM_THRESHOLD=0.05
MM_CFG__ALBUM_ART__SIDECAR_DEPLOY_MODE=disabled
MM_CFG__RELEASE_PACKING__MIN_CONFIDENCE=0.4
```

See `src/config/env_override.rs` for the full mapping.

## Using the TUI Inside the Container

The container includes `mm-tui`, which connects to the Witch's Unix socket and gives you the full terminal UI:

```bash
docker exec -it mm mm-tui
```

The `-it` flags are required — they allocate a TTY for the interactive terminal interface. The TUI auto-discovers the socket via `XDG_RUNTIME_DIR` (set to `/tmp` inside the container).

This is useful for quick corpus inspection, tag editing, and health review without opening a browser.

## Web API

The web UI is served at the root (`/`). The HTTP API is authenticated via bearer tokens — log in first to get a session token.

### Authentication

```bash
# Log in (returns a base64 session token)
curl -s http://localhost:3313/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"your-password"}' | jq -r .token

# Use the token for subsequent requests
TOKEN="your-base64-token"
curl -s http://localhost:3313/status -H "Authorization: Bearer $TOKEN" | jq
```

### Domain Queries

The main data access endpoint is `GET /queries/{name}` (simple) or `POST /queries/{name}` (with JSON body). All require authentication.

**Summary queries** (good starting points):

| Route | Returns |
|-------|---------|
| `insights` | Corpus health overview — file counts, signal counts, unindexed files |
| `deploy-status` | Library deployment state |
| `edit-history` | Recent tag edit sessions |
| `external-matches` | AcoustID/MusicBrainz match overview |
| `packing-dirs` | Release packing state per directory |

**Browsing and search:**

| Route | Returns |
|-------|---------|
| `directory-listing` | File/directory listing (POST with `{"path": "..."}`) |
| `search-corpus` | Full-text corpus search (POST with `{"query": "..."}`) |
| `search-with-conditions` | Structured search with tag conditions |
| `corpus-tags` | All distinct tag name/value pairs |

**Detail loaders** (for specific signals/modals):

| Route | Returns |
|-------|---------|
| `oob-files` | Out-of-band tag change details |
| `moved-files` | Relocated file details |
| `corrupt-file-data` | Corrupt file info |
| `subpar-duplicate-data` | Duplicate analysis details |
| `disc-extraction-data` | Multi-disc extraction groups |
| `packing-knots` | Complex release packing constraints |
| `tag-canonicity-resolution` | Tag normalization candidates |
| `compound-split-resolution` | Multi-value tag splitting data |
| `release-review` | MusicBrainz release review data |
| `inode-details` | Detailed file info (POST with `{"inodes": [...]}`) |

**Examples:**

```bash
# Corpus health summary
curl -s http://localhost:3313/queries/insights \
  -H "Authorization: Bearer $TOKEN" | jq

# Browse a directory
curl -s http://localhost:3313/queries/directory-listing \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"path": "web/releases/bandcamp"}' | jq

# Search for an artist
curl -s http://localhost:3313/queries/search-corpus \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"query": "Deafheaven"}' | jq

# File details by inode
curl -s http://localhost:3313/queries/inode-details \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"inodes": [12345, 67890]}' | jq
```

### WebSocket Events

Live server-pushed events are available via WebSocket:

```
ws://localhost:3313/ws?token=YOUR_BASE64_TOKEN
```

Events include Witch status changes, computation progress, and signal updates. The web UI uses this for real-time updates.

## Database Inspection

MM uses a single SQLite database at `/data/mm/mm.db`. The container includes `sqlite3` for direct inspection.

### Opening the Database

**From inside the container** (while MM is running):

```bash
# Read-only — safe while the Witch is active
docker exec -it mm sqlite3 -readonly file:///data/mm/mm.db
```

**From the host** (if using bind mounts):

```bash
sqlite3 -readonly file:///srv/mm/data/mm/mm.db
```

The `-readonly` flag is important — it prevents accidental writes and avoids lock contention with the running Witch. You can also open normally for one-off inspection when MM is stopped.

### WAL Mode

MM runs SQLite in WAL (Write-Ahead Logging) mode, which means you'll see three files:

```
mm.db        # Main database
mm.db-wal    # Write-ahead log (uncommitted pages)
mm.db-shm    # Shared memory index
```

When opening read-only from outside the container, SQLite reads the WAL automatically — you always see the latest committed state. If the `-wal` file is large, it means the Witch has been writing actively; it checkpoints automatically.

### Core Tables

These are the primary data tables — the ones you'll query most often:

```sql
-- files: Every file in the corpus and library zones
-- Key columns: inode, zone ('corpus'/'library'), path, file_size, mtime_secs
SELECT zone, COUNT(*) FROM files GROUP BY zone;

-- audio_info: Audio metadata keyed by inode
-- Key columns: inode, file_type, duration_ms, bitrate_kbps, sample_rate, has_pictures
SELECT file_type, COUNT(*) FROM audio_info GROUP BY file_type;

-- corpus_tags: Vorbis Comment tags (one row per inode/name/value triple)
-- Key columns: inode, tag_name, tag_value
SELECT tag_name, COUNT(DISTINCT tag_value) FROM corpus_tags GROUP BY tag_name ORDER BY 2 DESC;

-- tag_edit_history: Audit log of tag changes
-- Key columns: inode, field_name, old_value, new_value, edited_at, session_id
SELECT * FROM tag_edit_history ORDER BY edited_at DESC LIMIT 20;
```

### Useful Queries

**Corpus overview:**

```sql
-- Total files by zone
SELECT zone, COUNT(*) as files, SUM(file_size) / 1073741824.0 as gb
FROM files WHERE is_dir = 0
GROUP BY zone;

-- Audio format breakdown
SELECT a.file_type, COUNT(*) as tracks,
       ROUND(AVG(a.bitrate_kbps)) as avg_kbps,
       ROUND(SUM(a.duration_ms) / 3600000.0, 1) as hours
FROM audio_info a
JOIN files f ON a.inode = f.inode
WHERE f.zone = 'corpus'
GROUP BY a.file_type;

-- Tracks with embedded art
SELECT has_pictures, COUNT(*) FROM audio_info GROUP BY has_pictures;
```

**Tag inspection:**

```sql
-- All tags for a specific file (by path)
SELECT ct.tag_name, ct.tag_value
FROM corpus_tags ct
JOIN files f ON ct.inode = f.inode
WHERE f.path LIKE '%Artist/Album%'
ORDER BY ct.tag_name;

-- Albums with track counts
SELECT
  MAX(CASE WHEN ct.tag_name = 'ALBUM' THEN ct.tag_value END) as album,
  MAX(CASE WHEN ct.tag_name = 'ARTIST' THEN ct.tag_value END) as artist,
  COUNT(DISTINCT ct.inode) as tracks
FROM corpus_tags ct
WHERE ct.tag_name IN ('ALBUM', 'ARTIST')
GROUP BY ct.inode
ORDER BY album;

-- Find all values of a specific tag
SELECT tag_value, COUNT(*) as tracks
FROM corpus_tags
WHERE tag_name = 'GENRE'
GROUP BY tag_value
ORDER BY 2 DESC;

-- Files missing a required tag
SELECT f.path
FROM files f
JOIN audio_info a ON f.inode = a.inode
WHERE f.zone = 'corpus'
  AND f.inode NOT IN (
    SELECT inode FROM corpus_tags WHERE tag_name = 'ALBUM_ARTIST'
  )
LIMIT 20;
```

**Duplicate analysis:**

```sql
-- Fingerprint groups (files that sound the same)
SELECT a.fingerprint IS NOT NULL as has_fp, COUNT(*)
FROM audio_info a
JOIN files f ON a.inode = f.inode
WHERE f.zone = 'corpus'
GROUP BY 1;

-- Exact duration duplicates (quick heuristic)
SELECT a.duration_ms, GROUP_CONCAT(f.path, char(10)) as paths
FROM audio_info a
JOIN files f ON a.inode = f.inode
WHERE f.zone = 'corpus' AND a.duration_ms IS NOT NULL
GROUP BY a.duration_ms
HAVING COUNT(*) > 1
LIMIT 10;
```

**External matching (MusicBrainz/AcoustID):**

```sql
-- Match coverage
SELECT
  (SELECT COUNT(DISTINCT inode) FROM external_matches) as matched,
  (SELECT COUNT(*) FROM external_no_match) as no_match,
  (SELECT COUNT(*) FROM external_retry) as retrying;

-- MusicBrainz cache stats
SELECT 'recordings' as type, COUNT(*) FROM mb_recording_cache
UNION ALL
SELECT 'releases', COUNT(*) FROM mb_release_cache
UNION ALL
SELECT 'artists', COUNT(*) FROM mb_artist_cache;

-- Recent AcoustID matches with confidence
SELECT em.confidence, f.path, em.recording_id
FROM external_matches em
JOIN files f ON em.inode = f.inode
ORDER BY em.fetched_at DESC
LIMIT 20;
```

**Signal tables:**

Signal tables are prefixed with `signal_` and contain computed health/analysis data. Each has at least `inode` or a keyed identifier plus a `data` BLOB column (bincode-serialized Rust structs). The human-readable columns vary per signal.

```sql
-- List all signal tables and their row counts
SELECT name, (SELECT COUNT(*) FROM pragma_table_info(name)) as cols
FROM sqlite_master
WHERE type = 'table' AND name LIKE 'signal_%'
ORDER BY name;

-- Quick signal counts (the health dashboard numbers)
SELECT 'unindexed' as signal, COUNT(*) FROM signal_unindexed_file
UNION ALL SELECT 'healthy', COUNT(*) FROM signal_healthy_file
UNION ALL SELECT 'corrupt', COUNT(*) FROM signal_corrupt_file
UNION ALL SELECT 'missing_tag', COUNT(*) FROM signal_missing_tag
UNION ALL SELECT 'fingerprint_overlap', COUNT(*) FROM signal_fingerprint_overlap
UNION ALL SELECT 'deploy_ready', COUNT(*) FROM signal_deploy_ready;

-- Dirty inodes awaiting recomputation
SELECT computation_type, COUNT(*) FROM dirty_inodes GROUP BY computation_type;
```

### Schema Reference

To see the full schema of any table:

```sql
.schema files
.schema audio_info
.schema corpus_tags
```

Or list all tables:

```sql
.tables
```

The authoritative schema source is `src/db/table_schema.rs` — the `schema_inventory()` function defines every table, its classification (`Core`, `Decision`, `Computed`), and its indexes.

### Database Size

A typical corpus of ~40,000 tracks produces a database of ~200-400 MB, depending on how many AcoustID fingerprints are stored (fingerprint BLOBs are ~7 KB each). The `db-cache` config option controls SQLite's in-memory page cache — `1gb` is reasonable for large corpora.

## Rebuilding from Source

If you're developing locally and the compose file points at the repo:

```yaml
services:
  mm:
    build:
      context: /path/to/magic
    # ...
```

Then rebuild with:

```bash
docker compose build mm && docker compose up -d mm
```

Or one-shot:

```bash
docker compose up -d --build mm
```

The Dockerfile uses a two-layer dependency caching strategy — only Cargo.toml/Cargo.lock changes invalidate the dep cache, so rebuilds after source-only changes are fast.

## Full Stack Example

A complete self-hosted music stack with MM, MusicBrainz mirror, Navidrome, and Audiomuse:

```yaml
services:
  # ── Music Magic ────────────────────────────────────────────────────────
  mm:
    image: ghcr.io/pandorasfox/music-magic:latest
    # Or build from source:
    # build:
    #   context: /path/to/magic
    container_name: mm
    restart: unless-stopped
    user: "1000:1000"
    volumes:
      - /path/to/library:/library
      - mm_config:/config
      - mm_data:/data
    environment:
      - MM_STORAGE_ROOT=/library/archive/audio
      - MM_LIBRARIES_ROOT=/library
      - MM_STASH_ROOT=/library/archive/stash
      - MM_MB_BASE_URL=http://musicbrainz-web:5000/ws/2
      - MM_MB_REQUESTS_PER_SECOND=50
      - MM_ACOUSTID_API_KEY=${ACOUSTID_API_KEY}

  # ── MusicBrainz Mirror ────────────────────────────────────────────────
  # Full local replica with search. ~8 GB RAM, ~60 GB disk.
  # See https://github.com/metabrainz/musicbrainz-docker
  #
  # You need a MetaBrainz access token (free, get one at
  # https://metabrainz.org/supporters/account-type) saved to
  # ./musicbrainz/secrets/metabrainz_access_token
  #
  # First-time setup: run `docker compose exec musicbrainz-web bash` then
  # `fetch-dump.sh` and `createdb.sh` to import the initial DB dump.
  # After that, replication keeps data fresh hourly.
  musicbrainz-db:
    image: metabrainz/musicbrainz-docker-db:16-build0
    container_name: musicbrainz-db
    restart: unless-stopped
    command: >-
      postgres
      -c "shared_buffers=2048MB"
      -c "effective_cache_size=4096MB"
      -c "work_mem=64MB"
      -c "maintenance_work_mem=512MB"
      -c "random_page_cost=1.1"
      -c "shared_preload_libraries=pg_amqp.so"
    environment:
      POSTGRES_USER: musicbrainz
      POSTGRES_PASSWORD: musicbrainz
    shm_size: "2GB"
    volumes:
      - mb_pgdata:/var/lib/postgresql/data

  musicbrainz-web:
    image: metabrainz/musicbrainz-docker-musicbrainz:v-2026-02-12.0-build1
    container_name: musicbrainz-web
    restart: unless-stopped
    volumes:
      - mb_dbdump:/media/dbdump
      - ./musicbrainz/crons.conf:/crons.conf:ro
    environment:
      POSTGRES_USER: musicbrainz
      POSTGRES_PASSWORD: musicbrainz
      MUSICBRAINZ_POSTGRES_SERVER: musicbrainz-db
      MUSICBRAINZ_POSTGRES_READONLY_SERVER: musicbrainz-db
      MUSICBRAINZ_RABBITMQ_SERVER: musicbrainz-mq
      MUSICBRAINZ_REDIS_SERVER: musicbrainz-redis
      MUSICBRAINZ_SEARCH_SERVER: musicbrainz-search:8983/solr
      MUSICBRAINZ_SERVER_PROCESSES: 4
      MUSICBRAINZ_WEB_SERVER_HOST: localhost
      MUSICBRAINZ_WEB_SERVER_PORT: 5000
    secrets:
      - metabrainz_access_token
    depends_on:
      - musicbrainz-db
      - musicbrainz-mq
      - musicbrainz-search
      - musicbrainz-redis

  musicbrainz-search:
    image: metabrainz/mb-solr:4.1.0
    container_name: musicbrainz-search
    restart: unless-stopped
    environment:
      SOLR_HEAP: 512m
      LOG4J_FORMAT_MSG_NO_LOOKUPS: "true"
    volumes:
      - mb_solrdata:/var/solr

  # Search Index Rebuilder — keeps Solr in sync with the DB via RabbitMQ.
  # Build from a simple Dockerfile (see below) because the official
  # metabrainz/sir image bundles consul-template which doesn't work
  # outside their infra.
  musicbrainz-sir:
    build: ./musicbrainz/sir
    container_name: musicbrainz-sir
    restart: unless-stopped
    volumes:
      - ./musicbrainz/indexer.ini:/code/config.ini:ro
      - ./musicbrainz/sir-crons.conf:/crons.conf:ro
    depends_on:
      - musicbrainz-db
      - musicbrainz-mq
      - musicbrainz-search

  musicbrainz-mq:
    image: rabbitmq:3.6.16-management
    container_name: musicbrainz-mq
    restart: unless-stopped
    ulimits:
      nofile: 65536
    volumes:
      - mb_mqdata:/var/lib/rabbitmq

  musicbrainz-redis:
    image: redis:7-alpine
    container_name: musicbrainz-redis
    restart: unless-stopped

  # ── Navidrome (streaming) ─────────────────────────────────────────────
  navidrome:
    image: deluan/navidrome:latest
    container_name: navidrome
    restart: unless-stopped
    user: "1000:1000"
    volumes:
      - navidrome_data:/data
      - /path/to/library:/music:ro
    environment:
      - ND_SCANSCHEDULE=1h

  # ── Audiomuse (audio analysis) ────────────────────────────────────────
  audiomuse-db:
    image: postgres:16-alpine
    container_name: audiomuse-db
    restart: unless-stopped
    volumes:
      - audiomuse_pgdata:/var/lib/postgresql/data
    environment:
      - POSTGRES_USER=audiomuse
      - POSTGRES_PASSWORD=audiomuse
      - POSTGRES_DB=audiomuse

  audiomuse-redis:
    image: redis:7-alpine
    container_name: audiomuse-redis
    restart: unless-stopped

  audiomuse:
    image: ghcr.io/audiomuse/audiomuse:latest
    container_name: audiomuse
    restart: unless-stopped
    depends_on:
      - audiomuse-db
      - audiomuse-redis
    volumes:
      - /path/to/library:/music:ro

secrets:
  metabrainz_access_token:
    file: ./musicbrainz/secrets/metabrainz_access_token

volumes:
  mm_config:
  mm_data:
  mb_pgdata:
  mb_dbdump:
  mb_solrdata:
  mb_mqdata:
  navidrome_data:
  audiomuse_pgdata:
```

Adjust volume paths and image tags for your environment. The key points:

- **Volume layout**: The example mounts a single `/library` tree that contains corpus, libraries, and stash subdirectories. All three `MM_*_ROOT` paths must be on the same filesystem for hardlink-based deploys to work.
- **MusicBrainz mirror**: `MM_MB_BASE_URL` pointed at a local mirror eliminates the public API's 1 req/s rate limit. With a local mirror you can safely set `MM_MB_REQUESTS_PER_SECOND=50` or higher. The full mirror stack (6 containers) uses ~8 GB RAM at steady state and ~60 GB disk. Postgres is the main consumer (~5 GB with 2 GB shared_buffers). The initial DB import and search index build are one-time costs; after that, hourly replication and the SIR indexer keep everything current.
- **MusicBrainz SIR build**: The SIR Dockerfile is minimal — clone and pip install. See the [SIR setup section](#musicbrainz-search-index-rebuilder-sir) below.
- **Navidrome integration**: Navidrome reads from the library root (deploy target), not the corpus. MM deploys files into the library via hardlinks, and Navidrome picks them up on its scan interval.

### MusicBrainz Search Index Rebuilder (SIR)

The `musicbrainz-sir` service requires a small custom build because the official `metabrainz/sir` Docker image bundles consul-template for MetaBrainz's production infrastructure.

**`musicbrainz/sir/Dockerfile`:**

```dockerfile
ARG PYTHON_VERSION=3.13
ARG BASE_IMAGE_DATE=20250313
FROM metabrainz/python:${PYTHON_VERSION}-${BASE_IMAGE_DATE}

ARG DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install --no-install-recommends -qy \
      ca-certificates cron gcc git libc6-dev \
      libffi-dev libssl-dev libpq-dev libxslt1-dev libz-dev \
    && rm -rf /var/lib/apt/lists/*

ARG SIR_VERSION=4.0.1
RUN git clone --depth=1 --branch "v${SIR_VERSION}" https://github.com/metabrainz/sir.git /code \
    && cd /code && pip install -r requirements.txt && rm -f /code/config.ini

WORKDIR /code
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
```

**`musicbrainz/sir/entrypoint.sh`:**

```bash
#!/bin/sh
set -e
if [ -f /crons.conf ] && [ -s /crons.conf ]; then
    crontab /crons.conf
    cron
fi
exec python -m sir amqp_watch
```

**`musicbrainz/indexer.ini`:**

```ini
[database]
dbname = musicbrainz_db
host = musicbrainz-db
port = 5432
user = musicbrainz
password = musicbrainz

[solr]
uri = http://musicbrainz-search:8983/solr
batch_size = 200

[sir]
import_threads = 16
index_limit = 200000
live_index_batch_size = 100
process_delay = 15
query_batch_size = 5000
wscompat = on

[rabbitmq]
host = musicbrainz-mq
user = sir
password = sir
vhost = /search-index-rebuilder
prefetch_count = 350
```

**`musicbrainz/crons.conf`** (hourly replication, mounted into musicbrainz-web):

```
BASH_ENV=/noninteractive.bash_env
0 * * * * /usr/local/bin/replication.sh >> /musicbrainz-server/mirror.log 2>&1
```

**`musicbrainz/sir-crons.conf`** (weekly full reindex, mounted into musicbrainz-sir):

```
0 4 * * 0 cd /code && python -m sir reindex >> /var/log/sir-reindex.log 2>&1
```

**One-time setup after first boot:**

```bash
# 1. Set up RabbitMQ vhost and user for SIR
docker exec musicbrainz-mq rabbitmqctl add_vhost /search-index-rebuilder
docker exec musicbrainz-mq rabbitmqctl add_user sir sir
docker exec musicbrainz-mq rabbitmqctl set_permissions -p /search-index-rebuilder sir ".*" ".*" ".*"

# 2. Install pg_amqp extension and configure the broker
docker exec musicbrainz-db psql -U musicbrainz musicbrainz_db \
  -c "CREATE EXTENSION amqp;"
docker exec musicbrainz-db psql -U musicbrainz musicbrainz_db \
  -c "INSERT INTO amqp.broker (host, port, vhost, username, password) VALUES ('musicbrainz-mq', 5672, '/search-index-rebuilder', 'sir', 'sir');"

# 3. Set up AMQP queues and DB triggers
docker exec musicbrainz-sir python -m sir amqp_setup
docker exec musicbrainz-sir python -m sir triggers --broker-id 1 \
  -f /tmp/CreateFunctions.sql -t /tmp/CreateTriggers.sql
docker cp musicbrainz-sir:/tmp/CreateFunctions.sql /tmp/
docker cp musicbrainz-sir:/tmp/CreateTriggers.sql /tmp/
docker cp /tmp/CreateFunctions.sql musicbrainz-web:/tmp/
docker cp /tmp/CreateTriggers.sql musicbrainz-web:/tmp/
docker exec musicbrainz-web bash -c \
  'cd /musicbrainz-server && carton exec -- admin/psql < /tmp/CreateFunctions.sql'
docker exec musicbrainz-web bash -c \
  'cd /musicbrainz-server && carton exec -- admin/psql < /tmp/CreateTriggers.sql'

# 4. Build search indices (takes 1-4 hours depending on CPU)
docker exec musicbrainz-sir python -m sir reindex
```

## Debugging

### Container Logs

```bash
# Follow live output from both mm and mm-web
docker logs -f mm

# Last 100 lines
docker logs --tail 100 mm
```

### Shell Access

```bash
docker exec -it mm bash
```

From there you can inspect the filesystem, check the socket, or run queries:

```bash
# Is the socket alive?
ls -la /tmp/mm.sock

# What's in the config?
cat /config/mm/config.kdl

# Database size
ls -lh /data/mm/mm.db*

# Quick DB check
sqlite3 -readonly /data/mm/mm.db "SELECT COUNT(*) FROM files WHERE zone = 'corpus';"
```

### Common Issues

**Database locked errors:** Multiple writers contending. This shouldn't happen in normal operation — only one write thread exists. If you see this, check whether you have a `sqlite3` session open in write mode against the same database.

**Large WAL file:** Normal during heavy computation (initial scan, bulk AcoustID matching). The WAL checkpoints automatically. If it persists after the Witch is idle, a restart will force a checkpoint.

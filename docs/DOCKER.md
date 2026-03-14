# Docker Deployment

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

The container runs both `mm` (Witch server) and `mm-web` (HTTP API + web UI) together. On first start, connect to the web UI to run first-time setup.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `MM_ROOT` | `/music` | Corpus root inside the container |
| `MM_WEB_LISTEN` | `0.0.0.0:3313` | mm-web bind address |
| `MM_WEB_STATIC_DIR` | `/srv/mm-web/static` | Path to web UI static assets |
| `MM_ACOUSTID_API_KEY` | *(none)* | AcoustID API key for fingerprint matching |
| `MM_MB_BASE_URL` | *(public API)* | MusicBrainz API base URL (use local mirror) |
| `MM_MB_REQUESTS_PER_SECOND` | `1` | MB API rate limit (raise with local mirror) |
| `MM_WORKER_THREADS` | *(auto)* | Number of worker threads |
| `MM_DB_CACHE` | *(default)* | SQLite cache size (e.g. `512mb`, `1gb`) |

All `MM_CFG__BLOCK__FIELD` generic overrides also work — see `src/config/env_override.rs`.

## Volumes

| Container Path | Purpose |
|----------------|---------|
| `/music` | Music corpus root (bind-mount your library) |
| `/config` | `XDG_CONFIG_HOME` — stores `mm/config.kdl` |
| `/data` | `XDG_DATA_HOME` — stores `mm/mm.db` and logs |

## Rebuilding from HEAD

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
      - /mnt/pool/archive/music:/music
      - mm_config:/config
      - mm_data:/data
    environment:
      - MM_ROOT=/music
      - MM_MB_BASE_URL=http://musicbrainz-web:5000
      - MM_MB_REQUESTS_PER_SECOND=50
      - MM_ACOUSTID_API_KEY=${ACOUSTID_API_KEY}
      - MM_DB_CACHE=1gb

  # ── MusicBrainz Mirror ────────────────────────────────────────────────
  # Uses official prebuilt images. See https://musicbrainz.org/doc/MusicBrainz_Docker
  musicbrainz-db:
    image: ghcr.io/metabrainz/musicbrainz-docker-postgres:2024-01
    container_name: musicbrainz-db
    restart: unless-stopped
    volumes:
      - mb_pgdata:/var/lib/postgresql/data
    environment:
      - POSTGRES_USER=musicbrainz
      - POSTGRES_PASSWORD=musicbrainz

  musicbrainz-redis:
    image: redis:7-alpine
    container_name: musicbrainz-redis
    restart: unless-stopped

  musicbrainz-web:
    image: ghcr.io/metabrainz/musicbrainz-docker:latest
    container_name: musicbrainz-web
    restart: unless-stopped
    depends_on:
      - musicbrainz-db
      - musicbrainz-redis
    environment:
      - MUSICBRAINZ_SERVER_URL=http://musicbrainz-web:5000
      - MUSICBRAINZ_WEB_SERVER_HOST=musicbrainz-web
      - MUSICBRAINZ_DB_HOST=musicbrainz-db
      - MUSICBRAINZ_REDIS_HOST=musicbrainz-redis

  # ── Navidrome (streaming) ─────────────────────────────────────────────
  navidrome:
    image: deluan/navidrome:latest
    container_name: navidrome
    restart: unless-stopped
    user: "1000:1000"
    volumes:
      - navidrome_data:/data
      - /mnt/pool/archive/music/libraries:/music:ro
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
      - /mnt/pool/archive/music:/music:ro

volumes:
  mm_config:
  mm_data:
  mb_pgdata:
  navidrome_data:
  audiomuse_pgdata:
```

Adjust volume paths and image tags for your environment. The key integration point is `MM_MB_BASE_URL` — pointing MM at a local MusicBrainz mirror eliminates the 1 req/s public API limit.

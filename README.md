# Memories

Self-hosted **memory store for LLM workflows**. Your coding agents (Claude
Code, etc.) read and write markdown memory files through a REST API or MCP,
while humans browse, edit, organize and share the same files in a Material
Design web explorer. Live updates keep everyone — humans and LLMs — in sync.

- **Backend:** Rust (axum) — memories are plain **markdown files** on disk,
  metadata (users, groups, shares, tokens, events) in SQLite.
- **Frontend:** Vue 3, dark Material Design with paper elevation and a
  Polymer-style click ripple.
- **Interfaces:** Web explorer · REST API (`/api`) · MCP (`/mcp`) · SSE live
  events.

## Data model

```text
data/memories/
├── <project>/            # one folder per project
│   ├── <user>/           # one folder per user who keeps memories there
│   │   ├── notes.md
│   │   └── any/tree.md
│   └── <other-user>/...
└── <another-project>/...
```

- Every signed-in user has **full control of their own space**
  (`project/<their-username>/…`) in every project and can create projects.
- Nobody else's space is visible **unless shared** — with one user or a
  group, per item or per folder, read-only or read & write.
- Admins see everything, manage users and are the only ones who create
  groups. New users belong to **no group by default**.
- A project that contains a single visible folder opens straight into it.

## The explorer

- **Breadcrumb** — every ancestor is clickable (and a drag & drop target).
- **Rubber-band selection** — hold the left mouse button on empty space and
  draw; the translucent rectangle live-selects what it touches.
- **Keyboard** — `Del` deletes the selection (a floating action bar also
  appears), `Ctrl+C/X/V` copy/cut/paste, `Ctrl+A` select all, `Esc` clears.
- **Drag & drop** — drop items on a folder card or a breadcrumb ancestor to
  move them.
- **⋮ menu** on each item — Share…, Rename…, Delete.
- **Live badge** — the explorer subscribes to the current project over SSE
  and refreshes automatically when anyone (human or LLM) changes something.

## Quick start

```sh
cp .env.example .env       # set WEB_SECRET (openssl rand -hex 32) + admin creds
docker compose up -d       # or: podman-compose up -d
# open http://localhost:8090 — sign in with ADMIN_USERNAME / ADMIN_PASSWORD
```

Local development:

```sh
# backend (Rust)
cd backend && cargo run                  # API + MCP on :8090

# frontend (Vue) — hot reload on :5174, proxies /api and /mcp to :8090
cd frontend && npm install && npm run dev
```

`cargo test` runs the unit tests **and the Scriber-contract regression
tests** (see below).

## Give your LLM access (copy-paste)

Create a token first: **Settings → API tokens → Create token**
(scope *read & write* so the agent can update memories).

### MCP (Claude Code, Claude Desktop, …)

```sh
claude mcp add --transport http memories https://YOUR-MEMORIES-HOST/mcp \
  --header "Authorization: Bearer YOUR_API_TOKEN"
```

or in JSON configuration:

```json
{
  "mcpServers": {
    "memories": {
      "type": "http",
      "url": "https://YOUR-MEMORIES-HOST/mcp",
      "headers": { "Authorization": "Bearer YOUR_API_TOKEN" }
    }
  }
}
```

Then paste this into your LLM's instructions (CLAUDE.md, system prompt, …):

```text
You have a shared "memories" MCP server (tools: list_projects, list_dir,
read_memory, write_memory, mkdir, delete, move, get_changes, whoami).
Memory files are markdown at paths like <project>/<your-username>/<file>.md.

Working agreement:
1. BEFORE starting work on a project, call get_changes(project, since=0) and
   read the memories of every participant folder you can access — coworkers
   (human or LLM) may have updated them.
2. While working, periodically call get_changes(project, since=<latest_seq you
   saw>) to stay informed of modifications made by coworkers.
3. Push updates to YOUR memory files regularly with write_memory so others
   stay up to date — don't wait until the end of the session.
```

### REST API

```text
Base URL: https://YOUR-MEMORIES-HOST/api
Auth:     Authorization: Bearer YOUR_API_TOKEN

GET  /api/fs/list?path=<dir>            list a directory ("" = projects)
GET  /api/fs/file?path=<p>              read a memory   -> {"content"}
PUT  /api/fs/file {"path","content"}    create/update a memory (readwrite)
POST /api/fs/mkdir {"path"}             create folder/project
POST /api/fs/delete {"paths":[...]}     delete items (readwrite)
POST /api/fs/move {"paths":[],"dest"}   move items (readwrite)
POST /api/fs/copy {"paths":[],"dest"}   copy items (readwrite)
GET  /api/events?project=&since=<seq>   change log      -> {"events","latest_seq"}
GET  /api/events/stream?project=        SSE live stream (also ?access_token=)
```

Example — subscribe to live changes and update a memory:

```sh
# live events (SSE)
curl -N "https://HOST/api/events/stream?project=acme&access_token=$TOKEN"

# update a memory
curl -X PUT https://HOST/api/fs/file \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"path":"acme/claude/progress.md","content":"# Progress\n- refactor done"}'
```

## Live updates — the subscribe workflow

Every change (create / update / delete / move / rename) is recorded in a
sequential per-project event log and broadcast over SSE:

- **Humans** see the explorer refresh live (the `● live` badge).
- **LLMs** call the `get_changes` MCP tool (or `GET /api/events`) with the
  last `latest_seq` they saw — the MCP server's instructions tell agents to
  subscribe before starting and to push their updates regularly, so a team
  of humans and agents stays converged on the same memory state.

## Compatible memory providers (Scriber integration)

Memories can **mount an external memory source as a virtual project**. The
built-in provider mounts a [Scriber](https://github.com/lp177/Scriber)
instance: each meeting participant appears as a folder holding `memory.md`,
readable and writable from the explorer, API and MCP.

```ini
# .env
SCRIBER_URL=https://scriber.example.com
SCRIBER_TOKEN=<scriber API token — created in Scriber Settings, readwrite>
SCRIBER_MOUNT=scriber
```

### The provider contract

Any service can be integrated the same way by exposing these three
endpoints, authenticated with `Authorization: Bearer <token>` (readwrite
scope required for the PUT):

| Endpoint | Response |
| --- | --- |
| `GET /api/v1/participants?limit=&offset=` | `{"total": N, "items": [{"id", "display_name"}]}` |
| `GET /api/v1/participants/{id}/memory` | `{"content": "<markdown>"}` |
| `PUT /api/v1/participants/{id}/memory` `{"content"}` | `{"ok": true}` |

Unknown/missing tokens must yield **401**. Additional item fields are
ignored; `display_name` may be null (the id is shown instead).

### Regression tests

`backend/tests/scriber_contract.rs` pins this contract with a mock provider:
pagination, auth enforcement (401 on wrong token), memory read/write
round-trip, and safe folder-name mapping. Run with `cargo test`. If Scriber
changes its API shapes — or our client drifts — these tests fail first.

## Administration

- **Admin panel** (`/#/admin`, admins only): create users, edit pseudo /
  e-mail / password, toggle admin, delete; **Reset password** creates a
  one-hour link — e-mailed when SMTP is configured, otherwise shown for you
  to hand over. Group management: create/rename/delete groups, assign
  members.
- **Settings** (every user): pseudo, e-mail, avatar, password change, and
  **API tokens** (create with read or read & write scope, see last-used
  time, revoke instantly).

## Secrets via Infisical (optional)

In production the secrets (`WEB_SECRET`, `ADMIN_PASSWORD`, `SCRIBER_TOKEN`,
SMTP credentials) don't have to live in the `.env` file: the container
entrypoint can fetch them from an [Infisical](https://infisical.com) instance
at startup with a **Universal Auth machine identity** and inject them via
`infisical run`.

1. In Infisical: create a project (e.g. `memories`), add the secrets to an
   environment (e.g. `prod`), create a **machine identity** with Universal
   Auth and grant it read access to that project.
2. Keep only the identity + non-secret config in the env file:

   ```ini
   INFISICAL_API_URL=https://your-infisical-host/api
   INFISICAL_CLIENT_ID=<machine identity client id>
   INFISICAL_CLIENT_SECRET=<machine identity client secret>
   INFISICAL_PROJECT_ID=<project id>
   INFISICAL_ENV=prod
   ```

3. Restart the container. The entrypoint logs
   `fetching secrets from Infisical` and starts the app with the injected
   environment. When the `INFISICAL_*` values are absent it falls back to the
   plain environment, so development setups keep working unchanged.

## Environment reference

| Key | Default | Purpose |
| --- | --- | --- |
| `WEB_HOST` / `WEB_PORT` | `0.0.0.0` / `8090` | bind address (container-internal port) |
| `PUBLIC_URL` | `http://localhost:8090` | base URL used in reset links |
| `WEB_SECRET` | *(empty)* | HMAC key for sessions — set it in production |
| `ADMIN_USERNAME` / `ADMIN_PASSWORD` | `admin` / `change-me` | bootstrap admin (first run) |
| `MEMORIES_DATA_DIR` | `./data` (`/data` in container) | markdown tree + SQLite + avatars |
| `SMTP_HOST/PORT/USERNAME/PASSWORD/FROM` | *(unset)* | reset-mail delivery (optional) |
| `SCRIBER_URL` / `SCRIBER_TOKEN` / `SCRIBER_MOUNT` | *(unset)* / `scriber` | external Scriber provider |

## License

MIT.

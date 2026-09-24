# PR to Go

Review GitHub pull requests offline. Sync your PRs while you're connected,
review them with a full diff viewer on the plane or train, and your comments
and verdicts wait in an outbox until you're back online.

> **Status:** pre-release. Everything below works against the fake GitHub in
> our tests. Against real GitHub, syncing is verified with public PRs, and
> sending reviews with a throwaway repository. There are no signed builds. See [docs/DESIGN.md](docs/DESIGN.md) for the design, and
> [§15](docs/DESIGN.md#15-implementation-status) for what's built and what's
> still open.

## How it works

- **Your inbox** follows repositories and searches, such as "review requested
  from me". It checks them every 15 minutes while you're online. **Sync all**
  packs every PR in it for offline use and shows what's ready. You can also
  add any PR by URL.
- **Browse** lists every open PR you can reach, newest first: your own
  repositories, your organizations and repositories shared with you. It
  takes GitHub search filters such as `author:bob` or `repo:acme/api`. Pick
  one to take it offline; nothing else is downloaded.
- **Sync** fetches everything a review needs:
  - PR metadata and the description, with its images;
  - GitHub's per-file patches;
  - full file contents at the merge base and head, so you can expand context;
  - existing review threads;
  - a snapshot of CI status, re-checked while checks are still running.
- **Review offline** in side-by-side or unified diffs with syntax
  highlighting and find (⌘F). Tick files off as you go: **Viewed, next file**
  (⌘D) marks the current one and opens the next unviewed file. Generated files, such as lockfiles and anything marked
  `linguist-generated`, start collapsed. Draft inline comments on lines or
  ranges, replies, suggested changes, a summary and a verdict. Drafts are
  saved locally as you type.
- **Keyboard first.** Everything is in the macOS menu bar with a shortcut;
  **Help → Keyboard Shortcuts** (⌘/) lists them all.
- **The outbox** submits each review as a single GitHub review when you
  reconnect. If the PR changed while you were away, it stops and shows you
  what moved, so you can remap, keep or drop each comment instead of having
  them land on the wrong lines.

## Development

Requirements: [rustup](https://rustup.rs) (the toolchain is pinned in
`rust-toolchain.toml`), Node 22 and pnpm 10, plus the
[Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS.

```sh
cargo test                 # core, sync and outbox tests against a fake GitHub
cd app
pnpm install
pnpm tauri dev             # run the desktop app against real GitHub
pnpm test                  # UI unit tests
pnpm e2e                   # UI + core + fake GitHub in Chromium (Playwright)
pnpm validate:graphql      # check our GraphQL queries against GitHub's schema
pnpm tauri build           # package the app (unsigned for now)
```

Two tests talk to real GitHub and are skipped unless asked for. `live` syncs
public PRs and changes nothing. `live_write` opens pull requests in a
repository you name, sends reviews on them, and closes them again (about a
minute), so point it at a throwaway repository you can push to. The **Live
GitHub** workflow runs both every Monday against our playground repository.

```sh
GITHUB_TOKEN=$(gh auth token) cargo test -p pr-to-go-core --test live -- --ignored
GITHUB_TOKEN=$(gh auth token) LIVE_WRITE_REPO=you/throwaway \
  cargo test -p pr-to-go-core --test live_write -- --ignored
```

Logs are written to the platform's log folder
(`~/Library/Logs/com.abersager.prtogo` on macOS; **Settings → Show log
files**). `PRTOGO_LOG=debug` (or any `tracing` filter) changes what's logged.
Requests are logged without headers or bodies, so the token never appears.

### Without GitHub

`cargo run -p dev-server` (binary `prtg-dev`) runs the real core against a
fake GitHub filled with demo pull requests, and serves the UI's API over
HTTP. Then `pnpm dev` in `app/` and open http://localhost:1420 in a browser.
To simulate network loss and PR changes, POST to
`http://127.0.0.1:1421/__demo/<action>`:

- `offline` and `online` take GitHub away and bring it back.
- `push-retry` force-pushes the first demo PR.
- `request-changes` adds a blocking review from someone else.
- `large` adds a 400-file PR for performance work.
- `reset` starts over with a fresh demo.

To run the desktop app against it, in a debug build:

```sh
PRTOGO_API_BASE=http://127.0.0.1:1422 PRTOGO_TOKEN=test-token \
PRTOGO_DATA_DIR=/tmp/prtogo-dev pnpm tauri dev
```

These variables are ignored in release builds. The token stays in memory,
so a dev run never touches your keychain. On macOS, `PRTOGO_SNAPSHOT_DIR`
and `PRTOGO_SNAPSHOTS="name=#/pr/1/files;…"` save PNG snapshots of the
webview for each route.

Layout:

| Path | What |
|---|---|
| `crates/core` | Storage (SQLite), GitHub client, sync engine, outbox, remap engine. No UI dependencies. |
| `crates/fake-github` | A stateful in-process fake of the GitHub API subset we use, with fault injection, for tests. |
| `crates/dev-server` | `prtg-dev`: the core plus the fake GitHub with demo data, over HTTP, for browser development and E2E tests. |
| `app/src-tauri` | The Tauri shell: exposes the core to the UI. |
| `app/src-isolation` | Tauri's isolation frame: only the IPC calls the UI makes get through. |
| `app/src` | The React/TypeScript UI. |

## Prior art and credits

PR to Go stands on the shoulders of two projects. We borrowed ideas (not code)
from both; details are in [docs/DESIGN.md §1](docs/DESIGN.md#1-prior-art).

- **[Hubtty](https://github.com/hubtty/hubtty)** (Apache-2.0), a terminal UI
  for GitHub code review forked from Gertty. From Hubtty: the local database as
  the durable outbox, rescanned after reconnecting; holding a review when
  someone requests changes after you drafted an approval; its backoff
  schedules for secondary rate limits and for re-polling pending CI checks;
  noticing when search results hit GitHub's 1,000-result cap; collapsing
  generated files (`linguist-generated`).
- **[prr](https://github.com/danobi/prr)** (GPL-2.0), a CLI for mailing-list
  style reviews of GitHub PRs. From prr: pinning a review to the commit you
  reviewed, keeping the exact diff you reviewed, never overwriting unsubmitted
  work, and restricting comment ranges to a single hunk and file.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

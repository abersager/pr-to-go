# PR to Go

Review GitHub pull requests offline. Sync your PRs while you're connected,
review them with a full diff viewer on the plane or train, and your comments
and verdicts wait in an outbox until you're back online.

> **Status:** early development. See [docs/DESIGN.md](docs/DESIGN.md) for the
> design and the build plan.

## How it works

- **Sync** fetches everything a review needs: PR metadata and description
  (with images), GitHub's per-file patches, full file contents at the merge
  base and head (so you can expand context), existing review threads, and a
  snapshot of CI status.
- **Review offline** in side-by-side or unified diffs with syntax
  highlighting. Draft inline comments on lines or ranges, replies, suggested
  changes, a summary and a verdict. Drafts are saved locally as you type.
- **The outbox** submits each review as a single GitHub review when you
  reconnect. If the PR changed while you were away, it stops and shows you
  what moved, so you can remap, keep or drop each comment instead of having
  them land on the wrong lines.

## Development

Requirements: [rustup](https://rustup.rs) (the toolchain is pinned in
`rust-toolchain.toml`), Node 22 and pnpm 10, plus the
[Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS.

```sh
cargo test                 # core + sync/outbox tests against a fake GitHub
cd app
pnpm install
pnpm tauri dev             # run the desktop app
pnpm test                  # UI tests
pnpm tauri build           # package the app
```

Layout:

| Path | What |
|---|---|
| `crates/core` | Storage (SQLite), GitHub client, sync engine, outbox, remap engine. No UI dependencies. |
| `crates/fake-github` | A stateful in-process fake of the GitHub API subset we use, with fault injection, for tests. |
| `app/src-tauri` | The Tauri shell: exposes the core to the UI. |
| `app/src` | The React/TypeScript UI. |

## Prior art and credits

PR to Go stands on the shoulders of two projects. We borrowed ideas (not code)
from both; details are in [docs/DESIGN.md §1](docs/DESIGN.md#1-prior-art).

- **[Hubtty](https://github.com/hubtty/hubtty)** (Apache-2.0), a terminal UI
  for GitHub code review forked from Gertty. From Hubtty: the local database as
  the durable outbox, rescanned after reconnecting; holding a review when
  someone requests changes after you drafted an approval; a deduplicating
  priority task queue; its rate-limit backoff schedule; the fallback when
  search results are truncated; re-polling pending CI checks; collapsing
  generated files.
- **[prr](https://github.com/danobi/prr)** (GPL-2.0), a CLI for mailing-list
  style reviews of GitHub PRs. From prr: pinning a review to the commit you
  reviewed, keeping the exact diff you reviewed, never overwriting unsubmitted
  work, and restricting comment ranges to a single hunk and file.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

//! A realistic demo world for the dev server, screenshots and E2E tests.

use crate::model::{VIEWER, World};

pub const REPO: &str = "acme/widgets";

const PNG_A: &[u8] = include_bytes!("../assets/architecture-v1.png");
const PNG_B: &[u8] = include_bytes!("../assets/architecture-v2.png");
const BACKOFF_PNG: &[u8] = include_bytes!("../assets/backoff.png");

const CARGO_TOML: &str = r#"[package]
name = "widgets"
version = "0.3.0"
edition = "2021"

[dependencies]
reqwest = { version = "0.12", features = ["json"] }
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
"#;

const README: &str = "# Widgets\n\nA client for the Widgets API.\n\n## Usage\n\n```rust\nlet client = widgets::Client::new(token);\nlet w = client.widget(42).await?;\n```\n";

const LIB_RS: &str =
    "//! Client for the Widgets API.\n\nmod client;\n\npub use client::{Client, Error, Widget};\n";

const CLIENT_RS: &str = r#"use serde::Deserialize;

const API: &str = "https://api.widgets.example";

#[derive(Debug, Deserialize)]
pub struct Widget {
    pub id: u64,
    pub name: String,
    pub price_cents: u64,
}

#[derive(Debug)]
pub enum Error {
    Http(reqwest::Error),
    NotFound(u64),
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Http(e)
    }
}

pub struct Client {
    http: reqwest::Client,
    token: String,
}

impl Client {
    pub fn new(token: impl Into<String>) -> Self {
        Client { http: reqwest::Client::new(), token: token.into() }
    }

    /// Fetches one widget.
    pub async fn widget(&self, id: u64) -> Result<Widget, Error> {
        let resp = self
            .http
            .get(format!("{API}/widgets/{id}"))
            .bearer_auth(&self.token)
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Error::NotFound(id));
        }
        Ok(resp.error_for_status()?.json().await?)
    }

    /// Lists every widget. Pages through the results.
    pub async fn widgets(&self) -> Result<Vec<Widget>, Error> {
        let mut out = Vec::new();
        let mut page = 1;
        loop {
            let batch: Vec<Widget> = self
                .http
                .get(format!("{API}/widgets?page={page}"))
                .bearer_auth(&self.token)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            if batch.is_empty() {
                return Ok(out);
            }
            out.extend(batch);
            page += 1;
        }
    }
}
"#;

const RETRY_RS: &str = r#"//! Retries with exponential backoff and jitter.

use std::future::Future;
use std::time::Duration;

/// How many times to try, and how long to wait between tries.
#[derive(Clone, Copy, Debug)]
pub struct Backoff {
    pub attempts: u32,
    pub base: Duration,
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff { attempts: 5, base: Duration::from_millis(200), max: Duration::from_secs(10) }
    }
}

impl Backoff {
    /// The delay before attempt `n` (starting at 1), with full jitter.
    pub fn delay(&self, n: u32) -> Duration {
        let exp = self.base.saturating_mul(1 << n.min(16));
        let capped = exp.min(self.max);
        capped.mul_f64(rand::random::<f64>())
    }
}

/// Runs `f` until it succeeds, it returns an error `retryable` rejects, or
/// the attempts run out.
pub async fn retry<T, E, F, Fut>(b: Backoff, retryable: impl Fn(&E) -> bool, mut f: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut n = 1;
    loop {
        match f().await {
            Err(e) if n < b.attempts && retryable(&e) => {
                tokio::time::sleep(b.delay(n)).await;
                n += 1;
            }
            other => return other,
        }
    }
}
"#;

const SETTINGS_TSX: &str = r#"import { useState } from "react";
import "./theme.css";

type Settings = { notifications: boolean; currency: string };

export function SettingsPage({ initial, onSave }: { initial: Settings; onSave: (s: Settings) => void }) {
  const [settings, setSettings] = useState(initial);
  return (
    <form
      className="settings"
      onSubmit={(e) => {
        e.preventDefault();
        onSave(settings);
      }}
    >
      <h1>Settings</h1>
      <label>
        <input
          type="checkbox"
          checked={settings.notifications}
          onChange={(e) => setSettings({ ...settings, notifications: e.target.checked })}
        />
        Email me about price changes
      </label>
      <label>
        Currency
        <select value={settings.currency} onChange={(e) => setSettings({ ...settings, currency: e.target.value })}>
          <option>EUR</option>
          <option>USD</option>
        </select>
      </label>
      <button type="submit">Save</button>
    </form>
  );
}
"#;

const THEME_CSS: &str = ".settings {\n  background: white;\n  color: #222;\n  padding: 24px;\n}\n\n.settings label {\n  display: block;\n  margin: 12px 0;\n}\n";

fn fixtures(n: usize) -> String {
    let mut s = String::from("[\n");
    for i in 0..n {
        s.push_str(&format!(
            "  {{ \"id\": {i}, \"name\": \"Widget {i}\", \"price_cents\": {}, \"tags\": [\"demo\", \"fixture\"] }}{}\n",
            1000 + i * 7,
            if i + 1 < n { "," } else { "" }
        ));
    }
    s.push_str("]\n");
    s
}

pub struct Demo {
    pub retry_pr: u64,
    pub dark_mode_pr: u64,
    pub fixtures_pr: u64,
}

/// Builds the demo repo and its PRs.
pub fn build(w: &mut World) -> Demo {
    w.create_repo(REPO);
    w.patch_limit = 60_000;
    let base = w.commit_bytes(
        REPO,
        None,
        &[
            ("Cargo.toml", Some(CARGO_TOML.as_bytes())),
            ("README.md", Some(README.as_bytes())),
            ("src/lib.rs", Some(LIB_RS.as_bytes())),
            ("src/client.rs", Some(CLIENT_RS.as_bytes())),
            ("docs/architecture.png", Some(PNG_A)),
            ("web/settings.tsx", Some(SETTINGS_TSX.as_bytes())),
            ("web/theme.css", Some(THEME_CSS.as_bytes())),
            ("tests/fixtures/widgets.json", Some(fixtures(20).as_bytes())),
        ],
        "Initial commit",
    );
    w.set_branch(REPO, "main", &base);

    // PR 1: retries. Two commits, reviewed, then pushed to again.
    let c1 = w.commit(
        REPO,
        Some(&base),
        &[
            ("src/retry.rs", Some(RETRY_RS)),
            ("src/lib.rs", Some(&LIB_RS.replace("mod client;\n", "mod client;\nmod retry;\n"))),
            ("Cargo.toml", Some(&CARGO_TOML.replace("reqwest =", "rand = \"0.8\"\nreqwest ="))),
        ],
        "Add a retry helper with exponential backoff",
    );
    let client_v1 = CLIENT_RS
        .replace("use serde::Deserialize;\n", "use serde::Deserialize;\n\nuse crate::retry::{Backoff, retry};\n")
        .replace(
            "pub struct Client {\n    http: reqwest::Client,\n    token: String,\n}",
            "pub struct Client {\n    http: reqwest::Client,\n    token: String,\n    backoff: Backoff,\n}",
        )
        .replace(
            "Client { http: reqwest::Client::new(), token: token.into() }",
            "Client { http: reqwest::Client::new(), token: token.into(), backoff: Backoff::default() }",
        )
        .replace(
            "        let resp = self\n            .http\n            .get(format!(\"{API}/widgets/{id}\"))\n            .bearer_auth(&self.token)\n            .send()\n            .await?;",
            "        let url = format!(\"{API}/widgets/{id}\");\n        let resp = retry(self.backoff, |e: &reqwest::Error| e.is_timeout(), || {\n            self.http.get(&url).bearer_auth(&self.token).send()\n        })\n        .await?;",
        );
    let c2 = w.commit_bytes(
        REPO,
        Some(&c1),
        &[
            ("src/client.rs", Some(client_v1.as_bytes())),
            (
                "README.md",
                Some(
                    format!("{README}\nRequests that time out are retried up to five times, with exponential backoff.\n")
                        .as_bytes(),
                ),
            ),
            ("docs/architecture.png", Some(PNG_B)),
        ],
        "Retry timed-out requests in Client::widget",
    );
    let img = format!("{}/assets/retry-diagram.png", w.base_url);
    w.assets.insert("retry-diagram.png".into(), ("image/png".into(), BACKOFF_PNG.to_vec()));
    let retry_pr = w.open_pr(
        REPO,
        "main",
        "retry-backoff",
        &c2,
        "Retry failed requests with exponential backoff",
        &format!(
            "Requests to the Widgets API sometimes time out under load. This retries them with exponential \
             backoff and full jitter.\n\n- Adds `retry::retry` and `Backoff`\n- Uses it in `Client::widget`\n- \
             `Client::widgets` is next, in a follow-up\n\n![How retries are spaced]({img})"
        ),
        "alice",
    );
    w.add_review(
        REPO,
        retry_pr,
        "bob",
        "COMMENTED",
        "Nice. A couple of questions.",
        &[
            (
                "src/retry.rs",
                "RIGHT",
                16,
                "Should `attempts` be configurable per call? Bulk jobs might want more.",
            ),
            ("src/client.rs", "RIGHT", 40, "Retrying only on timeouts seems narrow. What about 502s?"),
        ],
    );
    w.add_issue_comment(REPO, retry_pr, "alice", "Good point about 502s, pushing a fix.");
    // Alice's follow-up push outdates Bob's comment on line 36.
    let client_v2 = client_v1.replace(
        "|e: &reqwest::Error| e.is_timeout()",
        "|e: &reqwest::Error| e.is_timeout() || e.status().is_some_and(|s| s.is_server_error())",
    );
    let c3 = w.commit(REPO, Some(&c2), &[("src/client.rs", Some(&client_v2))], "Also retry 5xx responses");
    w.push(REPO, retry_pr, &c3);
    w.set_checks(
        REPO,
        &c3,
        &[("test", "COMPLETED", Some("SUCCESS")), ("clippy", "COMPLETED", Some("FAILURE"))],
    );

    // PR 2: a draft, not reviewed yet.
    let dark = w.commit(
        REPO,
        Some(&base),
        &[
            (
                "web/theme.css",
                Some(
                    ".settings {\n  background: var(--bg, white);\n  color: var(--fg, #222);\n  padding: 24px;\n}\n\n.settings label {\n  display: block;\n  margin: 12px 0;\n}\n\n@media (prefers-color-scheme: dark) {\n  .settings {\n    --bg: #111;\n    --fg: #eee;\n  }\n}\n",
                ),
            ),
            (
                "web/settings.tsx",
                Some(&SETTINGS_TSX.replace("<h1>Settings</h1>", "<h1>Settings</h1>\n      <p className=\"hint\">Follows your system's light or dark mode.</p>")),
            ),
        ],
        "Dark mode for the settings page",
    );
    let dark_mode_pr = w.open_pr(
        REPO,
        "main",
        "dark-settings",
        &dark,
        "Dark mode for the settings page",
        "Uses CSS variables so the settings page follows the system theme.",
        "bob",
    );
    w.pr(REPO, dark_mode_pr).is_draft = true;
    w.set_checks(REPO, &dark, &[("test", "IN_PROGRESS", None)]);

    // PR 3: by the viewer, with a generated file too big for GitHub's patch.
    let fx = w.commit(
        REPO,
        Some(&base),
        &[
            ("tests/fixtures/widgets.json", Some(&fixtures(900))),
            (
                "README.md",
                Some(&format!("{README}\nFixtures are regenerated with `cargo xtask fixtures`.\n")),
            ),
        ],
        "Regenerate API fixtures",
    );
    let fixtures_pr = w.open_pr(
        REPO,
        "main",
        "fixtures",
        &fx,
        "Regenerate API fixtures",
        "Regenerated from the staging API. No hand edits.",
        VIEWER,
    );
    w.set_checks(REPO, &fx, &[("test", "COMPLETED", Some("SUCCESS"))]);

    Demo { retry_pr, dark_mode_pr, fixtures_pr }
}

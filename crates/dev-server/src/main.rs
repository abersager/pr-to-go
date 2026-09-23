//! `prtg-dev`: the real core, a fake GitHub with demo data, and the UI's API
//! over HTTP. Point the Vite dev server at it (see app/vite.config.ts) to
//! run the UI in a plain browser, or use it from Playwright.
//!
//! Options: `--port <n>` (default 1421), `--fake-port <n>` (default 1422),
//! `--data <dir>` (default: a fresh temp dir), `--empty` (don't add the demo
//! PRs), `--signed-out` (start at the sign-in screen).
//!
//! Demo controls (POST): `/__demo/offline`, `/__demo/online`,
//! `/__demo/push-retry` (force-push the retry PR), `/__demo/request-changes`.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, RawQuery, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use fake_github::{FakeGitHub, TOKEN, demo};
use pr_to_go_core::auth::{MemorySecretStore, SecretStore};
use pr_to_go_core::commands::UiError;
use pr_to_go_core::{Core, CoreOptions};
use serde_json::Value;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

struct Dev {
    core: Arc<Core>,
    fake: FakeGitHub,
    demo: demo::Demo,
}

type S = State<Arc<Dev>>;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let port = arg("--port").unwrap_or_else(|| "1421".into());
    let fake_port = arg("--fake-port").unwrap_or_else(|| "1422".into());
    let fake = FakeGitHub::start_on(&format!("127.0.0.1:{fake_port}")).await;
    let demo = fake.with(demo::build);

    let _tmp;
    let data_dir = match arg("--data") {
        Some(d) => std::path::PathBuf::from(d),
        None => {
            let t = tempfile::tempdir()?;
            let p = t.path().to_owned();
            _tmp = t;
            p
        }
    };
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
    let mut opts = CoreOptions::new(data_dir.clone(), secrets);
    opts.github.api_base = fake.api_base().to_string();
    opts.github.mutation_spacing = Duration::from_millis(100);
    let core = Core::open(opts)?;
    if !flag("--signed-out") {
        core.sign_in(TOKEN, "pat").await?;
        if !flag("--empty") {
            for n in [demo.retry_pr, demo.dark_mode_pr, demo.fixtures_pr] {
                core.add_pr(&format!("{}#{n}", demo::REPO)).await?;
            }
        }
    }

    let dev = Arc::new(Dev { core, fake, demo });
    let app = Router::new()
        .route("/api/{cmd}", post(api))
        .route("/events", get(events))
        .route("/prtg/{*path}", get(local))
        .route("/__demo/{action}", post(demo_action))
        .with_state(dev.clone());
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    println!("prtg-dev: API on http://127.0.0.1:{port}");
    println!("prtg-dev: fake GitHub on {} (token {TOKEN})", dev.fake.api_base());
    println!("prtg-dev: data in {}", data_dir.display());
    axum::serve(listener, app).await?;
    Ok(())
}

async fn api(State(d): S, Path(cmd): Path<String>, body: axum::body::Bytes) -> Response {
    let args: Value = if body.is_empty() {
        Value::Object(Default::default())
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => return (StatusCode::BAD_REQUEST, format!("bad JSON: {e}")).into_response(),
        }
    };
    match d.core.dispatch(&cmd, args).await {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, axum::Json(UiError::from(e))).into_response(),
    }
}

async fn events(State(d): S) -> Sse<impl tokio_stream::Stream<Item = Result<SseEvent, Infallible>>> {
    let stream = BroadcastStream::new(d.core.subscribe()).filter_map(|e| {
        let e = e.ok()?;
        Some(Ok(SseEvent::default().data(serde_json::to_string(&e).ok()?)))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn local(State(d): S, Path(path): Path<String>, RawQuery(q): RawQuery) -> Response {
    match d.core.local_resource(&path, q.as_deref()) {
        Some((bytes, ct)) => ([(header::CONTENT_TYPE, ct)], bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn demo_action(State(d): S, Path(action): Path<String>) -> Response {
    match action.as_str() {
        "offline" => d.fake.go_down(),
        "online" => d.fake.come_up().await,
        "push-retry" => {
            let pr = d.demo.retry_pr;
            d.fake.with(|w| {
                let head = w.pr(demo::REPO, pr).head_oid.clone();
                let text = w.repo(demo::REPO).file_text(&head, "src/retry.rs").unwrap();
                let text = text.replace("pub attempts: u32,", "pub attempts: u32,\n    /// Never wait longer than this in total.\n    pub deadline: Option<Duration>,")
                    .replace("Backoff { attempts: 5,", "Backoff { deadline: None, attempts: 5,");
                let base = w.repo(demo::REPO).branches["main"].clone();
                let c = w.commit(demo::REPO, Some(&base), &[("src/retry.rs", Some(&text))], "Squash: retries with a deadline");
                w.push(demo::REPO, pr, &c);
            });
        }
        "request-changes" => {
            let pr = d.demo.retry_pr;
            d.fake.with(|w| {
                w.add_review(demo::REPO, pr, "bob", "CHANGES_REQUESTED", "Please don't retry POSTs.", &[]);
            });
        }
        _ => return (StatusCode::NOT_FOUND, "unknown demo action").into_response(),
    }
    StatusCode::NO_CONTENT.into_response()
}

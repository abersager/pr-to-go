//! `prtg-dev`: the real core, a fake GitHub with demo data, and the UI's API
//! over HTTP. Point the Vite dev server at it (see app/vite.config.ts) to
//! run the UI in a plain browser, or use it from Playwright.
//!
//! Options: `--port <n>` (default 1421), `--fake-port <n>` (default 1422),
//! `--data <dir>` (default: a fresh temp dir), `--empty` (don't add the demo
//! PRs), `--signed-out` (start at the sign-in screen).
//!
//! Demo controls (POST): `/__demo/reset` (fresh world and app state),
//! `/__demo/offline`, `/__demo/online`, `/__demo/push-retry` (force-push the
//! retry PR), `/__demo/request-changes`.

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, RawQuery, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use fake_github::{FakeGitHub, TOKEN, World, demo};
use pr_to_go_core::auth::{MemorySecretStore, SecretStore};
use pr_to_go_core::commands::UiError;
use pr_to_go_core::{Core, CoreOptions};
use serde_json::Value;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

#[derive(Clone)]
struct Opts {
    data: Option<PathBuf>,
    empty: bool,
    signed_out: bool,
}

/// One app instance: its core, the worker running for it, and the demo.
struct Instance {
    core: Arc<Core>,
    worker: tokio::task::JoinHandle<()>,
    demo: demo::Demo,
    _tmp: Option<tempfile::TempDir>,
}

struct Dev {
    fake: FakeGitHub,
    opts: Opts,
    current: RwLock<Instance>,
}

impl Dev {
    fn core(&self) -> Arc<Core> {
        self.current.read().unwrap().core.clone()
    }
}

type S = State<Arc<Dev>>;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

async fn start(fake: &FakeGitHub, demo: demo::Demo, opts: &Opts) -> anyhow::Result<Instance> {
    let (data_dir, tmp) = match &opts.data {
        Some(d) => (d.clone(), None),
        None => {
            let t = tempfile::tempdir()?;
            (t.path().to_owned(), Some(t))
        }
    };
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
    let mut core_opts = CoreOptions::new(data_dir.clone(), secrets);
    core_opts.github.api_base = fake.api_base().to_string();
    core_opts.github.mutation_spacing = Duration::from_millis(100);
    let core = Core::open(core_opts)?;
    if !opts.signed_out {
        core.sign_in(TOKEN, "pat").await?;
        if !opts.empty {
            for n in [demo.retry_pr, demo.dark_mode_pr, demo.fixtures_pr] {
                core.add_pr(&format!("{}#{n}", demo::REPO)).await?;
            }
        }
    }
    println!("prtg-dev: data in {}", data_dir.display());
    let worker = tokio::spawn(core.clone().run_background());
    Ok(Instance { core, worker, demo, _tmp: tmp })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Quiet by default so E2E output stays readable; `PRTOGO_LOG=debug` shows
    // every GitHub request.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("PRTOGO_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    let port = arg("--port").unwrap_or_else(|| "1421".into());
    let fake_port = arg("--fake-port").unwrap_or_else(|| "1422".into());
    let opts = Opts {
        data: arg("--data").map(PathBuf::from),
        empty: flag("--empty"),
        signed_out: flag("--signed-out"),
    };
    let fake = FakeGitHub::start_on(&format!("127.0.0.1:{fake_port}")).await;
    let demo = fake.with(demo::build);
    let instance = start(&fake, demo, &opts).await?;
    let dev = Arc::new(Dev { fake, opts, current: RwLock::new(instance) });

    let app = Router::new()
        .route("/api/{cmd}", post(api))
        .route("/events", get(events))
        .route("/prtg/{*path}", get(local))
        .route("/__demo/{action}", post(demo_action))
        .with_state(dev.clone());
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    println!("prtg-dev: API on http://127.0.0.1:{port}");
    println!("prtg-dev: fake GitHub on {} (token {TOKEN})", dev.fake.api_base());
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
    match d.core().dispatch(&cmd, args).await {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, axum::Json(UiError::from(e))).into_response(),
    }
}

async fn events(State(d): S) -> Sse<impl tokio_stream::Stream<Item = Result<SseEvent, Infallible>>> {
    let stream = BroadcastStream::new(d.core().subscribe()).filter_map(|e| {
        let e = e.ok()?;
        Some(Ok(SseEvent::default().data(serde_json::to_string(&e).ok()?)))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn local(State(d): S, Path(path): Path<String>, RawQuery(q): RawQuery) -> Response {
    match d.core().local_resource(&path, q.as_deref()) {
        Some((bytes, ct)) => ([(header::CONTENT_TYPE, ct)], bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn demo_action(State(d): S, Path(action): Path<String>) -> Response {
    let retry_pr = d.current.read().unwrap().demo.retry_pr;
    match action.as_str() {
        "reset" => {
            d.fake.come_up().await;
            let demo = d.fake.with(|w| {
                let base = w.base_url.clone();
                *w = World::new();
                w.base_url = base;
                demo::build(w)
            });
            match start(&d.fake, demo, &d.opts).await {
                Ok(fresh) => {
                    let old = std::mem::replace(&mut *d.current.write().unwrap(), fresh);
                    old.worker.abort();
                }
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            }
        }
        "offline" => d.fake.go_down(),
        "online" => d.fake.come_up().await,
        "push-retry" => {
            d.fake.with(|w| {
                let head = w.pr(demo::REPO, retry_pr).head_oid.clone();
                let text = w.repo(demo::REPO).file_text(&head, "src/retry.rs").unwrap();
                let text = text
                    .replace(
                        "pub attempts: u32,",
                        "pub attempts: u32,\n    /// Never wait longer than this in total.\n    pub deadline: Option<Duration>,",
                    )
                    .replace("Backoff { attempts: 5,", "Backoff { deadline: None, attempts: 5,");
                let base = w.repo(demo::REPO).branches["main"].clone();
                let c = w.commit(demo::REPO, Some(&base), &[("src/retry.rs", Some(&text))], "Squash: retries with a deadline");
                w.push(demo::REPO, retry_pr, &c);
            });
        }
        "request-changes" => {
            d.fake.with(|w| {
                w.add_review(
                    demo::REPO,
                    retry_pr,
                    "bob",
                    "CHANGES_REQUESTED",
                    "Please don't retry POSTs.",
                    &[],
                );
            });
        }
        // A big PR for performance work: `POST /__demo/large`.
        "large" => {
            let number = d.fake.with(|w| demo::large_pr(w, 400, 20_000));
            return axum::Json(serde_json::json!({ "number": number })).into_response();
        }
        _ => return (StatusCode::NOT_FOUND, "unknown demo action").into_response(),
    }
    StatusCode::NO_CONTENT.into_response()
}

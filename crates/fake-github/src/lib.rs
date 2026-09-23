//! A stateful, in-process fake of the GitHub API subset PR to Go uses.
//!
//! Tests build a world (repos, commits, PRs, reviews), point the core's
//! GitHub client at [`FakeGitHub::api_base`], and inject faults with
//! [`World::fault`]. The fake enforces the rules the outbox relies on: one
//! pending review per user, threads anchored to their review's commit, 422s
//! for lines outside the diff, no approving your own PR.

pub mod demo;
pub mod faults;
pub mod gql;
pub mod model;
pub mod mutations;
pub mod rest;

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};

pub use faults::{Fault, FaultAction};
pub use model::{PrState, TOKEN, VIEWER, World, git_blob_oid};

pub type Shared = Arc<Mutex<World>>;

pub struct FakeGitHub {
    world: Shared,
    base: String,
    addr: std::net::SocketAddr,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

fn router(world: Shared) -> Router {
    Router::new()
        .route("/graphql", post(graphql))
        .route("/rate_limit", get(rest::rate_limit))
        .route("/repos/{owner}/{repo}/pulls/{number}/files", get(rest::pr_files))
        .route("/repos/{owner}/{repo}/compare/{basehead}", get(rest::compare))
        .route("/repos/{owner}/{repo}/git/blobs/{sha}", get(rest::git_blob))
        .route("/assets/{*path}", get(rest::asset))
        .with_state(world)
}

fn serve(listener: tokio::net::TcpListener, world: Shared) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        axum::serve(listener, router(world)).await.expect("fake github server");
    })
}

impl FakeGitHub {
    pub async fn start() -> FakeGitHub {
        Self::start_on("127.0.0.1:0").await
    }

    pub async fn start_on(addr: &str) -> FakeGitHub {
        let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
        let addr = listener.local_addr().unwrap();
        let base = format!("http://{addr}");
        let world = Arc::new(Mutex::new(World::new()));
        world.lock().unwrap().base_url = base.clone();
        let task = serve(listener, world.clone());
        FakeGitHub { world, base, addr, task: Mutex::new(Some(task)) }
    }

    /// The base URL to use as the GitHub API base.
    pub fn api_base(&self) -> &str {
        &self.base
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut World) -> R) -> R {
        f(&mut self.world.lock().unwrap())
    }

    /// Operations and REST routes handled so far.
    pub fn log(&self) -> Vec<String> {
        self.world.lock().unwrap().log.clone()
    }

    pub fn clear_log(&self) {
        self.world.lock().unwrap().log.clear();
    }

    /// Stops listening: connections are refused, as when the network is gone.
    pub fn go_down(&self) {
        if let Some(t) = self.task.lock().unwrap().take() {
            t.abort();
        }
    }

    /// Listens again on the same address, with the same world.
    pub async fn come_up(&self) {
        if self.task.lock().unwrap().is_some() {
            return;
        }
        let mut last = None;
        for _ in 0..50 {
            match tokio::net::TcpListener::bind(self.addr).await {
                Ok(l) => {
                    *self.task.lock().unwrap() = Some(serve(l, self.world.clone()));
                    return;
                }
                Err(e) => {
                    last = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            }
        }
        panic!("couldn't rebind {}: {last:?}", self.addr);
    }

    pub fn is_up(&self) -> bool {
        self.task.lock().unwrap().is_some()
    }
}

impl Drop for FakeGitHub {
    fn drop(&mut self) {
        self.go_down();
    }
}

pub(crate) fn authed(w: &Shared, headers: &HeaderMap) -> Option<String> {
    let v = headers.get("authorization")?.to_str().ok()?;
    let token = v
        .strip_prefix("Bearer ")
        .or_else(|| v.strip_prefix("bearer "))
        .or_else(|| v.strip_prefix("token "))?;
    w.lock().unwrap().tokens.get(token).cloned()
}

/// REST responses: the `core` budget, never low.
pub(crate) fn rate_headers(r: Response) -> Response {
    budget_headers(r, "core", 4999)
}

fn budget_headers(mut r: Response, resource: &str, remaining: u64) -> Response {
    let h = r.headers_mut();
    h.insert("x-ratelimit-resource", resource.parse().unwrap());
    h.insert("x-ratelimit-remaining", remaining.to_string().parse().unwrap());
    h.insert("x-ratelimit-reset", "4102444800".parse().unwrap());
    h.insert("x-oauth-scopes", "repo, read:org".parse().unwrap());
    r
}

pub(crate) fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, axum::Json(json!({ "message": "Bad credentials" }))).into_response()
}

pub(crate) fn not_found() -> Response {
    (StatusCode::NOT_FOUND, axum::Json(json!({ "message": "Not Found" }))).into_response()
}

/// Faults that act before the request is handled. Returns a response to send
/// instead of handling it.
pub(crate) async fn apply_fault_before(w: &Shared, fault: &Option<FaultAction>) -> Option<Response> {
    match fault {
        Some(FaultAction::Status(s)) => {
            let status = StatusCode::from_u16(*s).unwrap();
            Some((status, axum::Json(json!({ "message": "Server Error" }))).into_response())
        }
        Some(FaultAction::CaptivePortal) => Some(
            ([("content-type", "text/html")], "<html><body>Sign in to Train Wi-Fi</body></html>")
                .into_response(),
        ),
        Some(FaultAction::SecondaryRateLimit { retry_after }) => {
            let mut r = (
                StatusCode::FORBIDDEN,
                axum::Json(json!({ "message": "You have exceeded a secondary rate limit. Please wait a few minutes before you try again." })),
            )
                .into_response();
            if let Some(ra) = retry_after {
                r.headers_mut().insert("retry-after", ra.to_string().parse().unwrap());
            }
            r.headers_mut().insert("x-ratelimit-remaining", "4000".parse().unwrap());
            Some(r)
        }
        Some(FaultAction::Delay(d)) => {
            tokio::time::sleep(*d).await;
            None
        }
        Some(FaultAction::Hook(f)) => {
            f(&mut w.lock().unwrap());
            None
        }
        _ => None,
    }
}

/// Faults that act after the request was applied.
pub(crate) async fn apply_fault_after(fault: Option<FaultAction>, resp: Response) -> Response {
    match fault {
        Some(FaultAction::ApplyThenStatus(s)) => {
            (StatusCode::from_u16(s).unwrap(), axum::Json(json!({ "message": "Server Error" })))
                .into_response()
        }
        Some(FaultAction::ApplyThenDelay(d)) => {
            tokio::time::sleep(d).await;
            resp
        }
        _ => resp,
    }
}

async fn graphql(State(w): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(viewer) = authed(&w, &headers) else {
        return unauthorized();
    };
    let req: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad JSON").into_response(),
    };
    let op = req["operationName"].as_str().unwrap_or("").to_string();
    let vars = req.get("variables").cloned().unwrap_or(Value::Null);
    let fault = {
        let mut g = w.lock().unwrap();
        g.log.push(op.clone());
        g.take_fault(&op)
    };
    if let Some(r) = apply_fault_before(&w, &fault).await {
        return r;
    }
    let (result, remaining) = {
        let mut g = w.lock().unwrap();
        (gql::handle(&mut g, &viewer, &op, &vars), g.graphql_remaining)
    };
    let body = match result {
        Ok(data) => json!({ "data": data }),
        Err(e) => {
            let (ty, msg) = match e {
                gql::GqlError::NotFound(m) => ("NOT_FOUND", m),
                gql::GqlError::Unprocessable(m) => ("UNPROCESSABLE", m),
                gql::GqlError::Forbidden(m) => ("FORBIDDEN", m),
            };
            json!({ "data": null, "errors": [ { "type": ty, "message": msg } ] })
        }
    };
    apply_fault_after(fault, budget_headers(axum::Json(body).into_response(), "graphql", remaining)).await
}

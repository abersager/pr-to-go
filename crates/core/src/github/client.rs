//! HTTP transport for GitHub's GraphQL and REST APIs.
//!
//! Every failure is classified into a [`GhError`]. The important split is
//! between [`GhError::Offline`] (the request never reached GitHub, so it's safe
//! to retry) and [`GhError::Ambiguous`] (a mutation may have been applied, so
//! the outbox must check the server before retrying).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use super::GhError;
use crate::db::Db;

#[derive(Clone, Debug)]
pub struct GitHubConfig {
    /// REST base URL without a trailing slash. GraphQL is at `{api_base}/graphql`.
    pub api_base: String,
    pub request_timeout: Duration,
    /// GitHub asks for at least one second between mutations from one user.
    pub mutation_spacing: Duration,
    pub max_concurrency: usize,
    pub user_agent: String,
}

impl Default for GitHubConfig {
    fn default() -> Self {
        GitHubConfig {
            api_base: "https://api.github.com".into(),
            request_timeout: Duration::from_secs(30),
            mutation_spacing: Duration::from_secs(1),
            max_concurrency: 4,
            user_agent: concat!("PR-to-Go/", env!("CARGO_PKG_VERSION")).into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    Query,
    Mutation,
}

#[derive(Default)]
struct LimitState {
    /// Don't send anything before this instant (set by rate-limit responses).
    blocked_until: Option<Instant>,
    /// Consecutive secondary rate limits without a Retry-After header.
    secondary_streak: u32,
    remaining: Option<u64>,
    reset_epoch: Option<u64>,
}

/// Rate-limit budget as last reported by GitHub.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct RateBudget {
    pub remaining: Option<u64>,
    pub reset_epoch: Option<u64>,
}

struct Raw {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

pub struct GitHub {
    http: reqwest::Client,
    cfg: GitHubConfig,
    token: RwLock<Option<String>>,
    work_offline: AtomicBool,
    permits: Semaphore,
    last_mutation: tokio::sync::Mutex<Option<Instant>>,
    limits: Mutex<LimitState>,
    etag_db: Option<Arc<Db>>,
}

impl GitHub {
    pub fn new(cfg: GitHubConfig, etag_db: Option<Arc<Db>>) -> GitHub {
        let http = reqwest::Client::builder()
            .user_agent(cfg.user_agent.clone())
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build HTTP client");
        GitHub {
            http,
            permits: Semaphore::new(cfg.max_concurrency.max(1)),
            cfg,
            token: RwLock::new(None),
            work_offline: AtomicBool::new(false),
            last_mutation: tokio::sync::Mutex::new(None),
            limits: Mutex::new(LimitState::default()),
            etag_db,
        }
    }

    pub fn config(&self) -> &GitHubConfig {
        &self.cfg
    }

    pub fn set_token(&self, token: Option<String>) {
        *self.token.write().unwrap() = token;
    }

    pub fn has_token(&self) -> bool {
        self.token.read().unwrap().is_some()
    }

    /// "Work offline": every request fails with [`GhError::Offline`] without
    /// touching the network.
    pub fn set_work_offline(&self, offline: bool) {
        self.work_offline.store(offline, Ordering::SeqCst);
    }

    pub fn is_work_offline(&self) -> bool {
        self.work_offline.load(Ordering::SeqCst)
    }

    pub fn budget(&self) -> RateBudget {
        let l = self.limits.lock().unwrap();
        RateBudget { remaining: l.remaining, reset_epoch: l.reset_epoch }
    }

    fn token(&self) -> Result<String, GhError> {
        self.token.read().unwrap().clone().ok_or(GhError::NoToken)
    }

    fn gate(&self) -> Result<(), GhError> {
        if self.is_work_offline() {
            return Err(GhError::Offline("working offline".into()));
        }
        let l = self.limits.lock().unwrap();
        if let Some(until) = l.blocked_until {
            let now = Instant::now();
            if until > now {
                return Err(GhError::RateLimited {
                    retry_after_s: (until - now).as_secs().max(1),
                    secondary: false,
                });
            }
        }
        Ok(())
    }

    async fn send(&self, kind: OpKind, req: RequestBuilder) -> Result<Raw, GhError> {
        self.gate()?;
        let _permit = self.permits.acquire().await.expect("semaphore closed");
        // Space out mutations. The lock is held across the send so two
        // mutations can't race past the spacing check.
        let mut last_mutation = match kind {
            OpKind::Mutation => {
                let mut guard = self.last_mutation.lock().await;
                if let Some(prev) = *guard {
                    let elapsed = prev.elapsed();
                    if elapsed < self.cfg.mutation_spacing {
                        tokio::time::sleep(self.cfg.mutation_spacing - elapsed).await;
                    }
                }
                *guard = Some(Instant::now());
                Some(guard)
            }
            OpKind::Query => None,
        };
        let resp = req
            .timeout(self.cfg.request_timeout)
            .send()
            .await
            .map_err(|e| classify_transport(&e, kind, false))?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.bytes().await.map_err(|e| classify_transport(&e, kind, true))?.to_vec();
        if let Some(g) = last_mutation.as_mut() {
            **g = Some(Instant::now());
        }
        self.observe_limits(&headers);
        Ok(Raw { status, headers, body })
    }

    fn observe_limits(&self, headers: &HeaderMap) {
        let mut l = self.limits.lock().unwrap();
        if let Some(r) = header_u64(headers, "x-ratelimit-remaining") {
            l.remaining = Some(r);
        }
        if let Some(r) = header_u64(headers, "x-ratelimit-reset") {
            l.reset_epoch = Some(r);
        }
    }

    /// Classifies a non-2xx response. Rate limits also block further requests
    /// until they expire.
    fn check_status(&self, raw: &Raw, kind: OpKind) -> Result<(), GhError> {
        let s = raw.status.as_u16();
        if raw.status.is_success() || s == 304 {
            self.limits.lock().unwrap().secondary_streak = 0;
            return Ok(());
        }
        if s == 401 {
            return Err(GhError::Unauthorized);
        }
        if let Some(err) = self.rate_limit_error(raw) {
            return Err(err);
        }
        let message = error_message(&raw.body);
        match s {
            403 => Err(GhError::Forbidden { message, sso_url: sso_url(&raw.headers) }),
            404 => Err(GhError::NotFound),
            422 => Err(GhError::Unprocessable(message)),
            500..=599 => Err(match kind {
                OpKind::Mutation => GhError::Ambiguous(format!("HTTP {s}")),
                OpKind::Query => GhError::Server(s),
            }),
            _ => Err(GhError::Protocol(format!("HTTP {s}: {message}"))),
        }
    }

    fn rate_limit_error(&self, raw: &Raw) -> Option<GhError> {
        let s = raw.status.as_u16();
        if s != 403 && s != 429 {
            return None;
        }
        let retry_after = header_u64(&raw.headers, "retry-after");
        let remaining = header_u64(&raw.headers, "x-ratelimit-remaining");
        let reset = header_u64(&raw.headers, "x-ratelimit-reset");
        let body = String::from_utf8_lossy(&raw.body).to_lowercase();
        let (wait, secondary) = if let Some(ra) = retry_after {
            (ra, true)
        } else if remaining == Some(0) {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            (reset.map(|r| r.saturating_sub(now)).unwrap_or(60).max(1), false)
        } else if body.contains("secondary rate limit") || s == 429 {
            // No timing header: GitHub says wait at least a minute, then back
            // off exponentially (60 s doubling to 600 s, as Hubtty does).
            let mut l = self.limits.lock().unwrap();
            let wait = (60u64 << l.secondary_streak.min(4)).min(600);
            l.secondary_streak += 1;
            (wait, true)
        } else {
            return None;
        };
        self.limits.lock().unwrap().blocked_until = Some(Instant::now() + Duration::from_secs(wait));
        Some(GhError::RateLimited { retry_after_s: wait, secondary })
    }

    fn graphql_request(&self, op: &str, query: &str, vars: &Value) -> Result<RequestBuilder, GhError> {
        let body = json!({ "query": query, "variables": vars, "operationName": op });
        Ok(self
            .http
            .post(format!("{}/graphql", self.cfg.api_base))
            .bearer_auth(self.token()?)
            // Keep node ids in one format so stored ids never change shape.
            .header("X-Github-Next-Global-ID", "1")
            .json(&body))
    }

    /// Runs a GraphQL operation and returns `data` plus the response headers.
    pub async fn graphql_raw(
        &self,
        op: &str,
        query: &str,
        vars: Value,
        kind: OpKind,
    ) -> Result<(Value, HeaderMap), GhError> {
        let req = self.graphql_request(op, query, &vars)?;
        let raw = self.send(kind, req).await?;
        self.check_status(&raw, kind)?;
        let v: Value = serde_json::from_slice(&raw.body).map_err(|_| {
            let msg = "response wasn't JSON (captive portal or proxy?)".to_string();
            match kind {
                OpKind::Query => GhError::Offline(msg),
                OpKind::Mutation => GhError::Ambiguous(msg),
            }
        })?;
        if let Some(first) = v.get("errors").and_then(|e| e.as_array()).and_then(|a| a.first()) {
            return Err(self.graphql_error(first, kind, &raw.headers));
        }
        match v.get("data") {
            Some(d) if !d.is_null() => Ok((d.clone(), raw.headers)),
            _ => Err(GhError::Protocol(format!("{op}: response had no data"))),
        }
    }

    pub async fn graphql<T: DeserializeOwned>(
        &self,
        op: &str,
        query: &str,
        vars: Value,
        kind: OpKind,
    ) -> Result<T, GhError> {
        let (data, _) = self.graphql_raw(op, query, vars, kind).await?;
        serde_json::from_value(data).map_err(|e| GhError::Protocol(format!("{op}: {e}")))
    }

    fn graphql_error(&self, err: &Value, kind: OpKind, headers: &HeaderMap) -> GhError {
        let ty = err.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
        match ty {
            "NOT_FOUND" => GhError::NotFound,
            "FORBIDDEN" | "INSUFFICIENT_SCOPES" => {
                GhError::Forbidden { message: msg, sso_url: sso_url(headers) }
            }
            "RATE_LIMITED" => {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                let wait = header_u64(headers, "x-ratelimit-reset")
                    .map(|r| r.saturating_sub(now))
                    .unwrap_or(60)
                    .max(1);
                self.limits.lock().unwrap().blocked_until = Some(Instant::now() + Duration::from_secs(wait));
                GhError::RateLimited { retry_after_s: wait, secondary: false }
            }
            "UNPROCESSABLE" => GhError::Unprocessable(msg),
            // GitHub reports its own timeouts as "Something went wrong".
            _ if msg.contains("Something went wrong") || msg.to_lowercase().contains("timeout") => match kind
            {
                OpKind::Mutation => GhError::Ambiguous(msg),
                OpKind::Query => GhError::Server(502),
            },
            // A mutation error without a type is GitHub refusing the input.
            _ if kind == OpKind::Mutation => GhError::Unprocessable(msg),
            _ => GhError::Protocol(msg),
        }
    }

    fn rest(&self, path: &str) -> Result<RequestBuilder, GhError> {
        Ok(self
            .http
            .get(format!("{}/{}", self.cfg.api_base, path.trim_start_matches('/')))
            .bearer_auth(self.token()?)
            .header("X-GitHub-Api-Version", "2022-11-28"))
    }

    /// GET a REST path returning JSON. With `use_etag`, sends the cached ETag
    /// and returns the cached body on 304 (which costs no rate limit).
    pub async fn rest_get_json<T: DeserializeOwned>(&self, path: &str, use_etag: bool) -> Result<T, GhError> {
        let cached = if use_etag { self.etag_lookup(path) } else { None };
        let mut req = self.rest(path)?.header("Accept", "application/vnd.github+json");
        if let Some((etag, _)) = &cached {
            req = req.header("If-None-Match", etag.as_str());
        }
        let raw = self.send(OpKind::Query, req).await?;
        self.check_status(&raw, OpKind::Query)?;
        let body = if raw.status == StatusCode::NOT_MODIFIED {
            match cached {
                Some((_, body)) => body,
                None => {
                    return Err(GhError::Protocol(format!("{path}: 304 without a cached body")));
                }
            }
        } else {
            if use_etag && let Some(etag) = raw.headers.get("etag").and_then(|v| v.to_str().ok()) {
                self.etag_store(path, etag, &raw.body);
            }
            raw.body
        };
        serde_json::from_slice(&body)
            .map_err(|_| GhError::Offline(format!("{path}: response wasn't JSON (captive portal or proxy?)")))
    }

    /// GET every page of a REST list endpoint, using `page`/`per_page`.
    pub async fn rest_get_pages<T: DeserializeOwned>(
        &self,
        path: &str,
        per_page: usize,
        max_pages: usize,
        use_etag: bool,
    ) -> Result<Vec<T>, GhError> {
        let sep = if path.contains('?') { '&' } else { '?' };
        let mut out = Vec::new();
        for page in 1..=max_pages {
            let items: Vec<T> =
                self.rest_get_json(&format!("{path}{sep}per_page={per_page}&page={page}"), use_etag).await?;
            let n = items.len();
            out.extend(items);
            if n < per_page {
                break;
            }
        }
        Ok(out)
    }

    /// GET a REST path returning raw bytes (for example a git blob with
    /// `Accept: application/vnd.github.raw+json`).
    pub async fn rest_get_bytes(&self, path: &str, accept: &str) -> Result<Vec<u8>, GhError> {
        let req = self.rest(path)?.header("Accept", accept);
        let raw = self.send(OpKind::Query, req).await?;
        self.check_status(&raw, OpKind::Query)?;
        Ok(raw.body)
    }

    /// Downloads a public or pre-signed URL (images in PR bodies). Never sends
    /// the token: these URLs point at other hosts.
    pub async fn download(&self, url: &str, max_bytes: usize) -> Result<(Vec<u8>, Option<String>), GhError> {
        self.gate()?;
        let _permit = self.permits.acquire().await.expect("semaphore closed");
        let mut resp = self
            .http
            .get(url)
            .timeout(self.cfg.request_timeout)
            .send()
            .await
            .map_err(|e| classify_transport(&e, OpKind::Query, false))?;
        if !resp.status().is_success() {
            return Err(if resp.status() == StatusCode::NOT_FOUND {
                GhError::NotFound
            } else {
                GhError::Protocol(format!("HTTP {} for {url}", resp.status()))
            });
        }
        if resp.content_length().is_some_and(|n| n as usize > max_bytes) {
            return Err(GhError::Protocol(format!("{url} is larger than {max_bytes} bytes")));
        }
        let content_type =
            resp.headers().get("content-type").and_then(|v| v.to_str().ok()).map(str::to_owned);
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| classify_transport(&e, OpKind::Query, true))? {
            body.extend_from_slice(&chunk);
            if body.len() > max_bytes {
                return Err(GhError::Protocol(format!("{url} is larger than {max_bytes} bytes")));
            }
        }
        Ok((body, content_type))
    }

    /// Checks connectivity and the token with `GET /rate_limit`, which doesn't
    /// count against the rate limit. A captive portal fails the JSON check.
    pub async fn probe(&self) -> Result<RateBudget, GhError> {
        let v: Value = self.rest_get_json("rate_limit", false).await?;
        if v.get("resources").is_none() {
            return Err(GhError::Offline("unexpected /rate_limit response".into()));
        }
        Ok(self.budget())
    }

    /// The signed-in user and, for classic tokens, their scopes.
    pub async fn viewer(&self) -> Result<Viewer, GhError> {
        let (data, headers) =
            self.graphql_raw("Viewer", super::queries::VIEWER, json!({}), OpKind::Query).await?;
        let scopes = headers
            .get("x-oauth-scopes")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect());
        let v = &data["viewer"];
        Ok(Viewer {
            node_id: v["id"].as_str().unwrap_or_default().to_string(),
            login: v["login"].as_str().unwrap_or_default().to_string(),
            scopes,
        })
    }

    fn etag_lookup(&self, key: &str) -> Option<(String, Vec<u8>)> {
        use rusqlite::OptionalExtension;
        let db = self.etag_db.as_ref()?;
        db.read(|c| {
            Ok(c.query_row("SELECT etag, body FROM http_cache WHERE url_key = ?1", [key], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?)
        })
        .ok()
        .flatten()
    }

    fn etag_store(&self, key: &str, etag: &str, body: &[u8]) {
        let Some(db) = self.etag_db.as_ref() else {
            return;
        };
        let now = crate::clock::rfc3339(SystemTime::now());
        let res = db.write(|tx| {
            tx.execute(
                "INSERT INTO http_cache (url_key, etag, body, stored_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (url_key) DO UPDATE SET etag = excluded.etag, body = excluded.body,
                   stored_at = excluded.stored_at",
                rusqlite::params![key, etag, body, now],
            )?;
            Ok(())
        });
        if let Err(e) = res {
            tracing::warn!("failed to store ETag for {key}: {e}");
        }
    }

    /// Forget cached responses (for example after switching accounts).
    pub fn clear_etags(&self) {
        if let Some(db) = &self.etag_db {
            let _ = db.write(|tx| {
                tx.execute("DELETE FROM http_cache", [])?;
                Ok(())
            });
        }
    }
}

#[derive(Clone, Debug)]
pub struct Viewer {
    pub node_id: String,
    pub login: String,
    pub scopes: Option<Vec<String>>,
}

fn classify_transport(e: &reqwest::Error, kind: OpKind, response_started: bool) -> GhError {
    let msg = e.to_string();
    if e.is_connect() && !response_started {
        return GhError::Offline(msg);
    }
    match kind {
        OpKind::Query => GhError::Offline(msg),
        // Anything after the connection was made may have reached GitHub.
        OpKind::Mutation => GhError::Ambiguous(msg),
    }
}

fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(HeaderName::from_bytes(name.as_bytes()).ok()?)?.to_str().ok()?.trim().parse().ok()
}

/// `X-GitHub-SSO: required; url=https://github.com/orgs/acme/sso?authorization_request=…`
fn sso_url(headers: &HeaderMap) -> Option<String> {
    let v: &HeaderValue = headers.get("x-github-sso")?;
    let s = v.to_str().ok()?;
    s.split(';').map(str::trim).find_map(|p| p.strip_prefix("url=")).map(str::to_owned)
}

fn error_message(body: &[u8]) -> String {
    match serde_json::from_slice::<Value>(body) {
        Ok(v) => {
            let mut msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
            if let Some(errors) = v.get("errors").and_then(|e| e.as_array()) {
                for e in errors {
                    let detail = e
                        .get("message")
                        .and_then(|m| m.as_str())
                        .map(str::to_owned)
                        .unwrap_or_else(|| e.to_string());
                    msg.push_str(&format!(" ({detail})"));
                }
            }
            msg
        }
        Err(_) => String::from_utf8_lossy(body).chars().take(200).collect(),
    }
}

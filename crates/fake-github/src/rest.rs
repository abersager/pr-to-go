//! REST endpoints: rate limit, PR files, compare, raw blobs, images.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use sha1::{Digest, Sha1};
use std::collections::HashMap;

use crate::{Shared, apply_fault_after, apply_fault_before, authed, not_found, rate_headers};

fn etagged(headers: &HeaderMap, body: String) -> Response {
    let etag = format!("\"{}\"", hex::encode(Sha1::digest(body.as_bytes())));
    if headers.get("if-none-match").and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        return rate_headers((StatusCode::NOT_MODIFIED, [("etag", etag)]).into_response());
    }
    rate_headers(([("etag", etag), ("content-type", "application/json".into())], body).into_response())
}

pub async fn rate_limit(State(w): State<Shared>, headers: HeaderMap) -> Response {
    if authed(&w, &headers).is_none() {
        return crate::unauthorized();
    }
    let fault = w.lock().unwrap().take_fault("rest:rate_limit");
    if let Some(r) = apply_fault_before(&w, &fault).await {
        return r;
    }
    let body = json!({
        "resources": { "core": { "limit": 5000, "remaining": 4999 }, "graphql": { "limit": 5000, "remaining": 4999 } },
        "rate": { "limit": 5000, "remaining": 4999 },
    });
    apply_fault_after(fault, rate_headers(axum::Json(body).into_response())).await
}

pub async fn pr_files(
    State(w): State<Shared>,
    Path((owner, repo, number)): Path<(String, String, u64)>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if authed(&w, &headers).is_none() {
        return crate::unauthorized();
    }
    let fault = {
        let mut g = w.lock().unwrap();
        g.log.push("rest:files".into());
        g.take_fault("rest:files")
    };
    if let Some(r) = apply_fault_before(&w, &fault).await {
        return r;
    }
    let full = format!("{owner}/{repo}");
    let body = {
        let mut g = w.lock().unwrap();
        if !g.repos.get(&full).is_some_and(|r| r.prs.contains_key(&number)) {
            return not_found();
        }
        let files = g.pr_files(&full, number);
        let per_page: usize = q.get("per_page").and_then(|v| v.parse().ok()).unwrap_or(30);
        let page: usize = q.get("page").and_then(|v| v.parse().ok()).unwrap_or(1);
        let items: Vec<_> = files
            .iter()
            .skip((page - 1) * per_page)
            .take(per_page)
            .map(|f| {
                json!({
                    "sha": f.head_blob.clone().or(f.base_blob.clone()),
                    "filename": f.path,
                    "status": f.status,
                    "additions": f.additions,
                    "deletions": f.deletions,
                    "changes": f.additions + f.deletions,
                    "patch": f.patch,
                    "previous_filename": f.prev_path,
                })
            })
            .collect();
        serde_json::to_string(&items).unwrap()
    };
    apply_fault_after(fault, etagged(&headers, body)).await
}

pub async fn compare(
    State(w): State<Shared>,
    Path((owner, repo, basehead)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    if authed(&w, &headers).is_none() {
        return crate::unauthorized();
    }
    let fault = {
        let mut g = w.lock().unwrap();
        g.log.push("rest:compare".into());
        g.take_fault("rest:compare")
    };
    if let Some(r) = apply_fault_before(&w, &fault).await {
        return r;
    }
    let body = {
        let g = w.lock().unwrap();
        let Some(r) = g.repos.get(&format!("{owner}/{repo}")) else {
            return not_found();
        };
        let Some((base, head)) = basehead.split_once("...") else {
            return not_found();
        };
        let Some(mb) = r.merge_base(base, head) else {
            return not_found();
        };
        json!({ "merge_base_commit": { "sha": mb }, "status": "ahead", "files": [] }).to_string()
    };
    apply_fault_after(fault, etagged(&headers, body)).await
}

pub async fn git_blob(
    State(w): State<Shared>,
    Path((owner, repo, sha)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    if authed(&w, &headers).is_none() {
        return crate::unauthorized();
    }
    let fault = {
        let mut g = w.lock().unwrap();
        g.log.push("rest:blob".into());
        g.take_fault("rest:blob")
    };
    if let Some(r) = apply_fault_before(&w, &fault).await {
        return r;
    }
    let bytes = {
        let g = w.lock().unwrap();
        match g.repos.get(&format!("{owner}/{repo}")).and_then(|r| r.blobs.get(&sha)) {
            Some(b) => b.clone(),
            None => return not_found(),
        }
    };
    apply_fault_after(fault, rate_headers(Response::new(Body::from(bytes)))).await
}

pub async fn asset(State(w): State<Shared>, Path(path): Path<String>) -> Response {
    let fault = w.lock().unwrap().take_fault("rest:asset");
    if let Some(r) = apply_fault_before(&w, &fault).await {
        return r;
    }
    let found = w.lock().unwrap().assets.get(&path).cloned();
    match found {
        Some((ct, bytes)) => ([("content-type", ct)], bytes).into_response(),
        None => not_found(),
    }
}

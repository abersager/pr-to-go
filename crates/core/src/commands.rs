//! The UI's API as one entry point: `dispatch(command, args)`. The Tauri
//! shell passes calls straight through; the dev server exposes the same API
//! over HTTP so the UI can be driven in a browser (and in E2E tests).

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::github::GhError;
use crate::service::Core;

fn args<T: DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|e| Error::Invalid(format!("bad arguments: {e}")))
}

fn out<T: Serialize>(v: T) -> Result<Value> {
    Ok(serde_json::to_value(v)?)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrId {
    pr_id: i64,
}

impl Error {
    /// A coarse kind the UI branches on.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::GitHub(GhError::Offline(_)) => "offline",
            Error::GitHub(GhError::RateLimited { .. }) => "rateLimited",
            Error::GitHub(GhError::Unauthorized | GhError::NoToken) | Error::NotAuthenticated => "auth",
            Error::GitHub(GhError::NotFound) | Error::NotFound(_) => "notFound",
            Error::Invalid(_) => "invalid",
            _ => "other",
        }
    }
}

/// An error as the UI sees it.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UiError {
    pub message: String,
    pub kind: &'static str,
}

impl From<Error> for UiError {
    fn from(e: Error) -> Self {
        UiError { kind: e.kind(), message: e.to_string() }
    }
}

impl Core {
    pub async fn dispatch(&self, cmd: &str, a: Value) -> Result<Value> {
        match cmd {
            "auth_status" => out(self.auth_status()?),
            "sign_in" => {
                #[derive(serde::Deserialize)]
                struct A {
                    token: String,
                }
                let a: A = args(a)?;
                out(self.sign_in(&a.token, "pat").await?)
            }
            "sign_in_with_gh" => out(self.sign_in_with_gh().await?),
            "sign_out" => out(self.sign_out()?),
            "connectivity" => out(self.connectivity()),
            "check_connectivity" => out(self.check_connectivity().await),
            "set_work_offline" => {
                #[derive(serde::Deserialize)]
                struct A {
                    offline: bool,
                }
                let a: A = args(a)?;
                out(self.set_work_offline(a.offline)?)
            }
            "add_pr" => {
                #[derive(serde::Deserialize)]
                struct A {
                    input: String,
                }
                let a: A = args(a)?;
                out(self.add_pr(&a.input).await?)
            }
            "sync_pr" => out(self.sync_pr(args::<PrId>(a)?.pr_id).await?),
            "list_prs" => out(self.list_prs()?),
            "get_pr" => out(self.get_pr(args::<PrId>(a)?.pr_id)?),
            "file_diff" => {
                #[derive(serde::Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct A {
                    revision_id: i64,
                    path: String,
                }
                let a: A = args(a)?;
                out(self.file_diff(a.revision_id, &a.path)?)
            }
            "mark_seen" => out(self.mark_seen(args::<PrId>(a)?.pr_id)?),
            "set_file_viewed" => {
                #[derive(serde::Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct A {
                    pr_id: i64,
                    path: String,
                    head_blob_oid: Option<String>,
                    viewed: bool,
                }
                let a: A = args(a)?;
                out(self.set_file_viewed(a.pr_id, &a.path, a.head_blob_oid.as_deref(), a.viewed)?)
            }
            _ => Err(Error::Invalid(format!("unknown command {cmd}"))),
        }
    }

    /// Serves `asset/<sha256>` and `blob/<oid>?type=<mime>` for the UI's
    /// local URLs. Returns bytes and a content type.
    pub fn local_resource(&self, path: &str, query: Option<&str>) -> Option<(Vec<u8>, String)> {
        let path = path.trim_start_matches('/');
        if let Some(sha) = path.strip_prefix("asset/") {
            return self.asset(sha).ok().flatten();
        }
        let oid = path.strip_prefix("blob/")?;
        let ct = query
            .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("type=")))
            .map(|t| t.replace("%2F", "/"))
            .filter(|t| t.starts_with("image/"))
            .unwrap_or_else(|| "application/octet-stream".into());
        self.blob(oid).ok().flatten().map(|b| (b, ct))
    }
}

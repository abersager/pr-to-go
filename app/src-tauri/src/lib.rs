//! The Tauri shell: passes UI calls through to the core, forwards core
//! events, and serves cached images and file contents over the `prtg://`
//! scheme. The GitHub token never crosses into the webview.

use std::path::PathBuf;
use std::sync::Arc;

use pr_to_go_core::auth::{MemorySecretStore, SecretStore};
use pr_to_go_core::commands::UiError;
use pr_to_go_core::{Core, CoreOptions};
use serde_json::Value;
use tauri::http::{Response, StatusCode, header};
use tauri::{AppHandle, Emitter, Manager, State};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{self, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::writer::MakeWriterExt;

#[cfg(all(target_os = "macos", debug_assertions))]
mod dev_snapshot;

struct AppState(Arc<Core>);

/// Keeps the log writer's background thread alive (and flushes on exit).
struct LogGuard(#[allow(dead_code)] WorkerGuard);

/// Every UI call goes through here; see `pr_to_go_core::commands`.
#[tauri::command]
async fn core(s: State<'_, AppState>, cmd: String, args: Value) -> Result<Value, UiError> {
    Ok(s.0.dispatch(&cmd, args).await?)
}

/// Shows the log folder in Finder / Explorer / the file manager.
#[tauri::command]
fn reveal_logs(app: AppHandle) -> Result<(), String> {
    let dir = app.path().app_log_dir().map_err(|e| e.to_string())?;
    tauri_plugin_opener::reveal_item_in_dir(dir).map_err(|e| e.to_string())
}

/// Logs go to the platform's log folder (`~/Library/Logs/com.abersager.prtogo`
/// on macOS), one file a day, a week kept. `PRTOGO_LOG` takes a filter such as
/// `debug` or `pr_to_go_core=trace`. Requests are logged without headers or
/// bodies, so the token never reaches the log.
fn init_logging(app: &tauri::App) -> Option<WorkerGuard> {
    let dir = app.path().app_log_dir().ok()?;
    // The appender prunes old files at start and complains if there's no folder.
    std::fs::create_dir_all(&dir).ok()?;
    let files = rolling::Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("pr-to-go")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(files);
    let filter = EnvFilter::try_from_env("PRTOGO_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,pr_to_go_core=debug,pr_to_go_app=debug"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(writer.and(std::io::stderr.with_filter(|_| cfg!(debug_assertions))))
        .try_init()
        .ok()?;
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        default_hook(info);
    }));
    tracing::info!("PR to Go {} starting", env!("CARGO_PKG_VERSION"));
    Some(guard)
}

/// Development overrides, honoured only in debug builds: in a release build
/// `PRTOGO_API_BASE` could send the token somewhere else.
fn dev_env(name: &str) -> Option<String> {
    if cfg!(debug_assertions) { std::env::var(name).ok() } else { None }
}

fn secret_store() -> Arc<dyn SecretStore> {
    // `PRTOGO_TOKEN` keeps the token in memory, so a dev run against a fake
    // GitHub never touches the real keychain.
    if let Some(token) = dev_env("PRTOGO_TOKEN") {
        let s = MemorySecretStore::default();
        let _ = s.set(&token);
        return Arc::new(s);
    }
    match pr_to_go_core::auth::KeychainSecretStore::new("com.abersager.prtogo") {
        Ok(k) => Arc::new(k),
        Err(e) => {
            tracing::error!("keychain unavailable, the token won't be remembered: {e}");
            Arc::new(MemorySecretStore::default())
        }
    }
}

fn options(app: &tauri::App) -> tauri::Result<CoreOptions> {
    // `PRTOGO_DATA_DIR` and `PRTOGO_API_BASE` point a dev run somewhere safe.
    let data_dir = match dev_env("PRTOGO_DATA_DIR") {
        Some(d) => PathBuf::from(d),
        None => app.path().app_data_dir()?,
    };
    let mut opts = CoreOptions::new(data_dir, secret_store());
    if let Some(base) = dev_env("PRTOGO_API_BASE") {
        opts.github.api_base = base;
    }
    Ok(opts)
}

fn serve(core: &Core, uri: &tauri::http::Uri) -> Response<Vec<u8>> {
    match core.local_resource(uri.path(), uri.query()) {
        Some((bytes, ct)) => Response::builder()
            .header(header::CONTENT_TYPE, ct)
            .header(header::CACHE_CONTROL, "max-age=31536000, immutable")
            .body(bytes)
            .unwrap(),
        None => Response::builder().status(StatusCode::NOT_FOUND).body(Vec::new()).unwrap(),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .register_asynchronous_uri_scheme_protocol("prtg", |ctx, request, responder| {
            let core = ctx.app_handle().state::<AppState>().0.clone();
            let uri = request.uri().clone();
            tauri::async_runtime::spawn_blocking(move || responder.respond(serve(&core, &uri)));
        })
        .setup(|app| {
            if let Some(guard) = init_logging(app) {
                app.manage(LogGuard(guard));
            }
            let core = Core::open(options(app)?)?;
            let handle = app.handle().clone();
            let mut events = core.subscribe();
            tauri::async_runtime::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(e) => {
                            let _ = handle.emit("core-event", e);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
            });
            tauri::async_runtime::spawn(core.clone().run_background());
            let startup = core.clone();
            tauri::async_runtime::spawn(async move {
                if let Some(token) = dev_env("PRTOGO_TOKEN")
                    && !startup.auth_status().map(|s| s.signed_in).unwrap_or(false)
                {
                    let _ = startup.sign_in(&token, "pat").await;
                }
                startup.check_connectivity().await;
            });
            app.manage(AppState(core));
            #[cfg(all(target_os = "macos", debug_assertions))]
            dev_snapshot::schedule(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![core, reveal_logs])
        .run(tauri::generate_context!())
        .expect("error while running PR to Go");
}

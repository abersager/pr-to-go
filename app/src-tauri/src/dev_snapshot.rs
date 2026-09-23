//! Debug builds on macOS only. Writes PNG snapshots of the webview, so native
//! rendering can be checked without screen-recording permission:
//!
//! `PRTOGO_SNAPSHOT_DIR=/tmp/shots PRTOGO_SNAPSHOTS="files=#/pr/1/files;conv=#/pr/1"`
//!
//! Each entry sets the route, waits for it to render, and saves `<name>.png`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tauri::Manager;

pub fn schedule(app: &tauri::AppHandle) {
    let (Ok(spec), Ok(dir)) = (std::env::var("PRTOGO_SNAPSHOTS"), std::env::var("PRTOGO_SNAPSHOT_DIR"))
    else {
        return;
    };
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(6));
        for item in spec.split(';') {
            let Some((name, route)) = item.split_once('=') else { continue };
            let Some(win) = app.get_webview_window("main") else { return };
            let _ = win.eval(format!("location.hash = {route:?}"));
            std::thread::sleep(Duration::from_secs(3));
            let path = Path::new(&dir).join(format!("{name}.png"));
            let _ = win.with_webview(move |wv| snapshot(wv.inner(), path));
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

fn snapshot(webview: *mut std::ffi::c_void, path: PathBuf) {
    use block2::RcBlock;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
    use objc2_foundation::{NSDictionary, NSError};
    use objc2_web_kit::WKWebView;

    // SAFETY: on macOS, wry's platform webview is a WKWebView, and
    // `with_webview` runs this on the main thread.
    let wk = unsafe { &*(webview as *const WKWebView) };
    let done = RcBlock::new(move |image: *mut NSImage, _error: *mut NSError| {
        // SAFETY: WebKit passes a valid image or null.
        let Some(image) = (unsafe { image.as_ref() }) else { return };
        let Some(tiff) = image.TIFFRepresentation() else { return };
        let Some(rep) = NSBitmapImageRep::imageRepWithData(&tiff) else { return };
        let props = NSDictionary::new();
        // SAFETY: an empty properties dictionary is valid for PNG.
        let Some(png) =
            (unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) })
        else {
            return;
        };
        let _ = std::fs::write(&path, png.to_vec());
    });
    // SAFETY: a nil configuration snapshots the visible viewport.
    unsafe { wk.takeSnapshotWithConfiguration_completionHandler(None, &done) };
}

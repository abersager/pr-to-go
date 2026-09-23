//! Read-only smoke test against the real GitHub API. Ignored by default:
//! `GITHUB_TOKEN=$(gh auth token) cargo test --test live -- --ignored --nocapture`
//! Optionally set `LIVE_PRS="owner/repo#1 owner/repo#2"`.

use std::sync::Arc;

use pr_to_go_core::auth::{MemorySecretStore, SecretStore};
use pr_to_go_core::{Core, CoreOptions};

#[tokio::test]
#[ignore]
async fn syncs_real_pull_requests() {
    let token = std::env::var("GITHUB_TOKEN").expect("set GITHUB_TOKEN");
    let prs = std::env::var("LIVE_PRS").unwrap_or_else(|_| "hubtty/hubtty#153 danobi/prr#79".into());
    let dir = tempfile::tempdir().unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
    let core = Core::open(CoreOptions::new(dir.path().to_owned(), secrets)).unwrap();
    let status = core.sign_in(&token, "pat").await.unwrap();
    println!("signed in as {:?} scopes {:?}", status.login, status.scopes);
    for pr in prs.split_whitespace() {
        let id = core.add_pr(pr).await.unwrap_or_else(|e| panic!("{pr}: {e}"));
        let d = core.get_pr(id).unwrap();
        let rev = d.revision.as_ref().unwrap();
        println!(
            "{pr}: {:?} {} files, {} commits, {} reviews, {} threads, {} issue comments, checks {:?}, partial {:?}",
            d.summary.sync_state,
            d.files.len(),
            d.commits.len(),
            d.reviews.len(),
            d.threads.len(),
            d.issue_comments.len(),
            d.checks.as_ref().and_then(|c| c.rollup_state.clone()),
            rev.partial_reasons,
        );
        let local = d.body_html.matches("prtg://localhost/asset/").count();
        let remote: Vec<&str> = d
            .body_html
            .split("<img")
            .skip(1)
            .filter_map(|t| t.split("src=\"").nth(1)?.split('"').next())
            .filter(|src| !src.starts_with("prtg://"))
            .collect();
        println!("  images: {local} cached, not cached: {remote:?}");
        for f in &d.files {
            let fd = core.file_diff(rev.id, &f.path).unwrap();
            assert!(
                fd.base_text.is_some()
                    || fd.head_text.is_some()
                    || fd.base_binary
                    || fd.head_binary
                    || f.content_status != "ok",
                "{}: no contents",
                f.path
            );
        }
        // A second sync must reuse everything.
        let again = core.sync_pr(id).await.unwrap();
        assert_eq!(again.revision_id, rev.id);
    }
}

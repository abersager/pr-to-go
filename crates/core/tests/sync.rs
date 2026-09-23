//! Sync against the fake GitHub (DESIGN.md §9.2, §12).

mod common;

use common::*;
use fake_github::FaultAction;
use pr_to_go_core::Error;
use pr_to_go_core::diff::LineKind;
use pr_to_go_core::github::GhError;

#[tokio::test]
async fn syncs_everything_needed_to_review_offline() {
    let h = harness().await;
    let s = h.fake.with(seed);
    let pr_id = h.core.add_pr(&pr_url(s.number)).await.unwrap();

    // Now take GitHub away entirely.
    drop(h.fake);

    let pr = h.core.get_pr(pr_id).unwrap();
    assert_eq!(pr.summary.title, "Improve widgets");
    assert_eq!(pr.summary.sync_state, "ready");
    assert!(pr.summary.pinned);
    let rev = pr.revision.as_ref().unwrap();
    assert_eq!(rev.head_oid, s.head);
    assert_eq!(rev.merge_base_oid, s.base);
    assert_eq!(rev.status, "complete");

    let kinds: Vec<(&str, &str)> =
        pr.files.iter().map(|f| (f.path.as_str(), f.change_type.as_str())).collect();
    assert_eq!(
        kinds,
        [
            ("CHANGELOG", "removed"),
            ("logo.png", "modified"),
            ("new/name.txt", "renamed"),
            ("src/lib.rs", "modified"),
            ("src/new.rs", "added"),
        ]
    );
    assert_eq!(pr.commits.len(), 1);
    assert_eq!(pr.commits[0].headline, "Improve widgets");
    assert_eq!(pr.reviews.len(), 1);
    assert_eq!(pr.issue_comments.len(), 1);
    assert_eq!(pr.threads.len(), 1);
    assert_eq!(pr.threads[0].line, Some(5));
    assert!(pr.threads[0].comments[0].diff_hunk.as_deref().unwrap().starts_with("@@"));
    assert_eq!(pr.checks.as_ref().unwrap().rollup_state.as_deref(), Some("SUCCESS"));

    // The description image was downloaded and rewritten to a local URL.
    assert!(pr.body_html.contains("prtg://localhost/asset/"), "{}", pr.body_html);
    let sha = pr.body_html.split("prtg://localhost/asset/").nth(1).unwrap()[..64].to_string();
    let (bytes, ct) = h.core.asset(&sha).unwrap().unwrap();
    assert_eq!((bytes.as_slice(), ct.as_str()), (PNG_V1, "image/png"));

    // Diffs, with full contents for expanding context.
    let rid = rev.id;
    let lib = h.core.file_diff(rid, "src/lib.rs").unwrap();
    assert_eq!(lib.source, "github");
    assert!(lib.commentable);
    assert_eq!(lib.base_text.as_deref(), Some(numbered(30).as_str()));
    assert!(lib.head_text.as_deref().unwrap().contains("line five (edited)"));
    let added: Vec<&str> = lib
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind == LineKind::Add)
        .map(|l| l.text.as_str())
        .collect();
    assert_eq!(added, ["line five (edited)", "line 20a (new)", "line 20b (new)"]);

    let renamed = h.core.file_diff(rid, "new/name.txt").unwrap();
    assert_eq!(renamed.prev_path.as_deref(), Some("old/name.txt"));
    assert!(renamed.hunks.is_empty());
    assert_eq!(renamed.base_text, renamed.head_text);

    let logo = h.core.file_diff(rid, "logo.png").unwrap();
    assert_eq!(logo.patch_status, "binary");
    assert!(logo.base_binary && logo.head_binary);
    assert_eq!(h.core.blob(logo.head_blob_oid.as_deref().unwrap()).unwrap().unwrap(), PNG_V2);

    let removed = h.core.file_diff(rid, "CHANGELOG").unwrap();
    assert_eq!(removed.base_text.as_deref(), Some("v1\n"));
    assert_eq!(removed.head_text, None);

    // Syncing now fails as offline, and the PR stays readable.
    let err = h.core.sync_pr(pr_id).await.unwrap_err();
    assert!(matches!(err, Error::GitHub(GhError::Offline(_))), "{err:?}");
    let pr = h.core.get_pr(pr_id).unwrap();
    assert_eq!(pr.summary.sync_state, "stale");
    assert_eq!(pr.revision.unwrap().id, rid);
}

#[tokio::test]
async fn resync_without_changes_fetches_no_files_or_blobs() {
    let h = harness().await;
    let s = h.fake.with(seed);
    let pr_id = h.core.add_pr(&pr_url(s.number)).await.unwrap();
    let rid = h.core.get_pr(pr_id).unwrap().revision.unwrap().id;
    h.fake.clear_log();
    let out = h.core.sync_pr(pr_id).await.unwrap();
    assert_eq!(out.revision_id, rid);
    assert!(!out.head_moved);
    let log = h.fake.log();
    assert!(!log.iter().any(|l| l == "Blobs" || l == "rest:files" || l == "rest:compare"), "{log:?}");
}

#[tokio::test]
async fn new_push_creates_a_new_revision_and_keeps_the_old_one() {
    let h = harness().await;
    let s = h.fake.with(seed);
    let pr_id = h.core.add_pr(&pr_url(s.number)).await.unwrap();
    let old = h.core.get_pr(pr_id).unwrap().revision.unwrap();

    // Force-push: a new commit straight on top of main, dropping the old one.
    let new_head = h.fake.with(|w| {
        let lib = replace_line(&numbered(30), 12, "line twelve (rewritten)");
        let c = w.commit(REPO, Some(&s.base), &[("src/lib.rs", Some(&lib))], "Rewrite");
        w.push(REPO, s.number, &c);
        c
    });
    h.fake.clear_log();
    let out = h.core.sync_pr(pr_id).await.unwrap();
    assert!(out.head_moved);
    assert_ne!(out.revision_id, old.id);
    let pr = h.core.get_pr(pr_id).unwrap();
    let rev = pr.revision.unwrap();
    assert_eq!(rev.head_oid, new_head);
    assert_eq!(rev.is_force_push, Some(true));
    assert_eq!(pr.files.len(), 1);
    // The unchanged base blob wasn't fetched again: only the new head blob.
    let blob_queries = h.fake.log().iter().filter(|l| *l == "Blobs").count();
    assert_eq!(blob_queries, 1);
    // Bob's thread was on line 5, which the force-push reverted: outdated now.
    assert!(pr.threads[0].is_outdated);
    // The old revision is still there with its files.
    let old_diff = h.core.file_diff(old.id, "src/lib.rs").unwrap();
    assert!(old_diff.head_text.unwrap().contains("line five (edited)"));
}

#[tokio::test]
async fn paginates_every_connection() {
    let h = harness().await;
    let s = h.fake.with(|w| {
        let s = seed(w);
        w.page_size = Some(1);
        for i in 0..3 {
            w.add_review(
                REPO,
                s.number,
                "alice",
                "COMMENTED",
                &format!("r{i}"),
                &[("src/new.rs", "RIGHT", 1, "c")],
            );
            w.add_issue_comment(REPO, s.number, "alice", &format!("i{i}"));
        }
        // A second commit, so commits paginate too.
        let c = w.commit(REPO, Some(&s.head), &[("src/new.rs", Some("pub fn new() { todo!() }\n"))], "More");
        w.push(REPO, s.number, &c);
        // A thread with three comments.
        let pr = w.pr(REPO, s.number);
        let mut extra = pr.threads[0].comments[0].clone();
        for n in 0..2 {
            extra.node_id = format!("PRRC_extra{n}");
            pr.threads[0].comments.push(extra.clone());
        }
        s
    });
    let pr_id = h.core.add_pr(&pr_url(s.number)).await.unwrap();
    let pr = h.core.get_pr(pr_id).unwrap();
    assert_eq!(pr.reviews.len(), 4);
    assert_eq!(pr.issue_comments.len(), 4);
    assert_eq!(pr.threads.len(), 4);
    assert_eq!(pr.commits.len(), 2);
    let biggest = pr.threads.iter().map(|t| t.comments.len()).max().unwrap();
    assert_eq!(biggest, 3);
}

#[tokio::test]
async fn non_utf8_and_truncated_contents_are_fetched_raw() {
    let h = harness().await;
    let number = h.fake.with(|w| {
        w.create_repo(REPO);
        let base = w.commit(REPO, None, &[("a.txt", Some("a\n"))], "base");
        w.set_branch(REPO, "main", &base);
        let big = "x".repeat(300) + "\n";
        let head = w.commit_bytes(
            REPO,
            Some(&base),
            &[("latin1.txt", Some(b"caf\xe9\n".as_slice())), ("big.txt", Some(big.as_bytes()))],
            "add",
        );
        w.blob_text_limit = 100;
        w.open_pr(REPO, "main", "f", &head, "t", "", "alice")
    });
    let pr_id = h.core.add_pr(&pr_url(number)).await.unwrap();
    assert_eq!(h.fake.log().iter().filter(|l| *l == "rest:blob").count(), 2);
    let rid = h.core.get_pr(pr_id).unwrap().revision.unwrap().id;
    let latin = h.core.file_diff(rid, "latin1.txt").unwrap();
    let bytes = h.core.blob(latin.head_blob_oid.as_deref().unwrap()).unwrap().unwrap();
    assert_eq!(bytes, b"caf\xe9\n");
    let big = h.core.file_diff(rid, "big.txt").unwrap();
    assert_eq!(big.head_text.unwrap().len(), 301);
}

#[tokio::test]
async fn captive_portal_counts_as_offline() {
    let h = harness().await;
    let s = h.fake.with(seed);
    h.fake.with(|w| w.fault("PullRequestDetails", FaultAction::CaptivePortal));
    let err = h.core.add_pr(&pr_url(s.number)).await.unwrap_err();
    assert!(matches!(err, Error::GitHub(GhError::Offline(_))), "{err:?}");
    assert!(!h.core.connectivity().online);
}

#[tokio::test]
async fn missing_patch_is_partial_with_a_view_only_local_diff() {
    let h = harness().await;
    let s = h.fake.with(|w| {
        let s = seed(w);
        w.patch_limit = 60;
        s
    });
    let pr_id = h.core.add_pr(&pr_url(s.number)).await.unwrap();
    let pr = h.core.get_pr(pr_id).unwrap();
    assert_eq!(pr.summary.sync_state, "partial");
    let rev = pr.revision.unwrap();
    assert_eq!(rev.status, "partial");
    assert!(rev.partial_reasons.iter().any(|r| r.path == "src/lib.rs"));
    let lib = h.core.file_diff(rev.id, "src/lib.rs").unwrap();
    assert_eq!(lib.patch_status, "too_large");
    assert_eq!(lib.source, "local");
    assert!(!lib.commentable);
    assert!(!lib.hunks.is_empty());
}

#[tokio::test]
async fn head_moving_during_sync_starts_over() {
    let h = harness().await;
    let s = h.fake.with(seed);
    let base = s.base.clone();
    let number = s.number;
    // Push right before GitHub computes the file list, so the list is for a
    // newer head than the metadata we fetched first.
    h.fake.with(move |w| {
        w.hook("rest:files", move |w| {
            let c = w.commit(REPO, Some(&base), &[("src/lib.rs", Some("pushed mid-sync\n"))], "mid");
            w.push(REPO, number, &c);
        })
    });
    let pr_id = h.core.add_pr(&pr_url(s.number)).await.unwrap();
    let pr = h.core.get_pr(pr_id).unwrap();
    let rev = pr.revision.unwrap();
    assert_ne!(rev.head_oid, s.head, "must end on the pushed head");
    let lib = h.core.file_diff(rev.id, "src/lib.rs").unwrap();
    assert_eq!(lib.head_text.as_deref(), Some("pushed mid-sync\n"));
}

#[tokio::test]
async fn token_persists_across_restart() {
    let mut h = harness().await;
    let s = h.fake.with(seed);
    h.restart();
    assert!(h.core.auth_status().unwrap().signed_in);
    h.core.add_pr(&pr_url(s.number)).await.unwrap();
}

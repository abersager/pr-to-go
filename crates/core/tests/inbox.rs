//! Subscriptions, index polling, inbox membership and garbage collection
//! (DESIGN.md §9).

mod common;

use std::time::Duration;

use common::*;
use fake_github::VIEWER;
use pr_to_go_core::diff::Side;
use pr_to_go_core::drafts::{CommentKind, NewComment, SubjectType};

/// The seeded PR (review requested from me) plus one of Bob's that isn't.
fn world(h: &Harness) -> (Seeded, u64) {
    h.fake.with(|w| {
        let s = seed(w);
        w.request_review(REPO, s.number, VIEWER);
        let c = w.commit(REPO, Some(&s.base), &[("docs.md", Some("hello\n"))], "Docs");
        let other = w.open_pr(REPO, "main", "docs", &c, "Add docs", "", "bob");
        (s, other)
    })
}

fn titles(h: &Harness) -> Vec<String> {
    let mut t: Vec<String> = h.core.list_prs().unwrap().into_iter().map(|p| p.title).collect();
    t.sort();
    t
}

fn count(h: &Harness, op: &str) -> usize {
    h.fake.log().iter().filter(|l| *l == op).count()
}

#[tokio::test]
async fn a_search_fills_the_inbox_ready_for_offline() {
    let h = harness().await;
    world(&h);
    let s =
        h.core.add_subscription("search", "is:open review-requested:@me", Some("Review requests")).unwrap();
    assert_eq!(s.query.as_deref(), Some("is:open review-requested:@me is:pr"));
    let out = h.core.sync_inbox().await.unwrap();
    assert_eq!((out.polled, out.synced, out.failed), (1, 1, 0));
    assert_eq!(titles(&h), ["Improve widgets"]);
    let pr = &h.core.list_prs().unwrap()[0];
    assert_eq!(pr.sync_state, "ready");
    assert!(pr.in_inbox);
    let r = h.core.readiness().unwrap();
    assert_eq!((r.total, r.ready, r.not_synced), (1, 1, 0));
    assert!(r.bytes > 0);
    assert_eq!(h.core.subscriptions().unwrap()[0].prs, 1);
    assert!(
        h.core.add_subscription("search", "is:open review-requested:@me is:pr", None).is_err(),
        "no duplicates"
    );
}

#[tokio::test]
async fn a_repo_is_polled_incrementally_and_only_changes_are_synced() {
    let h = harness().await;
    let (_, other) = world(&h);
    h.core.add_subscription("repo", "https://github.com/acme/widgets", None).unwrap();
    h.core.sync_inbox().await.unwrap();
    assert_eq!(titles(&h), ["Add docs", "Improve widgets"]);

    // Nothing changed: one index page, no deep syncs.
    h.fake.clear_log();
    let out = h.core.sync_inbox().await.unwrap();
    assert_eq!(out.synced, 0);
    assert_eq!((count(&h, "RepoPrs"), count(&h, "PullRequestDetails")), (1, 0));

    // A new comment on one PR: only that one is synced again.
    h.fake.with(|w| w.add_issue_comment(REPO, other, "alice", "Nice docs"));
    h.fake.clear_log();
    let out = h.core.sync_inbox().await.unwrap();
    assert_eq!(out.synced, 1);
    assert_eq!(count(&h, "PullRequestDetails"), 1);
    let docs = h.core.list_prs().unwrap().into_iter().find(|p| p.title == "Add docs").unwrap();
    assert_eq!(h.core.get_pr(docs.id).unwrap().issue_comments.len(), 1);

    // Merged: it leaves the inbox.
    h.fake.with(|w| {
        let pr = w.pr(REPO, other);
        pr.state = fake_github::PrState::Merged;
        pr.updated_at = "2099-01-01T00:00:00Z".into();
    });
    h.core.sync_inbox().await.unwrap();
    assert_eq!(titles(&h), ["Improve widgets"]);
}

#[tokio::test]
async fn prs_leaving_the_inbox_are_kept_while_they_have_drafts() {
    let h = harness().await;
    let (s, _) = world(&h);
    h.core.add_subscription("search", "is:open review-requested:@me", None).unwrap();
    h.core.sync_inbox().await.unwrap();
    let pr = h.core.list_prs().unwrap()[0].id;
    let rev = h.core.get_pr(pr).unwrap().revision.unwrap().id;
    h.core
        .add_draft_comment(
            pr,
            NewComment {
                revision_id: rev,
                kind: CommentKind::Thread,
                subject_type: SubjectType::Line,
                path: Some("src/lib.rs".into()),
                side: Some(Side::Right),
                line: Some(5),
                start_side: None,
                start_line: None,
                reply_to_thread: None,
                body: "Still want to say this".into(),
            },
        )
        .unwrap();
    // The review request is withdrawn.
    h.fake.with(|w| w.pr(REPO, s.number).requested_reviewers.clear());
    h.core.sync_inbox().await.unwrap();
    let p = h.core.list_prs().unwrap();
    assert_eq!(p.len(), 1, "kept because of the draft");
    assert!(!p[0].in_inbox);
    assert_eq!(p[0].sync_state, "dormant");

    // Long after, GC still keeps it; without the draft it goes.
    h.clock.advance(Duration::from_secs(30 * 24 * 3600));
    assert_eq!(h.core.gc().unwrap().prs, 0);
    h.core.discard_review(pr).unwrap();
    // A discarded draft still counts as the user's; only PRs with no user
    // rows are removed. Here that means it stays until the draft rows go.
    assert_eq!(h.core.gc().unwrap().prs, 0);
}

#[tokio::test]
async fn gc_removes_dormant_prs_and_unused_revisions() {
    let h = harness().await;
    let (s, _) = world(&h);
    h.core.add_subscription("search", "is:open review-requested:@me", None).unwrap();
    h.core.sync_inbox().await.unwrap();
    let pr = h.core.list_prs().unwrap()[0].id;
    let first = h.core.get_pr(pr).unwrap().revision.unwrap().id;

    // Two pushes: the middle revision is needed by nothing.
    for text in ["one\n", "two\n"] {
        h.fake.with(|w| {
            let head = w.pr(REPO, s.number).head_oid.clone();
            let c = w.commit(REPO, Some(&head), &[("src/lib.rs", Some(text))], text);
            w.push(REPO, s.number, &c);
        });
        h.core.sync_pr(pr).await.unwrap();
    }
    let revisions = |h: &Harness| -> i64 {
        h.core.db().read(|c| Ok(c.query_row("SELECT count(*) FROM pr_revision", [], |r| r.get(0))?)).unwrap()
    };
    // current, plus the first (last viewed? no: never viewed) → only current stays.
    let report = h.core.gc().unwrap();
    assert!(report.revisions >= 1, "{report:?}");
    assert_eq!(revisions(&h), 1);
    assert!(h.core.file_diff(first, "src/lib.rs").is_err(), "the unused revision is gone");

    // The PR leaves the inbox and nothing of ours is attached.
    h.fake.with(|w| w.pr(REPO, s.number).requested_reviewers.clear());
    h.core.sync_inbox().await.unwrap();
    h.clock.advance(Duration::from_secs(15 * 24 * 3600));
    let report = h.core.gc().unwrap();
    assert_eq!(report.prs, 1);
    assert!(report.blobs > 0);
    assert!(h.core.list_prs().unwrap().is_empty());
    let blobs: i64 =
        h.core.db().read(|c| Ok(c.query_row("SELECT count(*) FROM blob", [], |r| r.get(0))?)).unwrap();
    assert_eq!(blobs, 0);
}

#[tokio::test]
async fn gc_keeps_the_revisions_a_draft_points_at() {
    let h = harness().await;
    let (s, _) = world(&h);
    let pr = h.core.add_pr(&pr_url(s.number)).await.unwrap();
    let basis = h.core.get_pr(pr).unwrap().revision.unwrap().id;
    h.core.update_draft_review(pr, Some("Thoughts so far"), None).unwrap();
    for text in ["one\n", "two\n"] {
        h.fake.with(|w| {
            let head = w.pr(REPO, s.number).head_oid.clone();
            let c = w.commit(REPO, Some(&head), &[("src/lib.rs", Some(text))], text);
            w.push(REPO, s.number, &c);
        });
        h.core.sync_pr(pr).await.unwrap();
    }
    h.core.gc().unwrap();
    // The draft's basis and the current revision remain.
    assert!(h.core.file_diff(basis, "src/lib.rs").is_ok());
    let n: i64 =
        h.core.db().read(|c| Ok(c.query_row("SELECT count(*) FROM pr_revision", [], |r| r.get(0))?)).unwrap();
    assert_eq!(n, 2);
}

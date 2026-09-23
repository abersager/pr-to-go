//! The outbox against the fake GitHub (DESIGN.md §10, §12 scenarios 2–15).

mod common;

use std::time::Duration;

use common::*;
use fake_github::{FaultAction, PrState, VIEWER};
use pr_to_go_core::diff::Side;
use pr_to_go_core::drafts::{CommentKind, NewComment, SubjectType, Verdict};
use pr_to_go_core::outbox::{CommentResolution, Reason, Resolution};

fn line(rev: i64, path: &str, side: Side, line: u32, body: &str) -> NewComment {
    NewComment {
        revision_id: rev,
        kind: CommentKind::Thread,
        subject_type: SubjectType::Line,
        path: Some(path.into()),
        side: Some(side),
        line: Some(line),
        start_side: None,
        start_line: None,
        reply_to_thread: None,
        body: body.into(),
    }
}

fn range(rev: i64, path: &str, start: u32, end: u32, body: &str) -> NewComment {
    NewComment {
        start_side: Some(Side::Right),
        start_line: Some(start),
        ..line(rev, path, Side::Right, end, body)
    }
}

fn file(rev: i64, path: &str, body: &str) -> NewComment {
    NewComment {
        subject_type: SubjectType::File,
        side: None,
        line: None,
        ..line(rev, path, Side::Right, 0, body)
    }
}

fn reply(rev: i64, thread: &str, body: &str) -> NewComment {
    NewComment {
        kind: CommentKind::Reply,
        reply_to_thread: Some(thread.into()),
        path: None,
        side: None,
        line: None,
        ..line(rev, "", Side::Right, 0, body)
    }
}

/// The latest review's status for a PR (active or not).
fn status(h: &Harness, pr: i64) -> String {
    h.core
        .db()
        .read(|c| {
            Ok(c.query_row(
                "SELECT status FROM draft_review WHERE pr_id = ?1 ORDER BY id DESC LIMIT 1",
                [pr],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap()
}

fn attention_reasons(h: &Harness, pr: i64) -> Vec<Reason> {
    let raw: String = h
        .core
        .db()
        .read(|c| {
            Ok(c.query_row(
                "SELECT attention FROM draft_review WHERE pr_id = ?1 ORDER BY id DESC LIMIT 1",
                [pr],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    serde_json::from_value(v["reasons"].clone()).unwrap()
}

/// Runs the outbox until the review settles, skipping backoff waits.
async fn drain(h: &Harness, pr: i64) -> String {
    for _ in 0..30 {
        let _ = h.core.run_outbox_once().await;
        let s = status(h, pr);
        if !matches!(s.as_str(), "queued" | "preflight" | "staging" | "submitting") {
            return s;
        }
        h.clock.advance(Duration::from_secs(600));
    }
    panic!("the outbox didn't settle: {}", status(h, pr));
}

#[derive(Debug, PartialEq)]
struct Posted {
    path: String,
    subject: String,
    side: Option<String>,
    line: Option<u32>,
    start_line: Option<u32>,
    body: String,
    reply: bool,
}

/// What the viewer has on GitHub: (reviews as (state, body, commit)), comments.
fn on_github(h: &Harness, number: u64) -> (Vec<(String, String, String)>, Vec<Posted>) {
    h.fake.with(|w| {
        let pr = w.pr(REPO, number);
        let reviews: Vec<(String, String, String)> = pr
            .reviews
            .iter()
            .filter(|r| r.author == VIEWER)
            .map(|r| (r.state.clone(), r.body.clone(), r.commit_oid.clone()))
            .collect();
        let mut comments = Vec::new();
        for t in &pr.threads {
            for c in t.comments.iter().filter(|c| c.author == VIEWER) {
                comments.push(Posted {
                    path: t.path.clone(),
                    subject: t.subject_type.clone(),
                    side: t.side.clone(),
                    line: t.original_line,
                    start_line: t.original_start_line,
                    body: c.body.clone(),
                    reply: c.reply_to.is_some(),
                });
            }
        }
        (reviews, comments)
    })
}

async fn setup() -> (Harness, i64, i64, Seeded) {
    let h = harness().await;
    let s = h.fake.with(seed);
    let pr = h.core.add_pr(&pr_url(s.number)).await.unwrap();
    let rev = h.core.get_pr(pr).unwrap().revision.unwrap().id;
    (h, pr, rev, s)
}

/// A full review: line, range, suggestion, file comment, reply, summary.
fn draft_everything(h: &Harness, pr: i64, rev: i64) {
    let thread = h.core.get_pr(pr).unwrap().threads[0].node_id.clone();
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "Why this name?")).unwrap();
    h.core.add_draft_comment(pr, range(rev, "src/lib.rs", 21, 22, "Merge these two")).unwrap();
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Left, 5, "The old name was clearer")).unwrap();
    h.core
        .add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 6, "```suggestion\nline six\n```"))
        .unwrap();
    h.core.add_draft_comment(pr, file(rev, "src/new.rs", "Needs tests")).unwrap();
    h.core.add_draft_comment(pr, reply(rev, &thread, "Agreed with Bob")).unwrap();
    h.core.update_draft_review(pr, Some("Nice. A few notes."), Some(Some(Verdict::Approve))).unwrap();
}

fn assert_everything_posted_once(h: &Harness, number: u64) {
    let (reviews, comments) = on_github(h, number);
    assert_eq!(reviews.len(), 1, "exactly one review: {reviews:?}");
    assert_eq!(reviews[0].0, "APPROVED");
    assert_eq!(reviews[0].1, "Nice. A few notes.");
    assert_eq!(comments.len(), 6, "each comment exactly once: {comments:#?}");
    let find =
        |body: &str| comments.iter().find(|c| c.body == body).unwrap_or_else(|| panic!("missing {body}"));
    let c = find("Why this name?");
    assert_eq!((c.side.as_deref(), c.line, c.start_line), (Some("RIGHT"), Some(5), None));
    let c = find("Merge these two");
    assert_eq!((c.line, c.start_line), (Some(22), Some(21)));
    let c = find("The old name was clearer");
    assert_eq!((c.side.as_deref(), c.line), (Some("LEFT"), Some(5)));
    assert!(find("```suggestion\nline six\n```").line == Some(6));
    let c = find("Needs tests");
    assert_eq!((c.subject.as_str(), c.path.as_str()), ("FILE", "src/new.rs"));
    let c = find("Agreed with Bob");
    assert!(c.reply);
    assert_eq!(c.path, "src/lib.rs");
}

#[tokio::test]
async fn submits_one_review_with_every_kind_of_comment() {
    let (h, pr, rev, s) = setup().await;
    draft_everything(&h, pr, rev);
    h.core.queue_review(pr).unwrap();
    h.fake.clear_log();
    assert_eq!(drain(&h, pr).await, "submitted");
    assert_everything_posted_once(&h, s.number);
    // Line threads went in the create call; the rest one by one.
    let log = h.fake.log();
    let count = |op: &str| log.iter().filter(|l| *l == op).count();
    assert_eq!(
        (count("AddReview"), count("AddThread"), count("AddReply"), count("SubmitReview")),
        (1, 1, 1, 1)
    );
    // The PR is free for a new draft, and the posted review is mirrored.
    assert!(h.core.draft(pr).unwrap().is_none());
    h.core.sync_pr(pr).await.unwrap();
    assert!(h.core.get_pr(pr).unwrap().reviews.iter().any(|r| r.author.as_deref() == Some(VIEWER)));
}

#[tokio::test]
async fn queued_offline_goes_out_when_back_online() {
    let (h, pr, rev, s) = setup().await;
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "Offline thought")).unwrap();
    h.fake.go_down();
    h.core.queue_review(pr).unwrap();
    h.core.run_outbox_once().await.unwrap();
    let d = h.core.draft(pr).unwrap().unwrap();
    assert_eq!(d.last_error_kind.as_deref(), Some("offline"));
    assert!(matches!(d.status.as_str(), "queued" | "preflight"));
    assert!(!h.core.connectivity().online);

    h.fake.come_up().await;
    assert_eq!(drain(&h, pr).await, "submitted");
    let (reviews, comments) = on_github(&h, s.number);
    assert_eq!((reviews.len(), comments.len()), (1, 1));
    assert_eq!(reviews[0].0, "COMMENTED");
}

#[tokio::test]
async fn force_push_asks_where_comments_go_then_posts_on_the_new_head() {
    let (h, pr, rev, s) = setup().await;
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 21, "About the new line")).unwrap();
    h.core.update_draft_review(pr, Some("LGTM"), Some(Some(Verdict::Approve))).unwrap();
    h.core.queue_review(pr).unwrap();
    // The author force-pushes: three lines added at the top shift everything.
    let new_head = h.fake.with(|w| {
        let lib = format!("new 1\nnew 2\nnew 3\n{}", w.repo(REPO).file_text(&s.head, "src/lib.rs").unwrap());
        let c = w.commit(REPO, Some(&s.base), &[("src/lib.rs", Some(&lib))], "Rework");
        w.push(REPO, s.number, &c);
        c
    });
    assert_eq!(drain(&h, pr).await, "needs_attention");
    let reasons = attention_reasons(&h, pr);
    let comment = h.core.draft(pr).unwrap().unwrap().comments[0].id;
    let Reason::HeadMoved { comments, verdict_stale, to_revision, .. } = &reasons[0] else {
        panic!("{reasons:?}")
    };
    assert_eq!(comments, &[comment]);
    assert!(verdict_stale);
    assert_eq!(*to_revision, h.core.get_pr(pr).unwrap().revision.unwrap().id);
    // Nothing reached GitHub.
    assert!(on_github(&h, s.number).0.is_empty());

    // The user moves the comment to its new line and confirms the approval.
    h.core
        .resolve_review(
            pr,
            Resolution {
                comments: vec![CommentResolution {
                    id: comment,
                    action: "remap".into(),
                    side: Some(Side::Right),
                    line: Some(24),
                    start_side: None,
                    start_line: None,
                    path: None,
                }],
                verdict: Some(Some(Verdict::Approve)),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(drain(&h, pr).await, "submitted");
    let (reviews, comments) = on_github(&h, s.number);
    assert_eq!(reviews[0].2, new_head, "the review targets the new head");
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].line, Some(24));
}

#[tokio::test]
async fn deleted_lines_become_a_file_comment() {
    let (h, pr, rev, s) = setup().await;
    h.core
        .add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 21, "```suggestion\nx\n```\nOr drop it"))
        .unwrap();
    h.core.queue_review(pr).unwrap();
    h.fake.with(|w| {
        let lib = w.repo(REPO).file_text(&s.head, "src/lib.rs").unwrap().replace("line 20a (new)\n", "");
        let c = w.commit(REPO, Some(&s.base), &[("src/lib.rs", Some(&lib))], "Drop 20a");
        w.push(REPO, s.number, &c);
    });
    assert_eq!(drain(&h, pr).await, "needs_attention");
    let id = h.core.draft(pr).unwrap().unwrap().comments[0].id;
    h.core
        .resolve_review(
            pr,
            Resolution {
                comments: vec![CommentResolution {
                    id,
                    action: "to_file".into(),
                    side: None,
                    line: None,
                    start_side: None,
                    start_line: None,
                    path: None,
                }],
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(drain(&h, pr).await, "submitted");
    let (_, comments) = on_github(&h, s.number);
    assert_eq!(comments[0].subject, "FILE");
    assert!(!comments[0].body.contains("```suggestion"), "suggestions can't survive as file comments");
}

#[tokio::test]
async fn keep_as_written_targets_the_reviewed_commit() {
    let (h, pr, rev, s) = setup().await;
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "Keep me where I was")).unwrap();
    h.core.queue_review(pr).unwrap();
    h.fake.with(|w| {
        let c = w.commit(REPO, Some(&s.head), &[("src/lib.rs", Some("all new\n"))], "Rewrite");
        w.push(REPO, s.number, &c);
    });
    assert_eq!(drain(&h, pr).await, "needs_attention");
    let keep = || Resolution { target_mode: Some("reviewed_commit".into()), ..Default::default() };
    h.core.resolve_review(pr, keep()).unwrap();
    assert_eq!(drain(&h, pr).await, "submitted");
    let (reviews, comments) = on_github(&h, s.number);
    assert_eq!(reviews[0].2, s.head, "posted on the commit that was reviewed");
    assert_eq!(comments[0].line, Some(5));
}

#[tokio::test]
async fn keep_as_written_fails_clearly_if_github_refuses_the_old_commit() {
    let (h, pr, rev, s) = setup().await;
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "x")).unwrap();
    h.core.queue_review(pr).unwrap();
    // A force-push that drops the reviewed commit from the PR entirely.
    h.fake.with(|w| {
        let c = w.commit(REPO, Some(&s.base), &[("src/lib.rs", Some("all new\n"))], "Rewrite");
        w.push(REPO, s.number, &c);
    });
    assert_eq!(drain(&h, pr).await, "needs_attention");
    h.core
        .resolve_review(pr, Resolution { target_mode: Some("reviewed_commit".into()), ..Default::default() })
        .unwrap();
    assert_eq!(drain(&h, pr).await, "needs_attention");
    assert!(matches!(attention_reasons(&h, pr)[0], Reason::ReviewedCommitUnavailable { .. }));
    assert!(on_github(&h, s.number).0.is_empty());
}

#[tokio::test]
async fn head_moving_again_during_resolution_asks_again() {
    let (h, pr, rev, s) = setup().await;
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "x")).unwrap();
    h.core.queue_review(pr).unwrap();
    let push = |w: &mut fake_github::World, text: &str| {
        let head = w.pr(REPO, s.number).head_oid.clone();
        let lib = format!("{text}\n{}", w.repo(REPO).file_text(&head, "src/lib.rs").unwrap());
        let c = w.commit(REPO, Some(&head), &[("src/lib.rs", Some(&lib))], text);
        w.push(REPO, s.number, &c);
    };
    h.fake.with(|w| push(w, "first push"));
    assert_eq!(drain(&h, pr).await, "needs_attention");
    let id = h.core.draft(pr).unwrap().unwrap().comments[0].id;
    // While the user decides, another push lands.
    h.fake.with(|w| push(w, "second push"));
    h.core
        .resolve_review(
            pr,
            Resolution {
                comments: vec![CommentResolution {
                    id,
                    action: "remap".into(),
                    side: Some(Side::Right),
                    line: Some(6),
                    start_side: None,
                    start_line: None,
                    path: None,
                }],
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(drain(&h, pr).await, "needs_attention", "the second push must be looked at too");
    assert!(on_github(&h, s.number).0.is_empty());
}

#[tokio::test]
async fn lost_responses_never_duplicate_anything() {
    for op in ["AddReview", "AddThread", "AddReply", "SubmitReview"] {
        let (h, pr, rev, s) = setup().await;
        draft_everything(&h, pr, rev);
        h.core.queue_review(pr).unwrap();
        // GitHub applies the mutation, then the response is lost.
        h.fake.with(|w| w.fault(op, FaultAction::ApplyThenStatus(502)));
        assert_eq!(drain(&h, pr).await, "submitted", "{op}");
        assert_everything_posted_once(&h, s.number);
    }
}

#[tokio::test]
async fn a_timeout_after_submit_is_reconciled() {
    let (h, pr, rev, s) = setup().await;
    draft_everything(&h, pr, rev);
    h.core.queue_review(pr).unwrap();
    // Longer than the test client's 1.5 s timeout.
    h.fake.with(|w| w.fault("SubmitReview", FaultAction::ApplyThenDelay(Duration::from_millis(2500))));
    assert_eq!(drain(&h, pr).await, "submitted");
    assert_everything_posted_once(&h, s.number);
}

#[tokio::test]
async fn crashes_between_any_two_steps_never_duplicate_anything() {
    for point in ["after_create_review", "after_add_thread", "after_add_reply", "after_submit"] {
        let (mut h, pr, rev, s) = setup().await;
        draft_everything(&h, pr, rev);
        h.core.queue_review(pr).unwrap();
        h.core.set_crash_point(Some(point));
        let mut crashed = false;
        for _ in 0..10 {
            if h.core.run_outbox_once().await.is_err() {
                crashed = true;
                break;
            }
            h.clock.advance(Duration::from_secs(600));
        }
        assert!(crashed, "{point} never happened");
        // The app restarts with whatever was stored, first without a
        // connection: a failed check must not count as "it didn't happen".
        h.restart();
        h.fake.go_down();
        let _ = h.core.run_outbox_once().await;
        h.clock.advance(Duration::from_secs(600));
        h.fake.come_up().await;
        assert_eq!(drain(&h, pr).await, "submitted", "{point}");
        assert_everything_posted_once(&h, s.number);
    }
}

#[tokio::test]
async fn a_rejected_comment_is_flagged_and_the_rest_stay_staged() {
    let (h, pr, rev, s) = setup().await;
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "fine 1")).unwrap();
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 6, "rejected")).unwrap();
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 7, "fine 2")).unwrap();
    h.core.queue_review(pr).unwrap();
    // GitHub refuses the batch, then the second thread on its own.
    h.fake.with(|w| {
        w.fault("AddReview", FaultAction::Status(422));
        w.fault_after("AddThread", 1, 1, FaultAction::Status(422));
    });
    assert_eq!(drain(&h, pr).await, "needs_attention");
    let d = h.core.draft(pr).unwrap().unwrap();
    let bad = d.comments.iter().find(|c| c.body_md == "rejected").unwrap().id;
    assert!(
        matches!(&attention_reasons(&h, pr)[0], Reason::CommentRejected { comment: Some(c), .. } if *c == bad)
    );
    // The pending review is invisible to others; nothing was submitted.
    assert!(on_github(&h, s.number).0.iter().all(|r| r.0 == "PENDING"));

    let drop = CommentResolution {
        id: bad,
        action: "drop".into(),
        side: None,
        line: None,
        start_side: None,
        start_line: None,
        path: None,
    };
    h.core.resolve_review(pr, Resolution { comments: vec![drop], ..Default::default() }).unwrap();
    assert_eq!(drain(&h, pr).await, "submitted");
    let (reviews, comments) = on_github(&h, s.number);
    assert_eq!(reviews.len(), 1, "the half-built pending review was cleaned up: {reviews:?}");
    let mut bodies: Vec<&str> = comments.iter().map(|c| c.body.as_str()).collect();
    bodies.sort();
    assert_eq!(bodies, ["fine 1", "fine 2"]);
}

#[tokio::test]
async fn adds_to_a_pending_review_started_on_github_without_deleting_it() {
    let (h, pr, rev, s) = setup().await;
    let web_review = h.fake.with(|w| {
        let id = w.add_review(
            REPO,
            s.number,
            VIEWER,
            "PENDING",
            "",
            &[("src/new.rs", "RIGHT", 1, "From the web")],
        );
        w.pr(REPO, s.number).reviews.last_mut().unwrap().submitted_at = None;
        id
    });
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "From the plane")).unwrap();
    h.core.update_draft_review(pr, Some("Both together"), None).unwrap();
    h.core.queue_review(pr).unwrap();
    assert_eq!(drain(&h, pr).await, "needs_attention");
    assert_eq!(attention_reasons(&h, pr), vec![Reason::ExistingPendingReview { review: web_review.clone() }]);
    h.core
        .resolve_review(
            pr,
            Resolution { acknowledge: vec![format!("merge_pending:{web_review}")], ..Default::default() },
        )
        .unwrap();
    assert_eq!(drain(&h, pr).await, "submitted");
    let (reviews, comments) = on_github(&h, s.number);
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].1, "Both together");
    let mut bodies: Vec<&str> = comments.iter().map(|c| c.body.as_str()).collect();
    bodies.sort();
    assert_eq!(bodies, ["From the plane", "From the web"]);
}

#[tokio::test]
async fn waits_out_a_secondary_rate_limit() {
    let (h, pr, rev, s) = setup().await;
    draft_everything(&h, pr, rev);
    h.core.queue_review(pr).unwrap();
    h.fake.with(|w| w.fault("AddThread", FaultAction::SecondaryRateLimit { retry_after: Some(1) }));
    h.core.run_outbox_once().await.unwrap();
    let d = h.core.draft(pr).unwrap().unwrap();
    assert_eq!(d.status, "staging");
    assert_eq!(d.last_error_kind.as_deref(), Some("rate_limited"));
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(drain(&h, pr).await, "submitted");
    assert_everything_posted_once(&h, s.number);
}

#[tokio::test]
async fn holds_an_approval_when_someone_requested_changes_meanwhile() {
    let (h, pr, rev, s) = setup().await;
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "Nit")).unwrap();
    h.core.update_draft_review(pr, Some("LGTM"), Some(Some(Verdict::Approve))).unwrap();
    h.core.queue_review(pr).unwrap();
    let blocking = h
        .fake
        .with(|w| w.add_review(REPO, s.number, "alice", "CHANGES_REQUESTED", "Wait, this breaks X", &[]));
    assert_eq!(drain(&h, pr).await, "needs_attention");
    assert!(
        matches!(&attention_reasons(&h, pr)[0], Reason::NewBlockingReview { review, .. } if *review == blocking)
    );
    // The user reads it and downgrades to a comment.
    h.core
        .resolve_review(
            pr,
            Resolution {
                acknowledge: vec![format!("blocking:{blocking}")],
                verdict: Some(Some(Verdict::Comment)),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(drain(&h, pr).await, "submitted");
    assert_eq!(on_github(&h, s.number).0[0].0, "COMMENTED");
}

#[tokio::test]
async fn a_merged_pr_needs_confirmation() {
    let (h, pr, rev, s) = setup().await;
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "Post-merge note")).unwrap();
    h.core.queue_review(pr).unwrap();
    h.fake.with(|w| w.pr(REPO, s.number).state = PrState::Merged);
    assert_eq!(drain(&h, pr).await, "needs_attention");
    assert_eq!(attention_reasons(&h, pr), vec![Reason::PrStateChanged { state: "MERGED".into() }]);
    h.core
        .resolve_review(pr, Resolution { acknowledge: vec!["pr_state:MERGED".into()], ..Default::default() })
        .unwrap();
    assert_eq!(drain(&h, pr).await, "submitted");
}

#[tokio::test]
async fn a_deleted_reply_target_is_caught_before_sending() {
    let (h, pr, rev, s) = setup().await;
    let thread = h.core.get_pr(pr).unwrap().threads[0].node_id.clone();
    h.core.add_draft_comment(pr, reply(rev, &thread, "Replying to nothing")).unwrap();
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "Still relevant")).unwrap();
    h.core.queue_review(pr).unwrap();
    h.fake.with(|w| w.pr(REPO, s.number).threads.clear());
    assert_eq!(drain(&h, pr).await, "needs_attention");
    let d = h.core.draft(pr).unwrap().unwrap();
    let r = d.comments.iter().find(|c| c.kind == CommentKind::Reply).unwrap().id;
    assert_eq!(attention_reasons(&h, pr), vec![Reason::ReplyTargetGone { comment: r }]);
    let fold = CommentResolution {
        id: r,
        action: "to_summary".into(),
        side: None,
        line: None,
        start_side: None,
        start_line: None,
        path: None,
    };
    h.core.resolve_review(pr, Resolution { comments: vec![fold], ..Default::default() }).unwrap();
    assert_eq!(drain(&h, pr).await, "submitted");
    let (reviews, comments) = on_github(&h, s.number);
    assert!(reviews[0].1.contains("Replying to nothing"), "folded into the summary: {:?}", reviews[0].1);
    assert_eq!(comments.len(), 1);
}

#[tokio::test]
async fn a_revoked_token_stops_without_losing_anything() {
    let (h, pr, rev, s) = setup().await;
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "Keep me safe")).unwrap();
    h.core.queue_review(pr).unwrap();
    h.fake.with(|w| w.tokens.clear());
    assert_eq!(drain(&h, pr).await, "needs_attention");
    assert!(matches!(attention_reasons(&h, pr)[0], Reason::Auth { .. }));
    assert_eq!(h.core.draft(pr).unwrap().unwrap().comments[0].body_md, "Keep me safe");
    assert!(h.core.export_review_markdown(pr).unwrap().contains("Keep me safe"));
    assert!(on_github(&h, s.number).0.is_empty());
}

#[tokio::test]
async fn a_captive_portal_sends_nothing() {
    let (h, pr, rev, _) = setup().await;
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "x")).unwrap();
    h.core.queue_review(pr).unwrap();
    h.fake.with(|w| w.fault("PullRequestDetails", FaultAction::CaptivePortal));
    h.fake.clear_log();
    h.core.run_outbox_once().await.unwrap();
    let log = h.fake.log();
    assert!(!log.iter().any(|l| l.starts_with("Add") || l == "SubmitReview"), "{log:?}");
    assert_eq!(h.core.draft(pr).unwrap().unwrap().last_error_kind.as_deref(), Some("offline"));
}

#[tokio::test]
async fn discarding_a_blocked_review_deletes_the_pending_review_we_staged() {
    let (h, pr, rev, s) = setup().await;
    draft_everything(&h, pr, rev);
    h.core.queue_review(pr).unwrap();
    h.fake.with(|w| w.fault("AddThread", FaultAction::Status(422)));
    assert_eq!(drain(&h, pr).await, "needs_attention");
    assert!(on_github(&h, s.number).0.iter().any(|r| r.0 == "PENDING"));
    h.core.discard_review(pr).unwrap();
    h.core.run_outbox_once().await.unwrap();
    assert!(on_github(&h, s.number).0.is_empty(), "{:?}", on_github(&h, s.number).0);
}

/// Staging started, then the connection went: the review sits half-sent.
/// It can still be edited (the partial pending review is cleaned up later),
/// and after re-queueing it goes out exactly once.
#[tokio::test]
async fn a_review_stuck_mid_send_can_be_edited() {
    let (h, pr, rev, s) = setup().await;
    draft_everything(&h, pr, rev);
    let id = h.core.queue_review(pr).unwrap().id;
    // queued → preflight → staging → pending review created with line threads.
    while h.core.draft(pr).unwrap().unwrap().status != "staging" {
        h.core.step_review(id).await.unwrap();
    }
    h.core.step_review(id).await.unwrap();
    assert!(on_github(&h, s.number).0.iter().any(|r| r.0 == "PENDING"));
    // Offline for the next comment.
    h.core.set_work_offline(true).unwrap();
    h.core.step_review(id).await.unwrap();
    let d = h.core.draft(pr).unwrap().unwrap();
    assert_eq!((d.status.as_str(), d.last_error_kind.as_deref()), ("staging", Some("offline")));

    // The user edits it on the plane.
    h.core.unqueue_review(pr).unwrap();
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 7, "One more")).unwrap();
    h.core.queue_review(pr).unwrap();

    h.core.set_work_offline(false).unwrap();
    assert_eq!(drain(&h, pr).await, "submitted");
    let (reviews, comments) = on_github(&h, s.number);
    assert_eq!(reviews.len(), 1, "the half-built pending review was replaced: {reviews:?}");
    assert_eq!(comments.len(), 7);
}

/// While GitHub hasn't confirmed whether the last step went through, the
/// review can't be taken back: that could send it twice.
#[tokio::test]
async fn an_unconfirmed_step_blocks_editing_until_checked() {
    let (mut h, pr, rev, s) = setup().await;
    draft_everything(&h, pr, rev);
    h.core.queue_review(pr).unwrap();
    h.core.set_crash_point(Some("after_submit"));
    let _ = h.core.run_outbox_once().await;
    h.restart();
    let err = h.core.unqueue_review(pr).unwrap_err().to_string();
    assert!(err.contains("waiting for GitHub to confirm"), "{err}");
    assert_eq!(drain(&h, pr).await, "submitted");
    assert_everything_posted_once(&h, s.number);
}

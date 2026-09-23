//! Remapping comments after the PR moves (DESIGN.md §10.4, §11; §12
//! scenarios 3–5).

mod common;

use std::time::Duration;

use common::*;
use fake_github::VIEWER;
use pr_to_go_core::diff::Side;
use pr_to_go_core::drafts::{CommentKind, NewComment, SubjectType};
use pr_to_go_core::outbox::{CommentResolution, Reason, Resolution};
use pr_to_go_core::remap::{Proposal, Status};

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

fn accept(id: i64) -> CommentResolution {
    CommentResolution {
        id,
        action: "remap".into(),
        side: None,
        line: None,
        start_side: None,
        start_line: None,
        path: None,
    }
}

async fn drain(h: &Harness, pr: i64) -> String {
    for _ in 0..30 {
        let _ = h.core.run_outbox_once().await;
        let s: String = h
            .core
            .db()
            .read(|c| {
                Ok(c.query_row(
                    "SELECT status FROM draft_review WHERE pr_id = ?1 ORDER BY id DESC LIMIT 1",
                    [pr],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        if !matches!(s.as_str(), "queued" | "preflight" | "staging" | "submitting") {
            return s;
        }
        h.clock.advance(Duration::from_secs(600));
    }
    panic!("didn't settle");
}

fn proposal_of(h: &Harness, pr: i64, comment: i64) -> Proposal {
    let d = h.core.draft(pr).unwrap().unwrap();
    let c = d.comments.iter().find(|c| c.id == comment).unwrap();
    serde_json::from_value(c.remap_proposal.clone().expect("a proposal")).unwrap()
}

fn posted(h: &Harness, number: u64) -> Vec<(String, Option<u32>, String)> {
    h.fake.with(|w| {
        w.pr(REPO, number)
            .threads
            .iter()
            .flat_map(|t| t.comments.iter().map(move |c| (t, c)))
            .filter(|(_, c)| c.author == VIEWER)
            .map(|(t, c)| (t.path.clone(), t.original_line, c.body.clone()))
            .collect()
    })
}

async fn setup() -> (Harness, i64, i64, Seeded) {
    let h = harness().await;
    let s = h.fake.with(seed);
    let pr = h.core.add_pr(&pr_url(s.number)).await.unwrap();
    let rev = h.core.get_pr(pr).unwrap().revision.unwrap().id;
    (h, pr, rev, s)
}

/// Pushes a new head: `f` edits src/lib.rs as of the current head.
fn push_lib(h: &Harness, s: &Seeded, f: impl Fn(String) -> String) {
    h.fake.with(|w| {
        let head = w.pr(REPO, s.number).head_oid.clone();
        let lib = f(w.repo(REPO).file_text(&head, "src/lib.rs").unwrap());
        let c = w.commit(REPO, Some(&s.base), &[("src/lib.rs", Some(&lib))], "Force-push");
        w.push(REPO, s.number, &c);
    });
}

#[tokio::test]
async fn the_outbox_proposes_where_moved_comments_go() {
    let (h, pr, rev, s) = setup().await;
    let d = h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 21, "About 20a")).unwrap();
    let moved = d.comments[0].id;
    let d = h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "About five")).unwrap();
    let edited = d.comments.iter().find(|c| c.body_md == "About five").unwrap().id;
    h.core.queue_review(pr).unwrap();
    // Three lines added at the top; line five reworded.
    push_lib(&h, &s, |lib| {
        format!("a\nb\nc\n{}", lib.replace("line five (edited)", "line five (edited again)"))
    });
    assert_eq!(drain(&h, pr).await, "needs_attention");
    let attention = h.core.draft(pr).unwrap().unwrap().attention.unwrap();
    let reason: Reason = serde_json::from_value(attention["reasons"][0].clone()).unwrap();
    let Reason::HeadMoved { comments, .. } = reason else { panic!("{reason:?}") };
    assert_eq!(comments.len(), 2);

    let p = proposal_of(&h, pr, moved);
    assert_eq!((p.status, p.line), (Status::Clean, Some(24)));
    let p = proposal_of(&h, pr, edited);
    assert_eq!((p.status, p.line), (Status::Fuzzy, Some(8)));
    assert_eq!(p.new_lines, ["line five (edited again)"]);

    // Accept both proposals as they are.
    h.core
        .resolve_review(
            pr,
            Resolution { comments: vec![accept(moved), accept(edited)], ..Default::default() },
        )
        .unwrap();
    assert_eq!(drain(&h, pr).await, "submitted");
    let mut got = posted(&h, s.number);
    got.sort_by_key(|p| p.1);
    assert_eq!(
        got,
        [
            ("src/lib.rs".into(), Some(8), "About five".into()),
            ("src/lib.rs".into(), Some(24), "About 20a".into())
        ]
    );
}

#[tokio::test]
async fn a_draft_can_be_moved_onto_the_new_version_before_queueing() {
    let (h, pr, rev, s) = setup().await;
    let d = h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 22, "About 20b")).unwrap();
    let id = d.comments[0].id;
    push_lib(&h, &s, |lib| format!("x\ny\n{lib}"));
    h.core.sync_pr(pr).await.unwrap();
    let current = h.core.get_pr(pr).unwrap().revision.unwrap().id;

    let proposals = h.core.draft_proposals(pr).unwrap();
    assert_eq!(proposals.len(), 1);
    assert_eq!((proposals[0].1.status, proposals[0].1.line), (Status::Clean, Some(24)));
    let d = h.core.rebase_draft(pr, vec![accept(id)]).unwrap();
    assert_eq!(d.comments[0].anchor_revision_id, Some(current));
    assert_eq!(d.comments[0].line, Some(24));
    assert_eq!(d.basis_revision_id, current);
    // Nothing left to ask about: it goes straight out.
    h.core.queue_review(pr).unwrap();
    assert_eq!(drain(&h, pr).await, "submitted");
    assert_eq!(posted(&h, s.number), [("src/lib.rs".into(), Some(24), "About 20b".into())]);
}

#[tokio::test]
async fn interdiff_shows_what_changed_since_the_review() {
    let (h, pr, rev, s) = setup().await;
    push_lib(&h, &s, |lib| lib.replace("line 12\n", "line twelve\n"));
    h.core.sync_pr(pr).await.unwrap();
    let current = h.core.get_pr(pr).unwrap().revision.unwrap().id;
    let diffs = h.core.interdiff(rev, current).unwrap();
    // src/lib.rs changed, and comes first.
    let lib = &diffs[0];
    assert_eq!((lib.path.as_str(), lib.change_type.as_str()), ("src/lib.rs", "changed"));
    let text: Vec<String> = lib.hunks.iter().flat_map(|h| h.lines.iter().map(|l| l.text.clone())).collect();
    assert!(text.contains(&"line twelve".to_string()));
    // The force-push dropped the PR's other changes: those files are back to
    // the base, not deleted.
    let new_rs = diffs.iter().find(|d| d.path == "src/new.rs").unwrap();
    assert_eq!(new_rs.change_type, "no_longer_changed");
    assert_eq!(new_rs.head_text, None, "src/new.rs didn't exist in the base");
    let readme = diffs.iter().find(|d| d.path == "CHANGELOG").unwrap();
    assert_eq!(
        (readme.change_type.as_str(), readme.head_text.as_deref()),
        ("no_longer_changed", Some("v1\n"))
    );
    let logo = diffs.iter().find(|d| d.path == "logo.png").unwrap();
    assert!(logo.base_binary && logo.hunks.is_empty());
}

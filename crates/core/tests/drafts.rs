//! Drafting (DESIGN.md §2 edge cases 1, 17, 22; §12 scenario 14).

mod common;

use common::*;
use pr_to_go_core::Error;
use pr_to_go_core::diff::Side;
use pr_to_go_core::drafts::{CommentKind, NewComment, SubjectType, Verdict};

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

fn range(rev: i64, path: &str, start: (Side, u32), end: (Side, u32), body: &str) -> NewComment {
    NewComment { start_side: Some(start.0), start_line: Some(start.1), ..line(rev, path, end.0, end.1, body) }
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

fn invalid(r: Result<impl std::fmt::Debug, Error>) -> String {
    match r {
        Err(Error::Invalid(m)) => m,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

async fn setup() -> (Harness, i64, i64, Seeded) {
    let h = harness().await;
    let s = h.fake.with(seed);
    let pr = h.core.add_pr(&pr_url(s.number)).await.unwrap();
    let rev = h.core.get_pr(pr).unwrap().revision.unwrap().id;
    (h, pr, rev, s)
}

#[tokio::test]
async fn drafts_comments_of_every_kind_with_anchors() {
    let (h, pr, rev, _) = setup().await;
    let thread = h.core.get_pr(pr).unwrap().threads[0].node_id.clone();

    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "Rename?")).unwrap();
    h.core
        .add_draft_comment(pr, range(rev, "src/lib.rs", (Side::Right, 21), (Side::Right, 22), "Merge these"))
        .unwrap();
    h.core
        .add_draft_comment(pr, range(rev, "src/lib.rs", (Side::Left, 5), (Side::Right, 5), "Across sides"))
        .unwrap();
    h.core.add_draft_comment(pr, file(rev, "src/new.rs", "Needs tests")).unwrap();
    let d = h.core.add_draft_comment(pr, reply(rev, &thread, "Agreed")).unwrap();

    assert_eq!(d.status, "draft");
    assert_eq!(d.basis_revision_id, rev);
    assert_eq!(d.comments.len(), 5);

    let single = d.comments.iter().find(|c| c.body_md == "Rename?").unwrap();
    let a = single.anchor.as_ref().unwrap();
    let right = a.right.as_ref().unwrap();
    assert_eq!((right.first, right.last), (5, 5));
    assert_eq!(right.lines, ["line five (edited)"]);
    assert_eq!(right.before, ["line 2", "line 3", "line 4"]);
    assert_eq!(right.after, ["line 6", "line 7", "line 8"]);
    assert!(a.left.is_none());

    let multi = d.comments.iter().find(|c| c.body_md == "Merge these").unwrap();
    assert_eq!((multi.start_side, multi.start_line, multi.line), (Some(Side::Right), Some(21), Some(22)));
    assert_eq!(
        multi.anchor.as_ref().unwrap().right.as_ref().unwrap().lines,
        ["line 20a (new)", "line 20b (new)"]
    );

    let across = d.comments.iter().find(|c| c.body_md == "Across sides").unwrap();
    let a = across.anchor.as_ref().unwrap();
    assert_eq!(a.left.as_ref().unwrap().lines, ["line 5"]);
    assert_eq!(a.right.as_ref().unwrap().lines, ["line five (edited)"]);

    let r = d.comments.iter().find(|c| c.kind == CommentKind::Reply).unwrap();
    assert_eq!(r.path.as_deref(), Some("src/lib.rs"));
    assert!(r.anchor.is_none());

    // Edits and deletes; the draft survives an app restart.
    h.core.update_draft_comment(single.id, "Rename to `edited`?").unwrap();
    h.core.delete_draft_comment(r.id).unwrap();
    h.core.update_draft_review(pr, Some("Looks good overall."), Some(Some(Verdict::Approve))).unwrap();
    let mut h = h;
    h.restart();
    let d = h.core.draft(pr).unwrap().unwrap();
    assert_eq!(d.comments.len(), 4);
    assert_eq!(d.body_md, "Looks good overall.");
    assert_eq!(d.verdict, Some(Verdict::Approve));
    assert!(d.comments.iter().any(|c| c.body_md == "Rename to `edited`?"));
}

#[tokio::test]
async fn rejects_comments_github_would_refuse() {
    let (h, pr, rev, _) = setup().await;
    let m = invalid(h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 13, "x")));
    assert!(m.contains("isn't part of the diff"), "{m}");
    let m = invalid(
        h.core.add_draft_comment(pr, range(rev, "src/lib.rs", (Side::Right, 5), (Side::Right, 21), "x")),
    );
    assert!(m.contains("one hunk"), "{m}");
    let m = invalid(
        h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Left, 5, "```suggestion\nx\n```")),
    );
    assert!(m.contains("Suggestions"), "{m}");
    let m = invalid(h.core.add_draft_comment(pr, file(rev, "src/new.rs", "```suggestion\nx\n```")));
    assert!(m.contains("Suggestions"), "{m}");
    let m = invalid(h.core.add_draft_comment(pr, reply(rev, "PRRT_missing", "x")));
    assert!(m.contains("no longer exists"), "{m}");
    let m = invalid(h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "   ")));
    assert!(m.contains("empty"), "{m}");
    let m = invalid(h.core.add_draft_comment(pr, file(rev, "README.md", "not in the PR")));
    assert!(m.contains("isn't changed"), "{m}");
    // A suggestion on new lines is fine.
    h.core
        .add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "```suggestion\nline 5\n```"))
        .unwrap();
    // Nothing invalid was stored.
    assert_eq!(h.core.draft(pr).unwrap().unwrap().comments.len(), 1);
}

#[tokio::test]
async fn line_comments_need_github_patch() {
    let h = harness().await;
    let s = h.fake.with(|w| {
        let s = seed(w);
        w.patch_limit = 60;
        s
    });
    let pr = h.core.add_pr(&pr_url(s.number)).await.unwrap();
    let rev = h.core.get_pr(pr).unwrap().revision.unwrap().id;
    let m = invalid(h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "x")));
    assert!(m.contains("only file comments"), "{m}");
    h.core.add_draft_comment(pr, file(rev, "src/lib.rs", "Too big to review line by line")).unwrap();
}

#[tokio::test]
async fn checks_before_queueing() {
    let (h, pr, rev, _) = setup().await;
    // Empty review.
    h.core.update_draft_review(pr, Some(""), None).unwrap();
    assert!(invalid(h.core.queue_review(pr)).contains("empty"));
    // Requesting changes needs a summary.
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "Nit")).unwrap();
    h.core.update_draft_review(pr, None, Some(Some(Verdict::RequestChanges))).unwrap();
    assert!(invalid(h.core.queue_review(pr)).contains("summary"));
    h.core.update_draft_review(pr, Some("Please fix the nit."), None).unwrap();
    let d = h.core.queue_review(pr).unwrap();
    assert_eq!(d.status, "queued");
    // Queued reviews are frozen until taken back.
    assert!(
        invalid(h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 6, "More")))
            .contains("Edit")
    );
    assert!(invalid(h.core.update_draft_review(pr, Some("x"), None)).contains("Edit"));
    let d = h.core.unqueue_review(pr).unwrap();
    assert_eq!(d.status, "draft");
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 6, "More")).unwrap();
    // Discarding frees the PR for a new draft.
    h.core.discard_review(pr).unwrap();
    assert!(h.core.draft(pr).unwrap().is_none());
    let d = h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "Fresh start")).unwrap();
    assert_eq!(d.comments.len(), 1);
}

#[tokio::test]
async fn cannot_approve_own_pull_request() {
    let h = harness().await;
    let s = h.fake.with(|w| {
        let s = seed(w);
        w.pr(REPO, s.number).author = fake_github::VIEWER.into();
        s
    });
    let pr = h.core.add_pr(&pr_url(s.number)).await.unwrap();
    h.core.update_draft_review(pr, Some("LGTM"), Some(Some(Verdict::Approve))).unwrap();
    assert!(invalid(h.core.queue_review(pr)).contains("your own"));
    h.core.update_draft_review(pr, None, Some(Some(Verdict::Comment))).unwrap();
    assert_eq!(h.core.queue_review(pr).unwrap().status, "queued");
}

/// The Hubtty failure (DESIGN.md §1.1.4): a force-push while drafting must
/// not lose the drafts, or the revision and file contents they point at.
#[tokio::test]
async fn sync_never_deletes_drafts() {
    let (h, pr, rev, s) = setup().await;
    h.core.add_draft_comment(pr, line(rev, "src/lib.rs", Side::Right, 5, "About this line")).unwrap();
    h.core.update_draft_review(pr, Some("Summary"), Some(Some(Verdict::Comment))).unwrap();
    let old = h.core.get_pr(pr).unwrap().revision.unwrap();

    // Force-push a rewrite, delete the thread we might reply to, and sync.
    h.fake.with(|w| {
        let c = w.commit(REPO, Some(&s.base), &[("src/lib.rs", Some("rewritten\n"))], "Rewrite");
        w.push(REPO, s.number, &c);
        w.pr(REPO, s.number).threads.clear();
    });
    let out = h.core.sync_pr(pr).await.unwrap();
    assert!(out.head_moved);

    let d = h.core.draft(pr).unwrap().unwrap();
    assert_eq!(d.comments.len(), 1);
    assert_eq!(d.comments[0].body_md, "About this line");
    assert_eq!(d.basis_revision_id, old.id);
    assert_eq!(d.comments[0].anchor_revision_id, Some(old.id));
    // The reviewed revision and its contents are still there.
    let old_diff = h.core.file_diff(old.id, "src/lib.rs").unwrap();
    assert!(old_diff.head_text.unwrap().contains("line five (edited)"));
    // And the database refuses to drop what a draft points at.
    let res = h.core.db().write(|tx| {
        tx.execute("DELETE FROM pr_revision WHERE id = ?1", [old.id])?;
        Ok(())
    });
    assert!(res.is_err());
}

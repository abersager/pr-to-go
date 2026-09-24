//! Review submission: pending reviews, threads, replies, submit and delete,
//! plus the queries the outbox uses to reconcile after an uncertain step.
//! The rules GitHub enforces are enforced here too.

use serde_json::{Value, json};

use crate::gql::{GqlError, GqlResult, comment_json, s};
use crate::model::{FileChange, Pr, Review, ReviewComment, Thread, World, in_hunks};

pub fn handle(w: &mut World, viewer: &str, op: &str, vars: &Value) -> GqlResult {
    match op {
        "AddReview" => add_review(w, viewer, vars),
        "AddThread" => add_thread(w, viewer, vars),
        "AddReply" => add_reply(w, viewer, vars),
        "SubmitReview" => submit_review(w, viewer, vars),
        "DeleteReview" => delete_review(w, viewer, vars),
        "PendingReviews" => pending_reviews(w, viewer, vars),
        "ReviewComments" => review_comments(w, viewer, vars),
        "ReviewState" => review_state(w, viewer, vars),
        _ => Err(GqlError::NotFound(format!("fake-github doesn't implement operation {op}"))),
    }
}

fn not_found(id: &str) -> GqlError {
    GqlError::NotFound(format!("Could not resolve to a node with the global id of '{id}'"))
}

fn opt_u32(v: &Value, k: &str) -> Option<u32> {
    v.get(k).and_then(|x| x.as_u64()).map(|x| x as u32)
}

fn opt_str<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
}

/// The PR's diff as of `commit`: merge base with the base branch → commit.
fn diff_at(w: &World, repo: &str, pr: &Pr, commit: &str) -> Vec<FileChange> {
    let r = &w.repos[repo];
    let base_tip = &r.branches[&pr.base_ref];
    let mb = r.merge_base(base_tip, commit).expect("merge base");
    r.diff(&mb, commit, w.patch_limit)
}

/// Index of the hunk containing `line` on `side`.
fn hunk_of(patch: &str, side: &str, line: u32) -> Option<usize> {
    let mut idx = None;
    let mut current = String::new();
    let mut n = 0usize;
    for l in patch.lines() {
        if l.starts_with("@@") {
            if !current.is_empty() && in_hunks(Some(&current), side, line) {
                idx = Some(n - 1);
            }
            current = format!("{l}\n");
            n += 1;
        } else {
            current.push_str(l);
            current.push('\n');
        }
    }
    if idx.is_none() && !current.is_empty() && in_hunks(Some(&current), side, line) {
        idx = Some(n - 1);
    }
    idx
}

struct NewThread<'a> {
    path: &'a str,
    subject: &'a str,
    side: Option<&'a str>,
    line: Option<u32>,
    start_side: Option<&'a str>,
    start_line: Option<u32>,
    body: &'a str,
}

/// Whether GitHub can place the thread in `files` (the diff it resolves
/// against). Measured on real GitHub (DESIGN.md §3): only lines inside a
/// hunk; a range may span hunks but not run backwards.
fn placeable(files: &[FileChange], t: &NewThread) -> bool {
    let Some(f) = files.iter().find(|f| f.path == t.path) else { return false };
    if t.subject == "FILE" {
        return true;
    }
    let side = t.side.unwrap_or("RIGHT");
    let (Some(line), Some(patch)) = (t.line, f.patch.as_deref()) else { return false };
    if hunk_of(patch, side, line).is_none() {
        return false;
    }
    match t.start_line {
        None => true,
        Some(start) => {
            let start_side = t.start_side.unwrap_or(side);
            hunk_of(patch, start_side, start).is_some() && (start_side != side || start <= line)
        }
    }
}

fn find_pr_mut<'a>(w: &'a mut World, pr_id: &str) -> Result<(String, &'a mut Pr), GqlError> {
    let (repo, number) = w.find_pr_by_node(pr_id).ok_or_else(|| not_found(pr_id))?;
    let pr = w.repos.get_mut(&repo).unwrap().prs.get_mut(&number).unwrap();
    Ok((repo, pr))
}

fn check_writable(w: &World, repo: &str) -> Result<(), GqlError> {
    if w.repos[repo].archived {
        return Err(GqlError::Forbidden("Repository was archived so is read-only.".into()));
    }
    Ok(())
}

fn new_thread(
    w: &mut World,
    repo: &str,
    number: u64,
    review_id: &str,
    commit: &str,
    t: &NewThread,
    author: &str,
) -> (String, String) {
    let tid = w.next_id("PRRT");
    let cid = w.next_id("PRRC");
    let now = w.tick();
    let hunk = match (t.side, t.line) {
        (Some(side), Some(line)) => w.diff_hunk_for(repo, number, t.path, side, line),
        (None, Some(line)) => w.diff_hunk_for(repo, number, t.path, "RIGHT", line),
        _ => String::new(),
    };
    let head = w.repos[repo].prs[&number].head_oid.clone();
    let mut thread = Thread {
        node_id: tid.clone(),
        path: t.path.into(),
        subject_type: t.subject.into(),
        side: if t.subject == "LINE" { Some(t.side.unwrap_or("RIGHT").into()) } else { None },
        line: t.line,
        start_side: t.start_line.map(|_| t.start_side.or(t.side).unwrap_or("RIGHT").into()),
        start_line: t.start_line,
        original_line: t.line,
        original_start_line: t.start_line,
        commit_oid: commit.into(),
        resolved: false,
        outdated: false,
        comments: vec![ReviewComment {
            node_id: cid.clone(),
            review_node_id: review_id.into(),
            author: author.into(),
            body: t.body.into(),
            created_at: now,
            commit_oid: commit.into(),
            original_commit_oid: commit.into(),
            diff_hunk: hunk,
            reply_to: None,
        }],
    };
    if commit != head {
        // A review on an older commit: GitHub shows the thread on the current
        // diff only if its lines survived.
        w.reanchor(repo, number, &mut thread, commit, &head);
    }
    w.repos.get_mut(repo).unwrap().prs.get_mut(&number).unwrap().threads.push(thread);
    (tid, cid)
}

fn add_review(w: &mut World, viewer: &str, vars: &Value) -> GqlResult {
    let pr_id = s(vars, "prId");
    let commit = s(vars, "commit").to_string();
    let (repo, pr) = find_pr_mut(w, pr_id)?;
    let (number, pr_snapshot) = (pr.number, pr.clone());
    check_writable(w, &repo)?;
    if pr_snapshot.reviews.iter().any(|r| r.state == "PENDING" && r.author == viewer) {
        return Err(GqlError::Unprocessable("User can only have one pending review per pull request".into()));
    }
    let r = &w.repos[&repo];
    let base_tip = &r.branches[&pr_snapshot.base_ref];
    let reaches = |head: &str| commit == head || r.pr_commits(base_tip, head).iter().any(|c| c.oid == commit);
    let in_pr = reaches(&pr_snapshot.head_oid)
        || (!w.refuse_force_pushed_review_commits && pr_snapshot.past_heads.iter().any(|h| reaches(h)));
    if !in_pr {
        return Err(GqlError::Validation("The commitOID is not part of the pull request".into()));
    }
    let files = diff_at(w, &repo, &pr_snapshot, &commit);
    let threads: Vec<Value> = vars.get("threads").and_then(|t| t.as_array()).cloned().unwrap_or_default();
    let parsed: Vec<NewThread> = threads
        .iter()
        .map(|t| NewThread {
            path: s(t, "path"),
            subject: "LINE",
            side: opt_str(t, "side"),
            line: opt_u32(t, "line"),
            start_side: opt_str(t, "startSide"),
            start_line: opt_u32(t, "startLine"),
            body: s(t, "body"),
        })
        .collect();
    // Threads in the create call are placed on the review's own commit.
    for t in &parsed {
        if t.body.trim().is_empty() {
            return Err(GqlError::Unprocessable("Body can't be blank".into()));
        }
        if !placeable(&files, t) {
            return Err(GqlError::Unprocessable("Line could not be resolved".into()));
        }
    }
    let review_id = w.next_id("PRR");
    let now = w.tick();
    w.pr(&repo, number).reviews.push(Review {
        node_id: review_id.clone(),
        author: viewer.into(),
        state: "PENDING".into(),
        body: String::new(),
        commit_oid: commit.clone(),
        submitted_at: None,
    });
    let _ = now;
    let mut comments = Vec::new();
    for t in &parsed {
        let (_, cid) = new_thread(w, &repo, number, &review_id, &commit, t, viewer);
        comments.push(json!({
            "id": cid, "path": t.path, "line": t.line, "startLine": t.start_line, "body": t.body,
        }));
    }
    Ok(json!({ "addPullRequestReview": { "pullRequestReview": {
        "id": review_id, "state": "PENDING", "comments": { "nodes": comments },
    }}}))
}

fn pending_review<'a>(
    w: &'a World,
    viewer: &str,
    review_id: &str,
) -> Result<(String, u64, &'a Review), GqlError> {
    let (repo, number, i) = w.find_review(review_id).ok_or_else(|| not_found(review_id))?;
    let rv = &w.repos[&repo].prs[&number].reviews[i];
    if rv.author != viewer {
        return Err(not_found(review_id));
    }
    if rv.state != "PENDING" {
        return Err(GqlError::Unprocessable("The review has already been submitted".into()));
    }
    Ok((repo, number, rv))
}

/// Adds one thread to a pending review. Unlike threads in the create call,
/// GitHub places it on the PR's *current* head, whatever commit the review
/// is on, and answers `thread: null` (no error) if it can't place it.
fn add_thread(w: &mut World, viewer: &str, vars: &Value) -> GqlResult {
    let review_id = s(vars, "reviewId");
    let (repo, number, _) = pending_review(w, viewer, review_id)?;
    check_writable(w, &repo)?;
    let pr = w.repos[&repo].prs[&number].clone();
    let t = NewThread {
        path: s(vars, "path"),
        subject: opt_str(vars, "subjectType").unwrap_or("LINE"),
        side: opt_str(vars, "side"),
        line: opt_u32(vars, "line"),
        start_side: opt_str(vars, "startSide"),
        start_line: opt_u32(vars, "startLine"),
        body: s(vars, "body"),
    };
    if t.subject == "LINE" && t.line.is_none() {
        return Err(GqlError::Validation("Line required for for line-level threads".into()));
    }
    if t.body.trim().is_empty() {
        return Err(GqlError::Unprocessable("Body can't be blank".into()));
    }
    let files = diff_at(w, &repo, &pr, &pr.head_oid);
    if !placeable(&files, &t) {
        return Ok(json!({ "addPullRequestReviewThread": { "thread": null } }));
    }
    let (tid, cid) = new_thread(w, &repo, number, review_id, &pr.head_oid, &t, viewer);
    Ok(
        json!({ "addPullRequestReviewThread": { "thread": { "id": tid, "comments": { "nodes": [ { "id": cid } ] } } } }),
    )
}

fn add_reply(w: &mut World, viewer: &str, vars: &Value) -> GqlResult {
    let review_id = s(vars, "reviewId");
    let thread_id = s(vars, "threadId");
    let body = s(vars, "body").to_string();
    let (repo, number, rv) = pending_review(w, viewer, review_id)?;
    let commit = rv.commit_oid.clone();
    check_writable(w, &repo)?;
    let pr = &w.repos[&repo].prs[&number];
    let Some(ti) = pr.threads.iter().position(|t| t.node_id == thread_id) else {
        return Err(not_found(thread_id));
    };
    if pr.locked {
        return Err(GqlError::Forbidden("Conversation is locked".into()));
    }
    if body.trim().is_empty() {
        return Err(GqlError::Unprocessable("Body can't be blank".into()));
    }
    let root = pr.threads[ti].comments[0].node_id.clone();
    let hunk = pr.threads[ti].comments[0].diff_hunk.clone();
    let cid = w.next_id("PRRC");
    let now = w.tick();
    w.pr(&repo, number).threads[ti].comments.push(ReviewComment {
        node_id: cid.clone(),
        review_node_id: review_id.into(),
        author: viewer.into(),
        body,
        created_at: now,
        commit_oid: commit.clone(),
        original_commit_oid: commit,
        diff_hunk: hunk,
        reply_to: Some(root),
    });
    Ok(json!({ "addPullRequestReviewThreadReply": { "comment": { "id": cid } } }))
}

fn submit_review(w: &mut World, viewer: &str, vars: &Value) -> GqlResult {
    let review_id = s(vars, "reviewId");
    let event = s(vars, "event");
    let body = opt_str(vars, "body").unwrap_or("").to_string();
    let (repo, number, _) = pending_review(w, viewer, review_id)?;
    check_writable(w, &repo)?;
    let pr = &w.repos[&repo].prs[&number];
    let has_comments = pr.threads.iter().any(|t| t.comments.iter().any(|c| c.review_node_id == review_id));
    let state = match event {
        "APPROVE" => {
            if pr.author == viewer {
                return Err(GqlError::Unprocessable("Can not approve your own pull request".into()));
            }
            "APPROVED"
        }
        "REQUEST_CHANGES" => {
            if pr.author == viewer {
                return Err(GqlError::Unprocessable(
                    "Can not request changes on your own pull request".into(),
                ));
            }
            if body.trim().is_empty() {
                return Err(GqlError::Unprocessable(
                    "Review body is required when requesting changes".into(),
                ));
            }
            "CHANGES_REQUESTED"
        }
        "COMMENT" => {
            if body.trim().is_empty() && !has_comments {
                return Err(GqlError::Unprocessable("Review body is required".into()));
            }
            "COMMENTED"
        }
        other => return Err(GqlError::Unprocessable(format!("Unknown event {other}"))),
    };
    let now = w.tick();
    let url = format!("https://github.com/{repo}/pull/{number}#pullrequestreview-{review_id}");
    let pr = w.pr(&repo, number);
    let rv = pr.reviews.iter_mut().find(|r| r.node_id == review_id).unwrap();
    rv.state = state.into();
    rv.body = body;
    rv.submitted_at = Some(now.clone());
    pr.updated_at = now.clone();
    Ok(json!({ "submitPullRequestReview": { "pullRequestReview": {
        "id": review_id, "state": state, "url": url, "submittedAt": now,
    }}}))
}

fn delete_review(w: &mut World, viewer: &str, vars: &Value) -> GqlResult {
    let review_id = s(vars, "reviewId").to_string();
    let (repo, number, _) = pending_review(w, viewer, &review_id)?;
    let pr = w.pr(&repo, number);
    pr.reviews.retain(|r| r.node_id != review_id);
    for t in pr.threads.iter_mut() {
        t.comments.retain(|c| c.review_node_id != review_id);
    }
    pr.threads.retain(|t| !t.comments.is_empty());
    Ok(json!({ "deletePullRequestReview": { "pullRequestReview": { "id": review_id } } }))
}

fn pending_reviews(w: &mut World, viewer: &str, vars: &Value) -> GqlResult {
    let pr_id = s(vars, "prId");
    let (repo, number) = w.find_pr_by_node(pr_id).ok_or_else(|| not_found(pr_id))?;
    let nodes: Vec<Value> = w.repos[&repo].prs[&number]
        .reviews
        .iter()
        .filter(|r| r.state == "PENDING" && r.author == viewer)
        .map(|r| json!({ "id": r.node_id, "author": { "login": r.author } }))
        .collect();
    Ok(json!({ "node": { "reviews": { "nodes": nodes } } }))
}

fn review_comments(w: &mut World, viewer: &str, vars: &Value) -> GqlResult {
    let review_id = s(vars, "reviewId");
    let (repo, number, i) = w.find_review(review_id).ok_or_else(|| not_found(review_id))?;
    let pr = &w.repos[&repo].prs[&number];
    let rv = &pr.reviews[i];
    if rv.state == "PENDING" && rv.author != viewer {
        return Err(not_found(review_id));
    }
    let nodes: Vec<Value> = pr
        .threads
        .iter()
        .flat_map(|t| t.comments.iter().map(move |c| (t, c)))
        .filter(|(_, c)| c.review_node_id == review_id)
        .map(|(t, c)| {
            let mut v = comment_json(pr, c);
            v["path"] = json!(t.path);
            v["line"] = json!(t.original_line);
            v["startLine"] = json!(t.original_start_line);
            v["subjectType"] = json!(t.subject_type);
            v["replyTo"] = match &c.reply_to {
                Some(id) => json!({ "id": id }),
                None => Value::Null,
            };
            v
        })
        .collect();
    Ok(json!({ "node": {
        "id": rv.node_id, "state": rv.state,
        "comments": { "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": nodes },
    }}))
}

fn review_state(w: &mut World, viewer: &str, vars: &Value) -> GqlResult {
    let review_id = s(vars, "reviewId");
    let (repo, number, i) = w.find_review(review_id).ok_or_else(|| not_found(review_id))?;
    let rv = &w.repos[&repo].prs[&number].reviews[i];
    if rv.state == "PENDING" && rv.author != viewer {
        return Err(not_found(review_id));
    }
    Ok(json!({ "node": {
        "id": rv.node_id, "state": rv.state, "submittedAt": rv.submitted_at,
        "url": format!("https://github.com/{repo}/pull/{number}#pullrequestreview-{}", rv.node_id),
    }}))
}

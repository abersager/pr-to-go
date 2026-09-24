//! The outbox against real GitHub. Each test opens a pull request in a
//! repository you can push to, posts reviews on it, and closes it again (a
//! failing test leaves it open to look at). Ignored by default; run it only
//! against a throwaway repository:
//!
//! ```sh
//! GITHUB_TOKEN=$(gh auth token) LIVE_WRITE_REPO=you/throwaway \
//!   cargo test -p pr-to-go-core --test live_write -- --ignored --nocapture
//! ```
//!
//! The pull requests are the token user's own, so reviews go out as comments:
//! GitHub doesn't let authors approve or request changes.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pr_to_go_core::auth::{MemorySecretStore, SecretStore};
use pr_to_go_core::diff::Side;
use pr_to_go_core::drafts::{CommentKind, NewComment, SubjectType, Verdict};
use pr_to_go_core::outbox::{CommentResolution, Reason, Resolution};
use pr_to_go_core::{Core, CoreOptions};
use reqwest::Method;
use serde_json::{Value, json};

// ─── GitHub, directly: to set up, and to check what arrived ──────────────

struct Gh {
    http: reqwest::Client,
    token: String,
    owner: String,
    name: String,
}

impl Gh {
    fn from_env() -> Gh {
        let token = std::env::var("GITHUB_TOKEN").expect("set GITHUB_TOKEN");
        let repo = std::env::var("LIVE_WRITE_REPO").expect("set LIVE_WRITE_REPO to a throwaway owner/repo");
        let (owner, name) = repo.split_once('/').expect("LIVE_WRITE_REPO is owner/repo");
        Gh { http: reqwest::Client::new(), token, owner: owner.into(), name: name.into() }
    }

    async fn rest(&self, method: Method, path: &str, body: Option<Value>) -> Value {
        let url = format!("https://api.github.com/repos/{}/{}/{path}", self.owner, self.name);
        let mut req = self
            .http
            .request(method.clone(), &url)
            .bearer_auth(&self.token)
            .header("User-Agent", "pr-to-go-live-test")
            .header("Accept", "application/vnd.github+json");
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.unwrap();
        let status = resp.status();
        let text = resp.text().await.unwrap();
        assert!(status.is_success(), "{method} {path}: {status} {text}");
        if text.is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap() }
    }

    /// The whole response, errors included.
    async fn graphql(&self, query: &str, vars: Value) -> Value {
        self.http
            .post("https://api.github.com/graphql")
            .bearer_auth(&self.token)
            .header("User-Agent", "pr-to-go-live-test")
            .json(&json!({ "query": query, "variables": vars }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn data(&self, query: &str, vars: Value) -> Value {
        let v = self.graphql(query, vars).await;
        assert!(v.get("errors").is_none(), "{v}");
        v["data"].clone()
    }

    /// A commit on `parent` with `files` written over its tree.
    async fn commit(&self, parent: &str, files: &[(&str, &str)], message: &str) -> String {
        let parent_tree =
            self.rest(Method::GET, &format!("git/commits/{parent}"), None).await["tree"]["sha"].clone();
        let entries: Vec<Value> = files
            .iter()
            .map(|(path, text)| json!({ "path": path, "mode": "100644", "type": "blob", "content": text }))
            .collect();
        let tree = self
            .rest(Method::POST, "git/trees", Some(json!({ "base_tree": parent_tree, "tree": entries })))
            .await["sha"]
            .clone();
        let c = json!({ "message": message, "tree": tree, "parents": [parent] });
        self.rest(Method::POST, "git/commits", Some(c)).await["sha"].as_str().unwrap().to_string()
    }
}

fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

/// `numbered(60)` with some lines replaced and some added at the top.
fn edited(top: &[&str], replace: &[(usize, &str)]) -> String {
    let mut out: String = top.iter().map(|l| format!("{l}\n")).collect();
    for i in 1..=60 {
        match replace.iter().find(|(n, _)| *n == i) {
            Some((_, with)) => out.push_str(&format!("{with}\n")),
            None => out.push_str(&format!("line {i}\n")),
        }
    }
    out
}

/// A pull request of the test's own: `src/lib.rs` with 60 numbered lines on
/// the base; the head edits lines 5 and 50 and adds `src/new.rs`.
struct LivePr {
    gh: Gh,
    number: u64,
    node_id: String,
    url: String,
    base: String,
    head: String,
    base_branch: String,
    head_branch: String,
}

impl LivePr {
    async fn open(name: &str) -> LivePr {
        let gh = Gh::from_env();
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let main = gh.rest(Method::GET, "git/ref/heads/main", None).await["object"]["sha"]
            .as_str()
            .unwrap()
            .to_string();
        let base = gh.commit(&main, &[("src/lib.rs", &numbered(60))], "Sixty lines").await;
        let lib = edited(&[], &[(5, "line five (edited)"), (50, "line fifty (edited)")]);
        let head = gh
            .commit(
                &base,
                &[("src/lib.rs", &lib), ("src/new.rs", "pub fn new() {}\n")],
                "Edit lines 5 and 50",
            )
            .await;
        let base_branch = format!("live/{stamp}-{name}/base");
        let head_branch = format!("live/{stamp}-{name}/head");
        for (branch, sha) in [(&base_branch, &base), (&head_branch, &head)] {
            let r = json!({ "ref": format!("refs/heads/{branch}"), "sha": sha });
            gh.rest(Method::POST, "git/refs", Some(r)).await;
        }
        let body =
            "Opened by `crates/core/tests/live_write.rs` in pr-to-go. It's closed when the test passes.";
        let pr = gh
            .rest(
                Method::POST,
                "pulls",
                Some(json!({ "title": format!("Live test: {name}"), "head": head_branch, "base": base_branch, "body": body })),
            )
            .await;
        let url = pr["html_url"].as_str().unwrap().to_string();
        println!("{name}: {url}");
        LivePr {
            number: pr["number"].as_u64().unwrap(),
            node_id: pr["node_id"].as_str().unwrap().into(),
            url,
            base,
            head,
            base_branch,
            head_branch,
            gh,
        }
    }

    /// Replaces the head with a new commit on the base (a force-push), and
    /// waits until GitHub's pull request shows it.
    async fn force_push(&mut self, lib: &str) -> String {
        let sha = self.gh.commit(&self.base, &[("src/lib.rs", lib)], "Rework").await;
        let r = json!({ "sha": sha, "force": true });
        self.gh.rest(Method::PATCH, &format!("git/refs/heads/{}", self.head_branch), Some(r)).await;
        for _ in 0..60 {
            if self.head_oid().await == sha {
                self.head = sha.clone();
                return sha;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        panic!("the pull request never showed the force-push");
    }

    async fn head_oid(&self) -> String {
        let q = "query($o:String!,$n:String!,$num:Int!){repository(owner:$o,name:$n){pullRequest(number:$num){headRefOid}}}";
        let v = self.gh.data(q, self.vars()).await;
        v["repository"]["pullRequest"]["headRefOid"].as_str().unwrap().to_string()
    }

    fn vars(&self) -> Value {
        json!({ "o": self.gh.owner, "n": self.gh.name, "num": self.number })
    }

    /// Every review comment on the pull request, as GitHub has it.
    async fn posted(&self) -> Vec<Posted> {
        let q = "query($o:String!,$n:String!,$num:Int!){repository(owner:$o,name:$n){pullRequest(number:$num){
            reviewThreads(first:100){nodes{id path subjectType diffSide startDiffSide originalLine originalStartLine
              comments(first:50){nodes{body replyTo{id} pullRequestReview{id state} originalCommit{oid}}}}}}}}";
        let v = self.gh.data(q, self.vars()).await;
        let mut out = Vec::new();
        for t in v["repository"]["pullRequest"]["reviewThreads"]["nodes"].as_array().unwrap() {
            for c in t["comments"]["nodes"].as_array().unwrap() {
                out.push(Posted {
                    thread: t["id"].as_str().unwrap().into(),
                    review: c["pullRequestReview"]["id"].as_str().unwrap_or_default().into(),
                    pending: c["pullRequestReview"]["state"] == "PENDING",
                    body: c["body"].as_str().unwrap().trim().into(),
                    path: t["path"].as_str().unwrap().into(),
                    file: t["subjectType"] == "FILE",
                    side: t["diffSide"].as_str().map(str::to_owned),
                    line: t["originalLine"].as_u64(),
                    start_side: t["startDiffSide"].as_str().map(str::to_owned),
                    start_line: t["originalStartLine"].as_u64(),
                    commit: c["originalCommit"]["oid"].as_str().unwrap_or_default().into(),
                    reply: !c["replyTo"].is_null(),
                });
            }
        }
        out
    }

    /// The viewer's reviews: (id, state, commit).
    async fn reviews(&self) -> Vec<(String, String, String)> {
        let q =
            "query($o:String!,$n:String!,$num:Int!){repository(owner:$o,name:$n){pullRequest(number:$num){
            reviews(first:50){nodes{id state viewerDidAuthor commit{oid}}}}}}";
        let v = self.gh.data(q, self.vars()).await;
        v["repository"]["pullRequest"]["reviews"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["viewerDidAuthor"] == true)
            .map(|r| {
                (
                    r["id"].as_str().unwrap().into(),
                    r["state"].as_str().unwrap().into(),
                    r["commit"]["oid"].as_str().unwrap_or_default().into(),
                )
            })
            .collect()
    }

    async fn close(self) {
        self.gh
            .rest(Method::PATCH, &format!("pulls/{}", self.number), Some(json!({ "state": "closed" })))
            .await;
        for b in [&self.head_branch, &self.base_branch] {
            self.gh.rest(Method::DELETE, &format!("git/refs/heads/{b}"), None).await;
        }
    }
}

#[derive(Debug, Clone)]
struct Posted {
    thread: String,
    review: String,
    pending: bool,
    body: String,
    path: String,
    file: bool,
    side: Option<String>,
    line: Option<u64>,
    start_side: Option<String>,
    start_line: Option<u64>,
    commit: String,
    reply: bool,
}

// ─── The app's core ──────────────────────────────────────────────────────

struct App {
    core: Arc<Core>,
    dir: tempfile::TempDir,
    secrets: Arc<MemorySecretStore>,
}

impl App {
    async fn new() -> App {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemorySecretStore::default());
        let core = Self::open(&dir, &secrets);
        core.sign_in(&std::env::var("GITHUB_TOKEN").unwrap(), "pat").await.unwrap();
        App { core, dir, secrets }
    }

    fn open(dir: &tempfile::TempDir, secrets: &Arc<MemorySecretStore>) -> Arc<Core> {
        let s: Arc<dyn SecretStore> = secrets.clone();
        Core::open(CoreOptions::new(dir.path().to_owned(), s)).unwrap()
    }

    /// Simulates a restart on the same data directory.
    fn restart(&mut self) {
        self.core = Self::open(&self.dir, &self.secrets);
    }

    async fn add(&self, pr: &LivePr) -> (i64, i64) {
        let id = self.core.add_pr(&pr.url).await.unwrap();
        let rev = self.core.get_pr(id).unwrap().revision.unwrap().id;
        (id, rev)
    }

    /// Adds a draft comment and returns its id (the newest; the draft lists
    /// comments by file and line).
    fn comment(&self, pr: i64, n: NewComment) -> i64 {
        self.core.add_draft_comment(pr, n).unwrap().comments.iter().map(|c| c.id).max().unwrap()
    }

    fn latest(&self, pr: i64, column: &str) -> Option<String> {
        self.core
            .db()
            .read(|c| {
                Ok(c.query_row(
                    &format!("SELECT {column} FROM draft_review WHERE pr_id = ?1 ORDER BY id DESC LIMIT 1"),
                    [pr],
                    |r| r.get(0),
                )?)
            })
            .unwrap()
    }

    fn reasons(&self, pr: i64) -> Vec<Reason> {
        let raw = self.latest(pr, "attention").unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        serde_json::from_value(v["reasons"].clone()).unwrap()
    }

    /// Runs the outbox until the review settles, skipping backoff waits.
    async fn drain(&self, pr: i64) -> String {
        for _ in 0..40 {
            if let Err(e) = self.core.run_outbox_once().await {
                println!("  outbox: {e}");
            }
            let s = self.latest(pr, "status").unwrap();
            if s == "needs_attention" {
                println!("  needs attention: {:?}", self.reasons(pr));
            }
            if !matches!(s.as_str(), "queued" | "preflight" | "staging" | "submitting") {
                return s;
            }
            println!("  {s}: {:?}", self.latest(pr, "last_error"));
            self.core.retry_review(pr).unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        panic!("the outbox didn't settle");
    }

    /// Queues the draft, crashes right after `point` succeeds on GitHub,
    /// restarts, and sends the rest.
    async fn send_with_crash(&mut self, pr: i64, point: &str) {
        self.core.queue_review(pr).unwrap();
        self.core.set_crash_point(Some(point));
        let mut crashed = false;
        for _ in 0..20 {
            match self.core.run_outbox_once().await {
                Err(e) if e.to_string().contains("simulated crash") => {
                    crashed = true;
                    break;
                }
                _ => self.core.retry_review(pr).unwrap(),
            }
        }
        assert!(crashed, "{point} never happened");
        self.restart();
        assert_eq!(self.drain(pr).await, "submitted", "after a crash at {point}");
    }

    fn submitted_review(&self, pr: i64) -> String {
        self.latest(pr, "submitted_review_node_id").expect("a submitted review")
    }
}

fn line(rev: i64, side: Side, line: u32, body: &str) -> NewComment {
    NewComment {
        revision_id: rev,
        kind: CommentKind::Thread,
        subject_type: SubjectType::Line,
        path: Some("src/lib.rs".into()),
        side: Some(side),
        line: Some(line),
        start_side: None,
        start_line: None,
        reply_to_thread: None,
        body: body.into(),
    }
}

fn range(rev: i64, start: (Side, u32), end: u32, body: &str) -> NewComment {
    NewComment { start_side: Some(start.0), start_line: Some(start.1), ..line(rev, Side::Right, end, body) }
}

fn file(rev: i64, path: &str, body: &str) -> NewComment {
    NewComment {
        subject_type: SubjectType::File,
        path: Some(path.into()),
        side: None,
        line: None,
        ..line(rev, Side::Right, 0, body)
    }
}

fn reply(rev: i64, thread: &str, body: &str) -> NewComment {
    NewComment {
        kind: CommentKind::Reply,
        reply_to_thread: Some(thread.into()),
        path: None,
        side: None,
        line: None,
        ..line(rev, Side::Right, 0, body)
    }
}

fn resolution(id: i64, action: &str) -> CommentResolution {
    CommentResolution {
        id,
        action: action.into(),
        side: None,
        line: None,
        start_side: None,
        start_line: None,
        path: None,
    }
}

/// A comment as expected on GitHub: body, file, (side, line), (start side,
/// start line).
type Want<'a> = (&'a str, &'a str, Option<(&'a str, u64)>, Option<(&'a str, u64)>);

/// The review's comments on GitHub are exactly `want`, each once.
fn assert_review(posted: &[Posted], review: &str, want: &[Want]) {
    let mine: Vec<&Posted> = posted.iter().filter(|p| p.review == review).collect();
    assert_eq!(mine.len(), want.len(), "comments in {review}: {mine:#?}");
    for (body, path, at, start) in want {
        let hits: Vec<&&Posted> = mine.iter().filter(|p| p.body == body.trim()).collect();
        assert_eq!(hits.len(), 1, "{body:?} once in {mine:#?}");
        let p = hits[0];
        assert_eq!(p.path, *path, "{body:?}");
        match at {
            Some((side, line)) => {
                assert!(!p.file, "{body:?} is a line comment");
                assert_eq!((p.side.as_deref(), p.line), (Some(*side), Some(*line)), "{body:?}");
            }
            None => assert!(p.file || p.reply, "{body:?} is a file comment or reply"),
        }
        let got_start = p.start_line.map(|l| (p.start_side.clone().unwrap_or_default(), l));
        assert_eq!(got_start, start.map(|(s, l)| (s.to_string(), l)), "{body:?} start");
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn reviews_reach_github_exactly_once() {
    let live = LivePr::open("outbox").await;
    let mut app = App::new().await;
    let (pr, rev) = app.add(&live).await;

    // 1. Every kind of comment in one review.
    let first = "Why this name? ünïcødé ✓\n\nSecond paragraph.\n";
    app.comment(pr, line(rev, Side::Right, 5, first));
    app.comment(pr, range(rev, (Side::Right, 3), 6, "These four lines"));
    app.comment(pr, line(rev, Side::Left, 5, "The old name was clearer"));
    app.comment(pr, range(rev, (Side::Left, 4), 7, "From old line 4 to new line 7"));
    app.comment(pr, line(rev, Side::Right, 6, "```suggestion\nline six, suggested\n```"));
    app.comment(pr, file(rev, "src/new.rs", "Needs tests"));
    app.core
        .update_draft_review(pr, Some("Round 1: every kind of comment."), Some(Some(Verdict::Comment)))
        .unwrap();
    app.core.queue_review(pr).unwrap();
    assert_eq!(app.drain(pr).await, "submitted");
    let round1 = app.submitted_review(pr);
    let posted = live.posted().await;
    assert_review(
        &posted,
        &round1,
        &[
            (first, "src/lib.rs", Some(("RIGHT", 5)), None),
            ("These four lines", "src/lib.rs", Some(("RIGHT", 6)), Some(("RIGHT", 3))),
            ("The old name was clearer", "src/lib.rs", Some(("LEFT", 5)), None),
            ("From old line 4 to new line 7", "src/lib.rs", Some(("RIGHT", 7)), Some(("LEFT", 4))),
            ("```suggestion\nline six, suggested\n```", "src/lib.rs", Some(("RIGHT", 6)), None),
            ("Needs tests", "src/new.rs", None, None),
        ],
    );
    assert!(posted.iter().all(|p| p.commit == live.head), "all on the head commit");
    assert!(app.core.draft(pr).unwrap().is_none(), "the PR is free for a new draft");

    // 2. A reply and a new thread; crash right after the pending review is
    //    created (the reply goes in after it).
    let thread = posted.iter().find(|p| p.body == first.trim()).unwrap().thread.clone();
    app.core.sync_pr(pr).await.unwrap();
    app.comment(pr, reply(rev, &thread, "Replying to myself"));
    app.comment(pr, line(rev, Side::Right, 50, "About line fifty"));
    app.core.update_draft_review(pr, Some("Round 2: crash after create."), None).unwrap();
    app.send_with_crash(pr, "after_create_review").await;
    let round2 = app.submitted_review(pr);
    let posted = live.posted().await;
    assert_review(
        &posted,
        &round2,
        &[
            ("Replying to myself", "src/lib.rs", None, None),
            ("About line fifty", "src/lib.rs", Some(("RIGHT", 50)), None),
        ],
    );
    let r = posted.iter().find(|p| p.body == "Replying to myself").unwrap();
    assert!(r.reply && r.thread == thread, "the reply is in the first thread");

    // 3. Crash right after a comment added on its own.
    app.comment(pr, line(rev, Side::Right, 49, "About line 49"));
    app.comment(pr, file(rev, "src/lib.rs", "About the whole file"));
    app.core.update_draft_review(pr, Some("Round 3: crash after adding a comment."), None).unwrap();
    app.send_with_crash(pr, "after_add_thread").await;
    assert_review(
        &live.posted().await,
        &app.submitted_review(pr),
        &[
            ("About line 49", "src/lib.rs", Some(("RIGHT", 49)), None),
            ("About the whole file", "src/lib.rs", None, None),
        ],
    );

    // 4. Crash right after the submit went through.
    app.comment(pr, line(rev, Side::Right, 51, "About line 51"));
    app.core.update_draft_review(pr, Some("Round 4: crash after submit."), None).unwrap();
    app.send_with_crash(pr, "after_submit").await;
    assert_review(
        &live.posted().await,
        &app.submitted_review(pr),
        &[("About line 51", "src/lib.rs", Some(("RIGHT", 51)), None)],
    );
    assert!(app.latest(pr, "submitted_url").unwrap().starts_with("https://github.com/"));

    // 5. A line GitHub can't place, moved there past the app's own checks.
    //    GitHub refuses the batch, then answers `thread: null` for that one.
    app.comment(pr, line(rev, Side::Right, 48, "About line 48"));
    let bad = app.comment(pr, line(rev, Side::Right, 52, "Nowhere to go"));
    app.core
        .db()
        .write(|tx| Ok(tx.execute("UPDATE draft_comment SET line = 30 WHERE id = ?1", [bad])?))
        .unwrap();
    app.core.update_draft_review(pr, Some("Round 5: one comment GitHub can't place."), None).unwrap();
    app.core.queue_review(pr).unwrap();
    assert_eq!(app.drain(pr).await, "needs_attention");
    let reasons = app.reasons(pr);
    assert!(
        matches!(&reasons[0], Reason::CommentRejected { comment: Some(c), .. } if *c == bad),
        "{reasons:?}"
    );
    app.core
        .resolve_review(pr, Resolution { comments: vec![resolution(bad, "drop")], ..Default::default() })
        .unwrap();
    assert_eq!(app.drain(pr).await, "submitted");
    let posted = live.posted().await;
    assert_review(
        &posted,
        &app.submitted_review(pr),
        &[("About line 48", "src/lib.rs", Some(("RIGHT", 48)), None)],
    );

    // Five reviews, nothing left pending, and the app mirrors them.
    let reviews = live.reviews().await;
    assert_eq!(reviews.len(), 5, "{reviews:?}");
    assert!(reviews.iter().all(|r| r.1 == "COMMENTED"), "{reviews:?}");
    assert!(!posted.iter().any(|p| p.pending), "no pending comments left");
    app.core.sync_pr(pr).await.unwrap();
    let mirrored = app.core.get_pr(pr).unwrap();
    assert_eq!(mirrored.reviews.iter().filter(|r| r.state == "COMMENTED").count(), 5);
    live.close().await;
}

#[tokio::test]
#[ignore]
async fn reviews_follow_force_pushes() {
    let mut live = LivePr::open("force-push").await;
    let app = App::new().await;
    let (pr, rev1) = app.add(&live).await;

    // 1. The head moves under a queued review: the comments move with the
    //    code, and the review goes to the new head.
    let c5 = app.comment(pr, line(rev1, Side::Right, 5, "About line five"));
    let c50 = app.comment(pr, line(rev1, Side::Right, 50, "About line fifty"));
    let whole = app.comment(pr, file(rev1, "src/lib.rs", "About the whole file"));
    app.core.update_draft_review(pr, Some("Moved to the new head."), None).unwrap();
    app.core.queue_review(pr).unwrap();
    let head2 = live
        .force_push(&edited(
            &["new 1", "new 2", "new 3"],
            &[(5, "line five (edited)"), (50, "line fifty (edited)")],
        ))
        .await;
    assert_eq!(app.drain(pr).await, "needs_attention");
    assert!(matches!(app.reasons(pr)[0], Reason::HeadMoved { .. }), "{:?}", app.reasons(pr));
    let res = Resolution {
        comments: vec![resolution(c5, "remap"), resolution(c50, "remap"), resolution(whole, "to_file")],
        ..Default::default()
    };
    app.core.resolve_review(pr, res).unwrap();
    assert_eq!(app.drain(pr).await, "submitted");
    let moved = app.submitted_review(pr);
    let posted = live.posted().await;
    assert_review(
        &posted,
        &moved,
        &[
            ("About line five", "src/lib.rs", Some(("RIGHT", 8)), None),
            ("About line fifty", "src/lib.rs", Some(("RIGHT", 53)), None),
            ("About the whole file", "src/lib.rs", None, None),
        ],
    );
    assert!(posted.iter().filter(|p| p.review == moved).all(|p| p.commit == head2));

    // 2. Keep as written: the head is force-pushed again, and the review
    //    still goes to the commit it was written on, although that commit
    //    is no longer part of the pull request (DESIGN.md §3 (a)).
    let rev2 = app.core.get_pr(pr).unwrap().revision.unwrap().id;
    app.comment(pr, line(rev2, Side::Right, 53, "Keep me on line 53"));
    app.comment(pr, range(rev2, (Side::Right, 6), 9, "Keep this range"));
    app.core.update_draft_review(pr, Some("Kept on the reviewed commit."), None).unwrap();
    app.core.queue_review(pr).unwrap();
    live.force_push(&edited(&["top 1", "top 2", "top 3", "top 4", "top 5"], &[(5, "line five (again)")]))
        .await;
    assert_eq!(app.drain(pr).await, "needs_attention");
    let keep = Resolution { target_mode: Some("reviewed_commit".into()), ..Default::default() };
    app.core.resolve_review(pr, keep).unwrap();
    assert_eq!(app.drain(pr).await, "submitted");
    let kept = app.submitted_review(pr);
    let posted = live.posted().await;
    assert_review(
        &posted,
        &kept,
        &[
            ("Keep me on line 53", "src/lib.rs", Some(("RIGHT", 53)), None),
            ("Keep this range", "src/lib.rs", Some(("RIGHT", 9)), Some(("RIGHT", 6))),
        ],
    );
    assert!(posted.iter().filter(|p| p.review == kept).all(|p| p.commit == head2), "on the reviewed commit");
    let reviews = live.reviews().await;
    assert_eq!(reviews.iter().find(|r| r.0 == kept).unwrap().2, head2);
    live.close().await;
}

/// What the outbox relies on GitHub doing (DESIGN.md §3). If one of these
/// fails, GitHub changed, and the outbox may need to change with it.
#[tokio::test]
#[ignore]
async fn github_behaves_as_the_outbox_assumes() {
    let mut live = LivePr::open("api").await;
    let gh = &live.gh;
    let create = "mutation($pr:ID!,$c:GitObjectID!,$t:[DraftPullRequestReviewThread]){addPullRequestReview(input:{
        pullRequestId:$pr,commitOID:$c,threads:$t}){pullRequestReview{id comments(first:10){nodes{line startLine body}}}}}";
    let add = "mutation($r:ID!,$l:Int!,$b:String!){addPullRequestReviewThread(input:{pullRequestReviewId:$r,
        path:\"src/lib.rs\",line:$l,side:RIGHT,body:$b}){thread{id comments(first:1){nodes{originalCommit{oid}}}}}}";
    let delete =
        "mutation($r:ID!){deletePullRequestReview(input:{pullRequestReviewId:$r}){pullRequestReview{id}}}";
    let t =
        |line: u32, body: &str| json!({ "path": "src/lib.rs", "line": line, "side": "RIGHT", "body": body });

    // (b) Lines outside the hunks take no comments. In the create call
    // that's an error; on its own, an empty result without any error.
    let v = gh
        .graphql(create, json!({ "pr": live.node_id, "c": live.head, "t": [t(8, "in"), t(30, "out")] }))
        .await;
    assert_eq!(v["errors"][0]["type"], "UNPROCESSABLE", "{v}");
    assert!(live.reviews().await.is_empty(), "nothing created");
    let v = gh.data(create, json!({ "pr": live.node_id, "c": live.head, "t": [t(8, "single")] })).await;
    let review = v["addPullRequestReview"]["pullRequestReview"].clone();
    // Reconciling matches comments by line and start line: a single line
    // has no start line (unlike the thread's `startLine`, which is set).
    assert_eq!(review["comments"]["nodes"][0]["startLine"], Value::Null, "{review}");
    let rid = review["id"].as_str().unwrap().to_string();
    let v = gh.graphql(add, json!({ "r": rid, "l": 30, "b": "outside" })).await;
    assert!(v.get("errors").is_none() && v["data"]["addPullRequestReviewThread"]["thread"].is_null(), "{v}");
    gh.data(delete, json!({ "r": rid })).await;

    // (a) After a force-push, a review can still go to the old head, with
    // the threads in the create call on its lines. A thread added on its
    // own lands on the current head instead.
    let old = live.head.clone();
    let new = live.force_push(&edited(&["new 1", "new 2", "new 3"], &[(5, "line five (reworked)")])).await;
    let gh = &live.gh;
    let v = gh.data(create, json!({ "pr": live.node_id, "c": old, "t": [t(50, "on the old head")] })).await;
    let rid = v["addPullRequestReview"]["pullRequestReview"]["id"].as_str().unwrap().to_string();
    // New line 11 is in the new head's diff only; line 51 in the old one's only.
    let v = gh.data(add, json!({ "r": rid, "l": 11, "b": "added on its own" })).await;
    let commit = &v["addPullRequestReviewThread"]["thread"]["comments"]["nodes"][0]["originalCommit"]["oid"];
    assert_eq!(commit.as_str(), Some(new.as_str()), "{v}");
    let v = gh.data(add, json!({ "r": rid, "l": 51, "b": "old diff only" })).await;
    assert!(v["addPullRequestReviewThread"]["thread"].is_null(), "{v}");
    let posted = live.posted().await;
    let on_old = posted.iter().find(|p| p.body == "on the old head").unwrap();
    assert_eq!((on_old.commit.as_str(), on_old.line), (old.as_str(), Some(50)));
    gh.data(delete, json!({ "r": rid })).await;

    // A commit that isn't part of the pull request is refused, with a
    // message the outbox recognizes as "the reviewed commit is gone".
    let v = gh.graphql(create, json!({ "pr": live.node_id, "c": "0".repeat(40), "t": [] })).await;
    let msg = v["errors"][0]["message"].as_str().unwrap_or_default().to_lowercase();
    assert!(msg.contains("commit"), "{v}");
    assert!(live.reviews().await.is_empty(), "nothing left behind");
    live.close().await;
}

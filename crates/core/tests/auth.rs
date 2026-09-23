//! Picking up a new token from the gh CLI after GitHub rejects the old one
//! (DESIGN.md §5, "Authentication").

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use fake_github::{FakeGitHub, TOKEN, VIEWER};
use pr_to_go_core::auth::{MemorySecretStore, SecretStore};
use pr_to_go_core::clock::TestClock;
use pr_to_go_core::diff::Side;
use pr_to_go_core::drafts::{CommentKind, NewComment, SubjectType};
use pr_to_go_core::github::GhError;
use pr_to_go_core::{Core, CoreOptions, Error};

/// A core signed in with a token "from gh", whose gh CLI returns whatever
/// `gh_token` holds.
async fn gh_core(
    fake: &FakeGitHub,
    dir: &tempfile::TempDir,
) -> (Arc<Core>, Arc<Mutex<String>>, Arc<TestClock>) {
    let gh_token = Arc::new(Mutex::new(TOKEN.to_string()));
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
    let clock = Arc::new(TestClock::new());
    let mut opts = CoreOptions::new(dir.path().to_owned(), secrets);
    opts.github = config(fake.api_base());
    opts.clock = clock.clone();
    let t = gh_token.clone();
    opts.gh_cli = Arc::new(move || Ok(t.lock().unwrap().clone()));
    let core = Core::open(opts).unwrap();
    core.sign_in_with_gh().await.unwrap();
    (core, gh_token, clock)
}

/// gh logged in again: the old token is revoked, a new one works.
fn rotate(fake: &FakeGitHub, gh_token: &Mutex<String>, new: &str, owner: &str) {
    fake.with(|w| {
        w.tokens.remove(TOKEN);
        w.tokens.insert(new.into(), owner.into());
    });
    *gh_token.lock().unwrap() = new.into();
}

#[tokio::test]
async fn a_rotated_gh_token_is_picked_up_and_sync_carries_on() {
    let fake = FakeGitHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    let s = fake.with(seed);
    let (core, gh_token, _) = gh_core(&fake, &dir).await;
    let pr = core.add_pr(&pr_url(s.number)).await.unwrap();

    rotate(&fake, &gh_token, "gho_new", VIEWER);
    core.sync_pr(pr).await.expect("recovers with the new token");
    assert!(core.auth_status().unwrap().signed_in);
    assert_eq!(core.check_connectivity().await.detail, None);
}

#[tokio::test]
async fn a_gh_token_for_another_account_is_not_used() {
    let fake = FakeGitHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    let s = fake.with(seed);
    let (core, gh_token, _) = gh_core(&fake, &dir).await;
    let pr = core.add_pr(&pr_url(s.number)).await.unwrap();

    rotate(&fake, &gh_token, "gho_alice", "alice");
    let err = core.sync_pr(pr).await.unwrap_err();
    assert!(matches!(err, Error::GitHub(GhError::Unauthorized)), "{err:?}");
    // The old token is still the one we use.
    fake.with(|w| w.tokens.insert(TOKEN.into(), VIEWER.into()));
    core.sync_pr(pr).await.expect("still signed in as me");
}

#[tokio::test]
async fn a_review_stopped_by_a_401_goes_out_once_gh_has_a_new_token() {
    let fake = FakeGitHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    let s = fake.with(seed);
    let (core, gh_token, clock) = gh_core(&fake, &dir).await;
    let pr = core.add_pr(&pr_url(s.number)).await.unwrap();
    let rev = core.get_pr(pr).unwrap().revision.unwrap().id;
    core.add_draft_comment(
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
            body: "Why this name?".into(),
        },
    )
    .unwrap();
    core.queue_review(pr).unwrap();

    // The token is revoked and gh has nothing better yet: the review stops.
    fake.with(|w| w.tokens.remove(TOKEN));
    core.run_outbox_once().await.unwrap();
    assert_eq!(core.draft(pr).unwrap().unwrap().status, "needs_attention");

    // gh logs in again; the next outbox pass picks the token up and sends.
    fake.with(|w| w.tokens.insert("gho_new".into(), VIEWER.into()));
    *gh_token.lock().unwrap() = "gho_new".into();
    for _ in 0..5 {
        clock.advance(std::time::Duration::from_secs(600));
        core.run_outbox_once().await.unwrap();
    }
    let reviews = fake.with(|w| w.pr(REPO, s.number).reviews.iter().filter(|r| r.author == VIEWER).count());
    assert_eq!(reviews, 1);
}

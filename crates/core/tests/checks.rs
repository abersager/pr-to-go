//! Re-polling CI checks that are still running (DESIGN.md §9.2, step 6).

mod common;

use std::time::Duration;

use common::*;
use pr_to_go_core::checks::CheckPoller;

fn rollup(h: &Harness, pr_id: i64) -> Option<String> {
    h.core.get_pr(pr_id).unwrap().checks.and_then(|c| c.rollup_state)
}

fn polls(h: &Harness) -> usize {
    h.fake.log().iter().filter(|l| *l == "PrChecks").count()
}

async fn pending_pr(h: &Harness) -> (Seeded, i64) {
    let s = h.fake.with(|w| {
        let s = seed(w);
        w.set_checks(REPO, &s.head, &[("test", "IN_PROGRESS", None)]);
        s
    });
    let pr_id = h.core.add_pr(&pr_url(s.number)).await.unwrap();
    assert_eq!(rollup(h, pr_id).as_deref(), Some("PENDING"));
    h.fake.clear_log();
    (s, pr_id)
}

#[tokio::test]
async fn pending_checks_are_polled_until_they_finish() {
    let h = harness().await;
    let (s, pr_id) = pending_pr(&h).await;
    let mut poller = CheckPoller::default();

    // First seen: the first poll is 30 s away.
    assert_eq!(h.core.poll_pending_checks(&mut poller).await.unwrap(), Duration::from_secs(30));
    assert_eq!(polls(&h), 0);
    h.clock.advance(Duration::from_secs(30));
    h.core.poll_pending_checks(&mut poller).await.unwrap();
    assert_eq!(polls(&h), 1);
    assert_eq!(rollup(&h, pr_id).as_deref(), Some("PENDING"));

    h.fake.with(|w| w.set_checks(REPO, &s.head, &[("test", "COMPLETED", Some("SUCCESS"))]));
    h.clock.advance(Duration::from_secs(30));
    h.core.poll_pending_checks(&mut poller).await.unwrap();
    assert_eq!(rollup(&h, pr_id).as_deref(), Some("SUCCESS"));
    let checks = h.core.get_pr(pr_id).unwrap().checks.unwrap();
    assert!(checks.contexts.to_string().contains("SUCCESS"), "{}", checks.contexts);

    // Done: no more polling, and never a full sync.
    h.clock.advance(Duration::from_secs(600));
    h.core.poll_pending_checks(&mut poller).await.unwrap();
    assert_eq!(polls(&h), 2);
    assert!(!h.fake.log().iter().any(|l| l == "PullRequestDetails"));
}

#[tokio::test]
async fn polling_backs_off_and_gives_up() {
    let h = harness().await;
    pending_pr(&h).await;
    let mut poller = CheckPoller::default();
    // Run the loop for simulated hours, noting when each poll happens.
    let (mut now, mut at) = (0u64, Vec::new());
    for _ in 0..500 {
        let before = polls(&h);
        let wait = h.core.poll_pending_checks(&mut poller).await.unwrap();
        if polls(&h) > before {
            at.push(now);
        }
        h.clock.advance(wait);
        now += wait.as_secs();
    }
    let gaps: Vec<u64> = at.windows(2).map(|w| w[1] - w[0]).collect();
    assert_eq!(at[0], 30);
    assert_eq!(&gaps[..6], [30, 60, 120, 240, 300, 300]);
    assert_eq!(at.len(), 20, "gives up after 20 polls");
}

#[tokio::test]
async fn a_new_head_seen_while_polling_marks_the_pr_stale() {
    let h = harness().await;
    let (s, pr_id) = pending_pr(&h).await;
    h.fake.with(|w| {
        let c = w.commit(REPO, Some(&s.head), &[("src/new.rs", Some("pub fn newer() {}\n"))], "More");
        w.push(REPO, s.number, &c);
    });
    let mut poller = CheckPoller::default();
    h.core.poll_pending_checks(&mut poller).await.unwrap();
    h.clock.advance(Duration::from_secs(30));
    h.core.poll_pending_checks(&mut poller).await.unwrap();
    assert_eq!(h.core.get_pr(pr_id).unwrap().summary.sync_state, "stale");
}

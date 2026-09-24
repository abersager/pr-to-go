//! Browsing the open pull requests the user can reach.

mod common;

use common::*;
use fake_github::{PrState, VIEWER, World};

/// A repository with one open PR titled `title`. Returns the PR number.
fn pr_in(w: &mut World, repo: &str, title: &str, author: &str) -> u64 {
    if !w.repos.contains_key(repo) {
        w.create_repo(repo);
        let base = w.commit(repo, None, &[("README.md", Some("hi\n"))], "init");
        w.set_branch(repo, "main", &base);
    }
    let main = w.repo(repo).branches["main"].clone();
    let head = w.commit(repo, Some(&main), &[("README.md", Some(&format!("{title}\n")))], title);
    let branch = format!("b-{}", w.repo(repo).prs.len());
    w.open_pr(repo, "main", &branch, &head, title, "", author)
}

fn titles(page: &pr_to_go_core::browse::BrowsePage) -> Vec<&str> {
    page.prs.iter().map(|p| p.title.as_str()).collect()
}

#[tokio::test]
async fn lists_open_prs_from_own_org_and_shared_repos_only() {
    let h = harness().await;
    let s = h.fake.with(|w| {
        let s = seed(w); // acme/widgets: "Improve widgets", by alice
        w.join_org("acme", VIEWER);
        pr_in(w, "me/dotfiles", "Tidy zsh config", VIEWER);
        pr_in(w, "bob/tools", "Add a linter", "bob");
        w.add_collaborator("bob/tools", VIEWER);
        pr_in(w, "zed/elsewhere", "Not mine to see", "zed");
        let closed = pr_in(w, "acme/widgets", "Already closed", "bob");
        w.pr(REPO, closed).state = PrState::Closed;
        pr_in(w, "acme/attic", "In an archived repo", "bob");
        w.repo("acme/attic").archived = true;
        s
    });

    let page = h.core.browse_prs(None, None).await.unwrap();
    let mut got = titles(&page);
    got.sort();
    assert_eq!(got, ["Add a linter", "Improve widgets", "Tidy zsh config"]);
    assert_eq!(page.total, 3);
    assert_eq!(
        (page.scope.login.as_str(), page.scope.orgs.as_slice()),
        ("me", ["acme".to_string()].as_slice())
    );
    assert_eq!(page.scope.shared_repos, 1);
    assert!(page.cursor.is_none());
    // Newest first.
    let times: Vec<&str> = page.prs.iter().map(|p| p.updated_at.as_str()).collect();
    assert!(times.windows(2).all(|t| t[0] >= t[1]), "{times:?}");
    assert!(page.prs.iter().all(|p| p.local_id.is_none()));

    // Taking one offline shows in the list.
    let id = h.core.add_pr(&pr_url(s.number)).await.unwrap();
    let page = h.core.browse_prs(None, None).await.unwrap();
    let widgets = page.prs.iter().find(|p| p.title == "Improve widgets").unwrap();
    assert_eq!((widgets.local_id, widgets.offline), (Some(id), true));
}

#[tokio::test]
async fn filters_by_words_or_by_a_named_repo() {
    let h = harness().await;
    h.fake.with(|w| {
        seed(w);
        w.join_org("acme", VIEWER);
        pr_in(w, "bob/tools", "Add a linter", "bob");
        w.add_collaborator("bob/tools", VIEWER);
    });
    let page = h.core.browse_prs(Some("  improve "), None).await.unwrap();
    assert_eq!(titles(&page), ["Improve widgets"]);
    assert!(!page.scope.custom);

    let page = h.core.browse_prs(Some("repo:bob/tools"), None).await.unwrap();
    assert_eq!(titles(&page), ["Add a linter"]);
    assert!(page.scope.custom);
}

#[tokio::test]
async fn pages_through_more_orgs_than_fit_in_one_search() {
    let h = harness().await;
    h.fake.with(|w| {
        seed(w);
        // Too many qualifiers for one 256-character query.
        for i in 0..20 {
            let org = format!("organization-number-{i:02}");
            w.join_org(&org, VIEWER);
            pr_in(w, &format!("{org}/app"), &format!("Change {i:02}"), "bob");
        }
        w.page_size = Some(4);
    });
    let mut all = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page = h.core.browse_prs(None, cursor.as_deref()).await.unwrap();
        assert_eq!(page.total, 20);
        all.extend(page.prs.iter().map(|p| p.title.clone()));
        pages += 1;
        match page.cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert!(pages > 1);
    all.sort();
    let expected: Vec<String> = (0..20).map(|i| format!("Change {i:02}")).collect();
    assert_eq!(all, expected, "every org's PR exactly once");
}

#[tokio::test]
async fn browsing_offline_says_so() {
    let h = harness().await;
    h.fake.with(seed);
    h.fake.go_down();
    let err = h.core.browse_prs(None, None).await.unwrap_err();
    assert_eq!(err.kind(), "offline", "{err:?}");
}

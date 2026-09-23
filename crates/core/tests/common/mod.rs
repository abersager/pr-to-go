//! Shared setup for tests against the fake GitHub.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use fake_github::{FakeGitHub, TOKEN, World};
use pr_to_go_core::auth::{MemorySecretStore, SecretStore};
use pr_to_go_core::clock::TestClock;
use pr_to_go_core::github::GitHubConfig;
use pr_to_go_core::{Core, CoreOptions};

pub const REPO: &str = "acme/widgets";

pub struct Harness {
    pub fake: FakeGitHub,
    pub core: Arc<Core>,
    pub dir: tempfile::TempDir,
    pub secrets: Arc<MemorySecretStore>,
    pub clock: Arc<TestClock>,
}

pub fn config(base: &str) -> GitHubConfig {
    GitHubConfig {
        api_base: base.to_string(),
        request_timeout: Duration::from_millis(1500),
        mutation_spacing: Duration::ZERO,
        max_concurrency: 4,
        user_agent: "pr-to-go-tests".into(),
    }
}

pub async fn harness() -> Harness {
    let fake = FakeGitHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecretStore::default());
    let clock = Arc::new(TestClock::new());
    let core = open_core(&fake, &dir, secrets.clone(), clock.clone());
    core.sign_in(TOKEN, "pat").await.expect("sign in");
    Harness { fake, core, dir, secrets, clock }
}

pub fn open_core(
    fake: &FakeGitHub,
    dir: &tempfile::TempDir,
    secrets: Arc<MemorySecretStore>,
    clock: Arc<TestClock>,
) -> Arc<Core> {
    let secrets: Arc<dyn SecretStore> = secrets;
    let mut opts = CoreOptions::new(dir.path().to_owned(), secrets);
    opts.github = config(fake.api_base());
    opts.clock = clock;
    Core::open(opts).expect("open core")
}

impl Harness {
    /// Simulates an app restart on the same data directory.
    pub fn restart(&mut self) {
        self.core = open_core(&self.fake, &self.dir, self.secrets.clone(), self.clock.clone());
    }
}

/// 30 numbered lines, so edits and comments have room to move around.
pub fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

pub fn replace_line(text: &str, line: usize, with: &str) -> String {
    text.lines()
        .enumerate()
        .map(|(i, l)| if i + 1 == line { format!("{with}\n") } else { format!("{l}\n") })
        .collect()
}

pub const PNG_V1: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDRv1";
pub const PNG_V2: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDRv2-changed";

pub struct Seeded {
    pub number: u64,
    pub base: String,
    pub head: String,
}

/// A repo with `main` and one open PR touching several kinds of file:
/// a modified source file, an added file, a pure rename, a binary change and
/// a deletion. The PR has a description image, a review with a line comment,
/// an issue comment and a passing check.
pub fn seed(w: &mut World) -> Seeded {
    w.create_repo(REPO);
    let lib = numbered(30);
    let base = w.commit_bytes(
        REPO,
        None,
        &[
            ("src/lib.rs", Some(lib.as_bytes())),
            ("README.md", Some(b"# Widgets\n")),
            ("old/name.txt", Some(b"moved without changes\n")),
            ("logo.png", Some(PNG_V1)),
            ("CHANGELOG", Some(b"v1\n")),
        ],
        "initial",
    );
    w.set_branch(REPO, "main", &base);
    let lib2 = replace_line(&lib, 5, "line five (edited)");
    let lib2 = lib2.replace("line 20\n", "line 20\nline 20a (new)\nline 20b (new)\n");
    let head = w.commit_bytes(
        REPO,
        Some(&base),
        &[
            ("src/lib.rs", Some(lib2.as_bytes())),
            ("src/new.rs", Some(b"pub fn new() {}\n")),
            ("old/name.txt", None),
            ("new/name.txt", Some(b"moved without changes\n")),
            ("logo.png", Some(PNG_V2)),
            ("CHANGELOG", None),
        ],
        "Improve widgets\n\nLonger explanation.",
    );
    let img = format!("{}/assets/diagram.png", w.base_url);
    w.assets.insert("diagram.png".into(), ("image/png".into(), PNG_V1.to_vec()));
    let number = w.open_pr(
        REPO,
        "main",
        "feature",
        &head,
        "Improve widgets",
        &format!("Does things.\n\n![diagram]({img})"),
        "alice",
    );
    w.add_review(
        REPO,
        number,
        "bob",
        "COMMENTED",
        "Looks close.",
        &[("src/lib.rs", "RIGHT", 5, "Why this name?")],
    );
    w.add_issue_comment(REPO, number, "bob", "CI is green.");
    w.set_checks(REPO, &head, &[("test", "COMPLETED", Some("SUCCESS"))]);
    Seeded { number, base, head }
}

pub fn pr_url(number: u64) -> String {
    format!("https://github.com/{REPO}/pull/{number}")
}

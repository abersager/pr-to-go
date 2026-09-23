//! The fake's state: users, repos with real git blob hashes, commits as
//! path→blob trees, PRs with reviews, threads and comments.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::{Duration, UNIX_EPOCH};

use sha1::{Digest, Sha1};

use crate::faults::Fault;

pub fn git_blob_oid(bytes: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(format!("blob {}\0", bytes.len()).as_bytes());
    h.update(bytes);
    hex::encode(h.finalize())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

impl PrState {
    pub fn as_str(self) -> &'static str {
        match self {
            PrState::Open => "OPEN",
            PrState::Closed => "CLOSED",
            PrState::Merged => "MERGED",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub oid: String,
    pub parents: Vec<String>,
    pub tree: BTreeMap<String, String>,
    pub message: String,
    pub author: String,
    pub date: String,
}

#[derive(Clone, Debug)]
pub struct Check {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Review {
    pub node_id: String,
    pub author: String,
    /// PENDING | COMMENTED | APPROVED | CHANGES_REQUESTED | DISMISSED
    pub state: String,
    pub body: String,
    pub commit_oid: String,
    pub submitted_at: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ReviewComment {
    pub node_id: String,
    pub review_node_id: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub commit_oid: String,
    pub original_commit_oid: String,
    pub diff_hunk: String,
}

#[derive(Clone, Debug)]
pub struct Thread {
    pub node_id: String,
    pub path: String,
    /// LINE | FILE
    pub subject_type: String,
    pub side: Option<String>,
    pub line: Option<u32>,
    pub start_side: Option<String>,
    pub start_line: Option<u32>,
    pub original_line: Option<u32>,
    pub original_start_line: Option<u32>,
    pub commit_oid: String,
    pub resolved: bool,
    pub outdated: bool,
    pub comments: Vec<ReviewComment>,
}

#[derive(Clone, Debug)]
pub struct IssueComment {
    pub node_id: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Clone, Debug)]
pub struct Pr {
    pub number: u64,
    pub node_id: String,
    pub title: String,
    pub body: String,
    pub author: String,
    pub base_ref: String,
    pub head_ref: String,
    pub head_oid: String,
    pub state: PrState,
    pub is_draft: bool,
    pub locked: bool,
    pub created_at: String,
    pub updated_at: String,
    pub reviews: Vec<Review>,
    pub threads: Vec<Thread>,
    pub issue_comments: Vec<IssueComment>,
}

#[derive(Clone, Debug, Default)]
pub struct Repo {
    pub node_id: String,
    pub owner: String,
    pub name: String,
    pub archived: bool,
    pub blobs: HashMap<String, Vec<u8>>,
    pub commits: HashMap<String, Commit>,
    pub branches: BTreeMap<String, String>,
    pub prs: BTreeMap<u64, Pr>,
    pub checks: HashMap<String, Vec<Check>>,
}

/// One changed file in a PR diff.
#[derive(Clone, Debug)]
pub struct FileChange {
    pub path: String,
    pub prev_path: Option<String>,
    /// added | removed | modified | renamed
    pub status: &'static str,
    pub base_blob: Option<String>,
    pub head_blob: Option<String>,
    pub patch: Option<String>,
    pub additions: usize,
    pub deletions: usize,
}

pub struct World {
    pub base_url: String,
    pub tokens: HashMap<String, String>,
    pub users: BTreeMap<String, String>,
    pub repos: BTreeMap<String, Repo>,
    pub faults: Vec<Fault>,
    /// Every request handled: GraphQL operation names and REST paths.
    pub log: Vec<String>,
    /// Images served under `/assets/…`.
    pub assets: HashMap<String, (String, Vec<u8>)>,
    /// Caps GraphQL page sizes so tests exercise pagination with few items.
    pub page_size: Option<usize>,
    /// Patches bigger than this are left out, as GitHub does for large diffs.
    pub patch_limit: usize,
    /// Blob text longer than this comes back truncated from GraphQL.
    pub blob_text_limit: usize,
    seq: u64,
}

pub const TOKEN: &str = "test-token";
pub const VIEWER: &str = "me";

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> World {
        let mut w = World {
            base_url: String::new(),
            tokens: HashMap::new(),
            users: BTreeMap::new(),
            repos: BTreeMap::new(),
            faults: Vec::new(),
            log: Vec::new(),
            assets: HashMap::new(),
            page_size: None,
            patch_limit: 400_000,
            blob_text_limit: 512 * 1024,
            seq: 0,
        };
        for u in [VIEWER, "alice", "bob"] {
            w.add_user(u);
        }
        w.tokens.insert(TOKEN.into(), VIEWER.into());
        w
    }

    pub fn next_id(&mut self, prefix: &str) -> String {
        self.seq += 1;
        format!("{prefix}_kwDO{:06}", self.seq)
    }

    /// Server time. Each call moves the clock forward by a minute, so events
    /// get distinct, ordered timestamps.
    pub fn tick(&mut self) -> String {
        self.seq += 1;
        let t = UNIX_EPOCH + Duration::from_secs(1_780_000_000 + self.seq * 60);
        humantime::format_rfc3339_seconds(t).to_string()
    }

    pub fn add_user(&mut self, login: &str) {
        let id = format!("U_{login}");
        self.users.insert(login.into(), id);
    }

    pub fn create_repo(&mut self, full_name: &str) -> &mut Repo {
        let (owner, name) = full_name.split_once('/').expect("owner/name");
        let node_id = self.next_id("R");
        self.repos.entry(full_name.into()).or_insert_with(|| Repo {
            node_id,
            owner: owner.into(),
            name: name.into(),
            ..Default::default()
        })
    }

    pub fn repo(&mut self, full_name: &str) -> &mut Repo {
        self.repos.get_mut(full_name).unwrap_or_else(|| panic!("no repo {full_name}"))
    }

    /// Creates a commit on top of `parent` applying `changes` (path, new
    /// contents, `None` to delete). Returns the commit oid.
    pub fn commit(
        &mut self,
        repo: &str,
        parent: Option<&str>,
        changes: &[(&str, Option<&str>)],
        message: &str,
    ) -> String {
        let changes: Vec<(&str, Option<&[u8]>)> =
            changes.iter().map(|(p, c)| (*p, c.map(str::as_bytes))).collect();
        self.commit_bytes(repo, parent, &changes, message)
    }

    pub fn commit_bytes(
        &mut self,
        repo: &str,
        parent: Option<&str>,
        changes: &[(&str, Option<&[u8]>)],
        message: &str,
    ) -> String {
        let date = self.tick();
        let seq = self.seq;
        let r = self.repo(repo);
        let mut tree = parent.map(|p| r.commits[p].tree.clone()).unwrap_or_default();
        for (path, content) in changes {
            match content {
                Some(bytes) => {
                    let oid = git_blob_oid(bytes);
                    r.blobs.insert(oid.clone(), bytes.to_vec());
                    tree.insert((*path).into(), oid);
                }
                None => {
                    tree.remove(*path);
                }
            }
        }
        let mut h = Sha1::new();
        h.update(format!("{parent:?}{tree:?}{message}{seq}").as_bytes());
        let oid = hex::encode(h.finalize());
        r.commits.insert(
            oid.clone(),
            Commit {
                oid: oid.clone(),
                parents: parent.map(|p| vec![p.to_string()]).unwrap_or_default(),
                tree,
                message: message.into(),
                author: VIEWER.into(),
                date,
            },
        );
        oid
    }

    pub fn set_branch(&mut self, repo: &str, branch: &str, oid: &str) {
        self.repo(repo).branches.insert(branch.into(), oid.into());
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open_pr(
        &mut self,
        repo: &str,
        base: &str,
        head_branch: &str,
        head_oid: &str,
        title: &str,
        body: &str,
        author: &str,
    ) -> u64 {
        self.set_branch(repo, head_branch, head_oid);
        let node_id = self.next_id("PR");
        let now = self.tick();
        let r = self.repo(repo);
        let number = r.prs.keys().max().copied().unwrap_or(0) + 1;
        r.prs.insert(
            number,
            Pr {
                number,
                node_id,
                title: title.into(),
                body: body.into(),
                author: author.into(),
                base_ref: base.into(),
                head_ref: head_branch.into(),
                head_oid: head_oid.into(),
                state: PrState::Open,
                is_draft: false,
                locked: false,
                created_at: now.clone(),
                updated_at: now,
                reviews: vec![],
                threads: vec![],
                issue_comments: vec![],
            },
        );
        number
    }

    pub fn pr(&mut self, repo: &str, number: u64) -> &mut Pr {
        self.repo(repo).prs.get_mut(&number).expect("no such PR")
    }

    /// Moves the PR head (a normal push or a force-push; the fake doesn't
    /// care which). Existing threads are re-anchored or marked outdated the
    /// way GitHub does it: a thread survives only if its lines are unchanged
    /// and still part of the new diff.
    pub fn push(&mut self, repo: &str, number: u64, new_head: &str) {
        let now = self.tick();
        let r = self.repo(repo);
        let pr = r.prs[&number].clone();
        let old_head = pr.head_oid.clone();
        let base_tip = r.branches[&pr.base_ref].clone();
        let old_mb = r.merge_base(&base_tip, &old_head).expect("merge base");
        let new_mb = r.merge_base(&base_tip, new_head).expect("merge base");
        let new_files = r.diff(&new_mb, new_head, usize::MAX);
        let mut threads = pr.threads.clone();
        for t in threads.iter_mut().filter(|t| t.subject_type == "LINE" && !t.outdated) {
            let side = t.side.clone().unwrap_or_else(|| "RIGHT".into());
            let (from, to) =
                if side == "LEFT" { (&old_mb, &new_mb) } else { (&old_head, &new_head.to_string()) };
            let old_text = r.file_text(from, &t.path);
            let new_text = r.file_text(to, &t.path);
            let mapped = match (old_text, new_text, t.line) {
                (Some(a), Some(b), Some(line)) => map_line(&a, &b, line).filter(|nl| {
                    new_files
                        .iter()
                        .find(|f| f.path == t.path)
                        .is_some_and(|f| in_hunks(f.patch.as_deref(), &side, *nl))
                }),
                _ => None,
            };
            match mapped {
                Some(nl) => {
                    let delta = nl as i64 - t.line.unwrap() as i64;
                    t.line = Some(nl);
                    t.start_line = t.start_line.map(|s| (s as i64 + delta) as u32);
                    t.commit_oid = new_head.into();
                }
                None => {
                    t.outdated = true;
                    t.line = None;
                    t.start_line = None;
                }
            }
        }
        r.branches.insert(pr.head_ref.clone(), new_head.into());
        let p = r.prs.get_mut(&number).unwrap();
        p.head_oid = new_head.into();
        p.threads = threads;
        p.updated_at = now;
    }

    pub fn set_checks(&mut self, repo: &str, commit: &str, checks: &[(&str, &str, Option<&str>)]) {
        let checks = checks
            .iter()
            .map(|(n, s, c)| Check { name: (*n).into(), status: (*s).into(), conclusion: c.map(Into::into) })
            .collect();
        self.repo(repo).checks.insert(commit.into(), checks);
    }

    /// Adds a submitted review with optional line threads, as another user
    /// would. Returns the review node id.
    pub fn add_review(
        &mut self,
        repo: &str,
        number: u64,
        author: &str,
        state: &str,
        body: &str,
        threads: &[(&str, &str, u32, &str)],
    ) -> String {
        let review_id = self.next_id("PRR");
        let now = self.tick();
        let head = self.pr(repo, number).head_oid.clone();
        let mut new_threads = Vec::new();
        for (path, side, line, text) in threads {
            let tid = self.next_id("PRRT");
            let cid = self.next_id("PRRC");
            let hunk = self.diff_hunk_for(repo, number, path, side, *line);
            new_threads.push(Thread {
                node_id: tid,
                path: (*path).into(),
                subject_type: "LINE".into(),
                side: Some((*side).into()),
                line: Some(*line),
                start_side: None,
                start_line: None,
                original_line: Some(*line),
                original_start_line: None,
                commit_oid: head.clone(),
                resolved: false,
                outdated: false,
                comments: vec![ReviewComment {
                    node_id: cid,
                    review_node_id: review_id.clone(),
                    author: author.into(),
                    body: (*text).into(),
                    created_at: now.clone(),
                    commit_oid: head.clone(),
                    original_commit_oid: head.clone(),
                    diff_hunk: hunk,
                }],
            });
        }
        let pr = self.pr(repo, number);
        pr.reviews.push(Review {
            node_id: review_id.clone(),
            author: author.into(),
            state: state.into(),
            body: body.into(),
            commit_oid: head,
            submitted_at: Some(now.clone()),
        });
        pr.threads.extend(new_threads);
        pr.updated_at = now;
        review_id
    }

    pub fn add_issue_comment(&mut self, repo: &str, number: u64, author: &str, body: &str) {
        let id = self.next_id("IC");
        let now = self.tick();
        let pr = self.pr(repo, number);
        pr.issue_comments.push(IssueComment {
            node_id: id,
            author: author.into(),
            body: body.into(),
            created_at: now.clone(),
        });
        pr.updated_at = now;
    }

    /// The hunk around a line, in the shape GitHub stores as `diffHunk`.
    pub fn diff_hunk_for(&mut self, repo: &str, number: u64, path: &str, side: &str, line: u32) -> String {
        let files = self.pr_files(repo, number);
        let Some(f) = files.iter().find(|f| f.path == path) else {
            return String::new();
        };
        let Some(patch) = &f.patch else {
            return String::new();
        };
        let mut out = String::new();
        let mut current = String::new();
        let (mut old, mut new) = (0u32, 0u32);
        for l in patch.lines() {
            if let Some((o, n)) = parse_hunk_starts(l) {
                if !out.is_empty() {
                    break;
                }
                current = format!("{l}\n");
                (old, new) = (o, n);
                continue;
            }
            current.push_str(l);
            current.push('\n');
            let hit = match l.as_bytes().first() {
                Some(b'+') => {
                    new += 1;
                    side == "RIGHT" && new - 1 == line
                }
                Some(b'-') => {
                    old += 1;
                    side == "LEFT" && old - 1 == line
                }
                _ => {
                    old += 1;
                    new += 1;
                    (side == "RIGHT" && new - 1 == line) || (side == "LEFT" && old - 1 == line)
                }
            };
            if hit {
                out = current.clone();
            }
        }
        out.trim_end().to_string()
    }

    /// The PR's current diff (merge base → head).
    pub fn pr_files(&mut self, repo: &str, number: u64) -> Vec<FileChange> {
        let limit = self.patch_limit;
        let r = self.repo(repo);
        let pr = &r.prs[&number];
        let base_tip = &r.branches[&pr.base_ref];
        let mb = r.merge_base(base_tip, &pr.head_oid).expect("merge base");
        r.diff(&mb, &pr.head_oid.clone(), limit)
    }

    pub fn take_fault(&mut self, key: &str) -> Option<crate::faults::FaultAction> {
        let idx = self.faults.iter().position(|f| f.matches(key))?;
        let f = &mut self.faults[idx];
        if f.skip > 0 {
            f.skip -= 1;
            return None;
        }
        let action = f.action.clone();
        f.times -= 1;
        if f.times == 0 {
            self.faults.remove(idx);
        }
        Some(action)
    }

    pub fn find_pr_by_node(&self, node_id: &str) -> Option<(String, u64)> {
        self.repos.iter().find_map(|(name, r)| {
            r.prs.values().find(|p| p.node_id == node_id).map(|p| (name.clone(), p.number))
        })
    }

    pub fn find_thread(&self, node_id: &str) -> Option<(String, u64, usize)> {
        self.repos.iter().find_map(|(name, r)| {
            r.prs.values().find_map(|p| {
                p.threads.iter().position(|t| t.node_id == node_id).map(|i| (name.clone(), p.number, i))
            })
        })
    }

    pub fn find_review(&self, node_id: &str) -> Option<(String, u64, usize)> {
        self.repos.iter().find_map(|(name, r)| {
            r.prs.values().find_map(|p| {
                p.reviews.iter().position(|rv| rv.node_id == node_id).map(|i| (name.clone(), p.number, i))
            })
        })
    }
}

impl Repo {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    fn ancestors(&self, oid: &str) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut order = Vec::new();
        let mut q = VecDeque::from([oid.to_string()]);
        while let Some(c) = q.pop_front() {
            if !seen.insert(c.clone()) {
                continue;
            }
            order.push(c.clone());
            if let Some(commit) = self.commits.get(&c) {
                q.extend(commit.parents.iter().cloned());
            }
        }
        order
    }

    pub fn merge_base(&self, a: &str, b: &str) -> Option<String> {
        let a_anc: HashSet<String> = self.ancestors(a).into_iter().collect();
        self.ancestors(b).into_iter().find(|c| a_anc.contains(c))
    }

    /// Commits reachable from `head` but not from `base_tip`, oldest first.
    pub fn pr_commits(&self, base_tip: &str, head: &str) -> Vec<&Commit> {
        let base_anc: HashSet<String> = self.ancestors(base_tip).into_iter().collect();
        let mut out: Vec<&Commit> = self
            .ancestors(head)
            .iter()
            .filter(|c| !base_anc.contains(*c))
            .filter_map(|c| self.commits.get(c))
            .collect();
        out.reverse();
        out
    }

    pub fn file_text(&self, commit: &str, path: &str) -> Option<String> {
        let blob = self.commits.get(commit)?.tree.get(path)?;
        Some(String::from_utf8_lossy(&self.blobs[blob]).into_owned())
    }

    pub fn resolve_expression(&self, expr: &str) -> Option<(String, &Vec<u8>)> {
        let (commit, path) = expr.split_once(':')?;
        let oid = self.commits.get(commit)?.tree.get(path)?;
        Some((oid.clone(), &self.blobs[oid]))
    }

    pub fn diff(&self, from: &str, to: &str, patch_limit: usize) -> Vec<FileChange> {
        let a = &self.commits[from].tree;
        let b = &self.commits[to].tree;
        let removed: Vec<&String> = a.keys().filter(|p| !b.contains_key(*p)).collect();
        let added: Vec<&String> = b.keys().filter(|p| !a.contains_key(*p)).collect();
        let mut renamed_from: HashMap<&String, &String> = HashMap::new();
        for new in &added {
            if let Some(old) =
                removed.iter().find(|old| a[**old] == b[*new] && !renamed_from.values().any(|v| v == *old))
            {
                renamed_from.insert(new, old);
            }
        }
        let mut out = Vec::new();
        let paths: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
        for path in paths {
            let (status, prev, base, head) = match (a.get(path), b.get(path)) {
                (Some(x), Some(y)) if x == y => continue,
                (Some(x), Some(y)) => ("modified", None, Some(x.clone()), Some(y.clone())),
                (None, Some(y)) => match renamed_from.get(path) {
                    Some(old) => ("renamed", Some((*old).clone()), Some(a[*old].clone()), Some(y.clone())),
                    None => ("added", None, None, Some(y.clone())),
                },
                (Some(x), None) => {
                    if renamed_from.values().any(|v| *v == path) {
                        continue;
                    }
                    ("removed", None, Some(x.clone()), None)
                }
                (None, None) => unreachable!(),
            };
            let old_bytes = base.as_ref().map(|o| self.blobs[o].as_slice()).unwrap_or(b"");
            let new_bytes = head.as_ref().map(|o| self.blobs[o].as_slice()).unwrap_or(b"");
            let binary = old_bytes.contains(&0) || new_bytes.contains(&0);
            let (patch, additions, deletions) = if binary {
                (None, 0, 0)
            } else {
                let old = String::from_utf8_lossy(old_bytes);
                let new = String::from_utf8_lossy(new_bytes);
                let diff = similar::TextDiff::from_lines(old.as_ref(), new.as_ref());
                let adds = diff.iter_all_changes().filter(|c| c.tag() == similar::ChangeTag::Insert).count();
                let dels = diff.iter_all_changes().filter(|c| c.tag() == similar::ChangeTag::Delete).count();
                let p = diff.unified_diff().context_radius(3).missing_newline_hint(true).to_string();
                let p = p.trim_end_matches('\n').to_string();
                if p.is_empty() || p.len() > patch_limit { (None, adds, dels) } else { (Some(p), adds, dels) }
            };
            out.push(FileChange {
                path: path.clone(),
                prev_path: prev,
                status,
                base_blob: base,
                head_blob: head,
                patch,
                additions,
                deletions,
            });
        }
        out
    }
}

fn parse_hunk_starts(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, _) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let o = old.split(',').next()?.parse().ok()?;
    let n = new.split(',').next()?.parse().ok()?;
    Some((o, n))
}

/// Whether `line` on `side` is inside one of the patch's hunks.
pub fn in_hunks(patch: Option<&str>, side: &str, line: u32) -> bool {
    let Some(patch) = patch else { return false };
    let (mut old, mut new) = (0u32, 0u32);
    for l in patch.lines() {
        if let Some((o, n)) = parse_hunk_starts(l) {
            (old, new) = (o, n);
            continue;
        }
        match l.as_bytes().first() {
            Some(b'+') => {
                if side == "RIGHT" && new == line {
                    return true;
                }
                new += 1;
            }
            Some(b'-') => {
                if side == "LEFT" && old == line {
                    return true;
                }
                old += 1;
            }
            Some(b'\\') => {}
            _ => {
                if (side == "RIGHT" && new == line) || (side == "LEFT" && old == line) {
                    return true;
                }
                old += 1;
                new += 1;
            }
        }
    }
    false
}

/// Maps a 1-based line from `old` to `new` if that line is unchanged.
pub fn map_line(old: &str, new: &str, line: u32) -> Option<u32> {
    let diff = similar::TextDiff::from_lines(old, new);
    for op in diff.ops() {
        if let similar::DiffOp::Equal { old_index, new_index, len } = *op {
            let l = line as usize - 1;
            if l >= old_index && l < old_index + len {
                return Some((new_index + (l - old_index) + 1) as u32);
            }
        }
    }
    None
}

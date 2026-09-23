//! GraphQL documents and their response shapes.
//!
//! Each document lives in `graphql/*.graphql` so `scripts/validate-graphql.mjs`
//! can check it against GitHub's published schema.

use serde::Deserialize;

pub const VIEWER: &str = include_str!("graphql/viewer.graphql");
pub const PULL_REQUEST_DETAILS: &str = include_str!("graphql/pull_request_details.graphql");
pub const PR_COMMITS: &str = include_str!("graphql/pr_commits.graphql");
pub const PR_REVIEWS: &str = include_str!("graphql/pr_reviews.graphql");
pub const PR_THREADS: &str = include_str!("graphql/pr_threads.graphql");
pub const THREAD_COMMENTS: &str = include_str!("graphql/thread_comments.graphql");
pub const PR_ISSUE_COMMENTS: &str = include_str!("graphql/pr_issue_comments.graphql");
pub const PR_CHECKS: &str = include_str!("graphql/pr_checks.graphql");

/// Builds the `Blobs` query for `n` expressions (`"<oid>:<path>"`), passed as
/// variables `$e0..$e{n-1}` and returned under aliases `b0..b{n-1}`.
pub fn blobs_query(n: usize) -> String {
    let mut vars = String::new();
    let mut fields = String::new();
    for i in 0..n {
        vars.push_str(&format!(", $e{i}: String!"));
        fields.push_str(&format!(
            "    b{i}: object(expression: $e{i}) {{ ... on Blob {{ oid byteSize isBinary isTruncated text }} }}\n"
        ));
    }
    format!(
        "query Blobs($owner: String!, $name: String!{vars}) {{\n  repository(owner: $owner, name: $name) {{\n{fields}  }}\n}}\n"
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Login {
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct Oid {
    pub oid: String,
}

#[derive(Debug, Deserialize)]
pub struct Id {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct NameWithOwner {
    #[serde(rename = "nameWithOwner")]
    pub name_with_owner: String,
}

// ─── PullRequestDetails ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DetailsData {
    pub repository: Option<RepoDetails>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoDetails {
    pub id: String,
    pub name: String,
    pub owner: Login,
    pub is_private: bool,
    pub is_archived: bool,
    pub viewer_permission: Option<String>,
    pub pull_request: Option<PrDetails>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrDetails {
    pub id: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub is_draft: bool,
    pub locked: bool,
    pub author: Option<Login>,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub base_ref_oid: String,
    pub head_ref_oid: String,
    pub head_repository: Option<NameWithOwner>,
    pub body: String,
    #[serde(rename = "bodyHTML")]
    pub body_html: String,
    pub review_decision: Option<String>,
    pub viewer_did_author: bool,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub created_at: String,
    pub updated_at: String,
    pub last_commit: Connection<CommitNodeWithChecks>,
}

#[derive(Debug, Deserialize)]
pub struct Connection<T> {
    pub nodes: Vec<T>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PagedConnection<T> {
    pub page_info: PageInfo,
    pub nodes: Vec<T>,
}

#[derive(Debug, Deserialize)]
pub struct CommitNodeWithChecks {
    pub commit: CommitWithChecks,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitWithChecks {
    pub oid: String,
    pub status_check_rollup: Option<Rollup>,
}

#[derive(Debug, Deserialize)]
pub struct Rollup {
    pub state: String,
    pub contexts: Connection<CheckContext>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "__typename")]
pub enum CheckContext {
    #[serde(rename_all = "camelCase")]
    CheckRun { name: String, status: String, conclusion: Option<String>, details_url: Option<String> },
    #[serde(rename_all = "camelCase")]
    StatusContext { context: String, state: String, target_url: Option<String>, description: Option<String> },
}

// ─── node(id:) wrappers ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct NodeData<T> {
    pub node: Option<T>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrChecksNode {
    pub last_commit: Connection<CommitNodeWithChecks>,
}

#[derive(Debug, Deserialize)]
pub struct PrCommitsNode {
    pub commits: PagedConnection<CommitNode>,
}

#[derive(Debug, Deserialize)]
pub struct CommitNode {
    pub commit: CommitInfo,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitInfo {
    pub oid: String,
    pub message_headline: String,
    pub message_body: String,
    pub authored_date: Option<String>,
    pub author: Option<GitActor>,
}

#[derive(Debug, Deserialize)]
pub struct GitActor {
    pub name: Option<String>,
    pub user: Option<Login>,
}

#[derive(Debug, Deserialize)]
pub struct PrReviewsNode {
    pub reviews: PagedConnection<ReviewInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewInfo {
    pub id: String,
    pub state: String,
    pub body: String,
    #[serde(rename = "bodyHTML")]
    pub body_html: String,
    pub submitted_at: Option<String>,
    pub author: Option<Login>,
    pub commit: Option<Oid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrThreadsNode {
    pub review_threads: PagedConnection<ThreadInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadInfo {
    pub id: String,
    pub path: String,
    pub subject_type: String,
    pub diff_side: Option<String>,
    pub line: Option<i64>,
    pub start_line: Option<i64>,
    pub start_diff_side: Option<String>,
    pub original_line: Option<i64>,
    pub original_start_line: Option<i64>,
    pub is_outdated: bool,
    pub is_resolved: bool,
    pub viewer_can_reply: bool,
    pub comments: PagedConnection<ReviewCommentInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewCommentInfo {
    pub id: String,
    pub body: String,
    #[serde(rename = "bodyHTML")]
    pub body_html: String,
    pub diff_hunk: String,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub state: Option<String>,
    pub author: Option<Login>,
    pub pull_request_review: Option<Id>,
    pub commit: Option<Oid>,
    pub original_commit: Option<Oid>,
}

#[derive(Debug, Deserialize)]
pub struct ThreadCommentsNode {
    pub comments: PagedConnection<ReviewCommentInfo>,
}

#[derive(Debug, Deserialize)]
pub struct PrIssueCommentsNode {
    pub comments: PagedConnection<IssueCommentInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueCommentInfo {
    pub id: String,
    pub body: String,
    #[serde(rename = "bodyHTML")]
    pub body_html: String,
    pub created_at: String,
    pub author: Option<Login>,
}

// ─── Blobs ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobInfo {
    pub oid: String,
    pub byte_size: i64,
    pub is_binary: Option<bool>,
    pub is_truncated: bool,
    pub text: Option<String>,
}

// ─── REST shapes ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct RestPrFile {
    pub sha: Option<String>,
    pub filename: String,
    pub status: String,
    #[serde(default)]
    pub additions: i64,
    #[serde(default)]
    pub deletions: i64,
    #[serde(default)]
    pub changes: i64,
    pub patch: Option<String>,
    pub previous_filename: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RestCompare {
    pub merge_base_commit: RestSha,
}

#[derive(Debug, Deserialize)]
pub struct RestSha {
    pub sha: String,
}

#[cfg(test)]
mod tests {
    #[test]
    fn blobs_query_shape() {
        let q = super::blobs_query(2);
        assert!(q.contains("$e0: String!, $e1: String!"));
        assert!(q.contains("b1: object(expression: $e1)"));
    }

    /// `blobs_sample.graphql` is what scripts/validate-graphql.mjs checks
    /// against GitHub's schema, so it must match what we actually send.
    #[test]
    fn blobs_sample_matches_generated_query() {
        assert_eq!(super::blobs_query(2), include_str!("graphql/blobs_sample.graphql"));
    }
}

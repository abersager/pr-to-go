//! GraphQL operations, dispatched by operation name. Responses have the shape
//! of the documents in `crates/core/src/github/graphql/`.

use serde_json::{Value, json};

use crate::model::{Pr, PrState, Repo, ReviewComment, Thread, World};

#[derive(Debug)]
pub enum GqlError {
    NotFound(String),
    Unprocessable(String),
    Forbidden(String),
}

pub type GqlResult = Result<Value, GqlError>;

pub fn handle(w: &mut World, viewer: &str, op: &str, vars: &Value) -> GqlResult {
    match op {
        "Viewer" => Ok(json!({ "viewer": { "id": w.users[viewer], "login": viewer } })),
        "PullRequestDetails" => pull_request_details(w, viewer, vars),
        "PrCommits" => pr_commits(w, vars),
        "PrReviews" => pr_reviews(w, viewer, vars),
        "PrThreads" => pr_threads(w, viewer, vars),
        "ThreadComments" => thread_comments(w, viewer, vars),
        "PrIssueComments" => pr_issue_comments(w, vars),
        "Blobs" => blobs(w, vars),
        _ => crate::mutations::handle(w, viewer, op, vars),
    }
}

pub fn s<'a>(vars: &'a Value, k: &str) -> &'a str {
    vars.get(k).and_then(|v| v.as_str()).unwrap_or("")
}

fn esc(t: &str) -> String {
    t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Inline Markdown: `code` and images.
fn inline(t: &str) -> String {
    let mut out = String::new();
    let mut rest = t;
    loop {
        let img = rest.find("![");
        let code = rest.find('`');
        match (img, code) {
            (Some(i), c) if c.is_none_or(|c| i < c) => {
                out.push_str(&esc(&rest[..i]));
                let after = &rest[i + 2..];
                match after
                    .split_once("](")
                    .and_then(|(alt, r)| r.split_once(')').map(|(url, tail)| (alt, url, tail)))
                {
                    Some((alt, url, tail)) => {
                        out.push_str(&format!("<img src=\"{}\" alt=\"{}\">", esc(url), esc(alt)));
                        rest = tail;
                    }
                    None => {
                        out.push_str("![");
                        rest = after;
                    }
                }
            }
            (_, Some(c)) => {
                out.push_str(&esc(&rest[..c]));
                let after = &rest[c + 1..];
                match after.split_once('`') {
                    Some((code, tail)) => {
                        out.push_str(&format!("<code>{}</code>", esc(code)));
                        rest = tail;
                    }
                    None => {
                        out.push('`');
                        rest = after;
                    }
                }
            }
            _ => {
                out.push_str(&esc(rest));
                return out;
            }
        }
    }
}

/// Renders the Markdown the tests and demo use: paragraphs, lists, fenced
/// code, inline code and images. GitHub's real renderer does far more.
pub fn render_md(md: &str) -> String {
    let mut blocks = Vec::new();
    let mut in_code = false;
    let mut code = String::new();
    let mut para: Vec<&str> = Vec::new();
    let mut list: Vec<&str> = Vec::new();
    let flush = |blocks: &mut Vec<String>, para: &mut Vec<&str>, list: &mut Vec<&str>| {
        if !para.is_empty() {
            blocks.push(format!("<p>{}</p>", inline(&para.join(" "))));
            para.clear();
        }
        if !list.is_empty() {
            let items: String = list.iter().map(|l| format!("<li>{}</li>", inline(l))).collect();
            blocks.push(format!("<ul>{items}</ul>"));
            list.clear();
        }
    };
    for line in md.lines() {
        if line.starts_with("```") {
            if in_code {
                blocks.push(format!("<pre><code>{}</code></pre>", esc(&code)));
                code.clear();
            } else {
                flush(&mut blocks, &mut para, &mut list);
            }
            in_code = !in_code;
        } else if in_code {
            code.push_str(line);
            code.push('\n');
        } else if let Some(item) = line.strip_prefix("- ") {
            if !para.is_empty() {
                let mut none = Vec::new();
                flush(&mut blocks, &mut para, &mut none);
            }
            list.push(item);
        } else if line.trim().is_empty() {
            flush(&mut blocks, &mut para, &mut list);
        } else {
            if !list.is_empty() {
                let mut none = Vec::new();
                flush(&mut blocks, &mut none, &mut list);
            }
            para.push(line);
        }
    }
    flush(&mut blocks, &mut para, &mut list);
    blocks.join("\n")
}

/// GraphQL-style pagination over a slice, with cursors `c<index>`.
fn page<T: Clone>(w: &World, items: &[T], first: usize, vars: &Value) -> (Vec<T>, Value) {
    let first = w.page_size.map_or(first, |p| p.min(first));
    let start = vars
        .get("after")
        .and_then(|a| a.as_str())
        .and_then(|a| a.strip_prefix('c'))
        .and_then(|n| n.parse::<usize>().ok())
        .map_or(0, |n| n + 1);
    let end = (start + first).min(items.len());
    let slice = items.get(start..end).unwrap_or(&[]).to_vec();
    let info = json!({
        "hasNextPage": end < items.len(),
        "endCursor": if end > start { Some(format!("c{}", end - 1)) } else { None },
    });
    (slice, info)
}

fn find_pr<'a>(w: &'a World, node_id: &str) -> Result<(&'a Repo, &'a Pr), GqlError> {
    let (repo, num) = w.find_pr_by_node(node_id).ok_or_else(|| {
        GqlError::NotFound(format!("Could not resolve to a node with the global id of '{node_id}'"))
    })?;
    let r = &w.repos[&repo];
    Ok((r, &r.prs[&num]))
}

fn review_visible(pr: &Pr, review_id: &str, viewer: &str) -> bool {
    pr.reviews
        .iter()
        .find(|r| r.node_id == review_id)
        .is_none_or(|r| r.state != "PENDING" || r.author == viewer)
}

fn review_state<'a>(pr: &'a Pr, review_id: &str) -> &'a str {
    pr.reviews.iter().find(|r| r.node_id == review_id).map_or("COMMENTED", |r| r.state.as_str())
}

fn pull_request_details(w: &mut World, viewer: &str, vars: &Value) -> GqlResult {
    let full = format!("{}/{}", s(vars, "owner"), s(vars, "name"));
    let number = vars["number"].as_u64().unwrap_or(0);
    let limit = w.patch_limit;
    let Some(r) = w.repos.get(&full) else {
        return Err(GqlError::NotFound(format!("Could not resolve to a Repository with the name '{full}'.")));
    };
    let Some(pr) = r.prs.get(&number) else {
        return Err(GqlError::NotFound(format!(
            "Could not resolve to a PullRequest with the number of {number}."
        )));
    };
    let base_tip = &r.branches[&pr.base_ref];
    let mb = r.merge_base(base_tip, &pr.head_oid).expect("merge base");
    let files = r.diff(&mb, &pr.head_oid, limit);
    let rollup = r.checks.get(&pr.head_oid).map(|cs| {
        let state = if cs.iter().any(|c| c.conclusion.as_deref() == Some("FAILURE")) {
            "FAILURE"
        } else if cs.iter().any(|c| c.status != "COMPLETED") {
            "PENDING"
        } else {
            "SUCCESS"
        };
        json!({
            "state": state,
            "contexts": { "nodes": cs.iter().map(|c| json!({
                "__typename": "CheckRun", "name": c.name, "status": c.status,
                "conclusion": c.conclusion, "detailsUrl": format!("{}/checks/{}", w.base_url, c.name),
            })).collect::<Vec<_>>() }
        })
    });
    let latest: std::collections::BTreeMap<&str, &str> = pr
        .reviews
        .iter()
        .filter(|rv| rv.state == "APPROVED" || rv.state == "CHANGES_REQUESTED")
        .map(|rv| (rv.author.as_str(), rv.state.as_str()))
        .collect();
    let decision = if latest.values().any(|s| *s == "CHANGES_REQUESTED") {
        "CHANGES_REQUESTED"
    } else if latest.values().any(|s| *s == "APPROVED") {
        "APPROVED"
    } else {
        "REVIEW_REQUIRED"
    };
    Ok(json!({ "repository": {
        "id": r.node_id,
        "name": r.name,
        "owner": { "login": r.owner },
        "isPrivate": false,
        "isArchived": r.archived,
        "viewerPermission": "WRITE",
        "pullRequest": {
            "id": pr.node_id,
            "number": pr.number,
            "title": pr.title,
            "url": format!("https://github.com/{full}/pull/{}", pr.number),
            "state": pr.state.as_str(),
            "isDraft": pr.is_draft,
            "locked": pr.locked,
            "author": { "login": pr.author },
            "baseRefName": pr.base_ref,
            "headRefName": pr.head_ref,
            "baseRefOid": base_tip,
            "headRefOid": pr.head_oid,
            "headRepository": { "nameWithOwner": full },
            "body": pr.body,
            "bodyHTML": render_md(&pr.body),
            "reviewDecision": decision,
            "viewerDidAuthor": pr.author == viewer,
            "additions": files.iter().map(|f| f.additions).sum::<usize>(),
            "deletions": files.iter().map(|f| f.deletions).sum::<usize>(),
            "changedFiles": files.len(),
            "createdAt": pr.created_at,
            "updatedAt": pr.updated_at,
            "lastCommit": { "nodes": [ { "commit": { "oid": pr.head_oid, "statusCheckRollup": rollup } } ] },
        }
    }}))
}

fn pr_commits(w: &mut World, vars: &Value) -> GqlResult {
    let (r, pr) = find_pr(w, s(vars, "id"))?;
    let commits: Vec<Value> = r
        .pr_commits(&r.branches[&pr.base_ref], &pr.head_oid)
        .iter()
        .map(|c| {
            let (headline, body) = c.message.split_once('\n').unwrap_or((&c.message, ""));
            json!({ "commit": {
                "oid": c.oid, "messageHeadline": headline, "messageBody": body.trim(),
                "authoredDate": c.date, "author": { "name": c.author, "user": { "login": c.author } },
            }})
        })
        .collect();
    let (nodes, info) = page(w, &commits, 100, vars);
    Ok(json!({ "node": { "commits": { "pageInfo": info, "nodes": nodes } } }))
}

fn pr_reviews(w: &mut World, viewer: &str, vars: &Value) -> GqlResult {
    let (_, pr) = find_pr(w, s(vars, "id"))?;
    let reviews: Vec<Value> = pr
        .reviews
        .iter()
        .filter(|rv| rv.state != "PENDING" || rv.author == viewer)
        .map(|rv| json!({
            "id": rv.node_id, "state": rv.state, "body": rv.body, "bodyHTML": render_md(&rv.body),
            "submittedAt": rv.submitted_at, "author": { "login": rv.author }, "commit": { "oid": rv.commit_oid },
        }))
        .collect();
    let (nodes, info) = page(w, &reviews, 50, vars);
    Ok(json!({ "node": { "reviews": { "pageInfo": info, "nodes": nodes } } }))
}

pub fn comment_json(pr: &Pr, c: &ReviewComment) -> Value {
    let state = if review_state(pr, &c.review_node_id) == "PENDING" { "PENDING" } else { "SUBMITTED" };
    json!({
        "id": c.node_id, "body": c.body, "bodyHTML": render_md(&c.body), "diffHunk": c.diff_hunk,
        "createdAt": c.created_at, "updatedAt": c.created_at, "state": state,
        "author": { "login": c.author }, "pullRequestReview": { "id": c.review_node_id },
        "commit": { "oid": c.commit_oid }, "originalCommit": { "oid": c.original_commit_oid },
    })
}

fn visible_comments<'a>(pr: &'a Pr, t: &'a Thread, viewer: &str) -> Vec<&'a ReviewComment> {
    t.comments.iter().filter(|c| review_visible(pr, &c.review_node_id, viewer)).collect()
}

fn pr_threads(w: &mut World, viewer: &str, vars: &Value) -> GqlResult {
    let (_, pr) = find_pr(w, s(vars, "id"))?;
    let threads: Vec<Value> = pr
        .threads
        .iter()
        .filter(|t| !visible_comments(pr, t, viewer).is_empty())
        .map(|t| {
            let comments: Vec<Value> =
                visible_comments(pr, t, viewer).into_iter().map(|c| comment_json(pr, c)).collect();
            let (nodes, info) = page(w, &comments, 50, &Value::Null);
            json!({
                "id": t.node_id, "path": t.path, "subjectType": t.subject_type,
                "diffSide": t.side.clone().unwrap_or_else(|| "RIGHT".into()),
                "line": t.line, "startLine": t.start_line, "startDiffSide": t.start_side,
                "originalLine": t.original_line, "originalStartLine": t.original_start_line,
                "isOutdated": t.outdated, "isResolved": t.resolved, "viewerCanReply": !pr.locked,
                "comments": { "pageInfo": info, "nodes": nodes },
            })
        })
        .collect();
    let (nodes, info) = page(w, &threads, 50, vars);
    Ok(json!({ "node": { "reviewThreads": { "pageInfo": info, "nodes": nodes } } }))
}

fn thread_comments(w: &mut World, viewer: &str, vars: &Value) -> GqlResult {
    let id = s(vars, "id");
    let (repo, num, ti) = w.find_thread(id).ok_or_else(|| {
        GqlError::NotFound(format!("Could not resolve to a node with the global id of '{id}'"))
    })?;
    let pr = &w.repos[&repo].prs[&num];
    let t = &pr.threads[ti];
    let comments: Vec<Value> =
        visible_comments(pr, t, viewer).into_iter().map(|c| comment_json(pr, c)).collect();
    let (nodes, info) = page(w, &comments, 50, vars);
    Ok(json!({ "node": { "comments": { "pageInfo": info, "nodes": nodes } } }))
}

fn pr_issue_comments(w: &mut World, vars: &Value) -> GqlResult {
    let (_, pr) = find_pr(w, s(vars, "id"))?;
    let comments: Vec<Value> = pr
        .issue_comments
        .iter()
        .map(|c| {
            json!({
                "id": c.node_id, "body": c.body, "bodyHTML": render_md(&c.body),
                "createdAt": c.created_at, "author": { "login": c.author },
            })
        })
        .collect();
    let (nodes, info) = page(w, &comments, 50, vars);
    Ok(json!({ "node": { "comments": { "pageInfo": info, "nodes": nodes } } }))
}

fn blobs(w: &mut World, vars: &Value) -> GqlResult {
    let full = format!("{}/{}", s(vars, "owner"), s(vars, "name"));
    let r = w.repos.get(&full).ok_or_else(|| GqlError::NotFound(format!("no repository {full}")))?;
    let mut out = serde_json::Map::new();
    for i in 0.. {
        let Some(expr) = vars.get(format!("e{i}")).and_then(|v| v.as_str()) else {
            break;
        };
        let v = match r.resolve_expression(expr) {
            None => Value::Null,
            Some((oid, bytes)) => {
                let binary = bytes.contains(&0);
                let truncated = bytes.len() > w.blob_text_limit;
                let text = if binary {
                    None
                } else {
                    let t =
                        String::from_utf8_lossy(&bytes[..bytes.len().min(w.blob_text_limit)]).into_owned();
                    Some(t)
                };
                json!({ "oid": oid, "byteSize": bytes.len(), "isBinary": binary, "isTruncated": truncated, "text": text })
            }
        };
        out.insert(format!("b{i}"), v);
    }
    Ok(json!({ "repository": Value::Object(out) }))
}

pub fn pr_state_is_open(pr: &Pr) -> bool {
    pr.state == PrState::Open
}

#[cfg(test)]
mod tests {
    use super::render_md;

    #[test]
    fn renders_the_markdown_we_use() {
        let html =
            render_md("Hi `x` <b>\n\n- one\n- two `y`\n\n```rust\nfn a() {}\n```\n\n![alt](http://i/a.png)");
        assert_eq!(
            html,
            "<p>Hi <code>x</code> &lt;b&gt;</p>\n<ul><li>one</li><li>two <code>y</code></li></ul>\n<pre><code>fn a() {}\n</code></pre>\n<p><img src=\"http://i/a.png\" alt=\"alt\"></p>"
        );
    }
}

//! Browsing the open pull requests the user can reach: their own
//! repositories, their organizations and repositories they collaborate on.
//! It's for picking what to take offline, so it needs a connection and
//! stores nothing; a PR picked from it is added like any other.

use std::collections::HashSet;

use futures_util::future::try_join_all;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::github::{GhError, GitHub, OpKind};
use crate::inbox::{IndexPr, SEARCH_PRS};
use crate::service::Core;

const VIEWER_SCOPES: &str = include_str!("github/graphql/viewer_scopes.graphql");
/// GitHub rejects longer search queries.
const MAX_QUERY: usize = 256;
const BASE_QUERY: &str = "is:pr is:open archived:false sort:updated-desc";

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BrowsePr {
    pub node_id: String,
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub author: Option<String>,
    pub updated_at: String,
    pub is_draft: bool,
    pub is_private: bool,
    /// Set when the PR is already in the local store.
    pub local_id: Option<i64>,
    /// A complete copy is available offline.
    pub offline: bool,
}

/// Where the list comes from, for saying so in the UI.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BrowseScope {
    pub login: String,
    pub orgs: Vec<String>,
    /// Repositories shared with the user by someone else.
    pub shared_repos: i64,
    /// The filter named its own repo:, org: or user:, so only that is searched.
    pub custom: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BrowsePage {
    pub prs: Vec<BrowsePr>,
    /// Open PRs matching, as GitHub counts them.
    pub total: i64,
    pub scope: BrowseScope,
    /// Pass back for the next page; `None` when there's no more.
    pub cursor: Option<String>,
}

/// One search per chunk of scope qualifiers, each with its own position.
#[derive(Serialize, Deserialize)]
struct Cursor {
    scope: BrowseScope,
    queries: Vec<String>,
    after: Vec<Option<String>>,
    more: Vec<bool>,
    total: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Scopes {
    viewer: ScopesViewer,
}

#[derive(Deserialize)]
struct ScopesViewer {
    login: String,
    organizations: crate::github::queries::Connection<crate::github::queries::Login>,
    repositories: SharedRepos,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SharedRepos {
    total_count: i64,
    nodes: Vec<SharedRepo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SharedRepo {
    name_with_owner: String,
    owner: crate::github::queries::Login,
}

fn has_scope_qualifier(filter: &str) -> bool {
    filter
        .split_whitespace()
        .any(|t| t.starts_with("repo:") || t.starts_with("org:") || t.starts_with("user:"))
}

/// Packs the scope qualifiers into as few queries as fit GitHub's length
/// limit. Qualifiers in one query are ORed, which is what we want.
fn chunk_queries(base: &str, scopes: &[String]) -> Result<Vec<String>> {
    let too_long = || Error::Invalid("That filter is too long for GitHub's search.".into());
    if base.len() > MAX_QUERY {
        return Err(too_long());
    }
    let mut out = Vec::new();
    let mut cur = base.to_string();
    let mut in_cur = 0;
    for s in scopes {
        if cur.len() + 1 + s.len() > MAX_QUERY {
            if in_cur == 0 {
                return Err(too_long());
            }
            out.push(std::mem::replace(&mut cur, base.to_string()));
            in_cur = 0;
        }
        cur.push(' ');
        cur.push_str(s);
        in_cur += 1;
    }
    if in_cur > 0 || out.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

async fn search_page(gh: &GitHub, q: &str, after: Option<&str>) -> Result<Value> {
    Ok(gh.graphql("SearchPrs", SEARCH_PRS, json!({ "q": q, "after": after }), OpKind::Query).await?)
}

impl Core {
    /// A page of open PRs the user can reach, most recently updated first.
    /// `filter` narrows it with GitHub search syntax (words, `author:bob`,
    /// `repo:acme/api`, ...); `cursor` comes from the previous page.
    pub async fn browse_prs(&self, filter: Option<&str>, cursor: Option<&str>) -> Result<BrowsePage> {
        let mut c = match cursor {
            Some(raw) => serde_json::from_str::<Cursor>(raw)
                .map_err(|_| Error::Invalid("That list is out of date; reload it.".into()))?,
            None => self.first_cursor(filter.unwrap_or("")).await?,
        };
        let open: Vec<usize> = (0..c.queries.len()).filter(|&i| c.more[i]).collect();
        let pages = try_join_all(
            open.iter()
                .map(|&i| search_page(&self.gh, &c.queries[i], c.after[i].as_deref()))
                .collect::<Vec<_>>(),
        )
        .await;
        if let Err(Error::GitHub(e)) = &pages {
            self.observe::<()>(&Err(e.clone()));
        }
        let pages = pages?;
        let first_page = cursor.is_none();
        let mut total = 0;
        let mut seen = HashSet::new();
        let mut found: Vec<IndexPr> = Vec::new();
        for (&i, page) in open.iter().zip(&pages) {
            let search = &page["search"];
            total += search["issueCount"].as_i64().unwrap_or(0);
            c.more[i] = search["pageInfo"]["hasNextPage"].as_bool() == Some(true);
            c.after[i] = search["pageInfo"]["endCursor"].as_str().map(str::to_owned);
            for node in search["nodes"].as_array().into_iter().flatten() {
                // Search can return issues; our fragment only fills PRs.
                if node.get("id").is_none() {
                    continue;
                }
                let pr: IndexPr = serde_json::from_value(node.clone())
                    .map_err(|e| GhError::Protocol(format!("SearchPrs: {e}")))?;
                if seen.insert(pr.id.clone()) {
                    found.push(pr);
                }
            }
        }
        if first_page {
            c.total = total;
        }
        found.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        let prs = self.db.read(|conn| {
            let mut st = conn.prepare(
                "SELECT id, current_revision_id IS NOT NULL AND sync_state IN ('ready', 'partial', 'stale')
                 FROM pull_request WHERE node_id = ?1",
            )?;
            found
                .into_iter()
                .map(|p| {
                    let local: Option<(i64, bool)> =
                        st.query_row([&p.id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
                    Ok(BrowsePr {
                        repo: format!("{}/{}", p.repository.owner.login, p.repository.name),
                        node_id: p.id,
                        number: p.number,
                        title: p.title,
                        url: p.url,
                        author: p.author.map(|a| a.login),
                        updated_at: p.updated_at,
                        is_draft: p.is_draft,
                        is_private: p.repository.is_private,
                        local_id: local.map(|l| l.0),
                        offline: local.is_some_and(|l| l.1),
                    })
                })
                .collect::<Result<Vec<_>>>()
        })?;
        let more = c.more.iter().any(|m| *m);
        Ok(BrowsePage {
            prs,
            total: c.total,
            scope: c.scope.clone(),
            cursor: if more { Some(serde_json::to_string(&c)?) } else { None },
        })
    }

    async fn first_cursor(&self, filter: &str) -> Result<Cursor> {
        let filter = filter.split_whitespace().collect::<Vec<_>>().join(" ");
        let base = if filter.is_empty() { BASE_QUERY.to_string() } else { format!("{BASE_QUERY} {filter}") };
        let (scope, queries) = if has_scope_qualifier(&filter) {
            // The user named where to look.
            (BrowseScope { custom: true, ..Default::default() }, chunk_queries(&base, &[])?)
        } else {
            let res =
                self.gh.graphql::<Scopes>("ViewerScopes", VIEWER_SCOPES, json!({}), OpKind::Query).await;
            self.observe(&res);
            let v = res?.viewer;
            let orgs: Vec<String> = v.organizations.nodes.into_iter().map(|o| o.login).collect();
            let covered: HashSet<&str> =
                std::iter::once(v.login.as_str()).chain(orgs.iter().map(String::as_str)).collect();
            let shared: Vec<String> = v
                .repositories
                .nodes
                .iter()
                .filter(|r| !covered.contains(r.owner.login.as_str()))
                .map(|r| format!("repo:{}", r.name_with_owner))
                .collect();
            if v.repositories.total_count > v.repositories.nodes.len() as i64 {
                tracing::info!(
                    "browse: only the first {} shared repositories are searched",
                    v.repositories.nodes.len()
                );
            }
            let mut qualifiers = vec![format!("user:{}", v.login)];
            qualifiers.extend(orgs.iter().map(|o| format!("org:{o}")));
            qualifiers.extend(shared.iter().cloned());
            let scope = BrowseScope {
                login: v.login.clone(),
                orgs: orgs.clone(),
                shared_repos: shared.len() as i64,
                custom: false,
            };
            (scope, chunk_queries(&base, &qualifiers)?)
        };
        let n = queries.len();
        Ok(Cursor { scope, queries, after: vec![None; n], more: vec![true; n], total: 0 })
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_QUERY, chunk_queries};

    #[test]
    fn packs_qualifiers_into_queries_that_fit() {
        let scopes: Vec<String> = (0..40).map(|i| format!("org:organization-{i:02}")).collect();
        let queries = chunk_queries("is:pr is:open", &scopes).unwrap();
        assert!(queries.len() > 1);
        assert!(queries.iter().all(|q| q.len() <= MAX_QUERY && q.starts_with("is:pr is:open ")));
        let joined = queries.join(" ");
        assert!(scopes.iter().all(|s| joined.contains(s.as_str())));
        assert_eq!(chunk_queries("is:pr", &[]).unwrap(), ["is:pr"]);
        assert!(chunk_queries(&"x".repeat(300), &[]).is_err());
    }
}

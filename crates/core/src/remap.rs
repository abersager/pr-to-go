//! Finds a new home for a draft comment after the PR moved (DESIGN.md §11).
//!
//! The core is a pure function over the old and new file contents, the new
//! diff's hunks and the comment's anchor snapshot, so it's tested with
//! tables of cases. `propose_for_comment` wires it to the database.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::blobstore::BlobStore;
use crate::db::Db;
use crate::diff::{Hunk, Side, check_range, parse_patch, split_lines};
use crate::drafts::{AnchorSnapshot, SideSnapshot, has_suggestion};
use crate::error::Result;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The anchored lines are unchanged and still in the diff.
    Clean,
    /// The lines changed or moved a little; a best guess, to confirm.
    Fuzzy,
    /// Unchanged, but no longer part of the diff (GitHub won't take a line
    /// comment there).
    NotCommentable,
    /// The lines are gone.
    Orphaned,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub status: Status,
    /// The file in the new revision (differs after a rename).
    pub path: Option<String>,
    pub side: Option<Side>,
    pub line: Option<u32>,
    pub start_side: Option<Side>,
    pub start_line: Option<u32>,
    /// 1.0 for clean; the match score for fuzzy.
    pub confidence: f64,
    /// The anchored lines as written, and what's at the proposed place now.
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
    /// The comment has a suggestion written for code that has changed.
    pub suggestion_stale: bool,
    /// The file is still part of the PR's diff (so a file comment works).
    pub file_in_diff: bool,
    pub to_revision: i64,
}

/// For each 1-based line of `old`, its 1-based line in `new` if unchanged.
pub fn line_map(old: &str, new: &str) -> Vec<Option<u32>> {
    let old_lines = split_lines(old);
    let mut map = vec![None; old_lines.len()];
    let diff = similar::TextDiff::configure().algorithm(similar::Algorithm::Patience).diff_lines(old, new);
    for op in diff.ops() {
        if let similar::DiffOp::Equal { old_index, new_index, len } = *op {
            for k in 0..len {
                if let Some(slot) = map.get_mut(old_index + k) {
                    *slot = Some((new_index + k + 1) as u32);
                }
            }
        }
    }
    map
}

/// Where `line` would be in `new`, following the nearest unchanged line
/// above it. For placing a fuzzy search.
fn translate(map: &[Option<u32>], line: u32) -> u32 {
    let l = line as usize;
    for back in 0..l.min(map.len() + 1) {
        let i = l - back; // 1-based
        if let Some(Some(n)) = map.get(i.wrapping_sub(1)) {
            return n + back as u32;
        }
    }
    line
}

fn similarity(a: &[&str], b: &[&str]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let a = a.iter().map(|l| l.trim()).collect::<Vec<_>>().join("\n");
    let b = b.iter().map(|l| l.trim()).collect::<Vec<_>>().join("\n");
    if a == b {
        return 1.0;
    }
    similar::TextDiff::from_chars(a.as_str(), b.as_str()).ratio() as f64
}

/// The fuzzy search window, in lines either side of the translated position.
const SEARCH_RADIUS: usize = 400;
/// Below this score a fuzzy match isn't worth proposing.
pub const FUZZY_THRESHOLD: f64 = 0.6;

/// Best place in `new_lines` for the snapshot's lines, near `near`.
/// Returns (first line, score).
fn fuzzy_find(snap: &SideSnapshot, new_lines: &[&str], near: u32) -> Option<(u32, f64)> {
    let want: Vec<&str> = snap.lines.iter().map(String::as_str).collect();
    let before: Vec<&str> = snap.before.iter().map(String::as_str).collect();
    let after: Vec<&str> = snap.after.iter().map(String::as_str).collect();
    let n = want.len().max(1);
    if new_lines.len() < n {
        return None;
    }
    let near = near.saturating_sub(1) as usize;
    let lo = near.saturating_sub(SEARCH_RADIUS);
    let hi = (near + SEARCH_RADIUS).min(new_lines.len() - n);
    let mut best: Option<(u32, f64)> = None;
    for i in lo..=hi {
        let content = similarity(&want, &new_lines[i..i + n]);
        if content < 0.3 {
            continue;
        }
        let ctx_before = similarity(&before, &new_lines[i.saturating_sub(before.len())..i]);
        let ctx_after = similarity(
            &after,
            &new_lines[(i + n).min(new_lines.len())..(i + n + after.len()).min(new_lines.len())],
        );
        let distance = (i as f64 - near as f64).abs();
        let proximity = 1.0 / (1.0 + distance / 200.0);
        let score = (0.6 * content + 0.2 * ctx_before + 0.2 * ctx_after) * (0.85 + 0.15 * proximity);
        if best.is_none_or(|(_, s)| score > s) {
            best = Some(((i + 1) as u32, score));
        }
    }
    best
}

/// Inputs for one side of the anchor.
pub struct SideInput<'a> {
    pub snapshot: Option<&'a SideSnapshot>,
    pub old_text: Option<&'a str>,
    pub new_text: Option<&'a str>,
}

/// A line comment's anchor, and the new revision's view of its file.
pub struct Input<'a> {
    pub side: Side,
    pub line: u32,
    pub start: Option<(Side, u32)>,
    pub body: &'a str,
    pub left: SideInput<'a>,
    pub right: SideInput<'a>,
    /// The file's hunks in the new revision (empty if it's not in the diff).
    pub new_hunks: &'a [Hunk],
    pub new_path: Option<String>,
    pub file_in_diff: bool,
    pub has_patch: bool,
    pub to_revision: i64,
}

/// Maps one end of the range. Returns (new line, exact).
fn map_end(input: &SideInput, line: u32) -> Option<(u32, bool)> {
    let (old, new) = (input.old_text?, input.new_text?);
    let map = line_map(old, new);
    match map.get(line as usize - 1) {
        Some(Some(n)) => Some((*n, true)),
        _ => None,
    }
}

fn side_input<'a>(input: &'a Input<'a>, side: Side) -> &'a SideInput<'a> {
    match side {
        Side::Left => &input.left,
        Side::Right => &input.right,
    }
}

fn lines_at(text: Option<&str>, first: u32, last: u32) -> Vec<String> {
    let Some(text) = text else { return vec![] };
    let lines = split_lines(text);
    let (a, b) = ((first as usize).saturating_sub(1), (last as usize).min(lines.len()));
    lines.get(a..b).unwrap_or(&[]).iter().map(|s| s.to_string()).collect()
}

pub fn propose(input: &Input) -> Proposal {
    let end_side = input.side;
    let start_pt = input.start.unwrap_or((end_side, input.line));
    let snap_for = |side: Side| side_input(input, side).snapshot;
    let old_lines = {
        let mut v = Vec::new();
        if let Some(s) = snap_for(start_pt.0).filter(|_| start_pt.0 != end_side) {
            v.extend(s.lines.clone());
        }
        if let Some(s) = snap_for(end_side) {
            v.extend(s.lines.clone());
        }
        v
    };
    let suggestion = has_suggestion(input.body);
    let base = Proposal {
        status: Status::Orphaned,
        path: input.new_path.clone(),
        side: None,
        line: None,
        start_side: None,
        start_line: None,
        confidence: 0.0,
        old_lines,
        new_lines: vec![],
        suggestion_stale: false,
        file_in_diff: input.file_in_diff,
        to_revision: input.to_revision,
    };
    if input.new_path.is_none() {
        return base;
    }

    // Exact: both ends map through unchanged lines, on their own sides.
    let exact_end = map_end(side_input(input, end_side), input.line);
    let exact_start =
        if input.start.is_some() { map_end(side_input(input, start_pt.0), start_pt.1) } else { exact_end };
    if let (Some((end, _)), Some((start, _))) = (exact_end, exact_start) {
        let same_len = start_pt.0 != end_side || end.checked_sub(start) == input.line.checked_sub(start_pt.1);
        if same_len {
            let start_opt = input.start.map(|(s, _)| (s, start));
            let commentable =
                input.has_patch && check_range(input.new_hunks, start_opt, (end_side, end)).is_ok();
            let new_text = side_input(input, end_side).new_text;
            let first = if start_pt.0 == end_side { start } else { end };
            return Proposal {
                status: if commentable { Status::Clean } else { Status::NotCommentable },
                side: Some(end_side),
                line: Some(end),
                start_side: input.start.map(|(s, _)| s),
                start_line: input.start.map(|_| start),
                confidence: 1.0,
                new_lines: lines_at(new_text, first, end),
                ..base
            };
        }
    }

    // Fuzzy: look for the anchored text near where it would have moved to.
    // Ranges are matched on their end side as a block.
    let si = side_input(input, end_side);
    let (Some(snap), Some(old), Some(new)) = (si.snapshot, si.old_text, si.new_text) else { return base };
    let near = translate(&line_map(old, new), snap.first);
    let new_lines = split_lines(new);
    let Some((first, score)) = fuzzy_find(snap, &new_lines, near) else { return base };
    if score < FUZZY_THRESHOLD {
        return base;
    }
    let len = snap.last - snap.first;
    let (start, end) = (first, first + len);
    let start_opt = if len > 0 { Some((end_side, start)) } else { None };
    let commentable = input.has_patch && check_range(input.new_hunks, start_opt, (end_side, end)).is_ok();
    Proposal {
        status: if commentable { Status::Fuzzy } else { Status::NotCommentable },
        side: Some(end_side),
        line: Some(end),
        start_side: start_opt.map(|s| s.0),
        start_line: start_opt.map(|s| s.1),
        confidence: (score * 100.0).round() / 100.0,
        new_lines: lines_at(Some(new), start, end),
        suggestion_stale: suggestion,
        ..base
    }
}

struct CommentRow {
    kind: String,
    subject: String,
    path: Option<String>,
    side: Option<String>,
    line: Option<u32>,
    start_side: Option<String>,
    start_line: Option<u32>,
    body: String,
    anchor_rev: Option<i64>,
    snapshot: Option<String>,
}

struct FileRow {
    path: String,
    prev_path: Option<String>,
    base: Option<String>,
    head: Option<String>,
    patch: Option<String>,
}

fn file_row(c: &Connection, rev: i64, path: &str) -> Result<Option<FileRow>> {
    Ok(c.query_row(
        "SELECT path, prev_path, base_blob_oid, head_blob_oid, patch FROM revision_file
         WHERE revision_id = ?1 AND path = ?2",
        params![rev, path],
        |r| {
            Ok(FileRow {
                path: r.get(0)?,
                prev_path: r.get(1)?,
                base: r.get(2)?,
                head: r.get(3)?,
                patch: r.get(4)?,
            })
        },
    )
    .optional()?)
}

/// Proposes a new home for a draft comment in revision `to_revision`.
/// Replies have no anchor and get `None`.
pub fn propose_for_comment(
    db: &Db,
    blobs: &BlobStore,
    comment_id: i64,
    to_revision: i64,
) -> Result<Option<Proposal>> {
    // Everything from the database first: reading blobs takes the same lock.
    let (r, old, new) = db.read(|c| {
        let r = c.query_row(
            "SELECT kind, subject_type, path, side, line, start_side, start_line, body_md, anchor_revision_id,
                    anchor_snapshot FROM draft_comment WHERE id = ?1",
            [comment_id],
            |r| {
                Ok(CommentRow {
                    kind: r.get(0)?,
                    subject: r.get(1)?,
                    path: r.get(2)?,
                    side: r.get(3)?,
                    line: r.get(4)?,
                    start_side: r.get(5)?,
                    start_line: r.get(6)?,
                    body: r.get(7)?,
                    anchor_rev: r.get(8)?,
                    snapshot: r.get(9)?,
                })
            },
        )?;
        let path = r.path.clone().unwrap_or_default();
        let old = match r.anchor_rev {
            Some(from) => file_row(c, from, &path)?,
            None => None,
        };
        // The same file in the new revision: same path, or the same original
        // (base) path after a rename.
        let base_path = old.as_ref().map(|f| f.prev_path.clone().unwrap_or_else(|| f.path.clone())).unwrap_or(path.clone());
        let new = match file_row(c, to_revision, &path)? {
            Some(f) => Some(f),
            None => {
                let p: Option<String> = c
                    .query_row(
                        "SELECT path FROM revision_file WHERE revision_id = ?1 AND COALESCE(prev_path, path) = ?2",
                        params![to_revision, base_path],
                        |r| r.get(0),
                    )
                    .optional()?;
                match p {
                    Some(p) => file_row(c, to_revision, &p)?,
                    None => None,
                }
            }
        };
        Ok((r, old, new))
    })?;
    if r.kind == "reply" || r.anchor_rev.is_none() {
        return Ok(None);
    }
    let text = |oid: &Option<String>| -> Result<Option<String>> {
        match oid {
            Some(o) => blobs.get_text(o),
            None => Ok(None),
        }
    };

    if r.subject == "FILE" {
        return Ok(Some(Proposal {
            status: if new.is_some() { Status::Clean } else { Status::Orphaned },
            path: new.as_ref().map(|f| f.path.clone()),
            side: None,
            line: None,
            start_side: None,
            start_line: None,
            confidence: if new.is_some() { 1.0 } else { 0.0 },
            old_lines: vec![],
            new_lines: vec![],
            suggestion_stale: false,
            file_in_diff: new.is_some(),
            to_revision,
        }));
    }

    let snapshot: AnchorSnapshot =
        r.snapshot.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
    // Without the old file (drafts pin their revision, so this shouldn't
    // happen), fall back to the snapshot's own blob ids.
    let old_base = old.as_ref().and_then(|f| f.base.clone()).or(snapshot.base_blob_oid.clone());
    let old_head = old.as_ref().and_then(|f| f.head.clone()).or(snapshot.head_blob_oid.clone());
    let (old_base_t, old_head_t) = (text(&old_base)?, text(&old_head)?);
    let (new_base_t, new_head_t) = match &new {
        Some(f) => (text(&f.base)?, text(&f.head)?),
        None => (None, None),
    };
    let hunks = match new.as_ref().and_then(|f| f.patch.as_deref()) {
        Some(p) => parse_patch(p).unwrap_or_default(),
        None => vec![],
    };
    let side = r.side.as_deref().and_then(Side::parse).unwrap_or(Side::Right);
    let start = match (r.start_side.as_deref().and_then(Side::parse), r.start_line) {
        (s, Some(l)) => Some((s.unwrap_or(side), l)),
        _ => None,
    };
    let input = Input {
        side,
        line: r.line.unwrap_or(1),
        start,
        body: &r.body,
        left: SideInput {
            snapshot: snapshot.left.as_ref(),
            old_text: old_base_t.as_deref(),
            new_text: new_base_t.as_deref(),
        },
        right: SideInput {
            snapshot: snapshot.right.as_ref(),
            old_text: old_head_t.as_deref(),
            new_text: new_head_t.as_deref(),
        },
        new_hunks: &hunks,
        new_path: new.as_ref().map(|f| f.path.clone()),
        file_in_diff: new.is_some(),
        has_patch: new.as_ref().is_some_and(|f| f.patch.is_some()),
        to_revision,
    };
    Ok(Some(propose(&input)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{local_patch, parse_patch};

    fn numbered(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}\n")).collect()
    }

    /// Base → old head (what was reviewed) and base → new head.
    struct Case {
        base: String,
        old_head: String,
        new_head: String,
    }

    fn snapshot_of(text: &str, first: u32, last: u32) -> SideSnapshot {
        let lines = split_lines(text);
        let get = |a: usize, b: usize| {
            lines[a.min(lines.len())..b.min(lines.len())].iter().map(|s| s.to_string()).collect()
        };
        SideSnapshot {
            first,
            last,
            lines: get(first as usize - 1, last as usize),
            before: get((first as usize).saturating_sub(4), first as usize - 1),
            after: get(last as usize, last as usize + 3),
        }
    }

    fn right(case: &Case, first: u32, last: u32, body: &str) -> Proposal {
        let snap = snapshot_of(&case.old_head, first, last);
        let hunks = parse_patch(&local_patch(&case.base, &case.new_head)).unwrap();
        propose(&Input {
            side: Side::Right,
            line: last,
            start: if first == last { None } else { Some((Side::Right, first)) },
            body,
            left: SideInput { snapshot: None, old_text: Some(&case.base), new_text: Some(&case.base) },
            right: SideInput {
                snapshot: Some(&snap),
                old_text: Some(&case.old_head),
                new_text: Some(&case.new_head),
            },
            new_hunks: &hunks,
            new_path: Some("f".into()),
            file_in_diff: true,
            has_patch: true,
            to_revision: 2,
        })
    }

    fn edit(text: &str, f: impl Fn(u32, &str) -> Option<String>) -> String {
        split_lines(text)
            .iter()
            .enumerate()
            .filter_map(|(i, l)| f(i as u32 + 1, l))
            .map(|l| l + "\n")
            .collect()
    }

    fn case() -> Case {
        let base = numbered(60);
        // The PR changes lines 20–22.
        let old_head = edit(&base, |i, l| {
            Some(if (20..=22).contains(&i) { format!("{l} (changed)") } else { l.into() })
        });
        Case { base, new_head: old_head.clone(), old_head }
    }

    #[test]
    fn lines_pushed_down_by_an_insertion_map_cleanly() {
        let mut c = case();
        c.new_head = format!("header 1\nheader 2\nheader 3\n{}", c.old_head);
        let p = right(&c, 21, 21, "x");
        assert_eq!((p.status, p.line), (Status::Clean, Some(24)));
        assert_eq!(p.new_lines, ["line 21 (changed)"]);
    }

    #[test]
    fn ranges_move_as_a_whole() {
        let mut c = case();
        c.new_head = format!("header\n{}", c.old_head);
        let p = right(&c, 20, 22, "x");
        assert_eq!((p.status, p.start_line, p.line), (Status::Clean, Some(21), Some(23)));
    }

    #[test]
    fn an_edited_line_is_a_fuzzy_match_at_its_new_place() {
        let mut c = case();
        c.new_head = format!(
            "header\n{}",
            edit(&c.old_head, |i, l| Some(if i == 21 { "line 21 (changed again)".into() } else { l.into() }))
        );
        let p = right(&c, 21, 21, "x");
        assert_eq!(p.status, Status::Fuzzy);
        assert_eq!(p.line, Some(22));
        assert!(p.confidence >= FUZZY_THRESHOLD && p.confidence < 1.0, "{}", p.confidence);
        assert_eq!(p.new_lines, ["line 21 (changed again)"]);
        assert!(!p.suggestion_stale);
    }

    #[test]
    fn a_suggestion_on_changed_code_is_flagged() {
        let mut c = case();
        c.new_head =
            edit(&c.old_head, |i, l| Some(if i == 21 { "line 21 (changed again)".into() } else { l.into() }));
        let p = right(&c, 21, 21, "```suggestion\nline 21 better\n```");
        assert_eq!(p.status, Status::Fuzzy);
        assert!(p.suggestion_stale);
    }

    #[test]
    fn deleted_lines_are_orphaned() {
        let mut c = case();
        c.new_head = edit(&c.old_head, |i, l| if (19..=23).contains(&i) { None } else { Some(l.into()) });
        let p = right(&c, 21, 21, "x");
        assert_eq!(p.status, Status::Orphaned);
    }

    #[test]
    fn a_reverted_change_is_not_commentable_any_more() {
        let mut c = case();
        // The author reverts lines 20–22 but changes line 50: line 21 is
        // back to the base and outside the diff.
        c.old_head = edit(&c.base, |i, l| Some(if i == 21 { "line 21".into() } else { l.into() }));
        c.old_head =
            edit(&c.old_head, |i, l| Some(if i == 20 { format!("{l} (changed)") } else { l.into() }));
        c.new_head = edit(&c.base, |i, l| Some(if i == 50 { format!("{l} (changed)") } else { l.into() }));
        let p = right(&c, 21, 21, "x");
        assert_eq!((p.status, p.line), (Status::NotCommentable, Some(21)));
    }

    #[test]
    fn left_side_comments_follow_the_base() {
        let c = case();
        let snap = snapshot_of(&c.base, 21, 21);
        // The base moved: a line was inserted at the top of the file.
        let new_base = format!("upstream\n{}", c.base);
        let new_head = format!("upstream\n{}", c.old_head);
        let hunks = parse_patch(&local_patch(&new_base, &new_head)).unwrap();
        let p = propose(&Input {
            side: Side::Left,
            line: 21,
            start: None,
            body: "x",
            left: SideInput { snapshot: Some(&snap), old_text: Some(&c.base), new_text: Some(&new_base) },
            right: SideInput { snapshot: None, old_text: Some(&c.old_head), new_text: Some(&new_head) },
            new_hunks: &hunks,
            new_path: Some("f".into()),
            file_in_diff: true,
            has_patch: true,
            to_revision: 2,
        });
        assert_eq!((p.status, p.side, p.line), (Status::Clean, Some(Side::Left), Some(22)));
    }

    #[test]
    fn a_file_that_left_the_diff_is_orphaned() {
        let c = case();
        let snap = snapshot_of(&c.old_head, 21, 21);
        let p = propose(&Input {
            side: Side::Right,
            line: 21,
            start: None,
            body: "x",
            left: SideInput { snapshot: None, old_text: None, new_text: None },
            right: SideInput { snapshot: Some(&snap), old_text: Some(&c.old_head), new_text: None },
            new_hunks: &[],
            new_path: None,
            file_in_diff: false,
            has_patch: false,
            to_revision: 2,
        });
        assert_eq!(p.status, Status::Orphaned);
        assert!(!p.file_in_diff);
    }

    #[test]
    fn line_map_and_translate() {
        let m = line_map("a\nb\nc\nd\n", "x\na\nc\nd\n");
        assert_eq!(m, [Some(2), None, Some(3), Some(4)]);
        assert_eq!(translate(&m, 2), 3);
    }
}

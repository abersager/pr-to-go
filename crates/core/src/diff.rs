//! Diff model. Hunks come from GitHub's own per-file patch, so the lines we
//! offer for comments are the lines GitHub accepts (DESIGN.md §8). When GitHub
//! sent no patch, we diff the stored blobs locally, for viewing only.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LineKind {
    Context,
    Add,
    Del,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "UPPERCASE")]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Left => "LEFT",
            Side::Right => "RIGHT",
        }
    }

    pub fn parse(s: &str) -> Option<Side> {
        match s {
            "LEFT" => Some(Side::Left),
            "RIGHT" => Some(Side::Right),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub text: String,
    /// Followed by `\ No newline at end of file`.
    pub no_newline: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub old_start: u32,
    pub old_len: u32,
    pub new_start: u32,
    pub new_len: u32,
    /// Text after the second `@@` (often the enclosing function).
    pub section: String,
    pub lines: Vec<DiffLine>,
}

fn parse_range(s: &str) -> Option<(u32, u32)> {
    match s.split_once(',') {
        Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

/// Parses `@@ -a,b +c,d @@ section`.
fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32, String)> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, section) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let (os, ol) = parse_range(old)?;
    let (ns, nl) = parse_range(new)?;
    Some((os, ol, ns, nl, section.trim_start().to_string()))
}

/// Parses a unified patch as GitHub returns it in `pulls/{n}/files`: hunks
/// only, no `diff --git`/`---`/`+++` headers (those are skipped if present).
pub fn parse_patch(patch: &str) -> Result<Vec<Hunk>, String> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut old_no, mut new_no) = (0u32, 0u32);
    let (mut old_left, mut new_left) = (0u32, 0u32);
    for raw in patch.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with("@@") {
            let (os, ol, ns, nl, section) =
                parse_hunk_header(line).ok_or_else(|| format!("bad hunk header: {line}"))?;
            hunks.push(Hunk {
                old_start: os,
                old_len: ol,
                new_start: ns,
                new_len: nl,
                section,
                lines: vec![],
            });
            (old_no, new_no, old_left, new_left) = (os, ns, ol, nl);
            continue;
        }
        let Some(h) = hunks.last_mut() else {
            // Headers before the first hunk.
            continue;
        };
        if line.starts_with('\\') {
            if let Some(last) = h.lines.last_mut() {
                last.no_newline = true;
            }
            continue;
        }
        if old_left == 0 && new_left == 0 {
            // Trailing text after a complete hunk (e.g. the final empty split).
            continue;
        }
        let (kind, text) = match line.as_bytes().first() {
            Some(b'+') => (LineKind::Add, &line[1..]),
            Some(b'-') => (LineKind::Del, &line[1..]),
            Some(b' ') => (LineKind::Context, &line[1..]),
            // Some tools strip the space from empty context lines.
            None => (LineKind::Context, ""),
            _ => return Err(format!("unexpected patch line: {line}")),
        };
        let dl = match kind {
            LineKind::Context => {
                let d = DiffLine {
                    kind,
                    old_no: Some(old_no),
                    new_no: Some(new_no),
                    text: text.into(),
                    no_newline: false,
                };
                old_no += 1;
                new_no += 1;
                old_left = old_left.saturating_sub(1);
                new_left = new_left.saturating_sub(1);
                d
            }
            LineKind::Add => {
                let d = DiffLine {
                    kind,
                    old_no: None,
                    new_no: Some(new_no),
                    text: text.into(),
                    no_newline: false,
                };
                new_no += 1;
                new_left = new_left.saturating_sub(1);
                d
            }
            LineKind::Del => {
                let d = DiffLine {
                    kind,
                    old_no: Some(old_no),
                    new_no: None,
                    text: text.into(),
                    no_newline: false,
                };
                old_no += 1;
                old_left = old_left.saturating_sub(1);
                d
            }
        };
        h.lines.push(dl);
    }
    Ok(hunks)
}

/// Diffs two texts locally, in the same hunk shape as GitHub's patches.
pub fn local_patch(old: &str, new: &str) -> String {
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .missing_newline_hint(true)
        .to_string()
}

/// Index of the hunk and of the line within it where `line` on `side` can
/// take a comment, if any.
pub fn locate(hunks: &[Hunk], side: Side, line: u32) -> Option<(usize, usize)> {
    for (hi, h) in hunks.iter().enumerate() {
        for (li, l) in h.lines.iter().enumerate() {
            let hit = match side {
                Side::Left => l.kind != LineKind::Add && l.old_no == Some(line),
                Side::Right => l.kind != LineKind::Del && l.new_no == Some(line),
            };
            if hit {
                return Some((hi, li));
            }
        }
    }
    None
}

/// Checks a comment range against GitHub's rules: both ends inside the same
/// hunk, start not after end. `start` is `None` for a single line.
pub fn check_range(hunks: &[Hunk], start: Option<(Side, u32)>, end: (Side, u32)) -> Result<(), String> {
    let (eh, el) = locate(hunks, end.0, end.1)
        .ok_or_else(|| format!("line {} ({}) isn't part of the diff", end.1, end.0.as_str()))?;
    if let Some((ss, sl)) = start {
        let (sh, sli) = locate(hunks, ss, sl)
            .ok_or_else(|| format!("line {sl} ({}) isn't part of the diff", ss.as_str()))?;
        if sh != eh {
            return Err("a comment range must stay within one hunk".into());
        }
        if sli > el {
            return Err("the range starts after it ends".into());
        }
    }
    Ok(())
}

/// The text lines of a file, as the diff shows them (no trailing newline,
/// no `\r`).
pub fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "@@ -1,4 +1,5 @@ fn main()\n line one\n-line two\n+line 2\n+line 2b\n line three\n line four\n@@ -10,2 +11,2 @@\n ten\n-eleven\n\\ No newline at end of file\n+11\n\\ No newline at end of file";

    #[test]
    fn parses_hunks_with_line_numbers() {
        let h = parse_patch(PATCH).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].section, "fn main()");
        let l = &h[0].lines;
        assert_eq!(l.len(), 6);
        assert_eq!((l[1].kind, l[1].old_no, l[1].new_no), (LineKind::Del, Some(2), None));
        assert_eq!((l[3].kind, l[3].old_no, l[3].new_no), (LineKind::Add, None, Some(3)));
        assert_eq!((l[5].old_no, l[5].new_no), (Some(4), Some(5)));
        let l = &h[1].lines;
        assert!(l[1].no_newline && l[2].no_newline);
        assert_eq!(l[2].new_no, Some(12));
    }

    #[test]
    fn parses_short_ranges_and_blank_context() {
        let h = parse_patch("@@ -0,0 +1 @@\n+only").unwrap();
        assert_eq!(h[0].lines[0].new_no, Some(1));
        let h = parse_patch("@@ -1,3 +1,3 @@\n a\n\n-b\n+c").unwrap();
        assert_eq!(h[0].lines[1].kind, LineKind::Context);
        assert_eq!(h[0].lines[1].new_no, Some(2));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_patch("@@ nonsense").is_err());
        assert!(parse_patch("@@ -1,1 +1,1 @@\n?what").is_err());
    }

    #[test]
    fn comment_rules() {
        let h = parse_patch(PATCH).unwrap();
        assert!(check_range(&h, None, (Side::Right, 3)).is_ok());
        assert!(check_range(&h, None, (Side::Left, 2)).is_ok());
        assert!(check_range(&h, None, (Side::Left, 3)).is_ok(), "context lines work on the left too");
        assert!(check_range(&h, None, (Side::Right, 8)).is_err(), "between hunks");
        assert!(check_range(&h, Some((Side::Left, 2)), (Side::Right, 3)).is_ok(), "ranges may cross sides");
        assert!(
            check_range(&h, Some((Side::Right, 1)), (Side::Right, 12)).is_err(),
            "ranges stay in one hunk"
        );
        assert!(check_range(&h, Some((Side::Right, 4)), (Side::Right, 2)).is_err());
    }

    #[test]
    fn local_patch_round_trips_through_the_parser() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n";
        let new = "a\nB\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm";
        let h = parse_patch(&local_patch(old, new)).unwrap();
        assert_eq!(h.len(), 2);
        let adds: Vec<_> = h.iter().flat_map(|h| &h.lines).filter(|l| l.kind == LineKind::Add).collect();
        assert_eq!(adds.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["B", "m"]);
        assert!(adds[1].no_newline);
        assert_eq!(adds[1].new_no, Some(13));
    }

    #[test]
    fn splits_lines_like_the_diff() {
        assert_eq!(split_lines("a\r\nb\n"), ["a", "b"]);
        assert_eq!(split_lines("a\nb"), ["a", "b"]);
        assert!(split_lines("").is_empty());
    }
}

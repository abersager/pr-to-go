//! Which files count as generated, so their diffs start collapsed (as on
//! GitHub). Sources, later ones winning:
//!
//! 1. Built-in patterns for lockfiles, minified files and the like.
//! 2. The repository's root `.gitattributes` (`linguist-generated`).
//! 3. The user's own patterns from Settings (`!pattern` means "not generated").
//!
//! The idea of honouring `linguist-generated` is borrowed from Hubtty.

use globset::{Glob, GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};

use crate::Result;
use crate::service::Core;

/// Setting key for the user's patterns, one per line.
pub const SETTING: &str = "generated_patterns";

const DEFAULTS: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "composer.lock",
    "Gemfile.lock",
    "Pipfile.lock",
    "poetry.lock",
    "uv.lock",
    "go.sum",
    "flake.lock",
    "*.min.js",
    "*.min.css",
    "*.js.map",
    "*.css.map",
    "*.pb.go",
    "*_pb2.py",
    "*_pb2_grpc.py",
];

pub struct Generated {
    defaults: GlobSet,
    /// In order; `None` resets to the default for that path.
    rules: Vec<(GlobMatcher, Option<bool>)>,
}

/// A gitattributes pattern as a glob over the repository-relative path: a
/// pattern without a slash matches the file name at any depth, one with a
/// slash is relative to the root.
fn glob(pattern: &str) -> Option<GlobMatcher> {
    let p = pattern.trim_end_matches('/');
    let p = match p.strip_prefix('/') {
        Some(anchored) => anchored.to_string(),
        None if p.contains('/') => p.to_string(),
        None => format!("**/{p}"),
    };
    GlobBuilder::new(&p).literal_separator(true).build().ok().map(|g| g.compile_matcher())
}

impl Generated {
    pub fn new(gitattributes: Option<&str>, user_patterns: Option<&str>) -> Generated {
        let mut defaults = GlobSetBuilder::new();
        for p in DEFAULTS {
            if let Ok(g) = Glob::new(&format!("**/{p}")) {
                defaults.add(g);
            }
        }
        let mut rules = Vec::new();
        for line in gitattributes.unwrap_or("").lines() {
            let mut parts = line.split_whitespace();
            let Some(pattern) = parts.next().filter(|p| !p.starts_with('#')) else { continue };
            let value = parts.fold(None, |acc, attr| match attr {
                "linguist-generated" | "linguist-generated=true" => Some(Some(true)),
                "-linguist-generated" | "linguist-generated=false" => Some(Some(false)),
                "!linguist-generated" => Some(None),
                _ => acc,
            });
            if let (Some(value), Some(m)) = (value, glob(pattern)) {
                rules.push((m, value));
            }
        }
        for line in user_patterns.unwrap_or("").lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (pattern, value) = match line.strip_prefix('!') {
                Some(p) => (p.trim(), false),
                None => (line, true),
            };
            if let Some(m) = glob(pattern) {
                rules.push((m, Some(value)));
            }
        }
        Generated { defaults: defaults.build().unwrap_or_else(|_| GlobSet::empty()), rules }
    }

    pub fn is_generated(&self, path: &str) -> bool {
        let default = self.defaults.is_match(path);
        self.rules.iter().rfind(|(m, _)| m.is_match(path)).map_or(default, |(_, v)| v.unwrap_or(default))
    }
}

impl Core {
    /// The user's patterns, one per line (`!pattern` for "not generated").
    pub fn generated_patterns(&self) -> Result<String> {
        Ok(self.db.get_setting(SETTING)?.unwrap_or_default())
    }

    pub fn set_generated_patterns(&self, patterns: &str) -> Result<()> {
        self.db.set_setting(SETTING, patterns)
    }
}

#[cfg(test)]
mod tests {
    use super::Generated;

    #[test]
    fn built_in_patterns() {
        let g = Generated::new(None, None);
        assert!(g.is_generated("Cargo.lock"));
        assert!(g.is_generated("web/package-lock.json"));
        assert!(g.is_generated("dist/app.min.js"));
        assert!(!g.is_generated("src/lib.rs"));
        assert!(!g.is_generated("docs/Cargo.lock.md"));
    }

    #[test]
    fn gitattributes_follow_git_pattern_rules() {
        let attrs = "# comment\n\
                     tests/fixtures/*.json linguist-generated\n\
                     *.pb.rs text linguist-generated=true\n\
                     /gen/** linguist-generated\n\
                     Cargo.lock -linguist-generated\n\
                     yarn.lock !linguist-generated\n\
                     *.md diff=markdown\n";
        let g = Generated::new(Some(attrs), None);
        // A pattern with a slash is anchored at the root; `*` stays in one directory.
        assert!(g.is_generated("tests/fixtures/widgets.json"));
        assert!(!g.is_generated("tests/fixtures/deep/widgets.json"));
        assert!(!g.is_generated("other/tests/fixtures/widgets.json"));
        // Without a slash it matches the file name anywhere.
        assert!(g.is_generated("proto/v1/api.pb.rs"));
        assert!(g.is_generated("gen/a/b.rs"));
        assert!(!g.is_generated("src/gen/b.rs"));
        // Unset and unspecified.
        assert!(!g.is_generated("Cargo.lock"));
        assert!(g.is_generated("yarn.lock"));
        assert!(!g.is_generated("README.md"));
    }

    #[test]
    fn later_rules_and_user_patterns_win() {
        let attrs = "*.json linguist-generated\nconfig.json -linguist-generated\n";
        let user = "!pnpm-lock.yaml\n  # mine\nsnapshots/**\n!snapshots/keep.snap\n";
        let g = Generated::new(Some(attrs), Some(user));
        assert!(g.is_generated("data/a.json"));
        assert!(!g.is_generated("app/config.json"));
        assert!(!g.is_generated("pnpm-lock.yaml"));
        assert!(g.is_generated("snapshots/a.snap"));
        assert!(!g.is_generated("snapshots/keep.snap"));
    }
}

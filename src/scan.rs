//! Directory scanning.
//!
//! Produces a flat list of files that look like skill or agent markdown.
//! Scans follow a strict policy:
//!
//! - Symlinks are skipped by default (and surface as [`IssueKind::Symlink`]
//!   notes). `--follow-symlinks` opts in.
//! - Files larger than `--max-file-size` are skipped and produce an
//!   [`IssueKind::Oversize`] error.
//! - `.skilldigest.toml` ignore globs are applied.
//! - Path traversal outside the scan root is rejected.

use std::fs;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use walkdir::WalkDir;

use crate::error::{Error, Result};
use crate::model::{Issue, IssueKind, Location, SkillId};

/// Policy controlling what the scanner does.
#[derive(Clone, Debug)]
pub struct ScanPolicy {
    /// Follow symlinks (default: false).
    pub follow_symlinks: bool,
    /// Skip files larger than this many bytes (default 1 MiB).
    pub max_file_size: u64,
    /// Gitignore-style globs to skip.
    pub ignore_globs: Vec<String>,
    /// File-name patterns considered skill files. Default matches common
    /// skill markdown conventions.
    pub skill_globs: Vec<String>,
}

impl Default for ScanPolicy {
    fn default() -> Self {
        Self {
            follow_symlinks: false,
            max_file_size: 1024 * 1024,
            ignore_globs: Vec::new(),
            skill_globs: vec![
                "**/SKILL.md".into(),
                "**/skill.md".into(),
                "**/AGENT.md".into(),
                "**/agent.md".into(),
                "**/AGENTS.md".into(),
                "**/CLAUDE.md".into(),
                "**/GEMINI.md".into(),
                "**/*.cursorrules".into(),
                "**/*.skill.md".into(),
                "**/.cursor/rules/**/*.md".into(),
                "**/.cursor/rules/**/*.mdc".into(),
                "**/.claude/skills/**/*.md".into(),
                "**/.claude/commands/**/*.md".into(),
                "**/plugin.toml".into(),
                "**/index.md".into(),
                "**/README.md".into(),
            ],
        }
    }
}

/// A file the scanner selected for parsing.
#[derive(Clone, Debug)]
pub struct DiscoveredFile {
    /// Absolute path on disk.
    pub absolute: PathBuf,
    /// Path relative to the scan root (normalised to forward slashes).
    pub relative: PathBuf,
    /// Size in bytes.
    pub size: u64,
    /// File bytes.
    pub bytes: Vec<u8>,
}

/// Scan output.
#[derive(Debug, Default)]
pub struct ScanOutput {
    /// Files selected for parsing.
    pub files: Vec<DiscoveredFile>,
    /// Issues found *during* the scan (before parsing), such as oversize,
    /// symlink, duplicate.
    pub issues: Vec<Issue>,
}

/// Walk `root` according to the policy.
pub fn scan_dir(root: &Path, policy: &ScanPolicy) -> Result<ScanOutput> {
    if !root.exists() {
        return Err(Error::BadRoot(root.to_path_buf()));
    }
    if !root.is_dir() {
        return Err(Error::BadRoot(root.to_path_buf()));
    }

    let ignore = build_globset(&policy.ignore_globs)?;
    let skill = build_globset(&policy.skill_globs)?;

    let canonical_root = match fs::canonicalize(root) {
        Ok(p) => p,
        Err(e) => return Err(Error::io(root, e)),
    };

    let mut output = ScanOutput::default();
    let walker = WalkDir::new(root)
        .follow_links(policy.follow_symlinks)
        .into_iter()
        .filter_entry(|e| {
            let rel = e.path().strip_prefix(root).unwrap_or_else(|_| e.path());
            !is_ignored(rel, &ignore)
        });

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                // surface as a non-fatal note
                let path = err.path().map(Path::to_path_buf).unwrap_or_default();
                output.issues.push(
                    Issue::new(
                        IssueKind::Symlink,
                        SkillId::new(path.to_string_lossy().as_ref()),
                        format!("walkdir error: {err}"),
                    )
                    .with_location(Location::start_of(path)),
                );
                continue;
            }
        };
        let ft = entry.file_type();
        if ft.is_symlink() && !policy.follow_symlinks {
            let rel = entry
                .path()
                .strip_prefix(root)
                .unwrap_or(entry.path())
                .to_path_buf();
            output.issues.push(
                Issue::new(
                    IssueKind::Symlink,
                    SkillId::new(rel.to_string_lossy().as_ref()),
                    format!("symlink skipped: {} (use --follow-symlinks)", rel.display()),
                )
                .with_location(Location::start_of(rel)),
            );
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        let rel = match entry.path().strip_prefix(root) {
            Ok(p) => p.to_path_buf(),
            Err(_) => continue,
        };

        // Path traversal guard — canonicalize and verify we stay under root.
        if let Ok(canonical) = fs::canonicalize(entry.path()) {
            if !canonical.starts_with(&canonical_root) {
                output.issues.push(
                    Issue::new(
                        IssueKind::Symlink,
                        SkillId::new(rel.to_string_lossy().as_ref()),
                        format!("path escapes scan root: {}", rel.display()),
                    )
                    .with_location(Location::start_of(rel)),
                );
                continue;
            }
        }

        if !skill.is_match(rel.to_string_lossy().as_ref()) {
            continue;
        }

        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                return Err(Error::io(entry.path(), std::io::Error::from(e)));
            }
        };
        let size = meta.len();
        if size > policy.max_file_size {
            let rel_id = SkillId::new(rel.to_string_lossy().as_ref());
            output.issues.push(
                Issue::new(
                    IssueKind::Oversize,
                    rel_id,
                    format!(
                        "file size {} bytes exceeds max {} bytes",
                        size, policy.max_file_size
                    ),
                )
                .with_location(Location::start_of(rel.clone())),
            );
            continue;
        }

        let bytes = match fs::read(entry.path()) {
            Ok(b) => b,
            Err(e) => return Err(Error::io(entry.path(), e)),
        };

        output.files.push(DiscoveredFile {
            absolute: entry.path().to_path_buf(),
            relative: normalise(&rel),
            size,
            bytes,
        });
    }

    // Deterministic ordering.
    output.files.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(output)
}

fn build_globset(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        let g = Glob::new(p).map_err(|e| Error::bad_arg(format!("bad glob {p:?}: {e}")))?;
        builder.add(g);
    }
    builder
        .build()
        .map_err(|e| Error::bad_arg(format!("globset build: {e}")))
}

fn is_ignored(path: &Path, ignore: &GlobSet) -> bool {
    let s = path.to_string_lossy();
    if s.starts_with(".git/") || s == ".git" {
        return true;
    }
    if s.starts_with("target/") || s == "target" {
        return true;
    }
    if s.starts_with("node_modules/") || s == "node_modules" {
        return true;
    }
    ignore.is_match(s.as_ref())
}

fn normalise(p: &Path) -> PathBuf {
    PathBuf::from(p.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(base: &Path, rel: &str, content: &[u8]) {
        let path = base.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn default_policy_is_sensible() {
        let p = ScanPolicy::default();
        assert!(!p.follow_symlinks);
        assert_eq!(p.max_file_size, 1024 * 1024);
        assert!(p.skill_globs.iter().any(|g| g.contains("SKILL.md")));
    }

    #[test]
    fn scan_picks_up_skill_files() {
        let dir = tempdir().unwrap();
        write(dir.path(), "a/SKILL.md", b"---\n---\nbody\n");
        write(dir.path(), "b/skill.md", b"body\n");
        write(dir.path(), "c/notes.txt", b"ignored\n");
        let out = scan_dir(dir.path(), &ScanPolicy::default()).unwrap();
        assert_eq!(out.files.len(), 2);
    }

    #[test]
    fn scan_rejects_non_directory() {
        let err = scan_dir(
            Path::new("/definitely/does/not/exist"),
            &ScanPolicy::default(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::BadRoot(_)));
    }

    #[test]
    fn scan_applies_ignore_globs() {
        let dir = tempdir().unwrap();
        write(dir.path(), "keep/SKILL.md", b"body");
        write(dir.path(), "archive/SKILL.md", b"body");
        let policy = ScanPolicy {
            ignore_globs: vec!["archive/**".into()],
            ..ScanPolicy::default()
        };
        let out = scan_dir(dir.path(), &policy).unwrap();
        assert_eq!(out.files.len(), 1);
        assert!(out.files[0].relative.starts_with("keep"));
    }

    #[test]
    fn scan_flags_oversize() {
        let dir = tempdir().unwrap();
        write(dir.path(), "a/SKILL.md", &vec![b'x'; 4096]);
        let policy = ScanPolicy {
            max_file_size: 1024,
            ..ScanPolicy::default()
        };
        let out = scan_dir(dir.path(), &policy).unwrap();
        assert!(out.files.is_empty());
        assert!(out.issues.iter().any(|i| i.kind == IssueKind::Oversize));
    }

    #[test]
    fn scan_is_deterministic() {
        let dir = tempdir().unwrap();
        write(dir.path(), "b/SKILL.md", b"body");
        write(dir.path(), "a/SKILL.md", b"body");
        write(dir.path(), "c/SKILL.md", b"body");
        let out = scan_dir(dir.path(), &ScanPolicy::default()).unwrap();
        let ids: Vec<_> = out
            .files
            .iter()
            .map(|f| f.relative.to_string_lossy().into_owned())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn scan_reads_file_bytes() {
        let dir = tempdir().unwrap();
        write(dir.path(), "a/SKILL.md", b"hello world");
        let out = scan_dir(dir.path(), &ScanPolicy::default()).unwrap();
        assert_eq!(out.files[0].bytes, b"hello world");
    }

    #[test]
    fn scan_skips_git_and_target_dirs() {
        let dir = tempdir().unwrap();
        write(dir.path(), ".git/SKILL.md", b"body");
        write(dir.path(), "target/SKILL.md", b"body");
        write(dir.path(), "node_modules/SKILL.md", b"body");
        write(dir.path(), "real/SKILL.md", b"body");
        let out = scan_dir(dir.path(), &ScanPolicy::default()).unwrap();
        assert_eq!(out.files.len(), 1);
        assert!(out.files[0].relative.starts_with("real"));
    }

    #[test]
    fn normalise_replaces_backslash() {
        let p = normalise(Path::new("foo\\bar.md"));
        assert_eq!(p.to_string_lossy(), "foo/bar.md");
    }

    #[test]
    fn build_globset_rejects_bad_pattern() {
        let err = build_globset(&["***invalid*[".into()]).unwrap_err();
        assert!(matches!(err, Error::BadArg(_)));
    }

    #[test]
    fn scan_picks_up_cursorrules() {
        let dir = tempdir().unwrap();
        write(dir.path(), ".cursorrules", b"rules");
        write(dir.path(), "keep/.cursorrules", b"rules");
        let out = scan_dir(dir.path(), &ScanPolicy::default()).unwrap();
        assert!(out
            .files
            .iter()
            .any(|f| f.relative.ends_with(".cursorrules")));
    }

    #[test]
    fn scan_uses_agents_md() {
        let dir = tempdir().unwrap();
        write(dir.path(), "AGENTS.md", b"body");
        let out = scan_dir(dir.path(), &ScanPolicy::default()).unwrap();
        assert_eq!(out.files.len(), 1);
    }
}

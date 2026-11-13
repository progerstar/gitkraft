//! Adding files, folders and extensions to the repository's `.gitignore`
//! (the "Ignore file / folder / all .ext files" menu items of GitHub Desktop).

use std::path::Path;

use anyhow::{Context, Result, bail};
use tracing::debug;

/// Name of git's ignore file.
pub const GITIGNORE: &str = ".gitignore";

/// What to add to the `.gitignore` at the repository root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IgnoreTarget {
    /// These files (repo-relative paths, `/`-separated).
    Files(Vec<String>),
    /// A folder and everything in it (repo-relative path, no trailing `/`).
    Folder(String),
    /// Every file with this extension (without the dot), in any folder.
    Extension(String),
}

impl IgnoreTarget {
    /// The `.gitignore` lines for this target. Paths are anchored to the
    /// repository root (`/a/b.txt`), so only that file or folder is ignored,
    /// and characters git would read as wildcards are escaped.
    pub fn patterns(&self) -> Vec<String> {
        match self {
            Self::Files(paths) => paths.iter().map(|p| format!("/{}", escape(p))).collect(),
            Self::Folder(dir) => vec![format!("/{}/", escape(dir.trim_end_matches('/')))],
            Self::Extension(ext) => vec![format!("*.{}", escape(ext))],
        }
    }

    /// Short description for status messages: `'a/b.txt'`, `3 files`,
    /// `'a/'`, `'*.log'`.
    pub fn label(&self) -> String {
        match self {
            Self::Files(paths) if paths.len() == 1 => format!("'{}'", paths[0]),
            Self::Files(paths) => format!("{} files", paths.len()),
            Self::Folder(dir) => format!("'{}/'", dir.trim_end_matches('/')),
            Self::Extension(ext) => format!("'*.{ext}'"),
        }
    }
}

/// Escape the characters git's ignore patterns treat specially, so `path`
/// matches only itself. Trailing spaces are kept (git drops them unless
/// escaped). `!` and `#` only matter at the start of a line, which is always
/// `/` or `*.` here.
fn escape(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for ch in path.chars() {
        if matches!(ch, '\\' | '*' | '?' | '[' | ']') {
            out.push('\\');
        }
        out.push(ch);
    }
    let kept = out.trim_end_matches(' ').len();
    let trailing = out.len() - kept;
    out.truncate(kept);
    out.push_str(&"\\ ".repeat(trailing));
    out
}

/// Folders containing `path`, deepest first: `a/b/c.txt` gives `a/b`, `a`.
pub fn parent_folders(path: &str) -> Vec<&str> {
    let mut dirs = Vec::new();
    let mut rest = path.trim_end_matches('/');
    while let Some((dir, _)) = rest.rsplit_once('/') {
        if dir.is_empty() {
            break;
        }
        dirs.push(dir);
        rest = dir;
    }
    dirs
}

/// The extension of the file at `path` (without the dot), if it has one.
/// Dotfiles like `.env` have none.
pub fn file_extension(path: &str) -> Option<&str> {
    Path::new(crate::utils::text::path_basename(path))
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty())
}

/// Whether `path` is a `.gitignore` file (ignoring one makes no sense).
pub fn is_gitignore(path: &str) -> bool {
    crate::utils::text::path_basename(path) == GITIGNORE
}

/// Append `target`'s patterns to `.gitignore` in `workdir`, creating the file
/// if needed. Patterns already listed are skipped; the file's line endings
/// (LF or CRLF) are kept. Returns how many patterns were added.
pub fn add_to_gitignore(workdir: &Path, target: &IgnoreTarget) -> Result<usize> {
    let patterns = target.patterns();
    if patterns.iter().any(|p| p.contains(['\n', '\r'])) {
        bail!("a path with a line break cannot be added to {GITIGNORE}");
    }

    let file = workdir.join(GITIGNORE);
    let current = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", file.display())),
    };

    let mut new: Vec<&str> = Vec::new();
    for pattern in &patterns {
        let listed = current.lines().any(|l| l.trim_end_matches('\r') == pattern);
        if !listed && !new.contains(&pattern.as_str()) {
            new.push(pattern);
        }
    }
    if new.is_empty() {
        return Ok(0);
    }

    let eol = if current.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut text = current;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push_str(eol);
    }
    for pattern in &new {
        text.push_str(pattern);
        text.push_str(eol);
    }
    debug!("adding {:?} to {}", new, file.display());
    std::fs::write(&file, text).with_context(|| format!("failed to write {}", file.display()))?;
    Ok(new.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn files(paths: &[&str]) -> IgnoreTarget {
        IgnoreTarget::Files(paths.iter().map(|p| p.to_string()).collect())
    }

    #[test]
    fn patterns_are_anchored_to_the_root() {
        assert_eq!(files(&["a/b.txt", "c"]).patterns(), ["/a/b.txt", "/c"]);
        assert_eq!(IgnoreTarget::Folder("a/b".into()).patterns(), ["/a/b/"]);
        assert_eq!(IgnoreTarget::Extension("log".into()).patterns(), ["*.log"]);
    }

    #[test]
    fn wildcards_and_trailing_spaces_are_escaped() {
        assert_eq!(
            files(&["a[1]/*b?.txt", "back\\slash", "!x", "#y", "sp  "]).patterns(),
            [
                r"/a\[1\]/\*b\?.txt",
                r"/back\\slash",
                "/!x",
                "/#y",
                r"/sp\ \ ",
            ]
        );
    }

    #[test]
    fn labels() {
        assert_eq!(files(&["a/b.txt"]).label(), "'a/b.txt'");
        assert_eq!(files(&["a", "b", "c"]).label(), "3 files");
        assert_eq!(IgnoreTarget::Folder("a/b".into()).label(), "'a/b/'");
        assert_eq!(IgnoreTarget::Extension("log".into()).label(), "'*.log'");
    }

    #[test]
    fn parent_folders_deepest_first() {
        assert_eq!(parent_folders("a/b/c.txt"), ["a/b", "a"]);
        assert_eq!(parent_folders("c.txt"), Vec::<&str>::new());
        assert_eq!(parent_folders("a/b/"), ["a"]);
    }

    #[test]
    fn extensions() {
        assert_eq!(file_extension("src/main.rs"), Some("rs"));
        assert_eq!(file_extension("a.tar.gz"), Some("gz"));
        assert_eq!(file_extension(".env"), None);
        assert_eq!(file_extension("dir.d/Makefile"), None);
        assert_eq!(file_extension("trailing."), None);
    }

    #[test]
    fn recognises_gitignore_files() {
        assert!(is_gitignore(".gitignore"));
        assert!(is_gitignore("sub/.gitignore"));
        assert!(!is_gitignore("a.gitignore"));
    }

    #[test]
    fn creates_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let added = add_to_gitignore(tmp.path(), &files(&["secret.env"])).unwrap();
        assert_eq!(added, 1);
        let text = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert_eq!(text, "/secret.env\n");
    }

    #[test]
    fn appends_after_a_missing_final_newline_and_skips_listed_patterns() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".gitignore");
        fs::write(&path, "/target\n/a.txt").unwrap();

        let added = add_to_gitignore(tmp.path(), &files(&["a.txt", "b.txt", "b.txt"])).unwrap();
        assert_eq!(added, 1);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "/target\n/a.txt\n/b.txt\n"
        );

        assert_eq!(add_to_gitignore(tmp.path(), &files(&["b.txt"])).unwrap(), 0);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "/target\n/a.txt\n/b.txt\n"
        );
    }

    #[test]
    fn keeps_crlf_line_endings() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".gitignore");
        fs::write(&path, "/target\r\n").unwrap();

        add_to_gitignore(tmp.path(), &IgnoreTarget::Extension("log".into())).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "/target\r\n*.log\r\n");
        // A CRLF line counts as listed.
        assert_eq!(
            add_to_gitignore(tmp.path(), &IgnoreTarget::Extension("log".into())).unwrap(),
            0
        );
    }

    #[test]
    fn rejects_line_breaks() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(add_to_gitignore(tmp.path(), &files(&["a\nb"])).is_err());
        assert!(!tmp.path().join(".gitignore").exists());
    }

    #[test]
    fn git_ignores_exactly_the_added_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(tmp.path()).unwrap();
        let wd = tmp.path();

        add_to_gitignore(wd, &files(&["a[1].txt", "notes.md", "sp "])).unwrap();
        add_to_gitignore(wd, &IgnoreTarget::Folder("build/out".into())).unwrap();
        add_to_gitignore(wd, &IgnoreTarget::Extension("log".into())).unwrap();

        let ignored = |p: &str| repo.is_path_ignored(p).unwrap();
        assert!(ignored("a[1].txt"));
        assert!(!ignored("a1.txt"), "brackets must not act as a wildcard");
        assert!(ignored("notes.md"));
        assert!(
            !ignored("docs/notes.md"),
            "file patterns are anchored to the root"
        );
        assert!(ignored("build/out/x.o"));
        assert!(!ignored("build/other.o"));
        assert!(ignored("deep/dir/run.log"));
        assert!(!ignored("run.logs"));
        assert!(ignored("sp "));
        assert!(
            !ignored("sp"),
            "an escaped trailing space is part of the name"
        );
    }
}

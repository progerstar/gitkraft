//! Commit message generation through an external command (an AI agent CLI).
//!
//! The staged diff goes to the command's stdin; whatever it prints to stdout
//! becomes the commit message. The command is user-configurable, so any CLI
//! that reads a prompt plus stdin works.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use git2::Repository;

/// Used when `commit_message_command` is not set in `settings.json`.
/// Haiku: commit messages don't need a big model, and it is fast and cheap.
pub const DEFAULT_COMMIT_MESSAGE_COMMAND: &str = "claude -p --model haiku \"Write a git commit \
message for the staged changes given on stdin: a list of all changed files, then their diffs \
(long diffs are shortened). First line: one short imperative sentence of at most 72 \
characters. Then a blank line and a short description of what changed and why. Output only \
the commit message as plain text: no preamble, no remarks about the input or its length, no \
code fences.\"";

/// Total size of the per-file diffs sent to the agent.
const MAX_PATCH_BYTES: usize = 120 * 1024;
/// One file's diff is cut beyond this, so a single huge file cannot crowd
/// out the others.
const MAX_FILE_PATCH_BYTES: usize = 16 * 1024;

/// How long the command may run before it is killed.
const TIMEOUT: Duration = Duration::from_secs(300);

/// Text describing the staged changes (HEAD → index) for the agent: a list of
/// every changed file with its line counts, then each file's diff, shortened
/// per file and in total. Errors when nothing is staged.
pub fn staged_patch(repo: &Repository) -> Result<String> {
    let head_tree = repo.head().ok().and_then(|r| r.peel_to_tree().ok());
    let diff = repo
        .diff_tree_to_index(head_tree.as_ref(), None, None)
        .context("failed to diff HEAD to index")?;
    let count = diff.deltas().len();
    anyhow::ensure!(count > 0, "nothing is staged");

    let mut files = String::new();
    let mut patches = String::new();
    let mut omitted = 0usize;
    for idx in 0..count {
        let Some(delta) = diff.get_delta(idx) else {
            continue;
        };
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let status = crate::features::diff::FileStatus::from_delta(delta.status());

        let patch = git2::Patch::from_diff(&diff, idx).context("failed to build file diff")?;
        let (adds, dels) = match &patch {
            Some(p) => p.line_stats().map(|(_, a, d)| (a, d)).unwrap_or((0, 0)),
            None => (0, 0),
        };
        files.push_str(&format!("{status} {path} (+{adds} -{dels})\n"));

        let text = match patch {
            Some(mut p) => String::from_utf8_lossy(&p.to_buf()?).into_owned(),
            None => format!("Binary file {path}\n"),
        };
        if patches.len() >= MAX_PATCH_BYTES {
            omitted += 1;
            continue;
        }
        let room = MAX_FILE_PATCH_BYTES.min(MAX_PATCH_BYTES - patches.len());
        if text.len() > room {
            patches.push_str(cut_at_char(&text, room));
            patches.push_str("\n[rest of this file's diff omitted]\n");
        } else {
            patches.push_str(&text);
        }
    }

    let mut out = format!("Changed files ({count}):\n{files}\n");
    out.push_str(&patches);
    if omitted > 0 {
        out.push_str(&format!(
            "\n[diffs of {omitted} more file(s) omitted; see the list above]\n"
        ));
    }
    Ok(out)
}

/// `text` cut to at most `max` bytes, on a character boundary.
fn cut_at_char(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Run `command` through the shell in `workdir`, feed it `patch` on stdin
/// and return its cleaned-up stdout.
pub fn generate_commit_message(workdir: &Path, command: &str, patch: &str) -> Result<String> {
    let mut cmd = shell_command(command);
    cmd.current_dir(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = path_with_user_bins() {
        cmd.env("PATH", path);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to start `{command}`"))?;

    // Write stdin and drain stdout/stderr on their own threads, so a large
    // diff or a chatty command cannot deadlock on full pipes.
    let input = patch.to_owned();
    let mut stdin = child.stdin.take().context("no stdin pipe")?;
    std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let stdout = drain(child.stdout.take().context("no stdout pipe")?);
    let stderr = drain(child.stderr.take().context("no stderr pipe")?);

    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().context("failed to wait for command")? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("`{command}` timed out after {} s", TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(100));
    };

    let out = stdout.join().unwrap_or_default();
    let err = stderr.join().unwrap_or_default();
    if !status.success() {
        let err = err.trim();
        anyhow::bail!(
            "`{command}` failed ({status}){}{err}",
            if err.is_empty() { "" } else { ": " }
        );
    }

    let message = clean_output(&out);
    anyhow::ensure!(!message.is_empty(), "the command printed no message");
    Ok(message)
}

/// Longest summary we keep; the prompt asks the agent for the same.
pub const SUMMARY_MAX_CHARS: usize = 72;

/// Split a full commit message into its summary (first line) and the
/// description (the rest, without the separating blank lines).
///
/// Agents sometimes write a whole paragraph on the first line. A first line
/// over [`SUMMARY_MAX_CHARS`] is cut after its last sentence that fits (or at
/// the last word boundary that fits) and the remainder opens the description.
pub fn split_commit_message(message: &str) -> (String, String) {
    let message = message.trim();
    let (first, rest) = match message.split_once('\n') {
        Some((first, rest)) => (first.trim(), rest.trim()),
        None => (message, ""),
    };
    if first.chars().count() <= SUMMARY_MAX_CHARS {
        return (first.to_string(), rest.to_string());
    }

    let cut = summary_cut(first);
    let summary = first[..cut]
        .trim_end()
        .trim_end_matches([',', ';', ':', '-', '—'])
        .trim_end();
    let overflow = first[cut..].trim();
    let description = [overflow, rest]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (summary.to_string(), description)
}

/// Byte index to cut an over-long first line at.
fn summary_cut(line: &str) -> usize {
    // Byte offset just past the SUMMARY_MAX_CHARS-th character.
    let limit = line
        .char_indices()
        .nth(SUMMARY_MAX_CHARS)
        .map_or(line.len(), |(i, _)| i);
    let head = &line[..limit];
    // Prefer the end of the last whole sentence that fits…
    let sentence_end = head
        .rmatch_indices(['.', '!', '?'])
        .map(|(i, _)| i + 1)
        .find(|&end| end > 1 && line[end..].starts_with(' '));
    // …else the last space, else a hard cut.
    sentence_end
        .or_else(|| head.rfind(' ').filter(|&i| i > 0))
        .unwrap_or(limit)
}

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    }
}

/// Apps started from a desktop launcher often lack `~/.local/bin` and
/// `~/.cargo/bin` in `PATH`, where agent CLIs usually live.
fn path_with_user_bins() -> Option<std::ffi::OsString> {
    let home = dirs::home_dir()?;
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<std::path::PathBuf> = std::env::split_paths(&current).collect();
    for extra in [home.join(".local/bin"), home.join(".cargo/bin")] {
        if extra.is_dir() && !paths.contains(&extra) {
            paths.push(extra);
        }
    }
    std::env::join_paths(paths).ok()
}

fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    })
}

/// Trim and drop a surrounding Markdown code fence, which models like to add.
fn clean_output(raw: &str) -> String {
    let text = raw.trim();
    let unfenced = text
        .strip_prefix("```")
        .and_then(|rest| rest.split_once('\n').map(|(_, body)| body))
        .and_then(|body| body.trim_end().strip_suffix("```"));
    unfenced.unwrap_or(text).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn repo_with_staged_change(dir: &Path) -> Repository {
        let repo = Repository::init(dir).unwrap();
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("a.txt")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        drop(tree);
        fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        index.add_path(Path::new("a.txt")).unwrap();
        index.write().unwrap();
        repo
    }

    #[test]
    fn staged_patch_contains_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo_with_staged_change(tmp.path());
        let patch = staged_patch(&repo).unwrap();
        assert!(
            patch.starts_with("Changed files (1):\nM a.txt (+1 -0)\n"),
            "{patch}"
        );
        assert!(patch.contains("+two"), "{patch}");
    }

    #[test]
    fn staged_patch_lists_every_file_even_when_diffs_are_cut() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo_with_staged_change(tmp.path());
        let mut index = repo.index().unwrap();
        // Ten big new files: their diffs exceed the total budget.
        let big = "line of text that is long enough\n".repeat(2000);
        for i in 0..10 {
            let name = format!("big{i}.md");
            fs::write(tmp.path().join(&name), &big).unwrap();
            index.add_path(Path::new(&name)).unwrap();
        }
        index.write().unwrap();

        let patch = staged_patch(&repo).unwrap();
        assert!(
            patch.starts_with("Changed files (11):"),
            "{}",
            &patch[..200]
        );
        for i in 0..10 {
            assert!(patch.contains(&format!("A big{i}.md (+2000 -0)")));
        }
        assert!(patch.contains("[rest of this file's diff omitted]"));
        assert!(patch.len() < MAX_PATCH_BYTES + 4096, "{}", patch.len());
    }

    #[test]
    fn staged_patch_errors_when_nothing_staged() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo_with_staged_change(tmp.path());
        let head = repo.head().unwrap().peel_to_tree().unwrap();
        let mut index = repo.index().unwrap();
        index.read_tree(&head).unwrap();
        index.write().unwrap();
        assert!(staged_patch(&repo).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn generate_reads_stdin_and_returns_stdout() {
        let tmp = tempfile::tempdir().unwrap();
        let msg = generate_commit_message(
            tmp.path(),
            "printf 'Add line\\n\\n'; wc -l | tr -d ' '",
            "a\nb\nc\n",
        )
        .unwrap();
        assert_eq!(msg, "Add line\n\n3");
    }

    #[cfg(unix)]
    #[test]
    fn generate_reports_failure_with_stderr() {
        let tmp = tempfile::tempdir().unwrap();
        let err = generate_commit_message(tmp.path(), "echo boom >&2; exit 3", "")
            .unwrap_err()
            .to_string();
        assert!(err.contains("boom"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn generate_rejects_empty_output() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(generate_commit_message(tmp.path(), "true", "x").is_err());
    }

    #[test]
    fn clean_output_strips_code_fence() {
        assert_eq!(
            clean_output("```text\nFix bug\n\nDetails\n```\n"),
            "Fix bug\n\nDetails"
        );
        assert_eq!(clean_output("  Fix bug  \n"), "Fix bug");
    }

    #[test]
    fn split_message_into_summary_and_description() {
        assert_eq!(
            split_commit_message("Fix bug\n\nLonger text\nmore"),
            ("Fix bug".to_string(), "Longer text\nmore".to_string())
        );
        assert_eq!(
            split_commit_message("Only summary\n"),
            ("Only summary".to_string(), String::new())
        );
    }

    #[test]
    fn long_first_line_is_cut_at_a_sentence_end() {
        let (s, d) = split_commit_message(
            "Отвязка CDEK-заказов при отмене. Резервная почта через mail() получает то же тело, что SMTP.\n\nДетали.",
        );
        assert_eq!(s, "Отвязка CDEK-заказов при отмене.");
        assert_eq!(
            d,
            "Резервная почта через mail() получает то же тело, что SMTP.\n\nДетали."
        );
    }

    #[test]
    fn long_first_line_without_sentences_is_cut_at_a_word() {
        let line = "word ".repeat(30);
        let (s, d) = split_commit_message(&line);
        assert!(s.chars().count() <= SUMMARY_MAX_CHARS, "{s}");
        assert!(!s.ends_with(' '));
        assert_eq!(format!("{s} {d}"), line.trim());
    }

    #[test]
    fn long_single_word_is_hard_cut() {
        let line = "x".repeat(100);
        let (s, d) = split_commit_message(&line);
        assert_eq!(s.len(), SUMMARY_MAX_CHARS);
        assert_eq!(d.len(), 100 - SUMMARY_MAX_CHARS);
    }
}

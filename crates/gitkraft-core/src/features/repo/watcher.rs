//! Background repository watcher.
//!
//! Watches the repository with the OS's native file-system notification API
//! (inotify on Linux, FSEvents on macOS, ReadDirectoryChangesW on Windows) and
//! calls a callback after each debounced burst of changes:
//!
//! - in `.git`: its top-level files (HEAD, index, MERGE_HEAD…) and `refs/`;
//! - in the working tree: every path git does not ignore, so a file edited,
//!   created or deleted outside the app shows up in the lists.
//!
//! Reactive events fire ~300 ms after a burst of changes ends.
//! A configurable fallback poll fires when no events arrive within the timeout
//! (default 60 s) — useful for network file systems or CI environments where
//! inotify events may not be delivered.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::event::ModifyKind;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// A burst of changes ends once nothing changed for this long: git writes
/// several files per operation, and an editor's save is a burst too.
const QUIET: Duration = Duration::from_millis(300);

/// The callback fires at most this long after a burst began, so a file that
/// is written non-stop still shows up.
const MAX_DELAY: Duration = Duration::from_secs(2);

/// Working-tree directories watched at most where every directory takes a
/// watch of its own (inotify, whose limit is per user and shared with every
/// other program). Changes beyond show up with the fallback poll.
const MAX_WATCHED_DIRS: usize = 10_000;

/// Spawn a background thread that watches the repository whose `.git` is
/// `git_dir` for changes and calls `on_change` after each debounced burst.
///
/// Uses a 60-second fallback poll so that the UI eventually reflects external
/// changes even on network file systems. Use `spawn_git_watcher_with_fallback`
/// when a custom fallback interval is needed (e.g. in tests).
pub fn spawn_git_watcher<F>(git_dir: PathBuf, on_change: F) -> JoinHandle<()>
where
    F: Fn() -> bool + Send + 'static,
{
    spawn_git_watcher_with_fallback(git_dir, Duration::from_secs(60), on_change)
}

/// Like [`spawn_git_watcher`] but with a custom fallback poll `timeout`.
///
/// Useful in tests to keep waiting times short.
pub(crate) fn spawn_git_watcher_with_fallback<F>(
    git_dir: PathBuf,
    fallback: Duration,
    on_change: F,
) -> JoinHandle<()>
where
    F: Fn() -> bool + Send + 'static,
{
    thread::spawn(move || {
        let (raw_tx, raw_rx) = mpsc::channel::<notify::Result<notify::Event>>();
        // Without a watcher (out of inotify instances) the channel would be
        // disconnected, and every wait below would end at once.
        let _keep_open = raw_tx.clone();

        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = raw_tx.send(res);
            },
            Config::default(),
        )
        .ok();

        let mut scope = Scope {
            git_dirs: spellings(&git_dir),
            worktree: WorkTree::open(&git_dir),
        };

        if let Some(ref mut w) = watcher {
            // Watch top-level .git files NON-recursively: HEAD, index,
            // packed-refs, MERGE_HEAD, COMMIT_EDITMSG, etc.
            // Intentionally excludes objects/, pack/, and logs/ which git2
            // writes to when READING commits — watching those directories
            // causes a read→write→notify→refresh→read loop.
            let _ = w.watch(&git_dir, RecursiveMode::NonRecursive);

            // Watch refs/ recursively to catch branch/tag/stash changes
            // (.git/refs/heads/, .git/refs/tags/, .git/refs/stash, …).
            let refs_dir = git_dir.join("refs");
            if refs_dir.exists() {
                let _ = w.watch(&refs_dir, RecursiveMode::Recursive);
            }

            if let Some(ref mut tree) = scope.worktree {
                tree.watch(w);
            }
        }

        loop {
            // Block until a relevant notify event arrives or fallback elapses.
            let deadline = Instant::now() + fallback;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                let Ok(res) = raw_rx.recv_timeout(left) else {
                    break;
                };
                scope.watch_new_dirs(&res, watcher.as_mut());
                if scope.is_relevant(&res) {
                    break;
                }
            }

            // Debounce: wait for the burst to end (see QUIET, MAX_DELAY).
            let last_call = Instant::now() + MAX_DELAY;
            let mut quiet_until = Instant::now() + QUIET;
            while let Some(left) = quiet_until
                .min(last_call)
                .checked_duration_since(Instant::now())
            {
                let Ok(res) = raw_rx.recv_timeout(left) else {
                    break;
                };
                scope.watch_new_dirs(&res, watcher.as_mut());
                if scope.is_relevant(&res) {
                    quiet_until = Instant::now() + QUIET;
                }
            }

            // Call the callback; stop the thread if it returns false.
            if !on_change() {
                break;
            }
        }
    })
}

/// What the watcher looks at, and which of its events mean the repository
/// changed.
struct Scope {
    /// `.git`, in every spelling (see [`spellings`]).
    git_dirs: Vec<PathBuf>,
    /// `None` for a bare repository, or one git2 cannot open.
    worktree: Option<WorkTree>,
}

impl Scope {
    /// Whether a notify event means the repository changed.
    ///
    /// Ignored: reads (Linux inotify reports Access/Open/Close-nowrite when
    /// git2 merely READS files, which would cause a read → event → refresh →
    /// read loop), and events whose paths all fall under [`Self::matters`]'s
    /// exceptions.
    fn is_relevant(&self, res: &notify::Result<notify::Event>) -> bool {
        match res {
            Ok(ev) => {
                !matches!(ev.kind, EventKind::Access(_)) && ev.paths.iter().any(|p| self.matters(p))
            }
            Err(_) => true,
        }
    }

    /// Whether a change to `path` is a change to the repository.
    fn matters(&self, path: &Path) -> bool {
        if let Some(rel) = relative(path, &self.git_dirs) {
            // Not `.git` itself: FSEvents on macOS delivers its creation late,
            // after watching started; a real change also reports the file
            // that changed. Not a `*.lock` file, nor objects/ or logs/ (see
            // the watches above), which a whole-tree watch reports too.
            return !rel.as_os_str().is_empty()
                && path.extension().is_none_or(|e| e != "lock")
                && (rel.components().count() == 1 || rel.starts_with("refs"));
        }
        match &self.worktree {
            Some(tree) => match relative(path, &tree.roots) {
                Some(rel) => !rel.as_os_str().is_empty() && !tree.skips(rel),
                None => true,
            },
            None => true,
        }
    }

    /// Watch the directories `res` created in the working tree, where every
    /// directory takes a watch of its own. Their contents so far come with
    /// the refresh this very event causes.
    fn watch_new_dirs(
        &mut self,
        res: &notify::Result<notify::Event>,
        watcher: Option<&mut RecommendedWatcher>,
    ) {
        let (Some(tree), Some(w), Ok(ev)) = (&mut self.worktree, watcher, res) else {
            return;
        };
        if !tree.per_dir
            || !matches!(
                ev.kind,
                EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_))
            )
        {
            return;
        }
        for path in &ev.paths {
            if path.is_dir() && relative(path, &tree.roots).is_some_and(|rel| !tree.skips(rel)) {
                tree.watch_dirs(w, path);
            }
        }
    }
}

/// The working tree.
struct WorkTree {
    /// For git's ignore rules.
    repo: git2::Repository,
    /// The working tree's root, in every spelling (see [`spellings`]).
    roots: Vec<PathBuf>,
    /// Every directory takes a watch of its own (inotify); otherwise one
    /// watch covers the whole tree.
    per_dir: bool,
    /// Directories watched so far (see [`MAX_WATCHED_DIRS`]).
    dirs: usize,
}

impl WorkTree {
    fn open(git_dir: &Path) -> Option<Self> {
        let repo = git2::Repository::open(git_dir).ok()?;
        let root = repo.workdir()?.to_path_buf();
        Some(Self {
            roots: spellings(&root),
            repo,
            per_dir: !cfg!(any(target_os = "macos", target_os = "windows")),
            dirs: 0,
        })
    }

    fn watch(&mut self, w: &mut RecommendedWatcher) {
        let root = self.roots[0].clone();
        if self.per_dir {
            self.watch_dirs(w, &root);
        } else {
            // FSEvents and ReadDirectoryChangesW watch a whole tree with one
            // handle; `Scope::matters` drops what git ignores.
            let _ = w.watch(&root, RecursiveMode::Recursive);
        }
    }

    /// Watch `top` and the directories under it, except what git ignores:
    /// build output, `target/` or `node_modules/` can hold more directories
    /// than inotify's per-user limit allows.
    fn watch_dirs(&mut self, w: &mut RecommendedWatcher, top: &Path) {
        let mut todo = vec![top.to_path_buf()];
        while let Some(dir) = todo.pop() {
            if self.dirs >= MAX_WATCHED_DIRS {
                tracing::debug!(
                    "watching {MAX_WATCHED_DIRS} directories already, not {}",
                    dir.display()
                );
                return;
            }
            match w.watch(&dir, RecursiveMode::NonRecursive) {
                Ok(()) => self.dirs += 1,
                Err(e) if matches!(e.kind, notify::ErrorKind::MaxFilesWatch) => {
                    tracing::debug!("out of inotify watches at {}", dir.display());
                    return;
                }
                // Gone meanwhile, or unreadable.
                Err(_) => continue,
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                // `file_type` does not follow symlinks, and neither does git.
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    let path = entry.path();
                    if relative(&path, &self.roots).is_some_and(|rel| !self.skips(rel)) {
                        todo.push(path);
                    }
                }
            }
        }
    }

    /// Whether changes at `rel` (relative to the root) leave git's view of
    /// the repository as it was: anything inside a `.git` (this one is
    /// watched on its own, a submodule's is not ours), and what git ignores.
    fn skips(&self, rel: &Path) -> bool {
        rel.components().any(|c| c.as_os_str() == ".git")
            || self.repo.is_path_ignored(rel).unwrap_or(false)
    }
}

/// `path` as given and, when different, canonicalized: macOS reports
/// `/private/var/…` for a watched `/var/…` path.
fn spellings(path: &Path) -> Vec<PathBuf> {
    let mut all = vec![path.to_path_buf()];
    if let Ok(canonical) = std::fs::canonicalize(path)
        && canonical != path
    {
        all.push(canonical);
    }
    all
}

/// `path` relative to the first of `roots` it lies under.
fn relative<'a>(path: &'a Path, roots: &[PathBuf]) -> Option<&'a Path> {
    roots.iter().find_map(|r| path.strip_prefix(r).ok())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn event(kind: notify::EventKind, path: &str) -> notify::Result<notify::Event> {
        Ok(notify::Event::new(kind).add_path(PathBuf::from(path)))
    }

    /// A [`Scope`] for `/r/.git`, without its working tree.
    fn git_only() -> Scope {
        Scope {
            git_dirs: vec![PathBuf::from("/r/.git")],
            worktree: None,
        }
    }

    #[test]
    fn an_event_on_the_watched_directory_itself_is_not_a_change() {
        // FSEvents delivers the creation of the watched .git late, after the
        // watcher started; that refreshed the UI for nothing (and failed CI).
        let scope = git_only();
        let created = notify::EventKind::Create(notify::event::CreateKind::Folder);
        assert!(!scope.is_relevant(&event(created, "/r/.git")));
        assert!(scope.is_relevant(&event(created, "/r/.git/refs")));
    }

    #[test]
    fn reads_and_lock_files_are_not_changes() {
        let scope = git_only();
        let read = notify::EventKind::Access(notify::event::AccessKind::Any);
        let write = notify::EventKind::Modify(notify::event::ModifyKind::Any);
        assert!(!scope.is_relevant(&event(read, "/r/.git/HEAD")));
        assert!(!scope.is_relevant(&event(write, "/r/.git/index.lock")));
        assert!(scope.is_relevant(&event(write, "/r/.git/HEAD")));
    }

    #[test]
    fn objects_and_logs_are_not_changes() {
        // A whole-tree watch (macOS, Windows) reports them too; git2 writes
        // there while reading.
        let scope = git_only();
        let write = notify::EventKind::Modify(notify::event::ModifyKind::Any);
        assert!(!scope.is_relevant(&event(write, "/r/.git/objects/ab/cdef")));
        assert!(!scope.is_relevant(&event(write, "/r/.git/logs/HEAD")));
        assert!(scope.is_relevant(&event(write, "/r/.git/refs/heads/main")));
        assert!(scope.is_relevant(&event(write, "/r/.git/MERGE_HEAD")));
    }

    #[test]
    fn working_tree_changes_count_unless_git_ignores_them() {
        let dir = tempfile::tempdir().unwrap();
        git2::Repository::init(dir.path()).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "build/\n*.log\n").unwrap();
        std::fs::create_dir_all(dir.path().join("build")).unwrap();
        std::fs::create_dir_all(dir.path().join("sub/.git")).unwrap();
        let scope = Scope {
            git_dirs: spellings(&dir.path().join(".git")),
            worktree: WorkTree::open(&dir.path().join(".git")),
        };
        let at = |rel: &str| dir.path().join(rel).to_string_lossy().into_owned();
        let removed = notify::EventKind::Remove(notify::event::RemoveKind::File);

        assert!(scope.is_relevant(&event(removed, &at("partitions.csv_"))));
        assert!(scope.is_relevant(&event(removed, &at("src/main.rs"))));
        assert!(scope.is_relevant(&event(removed, &at(".gitignore"))));
        assert!(!scope.is_relevant(&event(removed, &at("build/out.o"))));
        assert!(!scope.is_relevant(&event(removed, &at("debug.log"))));
        assert!(!scope.is_relevant(&event(removed, &at("sub/.git/index"))));
        assert!(!scope.is_relevant(&event(removed, dir.path().to_str().unwrap())));
    }

    fn wait_for<F: Fn() -> bool>(condition: F, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn watcher_calls_callback_when_git_head_changes() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        // Create a minimal HEAD file so the directory looks like a git repo.
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        let fired = Arc::new(Mutex::new(false));
        let fired_clone = Arc::clone(&fired);

        let _handle = spawn_git_watcher(git_dir.clone(), move || {
            *fired_clone.lock().unwrap() = true;
            false // stop after the first callback
        });

        // Give the watcher a moment to set up before triggering a change.
        thread::sleep(Duration::from_millis(200));

        // Simulate a branch checkout by rewriting HEAD.
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/feature\n").unwrap();

        // The callback fires after the 300 ms debounce — wait up to 2 s.
        assert!(
            wait_for(|| *fired.lock().unwrap(), Duration::from_secs(2)),
            "watcher did not call on_change within 2 seconds after HEAD changed"
        );
    }

    #[test]
    fn watcher_fires_fallback_poll_when_no_events() {
        // Use a path that doesn't exist — notify will fail to watch it, so no
        // events will ever arrive.  Uses a 2-second fallback (not the production
        // 60-second default) to keep the test fast.
        let dir = tempfile::tempdir().unwrap();
        let fake_git = dir.path().join(".git_nonexistent");

        let fired = Arc::new(Mutex::new(false));
        let fired_clone = Arc::clone(&fired);

        let _handle =
            spawn_git_watcher_with_fallback(fake_git, Duration::from_secs(2), move || {
                *fired_clone.lock().unwrap() = true;
                false
            });

        // 2 s fallback + 300 ms debounce + margin.
        assert!(
            wait_for(|| *fired.lock().unwrap(), Duration::from_secs(4)),
            "fallback poll did not fire within 4 seconds"
        );
    }

    #[test]
    fn watcher_thread_exits_when_callback_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();

        // 2-second fallback so the thread exits quickly in CI.
        let handle =
            spawn_git_watcher_with_fallback(git_dir.clone(), Duration::from_secs(2), move || {
                false // immediately request exit on first call
            });

        // Thread must finish within 4 s (2 s fallback + 300 ms debounce + margin).
        assert!(
            wait_for(|| handle.is_finished(), Duration::from_secs(4)),
            "watcher thread did not exit after callback returned false"
        );
    }

    #[test]
    fn watcher_ignores_read_only_access() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        let fired = Arc::new(Mutex::new(false));
        let fired_clone = Arc::clone(&fired);
        let _handle =
            spawn_git_watcher_with_fallback(git_dir.clone(), Duration::from_secs(5), move || {
                *fired_clone.lock().unwrap() = true;
                true
            });
        // macOS FSEvents may replay the setup write shortly after start; let it
        // settle, then clear the flag so only the read phase is observed.
        thread::sleep(Duration::from_millis(1200));
        *fired.lock().unwrap() = false;

        // Pure reads (inotify Access/Open events) must not trigger a refresh.
        for _ in 0..5 {
            let _ = std::fs::read_to_string(git_dir.join("HEAD")).unwrap();
            thread::sleep(Duration::from_millis(50));
        }

        assert!(
            !wait_for(|| *fired.lock().unwrap(), Duration::from_secs(2)),
            "read-only access wrongly triggered on_change"
        );
    }

    #[test]
    fn watcher_ignores_lock_file_only_changes() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();

        let fired = Arc::new(Mutex::new(false));
        let fired_clone = Arc::clone(&fired);
        let _handle =
            spawn_git_watcher_with_fallback(git_dir.clone(), Duration::from_secs(5), move || {
                *fired_clone.lock().unwrap() = true;
                false
            });
        thread::sleep(Duration::from_millis(200));

        std::fs::write(git_dir.join("index.lock"), "x").unwrap();
        std::fs::remove_file(git_dir.join("index.lock")).unwrap();

        assert!(
            !wait_for(|| *fired.lock().unwrap(), Duration::from_secs(2)),
            "lock-file change wrongly triggered on_change"
        );
    }

    /// Watch the repository at `dir` (fallback 30 s, out of the way); the
    /// returned flag is set on every callback.
    fn watch_repo(dir: &Path) -> Arc<Mutex<bool>> {
        let fired = Arc::new(Mutex::new(false));
        let fired_clone = Arc::clone(&fired);
        let _handle =
            spawn_git_watcher_with_fallback(dir.join(".git"), Duration::from_secs(30), move || {
                *fired_clone.lock().unwrap() = true;
                true
            });
        // Let it set up (and macOS FSEvents replay the setup writes), then
        // observe only what the test does.
        thread::sleep(Duration::from_millis(1200));
        *fired.lock().unwrap() = false;
        fired
    }

    #[test]
    fn watcher_sees_a_file_deleted_from_the_working_tree() {
        // Regression: only .git was watched, so an untracked file deleted
        // outside the app stayed listed (and failed to load) until the
        // fallback poll. Its directory also holds a stray empty `.git`.
        let dir = tempfile::tempdir().unwrap();
        git2::Repository::init(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join("Lua-RTOS-ESP32/.git")).unwrap();
        let file = dir.path().join("Lua-RTOS-ESP32/partitions.csv_");
        std::fs::write(&file, "nvs, data, nvs, 0x9000, 0x6000\n").unwrap();

        let fired = watch_repo(dir.path());
        std::fs::remove_file(&file).unwrap();

        assert!(
            wait_for(|| *fired.lock().unwrap(), Duration::from_secs(3)),
            "deleting a working-tree file did not call on_change"
        );
    }

    #[test]
    fn watcher_ignores_what_git_ignores() {
        let dir = tempfile::tempdir().unwrap();
        git2::Repository::init(dir.path()).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "build/\n*.log\n").unwrap();
        std::fs::create_dir_all(dir.path().join("build/obj")).unwrap();

        let fired = watch_repo(dir.path());
        std::fs::write(dir.path().join("build/obj/main.o"), "x").unwrap();
        std::fs::write(dir.path().join("debug.log"), "x").unwrap();

        assert!(
            !wait_for(|| *fired.lock().unwrap(), Duration::from_secs(2)),
            "a change git ignores wrongly triggered on_change"
        );
    }

    #[test]
    fn watcher_watches_directories_created_after_it_started() {
        let dir = tempfile::tempdir().unwrap();
        git2::Repository::init(dir.path()).unwrap();

        let fired = watch_repo(dir.path());
        std::fs::create_dir_all(dir.path().join("src/new")).unwrap();
        assert!(
            wait_for(|| *fired.lock().unwrap(), Duration::from_secs(3)),
            "creating a directory did not call on_change"
        );

        // After that refresh, a file appears inside the new directory.
        thread::sleep(Duration::from_millis(500));
        *fired.lock().unwrap() = false;
        std::fs::write(dir.path().join("src/new/a.txt"), "x").unwrap();
        assert!(
            wait_for(|| *fired.lock().unwrap(), Duration::from_secs(3)),
            "a file created in a new directory did not call on_change"
        );
    }
}

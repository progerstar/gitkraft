//! Update logic for staging-related messages.

use std::collections::HashSet;

use gitkraft_core::{DiffFileEntry, IgnoreTarget};
use iced::Task;

use crate::message::{Message, StagingPayload};
use crate::state::{GitKraft, RepoTab};

use super::commands;

/// Shared logic for shift-click / regular-click file selection in the staging area.
/// Updates `selection` and `anchor` based on click type.
/// Returns `true` if the clicked file is now selected (caller should load its diff).
fn toggle_staging_file(
    changes: &[DiffFileEntry],
    selection: &mut HashSet<String>,
    anchor: &mut Option<usize>,
    path: &str,
    shift_held: bool,
) -> bool {
    let clicked_idx = changes.iter().position(|d| d.display_path() == path);

    if shift_held {
        if let Some(idx) = clicked_idx {
            let range = crate::view_utils::shift_click_range(*anchor, idx);
            selection.clear();
            for i in &range {
                if let Some(d) = changes.get(*i) {
                    selection.insert(d.display_path().to_string());
                }
            }
        }
    } else {
        if selection.contains(path) {
            selection.remove(path);
        } else {
            selection.insert(path.to_string());
        }
        if let Some(idx) = clicked_idx {
            *anchor = Some(idx);
        }
    }

    // Return true if the file is now selected (so the caller can load its diff)
    selection.contains(path)
}

/// Put refreshed staged / unstaged lists into `tab`.
fn apply_staging(tab: &mut RepoTab, payload: StagingPayload) {
    tab.unstaged_changes = payload.unstaged;
    tab.staged_changes = payload.staged;
    tab.prune_changes_selection();
}

/// Status line after adding `target` to `.gitignore`.
fn gitignore_status(target: &IgnoreTarget, added: usize) -> String {
    let what = target.label();
    if added == 0 {
        format!("{what} is already in .gitignore.")
    } else {
        format!("Added {what} to .gitignore.")
    }
}

/// Changed files `target` covers that are still listed: git keeps tracking
/// files it already tracks whatever `.gitignore` says (untracked ones have
/// dropped out of the refreshed lists). Sorted, without duplicates.
fn tracked_matches(tab: &RepoTab, target: &IgnoreTarget) -> Vec<String> {
    let paths: std::collections::BTreeSet<&str> = tab
        .unstaged_changes
        .iter()
        .chain(&tab.staged_changes)
        .filter(|f| f.status != gitkraft_core::FileStatus::Untracked)
        .map(|f| f.display_path())
        .filter(|p| target.matches(p))
        .collect();
    paths.into_iter().map(str::to_string).collect()
}

/// Handle all staging-related messages, returning a [`Task`] for any follow-up
/// async work.
pub(crate) fn update(state: &mut GitKraft, message: Message) -> Task<Message> {
    match message {
        Message::StageFile(path) => {
            with_repo!(state, dismiss, format!("Staging '{path}'…"), |repo_path| {
                commands::stage_file(repo_path, path)
            })
        }

        Message::UnstageFile(path) => {
            with_repo!(
                state,
                dismiss,
                format!("Unstaging '{path}'…"),
                |repo_path| commands::unstage_file(repo_path, path)
            )
        }

        Message::StageAll => {
            with_repo!(state, dismiss, "Staging all files…".into(), |repo_path| {
                commands::stage_all(repo_path)
            })
        }

        Message::UnstageAll => {
            with_repo!(
                state,
                dismiss,
                "Unstaging all files…".into(),
                |repo_path| commands::unstage_all(repo_path)
            )
        }

        Message::DiscardFile(path) => {
            state.active_tab_mut().pending_discard = None;
            with_repo!(
                state,
                dismiss,
                format!("Discarding '{path}'…"),
                |repo_path| commands::discard_file(repo_path, path)
            )
        }

        Message::AskDiscard(path) => {
            state.active_tab_mut().pending_discard = Some(path);
            Task::none()
        }

        Message::ConfirmDiscard(path) => {
            with_repo!(
                state,
                format!("Discarding changes in '{path}'…"),
                |repo_path| {
                    state.active_tab_mut().pending_discard = None;
                    commands::discard_file(repo_path, path)
                }
            )
        }

        Message::CancelDiscard => {
            let tab = state.active_tab_mut();
            tab.pending_discard = None;
            tab.status_message = None;
            Task::none()
        }

        Message::StagingUpdated(result) => {
            let tab = state.active_tab_mut();
            match result {
                Ok(payload) => {
                    apply_staging(tab, payload);
                    tab.status_message = Some("Staging area updated.".into());
                }
                Err(e) => {
                    tab.error_message = Some(format!("Staging operation failed: {e}"));
                    tab.status_message = None;
                }
            }
            Task::none()
        }

        Message::ToggleSelectUnstaged(path) => {
            let shift_held = state.keyboard_modifiers.shift();
            let tab = state.active_tab_mut();
            let is_selected = toggle_staging_file(
                &tab.unstaged_changes,
                &mut tab.selected_unstaged,
                &mut tab.anchor_unstaged_index,
                &path,
                shift_held,
            );
            if is_selected {
                // Load the diff on demand
                if let Some(repo_path) = tab.repo_path.clone() {
                    let file_path = path.to_string();
                    return commands::load_staging_file_diff(repo_path, file_path, false);
                }
            }
            Task::none()
        }

        Message::ToggleSelectStaged(path) => {
            let shift_held = state.keyboard_modifiers.shift();
            let tab = state.active_tab_mut();
            let is_selected = toggle_staging_file(
                &tab.staged_changes,
                &mut tab.selected_staged,
                &mut tab.anchor_staged_index,
                &path,
                shift_held,
            );
            if is_selected {
                // Load the diff on demand
                if let Some(repo_path) = tab.repo_path.clone() {
                    let file_path = path.to_string();
                    return commands::load_staging_file_diff(repo_path, file_path, true);
                }
            }
            Task::none()
        }

        Message::StageSelected => {
            let paths: Vec<String> = state
                .active_tab()
                .selected_unstaged
                .iter()
                .cloned()
                .collect();
            if paths.is_empty() {
                return Task::none();
            }
            state.active_tab_mut().selected_unstaged.clear();
            with_repo!(
                state,
                dismiss,
                format!("Staging {} file(s)…", paths.len()),
                |repo_path| commands::stage_files(repo_path, paths)
            )
        }

        Message::UnstageSelected => {
            let paths: Vec<String> = state.active_tab().selected_staged.iter().cloned().collect();
            if paths.is_empty() {
                return Task::none();
            }
            state.active_tab_mut().selected_staged.clear();
            with_repo!(
                state,
                dismiss,
                format!("Unstaging {} file(s)…", paths.len()),
                |repo_path| commands::unstage_files(repo_path, paths)
            )
        }

        Message::DiscardSelected => {
            let unstaged_paths: Vec<String> = state
                .active_tab()
                .selected_unstaged
                .iter()
                .cloned()
                .collect();
            let staged_paths: Vec<String> =
                state.active_tab().selected_staged.iter().cloned().collect();

            if unstaged_paths.is_empty() && staged_paths.is_empty() {
                return Task::none();
            }

            let total = unstaged_paths.len() + staged_paths.len();
            state.active_tab_mut().selected_unstaged.clear();
            state.active_tab_mut().selected_staged.clear();
            with_repo!(
                state,
                dismiss,
                format!("Discarding {} file(s)…", total),
                |repo_path| commands::discard_all_selected(repo_path, unstaged_paths, staged_paths)
            )
        }

        Message::AddToGitignore(target) => with_repo!(
            state,
            dismiss,
            format!("Adding {} to .gitignore…", target.label()),
            |repo_path| commands::add_to_gitignore(repo_path, target)
        ),

        Message::GitignoreUpdated(target, result) => {
            let tab = state.active_tab_mut();
            match result {
                Ok((added, payload)) => {
                    apply_staging(tab, payload);
                    tab.status_message = Some(gitignore_status(&target, added));
                    tab.pending_untrack = tracked_matches(tab, &target);
                }
                Err(e) => {
                    tab.error_message = Some(format!("Could not update .gitignore: {e}"));
                    tab.status_message = None;
                }
            }
            Task::none()
        }

        Message::ConfirmUntrack => {
            let paths = std::mem::take(&mut state.active_tab_mut().pending_untrack);
            if paths.is_empty() {
                return Task::none();
            }
            with_repo!(
                state,
                format!("Untracking {} file(s)…", paths.len()),
                |repo_path| commands::untrack_files(repo_path, paths)
            )
        }

        Message::CancelUntrack => {
            state.active_tab_mut().pending_untrack.clear();
            Task::none()
        }

        Message::FilesUntracked(count, result) => {
            let tab = state.active_tab_mut();
            match result {
                Ok(payload) => {
                    apply_staging(tab, payload);
                    tab.status_message = Some(format!(
                        "Stopped tracking {count} file(s): they stay on disk; \
                         commit to remove them from the repository."
                    ));
                }
                Err(e) => {
                    tab.error_message = Some(format!("Could not stop tracking: {e}"));
                    tab.status_message = None;
                }
            }
            Task::none()
        }

        Message::DiscardStagedFile(path) => {
            with_repo!(
                state,
                dismiss,
                format!("Discarding '{path}'…"),
                |repo_path| commands::discard_staged_file(repo_path, path)
            )
        }

        _ => Task::none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ContextMenu;
    use gitkraft_core::FileStatus;

    fn entry(path: &str, status: FileStatus) -> DiffFileEntry {
        DiffFileEntry {
            old_file: path.into(),
            new_file: path.into(),
            status,
        }
    }

    fn files(names: &[&str]) -> IgnoreTarget {
        IgnoreTarget::Files(names.iter().map(|s| s.to_string()).collect())
    }

    fn updated(
        state: &mut GitKraft,
        target: IgnoreTarget,
        added: usize,
        unstaged: Vec<DiffFileEntry>,
    ) {
        let payload = StagingPayload {
            unstaged,
            staged: Vec::new(),
        };
        let _ = state.update(Message::GitignoreUpdated(target, Ok((added, payload))));
    }

    #[test]
    fn adding_to_gitignore_closes_the_menu() {
        let mut state = GitKraft::new();
        state.active_tab_mut().context_menu = Some(ContextMenu::UnstagedFile {
            path: "a.rs".into(),
        });
        let _ = state.update(Message::AddToGitignore(files(&["a.rs"])));
        assert!(state.active_tab().context_menu.is_none());
    }

    #[test]
    fn folder_item_opens_the_folder_menu() {
        let mut state = GitKraft::new();
        let _ = state.update(Message::OpenIgnoreFolderMenu("src/a.rs".into()));
        assert!(matches!(
            &state.active_tab().context_menu,
            Some(ContextMenu::IgnoreFolder { path }) if path == "src/a.rs"
        ));
    }

    #[test]
    fn status_after_ignoring_untracked_files() {
        let mut state = GitKraft::new();
        state.active_tab_mut().unstaged_changes = vec![entry("new.txt", FileStatus::Untracked)];
        updated(&mut state, files(&["new.txt"]), 1, Vec::new());
        let tab = state.active_tab();
        assert!(tab.unstaged_changes.is_empty());
        assert_eq!(
            tab.status_message.as_deref(),
            Some("Added 'new.txt' to .gitignore.")
        );
    }

    #[test]
    fn tracked_files_left_in_the_list_are_offered_for_untracking() {
        let mut state = GitKraft::new();
        let still_listed = vec![entry("a.rs", FileStatus::Modified)];
        updated(&mut state, files(&["a.rs", "new.txt"]), 2, still_listed);
        let tab = state.active_tab();
        assert_eq!(
            tab.status_message.as_deref(),
            Some("Added 2 files to .gitignore.")
        );
        assert_eq!(tab.pending_untrack, ["a.rs"]);
    }

    #[test]
    fn untracking_offer_covers_what_the_folder_rule_matches() {
        let mut state = GitKraft::new();
        let payload = StagingPayload {
            unstaged: vec![
                entry("src/ui/a.rs", FileStatus::Modified),
                entry("src/b.rs", FileStatus::Modified),
                entry("src/ui/new.rs", FileStatus::Untracked),
            ],
            staged: vec![
                entry("src/ui/c.rs", FileStatus::New),
                entry("src/ui/a.rs", FileStatus::Modified),
            ],
        };
        let target = IgnoreTarget::Folder("src/ui".into());
        let _ = state.update(Message::GitignoreUpdated(target, Ok((1, payload))));
        assert_eq!(
            state.active_tab().pending_untrack,
            ["src/ui/a.rs", "src/ui/c.rs"]
        );
    }

    #[test]
    fn no_untracking_offer_when_nothing_tracked_matches() {
        let mut state = GitKraft::new();
        let unrelated = vec![entry("b.rs", FileStatus::Modified)];
        updated(&mut state, files(&["a.rs"]), 1, unrelated);
        assert!(state.active_tab().pending_untrack.is_empty());
    }

    #[test]
    fn keeping_files_tracked_closes_the_offer() {
        let mut state = GitKraft::new();
        state.active_tab_mut().pending_untrack = vec!["a.rs".into()];
        let _ = state.update(Message::CancelUntrack);
        assert!(state.active_tab().pending_untrack.is_empty());

        state.active_tab_mut().pending_untrack = vec!["a.rs".into()];
        let _ = state.update(Message::CloseFileBlame); // Escape
        assert!(state.active_tab().pending_untrack.is_empty());
    }

    #[test]
    fn untracked_files_refresh_the_lists() {
        let mut state = GitKraft::new();
        state.active_tab_mut().unstaged_changes = vec![entry("a.csv", FileStatus::Modified)];
        let payload = StagingPayload {
            unstaged: Vec::new(),
            staged: vec![entry("a.csv", FileStatus::Deleted)],
        };
        let _ = state.update(Message::FilesUntracked(1, Ok(payload)));
        let tab = state.active_tab();
        assert!(tab.unstaged_changes.is_empty());
        assert_eq!(tab.staged_changes.len(), 1);
        assert!(
            tab.status_message
                .as_deref()
                .is_some_and(|s| s.starts_with("Stopped tracking 1 file(s)"))
        );
    }

    #[test]
    fn status_when_nothing_was_added() {
        let mut state = GitKraft::new();
        updated(
            &mut state,
            IgnoreTarget::Extension("log".into()),
            0,
            Vec::new(),
        );
        assert_eq!(
            state.active_tab().status_message.as_deref(),
            Some("'*.log' is already in .gitignore.")
        );
    }

    #[test]
    fn failure_is_shown_as_an_error() {
        let mut state = GitKraft::new();
        let _ = state.update(Message::GitignoreUpdated(
            files(&["a.rs"]),
            Err("permission denied".into()),
        ));
        let tab = state.active_tab();
        assert_eq!(
            tab.error_message.as_deref(),
            Some("Could not update .gitignore: permission denied")
        );
        assert!(tab.status_message.is_none());
    }
}

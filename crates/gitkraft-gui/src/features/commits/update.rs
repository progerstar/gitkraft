//! Update logic for commit-related messages.

use iced::Task;

use crate::message::Message;
use crate::state::GitKraft;

use super::commands;

/// Handle all commit-related messages, returning a [`Task`] for any follow-up
/// async work.
pub(crate) fn update(state: &mut GitKraft, message: Message) -> Task<Message> {
    match message {
        Message::SelectCommit(index) => {
            let shift_held = state.keyboard_modifiers.shift();

            if shift_held {
                // Shift+Click: range selection from anchor to clicked index.
                let anchor = state
                    .active_tab()
                    .anchor_commit_index
                    .or(state.active_tab().selected_commit);

                let range = crate::view_utils::shift_click_range(anchor, index);

                // Determine the oldest and newest commits in the selection.
                // Commits are stored newest-first, so the highest index is the oldest.
                let Some(&oldest_idx) = range.last() else {
                    return Task::none();
                };
                let newest_idx = range[0];
                let oldest_oid = state
                    .active_tab()
                    .commits
                    .get(oldest_idx)
                    .map(|c| c.oid.clone());
                let newest_oid = state
                    .active_tab()
                    .commits
                    .get(newest_idx)
                    .map(|c| c.oid.clone());
                let repo_path = state.active_tab().repo_path.clone();

                let tab = state.active_tab_mut();
                tab.selected_commits = range;
                tab.selected_commit = Some(index);
                tab.commit_range_diffs.clear();
                tab.is_loading_file_diff = true;

                if let (Some(oldest), Some(newest), Some(path)) =
                    (oldest_oid, newest_oid, repo_path)
                {
                    return commands::load_commit_range_diff(path, oldest, newest);
                }
                return Task::none();
            }

            // ── Regular click: single commit, set anchor ──────────────────────
            let repo_path = state.active_tab().repo_path.clone();
            let commit_info = state
                .active_tab()
                .commits
                .get(index)
                .map(|c| (c.oid.clone(), c.short_oid().to_string()));

            let tab = state.active_tab_mut();
            tab.anchor_commit_index = Some(index);
            tab.selected_commits.clear();
            tab.selected_commit = Some(index);
            tab.show_commit_detail = true;
            // Clear previous diff state immediately for snappy feedback.
            tab.commit_files.clear();
            tab.selected_diff = None;
            tab.selected_file_index = None;
            tab.diff_scroll_offset = 0.0;
            tab.selected_commit_file_indices.clear();
            tab.multi_file_diffs.clear();
            tab.commit_range_diffs.clear();
            // Exit the blame overlay — clicking a different commit means the
            // user wants to see that commit's diff, not the previous blame.
            tab.blame_path = None;
            tab.blame_lines.clear();
            tab.blame_scroll = 0.0;

            // Close any open file preview.
            clear_preview!(state);

            // Load just the file list (instant — no line parsing).
            if let (Some(path), Some((oid, short_oid))) = (repo_path, commit_info) {
                let tab = state.active_tab_mut();
                tab.status_message = Some(format!("Loading files for {short_oid}…"));
                tab.selected_commit_oid = Some(oid.clone());
                commands::load_commit_file_list(path, oid)
            } else {
                Task::none()
            }
        }

        Message::CommitFileListLoaded(result) => {
            match result {
                Ok(files) => {
                    let file_count = files.len();
                    let tab = state.active_tab_mut();
                    tab.commit_files = files;
                    tab.status_message = Some(format!("{file_count} file(s) changed."));

                    // Auto-select the first file and load its diff.
                    if file_count > 0 {
                        let first_file = &tab.commit_files[0];
                        let file_path = first_file.display_path().to_string();
                        tab.selected_file_index = Some(0);
                        tab.is_loading_file_diff = true;

                        if let (Some(repo_path), Some(oid)) =
                            (tab.repo_path.clone(), tab.selected_commit_oid.clone())
                        {
                            return commands::load_single_file_diff(repo_path, oid, file_path);
                        }
                    }
                }
                Err(e) => {
                    let tab = state.active_tab_mut();
                    tab.commit_files.clear();
                    tab.error_message = Some(format!("Failed to load commit files: {e}"));
                    tab.status_message = None;
                }
            }
            Task::none()
        }

        Message::SingleFileDiffLoaded(result) => {
            let tab = state.active_tab_mut();
            tab.is_loading_file_diff = false;
            match result {
                Ok(diff) => {
                    tab.selected_diff = Some(diff);
                    tab.diff_scroll_offset = 0.0;
                    // Clear multi-file state now that the single-file diff is
                    // ready.  Doing this here (instead of on click) avoids a
                    // widget-tree restructure that causes the file list to blink.
                    tab.multi_file_diffs.clear();
                    tab.commit_range_diffs.clear();
                    // Close any open file preview.
                    clear_preview!(state);
                }
                Err(e) => {
                    tab.selected_diff = None;
                    tab.error_message = Some(format!("Failed to load file diff: {e}"));
                }
            }
            Task::none()
        }

        Message::CommitMessageChanged(msg) => set_field!(state, commit_message, msg),

        Message::CreateCommit => {
            let msg;
            let staged_empty;
            {
                let tab = state.active_tab();
                msg = full_commit_message(&tab.commit_message, &tab.commit_description.text());
                staged_empty = tab.staged_changes.is_empty();
            }
            if msg.is_empty() || staged_empty {
                return Task::none();
            }
            with_repo!(state, loading, "Creating commit…".into(), |repo_path| {
                commands::create_commit(repo_path, msg)
            })
        }

        Message::CommitCreated(result) => {
            state.active_tab_mut().is_loading = false;
            match result {
                Ok(()) => {
                    {
                        let tab = state.active_tab_mut();
                        tab.commit_message.clear();
                        tab.commit_description = iced::widget::text_editor::Content::new();
                        tab.status_message = Some("Commit created.".into());
                    }
                    if state.push_after_commit {
                        return push_after_commit(state);
                    }
                    state.refresh_active_tab()
                }
                Err(e) => {
                    let tab = state.active_tab_mut();
                    tab.error_message = Some(format!("Commit failed: {e}"));
                    tab.status_message = None;
                    Task::none()
                }
            }
        }

        Message::DiffFileWithWorkingTree(oid, file_path) => {
            let label = file_path
                .rsplit('/')
                .next()
                .unwrap_or(&file_path)
                .to_string();
            with_repo!(
                state,
                dismiss,
                format!("Comparing {label} with working tree…"),
                |repo_path| commands::diff_file_with_working_tree(repo_path, oid, file_path)
            )
        }

        Message::DiffWithWorkingTreeLoaded(result) => {
            match result {
                Ok(diff) => {
                    let tab = state.active_tab_mut();
                    tab.selected_diff = Some(diff);
                    tab.diff_scroll_offset = 0.0;
                    tab.status_message = Some("Showing diff against working tree".into());
                    // Close any open file preview.
                    clear_preview!(state);
                }
                Err(e) => {
                    state.active_tab_mut().status_message = Some(format!("⚠ {e}"));
                }
            }
            Task::none()
        }

        Message::DiffMultiWithWorkingTree(oid, file_paths) => {
            state.active_tab_mut().context_menu = None;
            if let Some(path) = state.active_tab().repo_path.clone() {
                let count = file_paths.len();
                state.active_tab_mut().is_loading_file_diff = true;
                state.active_tab_mut().diff_scroll_offset = 0.0;
                state.active_tab_mut().status_message =
                    Some(format!("Comparing {count} files with working tree…"));
                commands::load_multi_file_commit_vs_workdir(path, oid.clone(), file_paths.clone())
            } else {
                Task::none()
            }
        }

        Message::CheckoutFileAtCommit(oid, file_path) => {
            let label = file_path
                .rsplit('/')
                .next()
                .unwrap_or(&file_path)
                .to_string();
            with_repo!(
                state,
                dismiss,
                format!("Restoring '{label}'…"),
                |repo_path| {
                    commands::checkout_file_at_commit(repo_path, oid.clone(), file_path.clone())
                }
            )
        }

        Message::CheckoutMultiFilesAtCommit(oid, file_paths) => {
            let count = file_paths.len();
            with_repo!(
                state,
                dismiss,
                format!("Restoring {count} files…"),
                |repo_path| {
                    commands::checkout_multi_files_at_commit(
                        repo_path,
                        oid.clone(),
                        file_paths.clone(),
                    )
                }
            )
        }

        Message::CherryPickCommits(oids) => {
            let count = oids.len();
            with_repo!(
                state,
                dismiss,
                format!("Cherry-picking {count} commit(s)…"),
                |repo_path| commands::cherry_pick_commits(repo_path, oids)
            )
        }

        Message::RevertCommits(oids) => {
            let count = oids.len();
            with_repo!(
                state,
                dismiss,
                format!("Reverting {count} commit(s)…"),
                |repo_path| commands::revert_commits(repo_path, oids)
            )
        }

        Message::CommitRangeDiffLoaded(result) => {
            let tab = state.active_tab_mut();
            tab.is_loading_file_diff = false;
            match result {
                Ok(diffs) => {
                    tab.commit_range_diffs = diffs;
                    tab.diff_scroll_offset = 0.0;
                    let count = tab.selected_commits.len();
                    tab.status_message = Some(format!("Showing combined diff for {count} commits"));
                }
                Err(e) => {
                    tab.commit_range_diffs.clear();
                    tab.error_message = Some(format!("Range diff failed: {e}"));
                }
            }
            Task::none()
        }

        Message::CommitDescriptionAction(action) => {
            state.active_tab_mut().commit_description.perform(action);
            Task::none()
        }

        Message::GenerateCommitMessage => {
            let tab = state.active_tab();
            if tab.generating_commit_message || tab.staged_changes.is_empty() {
                return Task::none();
            }
            with_repo!(state, "Generating commit message…".into(), |repo_path| {
                let tab = state.active_tab_mut();
                tab.generating_commit_message = true;
                // Clear the old message right away; keep it in case of failure.
                let old_description = std::mem::take(&mut tab.commit_description).text();
                let old_summary = std::mem::take(&mut tab.commit_message);
                tab.commit_message_backup = Some((old_summary, old_description));
                commands::generate_commit_message(repo_path)
            })
        }

        Message::CommitMessageGenerated(path, result) => {
            // The agent can take a while: write into the tab that asked,
            // even if the user switched tabs meanwhile.
            let Some(tab) = state
                .tabs
                .iter_mut()
                .find(|t| t.repo_path.as_deref() == Some(path.as_path()))
            else {
                return Task::none();
            };
            tab.generating_commit_message = false;
            let backup = tab.commit_message_backup.take();
            match result {
                Ok(message) => {
                    let (summary, description) = gitkraft_core::split_commit_message(&message);
                    tab.commit_message = summary;
                    tab.commit_description =
                        iced::widget::text_editor::Content::with_text(&description);
                    tab.status_message = Some("Commit message generated.".into());
                    // Show the start of the summary, not wherever the cursor was.
                    iced::widget::operation::move_cursor_to_front(iced::widget::Id::new(
                        "commit_message_input",
                    ))
                }
                Err(e) => {
                    // Put back what the user had, unless they typed something new.
                    if let Some((summary, description)) = backup
                        && tab.commit_message.is_empty()
                        && tab.commit_description.text().trim().is_empty()
                    {
                        tab.commit_message = summary;
                        tab.commit_description =
                            iced::widget::text_editor::Content::with_text(&description);
                    }
                    tab.error_message = Some(format!("Commit message generation failed: {e}"));
                    tab.status_message = None;
                    Task::none()
                }
            }
        }

        Message::TogglePushAfterCommit(enabled) => {
            state.push_after_commit = enabled;
            commands::save_push_after_commit(enabled)
        }

        _ => Task::none(),
    }
}

/// Summary plus optional description, separated by a blank line.
fn full_commit_message(summary: &str, description: &str) -> String {
    let (summary, description) = (summary.trim(), description.trim());
    if summary.is_empty() || description.is_empty() {
        summary.to_string()
    } else {
        format!("{summary}\n\n{description}")
    }
}

/// Push the current branch after a commit (the repo reloads when it
/// finishes). Skipped without a remote or on a detached HEAD.
fn push_after_commit(state: &mut GitKraft) -> Task<Message> {
    let tab = state.active_tab();
    let skip_reason = if tab.remotes.is_empty() {
        Some("no remote configured")
    } else if tab.current_branch.is_none() {
        Some("detached HEAD")
    } else {
        None
    };
    if let Some(reason) = skip_reason {
        state.active_tab_mut().status_message =
            Some(format!("Commit created. Push skipped: {reason}."));
        return state.refresh_active_tab();
    }
    let remote = tab.default_remote();
    let branch = tab.current_branch.clone().unwrap_or_default();
    with_repo!(
        state,
        loading,
        format!("Commit created. Pushing '{branch}' to {remote}…"),
        |path| crate::features::repo::commands::push_branch_async(path, branch, remote)
    )
}

#[cfg(test)]
mod tests {
    use super::full_commit_message;

    #[test]
    fn full_message_joins_summary_and_description() {
        assert_eq!(full_commit_message(" Fix bug ", ""), "Fix bug");
        assert_eq!(
            full_commit_message("Fix bug", "\nWhy it broke\n"),
            "Fix bug\n\nWhy it broke"
        );
        // A description alone is not a commit message.
        assert_eq!(full_commit_message("  ", "body"), "");
    }
}

//! Messages of the GitHub Desktop–style layout.

use iced::Task;

use super::changes::{self, ClickKind, Inclusion};
use crate::macros::StringErr;
use crate::message::Message;
use crate::state::{GhdTab, GitKraft, LayoutStyle};

pub(crate) fn update(state: &mut GitKraft, message: Message) -> Task<Message> {
    match message {
        Message::ToggleLayoutStyle => {
            state.layout_style = match state.layout_style {
                LayoutStyle::Classic => LayoutStyle::GitHubDesktop,
                LayoutStyle::GitHubDesktop => LayoutStyle::Classic,
            };
            state.branch_popup_open = false;
            let style = state.layout_style.as_setting();
            git_task!(
                Message::LayoutStyleSaved,
                gitkraft_core::features::persistence::save_layout_style(style).str_err()
            )
        }

        Message::SetGhdTab(which) => {
            if state.ghd_tab != which {
                state.ghd_tab = which;
                let tab = state.active_tab_mut();
                match which {
                    // Changes shows working-tree diffs only: drop any commit selection.
                    GhdTab::Changes => {
                        tab.selected_commit = None;
                        tab.selected_commit_oid = None;
                        tab.selected_commits.clear();
                        tab.anchor_commit_index = None;
                        tab.commit_files.clear();
                        tab.selected_file_index = None;
                        tab.selected_commit_file_indices.clear();
                        tab.multi_file_diffs.clear();
                        tab.commit_range_diffs.clear();
                        tab.is_loading_file_diff = false;
                        tab.selected_diff = None;
                        // Back to the selected rows: show the diff of the
                        // last one clicked again.
                        let selection = &tab.changes_selection;
                        if let Some(path) =
                            selection.cursor.clone().filter(|c| selection.contains(c))
                        {
                            tab.diff_scroll_offset = 0.0;
                            return show_change(state, path);
                        }
                    }
                    // History starts empty until a commit is picked.
                    GhdTab::History => {
                        if tab.selected_commit_oid.is_none() {
                            tab.selected_diff = None;
                        }
                    }
                }
                tab.diff_scroll_offset = 0.0;
            }
            Task::none()
        }

        Message::ToggleBranchPopup => {
            state.branch_popup_open = !state.branch_popup_open;
            state.branch_popup_pos = (state.cursor_pos.x, state.cursor_pos.y);
            Task::none()
        }

        Message::CloseBranchPopup => {
            state.branch_popup_open = false;
            Task::none()
        }

        Message::SelectChange(path) => {
            let kind = ClickKind::from_modifiers(state.keyboard_modifiers);
            let tab = state.active_tab_mut();
            let rows = changes::changed_paths(tab);
            tab.changes_selection.click(&rows, &path, kind);
            if tab.changes_selection.contains(&path) {
                show_change(state, path)
            } else {
                Task::none()
            }
        }

        Message::ToggleSelectedChangesStaged => toggle_selected_staged(state),

        _ => Task::none(),
    }
}

/// Whether the Changes list is on screen (GitHub Desktop layout, Changes tab).
pub(crate) fn changes_list_shown(state: &GitKraft) -> bool {
    state.layout_style == LayoutStyle::GitHubDesktop && state.ghd_tab == GhdTab::Changes
}

/// Whether list keys should act on the Changes list: it is shown and no menu,
/// dropdown or confirmation sits over it.
fn changes_keys_active(state: &GitKraft) -> bool {
    let tab = state.active_tab();
    changes_list_shown(state)
        && tab.repo_path.is_some()
        && tab.context_menu.is_none()
        && tab.pending_discard.is_none()
        && !state.branch_popup_open
}

/// Shift+Up / Shift+Down in the Changes list: grow or shrink the range from
/// the anchor and show the diff of the row reached.
pub(crate) fn extend_changes_selection(state: &mut GitKraft, delta: isize) -> Task<Message> {
    if !changes_keys_active(state) {
        return Task::none();
    }
    let tab = state.active_tab_mut();
    let rows = changes::changed_paths(tab);
    match tab.changes_selection.extend(&rows, delta) {
        Some(path) => show_change(state, path),
        None => Task::none(),
    }
}

/// Space in the Changes list: stage every selected row, or unstage them all
/// when each is already fully staged (like the row checkboxes).
fn toggle_selected_staged(state: &mut GitKraft) -> Task<Message> {
    if !changes_keys_active(state) {
        return Task::none();
    }
    let tab = state.active_tab();
    let files = changes::changed_files(tab);
    let selected: Vec<_> = files
        .iter()
        .filter(|f| tab.changes_selection.contains(f.path()))
        .collect();
    if selected.is_empty() {
        return Task::none();
    }
    // Fully staged rows only matter when all of them are: then Space unstages.
    let unstage = selected.iter().all(|f| f.inclusion == Inclusion::All);
    let paths: Vec<String> = selected
        .iter()
        .filter(|f| unstage || f.inclusion != Inclusion::All)
        .map(|f| f.path().to_string())
        .collect();
    let what = if unstage { "Unstaging" } else { "Staging" };
    with_repo!(
        state,
        dismiss,
        format!("{what} {} file(s)…", paths.len()),
        |repo_path| if unstage {
            crate::features::staging::commands::unstage_files(repo_path, paths)
        } else {
            crate::features::staging::commands::stage_files(repo_path, paths)
        }
    )
}

/// Load the diff of a Changes row into the diff viewer.
fn show_change(state: &mut GitKraft, path: String) -> Task<Message> {
    let staged = changes::changed_files(state.active_tab())
        .iter()
        .find(|f| f.path() == path)
        .is_some_and(|f| f.shows_staged());
    crate::features::diff::update::update(state, Message::LoadStagingFileDiff(path, staged))
}

#[cfg(test)]
mod tests {
    use gitkraft_core::{DiffFileEntry, FileStatus};
    use iced::keyboard::Modifiers;

    use super::*;

    fn entry(path: &str) -> DiffFileEntry {
        DiffFileEntry {
            old_file: path.into(),
            new_file: path.into(),
            status: FileStatus::Modified,
        }
    }

    /// Rows a, b, c, d: `a` and `c` unstaged, `b` fully staged, `d` partly.
    fn state_with_changes() -> GitKraft {
        let mut state = GitKraft::new();
        let tab = state.active_tab_mut();
        tab.repo_path = Some("/tmp/fake-repo-for-test".into());
        tab.unstaged_changes = vec![entry("a"), entry("c"), entry("d")];
        tab.staged_changes = vec![entry("b"), entry("d")];
        state
    }

    fn click(state: &mut GitKraft, path: &str, modifiers: Modifiers) {
        state.keyboard_modifiers = modifiers;
        let _ = state.update(Message::SelectChange(path.into()));
    }

    fn selected(state: &GitKraft) -> Vec<String> {
        let mut v: Vec<String> = state
            .active_tab()
            .changes_selection
            .paths
            .iter()
            .cloned()
            .collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn click_then_shift_click_selects_the_range() {
        let mut state = state_with_changes();
        click(&mut state, "a", Modifiers::default());
        click(&mut state, "c", Modifiers::SHIFT);
        assert_eq!(selected(&state), ["a", "b", "c"]);
        click(&mut state, "b", Modifiers::COMMAND);
        assert_eq!(selected(&state), ["a", "c"]);
    }

    #[test]
    fn space_stages_the_selection_unless_all_of_it_is_staged() {
        let mut state = state_with_changes();
        click(&mut state, "a", Modifiers::default());
        click(&mut state, "d", Modifiers::SHIFT);
        let _ = state.update(Message::ToggleSelectedChangesStaged);
        // `b` is staged already: only a, c and d are left to stage.
        assert_eq!(
            state.active_tab().status_message.as_deref(),
            Some("Staging 3 file(s)…")
        );

        click(&mut state, "b", Modifiers::default());
        let _ = state.update(Message::ToggleSelectedChangesStaged);
        assert_eq!(
            state.active_tab().status_message.as_deref(),
            Some("Unstaging 1 file(s)…")
        );
    }

    #[test]
    fn space_does_nothing_outside_the_changes_list() {
        let mut state = state_with_changes();
        click(&mut state, "a", Modifiers::default());
        state.active_tab_mut().status_message = None;

        state.ghd_tab = GhdTab::History;
        let _ = state.update(Message::ToggleSelectedChangesStaged);
        assert_eq!(state.active_tab().status_message, None);

        state.ghd_tab = GhdTab::Changes;
        state.active_tab_mut().pending_discard = Some("a".into());
        let _ = state.update(Message::ToggleSelectedChangesStaged);
        assert_eq!(state.active_tab().status_message, None);
    }

    #[test]
    fn shift_arrows_extend_the_changes_selection_not_the_commit_log() {
        let mut state = state_with_changes();
        state.active_tab_mut().commits = (0..3)
            .map(|i| gitkraft_core::CommitInfo {
                oid: i.to_string(),
                summary: String::new(),
                message: String::new(),
                author_name: String::new(),
                author_email: String::new(),
                time: Default::default(),
                parent_ids: Vec::new(),
                refs: Vec::new(),
            })
            .collect();
        click(&mut state, "b", Modifiers::default());
        state.keyboard_modifiers = Modifiers::SHIFT;
        let _ = state.update(Message::ShiftArrowDown);
        let _ = state.update(Message::ShiftArrowDown);
        assert_eq!(selected(&state), ["b", "c", "d"]);
        let _ = state.update(Message::ShiftArrowUp);
        assert_eq!(selected(&state), ["b", "c"]);
        assert_eq!(state.active_tab().selected_commit, None);
    }

    #[test]
    fn staging_refresh_forgets_rows_that_are_gone() {
        let mut state = state_with_changes();
        click(&mut state, "a", Modifiers::default());
        click(&mut state, "c", Modifiers::SHIFT);
        let _ = state.update(Message::StagingUpdated(Ok(
            crate::message::StagingPayload {
                unstaged: vec![entry("c")],
                staged: vec![],
            },
        )));
        assert_eq!(selected(&state), ["c"]);
    }
}

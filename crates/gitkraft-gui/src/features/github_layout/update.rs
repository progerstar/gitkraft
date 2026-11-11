//! Messages of the GitHub Desktop–style layout.

use iced::Task;

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

        _ => Task::none(),
    }
}

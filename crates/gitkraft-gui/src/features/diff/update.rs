//! Update logic for diff-related messages.

use iced::Task;

use gitkraft_core::ImageKind;

use super::image_view;
use crate::message::Message;
use crate::state::GitKraft;

/// Handle diff-related messages, returning a [`Task`] for any follow-up
/// async work.
pub(crate) fn update(state: &mut GitKraft, message: Message) -> Task<Message> {
    match message {
        Message::SelectDiffByIndex(index) => {
            let shift_held = state.keyboard_modifiers.shift();
            let tab = state.active_tab();
            let repo_path = tab.repo_path.clone();
            let oid = tab.selected_commit_oid.clone();

            if shift_held {
                // Shift+Click: range selection from anchor to clicked index.
                let anchor = state
                    .active_tab()
                    .anchor_file_index
                    .or(state.active_tab().selected_file_index);

                let range = crate::view_utils::shift_click_range(anchor, index);
                let count = range.len();

                let tab = state.active_tab_mut();
                tab.selected_commit_file_indices = range;
                tab.selected_file_index = Some(index);

                if count == 1 {
                    // Range collapsed to a single file — behave like a normal
                    // single-file selection (no multi-diff panel).
                    tab.multi_file_diffs.clear();
                    let file_entry = tab.commit_files.get(index).cloned();
                    if let (Some(entry), Some(path), Some(oid)) = (file_entry, repo_path, oid) {
                        let file_path = entry.display_path().to_string();
                        let tab = state.active_tab_mut();
                        tab.is_loading_file_diff = true;
                        tab.diff_scroll_offset = 0.0;
                        return crate::features::commits::commands::load_single_file_diff(
                            path, oid, file_path,
                        );
                    }
                } else {
                    // Multiple files in range — load and display them all.
                    tab.selected_diff = None;
                    tab.is_loading_file_diff = true;
                    tab.diff_scroll_offset = 0.0;
                    let file_paths: Vec<String> = tab
                        .selected_commit_file_indices
                        .iter()
                        .filter_map(|&i| {
                            tab.commit_files
                                .get(i)
                                .map(|f| f.display_path().to_string())
                        })
                        .collect();
                    if let (Some(path), Some(oid)) = (repo_path, oid) {
                        return crate::features::commits::commands::load_commit_multi_diffs(
                            path, oid, file_paths,
                        );
                    }
                }
                Task::none()
            } else {
                // ── Regular click: single-file selection, set range anchor ─────
                let tab = state.active_tab_mut();
                tab.anchor_file_index = Some(index); // fix the anchor for future Shift+Clicks
                tab.selected_commit_file_indices.clear();
                // NOTE: multi_file_diffs, commit_range_diffs, and
                // diff_scroll_offset are intentionally NOT cleared here.
                // Clearing them immediately would change the widget-tree
                // structure before the replacement diff is ready, causing the
                // file list panel to visually flash/blink.  Instead we defer
                // those resets to SingleFileDiffLoaded where the new diff
                // content is set atomically.

                let file_entry = tab.commit_files.get(index).cloned();
                if let (Some(entry), Some(path), Some(oid)) = (file_entry, repo_path, oid) {
                    let file_path = entry.display_path().to_string();
                    let tab = state.active_tab_mut();
                    tab.selected_file_index = Some(index);
                    tab.is_loading_file_diff = true;
                    crate::features::commits::commands::load_single_file_diff(path, oid, file_path)
                } else {
                    Task::none()
                }
            }
        }

        Message::LoadStagingFileDiff(path, staged) => {
            state.active_tab_mut().context_menu = None;
            if let Some(repo_path) = state.active_tab().repo_path.clone() {
                crate::features::staging::commands::load_staging_file_diff(repo_path, path, staged)
            } else {
                Task::none()
            }
        }

        Message::StagingFileDiffLoaded(result) => {
            let tab = state.active_tab_mut();
            match result {
                Ok(diff) => {
                    tab.selected_diff = Some(diff);
                    tab.diff_scroll_offset = 0.0;
                }
                Err(e) => {
                    tab.error_message = Some(format!("Failed to load staging diff: {e}"));
                }
            }
            // Close any open file preview.
            clear_preview!(state);
            Task::none()
        }

        Message::CommitMultiDiffLoaded(result) => {
            let tab = state.active_tab_mut();
            tab.is_loading_file_diff = false;
            match result {
                Ok(diffs) => {
                    tab.multi_file_diffs = diffs;
                    tab.selected_diff = None;
                    tab.diff_scroll_offset = 0.0;
                }
                Err(e) => {
                    tab.multi_file_diffs.clear();
                    tab.error_message = Some(format!("Failed to load multi-file diff: {e}"));
                }
            }
            Task::none()
        }

        Message::ImagePreviewLoaded(preview) => {
            if let Some(tab) = state
                .tabs
                .iter_mut()
                .find(|t| t.image_preview_pending.as_deref() == Some(preview.key.as_str()))
            {
                tab.image_preview_pending = None;
                tab.image_preview = Some(preview);
            }
            Task::none()
        }

        Message::ToggleSvgText => {
            state.svg_as_text = !state.svg_as_text;
            Task::none()
        }

        _ => Task::none(),
    }
}

impl GitKraft {
    /// If the active tab shows an image diff whose images are not loaded
    /// (or loading) yet, start loading them.
    pub(crate) fn ensure_image_preview(&mut self) -> Task<Message> {
        let svg_as_text = self.svg_as_text;
        let tab = self.active_tab();
        let (Some(diff), Some(path)) = (tab.selected_diff.as_ref(), tab.repo_path.as_ref()) else {
            return Task::none();
        };
        let Some(key) = image_view::image_key(diff) else {
            return Task::none();
        };
        if svg_as_text && gitkraft_core::image_kind(diff.display_path()) == Some(ImageKind::Svg) {
            return Task::none();
        }
        let loaded = tab.image_preview.as_ref().is_some_and(|p| p.key == key);
        if loaded || tab.image_preview_pending.as_deref() == Some(key.as_str()) {
            return Task::none();
        }
        let (diff, path) = (diff.clone(), path.clone());
        self.active_tab_mut().image_preview_pending = Some(key.clone());
        image_view::load_image_preview(path, key, diff)
    }
}

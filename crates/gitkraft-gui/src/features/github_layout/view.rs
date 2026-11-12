//! GitHub Desktop–style layout: a left column with "Changes" / "History"
//! tabs (changed files with checkboxes and the commit box, or the commit log)
//! next to the diff, and branches / stashes / remotes in a dropdown.

use iced::widget::{Space, button, column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Element, Font, Length};

use super::changes::{Inclusion, changed_files};
use crate::icons;
use crate::message::Message;
use crate::state::{DragTarget, GhdTab, GitKraft};
use crate::theme;
use crate::theme::ThemeColors;
use crate::view_utils;

/// Height of the commit box at the bottom of the Changes tab.
const COMMIT_PANEL_HEIGHT: f32 = 270.0;
/// Size of the branch dropdown.
const POPUP_WIDTH: f32 = 380.0;
const POPUP_MAX_HEIGHT: f32 = 560.0;

/// Left column | divider | diff viewer.
pub(crate) fn body<'a>(
    state: &'a GitKraft,
    left_width: f32,
    diff_viewer: Element<'a, Message>,
) -> Element<'a, Message> {
    let c = state.colors();
    let content: Element<'a, Message> = match state.ghd_tab {
        GhdTab::Changes => column![
            container(changes_list(state, &c)).height(Length::Fill),
            container(crate::features::staging::view::commit_view(state))
                .width(Length::Fill)
                .height(Length::Fixed(COMMIT_PANEL_HEIGHT)),
        ]
        .height(Length::Fill)
        .into(),
        GhdTab::History => crate::features::commits::view::view(state),
    };

    let left = container(
        column![tabs(state, &c), content]
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fixed(left_width))
    .height(Length::Fill)
    // Same colour as the list rows, so the column reads as one list.
    .style(theme::surface_style);

    let divider = crate::widgets::divider::vertical_divider(DragTarget::CommitLogRight, &c);

    row![left, divider, diff_viewer]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// "Changes (n) | History" tab strip.
fn tabs<'a>(state: &'a GitKraft, c: &ThemeColors) -> Element<'a, Message> {
    let count = changed_files(state.active_tab()).len();
    let tab = |label: String, which: GhdTab| -> Element<'a, Message> {
        let active = state.ghd_tab == which;
        let label = text(label)
            .size(13)
            .color(if active { c.text_primary } else { c.muted });
        let underline = container(Space::new().height(2))
            .width(Length::Fill)
            .style(if active {
                theme::accent_bar_style
            } else {
                theme::surface_style
            });
        button(
            column![
                container(label).padding([8, 0]).center_x(Length::Fill),
                underline
            ]
            .width(Length::Fill),
        )
        .padding(0)
        .width(Length::FillPortion(1))
        .style(theme::ghost_button)
        .on_press(Message::SetGhdTab(which))
        .into()
    };
    let changes_label = if count > 0 {
        format!("Changes  {count}")
    } else {
        "Changes".to_string()
    };
    row![
        tab(changes_label, GhdTab::Changes),
        tab("History".to_string(), GhdTab::History)
    ]
    .width(Length::Fill)
    .into()
}

/// Checkbox glyph for an inclusion state.
fn checkbox_icon<'a>(inclusion: Inclusion, c: &ThemeColors) -> Element<'a, Message> {
    let (glyph, color) = match inclusion {
        Inclusion::All => (icons::CHECK_SQUARE_FILL, c.accent),
        Inclusion::Partial => (icons::DASH_SQUARE_FILL, c.accent),
        Inclusion::None => (icons::SQUARE, c.muted),
    };
    icon!(glyph, 14, color).into()
}

/// Header with a select-all checkbox, then one row per changed file.
fn changes_list<'a>(state: &'a GitKraft, c: &ThemeColors) -> Element<'a, Message> {
    let tab = state.active_tab();
    let files = changed_files(tab);

    let all = if files.is_empty() || files.iter().all(|f| f.inclusion == Inclusion::None) {
        Inclusion::None
    } else if files.iter().all(|f| f.inclusion == Inclusion::All) {
        Inclusion::All
    } else {
        Inclusion::Partial
    };
    let toggle_all = (!files.is_empty()).then(|| {
        if tab.unstaged_changes.is_empty() {
            Message::UnstageAll
        } else {
            Message::StageAll
        }
    });
    let header = row![
        view_utils::on_press_maybe(
            button(checkbox_icon(all, c))
                .padding([2, 4])
                .style(theme::icon_button),
            toggle_all,
        ),
        Space::new().width(6),
        text(match files.len() {
            1 => "1 changed file".to_string(),
            n => format!("{n} changed files"),
        })
        .size(12)
        .color(c.text_secondary),
        Space::new().width(Length::Fill),
        small_text_button(
            "Stage All",
            (!tab.unstaged_changes.is_empty()).then_some(Message::StageAll),
        ),
        Space::new().width(4),
        small_text_button(
            "Unstage All",
            (!tab.staged_changes.is_empty()).then_some(Message::UnstageAll),
        ),
    ]
    .align_y(Alignment::Center)
    .padding([6, 8]);

    // Highlight the selected rows; with none, the file whose diff is shown
    // (opened from its context menu).
    let selection = &tab.changes_selection;
    let shown_path = (tab.selected_commit_oid.is_none() && selection.paths.is_empty())
        .then(|| tab.selected_diff.as_ref().map(|d| d.display_path()))
        .flatten();

    let mut list = column![].spacing(1).width(Length::Fill);
    if files.is_empty() {
        list = list.push(view_utils::empty_list_hint("No local changes", c.muted));
    }
    for file in files {
        let path = file.entry.display_path();
        let toggle = match file.inclusion {
            Inclusion::All => Message::UnstageFile(path.to_string()),
            Inclusion::Partial | Inclusion::None => Message::StageFile(path.to_string()),
        };
        let checkbox = button(checkbox_icon(file.inclusion, c))
            .padding([2, 4])
            .style(theme::icon_button)
            .on_press(toggle);

        let dir = file.entry.parent_dir();
        let name_row = row![
            text(if dir.is_empty() {
                String::new()
            } else {
                format!("{dir}/")
            })
            .size(12)
            .color(c.muted)
            .wrapping(iced::widget::text::Wrapping::None),
            text(file.entry.file_name().to_string())
                .size(12)
                .color(c.text_primary)
                .wrapping(iced::widget::text::Wrapping::None),
        ];
        let status = text(format!("{}", file.entry.status))
            .size(11)
            .font(Font::MONOSPACE)
            .color(theme::status_color(&file.entry.status, c));

        // Per-file actions, as in the classic staging lists: stage what is
        // not staged yet, unstage what is, discard working-tree changes.
        let mut actions = row![].spacing(2).align_y(Alignment::Center);
        if file.inclusion != Inclusion::All {
            actions = actions.push(icon_action(
                icons::PLUS_CIRCLE,
                c.green,
                Message::StageFile(path.to_string()),
            ));
        }
        if file.inclusion != Inclusion::None {
            actions = actions.push(icon_action(
                icons::DASH_CIRCLE,
                c.yellow,
                Message::UnstageFile(path.to_string()),
            ));
        }
        if file.inclusion != Inclusion::All {
            actions = actions.push(icon_action(
                icons::TRASH,
                c.red,
                view_utils::discard_or_delete(file.entry),
            ));
        }

        let line = row![
            checkbox,
            Space::new().width(4),
            container(name_row).width(Length::Fill).clip(true),
            Space::new().width(6),
            status,
            Space::new().width(6),
            // Fixed width so the status letters line up whatever the actions.
            container(actions)
                .width(Length::Fixed(66.0))
                .align_x(iced::alignment::Horizontal::Right),
        ]
        .align_y(Alignment::Center)
        .padding([2, 8]);

        let context_menu = if file.shows_staged() {
            Message::OpenStagedFileContextMenu(path.to_string())
        } else {
            Message::OpenUnstagedFileContextMenu(path.to_string())
        };
        let selected = selection.contains(path) || shown_path == Some(path);
        let style = row_style!(selected => theme::selected_row_style);
        list = list.push(
            mouse_area(container(line).width(Length::Fill).style(style))
                .on_press(Message::SelectChange(path.to_string()))
                .on_right_press(context_menu),
        );
    }

    column![
        header,
        scrollable(list)
            .height(Length::Fill)
            .direction(view_utils::thin_scrollbar())
            .style(theme::overlay_scrollbar),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Small icon button for a per-file action.
fn icon_action<'a>(glyph: char, color: iced::Color, msg: Message) -> Element<'a, Message> {
    button(icon!(glyph, 11, color))
        .padding([2, 4])
        .style(theme::icon_button)
        .on_press(msg)
        .into()
}

/// Compact text button, disabled when `msg` is `None`.
fn small_text_button<'a>(label: &'a str, msg: Option<Message>) -> Element<'a, Message> {
    view_utils::on_press_maybe(
        button(text(label).size(11))
            .padding([2, 8])
            .style(theme::toolbar_button),
        msg,
    )
    .into()
}

/// The "Current branch" dropdown: branches, stashes and remotes, under the
/// point where it was opened, with a click-outside-to-close backdrop.
pub(crate) fn branch_popup<'a>(
    state: &'a GitKraft,
    base: Element<'a, Message>,
) -> Element<'a, Message> {
    let c = state.colors();
    let (x, y) = state.branch_popup_pos;
    let (win_w, win_h) = (state.window_width, state.window_height);
    let left = x.min(win_w - POPUP_WIDTH - 8.0).max(8.0);
    let top = (y + 16.0).max(8.0);
    let height = POPUP_MAX_HEIGHT.min(win_h - top - 40.0).max(200.0);

    let sections = column![
        container(crate::features::branches::view::view(state))
            .width(Length::Fill)
            .height(Length::FillPortion(3)),
        divider_line(&c),
        container(crate::features::stash::view::view(state))
            .width(Length::Fill)
            .height(Length::FillPortion(2)),
        divider_line(&c),
        container(crate::features::remotes::view::view(state))
            .width(Length::Fill)
            .height(Length::FillPortion(1)),
    ]
    .width(Length::Fill)
    .height(Length::Fill);

    let panel = container(sections)
        .width(Length::Fixed(POPUP_WIDTH))
        .height(Length::Fixed(height))
        .style(theme::context_menu_style);

    let positioned = column![
        Space::new().height(top),
        row![Space::new().width(left), panel],
    ]
    .width(Length::Fill)
    .height(Length::Fill);

    let backdrop = mouse_area(
        container(Space::new().width(Length::Fill).height(Length::Fill))
            .style(theme::backdrop_style),
    )
    .on_press(Message::CloseBranchPopup);

    iced::widget::stack![base, backdrop, positioned].into()
}

fn divider_line<'a>(c: &ThemeColors) -> Element<'a, Message> {
    let color = c.border;
    container(Space::new().height(1))
        .width(Length::Fill)
        .style(move |_| iced::widget::container::Style {
            background: Some(iced::Background::Color(color)),
            ..Default::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitkraft_core::{DiffFileEntry, FileStatus};

    fn entry(path: &str, status: FileStatus) -> DiffFileEntry {
        DiffFileEntry {
            old_file: path.into(),
            new_file: path.into(),
            status,
        }
    }

    #[test]
    fn changed_files_merge_staged_and_unstaged_by_path() {
        let mut state = GitKraft::new();
        let tab = state.active_tab_mut();
        tab.unstaged_changes = vec![
            entry("b.rs", FileStatus::Modified),
            entry("c.rs", FileStatus::Untracked),
        ];
        tab.staged_changes = vec![
            entry("a.rs", FileStatus::New),
            entry("b.rs", FileStatus::Modified),
        ];
        let files = changed_files(state.active_tab());
        let got: Vec<(&str, Inclusion)> = files.iter().map(|f| (f.path(), f.inclusion)).collect();
        assert_eq!(
            got,
            vec![
                ("a.rs", Inclusion::All),
                ("b.rs", Inclusion::Partial),
                ("c.rs", Inclusion::None),
            ]
        );
    }

    #[test]
    fn tab_reports_ahead_and_behind_of_the_checked_out_branch() {
        let mut state = GitKraft::new();
        let tab = state.active_tab_mut();
        assert_eq!(tab.upstream_ahead_behind(), (0, 0));
        tab.current_branch = Some("main".into());
        tab.branches = vec![gitkraft_core::BranchInfo {
            name: "main".into(),
            branch_type: gitkraft_core::BranchType::Local,
            is_head: true,
            target_oid: None,
            upstream_ahead: Some(2),
            upstream_behind: Some(1),
        }];
        assert_eq!(tab.upstream_ahead_behind(), (2, 1));
    }

    #[test]
    fn trash_asks_first_and_deletes_only_untracked_files() {
        let tracked = entry("a.rs", FileStatus::Modified);
        let untracked = entry("new.rs", FileStatus::Untracked);
        assert!(matches!(
            view_utils::discard_or_delete(&tracked),
            Message::AskDiscard(p) if p == "a.rs"
        ));
        assert!(matches!(
            view_utils::discard_or_delete(&untracked),
            Message::DeleteFile(p) if p == "new.rs"
        ));

        let mut state = GitKraft::new();
        let _ = state.update(Message::AskDiscard("a.rs".into()));
        assert_eq!(state.active_tab().pending_discard.as_deref(), Some("a.rs"));
        let _ = state.update(Message::CancelDiscard);
        assert!(state.active_tab().pending_discard.is_none());
    }

    #[test]
    fn switching_to_changes_drops_the_commit_selection() {
        let mut state = GitKraft::new();
        state.ghd_tab = GhdTab::History;
        {
            let tab = state.active_tab_mut();
            tab.selected_commit = Some(0);
            tab.selected_commit_oid = Some("abc".into());
            tab.commit_files = vec![entry("x.rs", FileStatus::Modified)];
        }
        let _ = state.update(Message::SetGhdTab(GhdTab::Changes));
        let tab = state.active_tab();
        assert_eq!(state.ghd_tab, GhdTab::Changes);
        assert!(tab.selected_commit_oid.is_none());
        assert!(tab.commit_files.is_empty());
    }
}

//! The Changes list of the GitHub Desktop layout: one row per changed path
//! (staged and unstaged entries merged), and the selection over those rows.

use std::collections::{BTreeMap, HashSet};

use crate::state::RepoTab;

/// How much of a file's change is staged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Inclusion {
    None,
    Partial,
    All,
}

/// One row of the Changes list.
#[derive(Debug, Clone)]
pub(crate) struct ChangedFile<'a> {
    pub(crate) entry: &'a gitkraft_core::DiffFileEntry,
    pub(crate) inclusion: Inclusion,
}

impl ChangedFile<'_> {
    pub(crate) fn path(&self) -> &str {
        self.entry.display_path()
    }

    /// Show the unstaged diff when there is one (it is what changed in the
    /// working tree); a fully staged file shows its staged diff.
    pub(crate) fn shows_staged(&self) -> bool {
        self.inclusion == Inclusion::All
    }
}

/// Unstaged and staged entries merged by path, sorted by path.
pub(crate) fn changed_files(tab: &RepoTab) -> Vec<ChangedFile<'_>> {
    let staged: HashSet<&str> = tab
        .staged_changes
        .iter()
        .map(|f| f.display_path())
        .collect();
    let unstaged: HashSet<&str> = tab
        .unstaged_changes
        .iter()
        .map(|f| f.display_path())
        .collect();

    let mut by_path: BTreeMap<&str, ChangedFile<'_>> = BTreeMap::new();
    // The working-tree entry describes the whole change best; fall back to
    // the staged one for fully staged files.
    for entry in tab.unstaged_changes.iter().chain(&tab.staged_changes) {
        let path = entry.display_path();
        let inclusion = match (staged.contains(path), unstaged.contains(path)) {
            (true, false) => Inclusion::All,
            (true, true) => Inclusion::Partial,
            _ => Inclusion::None,
        };
        by_path
            .entry(path)
            .or_insert(ChangedFile { entry, inclusion });
    }
    by_path.into_values().collect()
}

/// Paths of the Changes list, in row order.
pub(crate) fn changed_paths(tab: &RepoTab) -> Vec<String> {
    changed_files(tab)
        .iter()
        .map(|f| f.path().to_string())
        .collect()
}

/// What a click on a row does to the selection, from the held modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClickKind {
    /// Select only the clicked row.
    Single,
    /// Shift: select the rows from the anchor to the clicked one.
    Range,
    /// Ctrl / Cmd: add or remove the clicked row.
    Toggle,
}

impl ClickKind {
    pub(crate) fn from_modifiers(modifiers: iced::keyboard::Modifiers) -> Self {
        if modifiers.shift() {
            Self::Range
        } else if modifiers.command() {
            Self::Toggle
        } else {
            Self::Single
        }
    }
}

/// Selected rows of the Changes list, kept by path so that they survive the
/// list being reloaded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangesSelection {
    /// Selected paths.
    pub paths: HashSet<String>,
    /// Where a Shift range starts: the last row clicked without Shift.
    pub anchor: Option<String>,
    /// The last row clicked or reached with Shift+arrows; its diff is shown.
    pub cursor: Option<String>,
}

impl ChangesSelection {
    pub(crate) fn contains(&self, path: &str) -> bool {
        self.paths.contains(path)
    }

    /// Apply a click on `path`; `rows` are the list's paths in order.
    pub(crate) fn click(&mut self, rows: &[String], path: &str, kind: ClickKind) {
        match kind {
            ClickKind::Single => {
                self.paths = HashSet::from([path.to_string()]);
                self.anchor = Some(path.to_string());
            }
            ClickKind::Toggle => {
                if !self.paths.remove(path) {
                    self.paths.insert(path.to_string());
                }
                self.anchor = Some(path.to_string());
            }
            ClickKind::Range => {
                let Some(target) = rows.iter().position(|r| r == path) else {
                    return;
                };
                self.select_range_to(rows, target);
            }
        }
        self.cursor = Some(path.to_string());
    }

    /// Up / Down: move the cursor by `delta` rows and select only that row.
    /// Without a cursor, start at the first (down) or last (up) row. At either
    /// end a multi-row selection still collapses to the cursor row. Returns the
    /// cursor path when the selection changed.
    pub(crate) fn step(&mut self, rows: &[String], delta: isize) -> Option<String> {
        let (current, target) = self.cursor_target(rows, delta)?;
        let path = &rows[target];
        let unchanged = current == Some(target) && self.paths.len() == 1 && self.contains(path);
        self.click(rows, path, ClickKind::Single);
        (!unchanged).then(|| path.clone())
    }

    /// Shift+arrow: move the cursor by `delta` rows and select from the anchor
    /// to it. Without a cursor, start at the first (down) or last (up) row.
    /// Returns the new cursor path when it moved.
    pub(crate) fn extend(&mut self, rows: &[String], delta: isize) -> Option<String> {
        let (current, target) = self.cursor_target(rows, delta)?;
        if current == Some(target) {
            return None;
        }
        if current.is_none() {
            self.anchor = None;
        }
        self.select_range_to(rows, target);
        self.cursor.clone()
    }

    /// The cursor's row and the row `delta` away from it (clamped to the
    /// list); without a cursor, the first (down) or last (up) row.
    fn cursor_target(&self, rows: &[String], delta: isize) -> Option<(Option<usize>, usize)> {
        if rows.is_empty() {
            return None;
        }
        let current = self
            .cursor
            .as_ref()
            .and_then(|c| rows.iter().position(|r| r == c));
        let target = match current {
            Some(i) => i.saturating_add_signed(delta).min(rows.len() - 1),
            None if delta < 0 => rows.len() - 1,
            None => 0,
        };
        Some((current, target))
    }

    /// Select the rows between the anchor and `target` (inclusive) and put the
    /// cursor on `target`. Without a usable anchor, `target` becomes it.
    fn select_range_to(&mut self, rows: &[String], target: usize) {
        let anchor = self
            .anchor
            .as_ref()
            .and_then(|a| rows.iter().position(|r| r == a))
            .unwrap_or(target);
        self.paths = crate::view_utils::shift_click_range(Some(anchor), target)
            .into_iter()
            .map(|i| rows[i].clone())
            .collect();
        self.anchor = Some(rows[anchor].clone());
        self.cursor = Some(rows[target].clone());
    }

    /// Forget paths that are no longer in the list.
    pub(crate) fn retain(&mut self, rows: &HashSet<&str>) {
        self.paths.retain(|p| rows.contains(p.as_str()));
        if self.anchor.as_deref().is_some_and(|a| !rows.contains(a)) {
            self.anchor = None;
        }
        if self.cursor.as_deref().is_some_and(|c| !rows.contains(c)) {
            self.cursor = None;
        }
    }
}

/// The visible part of the Changes list, as last reported by its scrollable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListViewport {
    /// How far the list is scrolled down.
    pub offset: f32,
    /// Height of the visible part.
    pub height: f32,
    /// Height of all the rows together.
    pub content_height: f32,
}

impl From<iced::widget::scrollable::Viewport> for ListViewport {
    fn from(vp: iced::widget::scrollable::Viewport) -> Self {
        Self {
            offset: vp.absolute_offset().y,
            height: vp.bounds().height,
            content_height: vp.content_bounds().height,
        }
    }
}

impl ListViewport {
    /// The offset that brings row `index` of `count` fully into view (just
    /// enough to show it), or `None` when it is visible already. The rows are
    /// taken to be the same height; the spacing between them is under a pixel
    /// off per row.
    pub(crate) fn reveal(&self, index: usize, count: usize) -> Option<f32> {
        if count == 0 {
            return None;
        }
        let row = self.content_height / count as f32;
        let top = index as f32 * row;
        let bottom = top + row;
        if top < self.offset {
            Some(top)
        } else if bottom > self.offset + self.height {
            Some(bottom - self.height)
        } else {
            None
        }
    }
}

impl RepoTab {
    /// Drop Changes-list selection entries whose files have no changes left.
    pub(crate) fn prune_changes_selection(&mut self) {
        let rows: HashSet<&str> = self
            .unstaged_changes
            .iter()
            .chain(&self.staged_changes)
            .map(|f| f.display_path())
            .collect();
        self.changes_selection.retain(&rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn selected(sel: &ChangesSelection) -> Vec<&str> {
        let mut v: Vec<&str> = sel.paths.iter().map(String::as_str).collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn click_selects_one_row_and_sets_the_anchor() {
        let rows = rows(&["a", "b", "c"]);
        let mut sel = ChangesSelection::default();
        sel.click(&rows, "a", ClickKind::Single);
        sel.click(&rows, "b", ClickKind::Single);
        assert_eq!(selected(&sel), ["b"]);
        assert_eq!(sel.anchor.as_deref(), Some("b"));
        assert_eq!(sel.cursor.as_deref(), Some("b"));
    }

    #[test]
    fn shift_click_selects_from_the_anchor_either_way() {
        let rows = rows(&["a", "b", "c", "d", "e"]);
        let mut sel = ChangesSelection::default();
        sel.click(&rows, "b", ClickKind::Single);
        sel.click(&rows, "d", ClickKind::Range);
        assert_eq!(selected(&sel), ["b", "c", "d"]);
        // The anchor stays put, so a second Shift+click re-ranges from it.
        sel.click(&rows, "a", ClickKind::Range);
        assert_eq!(selected(&sel), ["a", "b"]);
        assert_eq!(sel.anchor.as_deref(), Some("b"));
        assert_eq!(sel.cursor.as_deref(), Some("a"));
    }

    #[test]
    fn shift_click_without_an_anchor_selects_the_clicked_row() {
        let rows = rows(&["a", "b", "c"]);
        let mut sel = ChangesSelection::default();
        sel.click(&rows, "c", ClickKind::Range);
        assert_eq!(selected(&sel), ["c"]);
        assert_eq!(sel.anchor.as_deref(), Some("c"));
    }

    #[test]
    fn ctrl_click_toggles_one_row_and_moves_the_anchor() {
        let rows = rows(&["a", "b", "c", "d"]);
        let mut sel = ChangesSelection::default();
        sel.click(&rows, "a", ClickKind::Single);
        sel.click(&rows, "c", ClickKind::Toggle);
        assert_eq!(selected(&sel), ["a", "c"]);
        sel.click(&rows, "a", ClickKind::Toggle);
        assert_eq!(selected(&sel), ["c"]);
        // Shift+click ranges from the last Ctrl+clicked row.
        sel.click(&rows, "d", ClickKind::Range);
        assert_eq!(selected(&sel), ["a", "b", "c", "d"]);
    }

    #[test]
    fn shift_arrows_grow_and_shrink_the_range_from_the_anchor() {
        let rows = rows(&["a", "b", "c", "d"]);
        let mut sel = ChangesSelection::default();
        sel.click(&rows, "b", ClickKind::Single);
        assert_eq!(sel.extend(&rows, 1).as_deref(), Some("c"));
        assert_eq!(sel.extend(&rows, 1).as_deref(), Some("d"));
        assert_eq!(selected(&sel), ["b", "c", "d"]);
        // Clamped at the end.
        assert_eq!(sel.extend(&rows, 1), None);
        sel.extend(&rows, -1);
        sel.extend(&rows, -1);
        sel.extend(&rows, -1);
        assert_eq!(selected(&sel), ["a", "b"]);
        assert_eq!(sel.cursor.as_deref(), Some("a"));
    }

    #[test]
    fn shift_arrows_without_a_cursor_start_at_an_end() {
        let rows = rows(&["a", "b", "c"]);
        let mut sel = ChangesSelection::default();
        assert_eq!(sel.extend(&rows, 1).as_deref(), Some("a"));
        assert_eq!(selected(&sel), ["a"]);

        let mut sel = ChangesSelection::default();
        assert_eq!(sel.extend(&rows, -1).as_deref(), Some("c"));
        assert_eq!(selected(&sel), ["c"]);

        assert_eq!(ChangesSelection::default().extend(&[], 1), None);
    }

    #[test]
    fn arrows_move_a_single_row_selection() {
        let rows = rows(&["a", "b", "c"]);
        let mut sel = ChangesSelection::default();
        // Without a cursor, Down starts at the top and Up at the bottom.
        assert_eq!(sel.step(&rows, 1).as_deref(), Some("a"));
        assert_eq!(sel.step(&rows, 1).as_deref(), Some("b"));
        assert_eq!(selected(&sel), ["b"]);
        assert_eq!(sel.anchor.as_deref(), Some("b"));
        assert_eq!(sel.step(&rows, -1).as_deref(), Some("a"));
        // Clamped at the top.
        assert_eq!(sel.step(&rows, -1), None);
        assert_eq!(selected(&sel), ["a"]);

        let mut sel = ChangesSelection::default();
        assert_eq!(sel.step(&rows, -1).as_deref(), Some("c"));
        assert_eq!(ChangesSelection::default().step(&[], 1), None);
    }

    #[test]
    fn arrows_collapse_a_range_to_the_next_row_or_the_cursor_at_an_end() {
        let rows = rows(&["a", "b", "c", "d"]);
        let mut sel = ChangesSelection::default();
        sel.click(&rows, "a", ClickKind::Single);
        sel.click(&rows, "c", ClickKind::Range);
        assert_eq!(sel.step(&rows, 1).as_deref(), Some("d"));
        assert_eq!(selected(&sel), ["d"]);
        // Shift+arrows then range from the row reached.
        sel.extend(&rows, -1);
        assert_eq!(selected(&sel), ["c", "d"]);

        sel.click(&rows, "b", ClickKind::Range);
        sel.click(&rows, "d", ClickKind::Toggle);
        assert_eq!(selected(&sel), ["b", "c"]);
        // The cursor is on the last row: Down keeps it there, selected alone.
        assert_eq!(sel.step(&rows, 1).as_deref(), Some("d"));
        assert_eq!(selected(&sel), ["d"]);
    }

    #[test]
    fn reveal_scrolls_just_enough_to_show_the_row() {
        // 10 rows of 20 px, 60 px of them visible from 100 (rows 5 to 7).
        let vp = ListViewport {
            offset: 100.0,
            height: 60.0,
            content_height: 200.0,
        };
        assert_eq!(vp.reveal(5, 10), None);
        assert_eq!(vp.reveal(7, 10), None);
        assert_eq!(vp.reveal(4, 10), Some(80.0));
        assert_eq!(vp.reveal(8, 10), Some(120.0));
        assert_eq!(vp.reveal(0, 0), None);
    }

    #[test]
    fn retain_drops_paths_that_left_the_list() {
        let rows = rows(&["a", "b", "c"]);
        let mut sel = ChangesSelection::default();
        sel.click(&rows, "a", ClickKind::Single);
        sel.click(&rows, "c", ClickKind::Range);
        sel.retain(&HashSet::from(["b"]));
        assert_eq!(selected(&sel), ["b"]);
        assert_eq!(sel.anchor, None);
        assert_eq!(sel.cursor, None);
    }

    #[test]
    fn click_kind_follows_the_held_modifiers() {
        use iced::keyboard::Modifiers;
        assert_eq!(
            ClickKind::from_modifiers(Modifiers::default()),
            ClickKind::Single
        );
        assert_eq!(
            ClickKind::from_modifiers(Modifiers::SHIFT),
            ClickKind::Range
        );
        assert_eq!(
            ClickKind::from_modifiers(Modifiers::COMMAND),
            ClickKind::Toggle
        );
        assert_eq!(
            ClickKind::from_modifiers(Modifiers::SHIFT | Modifiers::COMMAND),
            ClickKind::Range
        );
    }
}

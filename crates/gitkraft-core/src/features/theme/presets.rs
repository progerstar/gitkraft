//! Preset themes — the **single source of truth** for every colour the GUI
//! uses. (The TUI takes its palettes from `tui-nuances`.)
//!
//! Each public function returns a fully-populated [`AppTheme`] with concrete
//! RGB values. The [`theme_by_index`] dispatcher maps an index to the matching
//! constructor; out-of-range indices silently fall back to `default()`.

use super::types::{AppTheme, Rgb};

// ── Theme catalogue ───────────────────────────────────────────────────────────

/// Ordered theme names. The position in this slice **is** the canonical index.
/// Index 0 is the default theme.
pub const THEME_NAMES: &[&str] = &["Ember", "Classic", "Desk Dark", "Desk Light"];

/// Total number of themes.
pub const THEME_COUNT: usize = 4;

/// Names saved by earlier versions, mapped to today's themes.
const LEGACY_NAMES: &[(&str, usize)] = &[
    ("Default", 1),
    ("Postman", 0),
    ("GitHub Desktop Dark", 2),
    ("GitHub Desktop Light", 3),
];

/// Get a theme by index (0-based). Returns the default (`ember()`) for
/// out-of-range.
pub fn theme_by_index(index: usize) -> AppTheme {
    match index {
        1 => classic(),
        2 => desk_dark(),
        3 => desk_light(),
        _ => ember(),
    }
}

/// Find theme index by name (case-insensitive ASCII), also accepting names of
/// earlier versions. Returns `0` (the default) if no match is found.
pub fn theme_index_by_name(name: &str) -> usize {
    THEME_NAMES
        .iter()
        .position(|n| n.eq_ignore_ascii_case(name))
        .or_else(|| {
            LEGACY_NAMES
                .iter()
                .find(|(old, _)| old.eq_ignore_ascii_case(name))
                .map(|&(_, idx)| idx)
        })
        .unwrap_or(0)
}

// ── Theme definitions ─────────────────────────────────────────────────────────

/// GitKraft's original dark theme (called "Default" upstream).
pub fn classic() -> AppTheme {
    AppTheme {
        is_dark: true,
        background: Rgb::new(18, 18, 26),
        surface: Rgb::new(28, 28, 40),
        border: Rgb::new(60, 80, 100),
        selection: Rgb::new(40, 60, 80),
        text_primary: Rgb::new(255, 255, 255),
        text_secondary: Rgb::new(255, 100, 30),
        text_muted: Rgb::new(120, 120, 130),
        accent: Rgb::new(80, 200, 255),
        success: Rgb::new(80, 220, 120),
        warning: Rgb::new(255, 200, 50),
        error: Rgb::new(255, 80, 80),
        diff_add: Rgb::new(80, 220, 120),
        diff_del: Rgb::new(255, 80, 80),
        diff_context: Rgb::new(120, 120, 130),
        diff_hunk: Rgb::new(255, 100, 30),
        graph_colors: [
            Rgb::new(80, 200, 255),
            Rgb::new(80, 220, 120),
            Rgb::new(255, 80, 80),
            Rgb::new(255, 200, 50),
            Rgb::new(255, 100, 30),
            Rgb::new(180, 140, 255),
            Rgb::new(0, 200, 180),
            Rgb::new(255, 130, 160),
        ],
    }
}

/// GitHub Desktop's dark theme: its CSS variables
/// (`app/styles/themes/_dark.scss`) over the Primer palette (`primer-support` 4.7).
pub fn desk_dark() -> AppTheme {
    const GRAY_100: Rgb = Rgb::new(0xf6, 0xf8, 0xfa);
    const GRAY_400: Rgb = Rgb::new(0x95, 0x9d, 0xa5);
    const GRAY_500: Rgb = Rgb::new(0x6a, 0x73, 0x7d);
    const GRAY_700: Rgb = Rgb::new(0x44, 0x4d, 0x56);
    const GRAY_900: Rgb = Rgb::new(0x24, 0x29, 0x2e);
    // --box-background-color: darken($gray-900, 3%)
    const BOX: Rgb = Rgb::new(0x1d, 0x21, 0x25);
    // --box-border-color
    const BORDER: Rgb = Rgb::new(0x14, 0x14, 0x14);
    // $link-color: the blue GitHub Desktop uses for text on dark
    const LINK: Rgb = Rgb::new(0x2e, 0x8f, 0xff);
    // --diff-alt-text-color: darken($gray-100, 20%)
    const DIFF_ALT_TEXT: Rgb = Rgb::new(0xb4, 0xc5, 0xd6);
    const GREEN: Rgb = Rgb::new(0x28, 0xa7, 0x45);
    const RED: Rgb = Rgb::new(0xd7, 0x3a, 0x49);
    const YELLOW_700: Rgb = Rgb::new(0xdb, 0xab, 0x09);

    AppTheme {
        is_dark: true,
        background: GRAY_900,
        surface: BOX,
        border: BORDER,
        selection: GRAY_700,
        text_primary: GRAY_100,
        text_secondary: GRAY_400,
        // GitHub Desktop's muted text (darken($gray-500, 10%)) is too faint
        // for the many hints GitKraft shows, so use $gray-500 itself.
        text_muted: GRAY_500,
        accent: LINK,
        success: GREEN,
        warning: YELLOW_700,
        error: RED,
        diff_add: GREEN,
        diff_del: RED,
        diff_context: DIFF_ALT_TEXT,
        // Hunk headers are a neutral band in GitHub Desktop, not a colour.
        diff_hunk: GRAY_500,
        graph_colors: [
            Rgb::new(0x21, 0x88, 0xff), // blue-400
            Rgb::new(0x34, 0xd0, 0x58), // green-400
            Rgb::new(0xea, 0x4a, 0x5a), // red-400
            Rgb::new(0x8a, 0x63, 0xd2), // purple-400
            Rgb::new(0xfb, 0x85, 0x32), // orange-400
            Rgb::new(0xf9, 0xc5, 0x13), // yellow-600
            Rgb::new(0x79, 0xb8, 0xff), // blue-300
            GRAY_400,
        ],
    }
}

/// GitHub Desktop's light theme: its CSS variables
/// (`app/styles/_variables.scss`) over the Primer palette (`primer-support` 4.7).
pub fn desk_light() -> AppTheme {
    // Primer colours used by GitHub Desktop.
    const WHITE: Rgb = Rgb::new(0xff, 0xff, 0xff);
    const GRAY_100: Rgb = Rgb::new(0xf6, 0xf8, 0xfa);
    const GRAY_200: Rgb = Rgb::new(0xe1, 0xe4, 0xe8);
    const GRAY_400: Rgb = Rgb::new(0x95, 0x9d, 0xa5);
    const GRAY_500: Rgb = Rgb::new(0x6a, 0x73, 0x7d);
    const GRAY_600: Rgb = Rgb::new(0x58, 0x60, 0x69);
    const GRAY_700: Rgb = Rgb::new(0x44, 0x4d, 0x56);
    const GRAY_900: Rgb = Rgb::new(0x24, 0x29, 0x2e);
    const BLUE: Rgb = Rgb::new(0x03, 0x66, 0xd6);
    const GREEN_500: Rgb = Rgb::new(0x28, 0xa7, 0x45);
    const GREEN_600: Rgb = Rgb::new(0x22, 0x86, 0x3a);
    const RED_500: Rgb = Rgb::new(0xd7, 0x3a, 0x49);
    const RED_600: Rgb = Rgb::new(0xcb, 0x24, 0x31);
    const PURPLE_500: Rgb = Rgb::new(0x6f, 0x42, 0xc1);
    const ORANGE_600: Rgb = Rgb::new(0xe3, 0x62, 0x09);
    const YELLOW_800: Rgb = Rgb::new(0xb0, 0x88, 0x00);
    const BLUE_700: Rgb = Rgb::new(0x04, 0x42, 0x89);
    // --color-modified: darken($yellow-700, 10%)
    const MODIFIED: Rgb = Rgb::new(0xaa, 0x85, 0x07);
    // --box-selected-background-color
    const SELECTED: Rgb = Rgb::new(0xeb, 0xee, 0xf1);

    AppTheme {
        is_dark: false,
        background: WHITE,
        surface: GRAY_100,
        border: GRAY_200,
        selection: SELECTED,
        text_primary: GRAY_900,
        text_secondary: GRAY_500,
        text_muted: GRAY_400,
        accent: BLUE,
        success: GREEN_600,
        warning: MODIFIED,
        error: RED_600,
        diff_add: GREEN_600,
        diff_del: RED_600,
        diff_context: GRAY_700,
        diff_hunk: BLUE,
        graph_colors: [
            BLUE, GREEN_500, RED_500, PURPLE_500, ORANGE_600, YELLOW_800, BLUE_700, GRAY_600,
        ],
    }
}

/// Postman's dark theme: the Aether design tokens (`dark` theme over the
/// primitive palette) shipped in the Postman VS Code extension 1.19.1.
/// Graphite surfaces, Postman orange as the accent, and the brand brown
/// (`background-color-brand`) for selection tints and hunk headers.
pub fn ember() -> AppTheme {
    // Primitive palette.
    const GREY_00: Rgb = Rgb::new(0xff, 0xff, 0xff);
    const GREY_40: Rgb = Rgb::new(0xa6, 0xa6, 0xa6);
    const GREY_50: Rgb = Rgb::new(0x6b, 0x6b, 0x6b);
    const GREY_60: Rgb = Rgb::new(0x3b, 0x3b, 0x3b);
    const GREY_85: Rgb = Rgb::new(0x26, 0x26, 0x26);
    const GREY_90: Rgb = Rgb::new(0x21, 0x21, 0x21);
    const ORANGE_50: Rgb = Rgb::new(0xff, 0x6c, 0x37);
    const ORANGE_90: Rgb = Rgb::new(0x55, 0x33, 0x26);
    const GREEN_30: Rgb = Rgb::new(0x6b, 0xdd, 0x9a);
    const YELLOW_30: Rgb = Rgb::new(0xff, 0xe4, 0x7e);
    const RED_30: Rgb = Rgb::new(0xf7, 0x9a, 0x8e);

    AppTheme {
        is_dark: true,
        background: GREY_90,     // background-color-primary
        surface: GREY_85,        // background-color-secondary
        border: GREY_60,         // border-color-strong
        selection: ORANGE_90,    // background-color-brand
        text_primary: GREY_00,   // content-color-primary
        text_secondary: GREY_40, // content-color-secondary
        text_muted: GREY_50,     // content-color-tertiary
        accent: ORANGE_50,       // base-color-brand
        success: GREEN_30,       // content-color-success
        warning: YELLOW_30,      // content-color-warning
        error: RED_30,           // content-color-error
        diff_add: GREEN_30,
        diff_del: RED_30,
        diff_context: GREY_40,
        diff_hunk: ORANGE_50,
        graph_colors: [
            ORANGE_50,
            Rgb::new(0x3e, 0x92, 0xf2), // blue-40
            Rgb::new(0x38, 0xcc, 0x74), // green-40
            Rgb::new(0xff, 0xcf, 0x3f), // yellow-40
            Rgb::new(0x92, 0x6c, 0xc2), // purple-40
            Rgb::new(0x3f, 0xd6, 0xe6), // teal-40
            Rgb::new(0xf1, 0x5e, 0xb0), // pink-40
            Rgb::new(0xf3, 0x72, 0x64), // red-40
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_count_matches_names() {
        assert_eq!(THEME_NAMES.len(), THEME_COUNT);
    }

    #[test]
    fn all_themes_resolve() {
        for i in 0..THEME_COUNT {
            let t = theme_by_index(i);
            let has_colour = t.text_primary.r > 0 || t.text_primary.g > 0 || t.text_primary.b > 0;
            assert!(has_colour, "theme index {i} has zero text_primary");
        }
    }

    #[test]
    fn out_of_range_returns_the_default_ember() {
        assert_eq!(theme_by_index(999).background, ember().background);
        assert_eq!(theme_by_index(0).background, ember().background);
    }

    #[test]
    fn theme_index_by_name_known() {
        assert_eq!(theme_index_by_name("ember"), 0);
        assert_eq!(theme_index_by_name("Classic"), 1);
        assert_eq!(theme_index_by_name("desk dark"), 2);
        assert_eq!(theme_index_by_name("Desk Light"), 3);
    }

    #[test]
    fn legacy_names_still_resolve() {
        assert_eq!(theme_index_by_name("Default"), 1);
        assert_eq!(theme_index_by_name("Postman"), 0);
        assert_eq!(theme_index_by_name("GitHub Desktop Dark"), 2);
        assert_eq!(theme_index_by_name("GitHub Desktop Light"), 3);
    }

    #[test]
    fn removed_or_unknown_names_fall_back_to_default() {
        assert_eq!(theme_index_by_name("NonExistent"), 0);
        assert_eq!(theme_index_by_name("Dracula"), 0);
    }

    #[test]
    fn dark_and_light_flags() {
        assert!(ember().is_dark);
        assert!(classic().is_dark);
        assert!(desk_dark().is_dark);
        assert!(!desk_light().is_dark);
    }
}

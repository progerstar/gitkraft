use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoHistoryEntry {
    pub path: PathBuf,
    pub display_name: String,
    pub last_opened: DateTime<Utc>,
}

/// Persisted pane layout dimensions so the UI restores exactly as the user left it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LayoutSettings {
    pub sidebar_width: Option<f32>,
    pub commit_log_width: Option<f32>,
    pub staging_height: Option<f32>,
    pub diff_file_list_width: Option<f32>,
    pub sidebar_expanded: Option<bool>,
    /// UI scale factor (1.0 = 100%, 0.8 = 80%, 1.2 = 120%).
    #[serde(default)]
    pub ui_scale: Option<f32>,
    /// Saved window width in logical pixels.
    #[serde(default)]
    pub window_width: Option<f32>,
    /// Saved window height in logical pixels.
    #[serde(default)]
    pub window_height: Option<f32>,
    /// Saved window X position (top-left corner, logical pixels).
    #[serde(default)]
    pub window_x: Option<f32>,
    /// Saved window Y position (top-left corner, logical pixels).
    #[serde(default)]
    pub window_y: Option<f32>,
    /// Whether the window was maximized when last closed.
    #[serde(default)]
    pub window_maximized: Option<bool>,
    /// Width of the BRANCH/TAG column in the commit log (pixels).
    #[serde(default)]
    pub commit_ref_width: Option<f32>,
    /// Width of the GRAPH column in the commit log (pixels).
    #[serde(default)]
    pub commit_graph_width: Option<f32>,
    /// Width of the list pane in the TUI hub, in percent of the screen (20–80).
    #[serde(default)]
    pub hub_list_percent: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    /// Last opened repository path.
    pub last_repo: Option<PathBuf>,
    /// Recently opened repositories, most recent first.
    pub recent_repos: Vec<RepoHistoryEntry>,
    /// Theme name (GUI) or theme index (TUI).
    pub theme_name: Option<String>,
    /// Max recent repos to store.
    #[serde(default = "default_max_recent")]
    pub max_recent: usize,
    /// Persisted pane layout dimensions.
    #[serde(default)]
    pub layout: Option<LayoutSettings>,
    /// Persisted editor name.
    #[serde(default)]
    pub editor_name: Option<String>,
    /// Paths of all tabs open when the app was last closed, in tab order.
    #[serde(default)]
    pub open_tabs: Vec<PathBuf>,
    /// Index of the active tab within open_tabs.
    #[serde(default)]
    pub active_tab_index: usize,
    /// Shell command that turns a staged diff (on stdin) into a commit message
    /// (on stdout). `None` uses `gitkraft_core::DEFAULT_COMMIT_MESSAGE_COMMAND`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_message_command: Option<String>,
    /// What the ✨ button (and the TUI draft key) asks for a commit message:
    /// `"agent"` runs `commit_message_command`, `"api"` calls the AI API
    /// (`GITKRAFT_AI_*` / `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` or the key
    /// saved by `gitkraft-tui ai-key`, styled by `.gitkraft/ai.toml`).
    #[serde(default)]
    pub commit_message_backend: CommitMessageBackend,
    /// Push the current branch right after a successful commit (GUI checkbox).
    #[serde(default)]
    pub push_after_commit: bool,
    /// GUI layout: `"classic"` or `"two-pane"` (the default when unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_style: Option<String>,
    /// Skip UI animations (menus open instantly).
    #[serde(default)]
    pub reduced_motion: bool,
    /// Start GitKraft (in the tray) when the user logs in.
    #[serde(default)]
    pub autostart: bool,
    /// Show the tray / menu bar icon while the window app runs.
    #[serde(default = "yes")]
    pub tray_icon: bool,
    /// Closing the window hides it to the tray (the schedule and notifications keep running).
    #[serde(default)]
    pub close_to_tray: bool,
}

/// Where commit messages are generated (`commit_message_backend` in `settings.json`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommitMessageBackend {
    /// The local agent command (`commit_message_command`).
    #[default]
    Agent,
    /// The AI API of the hub (Anthropic or an OpenAI-compatible endpoint).
    Api,
}

fn yes() -> bool {
    true
}

fn default_max_recent() -> usize {
    20
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            last_repo: None,
            recent_repos: Vec::new(),
            theme_name: None,
            max_recent: default_max_recent(),
            layout: None,
            editor_name: None,
            open_tabs: Vec::new(),
            active_tab_index: 0,
            commit_message_command: None,
            commit_message_backend: CommitMessageBackend::default(),
            push_after_commit: false,
            layout_style: None,
            reduced_motion: false,
            autostart: false,
            tray_icon: true,
            close_to_tray: false,
        }
    }
}

impl AppSettings {
    /// Whether the theme follows the OS light/dark setting (`theme_name == "System"`).
    pub fn follow_system_theme(&self) -> bool {
        self.theme_name
            .as_deref()
            .is_some_and(crate::theme::is_system_name)
    }

    /// Preset index for the saved theme. `"System"` resolves from the OS
    /// appearance; an unknown name falls back to `0`, a missing one to `DEFAULT_THEME`.
    pub fn theme_index(&self) -> usize {
        match self.theme_name.as_deref() {
            Some(n) if crate::theme::is_system_name(n) => {
                crate::theme::current_system_theme_index()
            }
            Some(n) => crate::theme::theme_index_by_name(n),
            None => crate::theme::DEFAULT_THEME,
        }
    }

    /// Add a repo to the recent list, moving it to the front if it already exists.
    /// Also sets it as the last_repo.
    pub fn add_recent_repo(&mut self, path: PathBuf) {
        let display_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        // Remove existing entry for this path
        self.recent_repos.retain(|e| e.path != path);

        // Insert at front
        self.recent_repos.insert(
            0,
            RepoHistoryEntry {
                path: path.clone(),
                display_name,
                last_opened: Utc::now(),
            },
        );

        // Trim to max
        if self.recent_repos.len() > self.max_recent {
            self.recent_repos.truncate(self.max_recent);
        }

        self.last_repo = Some(path);
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;

    #[test]
    fn system_theme_name_follows_system() {
        let s = AppSettings {
            theme_name: Some("System".to_string()),
            ..Default::default()
        };
        assert!(s.follow_system_theme());
        assert!(s.theme_index() < crate::theme::THEME_COUNT);
        assert!(!AppSettings::default().follow_system_theme());
        assert_eq!(
            AppSettings::default().theme_index(),
            crate::theme::DEFAULT_THEME
        );
    }

    #[test]
    fn theme_index_resolves_known_and_unknown() {
        let mut s = AppSettings {
            theme_name: Some("Desk Light".to_string()),
            ..Default::default()
        };
        assert_eq!(s.theme_index(), 3);
        s.theme_name = Some("Not A Real Theme".to_string());
        assert_eq!(s.theme_index(), 0);
        assert!(!s.follow_system_theme());
        // A fresh install starts on Ember.
        s.theme_name = None;
        assert_eq!(s.theme_index(), crate::theme::DEFAULT_THEME);
        assert_eq!(crate::theme::THEME_NAMES[s.theme_index()], "Ember");
    }

    #[test]
    fn commit_message_backend_defaults_to_the_agent_and_reads_api() {
        let s: AppSettings =
            serde_json::from_str(r#"{"recent_repos": []}"#).expect("settings without it");
        assert_eq!(s.commit_message_backend, CommitMessageBackend::Agent);
        let s: AppSettings =
            serde_json::from_str(r#"{"recent_repos": [], "commit_message_backend": "api"}"#)
                .expect("api");
        assert_eq!(s.commit_message_backend, CommitMessageBackend::Api);
        let json = serde_json::to_string(&AppSettings::default()).expect("json");
        assert!(json.contains(r#""commit_message_backend":"agent""#));
    }
}

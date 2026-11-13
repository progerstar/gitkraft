use gitkraft_gui::GitKraft;
use iced::Settings;

fn main() -> iced::Result {
    tracing_subscriber::fmt::init();

    gitkraft_core::limit_git_memory();

    // Read saved window geometry before starting the app.
    let saved_layout = gitkraft_core::features::persistence::ops::get_saved_layout()
        .ok()
        .flatten()
        .unwrap_or_default();

    let window_size = iced::Size::new(
        saved_layout.window_width.unwrap_or(1400.0),
        saved_layout.window_height.unwrap_or(800.0),
    );

    let window_position = match (saved_layout.window_x, saved_layout.window_y) {
        // Only restore position when it looks like a valid on-screen location.
        (Some(x), Some(y)) if x > -200.0 && y > -200.0 && x < 8000.0 && y < 8000.0 => {
            iced::window::Position::Specific(iced::Point::new(x, y))
        }
        _ => iced::window::Position::Default,
    };

    iced::application(boot, GitKraft::update, GitKraft::view)
        .title("GitKraft — Git IDE")
        .theme(|state: &GitKraft| state.iced_theme())
        .subscription(|state: &GitKraft| {
            let keyboard = iced::event::listen_with(|event, status, _window| {
                // ModifiersChanged fires regardless of focus/status — always track it.
                if let iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(mods)) = event
                {
                    return Some(gitkraft_gui::Message::ModifiersChanged(mods));
                }
                // Track window geometry for persistence.
                if let iced::Event::Window(iced::window::Event::Resized(size)) = &event {
                    return Some(gitkraft_gui::Message::WindowResized(
                        size.width,
                        size.height,
                    ));
                }
                if let iced::Event::Window(iced::window::Event::Moved(point)) = &event {
                    return Some(gitkraft_gui::Message::WindowMoved(point.x, point.y));
                }
                // Ctrl/Cmd shortcuts fire regardless of widget focus so that
                // Ctrl+, (settings), Ctrl+F (search), Ctrl++/- (zoom) etc.
                // always work even when a text input has keyboard focus.
                if let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key,
                    modifiers,
                    ..
                }) = event
                {
                    // Always handle Ctrl/Cmd shortcuts regardless of widget focus.
                    if modifiers.control() || modifiers.command() {
                        return handle_key_press(key, modifiers);
                    }
                    // Handle Shift+arrow shortcuts for range selection, unless a
                    // widget used the key (selecting text in the commit description).
                    if modifiers.shift()
                        && !matches!(status, iced::event::Status::Captured)
                        && matches!(
                            key,
                            iced::keyboard::Key::Named(
                                iced::keyboard::key::Named::ArrowDown
                                    | iced::keyboard::key::Named::ArrowUp
                            )
                        )
                    {
                        return handle_key_press(key, modifiers);
                    }
                    // Other shortcuts only when no widget has keyboard focus.
                    if let iced::event::Status::Ignored = status {
                        return handle_key_press(key, modifiers);
                    }
                }
                None
            });
            let repo_path = state.active_tab().repo_path.clone();
            let git_watcher = git_watch_subscription(repo_path);
            // Animation tick — ~10 fps. Only active while at least one tab is
            // loading to avoid CPU waste at rest.
            let any_loading = state
                .tabs
                .iter()
                .any(|t| t.is_loading || t.generating_commit_message);
            let animation_ticker = if any_loading {
                iced::Subscription::run(animation_tick_stream)
            } else {
                iced::Subscription::none()
            };
            // Background fetch while any open repo has a remote.
            let auto_fetch = if state.tabs.iter().any(|t| !t.remotes.is_empty()) {
                iced::Subscription::run(auto_fetch_stream)
            } else {
                iced::Subscription::none()
            };
            let system_theme = if state.follow_system {
                iced::Subscription::run(system_theme_stream)
            } else {
                iced::Subscription::none()
            };
            iced::Subscription::batch([
                keyboard,
                git_watcher,
                animation_ticker,
                system_theme,
                auto_fetch,
            ])
        })
        .scale_factor(|state: &GitKraft| state.ui_scale)
        .settings(Settings {
            fonts: vec![iced_fonts::BOOTSTRAP_FONT_BYTES.into()],
            default_text_size: iced::Pixels(13.0),
            ..Default::default()
        })
        .window(gitkraft_gui::window::settings(window_size, window_position))
        .run()
}

/// Boot function — initializes application state and returns startup tasks.
fn boot() -> (GitKraft, iced::Task<gitkraft_gui::Message>) {
    let (mut state, open_tabs) = GitKraft::new_with_session_paths();

    let maximize_task: iced::Task<gitkraft_gui::Message> =
        iced::window::oldest().and_then(|id| iced::window::maximize(id, true));

    let restore_task = if !open_tabs.is_empty() {
        // Restore every saved tab in parallel, each loading into its own index.
        let tasks: Vec<iced::Task<gitkraft_gui::Message>> = open_tabs
            .iter()
            .enumerate()
            .filter(|(_, p)| p.exists())
            .map(|(i, p)| gitkraft_gui::features::repo::commands::load_repo_at(i, p.clone()))
            .collect();
        iced::Task::batch(tasks)
    } else {
        // Legacy fallback: restore the single last-opened repo.
        match gitkraft_core::features::persistence::ops::get_last_repo() {
            Ok(Some(path)) if path.exists() => {
                let tab = state.active_tab_mut();
                tab.is_loading = true;
                tab.status_message = Some("Loading repository...".to_string());
                gitkraft_gui::features::repo::commands::load_repo(path)
            }
            _ => iced::Task::none(),
        }
    };

    (
        state,
        iced::Task::batch([maximize_task, gitkraft_gui::window::setup(), restore_task]),
    )
}

/// Reactive git-state watcher subscription.
///
/// Spawns a background OS thread that uses `notify` to watch the repo's `.git`
/// directory and its working tree (what git does not ignore) for any file
/// change.  Changes are debounced (300 ms of quiet) so that a single git
/// operation (which writes several files) triggers only one refresh.
/// A 60-second fallback poll is included so the UI stays current on network
/// file systems or environments where inotify/FSEvents are not delivered.
///
/// Returns `Subscription::none()` when no repository is open.
fn git_watch_subscription(
    repo_path: Option<std::path::PathBuf>,
) -> iced::Subscription<gitkraft_gui::Message> {
    let Some(path) = repo_path else {
        return iced::Subscription::none();
    };
    // `run_with` hashes both `path` and the function pointer as the subscription
    // ID, so switching repos automatically tears down the old watcher and starts
    // a fresh one.
    iced::Subscription::run_with(path, git_watch_builder)
}

/// Indirection required by `Subscription::run_with` (needs `fn` pointer, not closure).
///
/// `run_with` is typed as `fn(&D) -> S` where `D = PathBuf`, so the parameter
/// must be `&PathBuf` even though clippy prefers `&Path`.
#[allow(clippy::ptr_arg)]
fn git_watch_builder(
    path: &std::path::PathBuf,
) -> impl futures::Stream<Item = gitkraft_gui::Message> + use<> {
    git_watch_stream(path.clone())
}

/// Stream that emits `FileSystemChanged` whenever the repository changes.
///
/// Uses `spawn_git_watcher` from core (debounced, fallback poll); its thread
/// hands each change over through an async channel.
fn git_watch_stream(
    repo_path: std::path::PathBuf,
) -> impl futures::Stream<Item = gitkraft_gui::Message> {
    use futures::StreamExt;

    let (tx, rx) = futures::channel::mpsc::unbounded::<()>();
    let git_dir = repo_path.join(".git");

    // The watcher thread exits once the subscription ends and drops `rx`.
    gitkraft_core::spawn_git_watcher(git_dir, move || tx.unbounded_send(()).is_ok());

    rx.map(|()| gitkraft_gui::Message::FileSystemChanged)
}

/// Yields after `first`, then every `period`, waiting on a thread of its own.
///
/// A subscription stream must never block: iced polls every subscription and
/// every task on one small pool (a worker per CPU), so a blocking stream holds
/// a worker for good. With all of them held, task results (fetch, push,
/// refresh…) stop reaching the UI. The thread ends after the stream is dropped.
fn ticker(
    first: std::time::Duration,
    period: std::time::Duration,
) -> impl futures::Stream<Item = ()> {
    let (mut tx, rx) = futures::channel::mpsc::channel(1);
    std::thread::spawn(move || {
        std::thread::sleep(first);
        // A tick the UI has not taken yet makes this one a no-op.
        while !tx.try_send(()).is_err_and(|e| e.is_disconnected()) {
            std::thread::sleep(period);
        }
    });
    rx
}

/// Emits `AnimationTick` at ~10 fps. Only subscribed while loading is in progress.
fn animation_tick_stream() -> impl futures::Stream<Item = gitkraft_gui::Message> {
    use futures::StreamExt;

    let frame = std::time::Duration::from_millis(100);
    ticker(frame, frame).map(|()| gitkraft_gui::Message::AnimationTick)
}

/// Emits [`gitkraft_gui::Message::AutoFetchTick`] 10 s after it starts, then
/// every 5 minutes.
fn auto_fetch_stream() -> impl futures::Stream<Item = gitkraft_gui::Message> {
    use futures::StreamExt;

    ticker(
        std::time::Duration::from_secs(10),
        std::time::Duration::from_secs(300),
    )
    .map(|()| gitkraft_gui::Message::AutoFetchTick)
}

/// Emits [`gitkraft_gui::Message::SystemThemeTick`] every 3 s (only subscribed
/// while the theme follows the system).
fn system_theme_stream() -> impl futures::Stream<Item = gitkraft_gui::Message> {
    use futures::StreamExt;

    let period = std::time::Duration::from_secs(3);
    ticker(period, period).map(|()| gitkraft_gui::Message::SystemThemeTick)
}

/// Map keyboard shortcuts to messages.
fn handle_key_press(
    key: iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
) -> Option<gitkraft_gui::Message> {
    use iced::keyboard::Key;

    use iced::keyboard::key::Named;

    if modifiers.control() || modifiers.command() {
        match key {
            // Ctrl/Cmd + Plus (or Ctrl/Cmd + =)
            Key::Character(ref c) if c.as_str() == "+" || c.as_str() == "=" => {
                Some(gitkraft_gui::Message::ZoomIn)
            }
            // Ctrl/Cmd + Minus
            Key::Character(ref c) if c.as_str() == "-" => Some(gitkraft_gui::Message::ZoomOut),
            // Ctrl/Cmd + 0 — reset zoom
            Key::Character(ref c) if c.as_str() == "0" => Some(gitkraft_gui::Message::ZoomReset),
            // Ctrl/Cmd + F — toggle search
            Key::Character(ref c) if c.as_str() == "f" => Some(gitkraft_gui::Message::ToggleSearch),
            // Ctrl/Cmd + , — open settings.json in the configured editor (like Zed)
            Key::Character(ref c) if c.as_str() == "," => {
                Some(gitkraft_gui::Message::OpenSettingsFile)
            }
            // Ctrl/Cmd + T or N — new tab
            Key::Character(ref c) if c.as_str() == "t" || c.as_str() == "n" => {
                Some(gitkraft_gui::Message::NewTab)
            }
            // Ctrl/Cmd + W — close current tab / repo
            Key::Character(ref c) if c.as_str() == "w" => Some(gitkraft_gui::Message::CloseRepo),
            // Ctrl/Cmd + R — refresh
            Key::Character(ref c) if c.as_str() == "r" => Some(gitkraft_gui::Message::RefreshRepo),
            _ => None,
        }
    } else if modifiers.shift() {
        // Shift+arrow: extend range selection in the file list or commit log.
        match key {
            Key::Named(Named::ArrowDown) => Some(gitkraft_gui::Message::ShiftArrowDown),
            Key::Named(Named::ArrowUp) => Some(gitkraft_gui::Message::ShiftArrowUp),
            _ => None,
        }
    } else {
        // Bare-key shortcuts (only when no widget has focus / Status::Ignored)
        match key {
            Key::Named(iced::keyboard::key::Named::Escape) => {
                // Esc closes the blame overlay, context menu, or other overlays.
                Some(gitkraft_gui::Message::CloseFileBlame)
            }
            // F5 — refresh
            Key::Named(iced::keyboard::key::Named::F5) => Some(gitkraft_gui::Message::RefreshRepo),
            // Space — stage / unstage the files selected in the Changes list
            Key::Named(Named::Space) => Some(gitkraft_gui::Message::ToggleSelectedChangesStaged),
            // Up / Down — move through the Changes list
            Key::Named(Named::ArrowDown) => Some(gitkraft_gui::Message::ArrowDown),
            Key::Named(Named::ArrowUp) => Some(gitkraft_gui::Message::ArrowUp),
            _ => None,
        }
    }
}

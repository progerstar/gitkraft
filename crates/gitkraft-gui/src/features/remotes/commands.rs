//! Async command helpers for remote operations.
//!
//! Each function clones the `PathBuf`, spawns blocking work that opens the repo
//! inside, performs the remote operation, and maps the result into a [`Message`].

use std::path::PathBuf;

use iced::Task;

use crate::macros::StringErr;
use crate::message::Message;

/// Fetch from a named remote, then reload the repo.
///
/// If the remote doesn't exist or authentication is required, the task resolves
/// to an error message.
pub(crate) fn fetch_remote(path: PathBuf, remote_name: String) -> Task<Message> {
    remote_op_then_reload!(path, "Fetch completed.".to_string(), |wd| {
        // System git, not libgit2: respects credential helpers and the SSH agent.
        gitkraft_core::features::branches::fetch_remote_cli(&wd, &remote_name)
            .map_err(|e| format!("Fetch failed: {e}"))
    })
}

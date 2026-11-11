//! Update logic for remote-related messages.

use iced::Task;

use crate::message::Message;
use crate::state::GitKraft;

use super::commands;

/// Handle all remote-related messages, returning a [`Task`] for any follow-up
/// async work.
pub(crate) fn update(state: &mut GitKraft, message: Message) -> Task<Message> {
    match message {
        Message::Fetch => {
            let remote_name = state.active_tab().default_remote();
            state.start_remote_op(format!("Fetching from '{remote_name}'…"), |repo_path| {
                commands::fetch_remote(repo_path, remote_name)
            })
        }

        _ => Task::none(),
    }
}

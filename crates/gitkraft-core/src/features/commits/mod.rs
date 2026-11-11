//! Commit types and operations — list, create, and inspect commits.

pub mod actions;
pub mod message_gen;
pub mod ops;
pub mod types;

pub use actions::{COMMIT_MENU_GROUPS, CommitAction, CommitActionKind};
pub use message_gen::{
    DEFAULT_COMMIT_MESSAGE_COMMAND, generate_commit_message, split_commit_message, staged_patch,
};
pub use ops::*;
pub use types::*;

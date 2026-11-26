//! What each source may write: the owner everything, MCP clients as configured, CLI agents
//! only proposals. Pure.

use serde::{Deserialize, Serialize};

use crate::command::{Command, CommandError};
use crate::event::Source;
use crate::visibility::McpSettings;

/// A configured MCP access level, ordered by power.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpLevel {
    /// Reads only.
    ReadOnly,
    /// Reads, captures into the inbox, and submits proposals the owner reviews.
    Propose,
    /// Writes directly; the owner can undo everything it did in bulk.
    Trusted,
}

/// What a source is allowed to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// The CLI and the GUI.
    Owner,
    /// A trusted MCP client.
    Trusted,
    /// Capture and propose only.
    Propose,
    /// Nothing but reads.
    ReadOnly,
}

impl From<McpLevel> for Access {
    fn from(level: McpLevel) -> Self {
        match level {
            McpLevel::ReadOnly => Access::ReadOnly,
            McpLevel::Propose => Access::Propose,
            McpLevel::Trusted => Access::Trusted,
        }
    }
}

impl From<Access> for McpLevel {
    fn from(access: Access) -> Self {
        match access {
            Access::Owner | Access::Trusted => McpLevel::Trusted,
            Access::Propose => McpLevel::Propose,
            Access::ReadOnly => McpLevel::ReadOnly,
        }
    }
}

/// The access of `source`; MCP clients get their configured level, else the default.
pub fn access(source: &Source, mcp: &McpSettings) -> Access {
    match source {
        Source::Cli | Source::Gui => Access::Owner,
        Source::Agent { .. } => Access::Propose,
        Source::Mcp { client } => mcp
            .clients
            .get(client)
            .copied()
            .unwrap_or(mcp.default_level)
            .into(),
    }
}

/// Whether `access` may run `cmd`.
///
/// # Errors
/// [`CommandError::ReadOnly`], [`CommandError::ProposalOnly`], or [`CommandError::OwnerOnly`].
pub fn check(access: Access, cmd: &Command) -> Result<(), CommandError> {
    match access {
        Access::Owner => Ok(()),
        Access::Trusted => match cmd {
            Command::AcceptProposal(_)
            | Command::RejectProposal(_)
            | Command::Undo
            | Command::UndoSource(_) => Err(CommandError::OwnerOnly),
            _ => Ok(()),
        },
        Access::Propose => match cmd {
            Command::Capture(_) | Command::Propose(_) => Ok(()),
            _ => Err(CommandError::ProposalOnly),
        },
        Access::ReadOnly => Err(CommandError::ReadOnly),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Capture;

    #[test]
    fn mcp_clients_use_their_level_or_the_default() {
        let mut mcp = McpSettings::default();
        mcp.clients.insert("claude-code".into(), McpLevel::Trusted);
        let client = |c: &str| Source::Mcp { client: c.into() };
        assert_eq!(access(&client("claude-code"), &mcp), Access::Trusted);
        assert_eq!(access(&client("other"), &mcp), Access::Propose);
        assert_eq!(
            access(&Source::Agent { name: "x".into() }, &mcp),
            Access::Propose
        );
        assert_eq!(access(&Source::Gui, &mcp), Access::Owner);
    }

    #[test]
    fn levels_gate_commands() {
        let capture = Command::Capture(Capture { text: "x".into() });
        assert!(check(Access::Propose, &capture).is_ok());
        assert!(matches!(
            check(Access::Propose, &Command::Undo),
            Err(CommandError::ProposalOnly)
        ));
        assert!(matches!(
            check(Access::Trusted, &Command::Undo),
            Err(CommandError::OwnerOnly)
        ));
        assert!(matches!(
            check(Access::ReadOnly, &capture),
            Err(CommandError::ReadOnly)
        ));
        assert!(check(Access::Trusted, &capture).is_ok());
    }
}

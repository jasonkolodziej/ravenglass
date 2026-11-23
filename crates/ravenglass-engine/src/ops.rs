//! Journaled user actions (architecture §5 "Offline actions and conflicts"). Each one applies to
//! the local store at once, is replayed against the server, and knows its inverse for undo and
//! for rolling back a refused replay.

use mail_core::*;
use mail_store::Store;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Op {
    /// `before` holds each message's flags before the change, for the inverse.
    SetFlags { account: AccountId, before: Vec<(MessageId, Flags)>, add: Flags, remove: Flags },
    /// `from` holds each message's folder before the move.
    Move { account: AccountId, from: Vec<(MessageId, FolderId)>, to: FolderId },
}

impl Op {
    pub fn account(&self) -> &AccountId { match self { Op::SetFlags { account, .. } | Op::Move { account, .. } => account } }

    pub fn apply(&self, store: &mut Store) -> mail_store::Result<()> {
        match self {
            Op::SetFlags { before, add, remove, .. } => store.set_flags(&ids(before), *add, *remove),
            Op::Move { from, to, .. } => store.move_messages(&ids(from), to),
        }
    }

    pub fn revert(&self, store: &mut Store) -> mail_store::Result<()> {
        match self {
            Op::SetFlags { before, .. } => before.iter().try_for_each(|(id, flags)| store.set_flags(std::slice::from_ref(id), *flags, Flags(!flags.0))),
            Op::Move { from, .. } => from.iter().try_for_each(|(id, folder)| store.move_messages(std::slice::from_ref(id), folder)),
        }
    }

    pub async fn replay(&self, backend: &dyn MailBackend) -> Result<()> {
        match self {
            Op::SetFlags { before, add, remove, .. } => backend.set_flags(&ids(before), *add, *remove).await,
            Op::Move { from, to, .. } => backend.move_to(&ids(from), to).await,
        }
    }

    /// Undo on the server an action it already accepted.
    pub async fn replay_inverse(&self, backend: &dyn MailBackend) -> Result<()> {
        match self {
            Op::SetFlags { before, .. } => {
                for (id, flags) in before { backend.set_flags(std::slice::from_ref(id), *flags, Flags(!flags.0)).await?; }
                Ok(())
            }
            Op::Move { from, .. } => {
                for (id, folder) in from { backend.move_to(std::slice::from_ref(id), folder).await?; }
                Ok(())
            }
        }
    }
}

fn ids<T>(pairs: &[(MessageId, T)]) -> Vec<MessageId> { pairs.iter().map(|(id, _)| id.clone()).collect() }

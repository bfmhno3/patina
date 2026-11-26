//! Typed identifiers and short references.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub(crate) Uuid);

        impl $name {
            /// Wraps an existing UUID.
            pub fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            /// The underlying UUID.
            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

id_type!(
    /// Identifier of one event.
    EventId
);
id_type!(
    /// Identifier shared by all events of one command.
    CommandId
);
id_type!(
    /// Identifier of one device (one JSONL file).
    DeviceId
);
id_type!(
    /// Identifier of a task item (inbox entry, task, or composite node).
    ItemId
);
id_type!(
    /// Identifier of a habit.
    HabitId
);
id_type!(
    /// Identifier of an identity.
    IdentityId
);
id_type!(
    /// Identifier of one timer segment.
    SegmentId
);
id_type!(
    /// Identifier of a proposal.
    ProposalId
);

/// Creates a UUIDv7 for `now`; ids made with one context sort in creation order.
pub(crate) fn new_v7(now: jiff::Timestamp, ctx: &uuid::ContextV7) -> Uuid {
    let secs = u64::try_from(now.as_second()).unwrap_or(0);
    let nanos = u32::try_from(now.subsec_nanosecond()).unwrap_or(0);
    Uuid::new_v7(uuid::Timestamp::from_unix(ctx, secs, nanos))
}

/// Smallest length `>= 4` such that the trailing hex characters of every id are unique.
pub fn short_len<'a>(ids: impl Iterator<Item = &'a Uuid>) -> usize {
    let hex: Vec<String> = ids.map(|id| id.simple().to_string()).collect();
    for len in 4..32 {
        let mut seen = HashSet::new();
        if hex.iter().all(|h| seen.insert(&h[32 - len..])) {
            return len;
        }
    }
    32
}

/// The last `len` hex characters of `id`.
pub fn short(id: &Uuid, len: usize) -> String {
    let hex = id.simple().to_string();
    hex[32 - len.min(32)..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_len_grows_until_unique() {
        let a = Uuid::from_u128(0x1_0000);
        let b = Uuid::from_u128(0x2_0000);
        let c = Uuid::from_u128(0x1_0000 + 0x1000_0000);
        assert_eq!(short_len([&a, &b].into_iter()), 5);
        assert_eq!(short_len([&a, &c].into_iter()), 8);
        assert_eq!(short(&a, 4), "0000");
    }
}

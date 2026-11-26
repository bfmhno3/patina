//! Resolution of user-supplied references (short ids, full ids, exact names).

use serde::Serialize;
use uuid::Uuid;

use crate::ids::{HabitId, IdentityId, ItemId, ProposalId};
use crate::state::{ItemStatus, State};

/// A reference could not be resolved to exactly one entity.
#[derive(Clone, Debug, PartialEq, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum ResolveError {
    /// Nothing matches.
    #[error("no match for {input:?}")]
    NotFound {
        /// The reference as given.
        input: String,
    },
    /// Several things match.
    #[error("{input:?} is ambiguous ({} candidates)", candidates.len())]
    Ambiguous {
        /// The reference as given.
        input: String,
        /// Matching ids.
        candidates: Vec<Uuid>,
    },
}

struct Entry<'a> {
    id: Uuid,
    name: &'a str,
    open: bool,
}

fn resolve_entries(input: &str, entries: &[Entry<'_>]) -> Result<Uuid, ResolveError> {
    let not_found = || ResolveError::NotFound {
        input: input.to_string(),
    };
    let trimmed = input.trim();
    if let Ok(id) = Uuid::parse_str(trimmed) {
        return entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.id)
            .ok_or_else(not_found);
    }
    if (4..=32).contains(&trimmed.len()) && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        let suffix = trimmed.to_ascii_lowercase();
        let hits: Vec<Uuid> = entries
            .iter()
            .filter(|e| e.id.simple().to_string().ends_with(&suffix))
            .map(|e| e.id)
            .collect();
        match hits.as_slice() {
            [one] => return Ok(*one),
            [] => {}
            _ => {
                return Err(ResolveError::Ambiguous {
                    input: input.to_string(),
                    candidates: hits,
                });
            }
        }
    }
    let named: Vec<&Entry<'_>> = entries
        .iter()
        .filter(|e| e.name.trim() == trimmed)
        .collect();
    let open: Vec<&&Entry<'_>> = named.iter().filter(|e| e.open).collect();
    match (open.as_slice(), named.as_slice()) {
        ([one], _) => Ok(one.id),
        ([], [one]) => Ok(one.id),
        ([], []) => Err(not_found()),
        _ => {
            let pool: Vec<Uuid> = if open.len() > 1 {
                open.iter().map(|e| e.id).collect()
            } else {
                named.iter().map(|e| e.id).collect()
            };
            Err(ResolveError::Ambiguous {
                input: input.to_string(),
                candidates: pool,
            })
        }
    }
}

/// Resolves an item reference; discarded items are excluded.
///
/// # Errors
/// [`ResolveError`] when nothing or several things match.
pub fn item(state: &State, input: &str) -> Result<ItemId, ResolveError> {
    let entries: Vec<Entry<'_>> = state
        .items
        .values()
        .filter(|i| i.status != ItemStatus::Discarded)
        .map(|i| Entry {
            id: i.id.as_uuid(),
            name: &i.title,
            open: matches!(i.status, ItemStatus::Inbox | ItemStatus::Todo),
        })
        .collect();
    resolve_entries(input, &entries).map(ItemId::from_uuid)
}

/// Resolves a habit reference.
///
/// # Errors
/// [`ResolveError`] when nothing or several things match.
pub fn habit(state: &State, input: &str) -> Result<HabitId, ResolveError> {
    let entries: Vec<Entry<'_>> = state
        .habits
        .values()
        .map(|h| Entry {
            id: h.id.as_uuid(),
            name: &h.name,
            open: true,
        })
        .collect();
    resolve_entries(input, &entries).map(HabitId::from_uuid)
}

/// Resolves an identity by case-insensitive exact name.
///
/// # Errors
/// [`ResolveError::NotFound`] when no identity has that name.
pub fn identity(state: &State, input: &str) -> Result<IdentityId, ResolveError> {
    if let Ok(uuid) = Uuid::parse_str(input.trim()) {
        let id = IdentityId::from_uuid(uuid);
        if state.identities.contains_key(&id) {
            return Ok(id);
        }
    }
    let lower = input.trim().to_lowercase();
    state
        .identities
        .values()
        .find(|i| i.name.to_lowercase() == lower)
        .map(|i| i.id)
        .ok_or_else(|| ResolveError::NotFound {
            input: input.to_string(),
        })
}

/// Resolves a proposal by short id, full id, or exact summary; pending ones win name ties.
///
/// # Errors
/// [`ResolveError`] when nothing or several things match.
pub fn proposal(state: &State, input: &str) -> Result<ProposalId, ResolveError> {
    let entries: Vec<Entry<'_>> = state
        .proposals
        .values()
        .map(|p| Entry {
            id: p.id.as_uuid(),
            name: &p.summary,
            open: p.resolution.is_none(),
        })
        .collect();
    resolve_entries(input, &entries).map(ProposalId::from_uuid)
}

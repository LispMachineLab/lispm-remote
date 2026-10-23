// lispm-remote: the resources the relay can send.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The relay's side of the resource pool: every image Emacs transmitted,
//! by hash, for clients whose own pool lacks it.  It is bounded; the
//! oldest resources go first, and a client asking for one that went is
//! told `Unknown` and simply draws without it until Emacs sends it again.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

use lispm_remote_display::ResourceId;

/// The most bytes the pool keeps.
pub const MAX_POOL_BYTES: usize = 128 * 1024 * 1024;

#[derive(Default)]
struct Entries {
    bytes: HashMap<ResourceId, Arc<[u8]>>,
    order: VecDeque<ResourceId>,
    total: usize,
}

/// Resources by id.
///
/// Invariant: `total` is the sum of the kept resources' lengths, at
/// most [`MAX_POOL_BYTES`] after every insertion, and `order` lists each
/// kept id once, oldest first.
#[derive(Default)]
pub struct Pool {
    entries: Mutex<Entries>,
}

impl Pool {
    /// Keeps `bytes` as the resource `id`.
    ///
    /// Pre: `id` names `bytes`.
    pub fn insert(&self, id: ResourceId, bytes: Arc<[u8]>) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if entries.bytes.contains_key(&id) || bytes.len() > MAX_POOL_BYTES {
            return;
        }
        entries.total += bytes.len();
        entries.order.push_back(id);
        entries.bytes.insert(id, bytes);
        while entries.total > MAX_POOL_BYTES {
            let Some(oldest) = entries.order.pop_front() else {
                break;
            };
            if let Some(gone) = entries.bytes.remove(&oldest) {
                entries.total -= gone.len();
            }
        }
    }

    /// The resource `id`, if kept.
    #[must_use]
    pub fn get(&self, id: &ResourceId) -> Option<Arc<[u8]>> {
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.bytes.get(id).map(Arc::clone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_resource_is_given_back() {
        let pool = Pool::default();
        let bytes: Arc<[u8]> = Arc::from(&b"png"[..]);
        let id = ResourceId::of(&bytes);
        pool.insert(id, Arc::clone(&bytes));
        assert_eq!(pool.get(&id), Some(bytes));
        assert_eq!(pool.get(&ResourceId::of(b"other")), None);
    }
}

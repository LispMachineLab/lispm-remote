// lispm-remote: the replay store a node keeps its records in.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The directory a node keeps its replay records in.
//!
//! Rings refuses a sender whose message sequences go back, so a node that
//! restarts must find its records again.  A store written by an earlier
//! Rings may not decode: Rings 0.35 changed the record format
//! (RingsNetwork/rings#936), and Rings 0.37 asks every store to be removed
//! (RingsNetwork/rings#1072).  The store's directory names the Rings whose
//! records it holds in [`FORMAT_FILE`]; a store of another is removed
//! before it opens, which resets each stream's replay window, as clearing a
//! stream does.
//!
//! ```text
//!   open(d) = (format(d) ≠ FORMAT ⇒ remove(replay(d)) ; format(d) ← FORMAT) ;
//!             FileStorage(replay(d))
//! ```

use std::path::Path;
use std::path::PathBuf;

use rings_core::message::ReplayStorage;
use rings_core::storage::RecordAuthority;
use rings_core::storage::file::FileStorage;
use rings_node::native::stores::TRANSACTION_REPLAY_STORAGE_CAPACITY;
use rings_node::native::stores::TRANSACTION_REPLAY_STORAGE_DIR;

use crate::node::NodeError;

/// The file in a node's store that names the Rings whose records its
/// replay store holds.
const FORMAT_FILE: &str = "transaction-replay.format";
/// The Rings whose records this build writes.
const FORMAT: &str = "rings-0.37";

/// The replay store of the node whose store is `store`, opened at Rings's
/// authority for it, as its own daemon opens it: a record it could not
/// keep fails that message, and nothing is evicted.
///
/// # Errors
///
/// [`NodeError::Store`] if the directory could not be prepared,
/// [`NodeError::Rings`] if Rings could not open the store.
pub async fn replay(store: &Path) -> Result<ReplayStorage, NodeError> {
    std::fs::create_dir_all(store).map_err(NodeError::Store)?;
    let directory = replay_directory(store).map_err(NodeError::Store)?;
    let storage = FileStorage::new_with_cap_path_and_authority(
        TRANSACTION_REPLAY_STORAGE_CAPACITY,
        directory,
        RecordAuthority::Authoritative,
    )
    .await
    .map_err(|error| NodeError::Rings(error.to_string()))?;
    Ok(Box::new(storage))
}

/// The replay store's directory in `store`, emptied first when it was
/// written by another Rings than [`FORMAT`].  It is removed before the
/// format is written, so a node stopped in between removes it again.
fn replay_directory(store: &Path) -> std::io::Result<PathBuf> {
    let directory = store.join(TRANSACTION_REPLAY_STORAGE_DIR);
    let format = store.join(FORMAT_FILE);
    let current = std::fs::read_to_string(&format).is_ok_and(|text| text.trim() == FORMAT);
    if !current {
        match std::fs::remove_dir_all(&directory) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        std::fs::write(&format, format!("{FORMAT}\n"))?;
    }
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty directory of the test's own.
    fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("lispm-remote-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn a_replay_store_of_another_rings_is_removed_once() {
        let store = scratch("replay-format");
        let replay = store.join(TRANSACTION_REPLAY_STORAGE_DIR);
        std::fs::create_dir_all(&replay).unwrap();
        std::fs::write(store.join(FORMAT_FILE), "rings-0.35\n").unwrap();
        std::fs::write(replay.join("record"), "rings-0.35").unwrap();
        assert_eq!(replay_directory(&store).unwrap(), replay);
        assert!(!replay.exists());
        std::fs::create_dir_all(&replay).unwrap();
        std::fs::write(replay.join("record"), FORMAT).unwrap();
        assert_eq!(replay_directory(&store).unwrap(), replay);
        assert!(replay.join("record").exists());
        std::fs::remove_dir_all(&store).unwrap();
    }
}

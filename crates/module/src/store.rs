// lispm-remote: the replay store a node keeps its records in.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The directory a node keeps its replay records in, opened once per
//! process.
//!
//! Rings refuses a sender whose message sequences go back, so a node that
//! restarts must find its records again.  Two things stand in the way:
//!
//! - **The format.**  Rings 0.35 changed the record format
//!   (RingsNetwork/rings#936), and a store of an earlier format decodes no
//!   record, so every stream would fail closed.  The store's directory
//!   names its format in [`FORMAT_FILE`]; a store of another format is
//!   removed before it opens, which resets each stream's replay window, as
//!   clearing a stream does.
//! - **The lock.**  Rings locks an open store's directory
//!   (RingsNetwork/rings#1039), and a stopped node that has had peers never
//!   lets go of its store: each connection's callback holds the transport
//!   that holds the connection.  So a store, once open, stays open for the
//!   process, and a node Emacs starts again on its directory shares it.
//!   The stopped node's runtime is gone, so it writes nothing more.
//!
//! ```text
//!   open(d) = OPEN[d]                                          if d ∈ OPEN
//!           = (format(d) ≠ FORMAT ⇒ remove(replay(d)) ; format(d) ← FORMAT) ;
//!             OPEN[d] ← FileStorage(replay(d))                 otherwise
//! ```

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rings_core::error::Result;
use rings_core::message::ReplayRecord;
use rings_core::message::ReplayStorage;
use rings_core::storage::KvStorageInterface;
use rings_core::storage::KvStorageScan;
use rings_core::storage::RecordAuthority;
use rings_core::storage::ScannedRecord;
use rings_core::storage::file::FileStorage;
use rings_node::native::stores::TRANSACTION_REPLAY_STORAGE_CAPACITY;
use rings_node::native::stores::TRANSACTION_REPLAY_STORAGE_DIR;
use tokio::sync::Mutex;

use crate::node::NodeError;

/// The file in a node's store that names the record format of its replay
/// store.
const FORMAT_FILE: &str = "transaction-replay.format";
/// The replay record format this build's Rings writes.
const FORMAT: &str = "rings-0.35";

/// The replay stores this process has open, by their node's store.
static OPEN: Mutex<BTreeMap<PathBuf, Arc<FileStorage>>> = Mutex::const_new(BTreeMap::new());

/// The replay store of the node whose store is `store`, opened at Rings's
/// authority for it, as its own daemon opens it: a record it could not
/// keep fails that message, and nothing is evicted.
///
/// # Errors
///
/// [`NodeError::Store`] if the directory could not be prepared,
/// [`NodeError::Rings`] if Rings could not open the store.
pub async fn replay(store: &Path) -> std::result::Result<ReplayStorage, NodeError> {
    std::fs::create_dir_all(store).map_err(NodeError::Store)?;
    let store = std::fs::canonicalize(store).map_err(NodeError::Store)?;
    let mut open = OPEN.lock().await;
    if let Some(storage) = open.get(&store) {
        return Ok(Box::new(Shared(Arc::clone(storage))));
    }
    let directory = replay_directory(&store).map_err(NodeError::Store)?;
    let storage = FileStorage::new_with_cap_path_and_authority(
        TRANSACTION_REPLAY_STORAGE_CAPACITY,
        directory,
        RecordAuthority::Authoritative,
    )
    .await
    .map_err(|error| NodeError::Rings(error.to_string()))?;
    let storage = Arc::new(storage);
    open.insert(store, Arc::clone(&storage));
    Ok(Box::new(Shared(storage)))
}

/// The replay store's directory in `store`, emptied first when it was
/// written in another format than [`FORMAT`].  It is removed before the
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

/// A replay store the nodes started on one directory share.
struct Shared(Arc<FileStorage>);

#[async_trait]
impl KvStorageInterface<ReplayRecord> for Shared {
    async fn get(&self, key: &str) -> Result<Option<ReplayRecord>> {
        KvStorageInterface::<ReplayRecord>::get(&*self.0, key).await
    }

    async fn put(&self, key: &str, value: &ReplayRecord) -> Result<()> {
        self.0.put(key, value).await
    }

    async fn get_all(&self) -> Result<Vec<(String, ReplayRecord)>> {
        KvStorageInterface::<ReplayRecord>::get_all(&*self.0).await
    }

    async fn remove(&self, key: &str) -> Result<()> {
        KvStorageInterface::<ReplayRecord>::remove(&*self.0, key).await
    }

    async fn clear(&self) -> Result<()> {
        KvStorageInterface::<ReplayRecord>::clear(&*self.0).await
    }

    async fn count(&self) -> Result<u32> {
        KvStorageInterface::<ReplayRecord>::count(&*self.0).await
    }
}

#[async_trait]
impl KvStorageScan<ReplayRecord> for Shared {
    async fn scan(&self) -> Result<Vec<ScannedRecord<ReplayRecord>>> {
        KvStorageScan::<ReplayRecord>::scan(&*self.0).await
    }

    fn record_name(&self, key: &str) -> String {
        KvStorageScan::<ReplayRecord>::record_name(&*self.0, key)
    }
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
    fn a_replay_store_of_another_format_is_removed_once() {
        let store = scratch("replay-format");
        let replay = store.join(TRANSACTION_REPLAY_STORAGE_DIR);
        std::fs::create_dir_all(&replay).unwrap();
        std::fs::write(replay.join("record"), "rings-0.34").unwrap();
        assert_eq!(replay_directory(&store).unwrap(), replay);
        assert!(!replay.exists());
        std::fs::create_dir_all(&replay).unwrap();
        std::fs::write(replay.join("record"), FORMAT).unwrap();
        assert_eq!(replay_directory(&store).unwrap(), replay);
        assert!(replay.join("record").exists());
        std::fs::remove_dir_all(&store).unwrap();
    }
}

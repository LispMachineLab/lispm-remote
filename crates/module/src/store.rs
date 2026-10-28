// lispm-remote: the replay store a node keeps its records in.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Where a node keeps its replay records.
//!
//! Rings refuses a sender whose message sequences go back, so a node that
//! restarts must find its records again.  A node restarts in two ways:
//! Emacs starts it again, and it renews its delegation (`node`), each
//! renewal a new Rings processor of the same account.  So the records live
//! in a [`Replay`] that outlasts each processor: a directory, which also
//! outlasts the process, or a memory the node's processors share.
//!
//! A store written by an earlier Rings may not decode: Rings 0.35 changed
//! the record format (RingsNetwork/rings#936), Rings 0.37 asks every store
//! to be removed (RingsNetwork/rings#1072), and Rings 0.38 changes the
//! record format again (RingsNetwork/rings#1059).  The store's directory
//! names the Rings whose records it holds in [`FORMAT_FILE`]; a store of
//! another is removed before it opens, which resets each stream's replay
//! window, as clearing a stream does.
//!
//! ```text
//!   open(d) = (format(d) ≠ FORMAT ⇒ remove(replay(d)) ; format(d) ← FORMAT) ;
//!             FileStorage(replay(d))
//! ```

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rings_core::message::ReplayRecord;
use rings_core::message::ReplayStorage;
use rings_core::storage::KvStorageInterface;
use rings_core::storage::KvStorageScan;
use rings_core::storage::MemStorage;
use rings_core::storage::RecordAuthority;
use rings_core::storage::ScannedRecord;
use rings_core::storage::file::FileStorage;
use rings_node::native::stores::TRANSACTION_REPLAY_STORAGE_CAPACITY;
use rings_node::native::stores::TRANSACTION_REPLAY_STORAGE_DIR;
use rings_node::release::StoresReleased;

use crate::node::NodeError;

/// The file in a node's store that names the Rings whose records its
/// replay store holds.
const FORMAT_FILE: &str = "transaction-replay.format";
/// The Rings whose records this build writes.
const FORMAT: &str = "rings-0.38";

/// The replay records of a node, which each of its processors opens.
pub enum Replay {
    /// In the node's store, a directory: across processes.
    Directory(PathBuf),
    /// In memory, for a node whose records need not outlive the process.
    Memory(Arc<MemStorage<ReplayRecord>>),
}

impl Replay {
    /// The records in `store`, or in memory without one.
    #[must_use]
    pub fn new(store: Option<&Path>) -> Self {
        match store {
            Some(store) => Self::Directory(store.to_owned()),
            None => Self::Memory(Arc::new(MemStorage::new())),
        }
    }

    /// The records, opened for one processor, and the witness of their
    /// release: Rings locks a directory while it is open
    /// (RingsNetwork/rings#1039), so the next processor opens it once the
    /// witness resolves.  A memory has no lock, and no witness.
    ///
    /// # Errors
    ///
    /// [`NodeError::Store`] if the directory could not be prepared,
    /// [`NodeError::Rings`] if Rings could not open the store.
    pub async fn open(&self) -> Result<(ReplayStorage, StoresReleased), NodeError> {
        match self {
            Self::Directory(store) => {
                let storage = replay(store).await?;
                let released = StoresReleased::of([storage.released()]);
                Ok((Box::new(storage), released))
            }
            Self::Memory(memory) => Ok((
                Box::new(Shared(Arc::clone(memory))),
                StoresReleased::default(),
            )),
        }
    }
}

/// The replay store of the node whose store is `store`, opened at Rings's
/// authority for it, as its own daemon opens it: a record it could not
/// keep fails that message, and nothing is evicted.
async fn replay(store: &Path) -> Result<FileStorage, NodeError> {
    std::fs::create_dir_all(store).map_err(NodeError::Store)?;
    let directory = replay_directory(store).map_err(NodeError::Store)?;
    FileStorage::new_with_cap_path_and_authority(
        TRANSACTION_REPLAY_STORAGE_CAPACITY,
        directory,
        RecordAuthority::Authoritative,
    )
    .await
    .map_err(|error| NodeError::Rings(error.to_string()))
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

/// A memory the node's processors share: each operation is the memory's
/// own, so the records one processor wrote are those the next restores.
struct Shared(Arc<MemStorage<ReplayRecord>>);

#[async_trait]
impl KvStorageInterface<ReplayRecord> for Shared {
    async fn get(&self, key: &str) -> rings_core::error::Result<Option<ReplayRecord>> {
        self.0.get(key).await
    }

    async fn put(&self, key: &str, value: &ReplayRecord) -> rings_core::error::Result<()> {
        self.0.put(key, value).await
    }

    async fn get_all(&self) -> rings_core::error::Result<Vec<(String, ReplayRecord)>> {
        self.0.get_all().await
    }

    async fn remove(&self, key: &str) -> rings_core::error::Result<()> {
        self.0.remove(key).await
    }

    async fn clear(&self) -> rings_core::error::Result<()> {
        self.0.clear().await
    }

    async fn count(&self) -> rings_core::error::Result<u32> {
        self.0.count().await
    }
}

#[async_trait]
impl KvStorageScan<ReplayRecord> for Shared {
    async fn scan(&self) -> rings_core::error::Result<Vec<ScannedRecord<ReplayRecord>>> {
        self.0.scan().await
    }

    fn record_name(&self, key: &str) -> String {
        self.0.record_name(key)
    }

    async fn put_batch(&self, records: &[(String, ReplayRecord)]) -> rings_core::error::Result<()> {
        self.0.put_batch(records).await
    }

    async fn remove_batch(&self, keys: &[String]) -> rings_core::error::Result<()> {
        self.0.remove_batch(keys).await
    }
}

#[cfg(test)]
mod tests {
    use std::future::IntoFuture;

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

    #[test]
    fn a_directory_opens_again_once_released() {
        let store = scratch("replay-release");
        let replay = Replay::new(Some(&store));
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (storage, released) = runtime.block_on(replay.open()).unwrap();
        assert_eq!(released.len(), 1);
        drop(storage);
        runtime.block_on(released.into_future());
        let (storage, _) = runtime.block_on(replay.open()).unwrap();
        drop(storage);
        std::fs::remove_dir_all(&store).unwrap();
    }

    #[test]
    fn a_memory_keeps_its_records_for_the_next_processor() {
        let replay = Replay::new(None);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (first, released) = runtime.block_on(replay.open()).unwrap();
        assert!(released.is_empty());
        let record: ReplayRecord = serde_json::from_str("[1,2,3]").unwrap();
        runtime
            .block_on(first.put_batch(&[("stream".to_owned(), record.clone())]))
            .unwrap();
        drop(first);
        let (second, _) = runtime.block_on(replay.open()).unwrap();
        let scanned = runtime.block_on(second.scan()).unwrap();
        assert_eq!(
            scanned,
            vec![ScannedRecord::Filed {
                key: "stream".to_owned(),
                value: record
            }]
        );
    }
}

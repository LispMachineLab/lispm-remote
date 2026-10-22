// lispm-remote: a Rings Network node, as the module runs it.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A Rings node on a runtime of its own, and the queue between it and
//! Emacs.  Emacs's thread never waits on the network: it starts work
//! ([`Node::send`], [`Node::request`]) and collects what came of it
//! ([`Node::events`]).  The node pushes [`Event`]s onto an [`Inbox`] from
//! its runtime's threads; the first push after a drain calls the
//! [`Wake`]r, which for Emacs writes one byte to a pipe process, so a burst
//! of frames costs one wake:
//!
//! ```text
//!   push(e) : queue ← queue · e ;  ¬pending ⇒ (pending ← ⊤ ; wake)
//!   drain   : pending ← ⊥ ; (queue, lost) ← (ε, 0) ; queue · [Lost(lost) | lost > 0]
//! ```
//!
//! Invariant: `|queue| ≤ MAX_QUEUED`.  A frame that finds the queue full
//! is counted in `lost` instead, and the next drain reports the count;
//! replies to Emacs's own requests are always kept, since Emacs waits for
//! them.

use std::collections::VecDeque;
use std::future::Future;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use bytes::Bytes;
use rings_core::delegation::DelegateeKey;
use rings_core::delegation::DelegationBuilder;
use rings_core::dht::Did;
use rings_core::ecc::SecretKey;
use rings_core::storage::MemStorage;
use rings_node::native::config::DEFAULT_NETWORK_ID;
use rings_node::processor::ProcessorBuilder;
use rings_node::processor::ProcessorConfig;
use rings_node::provider::Provider;
use tokio::runtime::Runtime;

use crate::bridge::Forward;
use crate::bridge::Shell;

/// The namespace LispM Remote's frames travel under.
pub const NAMESPACE: &str = "lispm-remote/1";
/// The most events waiting for Emacs at once.
pub const MAX_QUEUED: usize = 4096;
/// How long stopping waits for the runtime's tasks.
const SHUTDOWN: Duration = Duration::from_secs(2);
/// The Chord stabilization interval, in seconds.
const STABILIZE_SECS: u64 = 3;

/// Something to call when events start waiting.
pub trait Wake: Send + Sync + 'static {
    /// Tell the consumer that [`Node::events`] has something.
    fn wake(&self);
}

/// What came of the node's work.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// A frame arrived under [`NAMESPACE`] from the peer `from`, whom
    /// Rings authenticated.
    Frame {
        /// The sender's DID.
        from: String,
        /// The frame's bytes.
        payload: Vec<u8>,
    },
    /// A [`Node::send`] left this node.
    Sent {
        /// The send's identifier.
        id: u64,
    },
    /// A [`Node::request`] answered.
    Reply {
        /// The request's identifier.
        id: u64,
        /// The result, as JSON.
        result: String,
    },
    /// A send or request failed.
    Failed {
        /// Its identifier.
        id: u64,
        /// Why, as Rings put it.
        error: String,
    },
    /// Frames were dropped because Emacs did not collect events in time.
    Lost {
        /// How many.
        frames: u64,
    },
}

impl Event {
    /// Whether Emacs is waiting for this event in particular.
    const fn answers_emacs(&self) -> bool {
        matches!(
            self,
            Self::Sent { .. } | Self::Reply { .. } | Self::Failed { .. }
        )
    }
}

/// The events waiting for Emacs, and how to tell it they are there.
pub(crate) struct Inbox {
    queue: Mutex<VecDeque<Event>>,
    lost: AtomicU64,
    pending: AtomicBool,
    /// Whether waking is still allowed; held across each wake, so once
    /// [`Inbox::close`] returns no wake is under way or can start.
    open: Mutex<bool>,
    waker: Box<dyn Wake>,
}

impl Inbox {
    fn new(waker: impl Wake) -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            lost: AtomicU64::new(0),
            pending: AtomicBool::new(false),
            open: Mutex::new(true),
            waker: Box::new(waker),
        }
    }

    fn queue(&self) -> MutexGuard<'_, VecDeque<Event>> {
        // A panic while holding the lock cannot leave the queue half
        // changed (push and drain are single calls), so poisoning is
        // ignored.
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues `event` and wakes Emacs if nothing was waiting.
    pub(crate) fn push(&self, event: Event) {
        {
            let mut queue = self.queue();
            if queue.len() < MAX_QUEUED || event.answers_emacs() {
                queue.push_back(event);
            } else {
                self.lost.fetch_add(1, Ordering::Relaxed);
            }
        }
        if !self.pending.swap(true, Ordering::AcqRel) {
            let open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
            if *open {
                self.waker.wake();
            }
        }
    }

    /// Stops waking.  Post: no wake runs after this returns.
    fn close(&self) {
        *self.open.lock().unwrap_or_else(PoisonError::into_inner) = false;
    }

    /// Everything waiting, oldest first, with the count of lost frames.
    fn drain(&self) -> Vec<Event> {
        self.pending.store(false, Ordering::Release);
        let mut events: Vec<Event> = self.queue().drain(..).collect();
        let lost = self.lost.swap(0, Ordering::Relaxed);
        if lost > 0 {
            events.push(Event::Lost { frames: lost });
        }
        events
    }
}

/// Why a node could not do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// The runtime could not start.
    #[error("the node's runtime could not start: {0}")]
    Runtime(std::io::Error),
    /// Rings refused.
    #[error("Rings: {0}")]
    Rings(String),
    /// An account key is not 64 hexadecimal digits of a valid secp256k1 key.
    #[error("the account key is invalid")]
    Account,
    /// A DID is not `0x` and 40 hexadecimal digits.
    #[error("{0:?} is not a DID")]
    Did(String),
    /// Request parameters are not a JSON object.
    #[error("request parameters are not JSON: {0}")]
    Params(String),
}

/// A secp256k1 account key: the node's identity on Rings.
pub struct Account(SecretKey);

impl Account {
    /// A fresh account from the operating system's random source.
    #[must_use]
    pub fn generate() -> Self {
        Self(SecretKey::random())
    }

    /// The account whose key is the hexadecimal `text`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Account`] unless `text` encodes a valid key.
    pub fn from_hex(text: &str) -> Result<Self, NodeError> {
        SecretKey::from_str(text)
            .map(Self)
            .map_err(|_| NodeError::Account)
    }

    /// The key, in hexadecimal, for the shell to store.
    #[must_use]
    pub fn to_hex(&self) -> String {
        self.0.to_string()
    }

    /// The DID the account goes by.
    #[must_use]
    pub fn did(&self) -> String {
        Did::from(self.0.address()).to_string()
    }

    /// A session key the account delegates to, as Rings nodes run on.
    fn delegate(&self) -> Result<DelegateeKey, NodeError> {
        let mut builder = DelegationBuilder::new(self.did(), "secp256k1".to_owned());
        let signature = self
            .0
            .sign(&builder.unsigned_proof())
            .map_err(|error| NodeError::Rings(error.to_string()))?;
        builder = builder.set_delegator_signature(signature.to_vec());
        builder
            .build()
            .map_err(|error| NodeError::Rings(error.to_string()))
    }
}

/// A running Rings node.
pub struct Node {
    /// `None` only while the node is being dropped.
    runtime: Option<Runtime>,
    provider: Arc<Provider>,
    inbox: Arc<Inbox>,
    did: String,
    next: AtomicU64,
}

impl Node {
    /// A node for `account`, its ICE servers `ice_servers` (Rings's
    /// `stun://host:port` list), telling `waker` when events wait.
    ///
    /// # Errors
    ///
    /// A [`NodeError`] if the runtime or the Rings processor could not start.
    pub fn start(
        account: &Account,
        ice_servers: &str,
        waker: impl Wake,
    ) -> Result<Self, NodeError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("lispm-remote")
            .enable_all()
            .build()
            .map_err(NodeError::Runtime)?;
        let config = ProcessorConfig::new(
            DEFAULT_NETWORK_ID,
            ice_servers.to_owned(),
            account.delegate()?,
            STABILIZE_SECS,
        );
        let inbox = Arc::new(Inbox::new(waker));
        let shell = Shell::new(Arc::clone(&inbox));
        let provider = runtime
            .block_on(async move {
                let processor = ProcessorBuilder::from_config(&config)?
                    .storage(Box::new(MemStorage::new()))
                    .build()?;
                let provider = Arc::new(Provider::from_processor(Arc::new(processor)));
                provider.register_protocol(Forward, shell)?;
                provider.set_backend()?;
                Ok::<_, rings_node::error::Error>(provider)
            })
            .map_err(|error| NodeError::Rings(error.to_string()))?;
        let listening = Arc::clone(&provider);
        // The listener ends only when the swarm does.
        runtime.spawn(async move {
            let _ = listening.listen().await;
        });
        Ok(Self {
            runtime: Some(runtime),
            provider,
            inbox,
            did: account.did(),
            next: AtomicU64::new(1),
        })
    }

    /// The node's DID.
    #[must_use]
    pub fn did(&self) -> &str {
        &self.did
    }

    fn next_id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    /// Runs `work` on the node's runtime and queues the event it yields.
    fn spawn<F>(&self, work: F)
    where
        F: Future<Output = Event> + Send + 'static,
    {
        let inbox = Arc::clone(&self.inbox);
        if let Some(runtime) = &self.runtime {
            runtime.spawn(async move { inbox.push(work.await) });
        }
    }

    /// Starts sending `payload` to `to` under [`NAMESPACE`]; its
    /// [`Event::Sent`] or [`Event::Failed`] carries the returned id.
    ///
    /// # Errors
    ///
    /// [`NodeError::Did`] if `to` is not a DID.
    pub fn send(&self, to: &str, payload: Vec<u8>) -> Result<u64, NodeError> {
        let to = Did::from_str(to).map_err(|_| NodeError::Did(to.to_owned()))?;
        let id = self.next_id();
        let provider = Arc::clone(&self.provider);
        self.spawn(async move {
            match provider.send(to, NAMESPACE, Bytes::from(payload)).await {
                Ok(()) => Event::Sent { id },
                Err(error) => Event::Failed {
                    id,
                    error: error.to_string(),
                },
            }
        });
        Ok(id)
    }

    /// Starts the JSON-RPC request `method` with `params`, a JSON object;
    /// its [`Event::Reply`] or [`Event::Failed`] carries the returned id.
    ///
    /// # Errors
    ///
    /// [`NodeError::Params`] if `params` is not JSON.
    pub fn request(&self, method: &str, params: &str) -> Result<u64, NodeError> {
        let params: serde_json::Value =
            serde_json::from_str(params).map_err(|error| NodeError::Params(error.to_string()))?;
        let id = self.next_id();
        let method = method.to_owned();
        let provider = Arc::clone(&self.provider);
        self.spawn(async move {
            match provider.request_internal(method, params).await {
                Ok(result) => Event::Reply {
                    id,
                    result: result.to_string(),
                },
                Err(error) => Event::Failed {
                    id,
                    error: error.to_string(),
                },
            }
        });
        Ok(id)
    }

    /// Everything that happened since the last call, oldest first.
    #[must_use]
    pub fn events(&self) -> Vec<Event> {
        self.inbox.drain()
    }

    /// Stops the node.  Post: nothing is woken after this returns.
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.inbox.close();
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(SHUTDOWN);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Condvar;

    /// A waker a test can wait on.
    #[derive(Default)]
    struct Bell {
        rung: Mutex<u64>,
        changed: Condvar,
    }

    impl Wake for Arc<Bell> {
        fn wake(&self) {
            *self.rung.lock().unwrap() += 1;
            self.changed.notify_all();
        }
    }

    struct Silent;

    impl Wake for Silent {
        fn wake(&self) {}
    }

    #[test]
    fn a_burst_wakes_once_until_drained() {
        let bell = Arc::new(Bell::default());
        let inbox = Inbox::new(Arc::clone(&bell));
        inbox.push(Event::Sent { id: 1 });
        inbox.push(Event::Sent { id: 2 });
        assert_eq!(*bell.rung.lock().unwrap(), 1);
        assert_eq!(
            inbox.drain(),
            vec![Event::Sent { id: 1 }, Event::Sent { id: 2 }]
        );
        inbox.push(Event::Sent { id: 3 });
        assert_eq!(*bell.rung.lock().unwrap(), 2);
    }

    #[test]
    fn frames_past_the_bound_are_counted_but_answers_are_kept() {
        let inbox = Inbox::new(Silent);
        let frame = || Event::Frame {
            from: "0x0".to_owned(),
            payload: Vec::new(),
        };
        for _ in 0..MAX_QUEUED + 3 {
            inbox.push(frame());
        }
        inbox.push(Event::Sent { id: 9 });
        let events = inbox.drain();
        assert_eq!(events.len(), MAX_QUEUED + 2);
        assert_eq!(events[MAX_QUEUED], Event::Sent { id: 9 });
        assert_eq!(events[MAX_QUEUED + 1], Event::Lost { frames: 3 });
        assert!(inbox.drain().is_empty());
    }

    #[test]
    fn an_account_round_trips_through_hex() {
        let account = Account::generate();
        let again = Account::from_hex(&account.to_hex()).unwrap();
        assert_eq!(account.did(), again.did());
        assert!(account.did().starts_with("0x"));
        assert!(matches!(Account::from_hex("zz"), Err(NodeError::Account)));
    }
}

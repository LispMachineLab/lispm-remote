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
//! them, and so is news of a renewal.
//!
//! A node runs on a session key its account delegates to, and a Rings
//! processor keeps that delegation for its life: from the delegation's
//! signing horizon `h = e − Δ` on, `e` its expiry and `Δ` Rings's delivery
//! bound, the processor sends nothing, and it stays mute
//! (RingsNetwork/rings#1063).  The `rings` daemon ends there for its
//! supervisor to restart it; an embedded node would go mute with Emacs
//! none the wiser.  So the node renews itself, [`Lifetime::lead`] before
//! the horizon, on one generation `g` after another:
//!
//! ```text
//!   renew_at(g) = h(d_g) − lead
//!   renew       : wait until renew_at(g) ; current ← ⊥ ; close(g) ;
//!                 await released(g) ; current ← g′ on a fresh d_g′ ; push Renewed
//! ```
//!
//! Each generation is a new processor of the same account, so the node's
//! DID stays; it opens the same replay records (`store`), so its message
//! sequences go on; and it pushes to the same inbox.  It starts connected
//! to no one: [`Event::Renewed`] tells Emacs to join the network again.
//! Work started while the node renews fails at once.

use std::collections::VecDeque;
use std::future::Future;
use std::path::Path;
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
use rings_core::consts::DEFAULT_DELEGATION_TTL_MS;
use rings_core::delegation::DelegateeKey;
use rings_core::delegation::DelegationBuilder;
use rings_core::dht::Did;
use rings_core::ecc::SecretKey;
use rings_core::lifecycle::StopSource;
use rings_core::storage::MemStorage;
use rings_core::utils::get_epoch_ms;
use rings_node::native::config::DEFAULT_NETWORK_ID;
use rings_node::native::horizon::HORIZON_POLL;
use rings_node::processor::DelegationHorizon;
use rings_node::processor::ProcessorBuilder;
use rings_node::processor::ProcessorConfig;
use rings_node::provider::Provider;
use rings_node::release::StoresReleased;
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use crate::bridge::Forward;
use crate::bridge::Shell;
use crate::store::Replay;

/// The namespace LispM Remote's frames travel under.
pub const NAMESPACE: &str = "lispm-remote/1";
/// The most events waiting for Emacs at once.
pub const MAX_QUEUED: usize = 4096;
/// How long stopping waits for the runtime's tasks.
const SHUTDOWN: Duration = Duration::from_secs(2);
/// The Chord stabilization interval, in seconds.
const STABILIZE_SECS: u64 = 3;
/// How long a renewal waits for the old generation to close, and then for
/// its replay records to be released.  Emacs does not wait on it.
const RENEWAL_CLOSE: Duration = Duration::from_secs(30);
/// How long after a renewal failed the node tries again.
const RENEWAL_RETRY: Duration = Duration::from_secs(60);

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
        /// The data of the JSON-RPC error a request failed with, as JSON,
        /// when Rings gave one: for a peer of another wire version,
        /// `{"ours": n, "theirs": m}`, `m` null for a peer from before wire
        /// versioning.
        data: Option<String>,
    },
    /// Frames were dropped because Emacs did not collect events in time.
    Lost {
        /// How many.
        frames: u64,
    },
    /// The node renewed its delegation: it runs on a new Rings processor,
    /// connected to no one until it joins the network again.
    Renewed,
    /// Renewing the node's delegation failed.  The node runs on no
    /// processor, so its work fails, and it tries again in a minute.
    RenewalFailed {
        /// Why, as Rings or the store put it.
        error: String,
    },
}

impl Event {
    /// The failure of the work `id` with `error`.  A request's error comes
    /// back from Rings's internal JSON-RPC handler, which keeps the data of
    /// the error the call failed with; its text alone would lose it.
    #[must_use]
    pub fn failed(id: u64, error: &rings_node::error::Error) -> Self {
        let data = match error {
            rings_node::error::Error::InternalRpcError(failure) => {
                failure.data.as_ref().map(ToString::to_string)
            }
            _ => None,
        };
        Self::Failed {
            id,
            error: error.to_string(),
            data,
        }
    }

    /// The failure of the work `id`, started while the node renews.
    fn renewing(id: u64) -> Self {
        Self::Failed {
            id,
            error: "the node is renewing its delegation".to_owned(),
            data: None,
        }
    }

    /// Whether the event is queued even past [`MAX_QUEUED`]: Emacs waits
    /// for the outcome of its own work, and must hear of a renewal.
    const fn kept(&self) -> bool {
        matches!(
            self,
            Self::Sent { .. }
                | Self::Reply { .. }
                | Self::Failed { .. }
                | Self::Renewed
                | Self::RenewalFailed { .. }
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
            if queue.len() < MAX_QUEUED || event.kept() {
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
    /// The node's store could not be prepared.
    #[error("the node's store could not be prepared: {0}")]
    Store(std::io::Error),
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
    /// A delegation of the [`Lifetime`] asked for would be renewed as soon
    /// as it starts, or its lifetime is past what Rings counts.
    #[error("the delegations' lifetime leaves them no time to run")]
    Lifetime,
}

/// How long the delegations a node runs on last, and how long before each
/// one's signing horizon the node renews it (module documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lifetime {
    /// How long each delegation lasts.
    pub ttl: Duration,
    /// How long before a delegation's signing horizon the node renews it.
    pub lead: Duration,
}

impl Lifetime {
    /// Rings's default lifetime of a delegation, 30 days, renewed a day
    /// before its horizon, as Rings starts warning that a node nears it
    /// (`SIGNING_HORIZON_LEAD_MS`).
    pub const DEFAULT: Self = Self {
        ttl: Duration::from_millis(DEFAULT_DELEGATION_TTL_MS),
        lead: Duration::from_secs(24 * 60 * 60),
    };
}

/// A secp256k1 account key: the node's identity on Rings.  A running node
/// keeps a copy, to delegate to each session key it renews.
#[derive(Clone)]
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

    /// A session key the account delegates to for `ttl`, as Rings nodes
    /// run on.
    fn delegate(&self, ttl: Duration) -> Result<DelegateeKey, NodeError> {
        let ttl = u64::try_from(ttl.as_millis()).map_err(|_| NodeError::Lifetime)?;
        let mut builder = DelegationBuilder::new(self.did(), "secp256k1".to_owned()).set_ttl(ttl);
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

/// One Rings processor of a node, on one delegation.
struct Generation {
    provider: Arc<Provider>,
    /// Stops its listener, before it closes its network.
    listening: StopSource,
    /// Resolves once its replay records are released.
    released: StoresReleased,
    /// When the node renews it, in milliseconds since the epoch: `lead`
    /// before its delegation's signing horizon.
    renew_at_ms: u128,
}

impl Generation {
    /// Stops the listener and closes the processor so that its tasks end
    /// and no socket stays bound (RingsNetwork/rings#1098, #1085), for at
    /// most `bound`; its peers see the connections close at once.  Returns
    /// the witness of its replay records' release.
    ///
    /// `close` takes a provider: this one, or a clone of it while work
    /// under way still holds it.  A clone stops the processor and closes
    /// its network all the same, and its stores are released once the
    /// work ends.
    async fn close(self, bound: Duration) -> StoresReleased {
        self.listening.request_stop();
        let provider = Arc::unwrap_or_clone(self.provider);
        // A close that fails or hangs still ends in a release, once the
        // last clone drops.
        let _ = tokio::time::timeout(bound, provider.close()).await;
        self.released
    }
}

/// The generation a node runs on, which the node and its renewal share;
/// `None` while it renews.
#[derive(Clone, Default)]
struct Current(Arc<Mutex<Option<Generation>>>);

impl Current {
    fn lock(&self) -> MutexGuard<'_, Option<Generation>> {
        // Each holder only takes, puts or reads the generation, which no
        // panic leaves half done, so poisoning is ignored.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn provider(&self) -> Option<Arc<Provider>> {
        self.lock()
            .as_ref()
            .map(|generation| Arc::clone(&generation.provider))
    }

    fn take(&self) -> Option<Generation> {
        self.lock().take()
    }

    fn put(&self, generation: Generation) {
        *self.lock() = Some(generation);
    }
}

/// What each generation of a node starts from.
struct Origin {
    account: Account,
    ice_servers: String,
    replay: Replay,
    lifetime: Lifetime,
    inbox: Arc<Inbox>,
}

impl Origin {
    /// A generation on a fresh delegation: a processor over the node's
    /// replay records, with LispM Remote's protocol and a listener.  It
    /// runs on the runtime it is started on.
    async fn start(&self) -> Result<Generation, NodeError> {
        let delegatee = self.account.delegate(self.lifetime.ttl)?;
        // The horizon of the delegation the processor will sign with, as
        // `Processor::signing_horizon` reads it.
        let horizon = DelegationHorizon::of(&delegatee.delegation());
        let renew_at_ms = horizon
            .horizon_ms()
            .saturating_sub(self.lifetime.lead.as_millis());
        if renew_at_ms <= get_epoch_ms() {
            return Err(NodeError::Lifetime);
        }
        let config = ProcessorConfig::new(
            DEFAULT_NETWORK_ID,
            self.ice_servers.clone(),
            delegatee,
            STABILIZE_SECS,
        );
        let (replay, released) = self.replay.open().await?;
        let shell = Shell::new(Arc::clone(&self.inbox));
        let rings = |error: rings_node::error::Error| NodeError::Rings(error.to_string());
        let processor = ProcessorBuilder::from_config(&config)
            .map_err(rings)?
            .storage(Box::new(MemStorage::new()))
            .replay_storage(replay)
            .stores_released(released.clone())
            .build()
            .map_err(rings)?;
        let provider = Arc::new(Provider::from_processor(Arc::new(processor)));
        provider.register_protocol(Forward, shell).map_err(rings)?;
        provider.set_backend().map_err(rings)?;
        let listener = Arc::clone(&provider);
        let listening = StopSource::new();
        let stop = listening.token();
        // The listener ends when the generation stops it.
        tokio::spawn(async move {
            let _ = listener.listen_with(stop).await;
        });
        Ok(Generation {
            provider,
            listening,
            released,
            renew_at_ms,
        })
    }

    /// Renews the node's generations in `current` for as long as the node
    /// runs, the first at `renew_at_ms` (module documentation).  A renewal
    /// that fails is tried again [`RENEWAL_RETRY`] later.
    async fn renew(self, current: Current, mut renew_at_ms: u128) {
        loop {
            until(renew_at_ms).await;
            if let Some(old) = current.take() {
                let released = old.close(RENEWAL_CLOSE).await;
                // Until then the replay records stay locked, and the start
                // below fails, to be tried again.
                let _ = tokio::time::timeout(RENEWAL_CLOSE, released).await;
            }
            match self.start().await {
                Ok(generation) => {
                    renew_at_ms = generation.renew_at_ms;
                    current.put(generation);
                    self.inbox.push(Event::Renewed);
                }
                Err(error) => {
                    renew_at_ms = get_epoch_ms().saturating_add(RENEWAL_RETRY.as_millis());
                    self.inbox.push(Event::RenewalFailed {
                        error: error.to_string(),
                    });
                }
            }
        }
    }
}

/// Waits until the epoch clock reads `at_ms`.  A sleep runs on the
/// monotonic clock, which stops while the host is suspended, so no sleep is
/// longer than [`HORIZON_POLL`] and each wake reads the epoch clock again:
/// a host suspended past `at_ms` gets there within a poll of resuming, as
/// Rings's daemon gets to its horizon (`rings_node::native::horizon`).
async fn until(at_ms: u128) {
    loop {
        let left = at_ms.saturating_sub(get_epoch_ms());
        if left == 0 {
            return;
        }
        let left = Duration::from_millis(u64::try_from(left).unwrap_or(u64::MAX));
        tokio::time::sleep(left.min(HORIZON_POLL)).await;
    }
}

/// A running Rings node.
pub struct Node {
    /// `None` only while the node is being dropped.
    runtime: Option<Runtime>,
    current: Current,
    /// The task that renews the node; `None` only while the node is being
    /// dropped.
    renewal: Option<JoinHandle<()>>,
    inbox: Arc<Inbox>,
    did: String,
    next: AtomicU64,
}

impl Node {
    /// A node for `account`, its ICE servers `ice_servers` (Rings's
    /// `stun://host:port` list), telling `waker` when events wait, on
    /// delegations of [`Lifetime::DEFAULT`].
    ///
    /// `store` is the directory the node keeps its replay records in.
    /// Rings refuses a sender whose sequences go back, so an account whose
    /// node restarts must find its records again there; without `store`
    /// they live in memory, kept across the node's renewals, for a node
    /// that Emacs never starts again.
    ///
    /// # Errors
    ///
    /// A [`NodeError`] if the runtime, the store or the Rings processor
    /// could not start.
    pub fn start(
        account: &Account,
        ice_servers: &str,
        store: Option<&Path>,
        waker: impl Wake,
    ) -> Result<Self, NodeError> {
        Self::start_lasting(account, ice_servers, store, Lifetime::DEFAULT, waker)
    }

    /// [`Node::start`], on delegations of `lifetime`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Lifetime`] if a delegation of `lifetime` would be
    /// renewed as soon as it starts; otherwise as [`Node::start`].
    pub fn start_lasting(
        account: &Account,
        ice_servers: &str,
        store: Option<&Path>,
        lifetime: Lifetime,
        waker: impl Wake,
    ) -> Result<Self, NodeError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("lispm-remote")
            .enable_all()
            .build()
            .map_err(NodeError::Runtime)?;
        let inbox = Arc::new(Inbox::new(waker));
        let origin = Origin {
            account: account.clone(),
            ice_servers: ice_servers.to_owned(),
            replay: Replay::new(store),
            lifetime,
            inbox: Arc::clone(&inbox),
        };
        let generation = runtime.block_on(origin.start())?;
        let renew_at_ms = generation.renew_at_ms;
        let current = Current::default();
        current.put(generation);
        let renewal = runtime.spawn(origin.renew(current.clone(), renew_at_ms));
        Ok(Self {
            runtime: Some(runtime),
            current,
            renewal: Some(renewal),
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
        let provider = self.current.provider();
        self.spawn(async move {
            let Some(provider) = provider else {
                return Event::renewing(id);
            };
            match provider.send(to, NAMESPACE, Bytes::from(payload)).await {
                Ok(()) => Event::Sent { id },
                Err(error) => Event::failed(id, &error),
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
        let provider = self.current.provider();
        self.spawn(async move {
            let Some(provider) = provider else {
                return Event::renewing(id);
            };
            match provider.request_internal(method, params).await {
                Ok(result) => Event::Reply {
                    id,
                    result: result.to_string(),
                },
                Err(error) => Event::failed(id, &error),
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
    /// Ends the renewal, closes the node's generation, then ends the
    /// runtime.
    ///
    /// ```text
    ///   drop = close(inbox) ; abort(renewal) ; close(current) ≤ SHUTDOWN ; shutdown(runtime) ≤ SHUTDOWN
    /// ```
    ///
    /// A renewal under way ends where it is, and the generation it held
    /// drops with it.  The shutdown drops the tasks' clones of the
    /// processor, the last ones, so the node's stores are released when the
    /// drop returns, for a node started again on them.
    fn drop(&mut self) {
        self.inbox.close();
        if let Some(runtime) = self.runtime.take() {
            if let Some(renewal) = self.renewal.take() {
                renewal.abort();
                let _ = runtime.block_on(renewal);
            }
            if let Some(generation) = self.current.take() {
                let _released = runtime.block_on(generation.close(SHUTDOWN));
            }
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

    const ICE: &str = "stun://stun.l.google.com:19302";

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
    fn a_stopped_node_lets_its_store_open_again() {
        // Rings locks a store while it is open (RingsNetwork/rings#1039),
        // and Emacs starts a node again in the same process.  The store
        // does not exist before the first start.
        let scratch =
            std::env::temp_dir().join(format!("lispm-remote-reopen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let store = scratch.join("store");
        let account = Account::generate();
        for _ in 0..2 {
            Node::start(&account, ICE, Some(&store), Silent)
                .unwrap()
                .stop();
        }
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_failure_keeps_the_data_of_its_json_rpc_error() {
        use rings_core::message::PeerWireVersion;
        use rings_core::message::WireVersionMismatch;
        use rings_node::error::Error;

        // A request refused as the node's JSON-RPC handler refuses it.
        let mismatch = Error::IncompatibleWireVersion(WireVersionMismatch {
            ours: 1,
            theirs: PeerWireVersion::BeforeVersioning,
        });
        let refusal = Error::InternalRpcError(mismatch.into());
        assert_eq!(
            Event::failed(3, &refusal),
            Event::Failed {
                id: 3,
                error: "Internal rpc services error: Server error: Incompatible wire version: ours is 1, the peer's is from before wire versioning..".to_owned(),
                data: Some(r#"{"ours":1,"theirs":null}"#.to_owned()),
            }
        );
        assert_eq!(
            Event::failed(4, &Error::ConnectionNotFound),
            Event::Failed {
                id: 4,
                error: "Connection not found.".to_owned(),
                data: None,
            }
        );
    }

    #[test]
    fn the_default_lifetime_is_rings_and_renews_when_rings_warns() {
        use rings_node::processor::SIGNING_HORIZON_LEAD_MS;

        assert_eq!(
            Lifetime::DEFAULT.ttl.as_millis(),
            u128::from(DEFAULT_DELEGATION_TTL_MS)
        );
        assert_eq!(Lifetime::DEFAULT.lead.as_millis(), SIGNING_HORIZON_LEAD_MS);
    }

    #[test]
    fn a_lifetime_without_time_to_run_is_refused() {
        // Rings stops signing 25 s before a delegation expires, and counts
        // its lifetime in milliseconds of a u64.
        let seconds = Duration::from_secs;
        for (ttl, lead) in [(20, 0), (60, 40), (u64::MAX, 0)] {
            let lifetime = Lifetime {
                ttl: seconds(ttl),
                lead: seconds(lead),
            };
            let node = Node::start_lasting(&Account::generate(), ICE, None, lifetime, Silent);
            assert!(matches!(node, Err(NodeError::Lifetime)), "{lifetime:?}");
        }
    }

    #[test]
    fn a_node_renews_over_its_store_again_and_again() {
        // A delegation of 30 s stops signing 5 s after it starts, and is
        // renewed 4 s before that.
        let lifetime = Lifetime {
            ttl: Duration::from_secs(30),
            lead: Duration::from_secs(4),
        };
        let scratch =
            std::env::temp_dir().join(format!("lispm-remote-renew-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let store = scratch.join("store");
        let account = Account::generate();
        let bell = Arc::new(Bell::default());
        let node =
            Node::start_lasting(&account, ICE, Some(&store), lifetime, Arc::clone(&bell)).unwrap();
        let mut seen = Vec::new();
        let mut wait_for = |node: &Node, pick: &dyn Fn(&Event) -> bool| {
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            loop {
                seen.extend(node.events());
                if let Some(index) = seen.iter().position(pick) {
                    return seen.remove(index);
                }
                assert!(std::time::Instant::now() < deadline, "saw {seen:?}");
                let rung = bell.rung.lock().unwrap();
                let _ = bell
                    .changed
                    .wait_timeout(rung, Duration::from_millis(100))
                    .unwrap();
            }
        };
        for _ in 0..2 {
            let event = wait_for(&node, &|event| {
                matches!(event, Event::Renewed | Event::RenewalFailed { .. })
            });
            assert_eq!(event, Event::Renewed);
        }
        // The node serves work on the generation it renewed to.
        let id = node.request("nodeInfo", "{}").unwrap();
        let event = wait_for(&node, &|event| match event {
            Event::Reply { id: got, .. } | Event::Failed { id: got, .. } => *got == id,
            _ => false,
        });
        assert!(matches!(event, Event::Reply { .. }), "{event:?}");
        node.stop();
        Node::start(&account, ICE, Some(&store), Silent)
            .unwrap()
            .stop();
        std::fs::remove_dir_all(&scratch).unwrap();
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

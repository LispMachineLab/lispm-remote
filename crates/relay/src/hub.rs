// lispm-remote: the surfaces, and what clients ask of Emacs.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The hub between Emacs and the clients.  Emacs gives it the windows
//! that can be opened and opens surfaces in it; clients' sessions read it
//! and leave requests for Emacs (open this window, close that surface),
//! which wake Emacs the way the node's events do: the first request after
//! a drain calls the [`Wake`]r.
//!
//! ```text
//!   changes  grows whenever the window list or the set of surfaces does
//!   notices  the clipboard and bells, numbered, the last MAX_NOTICES kept
//! ```

use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use lispm_remote_display::CellPixels;
use lispm_remote_display::ServerMessage;
use lispm_remote_display::Size;
use lispm_remote_display::SurfaceId;
use lispm_remote_display::Window;

use crate::pool::Pool;
use crate::surface::Surface;

/// The most windows a client is offered.
pub const MAX_WINDOWS: usize = 500;
/// How many notices are kept for sessions to catch up on.
const MAX_NOTICES: usize = 64;

/// Something to call when requests start waiting for Emacs.
pub trait Wake: Send + Sync + 'static {
    /// Tell Emacs that [`Hub::requests`] has something.
    fn wake(&self);
}

/// What a client asks of Emacs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Open `window` on a new surface.
    Open {
        /// The window, by the name Emacs gave it.
        window: String,
        /// The surface's size.
        size: Size,
        /// The client's cell size.
        cell: CellPixels,
    },
    /// Close the surface (delete its frame).
    Close {
        /// The surface.
        surface: SurfaceId,
    },
    /// The number of connected clients changed.
    Clients(usize),
}

/// Bells and the clipboard, numbered for sessions to follow.
#[derive(Default)]
pub struct Notices {
    queue: Mutex<VecDeque<(u64, ServerMessage)>>,
    next: AtomicU64,
}

impl Notices {
    fn push(&self, message: ServerMessage) {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        let number = self.next.fetch_add(1, Ordering::AcqRel) + 1;
        queue.push_back((number, message));
        while queue.len() > MAX_NOTICES {
            queue.pop_front();
        }
    }

    /// Emacs put `text` on the clipboard.
    pub fn clipboard(&self, text: String) {
        self.push(ServerMessage::Clipboard(text));
    }

    /// `surface` rang its bell.
    pub fn bell(&self, surface: SurfaceId) {
        self.push(ServerMessage::Bell { surface });
    }

    /// The notices numbered after `seen`, with the last number.
    pub(crate) fn since(&self, seen: u64) -> (u64, Vec<ServerMessage>) {
        let queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        let fresh = queue
            .iter()
            .filter(|(number, _)| *number > seen)
            .map(|(_, message)| message.clone())
            .collect();
        (self.next.load(Ordering::Acquire), fresh)
    }
}

/// The hub.
pub struct Hub {
    surfaces: Mutex<BTreeMap<SurfaceId, Arc<Surface>>>,
    next_surface: AtomicU32,
    windows: Mutex<Vec<Window>>,
    changes: AtomicU64,
    pub(crate) pool: Arc<Pool>,
    pub(crate) notices: Arc<Notices>,
    requests: Mutex<VecDeque<Request>>,
    pending: AtomicBool,
    clients: AtomicUsize,
    waker: Box<dyn Wake>,
}

impl Hub {
    /// A hub with no surface, calling `waker` when requests wait.
    #[must_use]
    pub fn new(waker: impl Wake) -> Self {
        Self {
            surfaces: Mutex::new(BTreeMap::new()),
            next_surface: AtomicU32::new(0),
            windows: Mutex::new(Vec::new()),
            changes: AtomicU64::new(1),
            pool: Arc::new(Pool::default()),
            notices: Arc::new(Notices::default()),
            requests: Mutex::new(VecDeque::new()),
            pending: AtomicBool::new(false),
            clients: AtomicUsize::new(0),
            waker: Box::new(waker),
        }
    }

    fn surfaces(&self) -> MutexGuard<'_, BTreeMap<SurfaceId, Arc<Surface>>> {
        self.surfaces.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Opens a surface for `window`; Emacs then makes a frame on its tty.
    ///
    /// # Errors
    ///
    /// The system's, when no pseudo-terminal can be had.
    pub fn open(&self, window: String, size: Size, cell: CellPixels) -> io::Result<Arc<Surface>> {
        let id = SurfaceId::new(self.next_surface.fetch_add(1, Ordering::AcqRel) + 1);
        let surface = Surface::open(
            id,
            window,
            size,
            cell,
            Arc::clone(&self.pool),
            Arc::clone(&self.notices),
        )?;
        self.surfaces().insert(id, Arc::clone(&surface));
        self.changed();
        Ok(surface)
    }

    /// Closes the surface `id`, after Emacs deleted its frame.
    pub fn close(&self, id: SurfaceId) {
        let surface = self.surfaces().remove(&id);
        if let Some(surface) = surface {
            surface.close();
            surface.join();
        }
        self.changed();
    }

    /// Closes every surface.
    pub fn close_all(&self) {
        let ids: Vec<SurfaceId> = self.surfaces().keys().copied().collect();
        for id in ids {
            self.close(id);
        }
    }

    /// The surface `id`, if open.
    #[must_use]
    pub fn surface(&self, id: SurfaceId) -> Option<Arc<Surface>> {
        self.surfaces().get(&id).map(Arc::clone)
    }

    /// Every open surface, by id.
    #[must_use]
    pub fn all_surfaces(&self) -> Vec<Arc<Surface>> {
        self.surfaces().values().map(Arc::clone).collect()
    }

    /// Sets the windows clients may open (at most [`MAX_WINDOWS`]).
    pub fn set_windows(&self, mut windows: Vec<Window>) {
        windows.truncate(MAX_WINDOWS);
        let mut current = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        if *current != windows {
            *current = windows;
            drop(current);
            self.changed();
        }
    }

    /// The windows clients may open.
    #[must_use]
    pub fn windows(&self) -> Vec<Window> {
        self.windows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// A number that grows whenever the windows or surfaces change.
    #[must_use]
    pub fn changes(&self) -> u64 {
        self.changes.load(Ordering::Acquire)
    }

    fn changed(&self) {
        self.changes.fetch_add(1, Ordering::AcqRel);
    }

    /// Leaves `request` for Emacs, waking it if nothing was waiting.
    pub(crate) fn request(&self, request: Request) {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(request);
        if !self.pending.swap(true, Ordering::AcqRel) {
            self.waker.wake();
        }
    }

    /// The requests waiting for Emacs, oldest first.
    pub fn requests(&self) -> Vec<Request> {
        self.pending.store(false, Ordering::Release);
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect()
    }

    pub(crate) fn client_joined(&self) {
        let count = self.clients.fetch_add(1, Ordering::AcqRel) + 1;
        self.request(Request::Clients(count));
    }

    pub(crate) fn client_left(&self) {
        let count = self
            .clients
            .fetch_sub(1, Ordering::AcqRel)
            .saturating_sub(1);
        self.request(Request::Clients(count));
    }
}

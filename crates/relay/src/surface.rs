// lispm-remote: one LispM window, as a pseudo-terminal Emacs draws on.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A surface: a pseudo-terminal, the terminal reading it, and a thread
//! that keeps reading.  The thread must keep the master drained: Emacs
//! writes a redisplay to its tty synchronously, so a full pseudo-terminal
//! would stop Emacs.  It never waits on a client, only on the master and
//! the terminal's lock, and what it keeps is the screen state, whose size
//! does not grow with the output.
//!
//! ```text
//!   loop: bytes ← read(master) ; terminal.feed(bytes)
//!         write(master, replies) ; pool ← images ; notices ← bell, clipboard
//!         version += 1   unless Emacs holds the change (mode 2026), for at most HOLD
//! ```

use std::fs::File;
use std::io;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

use lispm_remote_display::CellPixels;
use lispm_remote_display::Input;
use lispm_remote_display::Screen;
use lispm_remote_display::Size;
use lispm_remote_display::SurfaceId;

use crate::hub::Notices;
use crate::keys;
use crate::pool::Pool;
use crate::pty;
use crate::pty::Pty;
use crate::terminal::Event;
use crate::terminal::Options;
use crate::terminal::Terminal;

/// How long a synchronized update is held before it is shown anyway.
const HOLD: Duration = Duration::from_millis(200);
/// How long the reader waits for bytes before looking at its flags.
const POLL: Duration = Duration::from_millis(50);

/// One LispM window shown on clients.
pub struct Surface {
    id: SurfaceId,
    window: String,
    pty: Pty,
    writer: Mutex<File>,
    terminal: Mutex<Terminal>,
    version: AtomicU64,
    closed: AtomicBool,
    reader: Mutex<Option<JoinHandle<()>>>,
}

impl Surface {
    /// Opens a surface for `window` and starts its reader.
    ///
    /// # Errors
    ///
    /// The system's, when no pseudo-terminal or thread can be had.
    pub(crate) fn open(
        id: SurfaceId,
        window: String,
        size: Size,
        cell: CellPixels,
        pool: Arc<Pool>,
        notices: Arc<Notices>,
    ) -> io::Result<Arc<Self>> {
        let pty = Pty::open(size, cell)?;
        let writer = Mutex::new(pty.master()?);
        let options = Options {
            cell,
            ..Options::default()
        };
        let surface = Arc::new(Self {
            id,
            window,
            pty,
            writer,
            terminal: Mutex::new(Terminal::new(size, options)),
            version: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            reader: Mutex::new(None),
        });
        let reading = Arc::clone(&surface);
        let master = surface.pty.master()?;
        let thread = std::thread::Builder::new()
            .name(format!("lispm-remote-surface-{}", id.get()))
            .spawn(move || reading.read(master, &pool, &notices))?;
        *surface
            .reader
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(thread);
        Ok(surface)
    }

    /// The surface's id.
    #[must_use]
    pub const fn id(&self) -> SurfaceId {
        self.id
    }

    /// The window it shows.
    #[must_use]
    pub fn window(&self) -> &str {
        &self.window
    }

    /// The slave's path, for Emacs's `make-frame`.
    #[must_use]
    pub fn tty(&self) -> &Path {
        self.pty.name()
    }

    /// A number that grows whenever the screen changes.
    #[must_use]
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// Whether the surface is closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn terminal(&self) -> MutexGuard<'_, Terminal> {
        // A panic inside `feed` cannot leave a half-changed grid behind
        // that is worse than a stale one; poisoning is ignored.
        self.terminal.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The screen as it stands, with its version.
    #[must_use]
    pub fn snapshot(&self) -> (u64, Screen) {
        let terminal = self.terminal();
        (self.version(), terminal.screen().clone())
    }

    /// Sends what a person did to Emacs, as the terminal's input.
    ///
    /// # Errors
    ///
    /// The system's, when the master cannot be written.
    pub fn input(&self, input: &Input) -> io::Result<()> {
        let modes = *self.terminal().modes();
        let bytes = keys::encode(input, &modes);
        if bytes.is_empty() {
            return Ok(());
        }
        self.writer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write_all(&bytes)
    }

    /// Gives the surface a new size and tells Emacs.
    ///
    /// # Errors
    ///
    /// The system's.
    pub fn resize(&self, size: Size, cell: CellPixels) -> io::Result<()> {
        {
            let mut terminal = self.terminal();
            if terminal.screen().size() == size && terminal.options().cell == cell {
                return Ok(());
            }
            terminal.resize(size);
            let options = Options {
                cell,
                ..*terminal.options()
            };
            terminal.set_options(options);
        }
        self.pty.set_size(size, cell)?;
        self.version.fetch_add(1, Ordering::AcqRel);
        pty::tell_emacs_of_new_sizes()
    }

    /// Closes the surface: its reader stops within [`POLL`].
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.version.fetch_add(1, Ordering::AcqRel);
    }

    /// Waits for the reader to stop.  Pre: [`Surface::close`] was called.
    pub fn join(&self) {
        let thread = self
            .reader
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }

    fn read(&self, mut master: File, pool: &Pool, notices: &Notices) {
        let mut buffer = vec![0_u8; 64 * 1024];
        let mut held: Option<Instant> = None;
        while !self.is_closed() {
            match pty::readable(&master, POLL) {
                Ok(true) => {}
                Ok(false) => {
                    if held.is_some_and(|since| since.elapsed() > HOLD) {
                        held = None;
                        self.version.fetch_add(1, Ordering::AcqRel);
                    }
                    continue;
                }
                Err(_) => break,
            }
            let n = match master.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let Some(bytes) = buffer.get(..n) else { break };
            let (replies, events, holding) = {
                let mut terminal = self.terminal();
                terminal.feed(bytes);
                (
                    terminal.take_replies(),
                    terminal.take_events(),
                    terminal.modes().synchronized,
                )
            };
            if !replies.is_empty() {
                let _ = self
                    .writer
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .write_all(&replies);
            }
            for event in events {
                match event {
                    Event::Resource(id, bytes) => pool.insert(id, bytes),
                    Event::Clipboard(text) => notices.clipboard(text),
                    Event::Bell => notices.bell(self.id),
                }
            }
            match (holding, held) {
                (true, None) => held = Some(Instant::now()),
                (true, Some(since)) if since.elapsed() <= HOLD => {}
                _ => {
                    held = None;
                    self.version.fetch_add(1, Ordering::AcqRel);
                }
            }
        }
        self.close();
    }
}

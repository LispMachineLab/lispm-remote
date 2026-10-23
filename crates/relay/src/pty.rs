// lispm-remote: pseudo-terminals.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A pseudo-terminal pair: the relay holds the master, Emacs opens the
//! slave by name for a tty frame, exactly as it opens the terminal an
//! `emacsclient -t` names.  The relay also keeps the slave open itself,
//! so the pair stays up until the relay closes it.
//!
//! Resizing sets the slave's window size and sends `SIGWINCH` to this
//! process, which is Emacs: its handler reads the size of every tty it
//! has open, as when `emacsclient` forwards the signal.

use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::time::Duration;

use lispm_remote_display::CellPixels;
use lispm_remote_display::Size;
use rustix::event::PollFd;
use rustix::event::PollFlags;
use rustix::event::Timespec;
use rustix::fs::Mode;
use rustix::fs::OFlags;
use rustix::pty::OpenptFlags;
use rustix::termios::Winsize;

/// A pseudo-terminal pair.
pub struct Pty {
    master: File,
    slave: File,
    name: PathBuf,
}

impl Pty {
    /// A new pair, the slave's window `size` with cells of `cell` pixels.
    ///
    /// # Errors
    ///
    /// The system's, when it has no pseudo-terminal to give.
    pub fn open(size: Size, cell: CellPixels) -> io::Result<Self> {
        let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
        // Not every system takes O_CLOEXEC here; Emacs's subprocesses
        // must not inherit the master either way.
        rustix::io::fcntl_setfd(&master, rustix::io::FdFlags::CLOEXEC)?;
        rustix::pty::grantpt(&master)?;
        rustix::pty::unlockpt(&master)?;
        let name = rustix::pty::ptsname(&master, Vec::new())?;
        let name = PathBuf::from(OsStr::from_bytes(name.as_bytes()));
        let slave = rustix::fs::open(
            &name,
            OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let pty = Self {
            master: File::from(master),
            slave: File::from(slave),
            name,
        };
        pty.set_size(size, cell)?;
        Ok(pty)
    }

    /// The slave's path, for `(make-frame '((tty . PATH) …))`.
    #[must_use]
    pub const fn name(&self) -> &PathBuf {
        &self.name
    }

    /// The master, to read what Emacs writes and write what it reads.
    ///
    /// # Errors
    ///
    /// The system's, when the descriptor cannot be duplicated.
    pub fn master(&self) -> io::Result<File> {
        self.master.try_clone()
    }

    /// Sets the window size Emacs reads, without telling it.
    ///
    /// # Errors
    ///
    /// The system's.
    pub fn set_size(&self, size: Size, cell: CellPixels) -> io::Result<()> {
        let pixels = |cells: u16, cell: u16| cells.saturating_mul(cell);
        rustix::termios::tcsetwinsize(
            &self.slave,
            Winsize {
                ws_row: size.rows(),
                ws_col: size.cols(),
                ws_xpixel: pixels(size.cols(), cell.width),
                ws_ypixel: pixels(size.rows(), cell.height),
            },
        )?;
        Ok(())
    }
}

/// Tells Emacs (this process) that a terminal's size changed.
///
/// # Errors
///
/// The system's.
pub fn tell_emacs_of_new_sizes() -> io::Result<()> {
    rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::WINCH)?;
    Ok(())
}

/// Whether `file` has bytes to read within `timeout`.
///
/// # Errors
///
/// The system's.
pub fn readable(file: &File, timeout: Duration) -> io::Result<bool> {
    let timeout = Timespec {
        tv_sec: i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX),
        tv_nsec: i64::from(timeout.subsec_nanos()),
    };
    let mut fds = [PollFd::new(file, PollFlags::IN)];
    match rustix::event::poll(&mut fds, Some(&timeout)) {
        Ok(ready) => Ok(ready > 0),
        Err(rustix::io::Errno::INTR) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::io::Write;

    use super::*;

    #[test]
    fn what_the_slave_writes_the_master_reads() {
        let size = Size::new(80, 24).unwrap();
        let pty = Pty::open(
            size,
            CellPixels {
                width: 8,
                height: 16,
            },
        )
        .unwrap();
        assert!(pty.name().to_string_lossy().starts_with("/dev/"));
        let slave =
            rustix::fs::open(pty.name(), OFlags::RDWR | OFlags::NOCTTY, Mode::empty()).unwrap();
        let mut slave = File::from(slave);
        // The slave is in cooked mode, so a newline arrives as CR LF.
        slave.write_all(b"hi\n").unwrap();
        let mut master = pty.master().unwrap();
        // Linux may hand the line over in pieces: "hi", then CR LF.
        let mut read = Vec::new();
        while read.len() < 4 {
            assert!(readable(&master, Duration::from_secs(5)).unwrap());
            let mut buffer = [0; 16];
            let n = master.read(&mut buffer).unwrap();
            assert!(n > 0, "the master closed after {read:?}");
            read.extend_from_slice(&buffer[..n]);
        }
        assert_eq!(read, b"hi\r\n");
    }
}

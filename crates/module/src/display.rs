// lispm-remote: the display, as Lisp sees it.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The display's Lisp functions, named `lispm-remote-core-display-…`.
//! A display (a hub of surfaces and the server clients reach it by)
//! lives in a `user-ptr`.  Clients' requests wake Emacs through a pipe
//! process, as the node's events do, and `display-requests` collects
//! them:
//!
//! ```text
//!   (open WINDOW COLS ROWS CELL-WIDTH CELL-HEIGHT)   a client wants WINDOW
//!   (close ID)                                       a client closed surface ID
//!   (clients COUNT)                                  COUNT clients are connected
//! ```
//!
//! Emacs answers an `open` with `display-open`, which gives the tty to
//! make the frame on.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use emacs::Env;
use emacs::IntoLisp;
use emacs::Result;
use emacs::Value;
use emacs::Vector;
use emacs::defun;
use lispm_remote_display::CellPixels;
use lispm_remote_display::Size;
use lispm_remote_display::SurfaceId;
use lispm_remote_display::Window;
use lispm_remote_relay::Hub;
use lispm_remote_relay::Request;
use lispm_remote_relay::Server;
use lispm_remote_relay::Wake;

/// Wakes Emacs by writing to the pipe `open-channel` gave.
struct Pipe(Mutex<Box<dyn Write + Send>>);

impl Wake for Pipe {
    fn wake(&self) {
        if let Ok(mut writer) = self.0.lock() {
            // A failed write means Emacs is gone, and nobody waits.
            let _ = writer.write_all(b".");
        }
    }
}

/// A display: its hub, and its server until stopped.
struct Display {
    hub: Arc<Hub>,
    server: Option<Server>,
}

impl Display {
    fn server(&self) -> Result<&Server> {
        self.server
            .as_ref()
            .ok_or_else(|| emacs::Error::msg("the display is stopped"))
    }
}

fn size(cols: i64, rows: i64) -> Result<Size> {
    Ok(Size::new(u16::try_from(cols)?, u16::try_from(rows)?)?)
}

fn cell(width: i64, height: i64) -> Result<CellPixels> {
    Ok(CellPixels {
        width: u16::try_from(width)?,
        height: u16::try_from(height)?,
    })
}

/// Start a display serving clients on ADDRESS, such as "127.0.0.1:0",
/// with the client page from the directory WEB.  PROCESS is a pipe
/// process; the module writes to it when clients' requests wait.
#[defun(user_ptr)]
fn display_start(env: &Env, address: String, web: String, process: Value) -> Result<Display> {
    let writer = env.open_channel(process)?;
    let hub = Arc::new(Hub::new(Pipe(Mutex::new(Box::new(writer)))));
    let server = Server::start(Arc::clone(&hub), &address, PathBuf::from(web))?;
    Ok(Display {
        hub,
        server: Some(server),
    })
}

/// Stop DISPLAY: its server, then every surface.  Delete the surfaces'
/// frames first.
#[defun]
fn display_stop(display: &mut Display) -> Result<()> {
    if let Some(mut server) = display.server.take() {
        server.stop();
    }
    display.hub.close_all();
    Ok(())
}

/// Whether DISPLAY is serving.
#[defun]
fn display_live_p(display: &Display) -> Result<bool> {
    Ok(display.server.is_some())
}

/// The URL of DISPLAY's page, its token included.
#[defun]
fn display_url(display: &Display) -> Result<String> {
    Ok(display.server()?.url())
}

/// Open a surface of COLS × ROWS cells for WINDOW on DISPLAY, a client's
/// cells being CELL-WIDTH × CELL-HEIGHT pixels.  Return (ID . TTY): make
/// a frame on TTY and close ID with `lispm-remote-core-display-close'
/// once the frame is deleted.
#[defun]
fn display_open<'e>(
    env: &'e Env,
    display: &Display,
    window: String,
    cols: i64,
    rows: i64,
    cell_width: i64,
    cell_height: i64,
) -> Result<Value<'e>> {
    let surface = display
        .hub
        .open(window, size(cols, rows)?, cell(cell_width, cell_height)?)?;
    let tty = surface.tty().to_string_lossy().into_owned();
    env.cons(i64::from(surface.id().get()), tty)
}

/// Close the surface ID of DISPLAY.
#[defun]
fn display_close(display: &Display, id: i64) -> Result<()> {
    display.hub.close(SurfaceId::new(u32::try_from(id)?));
    Ok(())
}

/// Offer WINDOWS on DISPLAY: a vector of names and titles, alternating.
#[defun]
fn display_windows(display: &Display, windows: Vector) -> Result<()> {
    let mut offered = Vec::new();
    let mut index = 0;
    while index + 1 < windows.len() {
        offered.push(Window {
            name: windows.get(index)?,
            title: windows.get(index + 1)?,
        });
        index += 2;
    }
    display.hub.set_windows(offered);
    Ok(())
}

/// The requests clients left on DISPLAY since the last call, oldest
/// first.
#[defun]
fn display_requests<'e>(env: &'e Env, display: &Display) -> Result<Value<'e>> {
    let requests = display
        .hub
        .requests()
        .into_iter()
        .map(|request| request_value(env, request))
        .collect::<Result<Vec<_>>>()?;
    env.list(&requests)
}

fn request_value(env: &Env, request: Request) -> Result<Value<'_>> {
    match request {
        Request::Open { window, size, cell } => env.list((
            env.intern("open")?,
            window,
            i64::from(size.cols()),
            i64::from(size.rows()),
            i64::from(cell.width),
            i64::from(cell.height),
        )),
        Request::Close { surface } => env.list((env.intern("close")?, i64::from(surface.get()))),
        Request::Clients(count) => env.list((env.intern("clients")?, i64::try_from(count)?)),
    }
}

/// The text surface ID of DISPLAY shows, one line per row.
#[defun]
fn display_text<'e>(env: &'e Env, display: &Display, id: i64) -> Result<Value<'e>> {
    match display.hub.surface(SurfaceId::new(u32::try_from(id)?)) {
        Some(surface) => surface.snapshot().1.text().into_lisp(env),
        None => ().into_lisp(env),
    }
}

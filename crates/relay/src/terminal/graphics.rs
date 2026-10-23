// lispm-remote: Kitty graphics, as references into the client's pool.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the terminal does with Kitty graphics commands.  An image Emacs
//! transmits is hashed, and from then on stands as its [`ResourceId`]: a
//! placement on the screen names it, and the bytes go to the relay's
//! store (as an [`Event::Resource`]) for clients that lack them.
//!
//! ```text
//!   transmit(i, png)  : images[i] ← (id png, size png) ; emit Resource(id, png)
//!   put(i, p) at (c, r): placements[(i, p)] ← (images[i].id, cells, crop, z)
//!   delete(i[, p])    : placements without (i[, p]) ; `I` also forgets images[i]
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use lispm_remote_display::CellRect;
use lispm_remote_display::PixelRect;
use lispm_remote_display::Placement;
use lispm_remote_display::PlacementKey;
use lispm_remote_display::ResourceId;

use super::Event;
use super::State;
use crate::kitty::Action;
use crate::kitty::Command;
use crate::kitty::png_size;

/// The most images a terminal keeps.
const MAX_IMAGES: usize = 4096;
/// The longest image accepted, in base64.
const MAX_DATA: usize = 16 * 1024 * 1024;
/// Kitty's number for PNG.
const PNG: u32 = 100;

/// An image the terminal holds.
///
/// Law: `Copy` is sound because the image is named, not held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Image {
    resource: ResourceId,
    width: u32,
    height: u32,
}

/// The images, by Kitty number, and the transmission under way.
#[derive(Debug, Default)]
pub(super) struct Images {
    images: HashMap<u32, Image>,
    transfer: Option<(Command, Vec<u8>)>,
}

impl State {
    /// Acts on the APC `payload`, if it is a graphics command.
    pub(super) fn graphics(&mut self, payload: &[u8]) {
        let Some(command) = Command::parse(payload) else {
            return;
        };
        let open = self.images.transfer.as_ref().map(|(first, _)| first.image);
        if let Some(image) = open.filter(|&image| command.continues(image)) {
            self.continue_transfer(image, &command);
            return;
        }
        match command.action {
            Action::Transmit | Action::TransmitAndPut => self.start_transfer(command),
            Action::Put => self.put(&command),
            Action::Delete => self.delete(&command),
            Action::Query => self.answer(&command, "OK"),
            Action::Other(_) => {}
        }
    }

    fn start_transfer(&mut self, command: Command) {
        if command.format != PNG || command.medium != b'd' {
            self.answer(&command, "ENOTSUP:only direct PNG");
            return;
        }
        let data = command.data.clone();
        if command.more {
            self.images.transfer = Some((command, data));
        } else {
            self.finish_transfer(&command, &data);
        }
    }

    fn continue_transfer(&mut self, image: u32, chunk: &Command) {
        let Some((first, mut data)) = self.images.transfer.take() else {
            return;
        };
        if data.len().saturating_add(chunk.data.len()) > MAX_DATA {
            self.answer(&first, "EFBIG:too large");
            return;
        }
        data.extend_from_slice(&chunk.data);
        if chunk.more {
            self.images.transfer = Some((first, data));
        } else if first.image == image {
            self.finish_transfer(&first, &data);
        }
    }

    fn finish_transfer(&mut self, command: &Command, data: &[u8]) {
        let Some(bytes) = STANDARD.decode(data).ok() else {
            self.answer(command, "EINVAL:bad base64");
            return;
        };
        let Some((width, height)) = png_size(&bytes) else {
            self.answer(command, "EBADPNG:not a PNG");
            return;
        };
        if self.images.images.len() >= MAX_IMAGES
            && !self.images.images.contains_key(&command.image)
        {
            self.answer(command, "ENOSPC:too many images");
            return;
        }
        let resource = ResourceId::of(&bytes);
        self.images.images.insert(
            command.image,
            Image {
                resource,
                width,
                height,
            },
        );
        self.events
            .push(Event::Resource(resource, Arc::from(bytes)));
        if command.action == Action::TransmitAndPut {
            self.put(command);
        } else {
            self.answer(command, "OK");
        }
    }

    /// Places an image at the cursor.
    fn put(&mut self, command: &Command) {
        let Some(&image) = self.images.images.get(&command.image) else {
            self.answer(command, "ENOENT:no such image");
            return;
        };
        let crop = cropped(command, image);
        let (width, height) = crop.map_or((image.width, image.height), |crop| (crop.w, crop.h));
        let cell = self.options.cell;
        let cols = if command.cols == 0 {
            cells_for(
                width.saturating_add(u32::from(command.offset_x)),
                cell.width,
            )
        } else {
            command.cols
        };
        let rows = if command.rows == 0 {
            cells_for(
                height.saturating_add(u32::from(command.offset_y)),
                cell.height,
            )
        } else {
            command.rows
        };
        let cursor = self.screen.cursor;
        let key = PlacementKey {
            image: command.image,
            placement: command.placement,
        };
        self.screen
            .placements
            .retain(|placement| placement.key != key);
        self.screen.placements.push(Placement {
            key,
            resource: image.resource,
            cells: CellRect {
                col: cursor.col,
                row: cursor.row,
                cols,
                rows,
            },
            crop,
            offset_x: command.offset_x,
            offset_y: command.offset_y,
            z: command.z,
        });
        if !command.cursor_stays {
            self.move_by(i32::from(cols), i32::from(rows.saturating_sub(1)));
        }
        self.answer(command, "OK");
    }

    fn delete(&mut self, command: &Command) {
        let placements = &mut self.screen.placements;
        match command.delete {
            b'a' => placements.clear(),
            b'A' => {
                placements.clear();
                self.images.images.clear();
            }
            b'i' | b'I' => {
                placements.retain(|placement| {
                    placement.key.image != command.image
                        || (command.placement != 0 && placement.key.placement != command.placement)
                });
                if command.delete == b'I' {
                    self.images.images.remove(&command.image);
                }
            }
            _ => {}
        }
    }

    /// Answers `command` with `message`, unless it asked for quiet.
    fn answer(&mut self, command: &Command, message: &str) {
        let ok = message == "OK";
        let silenced = command.quiet >= 2 || (ok && command.quiet >= 1);
        if !silenced && command.image != 0 {
            let answer = format!("\x1b_Gi={};{message}\x1b\\", command.image);
            self.reply(answer.as_bytes());
        }
    }
}

/// The part of `image` the command shows, or `None` for all of it.
fn cropped(command: &Command, image: Image) -> Option<PixelRect> {
    let whole = command.x == 0 && command.y == 0 && command.w == 0 && command.h == 0;
    (!whole).then(|| {
        let x = command.x.min(image.width);
        let y = command.y.min(image.height);
        let w = if command.w == 0 {
            image.width - x
        } else {
            command.w
        };
        let h = if command.h == 0 {
            image.height - y
        } else {
            command.h
        };
        PixelRect { x, y, w, h }
    })
}

/// How many cells of `cell` pixels `pixels` pixels need.
fn cells_for(pixels: u32, cell: u16) -> u16 {
    let cell = u32::from(cell.max(1));
    u16::try_from(pixels.div_ceil(cell).max(1)).unwrap_or(u16::MAX)
}

// lispm-remote: what the client knows.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The client's state and how each message from LispM changes it, as a
//! pure function: the page performs the effects (sends, redraws, images).
//! Input is numbered and kept until LispM acknowledges it, and the page
//! sends again what waited too long, since a channel may lose it.
//!
//! ```text
//!   receive : Model × ServerMessage → Model × [Effect]
//!   Frame(s, base, seq, u):  the replica of s takes it (see `Replica`):
//!                            applied → Ack s seq ; stale → nothing ; unbased → Resync s
//!   Resource chunks:         assembled, then kept only when their hash is the id
//! ```

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::VecDeque;

use lispm_remote_display::CellPixels;
use lispm_remote_display::ClientMessage;
use lispm_remote_display::Input;
use lispm_remote_display::Received;
use lispm_remote_display::Replica;
use lispm_remote_display::ResourceId;
use lispm_remote_display::Screen;
use lispm_remote_display::Seq;
use lispm_remote_display::ServerMessage;
use lispm_remote_display::Size;
use lispm_remote_display::SurfaceId;
use lispm_remote_display::Update;
use lispm_remote_display::Window;

/// The longest resource assembled.
const MAX_RESOURCE: usize = 16 * 1024 * 1024;
/// How long, in milliseconds, input waits for LispM's acknowledgement
/// before it is sent again.
pub const INPUT_RESEND_MS: u64 = 300;
/// How long, in milliseconds, input is gathered before it is sent, so a
/// burst of keys travels in few messages.
pub const INPUT_GATHER_MS: u64 = 30;
/// The most unacknowledged inputs kept.
const MAX_PENDING: usize = 512;

/// The input LispM has not acknowledged, oldest first.
///
/// Invariant: `pending[i]` is input number `first + i`, and
/// `first + pending.len()` is the next number.
#[derive(Debug)]
struct Inputs {
    first: u64,
    pending: VecDeque<(SurfaceId, Input)>,
    unsent: bool,
    sent_at: Option<u64>,
}

impl Default for Inputs {
    fn default() -> Self {
        Self::from(1)
    }
}

impl Inputs {
    /// No input yet, the next numbered `first`.
    fn from(first: u64) -> Self {
        Self {
            first,
            pending: VecDeque::new(),
            unsent: false,
            sent_at: None,
        }
    }

    /// The next input's number.
    fn next(&self) -> u64 {
        let pending = u64::try_from(self.pending.len()).unwrap_or(u64::MAX);
        self.first.saturating_add(pending)
    }
}

/// The client's copy of one surface.
#[derive(Debug)]
pub struct Copy {
    /// The window it shows.
    pub window: String,
    replica: Replica,
    blank: Screen,
}

impl Copy {
    /// The screen to draw: the newest state, or a blank one of the
    /// surface's size until the first frame.
    #[must_use]
    pub fn screen(&self) -> &Screen {
        self.replica.screen().unwrap_or(&self.blank)
    }
}

/// What the page should do after a message.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    /// Send this to LispM.
    Send(ClientMessage),
    /// Draw the surface again.
    Redraw(SurfaceId),
    /// The windows offered changed.
    Windows,
    /// Surfaces opened or closed.
    Surfaces,
    /// Show this surface: the window this client asked for opened.
    Show(SurfaceId),
    /// Fit this surface, now shown, to this client.
    Fit(SurfaceId),
    /// Put text on the clipboard.
    Clipboard(String),
    /// Ring the surface's bell.
    Bell(SurfaceId),
    /// A resource arrived whole, its hash checked.
    Resource(ResourceId, Vec<u8>),
    /// LispM does not have the resource.
    Missing(ResourceId),
}

/// A resource being received.
#[derive(Debug)]
struct Assembly {
    bytes: Vec<u8>,
    received: usize,
}

/// The client's state.
#[derive(Debug, Default)]
pub struct Model {
    /// The windows LispM offers.
    pub windows: Vec<Window>,
    /// The surfaces, by id.
    pub surfaces: BTreeMap<SurfaceId, Copy>,
    /// The surface shown.
    pub active: Option<SurfaceId>,
    asked: Vec<String>,
    assemblies: HashMap<ResourceId, Assembly>,
    inputs: Inputs,
}

impl Model {
    /// Forgets everything, as when the connection is lost; input not
    /// acknowledged is dropped, not sent again to a new session, and
    /// input from now on is numbered past it.
    pub fn reset(&mut self) {
        *self = Self {
            asked: std::mem::take(&mut self.asked),
            inputs: Inputs::from(self.inputs.next()),
            ..Self::default()
        };
    }

    /// The message that starts a session: input is numbered from here.
    #[must_use]
    pub fn hello(&self) -> ClientMessage {
        ClientMessage::Hello {
            first: Seq::new(self.inputs.next()),
        }
    }

    /// The message that opens `window` at `size`, remembering that this
    /// client asked so it shows the surface once it opens.
    pub fn open(&mut self, window: &str, size: Size, cell: CellPixels) -> ClientMessage {
        self.asked.push(window.to_owned());
        ClientMessage::Open {
            window: window.to_owned(),
            size,
            cell,
        }
    }

    /// Takes `input`, made on `surface`, to send with the input LispM
    /// has not acknowledged yet (see [`Model::flush`]).
    pub fn input(&mut self, surface: SurfaceId, input: Input) {
        let inputs = &mut self.inputs;
        inputs.pending.push_back((surface, input));
        inputs.unsent = true;
        while inputs.pending.len() > MAX_PENDING {
            inputs.pending.pop_front();
            inputs.first += 1;
        }
    }

    /// The message to send at `now` (milliseconds), if any: all the input
    /// LispM has not acknowledged, once new input gathered for
    /// [`INPUT_GATHER_MS`] or nothing was acknowledged for
    /// [`INPUT_RESEND_MS`].
    pub fn flush(&mut self, now: u64) -> Option<ClientMessage> {
        let inputs = &mut self.inputs;
        if inputs.pending.is_empty() {
            return None;
        }
        let since = inputs
            .sent_at
            .map_or(u64::MAX, |sent| now.saturating_sub(sent));
        let due = (inputs.unsent && since >= INPUT_GATHER_MS) || since >= INPUT_RESEND_MS;
        if !due {
            return None;
        }
        inputs.sent_at = Some(now);
        inputs.unsent = false;
        Some(ClientMessage::Input {
            first: Seq::new(inputs.first),
            inputs: inputs.pending.iter().cloned().collect(),
        })
    }

    /// The surface shown, if any.
    #[must_use]
    pub fn shown(&self) -> Option<(SurfaceId, &Copy)> {
        let id = self.active?;
        Some((id, self.surfaces.get(&id)?))
    }

    /// Takes in a message from LispM.
    pub fn receive(&mut self, message: ServerMessage) -> Vec<Effect> {
        match message {
            ServerMessage::Windows(windows) => {
                self.windows = windows;
                vec![Effect::Windows]
            }
            ServerMessage::Opened {
                surface,
                window,
                size,
            } => self.opened(surface, window, size),
            ServerMessage::Frame {
                surface,
                base,
                seq,
                more,
                update,
            } => self.frame(surface, base, seq, more, &update),
            ServerMessage::Closed { surface } => self.closed(surface),
            ServerMessage::Resource {
                id,
                total,
                offset,
                bytes,
            } => self.chunk(id, total, offset, &bytes),
            ServerMessage::Unknown { id } => {
                self.assemblies.remove(&id);
                vec![Effect::Missing(id)]
            }
            ServerMessage::Clipboard(text) => vec![Effect::Clipboard(text)],
            ServerMessage::Bell { surface } => vec![Effect::Bell(surface)],
            ServerMessage::InputAck { seq } => {
                let inputs = &mut self.inputs;
                while inputs.first <= seq.get() && inputs.pending.pop_front().is_some() {
                    inputs.first += 1;
                }
                Vec::new()
            }
        }
    }

    fn opened(&mut self, surface: SurfaceId, window: String, size: Size) -> Vec<Effect> {
        let mut effects = vec![Effect::Surfaces];
        let asked = self.asked.iter().position(|asked| *asked == window);
        if let Some(index) = asked {
            self.asked.remove(index);
            self.active = Some(surface);
            effects.push(Effect::Show(surface));
        } else if self.active.is_none() {
            self.active = Some(surface);
            effects.push(Effect::Fit(surface));
        }
        self.surfaces.insert(
            surface,
            Copy {
                window,
                replica: Replica::new(),
                blank: Screen::blank(size),
            },
        );
        effects.push(Effect::Redraw(surface));
        effects
    }

    fn frame(
        &mut self,
        surface: SurfaceId,
        base: Seq,
        seq: Seq,
        more: bool,
        update: &Update,
    ) -> Vec<Effect> {
        let Some(copy) = self.surfaces.get_mut(&surface) else {
            return Vec::new();
        };
        match copy.replica.receive(base, seq, update) {
            Received::Applied { seq } => {
                let mut effects = vec![Effect::Send(ClientMessage::Ack { surface, seq })];
                if !more {
                    effects.push(Effect::Redraw(surface));
                }
                effects
            }
            Received::Stale => Vec::new(),
            Received::Unbased => vec![Effect::Send(ClientMessage::Resync { surface })],
        }
    }

    fn closed(&mut self, surface: SurfaceId) -> Vec<Effect> {
        self.surfaces.remove(&surface);
        let mut effects = vec![Effect::Surfaces];
        if self.active == Some(surface) {
            self.active = self.surfaces.keys().next().copied();
            effects.extend(self.active.map(Effect::Redraw));
        }
        effects
    }

    fn chunk(&mut self, id: ResourceId, total: u32, offset: u32, bytes: &[u8]) -> Vec<Effect> {
        let (Ok(total), Ok(offset)) = (usize::try_from(total), usize::try_from(offset)) else {
            return Vec::new();
        };
        if total > MAX_RESOURCE {
            return vec![Effect::Missing(id)];
        }
        let assembly = self.assemblies.entry(id).or_insert_with(|| Assembly {
            bytes: vec![0; total],
            received: 0,
        });
        let fits = assembly.bytes.len() == total
            && assembly
                .bytes
                .get_mut(offset..offset.saturating_add(bytes.len()))
                .map(|slot| slot.copy_from_slice(bytes))
                .is_some();
        if !fits {
            self.assemblies.remove(&id);
            return vec![Effect::Missing(id)];
        }
        assembly.received += bytes.len();
        if assembly.received < total {
            return Vec::new();
        }
        match self.assemblies.remove(&id) {
            Some(whole) if id.names(&whole.bytes) => vec![Effect::Resource(id, whole.bytes)],
            _ => vec![Effect::Missing(id)],
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lispm_remote_display::Cell;
    use lispm_remote_display::Content;
    use lispm_remote_display::Row;
    use lispm_remote_display::Style;
    use lispm_remote_display::diff;

    use super::*;

    fn size() -> Size {
        Size::new(4, 2).unwrap()
    }

    fn screen(text: &str) -> Screen {
        let mut screen = Screen::blank(size());
        let mut cells = vec![Cell::blank(Style::PLAIN); 4];
        for (cell, c) in cells.iter_mut().zip(text.chars()) {
            cell.content = Content::Char(c);
        }
        screen.set_row(0, Arc::new(Row::new(cells))).unwrap();
        screen
    }

    fn frame(base: u64, seq: u64, from: &Screen, to: &Screen) -> ServerMessage {
        ServerMessage::Frame {
            surface: SurfaceId::new(1),
            base: Seq::new(base),
            seq: Seq::new(seq),
            more: false,
            update: diff(from, to),
        }
    }

    fn opened(model: &mut Model) {
        model.receive(ServerMessage::Opened {
            surface: SurfaceId::new(1),
            window: "w".to_owned(),
            size: size(),
        });
    }

    #[test]
    fn the_window_asked_for_is_shown_when_it_opens() {
        let mut model = Model::default();
        model.receive(ServerMessage::Opened {
            surface: SurfaceId::new(7),
            window: "other".to_owned(),
            size: size(),
        });
        model.open(
            "w",
            size(),
            CellPixels {
                width: 8,
                height: 16,
            },
        );
        let effects = model.receive(ServerMessage::Opened {
            surface: SurfaceId::new(1),
            window: "w".to_owned(),
            size: size(),
        });
        assert!(effects.contains(&Effect::Show(SurfaceId::new(1))));
        assert_eq!(model.active, Some(SurfaceId::new(1)));
    }

    #[test]
    fn the_first_surface_is_shown_and_fitted_to_this_client() {
        let mut model = Model::default();
        let effects = model.receive(ServerMessage::Opened {
            surface: SurfaceId::new(3),
            window: "w".to_owned(),
            size: size(),
        });
        assert!(effects.contains(&Effect::Fit(SurfaceId::new(3))));
        assert_eq!(model.active, Some(SurfaceId::new(3)));
    }

    #[test]
    fn frames_apply_in_order_and_are_acknowledged() {
        let mut model = Model::default();
        opened(&mut model);
        let blank = Screen::blank(size());
        let effects = model.receive(frame(0, 1, &blank, &screen("ab")));
        assert_eq!(
            effects[0],
            Effect::Send(ClientMessage::Ack {
                surface: SurfaceId::new(1),
                seq: Seq::new(1)
            })
        );
        model.receive(frame(1, 2, &screen("ab"), &screen("abc")));
        assert_eq!(*model.shown().unwrap().1.screen(), screen("abc"));
    }

    #[test]
    fn a_frame_on_a_state_the_client_lacks_asks_for_the_whole_screen() {
        let mut model = Model::default();
        opened(&mut model);
        let resync = Effect::Send(ClientMessage::Resync {
            surface: SurfaceId::new(1),
        });
        assert_eq!(
            model.receive(frame(5, 6, &screen("a"), &screen("b"))),
            vec![resync]
        );
        let blank = Screen::blank(size());
        model.receive(frame(0, 8, &blank, &screen("d")));
        assert_eq!(*model.shown().unwrap().1.screen(), screen("d"));
    }

    #[test]
    fn a_resource_is_kept_only_when_its_hash_matches() {
        let mut model = Model::default();
        let bytes = b"0123456789".to_vec();
        let id = ResourceId::of(&bytes);
        let chunk = |offset: u32, part: &[u8]| ServerMessage::Resource {
            id,
            total: 10,
            offset,
            bytes: part.to_vec(),
        };
        assert!(model.receive(chunk(0, &bytes[..6])).is_empty());
        assert_eq!(
            model.receive(chunk(6, &bytes[6..])),
            vec![Effect::Resource(id, bytes.clone())]
        );
        model.receive(chunk(0, &bytes[..6]));
        assert_eq!(model.receive(chunk(6, b"XXXX")), vec![Effect::Missing(id)]);
    }

    #[test]
    fn input_travels_gathered_and_again_until_acknowledged() {
        let mut model = Model::default();
        let surface = SurfaceId::new(1);
        let batch = |first: u64, inputs: &[bool]| {
            Some(ClientMessage::Input {
                first: Seq::new(first),
                inputs: inputs
                    .iter()
                    .map(|&focus| (surface, Input::Focus(focus)))
                    .collect(),
            })
        };
        model.input(surface, Input::Focus(true));
        assert_eq!(model.flush(0), batch(1, &[true]), "the first goes at once");
        model.input(surface, Input::Focus(false));
        assert_eq!(model.flush(INPUT_GATHER_MS - 1), None, "gathering");
        assert_eq!(
            model.flush(INPUT_GATHER_MS),
            batch(1, &[true, false]),
            "all unacknowledged"
        );
        model.receive(ServerMessage::InputAck { seq: Seq::new(1) });
        assert_eq!(model.flush(INPUT_GATHER_MS + 1), None, "nothing new");
        assert_eq!(
            model.flush(INPUT_GATHER_MS + INPUT_RESEND_MS),
            batch(2, &[false]),
            "again"
        );
        model.receive(ServerMessage::InputAck { seq: Seq::new(2) });
        assert_eq!(model.flush(10 * INPUT_RESEND_MS), None);
    }

    #[test]
    fn a_new_session_numbers_input_past_the_last_ones() {
        let mut model = Model::default();
        let surface = SurfaceId::new(1);
        assert_eq!(model.hello(), ClientMessage::Hello { first: Seq::new(1) });
        model.input(surface, Input::Focus(true));
        model.input(surface, Input::Focus(false));
        model.reset();
        assert_eq!(model.hello(), ClientMessage::Hello { first: Seq::new(3) });
        assert_eq!(model.flush(0), None, "the old input is not sent again");
        model.input(surface, Input::Focus(true));
        assert_eq!(
            model.flush(0),
            Some(ClientMessage::Input {
                first: Seq::new(3),
                inputs: vec![(surface, Input::Focus(true))],
            })
        );
    }

    #[test]
    fn closing_the_shown_surface_shows_another() {
        let mut model = Model::default();
        opened(&mut model);
        model.receive(ServerMessage::Opened {
            surface: SurfaceId::new(2),
            window: "v".to_owned(),
            size: size(),
        });
        model.receive(ServerMessage::Closed {
            surface: SurfaceId::new(1),
        });
        assert_eq!(model.active, Some(SurfaceId::new(2)));
    }
}

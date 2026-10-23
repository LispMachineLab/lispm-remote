// lispm-remote: input and messages as bytes.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The byte forms of input and of both directions' messages, and the
//! public `encode` and `decode` of each.

use super::CodecError;
use super::Reader;
use super::Wire;
use super::decode_tagged;
use super::encode_tagged;
use super::wire_struct;
use super::wire_tags;
use crate::input::Button;
use crate::input::Input;
use crate::input::Key;
use crate::input::KeyCode;
use crate::input::Mods;
use crate::input::Named;
use crate::input::Pointer;
use crate::input::PointerKind;
use crate::message::CellPixels;
use crate::message::ClientMessage;
use crate::message::Seq;
use crate::message::ServerMessage;
use crate::message::SurfaceId;
use crate::message::Window;
use crate::resource::ResourceId;
use crate::screen::Size;
use crate::update::Update;

impl Wire for Mods {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(self.bits());
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(Self::from_bits(input.byte()?))
    }
}

impl Wire for Named {
    fn put(&self, out: &mut Vec<u8>) {
        let tag = match self {
            Self::Enter => 0,
            Self::Tab => 1,
            Self::Backspace => 2,
            Self::Escape => 3,
            Self::Up => 4,
            Self::Down => 5,
            Self::Left => 6,
            Self::Right => 7,
            Self::Home => 8,
            Self::End => 9,
            Self::PageUp => 10,
            Self::PageDown => 11,
            Self::Insert => 12,
            Self::Delete => 13,
            Self::F(n) => {
                out.push(14);
                out.push(*n);
                return;
            }
        };
        out.push(tag);
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(match input.byte()? {
            0 => Self::Enter,
            1 => Self::Tab,
            2 => Self::Backspace,
            3 => Self::Escape,
            4 => Self::Up,
            5 => Self::Down,
            6 => Self::Left,
            7 => Self::Right,
            8 => Self::Home,
            9 => Self::End,
            10 => Self::PageUp,
            11 => Self::PageDown,
            12 => Self::Insert,
            13 => Self::Delete,
            14 => Self::F(input.byte()?),
            tag => return Err(CodecError::Tag { what: "key", tag }),
        })
    }
}

impl Wire for KeyCode {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::Char(c) => {
                out.push(0);
                c.put(out);
            }
            Self::Named(named) => {
                out.push(1);
                named.put(out);
            }
        }
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        match input.byte()? {
            0 => Ok(Self::Char(char::get(input)?)),
            1 => Ok(Self::Named(Named::get(input)?)),
            tag => Err(CodecError::Tag {
                what: "key code",
                tag,
            }),
        }
    }
}

wire_struct!(Key { code, mods });

wire_tags!(Button ("button") {
    Left = 0,
    Middle = 1,
    Right = 2,
    WheelUp = 3,
    WheelDown = 4,
    WheelLeft = 5,
    WheelRight = 6,
    None = 7,
});

wire_tags!(PointerKind ("pointer kind") {
    Press = 0,
    Release = 1,
    Motion = 2,
});

wire_struct!(Pointer {
    kind,
    button,
    col,
    row,
    mods
});

impl Wire for Input {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::Key(key) => {
                out.push(0);
                key.put(out);
            }
            Self::Text(text) => {
                out.push(1);
                text.put(out);
            }
            Self::Pointer(pointer) => {
                out.push(2);
                pointer.put(out);
            }
            Self::Focus(focused) => {
                out.push(3);
                focused.put(out);
            }
        }
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        match input.byte()? {
            0 => Ok(Self::Key(Key::get(input)?)),
            1 => Ok(Self::Text(String::get(input)?)),
            2 => Ok(Self::Pointer(Pointer::get(input)?)),
            3 => Ok(Self::Focus(bool::get(input)?)),
            tag => Err(CodecError::Tag { what: "input", tag }),
        }
    }
}

impl Wire for SurfaceId {
    fn put(&self, out: &mut Vec<u8>) {
        self.get().put(out);
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(Self::new(u32::get(input)?))
    }
}

impl Wire for Seq {
    fn put(&self, out: &mut Vec<u8>) {
        self.get().put(out);
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(Self::new(u64::get(input)?))
    }
}

wire_struct!(Window { name, title });
wire_struct!(CellPixels { width, height });

impl ServerMessage {
    /// The message's bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Windows(windows) => encode_tagged(1, |out| windows.put(out)),
            Self::Opened {
                surface,
                window,
                size,
            } => encode_tagged(2, |out| {
                surface.put(out);
                window.put(out);
                size.put(out);
            }),
            Self::Frame {
                surface,
                base,
                seq,
                more,
                update,
            } => encode_tagged(3, |out| {
                surface.put(out);
                base.put(out);
                seq.put(out);
                more.put(out);
                update.put(out);
            }),
            Self::Closed { surface } => encode_tagged(4, |out| surface.put(out)),
            Self::Resource {
                id,
                total,
                offset,
                bytes,
            } => encode_tagged(5, |out| {
                id.put(out);
                total.put(out);
                offset.put(out);
                bytes.put(out);
            }),
            Self::Unknown { id } => encode_tagged(6, |out| id.put(out)),
            Self::Clipboard(text) => encode_tagged(7, |out| text.put(out)),
            Self::Bell { surface } => encode_tagged(8, |out| surface.put(out)),
            Self::InputAck { seq } => encode_tagged(9, |out| seq.put(out)),
        }
    }

    /// The message `bytes` hold.
    ///
    /// # Errors
    ///
    /// [`CodecError`] when they hold none.
    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        decode_tagged(bytes, |tag, input| {
            Ok(match tag {
                1 => Self::Windows(Vec::get(input)?),
                2 => Self::Opened {
                    surface: Wire::get(input)?,
                    window: String::get(input)?,
                    size: Size::get(input)?,
                },
                3 => Self::Frame {
                    surface: Wire::get(input)?,
                    base: Wire::get(input)?,
                    seq: Wire::get(input)?,
                    more: bool::get(input)?,
                    update: Update::get(input)?,
                },
                4 => Self::Closed {
                    surface: Wire::get(input)?,
                },
                5 => Self::Resource {
                    id: ResourceId::get(input)?,
                    total: u32::get(input)?,
                    offset: u32::get(input)?,
                    bytes: Vec::get(input)?,
                },
                6 => Self::Unknown {
                    id: ResourceId::get(input)?,
                },
                7 => Self::Clipboard(String::get(input)?),
                8 => Self::Bell {
                    surface: Wire::get(input)?,
                },
                9 => Self::InputAck {
                    seq: Wire::get(input)?,
                },
                tag => {
                    return Err(CodecError::Tag {
                        what: "message",
                        tag,
                    });
                }
            })
        })
    }
}

impl ClientMessage {
    /// The message's bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Open { window, size, cell } => encode_tagged(1, |out| {
                window.put(out);
                size.put(out);
                cell.put(out);
            }),
            Self::Close { surface } => encode_tagged(2, |out| surface.put(out)),
            Self::Resize {
                surface,
                size,
                cell,
            } => encode_tagged(3, |out| {
                surface.put(out);
                size.put(out);
                cell.put(out);
            }),
            Self::Input { first, inputs } => encode_tagged(4, |out| {
                first.put(out);
                inputs.put(out);
            }),
            Self::Ack { surface, seq } => encode_tagged(5, |out| {
                surface.put(out);
                seq.put(out);
            }),
            Self::Resync { surface } => encode_tagged(6, |out| surface.put(out)),
            Self::Want { id } => encode_tagged(7, |out| id.put(out)),
            Self::Hello { first } => encode_tagged(8, |out| first.put(out)),
            Self::Bye => encode_tagged(9, |_| {}),
        }
    }

    /// The message `bytes` hold.
    ///
    /// # Errors
    ///
    /// [`CodecError`] when they hold none.
    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        decode_tagged(bytes, |tag, input| {
            Ok(match tag {
                1 => Self::Open {
                    window: String::get(input)?,
                    size: Size::get(input)?,
                    cell: CellPixels::get(input)?,
                },
                2 => Self::Close {
                    surface: Wire::get(input)?,
                },
                3 => Self::Resize {
                    surface: Wire::get(input)?,
                    size: Size::get(input)?,
                    cell: CellPixels::get(input)?,
                },
                4 => Self::Input {
                    first: Wire::get(input)?,
                    inputs: Vec::get(input)?,
                },
                5 => Self::Ack {
                    surface: Wire::get(input)?,
                    seq: Wire::get(input)?,
                },
                6 => Self::Resync {
                    surface: Wire::get(input)?,
                },
                7 => Self::Want {
                    id: ResourceId::get(input)?,
                },
                8 => Self::Hello {
                    first: Wire::get(input)?,
                },
                9 => Self::Bye,
                tag => {
                    return Err(CodecError::Tag {
                        what: "message",
                        tag,
                    });
                }
            })
        })
    }
}

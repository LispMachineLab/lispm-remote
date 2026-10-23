// lispm-remote: the codec's laws, witnessed.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

use std::sync::Arc;

use proptest::prelude::*;

use super::CodecError;
use super::MAX_MESSAGE_LEN;
use super::VERSION;
use crate::cell::Cell;
use crate::cell::Content;
use crate::cell::Row;
use crate::color::Color;
use crate::color::Rgb;
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
use crate::screen::CellRect;
use crate::screen::Cursor;
use crate::screen::CursorShape;
use crate::screen::PixelRect;
use crate::screen::Placement;
use crate::screen::PlacementKey;
use crate::screen::Screen;
use crate::screen::Size;
use crate::style::Attrs;
use crate::style::Style;
use crate::style::Underline;
use crate::update::diff;

fn color() -> impl Strategy<Value = Color> {
    prop_oneof![
        Just(Color::Default),
        any::<(u8, u8, u8)>().prop_map(|(r, g, b)| Color::Rgb(Rgb::new(r, g, b))),
    ]
}

fn style() -> impl Strategy<Value = Style> {
    (color(), color(), any::<u16>(), 0_u8..6, color()).prop_map(
        |(fg, bg, bits, kind, underline_color)| Style {
            fg,
            bg,
            attrs: Attrs::from_bits(bits),
            underline: [
                Underline::None,
                Underline::Single,
                Underline::Double,
                Underline::Curly,
                Underline::Dotted,
                Underline::Dashed,
            ][usize::from(kind)],
            underline_color,
        },
    )
}

fn content() -> impl Strategy<Value = Content> {
    prop_oneof![
        any::<char>().prop_map(Content::Char),
        "\\PC{1,4}".prop_map(|text| Content::Cluster(Arc::from(text))),
        Just(Content::Spacer),
    ]
}

fn row(cols: u16) -> impl Strategy<Value = Row> {
    // Few styles, so runs of several cells occur.
    let styles = proptest::collection::vec(style(), 1..3);
    styles.prop_flat_map(move |styles| {
        proptest::collection::vec((content(), 0..styles.len()), usize::from(cols)).prop_map(
            move |cells| {
                Row::new(
                    cells
                        .into_iter()
                        .map(|(content, which)| Cell {
                            content,
                            style: styles[which],
                        })
                        .collect(),
                )
            },
        )
    })
}

fn screen(size: Size) -> impl Strategy<Value = Screen> {
    (
        proptest::collection::vec(row(size.cols()), usize::from(size.rows())),
        any::<(u16, u16, bool)>(),
        proptest::option::of("[a-z ]{0,8}"),
        proptest::collection::vec(placement(), 0..3),
    )
        .prop_map(move |(rows, (col, row, visible), title, placements)| {
            let mut screen =
                Screen::from_rows(size, rows.into_iter().map(Arc::new).collect()).unwrap();
            screen.cursor = Cursor {
                col,
                row,
                visible,
                shape: CursorShape::Bar,
            };
            screen.title = title;
            screen.placements = placements;
            screen
        })
}

fn placement() -> impl Strategy<Value = Placement> {
    (
        any::<(u32, u32, [u8; 32])>(),
        any::<(u16, u16, u16, u16)>(),
        proptest::option::of(any::<(u32, u32, u32, u32)>()),
        any::<(u16, u16, i32)>(),
    )
        .prop_map(
            |((image, placement, hash), (col, row, cols, rows), crop, (offset_x, offset_y, z))| {
                Placement {
                    key: PlacementKey { image, placement },
                    resource: ResourceId::from_hash(hash),
                    cells: CellRect {
                        col,
                        row,
                        cols,
                        rows,
                    },
                    crop: crop.map(|(x, y, w, h)| PixelRect { x, y, w, h }),
                    offset_x,
                    offset_y,
                    z,
                }
            },
        )
}

fn size() -> impl Strategy<Value = Size> {
    (1_u16..6, 1_u16..5).prop_map(|(cols, rows)| Size::new(cols, rows).unwrap())
}

fn frame() -> impl Strategy<Value = ServerMessage> {
    (size(), size())
        .prop_flat_map(|(a, b)| (screen(a), screen(b)))
        .prop_flat_map(|(a, b)| {
            let update = diff(&a, &b);
            (any::<(u32, u64, u64, bool)>(), Just(update))
        })
        .prop_map(
            |((surface, base, seq, more), update)| ServerMessage::Frame {
                surface: SurfaceId::new(surface),
                base: Seq::new(base),
                seq: Seq::new(seq),
                more,
                update,
            },
        )
}

fn server_message() -> impl Strategy<Value = ServerMessage> {
    let id = any::<[u8; 32]>().prop_map(ResourceId::from_hash);
    prop_oneof![
        proptest::collection::vec(("\\PC{0,6}", "\\PC{0,6}"), 0..4).prop_map(|windows| {
            ServerMessage::Windows(
                windows
                    .into_iter()
                    .map(|(name, title)| Window { name, title })
                    .collect(),
            )
        }),
        (any::<u32>(), "\\PC{0,6}", size()).prop_map(|(surface, window, size)| {
            ServerMessage::Opened {
                surface: SurfaceId::new(surface),
                window,
                size,
            }
        }),
        frame(),
        any::<u32>().prop_map(|surface| ServerMessage::Closed {
            surface: SurfaceId::new(surface)
        }),
        (
            id.clone(),
            any::<(u32, u32)>(),
            proptest::collection::vec(any::<u8>(), 0..64)
        )
            .prop_map(|(id, (total, offset), bytes)| ServerMessage::Resource {
                id,
                total,
                offset,
                bytes
            }),
        id.prop_map(|id| ServerMessage::Unknown { id }),
        "\\PC{0,12}".prop_map(ServerMessage::Clipboard),
        any::<u32>().prop_map(|surface| ServerMessage::Bell {
            surface: SurfaceId::new(surface)
        }),
    ]
}

fn named() -> impl Strategy<Value = Named> {
    prop_oneof![
        Just(Named::Enter),
        Just(Named::Tab),
        Just(Named::Backspace),
        Just(Named::Escape),
        Just(Named::Up),
        Just(Named::Down),
        Just(Named::Left),
        Just(Named::Right),
        Just(Named::Home),
        Just(Named::End),
        Just(Named::PageUp),
        Just(Named::PageDown),
        Just(Named::Insert),
        Just(Named::Delete),
        (1_u8..=12).prop_map(Named::F),
    ]
}

fn input() -> impl Strategy<Value = Input> {
    let mods = any::<u8>().prop_map(Mods::from_bits);
    let buttons = [
        Button::Left,
        Button::Middle,
        Button::Right,
        Button::WheelUp,
        Button::WheelDown,
        Button::WheelLeft,
        Button::WheelRight,
        Button::None,
    ];
    let kinds = [
        PointerKind::Press,
        PointerKind::Release,
        PointerKind::Motion,
    ];
    prop_oneof![
        (
            prop_oneof![
                any::<char>().prop_map(KeyCode::Char),
                named().prop_map(KeyCode::Named)
            ],
            mods.clone()
        )
            .prop_map(|(code, mods)| Input::Key(Key { code, mods })),
        "\\PC{0,8}".prop_map(Input::Text),
        (0..buttons.len(), 0..kinds.len(), any::<(u16, u16)>(), mods).prop_map(
            move |(button, kind, (col, row), mods)| Input::Pointer(Pointer {
                kind: kinds[kind],
                button: buttons[button],
                col,
                row,
                mods,
            })
        ),
        any::<bool>().prop_map(Input::Focus),
    ]
}

fn client_message() -> impl Strategy<Value = ClientMessage> {
    let surface = any::<u32>().prop_map(SurfaceId::new);
    let cell = any::<(u16, u16)>().prop_map(|(width, height)| CellPixels { width, height });
    prop_oneof![
        ("\\PC{0,6}", size(), cell.clone()).prop_map(|(window, size, cell)| ClientMessage::Open {
            window,
            size,
            cell
        }),
        surface
            .clone()
            .prop_map(|surface| ClientMessage::Close { surface }),
        (surface.clone(), size(), cell).prop_map(|(surface, size, cell)| ClientMessage::Resize {
            surface,
            size,
            cell
        }),
        (surface.clone(), input())
            .prop_map(|(surface, input)| ClientMessage::Input { surface, input }),
        (surface.clone(), any::<u64>()).prop_map(|(surface, seq)| ClientMessage::Ack {
            surface,
            seq: Seq::new(seq)
        }),
        surface.prop_map(|surface| ClientMessage::Resync { surface }),
        any::<[u8; 32]>().prop_map(|hash| ClientMessage::Want {
            id: ResourceId::from_hash(hash)
        }),
        Just(ClientMessage::Hello),
        Just(ClientMessage::Bye),
    ]
}

#[test]
fn a_plain_line_costs_one_style() {
    let size = Size::new(40, 1).unwrap();
    let mut screen = Screen::blank(size);
    let text = Row::new(
        "a line of plain text, forty cells wide.."
            .chars()
            .map(|c| Cell {
                content: Content::Char(c),
                style: Style::PLAIN,
            })
            .collect(),
    );
    screen.set_row(0, Arc::new(text)).unwrap();
    let message = ServerMessage::Frame {
        surface: SurfaceId::new(1),
        base: Seq::ZERO,
        seq: Seq::new(1),
        more: false,
        update: diff(&Screen::blank(size), &screen),
    };
    // 40 characters, one style (5 bytes), and some 20 bytes of framing.
    assert!(
        message.encode().len() < 70,
        "{} bytes",
        message.encode().len()
    );
    assert_eq!(ServerMessage::decode(&message.encode()), Ok(message));
}

#[test]
fn another_version_is_refused() {
    let mut bytes = ClientMessage::Resync {
        surface: SurfaceId::new(1),
    }
    .encode();
    bytes[0] = VERSION + 1;
    assert_eq!(
        ClientMessage::decode(&bytes),
        Err(CodecError::Version(VERSION + 1))
    );
}

#[test]
fn trailing_bytes_are_refused() {
    let mut bytes = ClientMessage::Resync {
        surface: SurfaceId::new(1),
    }
    .encode();
    bytes.push(0);
    assert_eq!(ClientMessage::decode(&bytes), Err(CodecError::Trailing(1)));
}

#[test]
fn an_overlong_message_is_refused() {
    let bytes = vec![VERSION; MAX_MESSAGE_LEN + 1];
    assert_eq!(ServerMessage::decode(&bytes), Err(CodecError::TooLong));
}

#[test]
fn a_huge_count_does_not_allocate() {
    // A Windows message claiming 2^60 entries in a dozen bytes.
    let mut bytes = vec![VERSION, 1];
    super::put_varint(&mut bytes, 1 << 60);
    assert_eq!(ServerMessage::decode(&bytes), Err(CodecError::Truncated));
}

#[test]
fn runs_that_overrun_the_row_are_refused() {
    use super::Reader;
    use super::Wire;
    let row = Row::new(vec![Cell::blank(Style::PLAIN); 3]);
    let mut bytes = Vec::new();
    row.put(&mut bytes);
    // The row says it is 2 cells long; its one piece holds 3.
    bytes[0] = 2;
    assert_eq!(Row::get(&mut Reader::new(&bytes)), Err(CodecError::Runs));
}

#[test]
fn wide_characters_travel_as_text() {
    use super::Reader;
    use super::Wire;
    let wide = |c| {
        [
            Cell {
                content: Content::Char(c),
                style: Style::PLAIN,
            },
            Cell {
                content: Content::Spacer,
                style: Style::PLAIN,
            },
        ]
    };
    let mut cells = vec![Cell {
        content: Content::Spacer,
        style: Style::PLAIN,
    }];
    cells.extend(['\u{4f60}', '\u{597d}'].into_iter().flat_map(wide));
    cells.push(Cell {
        content: Content::Char('a'),
        style: Style::PLAIN,
    });
    let row = Row::new(cells);
    let mut bytes = Vec::new();
    row.put(&mut bytes);
    // cells, style (5), pieces, a lone spacer (2), the wide piece
    // (1 + 1 + 6), the narrow piece (1 + 1 + 1).
    assert_eq!(bytes.len(), 1 + 5 + 1 + 2 + 8 + 3);
    assert_eq!(Row::get(&mut Reader::new(&bytes)), Ok(row));
}

proptest! {
    // Law: ∀ m. decode(encode(m)) = Ok(m).
    #[test]
    fn server_messages_round_trip(m in server_message()) {
        prop_assert_eq!(ServerMessage::decode(&m.encode()), Ok(m));
    }

    // Law: ∀ m. decode(encode(m)) = Ok(m).
    #[test]
    fn client_messages_round_trip(m in client_message()) {
        prop_assert_eq!(ClientMessage::decode(&m.encode()), Ok(m));
    }

    // Law: decode is total.
    #[test]
    fn decoding_arbitrary_bytes_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = ServerMessage::decode(&bytes);
        let _ = ClientMessage::decode(&bytes);
    }

    // Law: decode is total on damaged messages too, which reach deeper.
    #[test]
    fn decoding_a_damaged_message_never_panics(m in server_message(), at in any::<usize>(), byte in any::<u8>()) {
        let mut bytes = m.encode();
        let len = bytes.len();
        bytes[at % len] = byte;
        let _ = ServerMessage::decode(&bytes);
        bytes.truncate(at % len);
        let _ = ServerMessage::decode(&bytes);
    }
}

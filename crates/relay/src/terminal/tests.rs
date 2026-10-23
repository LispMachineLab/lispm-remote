// lispm-remote: the terminal, witnessed.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use lispm_remote_display::Color;
use lispm_remote_display::Content;
use lispm_remote_display::Rgb;
use lispm_remote_display::Size;
use lispm_remote_display::Underline;
use proptest::prelude::*;

use super::Event;
use super::MouseMode;
use super::Options;
use super::Terminal;

fn terminal(cols: u16, rows: u16) -> Terminal {
    Terminal::new(Size::new(cols, rows).unwrap(), Options::default())
}

fn lines(terminal: &Terminal) -> Vec<String> {
    terminal
        .screen()
        .text()
        .lines()
        .map(|line| line.trim_end().to_owned())
        .collect()
}

fn fed(cols: u16, rows: u16, bytes: &[u8]) -> Terminal {
    let mut terminal = terminal(cols, rows);
    terminal.feed(bytes);
    terminal
}

/// A 1×1 PNG's header, enough for its size to be read.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&width.to_be_bytes());
    png.extend_from_slice(&height.to_be_bytes());
    png.extend_from_slice(b"\x08\x06\0\0\0");
    png
}

#[test]
fn text_lands_where_the_cursor_goes() {
    let terminal = fed(10, 3, b"ab\r\ncd\x1b[3;5Hx");
    assert_eq!(lines(&terminal), ["ab", "cd", "    x"]);
    assert_eq!(
        (terminal.screen().cursor.col, terminal.screen().cursor.row),
        (5, 2)
    );
}

#[test]
fn emacs_line_starts_move_as_it_means() {
    // Emacs moves between lines with CR LF, then up and down again.
    let terminal = fed(10, 3, b"one\r\n\x1b[A\ntwo\r\n\x1b[A\nthree");
    assert_eq!(lines(&terminal), ["one", "two", "three"]);
}

#[test]
fn the_last_column_waits_to_wrap() {
    let mut terminal = fed(3, 2, b"abc");
    assert_eq!(lines(&terminal), ["abc", ""]);
    terminal.feed(b"d");
    assert_eq!(lines(&terminal), ["abc", "d"]);
}

#[test]
fn a_line_feed_at_the_bottom_scrolls_only_the_region() {
    let terminal = fed(4, 4, b"a\r\nb\r\nc\r\nd\x1b[2;3r\x1b[3;1H\nx");
    assert_eq!(lines(&terminal), ["a", "c", "x", "d"]);
}

#[test]
fn erasing_takes_the_pens_background() {
    let terminal = fed(4, 1, b"abcd\x1b[1;2H\x1b[41m\x1b[K");
    let row = terminal.screen().row(0).unwrap();
    assert_eq!(row.text(), "a   ");
    let red = Color::Rgb(Rgb::new(205, 0, 0));
    assert_eq!(row.cells()[2].style.bg, red);
    assert_eq!(row.cells()[0].style.bg, Color::Default);
}

#[test]
fn inserting_and_deleting_lines_work_inside_the_region() {
    let terminal = fed(2, 4, b"a\r\nb\r\nc\r\nd\x1b[2;1H\x1b[L");
    assert_eq!(lines(&terminal), ["a", "", "b", "c"]);
    let terminal = fed(2, 4, b"a\r\nb\r\nc\r\nd\x1b[2;1H\x1b[2M");
    assert_eq!(lines(&terminal), ["a", "d", "", ""]);
}

#[test]
fn a_wide_character_takes_two_cells_and_breaks_cleanly() {
    let mut terminal = fed(4, 1, "\u{4f60}\u{597d}".as_bytes());
    let row = terminal.screen().row(0).unwrap();
    assert_eq!(row.cells()[1].content, Content::Spacer);
    assert_eq!(row.text(), "\u{4f60}\u{597d}");
    // Overwriting the right half of the first blanks its left half.
    terminal.feed(b"\x1b[1;2Hx");
    assert_eq!(terminal.screen().row(0).unwrap().text(), " x\u{597d}");
}

#[test]
fn a_wide_character_at_the_last_column_wraps_first() {
    let terminal = fed(3, 2, "ab\u{4f60}".as_bytes());
    assert_eq!(lines(&terminal), ["ab", "\u{4f60}"]);
}

#[test]
fn combining_marks_join_their_base() {
    let terminal = fed(4, 1, "e\u{301}x".as_bytes());
    let row = terminal.screen().row(0).unwrap();
    assert_eq!(row.cells()[0].content, Content::Cluster("e\u{301}".into()));
    assert_eq!(row.cells()[1].content, Content::Char('x'));
}

#[test]
fn colours_come_in_every_form_emacs_writes() {
    let table: [(&[u8], Color); 4] = [
        (b"\x1b[38;5;196mx", Color::Rgb(Rgb::new(255, 0, 0))),
        (b"\x1b[38;2;1;2;3mx", Color::Rgb(Rgb::new(1, 2, 3))),
        (b"\x1b[38:2::4:5:6mx", Color::Rgb(Rgb::new(4, 5, 6))),
        (b"\x1b[91mx", Color::Rgb(Rgb::new(255, 0, 0))),
    ];
    for (bytes, color) in table {
        let terminal = fed(2, 1, bytes);
        assert_eq!(
            terminal.screen().row(0).unwrap().cells()[0].style.fg,
            color,
            "{bytes:?}"
        );
    }
    let terminal = fed(2, 1, b"\x1b[4:3;1mx\x1b[0my");
    let cells = terminal.screen().row(0).unwrap().cells();
    assert_eq!(cells[0].style.underline, Underline::Curly);
    assert_eq!(cells[1].style.underline, Underline::None);
}

#[test]
fn emacs_is_told_it_talks_to_a_new_xterm() {
    let mut terminal = fed(
        80,
        24,
        b"\x1b[c\x1b[>0c\x1b[>0q\x1b]11;?\x1b\\\x1b[16t\x1b[18t",
    );
    let replies = String::from_utf8(terminal.take_replies()).unwrap();
    assert_eq!(
        replies,
        "\x1b[?62;22;52c\x1b[>41;390;0c\x1bP>|LispM Remote\x1b\\\x1b]11;rgb:ffff/ffff/ffff\x1b\\\x1b[6;16;8t\x1b[8;24;80t"
    );
    assert!(terminal.take_replies().is_empty());
}

#[test]
fn modes_follow_what_emacs_sets() {
    let terminal = fed(
        10,
        2,
        b"\x1b[?1h\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[>4;1m\x1b[?25l",
    );
    let modes = terminal.modes();
    assert!(modes.app_cursor && modes.sgr_mouse && modes.bracketed_paste);
    assert_eq!(modes.mouse, MouseMode::Drag);
    assert_eq!(modes.modify_other_keys, 1);
    assert!(!terminal.screen().cursor.visible);
    let terminal = fed(10, 2, b"\x1b[?1002h\x1b[?1002l");
    assert_eq!(terminal.modes().mouse, MouseMode::Off);
}

#[test]
fn the_alternate_screen_keeps_the_main_one() {
    let mut terminal = fed(5, 2, b"main\x1b[?1049h");
    assert_eq!(lines(&terminal), ["", ""]);
    terminal.feed(b"alt\x1b[?1049l");
    assert_eq!(lines(&terminal), ["main", ""]);
}

#[test]
fn the_title_and_clipboard_are_kept() {
    let mut terminal = fed(5, 1, b"\x1b]2;*scratch*\x07");
    assert_eq!(terminal.screen().title.as_deref(), Some("*scratch*"));
    let osc = format!("\x1b]52;c;{}\x1b\\", STANDARD.encode("killed"));
    terminal.feed(osc.as_bytes());
    assert_eq!(
        terminal.take_events(),
        vec![Event::Clipboard("killed".to_owned())]
    );
}

#[test]
fn a_transmitted_image_is_placed_by_its_hash() {
    let bytes = png(16, 32);
    let data = STANDARD.encode(&bytes);
    let (first, second) = data.split_at(8);
    let mut terminal = terminal(20, 10);
    let stream = format!(
        "\x1b_Ga=t,q=2,f=100,i=7,m=1;{first}\x1b\\\x1b_Ga=t,q=2,f=100,i=7,m=0;{second}\x1b\\\
         \x1b[3;4H\x1b_Gq=2,a=p,i=7,p=1,c=2,r=2,z=-1,C=1\x1b\\"
    );
    terminal.feed(stream.as_bytes());
    let id = lispm_remote_display::ResourceId::of(&bytes);
    assert_eq!(
        terminal.take_events(),
        vec![Event::Resource(id, bytes.into())]
    );
    let placement = terminal.screen().placements[0];
    assert_eq!(placement.resource, id);
    assert_eq!((placement.cells.col, placement.cells.row), (3, 2));
    assert_eq!(
        (placement.cells.cols, placement.cells.rows, placement.z),
        (2, 2, -1)
    );
    assert_eq!(
        (terminal.screen().cursor.col, terminal.screen().cursor.row),
        (3, 2)
    );
    assert!(terminal.take_replies().is_empty());
    terminal.feed(b"\x1b_Ga=d,d=i,i=7,p=1,q=2\x1b\\");
    assert!(terminal.screen().placements.is_empty());
}

#[test]
fn a_graphics_query_is_answered() {
    let mut terminal = fed(20, 10, b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\");
    assert_eq!(terminal.take_replies(), b"\x1b_Gi=31;OK\x1b\\".to_vec());
}

#[test]
fn clearing_the_screen_takes_images_away() {
    let data = STANDARD.encode(png(8, 16));
    let stream = format!("\x1b_Ga=T,f=100,q=2,i=1;{data}\x1b\\\x1b[2J");
    let terminal = fed(10, 4, stream.as_bytes());
    assert!(terminal.screen().placements.is_empty());
}

#[test]
fn resizing_keeps_the_top_left() {
    let mut terminal = fed(6, 3, b"abcdef\r\nghi");
    terminal.resize(Size::new(4, 2).unwrap());
    assert_eq!(lines(&terminal), ["abcd", "ghi"]);
}

proptest! {
    // Law: feed is total, keeps the cursor on the grid and every row
    // as wide as the screen, whatever the bytes.
    #[test]
    fn any_bytes_leave_a_whole_screen(
        bytes in proptest::collection::vec(
            prop_oneof![
                Just(b"\x1b[".to_vec()),
                Just(b"\x1b[?1049h".to_vec()),
                Just(b"\x1b[2;3r".to_vec()),
                Just("\u{4f60}".as_bytes().to_vec()),
                Just(b"\x1b_Ga=p,i=1\x1b\\".to_vec()),
                proptest::collection::vec(any::<u8>(), 0..6),
                "[ -~]{0,6}".prop_map(String::into_bytes),
                Just(b"\x1b[L\x1b[M\x1b[@\x1b[P\x1b[X".to_vec()),
            ],
            0..40,
        ).prop_map(|parts| parts.concat()),
        (cols, rows) in (1_u16..12, 1_u16..6),
    ) {
        let terminal = fed(cols, rows, &bytes);
        let screen = terminal.screen();
        prop_assert!(screen.cursor.col < cols && screen.cursor.row < rows);
        prop_assert_eq!(screen.rows().len(), usize::from(rows));
        for row in screen.rows() {
            prop_assert!(row.fits(cols));
            // No spacer without a wide character before it.
            for (index, cell) in row.cells().iter().enumerate() {
                if cell.content.is_spacer() {
                    prop_assert!(index > 0 && !row.cells()[index - 1].content.is_spacer());
                }
            }
        }
    }
}

//! FPS counter and shared text overlay: 5x7 bitmap font, rasteriser and the
//! `glDrawPixels` overlay.
//!
//! This uses legacy `glDrawPixels` instead of a text renderer, which is why the
//! font is embedded as a bitmap. A single drawing API shared with `coke` mods
//! is the intended next step; see the mod API in `docs/ai/FINDINGS.md`.

use crate::*;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::OnceLock;

// ---- code ----
// ------------------------------------------------------------------ FPS overlay

/// Minimal 5-wide × 7-tall 1bpp bitmap font for digits 0-9, '.', and the
/// letters used by the sample `FPS` overlay. Each char is 5 bytes, one byte per
/// row (top to bottom), bits left to right in the high bits of each byte.
#[rustfmt::skip]
pub const FONT5X7: [[u8; 7]; 15] = [
    // 0
    [0b01110000, 0b10001000, 0b10011000, 0b10101000, 0b11001000, 0b10001000, 0b01110000],
    // 1
    [0b00100000, 0b01100000, 0b00100000, 0b00100000, 0b00100000, 0b00100000, 0b01110000],
    // 2
    [0b01110000, 0b10001000, 0b00001000, 0b00110000, 0b01000000, 0b10000000, 0b11111000],
    // 3
    [0b11111000, 0b00001000, 0b00010000, 0b00110000, 0b00001000, 0b10001000, 0b01110000],
    // 4
    [0b00010000, 0b00110000, 0b01010000, 0b10010000, 0b11111000, 0b00010000, 0b00010000],
    // 5
    [0b11111000, 0b10000000, 0b11110000, 0b00001000, 0b00001000, 0b10001000, 0b01110000],
    // 6
    [0b00110000, 0b01000000, 0b10000000, 0b11110000, 0b10001000, 0b10001000, 0b01110000],
    // 7
    [0b11111000, 0b00001000, 0b00010000, 0b00100000, 0b01000000, 0b01000000, 0b01000000],
    // 8
    [0b01110000, 0b10001000, 0b10001000, 0b01110000, 0b10001000, 0b10001000, 0b01110000],
    // 9
    [0b01110000, 0b10001000, 0b10001000, 0b01111000, 0b00001000, 0b00010000, 0b01100000],
    // '.' (index 10)
    [0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b01100000, 0b01100000],
    // 'F' (index 11)
    [0b11111000, 0b10000000, 0b11110000, 0b10000000, 0b10000000, 0b10000000, 0b10000000],
    // 'P' (index 12)
    [0b11110000, 0b10001000, 0b10001000, 0b11110000, 0b10000000, 0b10000000, 0b10000000],
    // 'S' (index 13)
    [0b01111000, 0b10000000, 0b10000000, 0b01110000, 0b00001000, 0b00001000, 0b11110000],
    // ' ' (index 14)
    [0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b00000000],
];

pub const FONT_ASCII: [[u8; 7]; 95] = [
    // 0x20 space
    [
        0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b00000000,
    ],
    // 0x21 bang
    [
        0b00100000, 0b00100000, 0b00100000, 0b00100000, 0b00100000, 0b00000000, 0b00100000,
    ],
    // 0x22 "
    [
        0b01010000, 0b01010000, 0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b00000000,
    ],
    // 0x23 #
    [
        0b01010000, 0b01010000, 0b11111000, 0b01010000, 0b11111000, 0b01010000, 0b01010000,
    ],
    // 0x24 $
    [
        0b00100000, 0b01111000, 0b10100000, 0b01110000, 0b00101000, 0b11110000, 0b00100000,
    ],
    // 0x25 %
    [
        0b11001000, 0b11001000, 0b00010000, 0b00100000, 0b01000000, 0b10011000, 0b10011000,
    ],
    // 0x26 &
    [
        0b01100000, 0b10010000, 0b10100000, 0b01000000, 0b10101000, 0b10010000, 0b01101000,
    ],
    // 0x27 '
    [
        0b00100000, 0b00100000, 0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b00000000,
    ],
    // 0x28 lparen
    [
        0b00010000, 0b00100000, 0b01000000, 0b01000000, 0b01000000, 0b00100000, 0b00010000,
    ],
    // 0x29 rparen
    [
        0b01000000, 0b00100000, 0b00010000, 0b00010000, 0b00010000, 0b00100000, 0b01000000,
    ],
    // 0x2a *
    [
        0b00000000, 0b10101000, 0b01110000, 0b11111000, 0b01110000, 0b10101000, 0b00000000,
    ],
    // 0x2b +
    [
        0b00000000, 0b00100000, 0b00100000, 0b11111000, 0b00100000, 0b00100000, 0b00000000,
    ],
    // 0x2c comma
    [
        0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b00100000, 0b00100000, 0b01000000,
    ],
    // 0x2d dash
    [
        0b00000000, 0b00000000, 0b00000000, 0b11111000, 0b00000000, 0b00000000, 0b00000000,
    ],
    // 0x2e dot
    [
        0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b01100000, 0b01100000,
    ],
    // 0x2f slash
    [
        0b00001000, 0b00010000, 0b00010000, 0b00100000, 0b01000000, 0b01000000, 0b10000000,
    ],
    // 0x30 0
    [
        0b01110000, 0b10001000, 0b10011000, 0b10101000, 0b11001000, 0b10001000, 0b01110000,
    ],
    // 0x31 1
    [
        0b00100000, 0b01100000, 0b00100000, 0b00100000, 0b00100000, 0b00100000, 0b01110000,
    ],
    // 0x32 2
    [
        0b01110000, 0b10001000, 0b00001000, 0b00010000, 0b00100000, 0b01000000, 0b11111000,
    ],
    // 0x33 3
    [
        0b11111000, 0b00010000, 0b00100000, 0b00010000, 0b00001000, 0b10001000, 0b01110000,
    ],
    // 0x34 4
    [
        0b00010000, 0b00110000, 0b01010000, 0b10010000, 0b11111000, 0b00010000, 0b00010000,
    ],
    // 0x35 5
    [
        0b11111000, 0b10000000, 0b11110000, 0b00001000, 0b00001000, 0b10001000, 0b01110000,
    ],
    // 0x36 6
    [
        0b00110000, 0b01000000, 0b10000000, 0b11110000, 0b10001000, 0b10001000, 0b01110000,
    ],
    // 0x37 7
    [
        0b11111000, 0b00001000, 0b00010000, 0b00100000, 0b01000000, 0b01000000, 0b01000000,
    ],
    // 0x38 8
    [
        0b01110000, 0b10001000, 0b10001000, 0b01110000, 0b10001000, 0b10001000, 0b01110000,
    ],
    // 0x39 9
    [
        0b01110000, 0b10001000, 0b10001000, 0b01111000, 0b00001000, 0b00010000, 0b01100000,
    ],
    // 0x3a colon
    [
        0b00000000, 0b01100000, 0b01100000, 0b00000000, 0b01100000, 0b01100000, 0b00000000,
    ],
    // 0x3b ;
    [
        0b00000000, 0b01100000, 0b01100000, 0b00000000, 0b01100000, 0b00100000, 0b01000000,
    ],
    // 0x3c <
    [
        0b00010000, 0b00100000, 0b01000000, 0b10000000, 0b01000000, 0b00100000, 0b00010000,
    ],
    // 0x3d =
    [
        0b00000000, 0b00000000, 0b11111000, 0b00000000, 0b11111000, 0b00000000, 0b00000000,
    ],
    // 0x3e >
    [
        0b01000000, 0b00100000, 0b00010000, 0b00001000, 0b00010000, 0b00100000, 0b01000000,
    ],
    // 0x3f question
    [
        0b01110000, 0b10001000, 0b00001000, 0b00010000, 0b00100000, 0b00000000, 0b00100000,
    ],
    // 0x40 @
    [
        0b01110000, 0b10001000, 0b00001000, 0b01101000, 0b10101000, 0b10101000, 0b01111000,
    ],
    // 0x41 A
    [
        0b01110000, 0b10001000, 0b10001000, 0b11111000, 0b10001000, 0b10001000, 0b10001000,
    ],
    // 0x42 B
    [
        0b11110000, 0b10001000, 0b10001000, 0b11110000, 0b10001000, 0b10001000, 0b11110000,
    ],
    // 0x43 C
    [
        0b01110000, 0b10001000, 0b10000000, 0b10000000, 0b10000000, 0b10001000, 0b01110000,
    ],
    // 0x44 D
    [
        0b11110000, 0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b11110000,
    ],
    // 0x45 E
    [
        0b11111000, 0b10000000, 0b10000000, 0b11110000, 0b10000000, 0b10000000, 0b11111000,
    ],
    // 0x46 F
    [
        0b11111000, 0b10000000, 0b10000000, 0b11110000, 0b10000000, 0b10000000, 0b10000000,
    ],
    // 0x47 G
    [
        0b01110000, 0b10001000, 0b10000000, 0b10111000, 0b10001000, 0b10001000, 0b01110000,
    ],
    // 0x48 H
    [
        0b10001000, 0b10001000, 0b10001000, 0b11111000, 0b10001000, 0b10001000, 0b10001000,
    ],
    // 0x49 I
    [
        0b01110000, 0b00100000, 0b00100000, 0b00100000, 0b00100000, 0b00100000, 0b01110000,
    ],
    // 0x4a J
    [
        0b00111000, 0b00010000, 0b00010000, 0b00010000, 0b00010000, 0b10010000, 0b01100000,
    ],
    // 0x4b K
    [
        0b10001000, 0b10010000, 0b10100000, 0b11000000, 0b10100000, 0b10010000, 0b10001000,
    ],
    // 0x4c L
    [
        0b10000000, 0b10000000, 0b10000000, 0b10000000, 0b10000000, 0b10000000, 0b11111000,
    ],
    // 0x4d M
    [
        0b10001000, 0b11011000, 0b10101000, 0b10101000, 0b10001000, 0b10001000, 0b10001000,
    ],
    // 0x4e N
    [
        0b10001000, 0b10001000, 0b11001000, 0b10101000, 0b10011000, 0b10001000, 0b10001000,
    ],
    // 0x4f O
    [
        0b01110000, 0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b01110000,
    ],
    // 0x50 P
    [
        0b11110000, 0b10001000, 0b10001000, 0b11110000, 0b10000000, 0b10000000, 0b10000000,
    ],
    // 0x51 Q
    [
        0b01110000, 0b10001000, 0b10001000, 0b10001000, 0b10101000, 0b10010000, 0b01101000,
    ],
    // 0x52 R
    [
        0b11110000, 0b10001000, 0b10001000, 0b11110000, 0b10100000, 0b10010000, 0b10001000,
    ],
    // 0x53 S
    [
        0b01111000, 0b10000000, 0b10000000, 0b01110000, 0b00001000, 0b00001000, 0b11110000,
    ],
    // 0x54 T
    [
        0b11111000, 0b00100000, 0b00100000, 0b00100000, 0b00100000, 0b00100000, 0b00100000,
    ],
    // 0x55 U
    [
        0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b01110000,
    ],
    // 0x56 V
    [
        0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b01010000, 0b00100000,
    ],
    // 0x57 W
    [
        0b10001000, 0b10001000, 0b10001000, 0b10101000, 0b10101000, 0b11011000, 0b10001000,
    ],
    // 0x58 X
    [
        0b10001000, 0b10001000, 0b01010000, 0b00100000, 0b01010000, 0b10001000, 0b10001000,
    ],
    // 0x59 Y
    [
        0b10001000, 0b10001000, 0b01010000, 0b00100000, 0b00100000, 0b00100000, 0b00100000,
    ],
    // 0x5a Z
    [
        0b11111000, 0b00001000, 0b00010000, 0b00100000, 0b01000000, 0b10000000, 0b11111000,
    ],
    // 0x5b lbracket
    [
        0b01110000, 0b01000000, 0b01000000, 0b01000000, 0b01000000, 0b01000000, 0b01110000,
    ],
    // 0x5c backslash
    [
        0b10000000, 0b01000000, 0b01000000, 0b00100000, 0b00010000, 0b00010000, 0b00001000,
    ],
    // 0x5d rbracket
    [
        0b01110000, 0b00010000, 0b00010000, 0b00010000, 0b00010000, 0b00010000, 0b01110000,
    ],
    // 0x5e ^
    [
        0b00100000, 0b01010000, 0b10001000, 0b00000000, 0b00000000, 0b00000000, 0b00000000,
    ],
    // 0x5f underscore
    [
        0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b11111000,
    ],
    // 0x60 `
    [
        0b01000000, 0b00100000, 0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b00000000,
    ],
    // 0x61 a
    [
        0b01110000, 0b10001000, 0b10001000, 0b11111000, 0b10001000, 0b10001000, 0b10001000,
    ],
    // 0x62 b
    [
        0b11110000, 0b10001000, 0b10001000, 0b11110000, 0b10001000, 0b10001000, 0b11110000,
    ],
    // 0x63 c
    [
        0b01110000, 0b10001000, 0b10000000, 0b10000000, 0b10000000, 0b10001000, 0b01110000,
    ],
    // 0x64 d
    [
        0b11110000, 0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b11110000,
    ],
    // 0x65 e
    [
        0b11111000, 0b10000000, 0b10000000, 0b11110000, 0b10000000, 0b10000000, 0b11111000,
    ],
    // 0x66 f
    [
        0b11111000, 0b10000000, 0b10000000, 0b11110000, 0b10000000, 0b10000000, 0b10000000,
    ],
    // 0x67 g
    [
        0b01110000, 0b10001000, 0b10000000, 0b10111000, 0b10001000, 0b10001000, 0b01110000,
    ],
    // 0x68 h
    [
        0b10001000, 0b10001000, 0b10001000, 0b11111000, 0b10001000, 0b10001000, 0b10001000,
    ],
    // 0x69 i
    [
        0b01110000, 0b00100000, 0b00100000, 0b00100000, 0b00100000, 0b00100000, 0b01110000,
    ],
    // 0x6a j
    [
        0b00111000, 0b00010000, 0b00010000, 0b00010000, 0b00010000, 0b10010000, 0b01100000,
    ],
    // 0x6b k
    [
        0b10001000, 0b10010000, 0b10100000, 0b11000000, 0b10100000, 0b10010000, 0b10001000,
    ],
    // 0x6c l
    [
        0b10000000, 0b10000000, 0b10000000, 0b10000000, 0b10000000, 0b10000000, 0b11111000,
    ],
    // 0x6d m
    [
        0b10001000, 0b11011000, 0b10101000, 0b10101000, 0b10001000, 0b10001000, 0b10001000,
    ],
    // 0x6e n
    [
        0b10001000, 0b10001000, 0b11001000, 0b10101000, 0b10011000, 0b10001000, 0b10001000,
    ],
    // 0x6f o
    [
        0b01110000, 0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b01110000,
    ],
    // 0x70 p
    [
        0b11110000, 0b10001000, 0b10001000, 0b11110000, 0b10000000, 0b10000000, 0b10000000,
    ],
    // 0x71 q
    [
        0b01110000, 0b10001000, 0b10001000, 0b10001000, 0b10101000, 0b10010000, 0b01101000,
    ],
    // 0x72 r
    [
        0b11110000, 0b10001000, 0b10001000, 0b11110000, 0b10100000, 0b10010000, 0b10001000,
    ],
    // 0x73 s
    [
        0b01111000, 0b10000000, 0b10000000, 0b01110000, 0b00001000, 0b00001000, 0b11110000,
    ],
    // 0x74 t
    [
        0b11111000, 0b00100000, 0b00100000, 0b00100000, 0b00100000, 0b00100000, 0b00100000,
    ],
    // 0x75 u
    [
        0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b01110000,
    ],
    // 0x76 v
    [
        0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b10001000, 0b01010000, 0b00100000,
    ],
    // 0x77 w
    [
        0b10001000, 0b10001000, 0b10001000, 0b10101000, 0b10101000, 0b11011000, 0b10001000,
    ],
    // 0x78 x
    [
        0b10001000, 0b10001000, 0b01010000, 0b00100000, 0b01010000, 0b10001000, 0b10001000,
    ],
    // 0x79 y
    [
        0b10001000, 0b10001000, 0b01010000, 0b00100000, 0b00100000, 0b00100000, 0b00100000,
    ],
    // 0x7a z
    [
        0b11111000, 0b00001000, 0b00010000, 0b00100000, 0b01000000, 0b10000000, 0b11111000,
    ],
    // 0x7b {
    [
        0b00010000, 0b00100000, 0b00100000, 0b01000000, 0b00100000, 0b00100000, 0b00010000,
    ],
    // 0x7c pipe
    [
        0b00100000, 0b00100000, 0b00100000, 0b00100000, 0b00100000, 0b00100000, 0b00100000,
    ],
    // 0x7d }
    [
        0b01000000, 0b00100000, 0b00100000, 0b00010000, 0b00100000, 0b00100000, 0b01000000,
    ],
    // 0x7e ~
    [
        0b00000000, 0b00001000, 0b01001000, 0b10101000, 0b10000000, 0b00000000, 0b00000000,
    ],
];

/// Index into `FONT_ASCII` for a printable ASCII character. Lowercase is
/// folded to uppercase by the table itself; anything outside 0x20..=0x7e
/// renders as a space.
#[inline]
fn ascii_index(ch: char) -> usize {
    let c = ch as u32;
    if (0x20..=0x7e).contains(&c) {
        (c - 0x20) as usize
    } else {
        0
    }
}

pub const CHAR_W: i32 = 6; // 5 pixels + 1 gap
pub const CHAR_H: i32 = 9; // 7 pixels + 2 gap

/// Glyph indices for an FPS value in tenths, e.g. `614` -> `[6, 1, 10, 4]`
/// ("61.4") and `33148` -> `[3, 3, 1, 4, 10, 8]` ("3314.8").
///
/// Each decimal digit is extracted separately. The previous implementation
/// pushed `integer / 100` as a glyph index, so any rate at or above 1000 (which
/// the uncapped menu reaches easily) produced an out-of-range index that was
/// clamped to the `.` glyph — the counter then read like `.33.8` or `50.3.1`.
fn fps_glyphs(fps10: u32) -> Vec<usize> {
    let integer = fps10 / 10;
    let frac = fps10 % 10;

    // Decimal digits of the integer part, least significant first.
    let mut digits = [0u8; 10]; // u32::MAX has ten digits
    let mut n = 0usize;
    let mut v = integer;
    loop {
        digits[n] = (v % 10) as u8;
        v /= 10;
        n += 1;
        if v == 0 || n == digits.len() {
            break;
        }
    }

    let mut chars: Vec<usize> = Vec::with_capacity(n + 2);
    for i in (0..n).rev() {
        chars.push(digits[i] as usize);
    }
    chars.push(10); // '.'
    chars.push(frac as usize);
    chars
}

/// Rasterise an FPS string (e.g. "61.4") into a yellow RGBA pixel buffer at 2×
/// scale. Returns (pixels, pixel_width, pixel_height).
pub fn rasterise_fps(fps10: u32) -> (Vec<u8>, i32, i32) {
    let chars = fps_glyphs(fps10);

    let scale = 2i32;
    let pw = (chars.len() as i32) * CHAR_W * scale;
    let ph = CHAR_H * scale;
    let mut pixels = vec![0u8; (pw * ph * 4) as usize];

    for (ci, &ch) in chars.iter().enumerate() {
        let glyph = &FONT5X7[ch];
        for row in 0..7i32 {
            let byte = glyph[row as usize];
            for col in 0..5i32 {
                let lit = (byte >> (7 - col)) & 1 != 0;
                if !lit {
                    continue;
                }
                // fill scale×scale block
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = ci as i32 * CHAR_W * scale + col * scale + sx;
                        let py = (ph - 1) - (row * scale + sy); // GL bottom-left origin
                        if px < pw && py >= 0 && py < ph {
                            let idx = ((py * pw + px) * 4) as usize;
                            pixels[idx] = 0xff; // R
                            pixels[idx + 1] = 0xff; // G
                            pixels[idx + 2] = 0x00; // B  → yellow
                            pixels[idx + 3] = 0xff; // A
                        }
                    }
                }
            }
        }
    }
    (pixels, pw, ph)
}

/// Rasterise arbitrary ASCII text into an RGBA buffer at `scale`×. `rgb` is
/// 0x00RRGGBB; the alpha is always opaque. Lowercase renders as uppercase and
/// characters outside printable ASCII become spaces.
pub fn rasterise_text(text: &str, rgb: u32, scale: i32) -> (Vec<u8>, i32, i32) {
    let chars: Vec<char> = text.chars().collect();
    let pw = (chars.len() as i32) * CHAR_W * scale;
    let ph = CHAR_H * scale;
    let mut pixels = vec![0u8; (pw * ph * 4) as usize];
    let (r, g, b) = (
        ((rgb >> 16) & 0xff) as u8,
        ((rgb >> 8) & 0xff) as u8,
        (rgb & 0xff) as u8,
    );

    for (ci, &ch) in chars.iter().enumerate() {
        let glyph = &FONT_ASCII[ascii_index(ch)];
        for row in 0..7i32 {
            let byte = glyph[row as usize];
            for col in 0..5i32 {
                let lit = (byte >> (7 - col)) & 1 != 0;
                if !lit {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = ci as i32 * CHAR_W * scale + col * scale + sx;
                        let py = (ph - 1) - (row * scale + sy); // GL bottom-left origin
                        if px < pw && py >= 0 && py < ph {
                            let idx = ((py * pw + px) * 4) as usize;
                            pixels[idx] = r;
                            pixels[idx + 1] = g;
                            pixels[idx + 2] = b;
                            pixels[idx + 3] = 0xff;
                        }
                    }
                }
            }
        }
    }
    (pixels, pw, ph)
}

/// Draw an RGBA pixel buffer at `(x, y_from_top)` in physical pixels.
/// Saves and restores all GL state we touch.
pub unsafe fn draw_rgba_overlay(
    pixels: &[u8],
    pw: i32,
    ph: i32,
    x: i32,
    y_from_top: i32,
    phys_w: i32,
    phys_h: i32,
) {
    unsafe {
        let real_viewport = REAL_GL_VIEWPORT.load(Ordering::Relaxed);
        let real_draw_pixels = REAL_GL_DRAW_PIXELS.load(Ordering::Relaxed);
        let real_window_pos = gl_proc(b"glWindowPos2i\0");
        if real_viewport == 0 || real_draw_pixels == 0 || real_window_pos == 0 {
            return;
        }
        if pw <= 0 || ph <= 0 || pixels.len() < (pw * ph * 4) as usize {
            return;
        }

        let vp: unsafe extern "system" fn(i32, i32, i32, i32) = core::mem::transmute(real_viewport);
        let dp: unsafe extern "system" fn(i32, i32, u32, u32, *const u8) =
            core::mem::transmute(real_draw_pixels);
        let wp: unsafe extern "system" fn(i32, i32) = core::mem::transmute(real_window_pos);
        let Some(_server) = save_server_attributes() else {
            return;
        };
        let Some(_client) = tight_pixel_store(false) else {
            return;
        };
        let _framebuffer = default_draw_framebuffer();
        let _program = unbind(gl_proc(b"glUseProgram\0"), 0x8b8d);
        let _unpack = if pixel_buffers_supported() {
            bind_zero(buffer_binding_proc(), 0x88ec, 0x88ef) // GL_PIXEL_UNPACK_BUFFER
        } else {
            None
        };

        let disable: unsafe extern "system" fn(u32) =
            core::mem::transmute(REAL_GL_DISABLE.load(Ordering::Relaxed));
        for state in [
            0x0b71, 0x0bc0, 0x0be2, 0x0b90, 0x0b50, 0x0de1, 0x0c11, 0x0b60, 0x0bf2,
        ] {
            disable(state);
        }
        vp(0, 0, phys_w, phys_h);
        wp(x, phys_h - ph - y_from_top);
        dp(pw, ph, 0x1908, 0x1401, pixels.as_ptr());
    }
}

/// Draw the FPS counter into the top-right corner.
pub unsafe fn draw_fps_overlay(fps10: u32, phys_w: i32, phys_h: i32) {
    unsafe {
        let cache =
            FPS_OVERLAY_CACHE.get_or_init(|| std::sync::Mutex::new((u32::MAX, Vec::new(), 0, 0)));
        if let Ok(mut guard) = cache.try_lock() {
            if guard.0 != fps10 {
                let (px, w, h) = rasterise_fps(fps10);
                *guard = (fps10, px, w, h);
            }
            let (pw, ph) = (guard.2, guard.3);
            if pw > 0 && ph > 0 && !guard.1.is_empty() {
                draw_rgba_overlay(&guard.1, pw, ph, phys_w - pw - 8, 8, phys_w, phys_h);
            }
        }
    }
}

/// Draw arbitrary text at `(x, y_from_top)` in physical pixels. Used by the
/// `coke_fps` sample mod through the host API, and available to future mods.
pub unsafe fn draw_text_overlay(
    x: i32,
    y_from_top: i32,
    text: &str,
    rgb: u32,
    phys_w: i32,
    phys_h: i32,
) {
    unsafe {
        let (pixels, pw, ph) = rasterise_text(text, rgb, 2);
        draw_rgba_overlay(&pixels, pw, ph, x, y_from_top, phys_w, phys_h);
    }
}

// ------------------------------------------------------- mod world triangles

#[repr(C)]
#[derive(Clone, Copy)]
struct ScreenVertex {
    x: f32,
    y: f32,
    rgba: u32,
}

static WORLD_DRAW_LOGGED: AtomicU32 = AtomicU32::new(0);
pub(crate) struct Restore<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Drop for Restore<F> {
    fn drop(&mut self) {
        if let Some(restore) = self.0.take() {
            restore();
        }
    }
}

unsafe fn save_server_attributes() -> Option<Restore<impl FnOnce()>> {
    if gl_integer(0x0bb0) >= gl_integer(0x0d35) {
        return None;
    }
    let push: unsafe extern "system" fn(u32) =
        unsafe { core::mem::transmute(REAL_GL_PUSH_ATTRIB.load(Ordering::Relaxed)) };
    let pop: unsafe extern "system" fn() =
        unsafe { core::mem::transmute(REAL_GL_POP_ATTRIB.load(Ordering::Relaxed)) };
    unsafe { push(0x000f_ffff) }; // GL_ALL_ATTRIB_BITS
    Some(Restore(Some(move || unsafe { pop() })))
}

unsafe fn save_client_attributes(mask: u32) -> Option<Restore<impl FnOnce()>> {
    if gl_integer(0x0bb1) >= gl_integer(0x0d3b) {
        return None;
    }
    // The generated export thunks are declared as diverging. Internal calls
    // need the real OpenGL functions with their returning signatures.
    let push: unsafe extern "system" fn(u32) =
        unsafe { core::mem::transmute(REAL_GL_PUSH_CLIENT_ATTRIB.load(Ordering::Relaxed)) };
    let pop: unsafe extern "system" fn() =
        unsafe { core::mem::transmute(REAL_GL_POP_CLIENT_ATTRIB.load(Ordering::Relaxed)) };
    unsafe { push(mask) };
    Some(Restore(Some(move || unsafe { pop() })))
}

pub(crate) unsafe fn tight_pixel_store(pack: bool) -> Option<Restore<impl FnOnce()>> {
    let guard = unsafe { save_client_attributes(0x1) }?; // GL_CLIENT_PIXEL_STORE_BIT
    let pixel_store: unsafe extern "system" fn(u32, i32) =
        unsafe { core::mem::transmute(REAL_GL_PIXEL_STORE_I.load(Ordering::Relaxed)) };
    let first = if pack { 0x0d00 } else { 0x0cf0 };
    for (offset, value) in [0, 0, 0, 0, 0, 1].into_iter().enumerate() {
        unsafe { pixel_store(first + offset as u32, value) };
    }
    Some(guard)
}

unsafe fn default_draw_framebuffer() -> Option<Restore<impl FnOnce()>> {
    let (bind, target) = framebuffer_proc()?;
    let previous = gl_integer(0x8ca6) as u32; // GL_DRAW_FRAMEBUFFER_BINDING
    if previous == 0 {
        return None;
    }
    unsafe { bind(target, 0) };
    Some(Restore(Some(move || unsafe { bind(target, previous) })))
}

fn buffer_binding_proc() -> usize {
    let core = gl_proc(b"glBindBuffer\0");
    if core != 0 {
        core
    } else {
        gl_proc(b"glBindBufferARB\0")
    }
}

unsafe fn bind_zero(address: usize, target: u32, binding: u32) -> Option<Restore<impl FnOnce()>> {
    if address == 0 {
        return None;
    }
    let previous = gl_integer(binding) as u32;
    let bind: unsafe extern "system" fn(u32, u32) = unsafe { core::mem::transmute(address) };
    if previous != 0 {
        unsafe { bind(target, 0) };
    }
    Some(Restore(Some(move || unsafe { bind(target, previous) })))
}

unsafe fn unbind(address: usize, binding: u32) -> Option<Restore<impl FnOnce()>> {
    if address == 0 {
        return None;
    }
    let previous = gl_integer(binding) as u32;
    let bind: unsafe extern "system" fn(u32) = unsafe { core::mem::transmute(address) };
    if previous != 0 {
        unsafe { bind(0) };
    }
    Some(Restore(Some(move || unsafe { bind(previous) })))
}

/// Physical-pixel rectangle used for world overlays. The proxy makes the engine
/// render at the client size, so the GL viewport is the whole window. Using the
/// scene-widget rect clipped the fan (the HUD got painted over it) and skewed
/// its aspect when that rect was stale.
pub unsafe fn overlay_rect(pw: i32, ph: i32) -> [i32; 4] {
    unsafe {
        if config().los_scene_rect
            && let Some(r) = scene_rect().filter(|r| r[2] >= 8 && r[3] >= 8)
        {
            return r;
        }
        [0, 0, pw, ph]
    }
}

fn project_tris(verts: &[WorldVertex], cam: &Camera, rect: [i32; 4]) -> Vec<ScreenVertex> {
    let proj = projection(cam, rect);
    let p = |v: WorldVertex| {
        proj.project(Vec3 {
            x: v.x,
            y: v.y,
            z: v.z,
        })
    };
    let mut out: Vec<ScreenVertex> = Vec::with_capacity(verts.len());
    let mut i = 0usize;
    while i + 2 < verts.len() {
        if let (Some(a), Some(b), Some(c)) = (p(verts[i]), p(verts[i + 1]), p(verts[i + 2])) {
            out.push(ScreenVertex {
                x: a.0,
                y: a.1,
                rgba: verts[i].rgba,
            });
            out.push(ScreenVertex {
                x: b.0,
                y: b.1,
                rgba: verts[i + 1].rgba,
            });
            out.push(ScreenVertex {
                x: c.0,
                y: c.1,
                rgba: verts[i + 2].rgba,
            });
        }
        i += 3;
    }
    out
}

/// Expand world-space segment pairs into screen-space quads of constant pixel
/// width, so the overlay keeps a clean line weight at every distance.
fn project_lines(
    verts: &[WorldVertex],
    cam: &Camera,
    rect: [i32; 4],
    width: f32,
) -> Vec<ScreenVertex> {
    let half = (width * 0.5).max(0.5);
    let proj = projection(cam, rect);
    let p = |v: WorldVertex| {
        proj.project(Vec3 {
            x: v.x,
            y: v.y,
            z: v.z,
        })
    };
    let mut out: Vec<ScreenVertex> = Vec::with_capacity(verts.len() * 3);
    let mut i = 0usize;
    while i + 1 < verts.len() {
        if let (Some(a), Some(b)) = (p(verts[i]), p(verts[i + 1])) {
            let dx = b.0 - a.0;
            let dy = b.1 - a.1;
            let len = (dx * dx + dy * dy).sqrt();
            if len >= 0.01 {
                let nx = -dy / len * half;
                let ny = dx / len * half;
                let rgba = verts[i].rgba;
                out.push(ScreenVertex {
                    x: a.0 + nx,
                    y: a.1 + ny,
                    rgba,
                });
                out.push(ScreenVertex {
                    x: a.0 - nx,
                    y: a.1 - ny,
                    rgba,
                });
                out.push(ScreenVertex {
                    x: b.0 - nx,
                    y: b.1 - ny,
                    rgba,
                });
                out.push(ScreenVertex {
                    x: a.0 + nx,
                    y: a.1 + ny,
                    rgba,
                });
                out.push(ScreenVertex {
                    x: b.0 - nx,
                    y: b.1 - ny,
                    rgba,
                });
                out.push(ScreenVertex {
                    x: b.0 + nx,
                    y: b.1 + ny,
                    rgba,
                });
            }
        }
        i += 2;
    }
    out
}

/// World-space triangles (fills, markers) drawn over the completed frame.
pub unsafe fn draw_world_triangles(verts: &[WorldVertex]) {
    unsafe {
        if verts.is_empty() || !verts.len().is_multiple_of(3) {
            return;
        }
        let Some(cam) = stable_camera() else { return };
        let pw = PHYS_W.load(Ordering::Relaxed);
        let ph = PHYS_H.load(Ordering::Relaxed);
        if pw <= 0 || ph <= 0 {
            return;
        }
        let rect = overlay_rect(pw, ph);
        let mut screen = project_tris(verts, &cam, rect);
        if screen.is_empty() {
            // Some screens store the camera's look direction negated. Try the
            // opposite once before giving up; this is the common first-run failure.
            let mut flipped = cam;
            flipped.dir = Vec3 {
                x: -cam.dir.x,
                y: -cam.dir.y,
                z: -cam.dir.z,
            };
            screen = project_tris(verts, &flipped, rect);
        }
        if screen.is_empty() {
            if WORLD_DRAW_LOGGED.swap(1, Ordering::Relaxed) == 0 {
                log(&format!(
                    "cokelos: no visible triangles (cam=({:.1},{:.1},{:.1}) dir=({:.2},{:.2},{:.2}) rect={:?})",
                    cam.pos.x, cam.pos.y, cam.pos.z, cam.dir.x, cam.dir.y, cam.dir.z, rect
                ));
            }
            return;
        }
        draw_screen_quads(&screen, pw, ph);
    }
}

/// World-space line segments (contour, rays) at a constant screen width.
pub unsafe fn draw_world_lines(verts: &[WorldVertex], width: f32) {
    unsafe {
        if verts.len() < 2 || !verts.len().is_multiple_of(2) {
            return;
        }
        let Some(cam) = stable_camera() else { return };
        let pw = PHYS_W.load(Ordering::Relaxed);
        let ph = PHYS_H.load(Ordering::Relaxed);
        if pw <= 0 || ph <= 0 {
            return;
        }
        let rect = overlay_rect(pw, ph);
        let w = width.clamp(0.5, 16.0);
        let mut screen = project_lines(verts, &cam, rect, w);
        if screen.is_empty() {
            let mut flipped = cam;
            flipped.dir = Vec3 {
                x: -cam.dir.x,
                y: -cam.dir.y,
                z: -cam.dir.z,
            };
            screen = project_lines(verts, &flipped, rect, w);
        }
        if screen.is_empty() {
            if WORLD_DRAW_LOGGED.swap(1, Ordering::Relaxed) == 0 {
                log(&format!(
                    "cokelos: no visible lines (cam=({:.1},{:.1},{:.1}) dir=({:.2},{:.2},{:.2}) rect={:?})",
                    cam.pos.x, cam.pos.y, cam.pos.z, cam.dir.x, cam.dir.y, cam.dir.z, rect
                ));
            }
            return;
        }
        draw_screen_quads(&screen, pw, ph);
    }
}

/// Submit already-projected triangles. Draws into the default framebuffer even
/// when the engine left an offscreen target bound, so the overlay cannot
/// silently land in an FBO and vanish.
unsafe fn draw_screen_quads(screen: &[ScreenVertex], pw: i32, ph: i32) {
    unsafe {
        if screen.is_empty() || !screen.len().is_multiple_of(3) {
            return;
        }
        let (Some(real_vp), Some(real_mm), Some(real_li), Some(real_pm), Some(real_popm)) = (
            Some(REAL_GL_VIEWPORT.load(Ordering::Relaxed)),
            Some(REAL_GL_MATRIX_MODE.load(Ordering::Relaxed)),
            Some(REAL_GL_LOAD_IDENTITY.load(Ordering::Relaxed)),
            Some(REAL_GL_PUSH_MATRIX.load(Ordering::Relaxed)),
            Some(REAL_GL_POP_MATRIX.load(Ordering::Relaxed)),
        ) else {
            return;
        };
        let real_ortho = REAL_GL_ORTHO.load(Ordering::Relaxed);
        let real_vptr = REAL_GL_VERTEX_POINTER.load(Ordering::Relaxed);
        let real_cptr = REAL_GL_COLOR_POINTER.load(Ordering::Relaxed);
        let real_draw = REAL_GL_DRAW_ARRAYS.load(Ordering::Relaxed);
        let real_ecs = REAL_GL_ENABLE_CLIENT_STATE.load(Ordering::Relaxed);
        let real_dcs = REAL_GL_DISABLE_CLIENT_STATE.load(Ordering::Relaxed);
        let real_blend = REAL_GL_BLEND_FUNC.load(Ordering::Relaxed);
        let real_enable = REAL_GL_ENABLE.load(Ordering::Relaxed);
        let real_disable = REAL_GL_DISABLE.load(Ordering::Relaxed);
        if real_vp == 0
            || real_mm == 0
            || real_li == 0
            || real_pm == 0
            || real_popm == 0
            || real_ortho == 0
            || real_vptr == 0
            || real_cptr == 0
            || real_draw == 0
            || real_ecs == 0
            || real_dcs == 0
        {
            return;
        }

        let vp: unsafe extern "system" fn(i32, i32, i32, i32) = core::mem::transmute(real_vp);
        let mm: unsafe extern "system" fn(u32) = core::mem::transmute(real_mm);
        let li: unsafe extern "system" fn() = core::mem::transmute(real_li);
        let pushm: unsafe extern "system" fn() = core::mem::transmute(real_pm);
        let popm: unsafe extern "system" fn() = core::mem::transmute(real_popm);
        let ortho: unsafe extern "system" fn(f64, f64, f64, f64, f64, f64) =
            core::mem::transmute(real_ortho);
        let vptr: unsafe extern "system" fn(i32, u32, i32, *const core::ffi::c_void) =
            core::mem::transmute(real_vptr);
        let cptr: unsafe extern "system" fn(i32, u32, i32, *const core::ffi::c_void) =
            core::mem::transmute(real_cptr);
        let draw: unsafe extern "system" fn(u32, i32, i32) = core::mem::transmute(real_draw);
        let ecs: unsafe extern "system" fn(u32) = core::mem::transmute(real_ecs);
        let dcs: unsafe extern "system" fn(u32) = core::mem::transmute(real_dcs);
        let enable: unsafe extern "system" fn(u32) = core::mem::transmute(real_enable);
        let disable: unsafe extern "system" fn(u32) = core::mem::transmute(real_disable);

        let Some(_server) = save_server_attributes() else {
            return;
        };
        let _framebuffer = default_draw_framebuffer();
        let _program = unbind(gl_proc(b"glUseProgram\0"), 0x8b8d);
        let _vao = unbind(gl_proc(b"glBindVertexArray\0"), 0x85b5);
        let Some(_client) = save_client_attributes(0x2) else {
            return;
        }; // GL_CLIENT_VERTEX_ARRAY_BIT
        let _array_buffer = bind_zero(buffer_binding_proc(), 0x8892, 0x8894);
        if gl_integer(0x0ba3) >= gl_integer(0x0d36) // modelview stack
            || gl_integer(0x0ba4) >= gl_integer(0x0d38)
        // projection stack
        {
            return;
        }
        mm(GL_PROJECTION);
        pushm();
        li();
        // Top-left origin, matching the projected coordinates.
        ortho(0.0, pw as f64, ph as f64, 0.0, -1.0, 1.0);
        mm(GL_MODELVIEW);
        pushm();
        li();
        vp(0, 0, pw, ph);

        disable(GL_DEPTH_TEST);
        disable(GL_ALPHA_TEST);
        disable(GL_LIGHTING);
        disable(GL_TEXTURE_2D);
        disable(GL_CULL_FACE);
        // Clip off the bottom HUD instead of painting the fan over the command
        // panel. GL scissor uses a bottom-left origin, so the excluded strip is the
        // scissor's y offset.
        let clip_pct = config().los_clip_bottom_pct.min(40);
        let clip_bottom = (ph * clip_pct as i32) / 100;
        if clip_pct > 0 && clip_bottom > 0 {
            let real_scissor = REAL_GL_SCISSOR.load(Ordering::Relaxed);
            if real_scissor != 0 {
                enable(GL_SCISSOR_TEST);
                let sc: unsafe extern "system" fn(i32, i32, i32, i32) =
                    core::mem::transmute(real_scissor);
                sc(0, clip_bottom, pw, (ph - clip_bottom).max(1));
            }
        } else {
            disable(GL_SCISSOR_TEST);
        }
        enable(GL_BLEND);
        if real_blend != 0 {
            let blend: unsafe extern "system" fn(u32, u32) = core::mem::transmute(real_blend);
            blend(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
        }

        let depth_mask: unsafe extern "system" fn(u8) =
            core::mem::transmute(REAL_GL_DEPTH_MASK.load(Ordering::Relaxed));
        depth_mask(0);
        for array in [0x8075, 0x8077, 0x8079] {
            // normal, index, edge flag
            dcs(array);
        }
        let client_active = gl_proc(b"glClientActiveTexture\0");
        if client_active != 0 {
            let active: unsafe extern "system" fn(u32) = core::mem::transmute(client_active);
            for unit in 0..gl_integer(0x84e2).clamp(1, 32) as u32 {
                active(0x84c0 + unit);
                dcs(0x8078); // GL_TEXTURE_COORD_ARRAY
            }
            active(0x84c0);
        } else {
            dcs(0x8078);
        }
        if gl_proc(b"glSecondaryColorPointer\0") != 0 {
            dcs(0x845e); // GL_SECONDARY_COLOR_ARRAY
        }
        if gl_proc(b"glFogCoordPointer\0") != 0 {
            dcs(0x8457); // GL_FOG_COORD_ARRAY
        }

        ecs(GL_VERTEX_ARRAY);
        ecs(GL_COLOR_ARRAY);
        vptr(
            2,
            GL_FLOAT,
            core::mem::size_of::<ScreenVertex>() as i32,
            screen.as_ptr() as *const _,
        );
        cptr(
            4,
            GL_UNSIGNED_BYTE,
            core::mem::size_of::<ScreenVertex>() as i32,
            screen.as_ptr().cast::<u8>().add(8) as *const _,
        );
        draw(GL_TRIANGLES, 0, screen.len() as i32);
        popm();
        mm(GL_PROJECTION);
        popm();
    }
}

pub static REAL_GL_GET_BOOLEANV: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_GET_FLOATV: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_GET_INTEGERV: AtomicUsize = AtomicUsize::new(0);

/// Cached FPS overlay pixel buffer. Regenerated only when fps10 changes.
pub static FPS_OVERLAY_CACHE: OnceLock<std::sync::Mutex<(u32, Vec<u8>, i32, i32)>> =
    OnceLock::new();

pub static LAST_SWAP_SCALE_X: AtomicU32 = AtomicU32::new(0);
pub static LAST_SWAP_SCALE_Y: AtomicU32 = AtomicU32::new(0);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_glyphs_keep_every_decimal_digit() {
        assert_eq!(fps_glyphs(0), vec![0, 10, 0]); // "0.0"
        assert_eq!(fps_glyphs(614), vec![6, 1, 10, 4]); // "61.4"
        assert_eq!(fps_glyphs(503), vec![5, 0, 10, 3]); // "50.3"
        assert_eq!(fps_glyphs(33148), vec![3, 3, 1, 4, 10, 8]); // "3314.8"
        assert_eq!(fps_glyphs(123456), vec![1, 2, 3, 4, 5, 10, 6]);
    }

    #[test]
    fn fps_rasteriser_only_uses_digits_and_the_separator() {
        for fps10 in [0u32, 9, 10, 99, 100, 503, 999, 1000, 26449, 33148, 100000] {
            let glyphs = fps_glyphs(fps10);
            assert!(
                glyphs.iter().all(|&g| g <= 10),
                "{fps10}: glyph index out of the digit/dot range: {glyphs:?}"
            );
            // Exactly one separator, and it is not the leading character.
            assert_eq!(glyphs.iter().filter(|&&g| g == 10).count(), 1);
            assert_ne!(glyphs[0], 10);

            let (pixels, pw, ph) = rasterise_fps(fps10);
            assert_eq!(pw, glyphs.len() as i32 * CHAR_W * 2);
            assert_eq!(ph, CHAR_H * 2);
            assert_eq!(pixels.len(), (pw * ph * 4) as usize);
        }
    }
}

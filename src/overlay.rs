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
        let real_window_pos = REAL_GL_WINDOW_POS.load(Ordering::Relaxed);
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

        let gl_disable = |id: u32| {
            let f = REAL_GL_DISABLE.load(Ordering::Relaxed);
            if f != 0 {
                let f: unsafe extern "system" fn(u32) = core::mem::transmute(f);
                f(id);
            }
        };
        let gl_enable = |id: u32| {
            let f = REAL_GL_ENABLE.load(Ordering::Relaxed);
            if f != 0 {
                let f: unsafe extern "system" fn(u32) = core::mem::transmute(f);
                f(id);
            }
        };

        // Query which of the states we touch are currently enabled, so we can
        // restore them exactly rather than blindly turning things back on.
        let mut was_depth: u8 = 0;
        let mut was_alpha: u8 = 0;
        let mut was_blend: u8 = 0;
        let mut was_stencil: u8 = 0;
        let mut was_lighting: u8 = 0;
        let mut was_texture: u8 = 0;
        let mut was_scissor: u8 = 0;
        let mut was_fog: u8 = 0;
        let mut was_logic: u8 = 0;
        let f_getb = REAL_GL_GET_BOOLEANV.load(Ordering::Relaxed);
        if f_getb != 0 {
            let getb: unsafe extern "system" fn(u32, *mut u8) = core::mem::transmute(f_getb);
            getb(0x0B71, &mut was_depth); // GL_DEPTH_TEST
            getb(0x0BC0, &mut was_alpha); // GL_ALPHA_TEST
            getb(0x0BE2, &mut was_blend); // GL_BLEND
            getb(0x0B90, &mut was_stencil); // GL_STENCIL_TEST
            getb(0x0B50, &mut was_lighting); // GL_LIGHTING
            getb(0x0DE1, &mut was_texture); // GL_TEXTURE_2D
            getb(0x0C11, &mut was_scissor); // GL_SCISSOR_TEST
            getb(0x0B60, &mut was_fog); // GL_FOG
            getb(0x0BF2, &mut was_logic); // GL_COLOR_LOGIC_OP
        }

        // Save current viewport.
        let mut saved_vp = [0i32; 4];
        let f_geti = REAL_GL_GET_INTEGERV.load(Ordering::Relaxed);
        if f_geti != 0 {
            let geti: unsafe extern "system" fn(u32, *mut i32) = core::mem::transmute(f_geti);
            geti(0x0BA2, saved_vp.as_mut_ptr()); // GL_VIEWPORT
        }

        // The engine may leave an offscreen render target bound at swap time. The
        // text pipeline uses glDrawPixels, which would silently land in that FBO
        // and never reach the screen (this is what made the FPS counter disappear
        // when a world-overlay mod was active). Bind the default framebuffer for
        // the draw and restore the engine's binding afterwards.
        let bind_fbo = REAL_BIND_FRAMEBUFFER.load(Ordering::Relaxed);
        let mut prev_fbo = 0i32;
        if f_geti != 0 {
            let geti: unsafe extern "system" fn(u32, *mut i32) = core::mem::transmute(f_geti);
            geti(0x8CA6, &mut prev_fbo); // GL_FRAMEBUFFER_BINDING
        }
        if prev_fbo != 0 && bind_fbo != 0 {
            let f: unsafe extern "system" fn(u32, u32) = core::mem::transmute(bind_fbo);
            f(0x8D40, 0); // GL_FRAMEBUFFER
        }

        // Set up clean state for drawing.
        gl_disable(0x0B71); // GL_DEPTH_TEST
        gl_disable(0x0BC0); // GL_ALPHA_TEST
        gl_disable(0x0BE2); // GL_BLEND
        gl_disable(0x0B90); // GL_STENCIL_TEST
        gl_disable(0x0B50); // GL_LIGHTING
        gl_disable(0x0DE1); // GL_TEXTURE_2D
        gl_disable(0x0C11); // GL_SCISSOR_TEST — it would clip the overlay
        gl_disable(0x0B60); // GL_FOG — can tint the text into the background
        gl_disable(0x0BF2); // GL_COLOR_LOGIC_OP — XOR/AND can erase the text

        // Full physical viewport so glWindowPos coords map correctly.
        vp(0, 0, phys_w, phys_h);

        // glWindowPos uses a bottom-left origin; convert the requested top-left y.
        wp(x, phys_h - ph - y_from_top);
        dp(pw, ph, 0x1908, 0x1401, pixels.as_ptr()); // GL_RGBA, GL_UNSIGNED_BYTE

        // Restore saved state.
        if was_depth != 0 {
            gl_enable(0x0B71);
        } else {
            gl_disable(0x0B71);
        }
        if was_alpha != 0 {
            gl_enable(0x0BC0);
        } else {
            gl_disable(0x0BC0);
        }
        if was_blend != 0 {
            gl_enable(0x0BE2);
        } else {
            gl_disable(0x0BE2);
        }
        if was_stencil != 0 {
            gl_enable(0x0B90);
        } else {
            gl_disable(0x0B90);
        }
        if was_lighting != 0 {
            gl_enable(0x0B50);
        } else {
            gl_disable(0x0B50);
        }
        if was_texture != 0 {
            gl_enable(0x0DE1);
        } else {
            gl_disable(0x0DE1);
        }
        if was_scissor != 0 {
            gl_enable(0x0C11);
        } else {
            gl_disable(0x0C11);
        }
        if was_fog != 0 {
            gl_enable(0x0B60);
        } else {
            gl_disable(0x0B60);
        }
        if was_logic != 0 {
            gl_enable(0x0BF2);
        } else {
            gl_disable(0x0BF2);
        }
        if prev_fbo != 0 && bind_fbo != 0 {
            let f: unsafe extern "system" fn(u32, u32) = core::mem::transmute(bind_fbo);
            f(0x8D40, prev_fbo as u32);
        }
        if f_geti != 0 {
            vp(saved_vp[0], saved_vp[1], saved_vp[2], saved_vp[3]);
        }
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
static GL_USE_PROGRAM: AtomicUsize = AtomicUsize::new(usize::MAX); // MAX = not tried
static GL_BIND_VAO: AtomicUsize = AtomicUsize::new(usize::MAX);

unsafe fn wgl_proc(name: &[u8]) -> usize {
    unsafe {
        let wgl = REAL_WGL_GET_PROC_ADDRESS.load(Ordering::Relaxed);
        if wgl == 0 {
            return 0;
        }
        let f: unsafe extern "system" fn(*const std::ffi::c_char) -> *mut core::ffi::c_void =
            core::mem::transmute(wgl);
        f(name.as_ptr() as *const std::ffi::c_char) as usize
    }
}

/// Resolve `glUseProgram` lazily (it is a wgl extension, not an opengl32 export).
unsafe fn use_program_ptr() -> usize {
    unsafe {
        let cached = GL_USE_PROGRAM.load(Ordering::Relaxed);
        if cached != usize::MAX {
            return cached;
        }
        let p = wgl_proc(b"glUseProgram\0");
        GL_USE_PROGRAM.store(p, Ordering::Relaxed);
        p
    }
}

/// Resolve `glBindVertexArray` so client arrays work even if the engine left a
/// VAO bound.
unsafe fn bind_vao_ptr() -> usize {
    unsafe {
        let cached = GL_BIND_VAO.load(Ordering::Relaxed);
        if cached != usize::MAX {
            return cached;
        }
        let p = wgl_proc(b"glBindVertexArray\0");
        GL_BIND_VAO.store(p, Ordering::Relaxed);
        p
    }
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
        let real_push = REAL_GL_PUSH_ATTRIB.load(Ordering::Relaxed);
        let real_pop = REAL_GL_POP_ATTRIB.load(Ordering::Relaxed);
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

        // The engine may leave an offscreen render target bound at swap time. Query
        // the real binding instead of trusting the tracked value: restoring a stale
        // id could send the engine's next frame into a dead framebuffer (flicker).
        let bind_fbo = REAL_BIND_FRAMEBUFFER.load(Ordering::Relaxed);
        let fbo: Option<unsafe extern "system" fn(u32, u32)> = if bind_fbo != 0 {
            Some(core::mem::transmute::<
                usize,
                unsafe extern "system" fn(u32, u32),
            >(bind_fbo))
        } else {
            None
        };
        let mut prev_fbo = 0i32;
        {
            let geti = REAL_GL_GET_INTEGERV.load(Ordering::Relaxed);
            if geti != 0 {
                let g: unsafe extern "system" fn(u32, *mut i32) = core::mem::transmute(geti);
                g(0x8CA6, &mut prev_fbo); // GL_FRAMEBUFFER_BINDING
            }
        }
        if prev_fbo != 0
            && let Some(f) = fbo
        {
            f(0x8D40, 0); // GL_FRAMEBUFFER
        }

        if real_push != 0 {
            let push: unsafe extern "system" fn(u32) = core::mem::transmute(real_push);
            push(0x000f_ffff); // GL_ALL_ATTRIB_BITS
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

        // With the depth test disabled, fragments still write depth. That can
        // corrupt the engine's next frame (water planes, early-z, LOD), so turn
        // writes off and restore the previous mask.
        let dm = REAL_GL_DEPTH_MASK.load(Ordering::Relaxed);
        let depth_mask: Option<unsafe extern "system" fn(u8)> = if dm != 0 {
            Some(core::mem::transmute::<usize, unsafe extern "system" fn(u8)>(dm))
        } else {
            None
        };
        let mut prev_mask: u8 = 1;
        {
            let getb = REAL_GL_GET_BOOLEANV.load(Ordering::Relaxed);
            if getb != 0 {
                let g: unsafe extern "system" fn(u32, *mut u8) = core::mem::transmute(getb);
                g(0x0B72, &mut prev_mask); // GL_DEPTH_WRITEMASK
            }
        }
        if let Some(f) = depth_mask {
            f(0);
        }

        // Client arrays are offsets into whatever VBO is bound; bind none so the
        // engine's buffer binding cannot turn the overlay into garbage (and restore
        // it afterwards, since attrib push does not cover buffer bindings).
        let bind_buffer = REAL_GL_BIND_BUFFER.load(Ordering::Relaxed);
        let mut prev_buf = 0i32;
        if bind_buffer != 0 {
            let geti = REAL_GL_GET_INTEGERV.load(Ordering::Relaxed);
            if geti != 0 {
                let g: unsafe extern "system" fn(u32, *mut i32) = core::mem::transmute(geti);
                g(0x8894, &mut prev_buf); // GL_ARRAY_BUFFER_BINDING
            }
            if prev_buf != 0 {
                let f: unsafe extern "system" fn(u32, u32) = core::mem::transmute(bind_buffer);
                f(0x8892, 0); // GL_ARRAY_BUFFER
            }
        }

        // The engine's last 3D/UI pass may have left a GLSL program bound; the
        // fixed-function client arrays below are ignored while a program is active.
        let use_prog = use_program_ptr();
        let mut prev_prog = 0i32;
        if use_prog != 0 {
            let geti = REAL_GL_GET_INTEGERV.load(Ordering::Relaxed);
            if geti != 0 {
                let g: unsafe extern "system" fn(u32, *mut i32) = core::mem::transmute(geti);
                g(0x8b8d, &mut prev_prog); // GL_CURRENT_PROGRAM
            }
            let f: unsafe extern "system" fn(u32) = core::mem::transmute(use_prog);
            f(0);
        }
        let bind_vao = bind_vao_ptr();
        let mut prev_vao = 0i32;
        if bind_vao != 0 {
            let geti = REAL_GL_GET_INTEGERV.load(Ordering::Relaxed);
            if geti != 0 {
                let g: unsafe extern "system" fn(u32, *mut i32) = core::mem::transmute(geti);
                g(0x85b5, &mut prev_vao); // GL_VERTEX_ARRAY_BINDING
            }
            let f: unsafe extern "system" fn(u32) = core::mem::transmute(bind_vao);
            f(0);
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
        dcs(GL_VERTEX_ARRAY);
        dcs(GL_COLOR_ARRAY);
        if let Some(f) = depth_mask {
            f(prev_mask);
        }
        if bind_buffer != 0 && prev_buf != 0 {
            let f: unsafe extern "system" fn(u32, u32) = core::mem::transmute(bind_buffer);
            f(0x8892, prev_buf as u32);
        }
        if use_prog != 0 {
            let f: unsafe extern "system" fn(u32) = core::mem::transmute(use_prog);
            f(prev_prog as u32);
        }
        if bind_vao != 0 {
            let f: unsafe extern "system" fn(u32) = core::mem::transmute(bind_vao);
            f(prev_vao as u32);
        }

        if real_pop != 0 {
            let pop: unsafe extern "system" fn() = core::mem::transmute(real_pop);
            pop();
        }
        popm();
        mm(GL_PROJECTION);
        popm();
        mm(GL_MODELVIEW);

        if prev_fbo != 0
            && let Some(f) = fbo
        {
            f(0x8D40, prev_fbo as u32);
        }
    }
}

pub static REAL_GL_GET_BOOLEANV: AtomicUsize = AtomicUsize::new(0);
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

// SPDX-License-Identifier: GPL-3.0-or-later

//! `spec/10` §7's budget for a scrolling capture, measured rather than assumed.
//!
//! > Scroll-assist iteration (capture + match) | < 120 ms per step
//!
//! The capture half is the extension's and the compositor's; this is the match half, at
//! the size of a real selection on the target machine. Run it with
//! `cargo run --release --example budget -p octosnap-stitch`.

// A measurement, not a service: there is no D-Bus boundary here for a panic to cross, and
// a fixture that cannot be built is a broken measurement rather than a broken capture.
#![allow(clippy::expect_used)]

fn main() {
    // Prose is the ordinary page; the even listing is the worst case, where the
    // correlation ties at every line pitch and the pixels are asked about eight of them.
    for even in [false, true] {
        println!("{}:", if even { "an even listing" } else { "a page of prose" });
        run(even);
    }
}

fn run(even: bool) {
    let (w, h) = (1920u32, 1200u32);
    let step = octosnap_stitch::step_for(h);
    let mut frames = Vec::new();
    for i in 0..6u32 {
        frames.push(page(w, h, i * step, even));
    }
    let mut stitcher = octosnap_stitch::Stitcher::new(
        octosnap_stitch::Direction::Down,
        octosnap_stitch::Limits::default(),
    );
    for (i, frame) in frames.into_iter().enumerate() {
        let start = std::time::Instant::now();
        let next = stitcher.push(frame, step).expect("a frame");
        println!("  frame {i}: {:?} in {:?}", next, start.elapsed());
    }
    let start = std::time::Instant::now();
    let composed = stitcher.composed().expect("a canvas");
    println!("  composed {}x{} in {:?}", composed.width(), composed.height(), start.elapsed());
}

fn page(width: u32, height: u32, top: u32, even: bool) -> octosnap_stitch::Frame {
    let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);
    let word = hash(0) % (1 << 16);
    for y in 0..height {
        let row = top + y;
        let within = row % 20;
        let line = row / 20;
        let bits = hash(line);
        for x in 0..width {
            let on = within < 11
                && if even {
                    (word >> ((x / 3 + line) % 16)) & 1 == 1
                } else {
                    (bits >> ((x / 3) % 29)) & 1 == 1 && (x / 3) % 11 != 10
                };
            let tone = if on { 34u8 } else { 246u8 };
            pixels.extend_from_slice(&[tone, tone, tone, 255]);
        }
    }
    octosnap_stitch::Frame::new(width, height, pixels).expect("a frame")
}

fn hash(row: u32) -> u32 {
    let mut x = row.wrapping_mul(0x9E37_79B1).wrapping_add(0x1234_5678);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    x
}

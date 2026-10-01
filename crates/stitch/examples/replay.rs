// SPDX-License-Identifier: GPL-3.0-or-later

//! Replays the frames a scrolling capture kept (`OCTOSNAP_KEEP_FRAMES`) through the
//! stitcher, frame by frame, says what became of each, and writes the result if asked.
//!
//! `cargo run --release --example replay -p octosnap-stitch -- <dir> [down|up|left|right]
//! [both] [result.png]`
//!
//! `both` replays it the way manual mode stitches, following the page either way (D112),
//! and `RUST_LOG=octosnap_stitch=debug` says why each frame went where it did.

#![allow(clippy::expect_used)]

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .without_time()
        .init();
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("a directory of kept frames");
    let (mut direction, mut both, mut result) = (octosnap_stitch::Direction::Down, false, None);
    for arg in args {
        match arg.as_str() {
            "down" => direction = octosnap_stitch::Direction::Down,
            "up" => direction = octosnap_stitch::Direction::Up,
            "left" => direction = octosnap_stitch::Direction::Left,
            "right" => direction = octosnap_stitch::Direction::Right,
            "both" => both = true,
            _ => result = Some(arg),
        }
    }
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("a readable directory")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "png"))
        .collect();
    paths.sort();
    let mut stitcher =
        octosnap_stitch::Stitcher::new(direction, octosnap_stitch::Limits::default());
    if both {
        stitcher = stitcher.both_ways();
    }
    for path in &paths {
        let frame = octosnap_stitch::Frame::read(path).expect("a frame");
        let started = std::time::Instant::now();
        let next = stitcher.push(frame, stitcher.growth()).expect("a frame of the right size");
        let (width, height) = stitcher.size();
        println!(
            "{}: {next:?} -> {width}x{height} growing {:?} in {:?}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            stitcher.growing(),
            started.elapsed()
        );
    }
    if let Some(result) = result {
        let stitched = stitcher.finish().expect("a capture");
        stitched.write(std::path::Path::new(&result)).expect("a writable result");
    }
}

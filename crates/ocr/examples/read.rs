// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads a PNG with the real engine, which is the only way to find out whether it works.
//!
//! ```sh
//! cargo run --release --example read -p octosnap-ocr -- shot.png [latin|cjk|...]
//! ```
//!
//! Needs ONNX Runtime and at least one pack installed -- `$XDG_DATA_HOME/octosnap/ocr`,
//! or wherever `OCTOSNAP_OCR_HOME` points. It prints the two shortcuts' texts and every
//! link found in them, which between them is the whole of `spec/07` §2.1 except the window.

// A spike, not a service: a fixture that cannot be opened is a broken measurement.
#![allow(clippy::expect_used)]

use std::time::Instant;

use octosnap_ocr::{Breaks, Engine, Gray, Read, Reader, Script, engine::Null, rapid::Rapid};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("a path to a PNG");
    let mut script = None;
    let mut rect = None;
    for arg in args {
        if let Some(found) = Script::from_tag(&arg) {
            script = Some(found);
        } else {
            let numbers: Vec<i32> = arg.split(',').filter_map(|n| n.parse().ok()).collect();
            if let [x, y, w, h] = numbers[..] {
                rect = Some(octosnap_ocr::Bounds::new(x, y, w, h));
            }
        }
    }

    let (width, height, rgba) = decode(&path);
    let mut image = Gray::from_rgba(width, height, &rgba).expect("a grey image");
    // A capture of a dark-mode page, without needing one to hand.
    if std::env::var_os("OCTOSNAP_OCR_FLIP").is_some() {
        image = image.inverted();
    }
    if let Some(rect) = rect {
        image = image.crop(rect).expect("a crop inside the image");
    }
    println!(
        "{path}: {}x{}, mean luminance {:.3}",
        image.width(),
        image.height(),
        image.mean()
    );

    let home = std::env::var_os("OCTOSNAP_OCR_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(Rapid::home);
    println!("packs in {}:", home.display());
    match Rapid::open(&home) {
        Ok(engine) => {
            for pack in engine.packs() {
                let state = if pack.installed { "installed" } else { "available" };
                let size = pack.bytes as f64 / 1e6;
                println!("  {:<28} {size:>5.1} MB  {state}", pack.script.name());
            }
            run(&Reader::new(engine), &image, script);
        }
        Err(why) => {
            println!("  no engine: {why}");
            run(&Reader::new(Null), &image, script);
        }
    }
}

fn run<E: Engine>(reader: &Reader<E>, image: &Gray, script: Option<Script>) {
    // The first read pays for loading the weights; `spec/10` §7's 1.5 s budget is the
    // second one, which is what every read after a capture costs.
    let start = Instant::now();
    let read = reader.scan(image, script).expect("a read");
    let cold = start.elapsed();
    let start = Instant::now();
    let read = reader.scan(image, script).unwrap_or(read);
    let warm = start.elapsed();
    match &read {
        Read::Codes(codes) => println!("\n{} codes in {cold:?} / {warm:?}\n", codes.len()),
        Read::Text(shaped) => {
            let lines: usize = shaped.paragraphs.iter().map(|p| p.lines.len()).sum();
            println!(
                "\n{lines} lines in {} paragraphs -- {cold:?} cold, {warm:?} warm (budget 1.5 s)\n",
                shaped.paragraphs.len()
            );
        }
    }

    for breaks in [Breaks::Keep, Breaks::Join] {
        let text = read.text(breaks);
        println!("--- {breaks:?} ---");
        println!("{text}");
        for link in octosnap_ocr::links(&text) {
            println!("  [{:?}] {} -> {}", link.kind, &text[link.range.clone()], link.target);
        }
        println!();
    }
}

fn decode(path: &str) -> (u32, u32, Vec<u8>) {
    let file = std::fs::File::open(path).expect("the file");
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder.read_info().expect("a PNG");
    let mut buffer = vec![0; reader.output_buffer_size().expect("a size")];
    let info = reader.next_frame(&mut buffer).expect("a frame");
    let (width, height) = (info.width, info.height);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buffer[..info.buffer_size()]
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        other => panic!("a {other:?} PNG is not something this spike reads"),
    };
    (width, height, rgba)
}

/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

//! `paged place` — through the binary, because the two things that can
//! break it are both outside the library.
//!
//! **The address form.** `ReplaceImageBytes` matches on the BARE self
//! id while `SetElementProperty` takes the full `kind:id` address, and
//! the lowering answers an unresolvable id with
//! `not implemented: Mutation::ReplaceImageBytes` — a message about the
//! wrong thing entirely. The command reconciles the two the way the Boa
//! bridge does; this pins that it stays reconciled.
//!
//! **The saved file.** A place that mutates the loaded model and never
//! reaches disk looks identical on stdout. So the assertion is on the
//! REOPENED document, not on the session that wrote it.

use std::path::PathBuf;
use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_paged"))
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run paged {args:?}: {e}"))
}

fn paged(args: &[&str]) -> std::process::Output {
    let out = run(args);
    assert!(
        out.status.success(),
        "paged {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn dir(who: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("paged-cli-{who}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

/// A 2×2 red PNG, by hand — no image crate in this test's dependencies,
/// and a fixture file would be one more thing to keep in step.
fn a_png() -> Vec<u8> {
    fn chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        let mut crc_input = kind.to_vec();
        crc_input.extend_from_slice(body);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
        out
    }
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    // Stored (uncompressed) deflate blocks, so the pixels need no
    // encoder: two scanlines of filter-0 + two RGB triples each.
    let raw: Vec<u8> = vec![0, 255, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255, 0, 0];
    let mut z = vec![0x78, 0x01];
    z.push(0x01);
    z.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    z.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    z.extend_from_slice(&raw);
    let mut adler = (1u32, 0u32);
    for b in &raw {
        adler.0 = (adler.0 + u32::from(*b)) % 65521;
        adler.1 = (adler.1 + adler.0) % 65521;
    }
    z.extend_from_slice(&((adler.1 << 16) | adler.0).to_be_bytes());

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = 2u32.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&2u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
    png.extend_from_slice(&chunk(b"IHDR", &ihdr));
    png.extend_from_slice(&chunk(b"IDAT", &z));
    png.extend_from_slice(&chunk(b"IEND", &[]));
    png
}

const MAKE_FRAME: &str = r#"const pid = JSON.parse(paged.pages())[0].selfId;
console.log('FRAME ' + paged.insertFrame(pid, [60, 60, 300, 300]));"#;

#[test]
fn place_puts_an_image_on_the_page_and_the_saved_file_keeps_it() {
    let d = dir("place");
    let (blank, js) = (d.join("blank.paged"), d.join("frame.js"));
    let (framed, placed) = (d.join("framed.paged"), d.join("placed.paged"));
    let img = d.join("red.png");
    std::fs::write(&js, MAKE_FRAME).unwrap();
    std::fs::write(&img, a_png()).unwrap();
    let s = |p: &PathBuf| p.to_str().unwrap().to_string();

    paged(&["new", "--size", "a5", "-o", &s(&blank)]);
    let out = paged(&["script", &s(&blank), &s(&js), "-o", &s(&framed)]);
    let log = String::from_utf8_lossy(&out.stdout);
    let frame = log
        .lines()
        .find_map(|l| l.split("FRAME ").nth(1))
        .unwrap_or_else(|| panic!("no frame minted: {log}"))
        .trim()
        .to_string();
    assert!(
        frame.starts_with("rectangle:"),
        "insertFrame answered {frame:?}"
    );

    // The page before, so the comparison has something to be a change
    // FROM — an empty graphic frame paints nothing, so "the image
    // arrived" and "the page is blank" are otherwise the same picture.
    let before = d.join("before.png");
    paged(&["render", &s(&framed), "--dpi", "72", "-o", &s(&before)]);

    paged(&["place", &s(&framed), &frame, &s(&img), "-o", &s(&placed)]);

    let after = d.join("after.png");
    paged(&["render", &s(&placed), "--dpi", "72", "-o", &s(&after)]);
    let same = run(&["diff", &s(&before), &s(&after)]);
    assert!(
        !same.status.success(),
        "placing an image changed nothing on the page: {}",
        String::from_utf8_lossy(&same.stdout)
    );
}

#[test]
fn a_bad_address_is_refused_before_anything_is_loaded() {
    let d = dir("place-bad");
    let (blank, img) = (d.join("blank.paged"), d.join("red.png"));
    std::fs::write(&img, a_png()).unwrap();
    let s = |p: &PathBuf| p.to_str().unwrap().to_string();
    paged(&["new", "--size", "a5", "-o", &s(&blank)]);

    let out = run(&["place", &s(&blank), "u404", &s(&img)]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not an element address"), "{err}");
}

#[test]
fn a_format_the_engine_cannot_decode_is_refused_rather_than_placed_blank() {
    let d = dir("place-jpx");
    let (blank, img) = (d.join("blank.paged"), d.join("photo.jp2"));
    // Content is irrelevant: the refusal is on the format, and it
    // happens before the file is read.
    std::fs::write(&img, b"not really a jp2").unwrap();
    let s = |p: &PathBuf| p.to_str().unwrap().to_string();
    paged(&["new", "--size", "a5", "-o", &s(&blank)]);

    let out = run(&["place", &s(&blank), "rectangle:u1", &s(&img)]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("Transcode"), "{err}");
}

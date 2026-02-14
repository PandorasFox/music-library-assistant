//! Reproduction cases for lofty 0.23.1 FLAC bugs.
//!
//! Bug 1 (#608): FlacFile save_to_path fails on FLAC with prepended ID3v2.
//!   - save without remove_id3v2: "Attempted to write a tag to a format that does not support it"
//!   - save with remove_id3v2: "File missing fLaC stream marker"
//!
//! Bug 2 (#607): Duplicate Last-metadata-block flags after save with PADDING.
//!   When lofty inserts a PADDING block, it doesn't clear the is_last flag on
//!   the preceding block, producing files with multiple is_last markers. Strict
//!   decoders reject these as malformed.
//!
//! Generates real FLAC files via ffmpeg for both tests.
//! See: https://github.com/Serial-ATA/lofty-rs/pull/609

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::AudioFile;
use lofty::ogg::OggPictureStorage;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::Accessor;
use std::io::{BufReader, Write};
use std::path::Path;

// ============================================================================
// Helpers
// ============================================================================

fn make_real_flac(path: &Path) {
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-y", "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo",
            "-t", "1", "-c:a", "flac",
            path.to_str().unwrap(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("ffmpeg not found");
    assert!(status.success(), "ffmpeg failed");
}

fn prepend_id3v2(path: &Path) {
    let flac_data = std::fs::read(path).unwrap();
    assert_eq!(&flac_data[..4], b"fLaC");

    let mut id3 = Vec::new();
    id3.extend_from_slice(b"ID3");
    id3.extend_from_slice(&[4, 0, 0x00]); // v2.4, no flags

    let mut frames = Vec::new();
    frames.extend_from_slice(b"TIT2");
    let text = b"\x03Set Me On Fire";
    frames.extend_from_slice(&(text.len() as u32).to_be_bytes());
    frames.extend_from_slice(&[0x00, 0x00]);
    frames.extend_from_slice(text);
    frames.extend_from_slice(b"TPE1");
    let text2 = b"\x03Pendulum";
    frames.extend_from_slice(&(text2.len() as u32).to_be_bytes());
    frames.extend_from_slice(&[0x00, 0x00]);
    frames.extend_from_slice(text2);
    frames.extend_from_slice(&[0x00; 1024]); // padding

    let total = frames.len();
    id3.extend_from_slice(&[
        ((total >> 21) & 0x7F) as u8,
        ((total >> 14) & 0x7F) as u8,
        ((total >> 7) & 0x7F) as u8,
        (total & 0x7F) as u8,
    ]);
    id3.extend_from_slice(&frames);

    let mut out = std::fs::File::create(path).unwrap();
    out.write_all(&id3).unwrap();
    out.write_all(&flac_data).unwrap();
    out.flush().unwrap();
    println!("  Prepended {} byte ID3v2", id3.len());
}

fn make_picture() -> Picture {
    // Minimal valid 1x1 RGB PNG
    let png_data: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A,
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
        0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41,
        0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
        0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21, 0xBC,
        0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
        0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    Picture::unchecked(png_data)
        .pic_type(PictureType::CoverFront)
        .mime_type(MimeType::Png)
        .build()
}

/// FLAC metadata block type names for display.
fn block_type_name(t: u8) -> &'static str {
    match t {
        0 => "STREAMINFO",
        1 => "PADDING",
        2 => "APPLICATION",
        3 => "SEEKTABLE",
        4 => "VORBIS_COMMENT",
        5 => "CUESHEET",
        6 => "PICTURE",
        _ => "UNKNOWN",
    }
}

/// Scan FLAC metadata block headers and report is_last flags.
///
/// Returns (total_blocks, number_of_blocks_with_is_last_set, is_last_only_on_final).
/// A well-formed FLAC file should have exactly 1 is_last flag, on the final metadata block.
///
/// IMPORTANT: This scanner does NOT stop at the first is_last=true. It keeps
/// scanning to detect duplicate is_last flags — which is the exact bug we're
/// looking for. A strict FLAC decoder would stop at the first is_last, hiding
/// the duplicate from view.
fn check_is_last_flags(path: &Path) -> (usize, usize, bool) {
    let data = std::fs::read(path).unwrap();

    // Find fLaC marker (skip ID3v2 if present)
    let flac_start = if &data[..3] == b"ID3" {
        let size = ((data[6] as usize & 0x7F) << 21)
            | ((data[7] as usize & 0x7F) << 14)
            | ((data[8] as usize & 0x7F) << 7)
            | (data[9] as usize & 0x7F);
        let mut end = 10 + size;
        if data[5] & 0x10 != 0 { end += 10; }
        end
    } else {
        0
    };

    assert_eq!(&data[flac_start..flac_start + 4], b"fLaC",
        "fLaC marker not found at offset {}", flac_start);

    let mut pos = flac_start + 4; // skip "fLaC"
    let mut blocks = Vec::new();
    let mut saw_first_is_last = false;

    loop {
        if pos + 4 > data.len() { break; }

        let header_byte = data[pos];
        let is_last = (header_byte & 0x80) != 0;
        let block_type = header_byte & 0x7F;
        let block_len = ((data[pos + 1] as usize) << 16)
            | ((data[pos + 2] as usize) << 8)
            | (data[pos + 3] as usize);

        // Sanity: block_type > 126 or block_len extends past file = we've hit audio frames
        if block_type > 126 || pos + 4 + block_len > data.len() {
            if !saw_first_is_last {
                println!("    WARNING: hit audio data without seeing is_last!");
            }
            break;
        }

        blocks.push((block_type, block_len, is_last));
        let marker = if is_last && saw_first_is_last { " ← DUPLICATE" }
            else if is_last { "" } else { "" };
        println!("    block {:2}: type={} ({:16})  len={:6}  is_last={}{}",
            blocks.len() - 1, block_type, block_type_name(block_type),
            block_len, is_last, marker);

        if is_last {
            if saw_first_is_last {
                // Already saw one is_last — this is the bug. Stop here.
                break;
            }
            saw_first_is_last = true;
            // Keep scanning to see if there are more metadata blocks after this
            // (which would indicate the is_last flag was set too early)
        }

        pos += 4 + block_len;
    }

    let total = blocks.len();
    let is_last_count = blocks.iter().filter(|(_, _, last)| *last).count();
    let only_on_final = is_last_count == 1
        && blocks.last().map_or(false, |(_, _, last)| *last);

    (total, is_last_count, only_on_final)
}

// ============================================================================
// Bug #608: ID3v2 in FLAC breaks save_to_path
// ============================================================================

fn test_id3v2_bug() {
    println!("=== BUG #608: ID3v2 in FLAC breaks save_to_path ===\n");

    let clean = Path::new("test_608_clean.flac");
    let dirty = Path::new("test_608_dirty.flac");

    make_real_flac(clean);
    println!("  Clean FLAC: {} bytes\n", std::fs::metadata(clean).unwrap().len());

    // Test 1: save without remove_id3v2 → "Attempted to write a tag..."
    std::fs::copy(clean, dirty).unwrap();
    prepend_id3v2(dirty);
    println!("  Test 1: save_to_path without remove_id3v2");
    {
        let file = std::fs::File::open(dirty).unwrap();
        let mut r = BufReader::new(file);
        let flac = lofty::flac::FlacFile::read_from(&mut r, ParseOptions::default()).unwrap();
        drop(r);
        match flac.save_to_path(dirty, WriteOptions::new().preferred_padding(0)) {
            Ok(()) => println!("  PASS: save_to_path OK"),
            Err(e) => println!("  FAIL: {}", e),
        }
    }

    // Test 2: save with remove_id3v2 + insert_picture → "File missing fLaC..."
    std::fs::copy(clean, dirty).unwrap();
    prepend_id3v2(dirty);
    println!("  Test 2: save_to_path with remove_id3v2 + insert_picture");
    {
        let file = std::fs::File::open(dirty).unwrap();
        let mut r = BufReader::new(file);
        let mut flac = lofty::flac::FlacFile::read_from(&mut r, ParseOptions::default()).unwrap();
        drop(r);
        flac.remove_id3v2();
        let _ = flac.insert_picture(make_picture(), None);
        match flac.save_to_path(dirty, WriteOptions::new().preferred_padding(0)) {
            Ok(()) => println!("  PASS: save_to_path OK"),
            Err(e) => println!("  FAIL: {}", e),
        }
    }

    // Test 3: save with remove_id3v2 only (no picture) → also fails
    std::fs::copy(clean, dirty).unwrap();
    prepend_id3v2(dirty);
    println!("  Test 3: save_to_path with remove_id3v2 only (no picture insert)");
    {
        let file = std::fs::File::open(dirty).unwrap();
        let mut r = BufReader::new(file);
        let mut flac = lofty::flac::FlacFile::read_from(&mut r, ParseOptions::default()).unwrap();
        drop(r);
        flac.remove_id3v2();
        match flac.save_to_path(dirty, WriteOptions::new().preferred_padding(0)) {
            Ok(()) => println!("  PASS: save_to_path OK"),
            Err(e) => println!("  FAIL: {}", e),
        }
    }

    // Cleanup
    let _ = std::fs::remove_file(clean);
    let _ = std::fs::remove_file(dirty);

    println!();
}

// ============================================================================
// Bug #607: Duplicate Last-metadata-block flags
// ============================================================================

/// Generate a minimal FLAC via flac-codec then strip to STREAMINFO-only.
///
/// flac-codec outputs STREAMINFO + PADDING + SEEKTABLE, but the bug triggers
/// when lofty encounters a FLAC with ONLY STREAMINFO (is_last set on it).
/// We strip everything after STREAMINFO and splice in the audio frames directly.
fn make_streaminfo_only_flac(path: &Path) {
    let tmp = path.with_extension("tmp.flac");

    // Step 1: encode via flac-codec
    let sample_rate = 44100u32;
    let channels = 2u8;
    let bits_per_sample = 16u32;
    let samples = vec![0i32; 4410 * channels as usize];

    let mut encoder = flac_codec::encode::FlacSampleWriter::create(
        &tmp,
        flac_codec::encode::Options::default(),
        sample_rate,
        bits_per_sample,
        channels,
        None,
    ).expect("flac-codec encoder creation failed");
    encoder.write(&samples).expect("flac-codec write failed");
    encoder.finalize().expect("flac-codec finalize failed");

    // Step 2: parse block layout and rebuild with STREAMINFO only
    let data = std::fs::read(&tmp).unwrap();
    assert_eq!(&data[..4], b"fLaC");

    // Walk metadata blocks to find end of metadata / start of audio frames
    let mut pos = 4;
    let mut streaminfo_end = 0;
    let mut audio_start = 0;
    loop {
        if pos + 4 > data.len() { break; }
        let is_last = (data[pos] & 0x80) != 0;
        let block_type = data[pos] & 0x7F;
        let block_len = ((data[pos + 1] as usize) << 16)
            | ((data[pos + 2] as usize) << 8)
            | (data[pos + 3] as usize);
        let block_end = pos + 4 + block_len;
        if block_type == 0 {
            // STREAMINFO — keep this one
            streaminfo_end = block_end;
        }
        if is_last {
            audio_start = block_end;
            break;
        }
        pos = block_end;
    }

    assert!(streaminfo_end > 4 && audio_start > streaminfo_end,
        "Failed to parse flac-codec output");

    // Rebuild: fLaC + STREAMINFO (with is_last set) + audio frames
    let mut out = Vec::new();
    out.extend_from_slice(b"fLaC");
    // Copy STREAMINFO header but set is_last bit (0x80 | block_type)
    out.push(data[4] | 0x80); // is_last=true, type=0 (STREAMINFO)
    out.extend_from_slice(&data[5..streaminfo_end]); // rest of STREAMINFO
    out.extend_from_slice(&data[audio_start..]); // audio frames

    std::fs::write(path, &out).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

/// Generate a FLAC via flac-codec (as the transcode pipeline does).
/// Produces STREAMINFO + PADDING + SEEKTABLE — no VORBIS_COMMENT.
fn make_flac_codec_flac(path: &Path) {
    let sample_rate = 44100u32;
    let channels = 2u8;
    let bits_per_sample = 16u32;
    let samples = vec![0i32; 4410 * channels as usize];

    let mut encoder = flac_codec::encode::FlacSampleWriter::create(
        path,
        flac_codec::encode::Options::default(),
        sample_rate,
        bits_per_sample,
        channels,
        None,
    ).expect("flac-codec encoder creation failed");
    encoder.write(&samples).expect("flac-codec write failed");
    encoder.finalize().expect("flac-codec finalize failed");
}

fn test_is_last_bug() {
    println!("=== BUG #607: Duplicate Last-metadata-block flags ===\n");

    let clean = Path::new("test_607_clean.flac");
    let saved = Path::new("test_607_saved.flac");

    // --- STREAMINFO-only FLAC (stripped from flac-codec output) ---
    make_streaminfo_only_flac(clean);
    println!("  STREAMINFO-only FLAC: {} bytes", std::fs::metadata(clean).unwrap().len());

    println!("\n  Baseline (STREAMINFO-only):");
    let (total, is_last_count, correct) = check_is_last_flags(clean);
    println!("    → {} blocks, {} with is_last, correct={}", total, is_last_count, correct);

    // Test 1: add VorbisComments + picture with default padding to STREAMINFO-only
    std::fs::copy(clean, saved).unwrap();
    println!("\n  Test 1: STREAMINFO-only → insert_picture + set_artist + default padding");
    {
        let file = std::fs::File::open(saved).unwrap();
        let mut r = BufReader::new(file);
        let mut flac = lofty::flac::FlacFile::read_from(&mut r, ParseOptions::default()).unwrap();
        drop(r);
        let _ = flac.insert_picture(make_picture(), None);
        if let Some(vc) = flac.vorbis_comments_mut() {
            vc.set_artist(String::from("Test Artist"));
        }
        flac.save_to_path(saved, WriteOptions::default()).unwrap();
    }
    let (total, is_last_count, correct) = check_is_last_flags(saved);
    println!("    → {} blocks, {} with is_last, correct={}", total, is_last_count, correct);
    if !correct { println!("    BUG: Multiple blocks have is_last set!"); }

    let _ = std::fs::remove_file(clean);
    let _ = std::fs::remove_file(saved);

    // --- Full flac-codec FLAC (STREAMINFO + PADDING + SEEKTABLE) ---
    // This is the actual layout produced by the transcode pipeline.
    make_flac_codec_flac(clean);
    println!("\n  flac-codec FLAC: {} bytes", std::fs::metadata(clean).unwrap().len());

    println!("\n  Baseline (flac-codec output):");
    let (total, is_last_count, correct) = check_is_last_flags(clean);
    println!("    → {} blocks, {} with is_last, correct={}", total, is_last_count, correct);

    // Test 2: replicate transcode flow — copy_pictures with DEFAULT padding
    // This is the path that was borking files before the preferred_padding(0) workaround.
    std::fs::copy(clean, saved).unwrap();
    println!("\n  Test 2: flac-codec → insert_picture with DEFAULT padding (transcode flow)");
    {
        let file = std::fs::File::open(saved).unwrap();
        let mut r = BufReader::new(file);
        let mut flac = lofty::flac::FlacFile::read_from(&mut r, ParseOptions::default()).unwrap();
        drop(r);
        let _ = flac.insert_picture(make_picture(), None);
        flac.save_to_path(saved, WriteOptions::default()).unwrap();
    }
    let (total, is_last_count, correct) = check_is_last_flags(saved);
    println!("    → {} blocks, {} with is_last, correct={}", total, is_last_count, correct);
    if !correct { println!("    BUG: Multiple blocks have is_last set!"); }

    // Test 3: then add VorbisComments with DEFAULT padding (simulating copy_tags)
    println!("\n  Test 3: → then set_artist with DEFAULT padding (simulating copy_tags)");
    {
        let file = std::fs::File::open(saved).unwrap();
        let mut r = BufReader::new(file);
        let mut flac = lofty::flac::FlacFile::read_from(&mut r, ParseOptions::default()).unwrap();
        drop(r);
        if let Some(vc) = flac.vorbis_comments_mut() {
            vc.set_artist(String::from("Test Artist"));
        } else {
            let mut vc = lofty::ogg::VorbisComments::default();
            vc.set_artist(String::from("Test Artist"));
            flac.set_vorbis_comments(vc);
        }
        flac.save_to_path(saved, WriteOptions::default()).unwrap();
    }
    let (total, is_last_count, correct) = check_is_last_flags(saved);
    println!("    → {} blocks, {} with is_last, correct={}", total, is_last_count, correct);
    if !correct { println!("    BUG: Multiple blocks have is_last set!"); }

    // Test 4: fresh flac-codec → set VorbisComments with DEFAULT padding (no picture first)
    std::fs::copy(clean, saved).unwrap();
    println!("\n  Test 4: flac-codec → set_artist only with DEFAULT padding");
    {
        let file = std::fs::File::open(saved).unwrap();
        let mut r = BufReader::new(file);
        let mut flac = lofty::flac::FlacFile::read_from(&mut r, ParseOptions::default()).unwrap();
        drop(r);
        if let Some(vc) = flac.vorbis_comments_mut() {
            vc.set_artist(String::from("Test Artist"));
        } else {
            let mut vc = lofty::ogg::VorbisComments::default();
            vc.set_artist(String::from("Test Artist"));
            flac.set_vorbis_comments(vc);
        }
        flac.save_to_path(saved, WriteOptions::default()).unwrap();
    }
    let (total, is_last_count, correct) = check_is_last_flags(saved);
    println!("    → {} blocks, {} with is_last, correct={}", total, is_last_count, correct);
    if !correct { println!("    BUG: Multiple blocks have is_last set!"); }

    // Test 5: fresh flac-codec → VorbisComments with preferred_padding(0) (the workaround)
    std::fs::copy(clean, saved).unwrap();
    println!("\n  Test 5: flac-codec → set_artist with preferred_padding(0) (workaround)");
    {
        let file = std::fs::File::open(saved).unwrap();
        let mut r = BufReader::new(file);
        let mut flac = lofty::flac::FlacFile::read_from(&mut r, ParseOptions::default()).unwrap();
        drop(r);
        if let Some(vc) = flac.vorbis_comments_mut() {
            vc.set_artist(String::from("Test Artist"));
        } else {
            let mut vc = lofty::ogg::VorbisComments::default();
            vc.set_artist(String::from("Test Artist"));
            flac.set_vorbis_comments(vc);
        }
        flac.save_to_path(saved, WriteOptions::new().preferred_padding(0)).unwrap();
    }
    let (total, is_last_count, correct) = check_is_last_flags(saved);
    println!("    → {} blocks, {} with is_last, correct={}", total, is_last_count, correct);
    if !correct { println!("    BUG: Multiple blocks have is_last set!"); }

    // Cleanup
    let _ = std::fs::remove_file(clean);
    let _ = std::fs::remove_file(saved);

    println!();
}

// ============================================================================

fn main() {
    println!("lofty version: 0.23.x\n");
    test_id3v2_bug();
    test_is_last_bug();
    println!("Done.");
}

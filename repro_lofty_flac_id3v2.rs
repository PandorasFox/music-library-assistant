//! Reproduction case: lofty 0.23.1 FlacFile save_to_path on FLAC with ID3v2.
//!
//! Generates a real FLAC file via ffmpeg, prepends ID3v2, tests save behavior.
//! Also validates the in-place strip workaround.

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::AudioFile;
use lofty::ogg::OggPictureStorage;
use lofty::picture::{MimeType, Picture, PictureType};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

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
    id3.extend_from_slice(&[4, 0, 0x00]);

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
    frames.extend_from_slice(&[0x00; 1024]);

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

/// Workaround: strip ID3v2 in-place by shifting FLAC data forward + truncate.
fn strip_id3v2_on_disk(path: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    let mut file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;

    let mut header = [0u8; 10];
    if file.read_exact(&mut header).is_err() {
        return Ok(false);
    }
    if &header[..3] != b"ID3" {
        return Ok(false);
    }

    let size = ((header[6] as u64 & 0x7F) << 21)
        | ((header[7] as u64 & 0x7F) << 14)
        | ((header[8] as u64 & 0x7F) << 7)
        | (header[9] as u64 & 0x7F);
    let mut id3_end = 10 + size;
    if header[5] & 0x10 != 0 { id3_end += 10; }

    file.seek(SeekFrom::Start(id3_end))?;
    let mut marker = [0u8; 4];
    file.read_exact(&mut marker)?;
    if &marker != b"fLaC" {
        return Err(format!("fLaC not found at offset {}", id3_end).into());
    }

    let file_len = file.metadata()?.len();
    let flac_len = file_len - id3_end;
    const CHUNK: usize = 64 * 1024;
    let mut buf = vec![0u8; CHUNK];
    let mut read_pos = id3_end;
    let mut write_pos = 0u64;
    while read_pos < file_len {
        file.seek(SeekFrom::Start(read_pos))?;
        let n = file.read(&mut buf)?;
        if n == 0 { break; }
        file.seek(SeekFrom::Start(write_pos))?;
        file.write_all(&buf[..n])?;
        read_pos += n as u64;
        write_pos += n as u64;
    }
    file.set_len(flac_len)?;
    file.flush()?;
    println!("  strip_id3v2_on_disk: stripped {} bytes", id3_end);
    Ok(true)
}

fn make_picture() -> Picture {
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

fn main() {
    let clean = Path::new("test_clean.flac");
    let dirty = Path::new("test_id3v2.flac");

    println!("Generating real FLAC via ffmpeg...");
    make_real_flac(clean);
    println!("  Clean FLAC: {} bytes\n", std::fs::metadata(clean).unwrap().len());

    // === Bug reproduction ===
    println!("--- BUG REPRODUCTION ---");

    std::fs::copy(clean, dirty).unwrap();
    prepend_id3v2(dirty);
    println!("  Test: save without remove_id3v2");
    {
        let file = std::fs::File::open(dirty).unwrap();
        let mut r = BufReader::new(file);
        let flac = lofty::flac::FlacFile::read_from(&mut r, ParseOptions::default()).unwrap();
        drop(r);
        match flac.save_to_path(dirty, WriteOptions::new().preferred_padding(0)) {
            Ok(()) => println!("  save_to_path: OK"),
            Err(e) => println!("  save_to_path FAILED: {}", e),
        }
    }

    std::fs::copy(clean, dirty).unwrap();
    prepend_id3v2(dirty);
    println!("  Test: save with remove_id3v2 + insert_picture");
    {
        let file = std::fs::File::open(dirty).unwrap();
        let mut r = BufReader::new(file);
        let mut flac = lofty::flac::FlacFile::read_from(&mut r, ParseOptions::default()).unwrap();
        drop(r);
        flac.remove_id3v2();
        let _ = flac.insert_picture(make_picture(), None);
        match flac.save_to_path(dirty, WriteOptions::new().preferred_padding(0)) {
            Ok(()) => println!("  save_to_path: OK"),
            Err(e) => println!("  save_to_path FAILED: {}", e),
        }
    }

    // === Workaround validation ===
    println!("\n--- WORKAROUND: strip_id3v2_on_disk ---");

    std::fs::copy(clean, dirty).unwrap();
    prepend_id3v2(dirty);
    let before_inode = std::fs::metadata(dirty).map(|m| {
        use std::os::unix::fs::MetadataExt;
        m.ino()
    }).unwrap();
    strip_id3v2_on_disk(dirty).unwrap();
    let after_inode = std::fs::metadata(dirty).map(|m| {
        use std::os::unix::fs::MetadataExt;
        m.ino()
    }).unwrap();
    println!("  Inode preserved: {} (before={}, after={})", before_inode == after_inode, before_inode, after_inode);

    // Verify starts with fLaC
    let head = std::fs::read(dirty).unwrap();
    println!("  Starts with fLaC: {}", &head[..4] == b"fLaC");

    // Now try the full embed flow
    println!("  Test: read + remove_id3v2 + insert_picture + save (after strip)");
    {
        let file = std::fs::File::open(dirty).unwrap();
        let mut r = BufReader::new(file);
        let mut flac = lofty::flac::FlacFile::read_from(&mut r, ParseOptions::default()).unwrap();
        drop(r);
        flac.remove_id3v2();
        let _ = flac.insert_picture(make_picture(), None);
        match flac.save_to_path(dirty, WriteOptions::new().preferred_padding(0)) {
            Ok(()) => println!("  save_to_path: OK"),
            Err(e) => println!("  save_to_path FAILED: {}", e),
        }
    }

    // Verify the saved file has pictures
    {
        let file = std::fs::File::open(dirty).unwrap();
        let mut r = BufReader::new(file);
        let flac = lofty::flac::FlacFile::read_from(&mut r, ParseOptions::default()).unwrap();
        println!("  Pictures in saved file: {}", flac.pictures().len());
    }

    // Cleanup
    let _ = std::fs::remove_file(clean);
    let _ = std::fs::remove_file(dirty);
    println!("\nDone.");
}

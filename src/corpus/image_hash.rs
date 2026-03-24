//! Perceptual image hashing for visual similarity comparison.
//!
//! Used to determine if a higher-resolution image from the Cover Art Archive
//! is visually identical to an existing sidecar (same art, better fidelity).

use image::imageops::FilterType;

/// 64-bit difference hash (dHash).
///
/// Grayscale -> resize to 9x8 -> compare adjacent pixel brightness.
/// Produces a perceptually stable hash: similar images yield similar hashes
/// regardless of resolution, compression, or minor color adjustments.
pub fn dhash(image_bytes: &[u8]) -> anyhow::Result<u64> {
    let img = image::load_from_memory(image_bytes)?;
    let gray = img
        .grayscale()
        .resize_exact(9, 8, FilterType::Lanczos3)
        .to_luma8();
    let mut hash: u64 = 0;
    for y in 0..8u32 {
        for x in 0..8u32 {
            if gray.get_pixel(x, y).0[0] > gray.get_pixel(x + 1, y).0[0] {
                hash |= 1 << (y * 8 + x);
            }
        }
    }
    Ok(hash)
}

/// Hamming distance between two dHash values.
pub fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Check if two images are visually the same (different resolution is OK).
///
/// Threshold of 10 bits (out of 64) allows for compression artifacts
/// and minor color space differences while catching genuinely different images.
pub fn is_visual_match(existing_bytes: &[u8], candidate_bytes: &[u8]) -> anyhow::Result<bool> {
    let h1 = dhash(existing_bytes)?;
    let h2 = dhash(candidate_bytes)?;
    Ok(hamming_distance(h1, h2) <= 10)
}

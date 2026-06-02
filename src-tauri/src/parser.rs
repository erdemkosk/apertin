use std::collections::HashSet;
use std::fs::File;
use std::path::Path;
use memmap2::MmapOptions;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMetadata {
    pub file_path: String,
    pub file_name: String,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub iso: Option<u32>,
    pub aperture: Option<f32>,
    pub shutter_speed: Option<String>,
    pub focal_length: Option<f32>,
    pub date_time: Option<String>,
    pub orientation: Option<u16>,
    pub preview_offset: u32,
    pub preview_length: u32,
}

impl RawMetadata {
    pub fn new(file_path: String, file_name: String) -> Self {
        Self {
            file_path,
            file_name,
            camera_make: None,
            camera_model: None,
            lens_model: None,
            width: None,
            height: None,
            iso: None,
            aperture: None,
            shutter_speed: None,
            focal_length: None,
            date_time: None,
            orientation: None,
            preview_offset: 0,
            preview_length: 0,
        }
    }
}

// Helper struct for parsing TIFF format structure
struct TIFFReader<'a> {
    data: &'a [u8],
    is_little_endian: bool,
}

impl<'a> TIFFReader<'a> {
    fn read_u16(&self, offset: usize) -> Option<u16> {
        if offset + 2 > self.data.len() { return None; }
        let bytes = &self.data[offset..offset + 2];
        if self.is_little_endian {
            Some(u16::from_le_bytes([bytes[0], bytes[1]]))
        } else {
            Some(u16::from_be_bytes([bytes[0], bytes[1]]))
        }
    }

    fn read_u32(&self, offset: usize) -> Option<u32> {
        if offset + 4 > self.data.len() { return None; }
        let bytes = &self.data[offset..offset + 4];
        if self.is_little_endian {
            Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        } else {
            Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        }
    }

    fn read_string(&self, offset: usize, count: usize) -> Option<String> {
        if offset + count > self.data.len() { return None; }
        let bytes = &self.data[offset..offset + count];
        let len = bytes.iter().position(|&x| x == 0).unwrap_or(count);
        Some(String::from_utf8_lossy(&bytes[..len]).to_string().trim().to_string())
    }

    fn get_field_offset(&self, entry_offset: usize, type_size: usize, count: usize) -> Option<usize> {
        let total_size = type_size * count;
        if total_size <= 4 {
            Some(entry_offset + 8)
        } else {
            self.read_u32(entry_offset + 8).map(|off| off as usize)
        }
    }

    fn read_ascii_tag(&self, entry_offset: usize, count: usize) -> Option<String> {
        let offset = self.get_field_offset(entry_offset, 1, count)?;
        self.read_string(offset, count)
    }

    fn read_numeric_tag(&self, entry_offset: usize, tag_type: u16, count: usize) -> Option<u32> {
        if count == 0 { return None; }
        let type_size = match tag_type {
            1 | 7 => 1,
            3 | 8 => 2,
            4 | 9 => 4,
            _ => return None,
        };
        let offset = self.get_field_offset(entry_offset, type_size, count)?;
        match type_size {
            1 => self.data.get(offset).map(|&b| b as u32),
            2 => self.read_u16(offset).map(|val| val as u32),
            4 => self.read_u32(offset),
            _ => None,
        }
    }

    fn read_rational_tag(&self, entry_offset: usize, count: usize) -> Option<(u32, u32)> {
        if count == 0 { return None; }
        let offset = self.get_field_offset(entry_offset, 8, count)?;
        let num = self.read_u32(offset)?;
        let den = self.read_u32(offset + 4)?;
        Some((num, den))
    }
}

pub fn parse_tiff_bytes(tiff_data: &[u8], meta: &mut RawMetadata, is_file_level: bool) {
    if tiff_data.len() < 8 { return; }

    let is_little_endian = match &tiff_data[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => return,
    };

    let magic = if is_little_endian {
        u16::from_le_bytes([tiff_data[2], tiff_data[3]])
    } else {
        u16::from_be_bytes([tiff_data[2], tiff_data[3]])
    };

    if magic != 42 { return; }

    let reader = TIFFReader {
        data: tiff_data,
        is_little_endian,
    };

    let ifd0_offset = match reader.read_u32(4) {
        Some(off) => off as usize,
        None => return,
    };

    let mut visited_ifds = HashSet::new();
    let mut ifd_queue = vec![ifd0_offset];

    let mut best_preview_offset = 0u32;
    let mut best_preview_length = 0u32;

    while let Some(ifd_offset) = ifd_queue.pop() {
        if ifd_offset == 0 || !visited_ifds.insert(ifd_offset) {
            continue;
        }

        let num_entries = match reader.read_u16(ifd_offset) {
            Some(num) => num as usize,
            None => continue,
        };

        // Enqueue next IFD in the main TIFF chain
        let next_ifd_offset_pos = ifd_offset + 2 + num_entries * 12;
        if let Some(next_ifd) = reader.read_u32(next_ifd_offset_pos) {
            if next_ifd != 0 {
                ifd_queue.push(next_ifd as usize);
            }
        }

        let mut ifd_preview_offset = None;
        let mut ifd_preview_length = None;

        for i in 0..num_entries {
            let entry_offset = ifd_offset + 2 + i * 12;
            let tag = match reader.read_u16(entry_offset) {
                Some(t) => t,
                None => continue,
            };
            let tag_type = match reader.read_u16(entry_offset + 2) {
                Some(t) => t,
                None => continue,
            };
            let count = match reader.read_u32(entry_offset + 4) {
                Some(c) => c as usize,
                None => continue,
            };

            match tag {
                // Make
                0x010f => {
                    if meta.camera_make.is_none() {
                        meta.camera_make = reader.read_ascii_tag(entry_offset, count);
                    }
                }
                // Model
                0x0110 => {
                    if meta.camera_model.is_none() {
                        meta.camera_model = reader.read_ascii_tag(entry_offset, count);
                    }
                }
                // LensModel
                0xa434 => {
                    if meta.lens_model.is_none() {
                        meta.lens_model = reader.read_ascii_tag(entry_offset, count);
                    }
                }
                // Orientation
                0x0112 => {
                    if meta.orientation.is_none() {
                        if let Some(val) = reader.read_numeric_tag(entry_offset, tag_type, count) {
                            meta.orientation = Some(val as u16);
                        }
                    }
                }
                // SubIFDs
                0x014a => {
                    let type_size = match tag_type {
                        3 | 8 => 2,
                        4 | 9 => 4,
                        _ => continue,
                    };
                    if let Some(offset) = reader.get_field_offset(entry_offset, type_size, count) {
                        for c in 0..count {
                            let sub_ifd = match type_size {
                                2 => reader.read_u16(offset + c * 2).map(|v| v as u32),
                                4 => reader.read_u32(offset + c * 4),
                                _ => None,
                            };
                            if let Some(sub_ifd_offset) = sub_ifd {
                                ifd_queue.push(sub_ifd_offset as usize);
                            }
                        }
                    }
                }
                // ExifIFD
                0x8769 => {
                    if let Some(exif_offset) = reader.read_numeric_tag(entry_offset, tag_type, count) {
                        ifd_queue.push(exif_offset as usize);
                    }
                }
                // JPEGInterchangeFormat (Preview Offset)
                0x0201 => {
                    if let Some(off) = reader.read_numeric_tag(entry_offset, tag_type, count) {
                        if is_file_level {
                            ifd_preview_offset = Some(off);
                        }
                    }
                }
                // JPEGInterchangeFormatLength (Preview Length)
                0x0202 => {
                    if let Some(len) = reader.read_numeric_tag(entry_offset, tag_type, count) {
                        if is_file_level {
                            ifd_preview_length = Some(len);
                        }
                    }
                }
                // ExposureTime
                0x829a => {
                    if meta.shutter_speed.is_none() {
                        if let Some((num, den)) = reader.read_rational_tag(entry_offset, count) {
                            if den != 0 {
                                let val = num as f32 / den as f32;
                                if val < 1.0 {
                                    let rounded_den = (1.0 / val).round() as u32;
                                    meta.shutter_speed = Some(format!("1/{}", rounded_den));
                                } else {
                                    meta.shutter_speed = Some(format!("{:.1}s", val));
                                }
                            }
                        }
                    }
                }
                // FNumber (Aperture)
                0x829d => {
                    if meta.aperture.is_none() {
                        if let Some((num, den)) = reader.read_rational_tag(entry_offset, count) {
                            if den != 0 {
                                meta.aperture = Some(num as f32 / den as f32);
                            }
                        }
                    }
                }
                // ISOSpeedRatings
                0x8827 => {
                    if meta.iso.is_none() {
                        if let Some(val) = reader.read_numeric_tag(entry_offset, tag_type, count) {
                            meta.iso = Some(val);
                        }
                    }
                }
                // FocalLength
                0x920a => {
                    if meta.focal_length.is_none() {
                        if let Some((num, den)) = reader.read_rational_tag(entry_offset, count) {
                            if den != 0 {
                                meta.focal_length = Some(num as f32 / den as f32);
                            }
                        }
                    }
                }
                // DateTimeOriginal
                0x9003 => {
                    if meta.date_time.is_none() {
                        meta.date_time = reader.read_ascii_tag(entry_offset, count);
                    }
                }
                _ => {}
            }
        }

        if let (Some(off), Some(len)) = (ifd_preview_offset, ifd_preview_length) {
            if len > best_preview_length {
                best_preview_offset = off;
                best_preview_length = len;
            }
        }
    }

    if is_file_level && best_preview_length > 0 {
        if best_preview_length > meta.preview_length {
            meta.preview_offset = best_preview_offset;
            meta.preview_length = best_preview_length;
        }
    }
}

// ISOBMFF helper read functions (Big Endian)
fn read_u16_be(data: &[u8], offset: usize) -> Option<u16> {
    if offset + 2 > data.len() { return None; }
    Some(u16::from_be_bytes([data[offset], data[offset + 1]]))
}

fn read_u32_be(data: &[u8], offset: usize) -> Option<u32> {
    if offset + 4 > data.len() { return None; }
    Some(u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]))
}

fn read_u64_be(data: &[u8], offset: usize) -> Option<u64> {
    if offset + 8 > data.len() { return None; }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&data[offset..offset + 8]);
    Some(u64::from_be_bytes(bytes))
}

// Parses ISOBMFF boxes recursively for Canon CR3
pub fn parse_isobmff(
    data: &[u8],
    start: usize,
    end: usize,
    meta: &mut RawMetadata,
) {
    let mut offset = start;
    while offset + 8 <= end {
        let box_size = match read_u32_be(data, offset) {
            Some(s) => s as usize,
            None => break,
        };
        let type_bytes = &data[offset + 4..offset + 8];
        let box_type = match std::str::from_utf8(type_bytes) {
            Ok(s) => s.to_string(),
            Err(_) => break,
        };

        let header_size = 8;
        let (real_size, header_size) = if box_size == 1 {
            if offset + 16 <= end {
                let large_size = read_u64_be(data, offset + 8).unwrap_or(0) as usize;
                (large_size, 16)
            } else {
                (box_size, header_size)
            }
        } else {
            (box_size, header_size)
        };

        if real_size == 0 || offset + real_size > end {
            break;
        }

        let box_end = offset + real_size;
        let payload_start = offset + header_size;

        match box_type.as_str() {
            "moov" | "trak" | "mdia" | "minf" | "stbl" => {
                parse_isobmff(data, payload_start, box_end, meta);
            }
            "uuid" => {
                if payload_start + 16 <= box_end {
                    let uuid = &data[payload_start..payload_start + 16];
                    let expected_uuid = [
                        0xea, 0xf4, 0x2b, 0x5e, 0x1c, 0x98, 0x4b, 0x88,
                        0xb9, 0xfb, 0xb7, 0xdc, 0x40, 0x6e, 0x4d, 0x16
                    ];
                    if uuid == expected_uuid {
                        let prvw_header_offset = payload_start + 16;
                        if prvw_header_offset + 32 <= box_end {
                            if let Some(jpeg_size) = read_u32_be(data, prvw_header_offset + 8) {
                                if &data[prvw_header_offset + 12..prvw_header_offset + 16] == b"PRVW" {
                                    if jpeg_size > meta.preview_length {
                                        meta.preview_offset = (prvw_header_offset + 32) as u32;
                                        meta.preview_length = jpeg_size;
                                        meta.width = read_u16_be(data, prvw_header_offset + 22).map(|w| w as u32);
                                        meta.height = read_u16_be(data, prvw_header_offset + 24).map(|h| h as u32);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            "CMT1" | "CMT2" => {
                if payload_start < box_end {
                    let payload = &data[payload_start..box_end];
                    if payload.starts_with(b"II\x2a\x00") || payload.starts_with(b"MM\x00\x2a") {
                        parse_tiff_bytes(payload, meta, false);
                    }
                }
            }
            _ => {}
        }

        offset = box_end;
    }
}

/// Computes a 64-bit dHash (difference hash) from raw JPEG bytes.
/// Perceptual hash (pHash) via 2-D DCT.
///
/// Algorithm:
///   1. Resize to 32×32 with a good downscale filter (Triangle/bilinear).
///   2. Convert to grayscale (luma).
///   3. Apply separable 2-D DCT-II (row-wise then column-wise).
///   4. Take the top-left 8×8 block of DCT coefficients (low frequencies).
///      These encode the "scene structure" — robust to noise, blur, and
///      mild exposure differences because those are high-frequency artifacts.
///   5. Build a 64-bit hash: bit i = 1 if coeff[i] > median(all 64 coeffs).
///
/// Recommended Hamming thresholds:
///   ≤  6 → strict  (burst / near-identical shots)
///   ≤ 10 → normal  (same scene, varying exposure/framing)
///   ≤ 15 → loose   (similar subject, different angle)
pub fn compute_phash(jpeg_bytes: &[u8]) -> Option<u64> {
    const SIZE: usize = 32;
    const HASH_BITS: usize = 8; // use 8×8 = 64-bit hash

    let img = image::load_from_memory(jpeg_bytes).ok()?;
    let resized = img.resize_exact(SIZE as u32, SIZE as u32, image::imageops::FilterType::Triangle);
    let gray = resized.to_luma8();

    let pixels: Vec<f32> = gray.pixels().map(|p| p[0] as f32).collect();

    // Separable 2D DCT-II
    let dct = dct2d(&pixels, SIZE);

    // Extract top-left HASH_BITS × HASH_BITS block (skip nothing — DC included)
    let mut low = [0f32; 64];
    for y in 0..HASH_BITS {
        for x in 0..HASH_BITS {
            low[y * HASH_BITS + x] = dct[y * SIZE + x];
        }
    }

    // Median of the 64 low-frequency coefficients
    let mut sorted = low;
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[32];

    // Hash: bit i set iff low[i] > median
    let mut hash: u64 = 0;
    for (i, &v) in low.iter().enumerate() {
        if v > median {
            hash |= 1u64 << i;
        }
    }
    Some(hash)
}

/// 1-D DCT-II (unnormalised): output[k] = Σ_i input[i] · cos(π·k·(2i+1) / (2N))
fn dct1d(input: &[f32]) -> Vec<f32> {
    let n = input.len();
    let nf = n as f32;
    (0..n)
        .map(|k| {
            input
                .iter()
                .enumerate()
                .map(|(i, &x)| {
                    x * (std::f32::consts::PI * k as f32 * (2 * i + 1) as f32 / (2.0 * nf)).cos()
                })
                .sum::<f32>()
        })
        .collect()
}

/// Row-then-column separable 2-D DCT-II on a `size×size` image.
fn dct2d(pixels: &[f32], size: usize) -> Vec<f32> {
    // Row-wise DCT
    let mut tmp = vec![0f32; size * size];
    for y in 0..size {
        let row_dct = dct1d(&pixels[y * size..(y + 1) * size]);
        tmp[y * size..(y + 1) * size].copy_from_slice(&row_dct);
    }
    // Column-wise DCT
    let mut out = vec![0f32; size * size];
    for x in 0..size {
        let col: Vec<f32> = (0..size).map(|y| tmp[y * size + x]).collect();
        let col_dct = dct1d(&col);
        for y in 0..size {
            out[y * size + x] = col_dct[y];
        }
    }
    out
}

/// Scans PNG chunks for an `eXIf` chunk and parses the TIFF/EXIF data inside.
pub fn parse_png_exif(data: &[u8], meta: &mut RawMetadata) {
    // PNG signature is 8 bytes; chunks start at offset 8.
    // Each chunk: 4-byte length | 4-byte type | <length> bytes data | 4-byte CRC
    let mut offset = 8usize;
    while offset + 12 <= data.len() {
        let chunk_len = match read_u32_be(data, offset) {
            Some(l) => l as usize,
            None => break,
        };
        let chunk_type = &data[offset + 4..offset + 8];
        let payload_start = offset + 8;
        let payload_end = payload_start + chunk_len;

        if payload_end > data.len() {
            break;
        }

        // eXIf chunk (PNG 1.6+) contains raw TIFF/EXIF data
        if chunk_type == b"eXIf" {
            let exif_data = &data[payload_start..payload_end];
            if exif_data.starts_with(b"II\x2a\x00") || exif_data.starts_with(b"MM\x00\x2a") {
                parse_tiff_bytes(exif_data, meta, false);
            }
            return;
        }

        // IEND marks the end of the PNG
        if chunk_type == b"IEND" {
            break;
        }

        // 4-byte CRC follows the data
        offset = payload_end + 4;
    }
}

/// Scans JPEG bytes for standard APP1 EXIF segment (fallback)
pub fn extract_tiff_from_jpeg(jpeg_data: &[u8]) -> Option<&[u8]> {
    if jpeg_data.len() < 4 { return None; }
    if jpeg_data[0] != 0xFF || jpeg_data[1] != 0xD8 {
        return None;
    }

    let mut offset = 2;
    while offset + 4 <= jpeg_data.len() {
        if jpeg_data[offset] != 0xFF {
            offset += 1;
            continue;
        }

        let marker = jpeg_data[offset + 1];
        if marker == 0x00 || marker == 0xFF {
            offset += 1;
            continue;
        }

        if marker == 0xD9 {
            break; // End of image
        }

        let length = u16::from_be_bytes([jpeg_data[offset + 2], jpeg_data[offset + 3]]) as usize;
        let marker_end = offset + 2 + length;
        if marker_end > jpeg_data.len() {
            break; // Out of bounds
        }

        if marker == 0xE1 { // APP1
            let app1_payload = &jpeg_data[offset + 4..marker_end];
            if app1_payload.len() >= 6 && &app1_payload[0..6] == b"Exif\0\0" {
                return Some(&app1_payload[6..]);
            }
        }

        offset = marker_end;
    }
    None
}

// Extracts metadata from the embedded JPEG preview to fill in missing tags
pub fn extract_metadata_from_embedded_jpeg(
    file_data: &[u8],
    preview_offset: u32,
    preview_length: u32,
    meta: &mut RawMetadata,
) {
    if preview_length == 0 { return; }
    let start = preview_offset as usize;
    let end = start + preview_length as usize;
    if end <= file_data.len() {
        let jpeg_bytes = &file_data[start..end];
        if let Some(tiff_slice) = extract_tiff_from_jpeg(jpeg_bytes) {
            parse_tiff_bytes(tiff_slice, meta, false);
        }
    }
}



// Fast search for the largest embedded JPEG by scanning signatures
pub fn scan_for_largest_jpeg(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 4 { return None; }

    let mut best_offset = 0;
    let mut best_len = 0;

    let mut i = 0;
    // Previews are always in the header/prefix, so scanning up to 15MB is extremely safe and fast
    let scan_limit = std::cmp::min(data.len() - 4, 15 * 1024 * 1024);

    while i < scan_limit {
        if data[i] == 0xFF && data[i + 1] == 0xD8 && data[i + 2] == 0xFF {
            let start_offset = i;
            let mut search_idx = start_offset + 2;
            let search_limit = std::cmp::min(data.len(), start_offset + 10 * 1024 * 1024); // max 10MB preview

            while search_idx + 1 < search_limit {
                if data[search_idx] == 0xFF && data[search_idx + 1] == 0xD9 {
                    let length = (search_idx + 2 - start_offset) as u32;
                    if length > best_len {
                        best_offset = start_offset as u32;
                        best_len = length;
                    }
                    break;
                }
                search_idx += 1;
            }
            if best_len > 0 && start_offset + best_len as usize > i {
                i = start_offset + best_len as usize;
                continue;
            }
        }
        i += 1;
    }

    if best_len > 0 {
        Some((best_offset, best_len))
    } else {
        None
    }
}

// Main API function to parse any RAW file
pub fn parse_raw_file(file_path: &str) -> Result<RawMetadata, String> {
    let path = Path::new(file_path);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let file = File::open(path).map_err(|e| format!("Failed to open file: {}", e))?;
    let mmap = unsafe {
        MmapOptions::new()
            .map(&file)
            .map_err(|e| format!("Failed to memory map file: {}", e))?
    };

    if mmap.len() < 16 {
        return Err("File is too small to be a valid RAW image".to_string());
    }

    let mut meta = RawMetadata::new(file_path.to_string(), file_name);

    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    if ext == "jpg" || ext == "jpeg" || mmap.starts_with(&[0xFF, 0xD8, 0xFF]) {
        // Standalone JPEG — the file itself is the displayable image.
        meta.preview_offset = 0;
        meta.preview_length = mmap.len() as u32;
        if let Some(tiff_slice) = extract_tiff_from_jpeg(&mmap) {
            parse_tiff_bytes(tiff_slice, &mut meta, false);
        }
        return Ok(meta);
    } else if ext == "png" || mmap.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        // Standalone PNG — the file itself is the displayable image.
        meta.preview_offset = 0;
        meta.preview_length = mmap.len() as u32;
        // IHDR chunk: width at byte 16, height at byte 20 (big-endian)
        if mmap.len() >= 24 {
            meta.width = read_u32_be(&mmap, 16);
            meta.height = read_u32_be(&mmap, 20);
        }
        // Try to extract EXIF from eXIf / tEXt chunks
        parse_png_exif(&mmap, &mut meta);
        return Ok(meta);
    } else if ext == "cr3" {
        parse_isobmff(&mmap, 0, mmap.len(), &mut meta);
    } else if ext == "raf" || mmap.starts_with(b"FUJIFILMCCD-RAW") {
        if mmap.len() >= 92 {
            let meta_offset = u32::from_be_bytes([mmap[76], mmap[77], mmap[78], mmap[79]]) as usize;
            let meta_len = u32::from_be_bytes([mmap[80], mmap[81], mmap[82], mmap[83]]) as usize;
            let jpeg_offset = u32::from_be_bytes([mmap[84], mmap[85], mmap[86], mmap[87]]);
            let jpeg_len = u32::from_be_bytes([mmap[88], mmap[89], mmap[90], mmap[91]]);

            meta.preview_offset = jpeg_offset;
            meta.preview_length = jpeg_len;

            // Try parsing meta container if it starts with TIFF
            if meta_offset > 0 && meta_offset + meta_len <= mmap.len() {
                let meta_slice = &mmap[meta_offset..meta_offset + meta_len];
                if meta_slice.starts_with(b"II\x2a\x00") || meta_slice.starts_with(b"MM\x00\x2a") {
                    parse_tiff_bytes(meta_slice, &mut meta, false);
                }
            }

            // Extract metadata from the embedded JPEG preview (bulletproof fallback)
            extract_metadata_from_embedded_jpeg(&mmap, jpeg_offset, jpeg_len, &mut meta);
        }
    } else {
        // Assume TIFF-based (ARW, NEF, CR2, DNG, etc.)
        parse_tiff_bytes(&mmap, &mut meta, true);

        // Extract metadata from JPEG preview as a fallback if tags are missing
        if meta.preview_length > 0 {
            extract_metadata_from_embedded_jpeg(&mmap, meta.preview_offset, meta.preview_length, &mut meta);
        }
    }

    // If preview is missing or too small (e.g. less than 500KB, which is usually a thumbnail),
    // scan the first 15MB of the file to extract the largest high-resolution JPEG preview.
    if meta.preview_length < 500_000 {
        if let Some((offset, len)) = scan_for_largest_jpeg(&mmap) {
            if len > meta.preview_length {
                meta.preview_offset = offset;
                meta.preview_length = len;
                extract_metadata_from_embedded_jpeg(&mmap, offset, len, &mut meta);
            }
        }
    }

    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_tiff_from_jpeg() {
        let mock_jpeg = [
            0xFF, 0xD8,
            0xFF, 0xE1,
            0x00, 0x0E,
            b'E', b'x', b'i', b'f', 0x00, 0x00,
            b'I', b'I', 0x2A, 0x00, 0x08, 0x00,
            0xFF, 0xD9,
        ];

        let extracted = extract_tiff_from_jpeg(&mock_jpeg);
        assert!(extracted.is_some());
        assert_eq!(extracted.unwrap(), &[b'I', b'I', 0x2A, 0x00, 0x08, 0x00]);
    }

    #[test]
    fn test_parse_tiff_bytes() {
        let mut mock_tiff = vec![
            b'I', b'I', 0x2A, 0x00,
            0x08, 0x00, 0x00, 0x00,
            0x02, 0x00,
        ];

        mock_tiff.extend_from_slice(&[0x0F, 0x01, 0x02, 0x00, 0x05, 0x00, 0x00, 0x00, 0x26, 0x00, 0x00, 0x00]);
        mock_tiff.extend_from_slice(&[0x10, 0x01, 0x02, 0x00, 0x05, 0x00, 0x00, 0x00, 0x2B, 0x00, 0x00, 0x00]);
        mock_tiff.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);

        mock_tiff.extend_from_slice(b"Sony\0");
        mock_tiff.extend_from_slice(b"A7R3\0");

        let mut meta = RawMetadata::new("test.arw".to_string(), "test.arw".to_string());
        parse_tiff_bytes(&mock_tiff, &mut meta, true);

        assert_eq!(meta.camera_make.as_deref(), Some("Sony"));
        assert_eq!(meta.camera_model.as_deref(), Some("A7R3"));
    }

    #[test]
    fn test_scan_for_jpeg() {
        let mock_data = [
            0x01, 0x02,
            0xFF, 0xD8,
            0xFF, 0xE0, 0x00, 0x02,
            0xFF, 0xD9,
            0x03, 0x04,
        ];

        let result = scan_for_largest_jpeg(&mock_data);
        assert_eq!(result, Some((2, 8)));
    }
}

use crate::config::ImageFormat;
use crate::driver::{DriverError, Rect};
use crate::json::parse_json;
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex, OnceLock,
};

#[derive(Debug, Clone)]
pub struct CachedFrame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

static FRAME_SEQ: AtomicU64 = AtomicU64::new(1);
static FRAME_CACHE: OnceLock<Mutex<VecDeque<(String, CachedFrame)>>> = OnceLock::new();

fn frame_cache() -> &'static Mutex<VecDeque<(String, CachedFrame)>> {
    FRAME_CACHE.get_or_init(|| Mutex::new(VecDeque::with_capacity(8)))
}

pub fn cache_rgba_frame(prefix: &str, width: usize, height: usize, rgba: Vec<u8>) -> String {
    let id = format!("cap_{}_{}", prefix, FRAME_SEQ.fetch_add(1, Ordering::Relaxed));
    if let Ok(mut guard) = frame_cache().lock() {
        if guard.len() >= 8 {
            guard.pop_front();
        }
        guard.push_back((
            id.clone(),
            CachedFrame {
                width,
                height,
                rgba,
            },
        ));
    }
    id
}

pub fn take_cached_frame(cap_id: &str) -> Option<CachedFrame> {
    let mut guard = frame_cache().lock().ok()?;
    let idx = guard.iter().position(|(id, _)| id == cap_id)?;
    guard.remove(idx).map(|(_, frame)| frame)
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

pub fn push_png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let mut crc_in = Vec::with_capacity(4 + data.len());
    crc_in.extend_from_slice(tag);
    crc_in.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_in).to_be_bytes());
}

pub fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(((bytes.len() + 2) / 3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

pub fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity((s.len() * 3) / 4);
    let mut buf = [0u8; 4];
    let mut count = 0usize;
    let mut pad = 0usize;
    for &b in s.as_bytes() {
        if b.is_ascii_whitespace() {
            continue;
        }
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => {
                pad += 1;
                0
            }
            _ => return Err("invalid base64 character".into()),
        };
        buf[count] = val;
        count += 1;
        if count == 4 {
            let n = ((buf[0] as u32) << 18)
                | ((buf[1] as u32) << 12)
                | ((buf[2] as u32) << 6)
                | (buf[3] as u32);
            out.push(((n >> 16) & 0xFF) as u8);
            if pad < 2 {
                out.push(((n >> 8) & 0xFF) as u8);
            }
            if pad == 0 {
                out.push((n & 0xFF) as u8);
            }
            count = 0;
            pad = 0;
        }
    }
    if count != 0 {
        return Err("truncated base64 input".into());
    }
    Ok(out)
}

pub fn encode_rgba_png_bytes(width: usize, height: usize, rgba: &[u8]) -> Vec<u8> {
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return Vec::new();
    }
    let row_bytes = width * 4;
    let mut raw = Vec::with_capacity(height * (1 + row_bytes));
    for y in 0..height {
        raw.push(0);
        let row = &rgba[y * row_bytes..(y + 1) * row_bytes];
        raw.extend_from_slice(row);
    }
    let mut zlib = Vec::with_capacity(raw.len() + 64);
    zlib.extend_from_slice(&[0x78, 0x01]);
    let mut pos = 0;
    while pos < raw.len() {
        let chunk = (raw.len() - pos).min(65535);
        let is_last = if pos + chunk == raw.len() { 1u8 } else { 0u8 };
        zlib.push(is_last);
        zlib.extend_from_slice(&(chunk as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(chunk as u16)).to_le_bytes());
        zlib.extend_from_slice(&raw[pos..pos + chunk]);
        pos += chunk;
    }
    let adler = adler32(&raw);
    zlib.extend_from_slice(&adler.to_be_bytes());

    let mut png = Vec::with_capacity(zlib.len() + 64);
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    push_png_chunk(&mut png, b"IHDR", &ihdr);
    push_png_chunk(&mut png, b"IDAT", &zlib);
    push_png_chunk(&mut png, b"IEND", &[]);
    png
}

pub fn encode_rgba_png_b64(width: usize, height: usize, rgba: &[u8]) -> String {
    base64_encode(&encode_rgba_png_bytes(width, height, rgba))
}

// MARK: - Pure-Rust Baseline JPEG Encoder

const STD_LUMA_QUANT: [u8; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61,
    12, 12, 14, 19, 26, 58, 60, 55,
    14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62,
    18, 22, 37, 56, 68, 109, 103, 77,
    24, 35, 55, 64, 81, 104, 113, 92,
    49, 64, 78, 87, 103, 121, 120, 101,
    72, 92, 95, 98, 112, 100, 103, 99,
];

const STD_CHROMA_QUANT: [u8; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99,
    18, 21, 26, 66, 99, 99, 99, 99,
    24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
];

const ZIGZAG: [usize; 64] = [
     0,  1,  8, 16,  9,  2,  3, 10,
    17, 24, 32, 25, 18, 11,  4,  5,
    12, 19, 26, 33, 40, 48, 41, 34,
    27, 20, 13,  6,  7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36,
    29, 22, 15, 23, 30, 37, 44, 51,
    58, 59, 52, 45, 38, 31, 39, 46,
    53, 60, 61, 54, 47, 55, 62, 63,
];

const BITS_DC_LUMA: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const VALS_DC_LUMA: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

const BITS_DC_CHROMA: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const VALS_DC_CHROMA: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

const BITS_AC_LUMA: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 125];
const VALS_AC_LUMA: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12,
    0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08,
    0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16,
    0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39,
    0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59,
    0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79,
    0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98,
    0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
    0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6,
    0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5,
    0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4,
    0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea,
    0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];

const BITS_AC_CHROMA: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 119];
const VALS_AC_CHROMA: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21,
    0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91,
    0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0,
    0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34,
    0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26,
    0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38,
    0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
    0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78,
    0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96,
    0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
    0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4,
    0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
    0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2,
    0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
    0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9,
    0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];

#[derive(Clone, Copy, Default)]
struct HuffmanCode {
    code: u16,
    len: u8,
}

fn build_huffman_lut(bits: &[u8; 16], values: &[u8]) -> [HuffmanCode; 256] {
    let mut lut = [HuffmanCode { code: 0, len: 0 }; 256];
    let mut code = 0u16;
    let mut val_idx = 0;
    for len in 1..=16 {
        let count = bits[len - 1] as usize;
        for _ in 0..count {
            let val = values[val_idx] as usize;
            lut[val] = HuffmanCode {
                code,
                len: len as u8,
            };
            code += 1;
            val_idx += 1;
        }
        code <<= 1;
    }
    lut
}

struct HuffmanLuts {
    dc_luma: [HuffmanCode; 256],
    dc_chroma: [HuffmanCode; 256],
    ac_luma: [HuffmanCode; 256],
    ac_chroma: [HuffmanCode; 256],
}

fn get_huffman_luts() -> &'static HuffmanLuts {
    static LUTS: OnceLock<HuffmanLuts> = OnceLock::new();
    LUTS.get_or_init(|| HuffmanLuts {
        dc_luma: build_huffman_lut(&BITS_DC_LUMA, &VALS_DC_LUMA),
        dc_chroma: build_huffman_lut(&BITS_DC_CHROMA, &VALS_DC_CHROMA),
        ac_luma: build_huffman_lut(&BITS_AC_LUMA, &VALS_AC_LUMA),
        ac_chroma: build_huffman_lut(&BITS_AC_CHROMA, &VALS_AC_CHROMA),
    })
}

fn get_dct_coeff() -> &'static [[f32; 8]; 8] {
    static DCT_COEFF: OnceLock<[[f32; 8]; 8]> = OnceLock::new();
    DCT_COEFF.get_or_init(|| {
        let mut lut = [[0.0f32; 8]; 8];
        for u in 0..8 {
            let c = if u == 0 { 1.0 / std::f32::consts::SQRT_2 } else { 1.0 };
            for x in 0..8 {
                let angle = (2 * x + 1) as f32 * u as f32 * std::f32::consts::PI / 16.0;
                lut[u][x] = 0.5 * c * angle.cos();
            }
        }
        lut
    })
}

fn scale_quant_table(base: &[u8; 64], quality: u8) -> [u8; 64] {
    let q = quality.clamp(1, 100) as u32;
    let s = if q < 50 {
        5000 / q
    } else {
        200 - q * 2
    };
    let mut table = [0u8; 64];
    for i in 0..64 {
        let val = (base[i] as u32 * s + 50) / 100;
        table[i] = val.clamp(1, 255) as u8;
    }
    table
}

fn fdct_2d(block: &mut [f32; 64], coeff: &[[f32; 8]; 8]) {
    let mut temp = [0.0f32; 64];
    for y in 0..8 {
        for u in 0..8 {
            let mut sum = 0.0f32;
            for x in 0..8 {
                sum += block[y * 8 + x] * coeff[u][x];
            }
            temp[y * 8 + u] = sum;
        }
    }
    for u in 0..8 {
        for v in 0..8 {
            let mut sum = 0.0f32;
            for y in 0..8 {
                sum += temp[y * 8 + u] * coeff[v][y];
            }
            block[v * 8 + u] = sum;
        }
    }
}

fn quantize_and_zigzag(block: &[f32; 64], q_table: &[u8; 64]) -> [i16; 64] {
    let mut out = [0i16; 64];
    for k in 0..64 {
        let idx = ZIGZAG[k];
        let coeff = block[idx];
        let q = q_table[idx] as f32;
        let rounded = (coeff / q).round();
        if k == 0 {
            out[k] = rounded.clamp(-2047.0, 2047.0) as i16;
        } else {
            out[k] = rounded.clamp(-1023.0, 1023.0) as i16;
        }
    }
    out
}

struct BitWriter {
    buffer: u32,
    bits_in_buffer: u8,
    output: Vec<u8>,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            buffer: 0,
            bits_in_buffer: 0,
            output: Vec::with_capacity(32 * 1024),
        }
    }

    fn write_bits(&mut self, code: u16, len: u8) {
        if len == 0 {
            return;
        }
        self.buffer = (self.buffer << len) | ((code as u32) & ((1u32 << len) - 1));
        self.bits_in_buffer += len;
        while self.bits_in_buffer >= 8 {
            self.bits_in_buffer -= 8;
            let byte = ((self.buffer >> self.bits_in_buffer) & 0xFF) as u8;
            self.output.push(byte);
            if byte == 0xFF {
                self.output.push(0x00);
            }
        }
    }

    fn flush(&mut self) {
        if self.bits_in_buffer > 0 {
            let pad = 8 - self.bits_in_buffer;
            let byte = (((self.buffer << pad) | ((1u32 << pad) - 1)) & 0xFF) as u8;
            self.output.push(byte);
            if byte == 0xFF {
                self.output.push(0x00);
            }
            self.buffer = 0;
            self.bits_in_buffer = 0;
        }
    }
}

fn bit_length(val: i32) -> u8 {
    let abs_val = val.unsigned_abs();
    if abs_val == 0 {
        0
    } else {
        32 - abs_val.leading_zeros() as u8
    }
}

fn value_bits(val: i16, cat: u8) -> u16 {
    if val >= 0 {
        val as u16
    } else {
        (val as i32 + ((1i32 << cat) - 1)) as u16
    }
}

fn encode_block(
    block: &[i16; 64],
    dc_prev: &mut i16,
    dc_lut: &[HuffmanCode; 256],
    ac_lut: &[HuffmanCode; 256],
    writer: &mut BitWriter,
) {
    let raw_diff = (block[0] as i32) - (*dc_prev as i32);
    let diff = raw_diff.clamp(-2047, 2047) as i16;
    *dc_prev = (*dc_prev as i32 + diff as i32) as i16;

    let cat = bit_length(diff as i32);
    let dc_code = dc_lut[cat as usize];
    writer.write_bits(dc_code.code, dc_code.len);
    if cat > 0 {
        let v = value_bits(diff, cat);
        writer.write_bits(v, cat);
    }

    let mut r = 0u8;
    for k in 1..64 {
        let val = block[k];
        if val == 0 {
            r += 1;
        } else {
            while r >= 16 {
                let zrl = ac_lut[0xF0];
                writer.write_bits(zrl.code, zrl.len);
                r -= 16;
            }
            let cat = bit_length(val as i32);
            let symbol = (r << 4) | cat;
            let code = ac_lut[symbol as usize];
            writer.write_bits(code.code, code.len);
            let v = value_bits(val, cat);
            writer.write_bits(v, cat);
            r = 0;
        }
    }
    if r > 0 {
        let eob = ac_lut[0x00];
        writer.write_bits(eob.code, eob.len);
    }
}

pub fn encode_rgba_jpeg_bytes(w: usize, h: usize, rgba: &[u8], quality: u8) -> Vec<u8> {
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return Vec::new();
    }

    let q_luma = scale_quant_table(&STD_LUMA_QUANT, quality);
    let q_chroma = scale_quant_table(&STD_CHROMA_QUANT, quality);

    let mut out = Vec::with_capacity(w * h / 2 + 1024);

    // SOI (0xFF, 0xD8)
    out.extend_from_slice(&[0xFF, 0xD8]);

    // APP0 JFIF (0xFF, 0xE0)
    out.extend_from_slice(&[
        0xFF, 0xE0, // marker
        0x00, 0x10, // length = 16
        b'J', b'F', b'I', b'F', 0x00, // identifier "JFIF\0"
        0x01, 0x01, // version 1.1
        0x00,       // density units (0 = no units)
        0x00, 0x01, // X density = 1
        0x00, 0x01, // Y density = 1
        0x00,       // thumbnail width = 0
        0x00,       // thumbnail height = 0
    ]);

    // DQT Luma (table 0)
    out.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x43, 0x00]);
    for &z in &ZIGZAG {
        out.push(q_luma[z]);
    }

    // DQT Chroma (table 1)
    out.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x43, 0x01]);
    for &z in &ZIGZAG {
        out.push(q_chroma[z]);
    }

    // SOF0 (Baseline DCT)
    out.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
    out.extend_from_slice(&(h as u16).to_be_bytes());
    out.extend_from_slice(&(w as u16).to_be_bytes());
    out.push(3); // 3 components
    out.extend_from_slice(&[0x01, 0x11, 0x00]); // Y: 1x1, table 0
    out.extend_from_slice(&[0x02, 0x11, 0x01]); // Cb: 1x1, table 1
    out.extend_from_slice(&[0x03, 0x11, 0x01]); // Cr: 1x1, table 1

    // DHT Luma DC
    out.extend_from_slice(&[0xFF, 0xC4]);
    let len_dc_luma = (2 + 1 + 16 + VALS_DC_LUMA.len()) as u16;
    out.extend_from_slice(&len_dc_luma.to_be_bytes());
    out.push(0x00);
    out.extend_from_slice(&BITS_DC_LUMA);
    out.extend_from_slice(&VALS_DC_LUMA);

    // DHT Luma AC
    out.extend_from_slice(&[0xFF, 0xC4]);
    let len_ac_luma = (2 + 1 + 16 + VALS_AC_LUMA.len()) as u16;
    out.extend_from_slice(&len_ac_luma.to_be_bytes());
    out.push(0x10);
    out.extend_from_slice(&BITS_AC_LUMA);
    out.extend_from_slice(&VALS_AC_LUMA);

    // DHT Chroma DC
    out.extend_from_slice(&[0xFF, 0xC4]);
    let len_dc_chroma = (2 + 1 + 16 + VALS_DC_CHROMA.len()) as u16;
    out.extend_from_slice(&len_dc_chroma.to_be_bytes());
    out.push(0x01);
    out.extend_from_slice(&BITS_DC_CHROMA);
    out.extend_from_slice(&VALS_DC_CHROMA);

    // DHT Chroma AC
    out.extend_from_slice(&[0xFF, 0xC4]);
    let len_ac_chroma = (2 + 1 + 16 + VALS_AC_CHROMA.len()) as u16;
    out.extend_from_slice(&len_ac_chroma.to_be_bytes());
    out.push(0x11);
    out.extend_from_slice(&BITS_AC_CHROMA);
    out.extend_from_slice(&VALS_AC_CHROMA);

    // SOS (Start of Scan)
    out.extend_from_slice(&[
        0xFF, 0xDA, // marker
        0x00, 0x0C, // length = 12
        0x03,       // 3 components
        0x01, 0x00, // Y: DC 0, AC 0
        0x02, 0x11, // Cb: DC 1, AC 1
        0x03, 0x11, // Cr: DC 1, AC 1
        0x00,       // Ss = 0
        0x3F,       // Se = 63
        0x00,       // Ah=0, Al=0
    ]);

    let mut writer = BitWriter::new();
    let luts = get_huffman_luts();
    let dct_coeff = get_dct_coeff();

    let mcu_cols = (w + 7) / 8;
    let mcu_rows = (h + 7) / 8;

    let mut dc_y = 0i16;
    let mut dc_cb = 0i16;
    let mut dc_cr = 0i16;

    let mut y_block = [0.0f32; 64];
    let mut cb_block = [0.0f32; 64];
    let mut cr_block = [0.0f32; 64];

    for mcu_y in 0..mcu_rows {
        for mcu_x in 0..mcu_cols {
            for by in 0..8 {
                let py = (mcu_y * 8 + by).min(h - 1);
                for bx in 0..8 {
                    let px = (mcu_x * 8 + bx).min(w - 1);
                    let idx = (py * w + px) * 4;
                    let r = rgba[idx] as f32;
                    let g = rgba[idx + 1] as f32;
                    let b = rgba[idx + 2] as f32;

                    let b_idx = by * 8 + bx;
                    y_block[b_idx] = 0.299 * r + 0.587 * g + 0.114 * b - 128.0;
                    cb_block[b_idx] = -0.168736 * r - 0.331264 * g + 0.5 * b;
                    cr_block[b_idx] = 0.5 * r - 0.418688 * g - 0.081312 * b;
                }
            }

            fdct_2d(&mut y_block, dct_coeff);
            let y_quant = quantize_and_zigzag(&y_block, &q_luma);
            encode_block(&y_quant, &mut dc_y, &luts.dc_luma, &luts.ac_luma, &mut writer);

            fdct_2d(&mut cb_block, dct_coeff);
            let cb_quant = quantize_and_zigzag(&cb_block, &q_chroma);
            encode_block(&cb_quant, &mut dc_cb, &luts.dc_chroma, &luts.ac_chroma, &mut writer);

            fdct_2d(&mut cr_block, dct_coeff);
            let cr_quant = quantize_and_zigzag(&cr_block, &q_chroma);
            encode_block(&cr_quant, &mut dc_cr, &luts.dc_chroma, &luts.ac_chroma, &mut writer);
        }
    }

    writer.flush();
    out.extend_from_slice(&writer.output);

    // EOI (0xFF, 0xD9)
    out.extend_from_slice(&[0xFF, 0xD9]);

    out
}

pub fn encode_rgba_jpeg_b64(w: usize, h: usize, rgba: &[u8], quality: u8) -> String {
    base64_encode(&encode_rgba_jpeg_bytes(w, h, rgba, quality))
}

pub fn encode_rgba_image_b64(
    w: usize,
    h: usize,
    rgba: &[u8],
    format: ImageFormat,
    quality: u8,
) -> String {
    match format {
        ImageFormat::Jpeg => encode_rgba_jpeg_b64(w, h, rgba, quality),
        ImageFormat::Png => encode_rgba_png_b64(w, h, rgba),
    }
}

/// Computes an 8x8 block-averaged luminance hex digest (128 hex chars), matching
/// `gateway::visual_digest_changed` (>= 32 hex chars).
pub fn compute_pixel_digest(width: usize, height: usize, rgba: &[u8]) -> String {
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return String::new();
    }
    const GRID: usize = 8;
    let mut out = String::with_capacity(GRID * GRID * 2);
    for gy in 0..GRID {
        let y0 = gy * height / GRID;
        let y1 = ((gy + 1) * height / GRID).max(y0 + 1).min(height);
        for gx in 0..GRID {
            let x0 = gx * width / GRID;
            let x1 = ((gx + 1) * width / GRID).max(x0 + 1).min(width);
            let mut sum = 0u64;
            let mut count = 0u64;
            let step_y = ((y1 - y0) / 8).max(1);
            let step_x = ((x1 - x0) / 8).max(1);
            let mut y = y0;
            while y < y1 {
                let mut x = x0;
                while x < x1 {
                    let i = (y * width + x) * 4;
                    let luma = (77u64 * rgba[i] as u64
                        + 150u64 * rgba[i + 1] as u64
                        + 29u64 * rgba[i + 2] as u64)
                        >> 8;
                    sum += luma;
                    count += 1;
                    x += step_x;
                }
                y += step_y;
            }
            let avg = if count > 0 { (sum / count) as u8 } else { 0 };
            out.push_str(&format!("{avg:02x}"));
        }
    }
    out
}

pub fn downscale_rgba(rgba: &[u8], src_w: usize, src_h: usize, max_w: usize) -> (usize, usize, Vec<u8>) {
    if src_w <= max_w || src_w == 0 || src_h == 0 || max_w == 0 {
        return (src_w, src_h, rgba.to_vec());
    }
    let dst_w = max_w;
    let dst_h = ((src_h as f64) * (dst_w as f64) / (src_w as f64))
        .round()
        .max(1.0) as usize;
    let mut out = vec![0u8; dst_w * dst_h * 4];
    for dy in 0..dst_h {
        let sy = (dy * src_h / dst_h).min(src_h - 1);
        for dx in 0..dst_w {
            let sx = (dx * src_w / dst_w).min(src_w - 1);
            let si = (sy * src_w + sx) * 4;
            let di = (dy * dst_w + dx) * 4;
            out[di..di + 4].copy_from_slice(&rgba[si..si + 4]);
        }
    }
    (dst_w, dst_h, out)
}

// MARK: - Minimal RFC 1951 Inflate + PNG Decoder

struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_buf: u32,
    bit_cnt: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_buf: 0,
            bit_cnt: 0,
        }
    }

    fn ensure(&mut self, n: u8) -> Result<(), String> {
        while self.bit_cnt < n {
            let b = *self
                .data
                .get(self.byte_pos)
                .ok_or_else(|| "unexpected end of deflate stream".to_string())?;
            self.byte_pos += 1;
            self.bit_buf |= (b as u32) << self.bit_cnt;
            self.bit_cnt += 8;
        }
        Ok(())
    }

    fn read_bits(&mut self, n: u8) -> Result<u32, String> {
        if n == 0 {
            return Ok(0);
        }
        self.ensure(n)?;
        let val = self.bit_buf & ((1u32 << n) - 1);
        self.bit_buf >>= n;
        self.bit_cnt -= n;
        Ok(val)
    }

    fn align_byte(&mut self) {
        let rem = self.bit_cnt % 8;
        if rem > 0 {
            self.bit_buf >>= rem;
            self.bit_cnt -= rem;
        }
    }

    fn read_u16_aligned(&mut self) -> Result<u16, String> {
        self.align_byte();
        let lo = self.read_bits(8)?;
        let hi = self.read_bits(8)?;
        Ok(((hi << 8) | lo) as u16)
    }
}

struct HuffTable {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl HuffTable {
    fn build(lengths: &[u8]) -> Result<Self, String> {
        let mut counts = [0u16; 16];
        for &len in lengths {
            if (len as usize) >= counts.len() {
                return Err("huffman length overflow".into());
            }
            counts[len as usize] += 1;
        }
        let mut offsets = [0u16; 16];
        for len in 1..15 {
            offsets[len + 1] = offsets[len] + counts[len];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (sym, &len) in lengths.iter().enumerate() {
            if len != 0 {
                let idx = offsets[len as usize] as usize;
                offsets[len as usize] += 1;
                if idx >= symbols.len() {
                    return Err("huffman symbol overflow".into());
                }
                symbols[idx] = sym as u16;
            }
        }
        Ok(Self { counts, symbols })
    }

    fn decode(&self, reader: &mut BitReader<'_>) -> Result<u16, String> {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for len in 1..=15 {
            code |= reader.read_bits(1)? as i32;
            let count = self.counts[len] as i32;
            if code - count < first {
                let slot = (index + (code - first)) as usize;
                return self
                    .symbols
                    .get(slot)
                    .copied()
                    .ok_or_else(|| "huffman slot out of bounds".into());
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err("invalid huffman code".into())
    }
}

fn inflate_zlib(zlib: &[u8]) -> Result<Vec<u8>, String> {
    if zlib.len() < 6 {
        return Err("zlib stream too short".into());
    }
    let cmf = zlib[0];
    let flg = zlib[1];
    if (cmf & 0x0F) != 8 || (((cmf as u16) * 256 + (flg as u16)) % 31) != 0 {
        return Err("invalid zlib header".into());
    }
    let mut start = 2usize;
    if (flg & 0x20) != 0 {
        start += 4;
    }
    if start >= zlib.len() {
        return Err("truncated zlib payload".into());
    }
    let mut reader = BitReader::new(&zlib[start..]);
    let mut out = Vec::new();

    const LEN_BASE: [u16; 29] = [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
        131, 163, 195, 227, 258,
    ];
    const LEN_EXT: [u8; 29] = [
        0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
    ];
    const DIST_BASE: [u16; 30] = [
        1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025,
        1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
    ];
    const DIST_EXT: [u8; 30] = [
        0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
        13, 13,
    ];
    const CL_ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];

    loop {
        let bfinal = reader.read_bits(1)?;
        let btype = reader.read_bits(2)?;
        match btype {
            0 => {
                let len = reader.read_u16_aligned()? as usize;
                let nlen = reader.read_u16_aligned()?;
                if len as u16 != !nlen {
                    return Err("stored block length mismatch".into());
                }
                for _ in 0..len {
                    out.push(reader.read_bits(8)? as u8);
                }
            }
            1 | 2 => {
                let (lit_tbl, dist_tbl) = if btype == 1 {
                    let mut lit_lens = vec![0u8; 288];
                    for b in &mut lit_lens[0..=143] {
                        *b = 8;
                    }
                    for b in &mut lit_lens[144..=255] {
                        *b = 9;
                    }
                    for b in &mut lit_lens[256..=279] {
                        *b = 7;
                    }
                    for b in &mut lit_lens[280..=287] {
                        *b = 8;
                    }
                    let dist_lens = vec![5u8; 32];
                    (HuffTable::build(&lit_lens)?, HuffTable::build(&dist_lens)?)
                } else {
                    let hlit = reader.read_bits(5)? as usize + 257;
                    let hdist = reader.read_bits(5)? as usize + 1;
                    let hclen = reader.read_bits(4)? as usize + 4;
                    let mut cl_lens = [0u8; 19];
                    for i in 0..hclen {
                        cl_lens[CL_ORDER[i]] = reader.read_bits(3)? as u8;
                    }
                    let cl_tbl = HuffTable::build(&cl_lens)?;
                    let total = hlit + hdist;
                    let mut lens = Vec::with_capacity(total);
                    while lens.len() < total {
                        let sym = cl_tbl.decode(&mut reader)?;
                        match sym {
                            0..=15 => lens.push(sym as u8),
                            16 => {
                                let prev = *lens
                                    .last()
                                    .ok_or_else(|| "invalid repeat code".to_string())?;
                                let rep = reader.read_bits(2)? as usize + 3;
                                lens.extend(std::iter::repeat_n(prev, rep));
                            }
                            17 => {
                                let rep = reader.read_bits(3)? as usize + 3;
                                lens.extend(std::iter::repeat_n(0u8, rep));
                            }
                            18 => {
                                let rep = reader.read_bits(7)? as usize + 11;
                                lens.extend(std::iter::repeat_n(0u8, rep));
                            }
                            _ => return Err("invalid code length symbol".into()),
                        }
                    }
                    if lens.len() != total {
                        return Err("code length overshoot".into());
                    }
                    (
                        HuffTable::build(&lens[..hlit])?,
                        HuffTable::build(&lens[hlit..])?,
                    )
                };

                loop {
                    let sym = lit_tbl.decode(&mut reader)?;
                    if sym < 256 {
                        out.push(sym as u8);
                    } else if sym == 256 {
                        break;
                    } else {
                        let lidx = (sym - 257) as usize;
                        if lidx >= LEN_BASE.len() {
                            return Err("invalid length symbol".into());
                        }
                        let length = LEN_BASE[lidx] as usize + reader.read_bits(LEN_EXT[lidx])? as usize;
                        let dsym = dist_tbl.decode(&mut reader)? as usize;
                        if dsym >= DIST_BASE.len() {
                            return Err("invalid distance symbol".into());
                        }
                        let dist = DIST_BASE[dsym] as usize + reader.read_bits(DIST_EXT[dsym])? as usize;
                        if dist == 0 || dist > out.len() {
                            return Err("invalid backreference distance".into());
                        }
                        for _ in 0..length {
                            let b = out[out.len() - dist];
                            out.push(b);
                        }
                    }
                }
            }
            _ => return Err("reserved deflate block type".into()),
        }
        if bfinal == 1 {
            break;
        }
    }
    Ok(out)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let a = a as i32;
    let b = b as i32;
    let c = c as i32;
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

pub fn decode_png_rgba(png: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    if png.len() < 33 || &png[0..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("invalid png signature".into());
    }
    let mut pos = 8usize;
    let mut width = 0usize;
    let mut height = 0usize;
    let mut color_type = 0u8;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut idat = Vec::new();

    while pos + 12 <= png.len() {
        let len = u32::from_be_bytes([png[pos], png[pos + 1], png[pos + 2], png[pos + 3]]) as usize;
        let tag = &png[pos + 4..pos + 8];
        let data_start = pos + 8;
        let data_end = data_start.checked_add(len).ok_or("chunk length overflow")?;
        if data_end + 4 > png.len() {
            return Err("truncated png chunk".into());
        }
        let data = &png[data_start..data_end];
        match tag {
            b"IHDR" => {
                if len < 13 {
                    return Err("invalid IHDR".into());
                }
                width = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
                height = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
                let bit_depth = data[8];
                color_type = data[9];
                if bit_depth != 8 || data[10] != 0 || data[11] != 0 || data[12] != 0 {
                    return Err("unsupported png format".into());
                }
            }
            b"PLTE" => {
                for rgb in data.chunks_exact(3) {
                    palette.push([rgb[0], rgb[1], rgb[2]]);
                }
            }
            b"tRNS" => {
                trns.extend_from_slice(data);
            }
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        pos = data_end + 4;
    }

    if width == 0 || height == 0 {
        return Err("missing PNG IHDR dimensions".into());
    }
    let channels = match color_type {
        0 => 1usize,
        2 => 3usize,
        3 => 1usize,
        4 => 2usize,
        6 => 4usize,
        _ => return Err("unsupported png color type".into()),
    };
    let raw = inflate_zlib(&idat)?;
    let stride = width * channels;
    if raw.len() < height * (stride + 1) {
        return Err("truncated decoded PNG scanlines".into());
    }
    let mut prev_row = vec![0u8; stride];
    let mut row = vec![0u8; stride];
    let mut rgba = Vec::with_capacity(width * height * 4);

    for y in 0..height {
        let base = y * (stride + 1);
        let filter = raw[base];
        let src = &raw[base + 1..base + 1 + stride];
        for i in 0..stride {
            let a = if i >= channels { row[i - channels] } else { 0 };
            let b = prev_row[i];
            let c = if i >= channels { prev_row[i - channels] } else { 0 };
            row[i] = match filter {
                0 => src[i],
                1 => src[i].wrapping_add(a),
                2 => src[i].wrapping_add(b),
                3 => src[i].wrapping_add((((a as u16) + (b as u16)) >> 1) as u8),
                4 => src[i].wrapping_add(paeth(a, b, c)),
                _ => return Err("unsupported png filter".into()),
            };
        }
        match color_type {
            0 => {
                for &g in &row {
                    rgba.extend_from_slice(&[g, g, g, 255]);
                }
            }
            2 => {
                for rgb in row.chunks_exact(3) {
                    rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
                }
            }
            3 => {
                for &idx in &row {
                    let [r, g, b] = palette.get(idx as usize).copied().unwrap_or([0, 0, 0]);
                    let a = trns.get(idx as usize).copied().unwrap_or(255);
                    rgba.extend_from_slice(&[r, g, b, a]);
                }
            }
            4 => {
                for ga in row.chunks_exact(2) {
                    rgba.extend_from_slice(&[ga[0], ga[0], ga[0], ga[1]]);
                }
            }
            6 => rgba.extend_from_slice(&row),
            _ => unreachable!(),
        }
        prev_row.copy_from_slice(&row);
    }

    Ok((width, height, rgba))
}

// MARK: - Pure-Rust Set-of-Mark RGBA Overlay Renderer

fn blend_pixel(rgba: &mut [u8], width: usize, height: usize, x: i32, y: i32, col: [u8; 4]) {
    if x < 0 || y < 0 || (x as usize) >= width || (y as usize) >= height {
        return;
    }
    let idx = (y as usize * width + x as usize) * 4;
    let a = col[3] as u32;
    if a == 255 {
        rgba[idx] = col[0];
        rgba[idx + 1] = col[1];
        rgba[idx + 2] = col[2];
        rgba[idx + 3] = 255;
        return;
    }
    let inv = 255 - a;
    rgba[idx] = ((col[0] as u32 * a + rgba[idx] as u32 * inv) / 255) as u8;
    rgba[idx + 1] = ((col[1] as u32 * a + rgba[idx + 1] as u32 * inv) / 255) as u8;
    rgba[idx + 2] = ((col[2] as u32 * a + rgba[idx + 2] as u32 * inv) / 255) as u8;
    rgba[idx + 3] = 255;
}

fn fill_rect(rgba: &mut [u8], w: usize, h: usize, x: i32, y: i32, rw: i32, rh: i32, col: [u8; 4]) {
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + rw).min(w as i32);
    let y1 = (y + rh).min(h as i32);
    for py in y0..y1 {
        for px in x0..x1 {
            blend_pixel(rgba, w, h, px, py, col);
        }
    }
}

fn stroke_rect(
    rgba: &mut [u8],
    w: usize,
    h: usize,
    x: i32,
    y: i32,
    rw: i32,
    rh: i32,
    thick: i32,
    col: [u8; 4],
) {
    let t = thick.max(1);
    fill_rect(rgba, w, h, x, y, rw, t, col);
    fill_rect(rgba, w, h, x, y + rh - t, rw, t, col);
    fill_rect(rgba, w, h, x, y, t, rh, col);
    fill_rect(rgba, w, h, x + rw - t, y, t, rh, col);
}

fn glyph_5x7(ch: char) -> [u8; 7] {
    match ch {
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        'm' => [0b00000, 0b00000, 0b11010, 0b10101, 0b10101, 0b10001, 0b10001],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'e' => [0b00000, 0b00000, 0b01110, 0b10001, 0b11111, 0b10000, 0b01110],
        '[' => [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110],
        ']' => [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        _ => [0b00000, 0b01110, 0b10001, 0b00110, 0b00100, 0b00000, 0b00100],
    }
}

fn draw_text_5x7(
    rgba: &mut [u8],
    w: usize,
    h: usize,
    x: i32,
    y: i32,
    scale: i32,
    text: &str,
    col: [u8; 4],
) {
    let mut cx = x;
    for ch in text.chars() {
        let rows = glyph_5x7(ch);
        for (ry, bits) in rows.iter().enumerate() {
            for rx in 0..5 {
                if (bits >> (4 - rx)) & 1 == 1 {
                    fill_rect(
                        rgba,
                        w,
                        h,
                        cx + rx * scale,
                        y + (ry as i32) * scale,
                        scale,
                        scale,
                        col,
                    );
                }
            }
        }
        cx += 6 * scale;
    }
}

pub fn render_som_image_to_b64(
    img_or_cap_id: &str,
    marks_json: &str,
    win_frame: &Rect,
    format: ImageFormat,
    quality: u8,
    max_width: usize,
) -> Result<String, DriverError> {
    if win_frame.w <= 0.0 || win_frame.h <= 0.0 {
        return Err(DriverError::new("invalid_argument", "Invalid window frame dimensions"));
    }
    let from_cache = img_or_cap_id.starts_with("cap_");
    let (src_w, src_h, src_rgba) = if from_cache {
        let frame = take_cached_frame(img_or_cap_id)
            .ok_or_else(|| DriverError::new("capture_cache_miss", "Cached capture frame expired"))?;
        (frame.width, frame.height, frame.rgba)
    } else {
        let png_bytes = base64_decode(img_or_cap_id)
            .map_err(|e| DriverError::new("invalid_argument", e))?;
        decode_png_rgba(&png_bytes).map_err(|e| DriverError::new("invalid_png", e))?
    };

    let target_max_w = max_width.clamp(640, 2560);
    let (img_w, img_h, mut rgba) = if src_w > target_max_w {
        downscale_rgba(&src_rgba, src_w, src_h, target_max_w)
    } else {
        (src_w, src_h, src_rgba)
    };

    let marks_val = parse_json(marks_json).map_err(|e| DriverError::new("invalid_marks", e))?;
    let marks = marks_val
        .as_array()
        .ok_or_else(|| DriverError::new("invalid_marks", "Marks JSON must be an array"))?;

    let sx = img_w as f64 / win_frame.w;
    let sy = img_h as f64 / win_frame.h;
    let font_scale = if img_w >= 960 { 2 } else { 1 };

    for m in marks {
        let mid = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if mid.is_empty() {
            continue;
        }
        let source = m.get("source").and_then(|v| v.as_str()).unwrap_or("ax");
        let gx = m.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let gy = m.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let gw = m.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let gh = m.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0);

        let lx = ((gx - win_frame.x) * sx).round() as i32;
        let ly = ((gy - win_frame.y) * sy).round() as i32;
        let lw = (gw * sx).round().max(8.0) as i32;
        let lh = (gh * sy).round().max(8.0) as i32;

        if lx + lw < 0 || ly + lh < 0 || lx > img_w as i32 || ly > img_h as i32 {
            continue;
        }

        let is_ocr = source == "ocr";
        let stroke = if is_ocr { [245, 158, 10, 240] } else { [15, 199, 166, 240] };
        let fill = if is_ocr { [245, 158, 10, 32] } else { [15, 199, 166, 28] };

        fill_rect(&mut rgba, img_w, img_h, lx, ly, lw, lh, fill);
        stroke_rect(&mut rgba, img_w, img_h, lx, ly, lw, lh, 2, stroke);

        let char_count = mid.chars().count() as i32;
        let pad_x = 3 * font_scale;
        let pad_y = 2 * font_scale;
        let pill_w = char_count * 6 * font_scale + pad_x * 2 - font_scale;
        let pill_h = 7 * font_scale + pad_y * 2;
        let bx = lx.clamp(0, (img_w as i32 - pill_w).max(0));
        let by = if ly >= pill_h { ly - pill_h } else { ly.clamp(0, (img_h as i32 - pill_h).max(0)) };

        fill_rect(&mut rgba, img_w, img_h, bx, by, pill_w, pill_h, [15, 23, 42, 235]);
        stroke_rect(&mut rgba, img_w, img_h, bx, by, pill_w, pill_h, 1, stroke);
        draw_text_5x7(
            &mut rgba,
            img_w,
            img_h,
            bx + pad_x,
            by + pad_y,
            font_scale,
            mid,
            [255, 255, 255, 255],
        );
    }

    Ok(encode_rgba_image_b64(img_w, img_h, &rgba, format, quality))
}

pub fn render_som_rgba_to_b64(
    png_b64_or_cap_id: &str,
    marks_json: &str,
    win_frame: &Rect,
) -> Result<String, DriverError> {
    render_som_image_to_b64(
        png_b64_or_cap_id,
        marks_json,
        win_frame,
        ImageFormat::Png,
        80,
        1440,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_roundtrip_and_som_overlay_and_digest() {
        let w = 120usize;
        let h = 80usize;
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                rgba[i] = (x * 2) as u8;
                rgba[i + 1] = (y * 3) as u8;
                rgba[i + 2] = 180;
                rgba[i + 3] = 255;
            }
        }
        let d1 = compute_pixel_digest(w, h, &rgba);
        assert_eq!(d1.len(), 128);

        let b64 = encode_rgba_png_b64(w, h, &rgba);
        let decoded = decode_png_rgba(&base64_decode(&b64).unwrap()).unwrap();
        assert_eq!((decoded.0, decoded.1), (w, h));
        assert_eq!(decoded.2, rgba);

        let out_b64 = render_som_rgba_to_b64(
            &b64,
            r#"[{"id":"m1","source":"ax","x":110,"y":110,"w":40,"h":20},{"id":"m2","source":"ocr","x":160,"y":140,"w":50,"h":16}]"#,
            &Rect {
                x: 100.0,
                y: 100.0,
                w: 120.0,
                h: 80.0,
            },
        )
        .unwrap();
        let (_, _, after) = decode_png_rgba(&base64_decode(&out_b64).unwrap()).unwrap();
        let d2 = compute_pixel_digest(w, h, &after);
        assert_ne!(d1, d2);
    }

    #[test]
    fn cache_frame_stores_and_downscales_wide_images() {
        let cap_id = cache_rgba_frame("lin", 1600, 900, vec![64u8; 1600 * 900 * 4]);
        let b64 = render_som_rgba_to_b64(
            &cap_id,
            r#"[{"id":"m1","source":"ax","x":50,"y":50,"w":100,"h":40}]"#,
            &Rect {
                x: 0.0,
                y: 0.0,
                w: 1600.0,
                h: 900.0,
            },
        )
        .unwrap();
        let (dw, dh, _) = decode_png_rgba(&base64_decode(&b64).unwrap()).unwrap();
        assert_eq!((dw, dh), (1440, 810));
    }

    #[test]
    fn decode_sub_filter_stored_png() {
        let raw_scan = vec![1u8, 10, 20, 30, 5, 10, 15]; // pixel0=(10,20,30), pixel1=(15,30,45)
        let mut zlib = vec![0x78u8, 0x01, 0x01];
        zlib.extend_from_slice(&(raw_scan.len() as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(raw_scan.len() as u16)).to_le_bytes());
        zlib.extend_from_slice(&raw_scan);
        zlib.extend_from_slice(&adler32(&raw_scan).to_be_bytes());
        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        push_png_chunk(&mut png, b"IHDR", &ihdr);
        push_png_chunk(&mut png, b"IDAT", &zlib);
        push_png_chunk(&mut png, b"IEND", &[]);
        let (w, h, rgba) = decode_png_rgba(&png).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgba, vec![10, 20, 30, 255, 15, 30, 45, 255]);
    }

    #[test]
    fn decode_compressed_png_with_zlib_and_up_filter() {
        let zlib_png: [u8; 86] = [
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 4, 0, 0, 0, 4,
            8, 6, 0, 0, 0, 169, 241, 158, 126, 0, 0, 0, 29, 73, 68, 65, 84, 120, 218, 99, 96, 96,
            216, 242, 95, 3, 136, 3, 128, 184, 2, 136, 153, 24, 140, 24, 24, 144, 49, 97, 1, 0, 1,
            54, 10, 27, 189, 141, 100, 167, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ];
        let (w, h, rgba) = decode_png_rgba(&zlib_png).unwrap();
        assert_eq!((w, h), (4, 4));
        assert_eq!(&rgba[0..4], &[0, 0, 180, 255]);
        assert_eq!(&rgba[4 * 4..4 * 4 + 4], &[0, 50, 180, 255]);
        assert_eq!(&rgba[(3 * 4 + 2) * 4..(3 * 4 + 2) * 4 + 4], &[80, 150, 180, 255]);
    }

    #[test]
    fn jpeg_encoder_writes_valid_baseline_jfif_for_non_multiple_of_eight_dims() {
        let w = 123usize;
        let h = 77usize;
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                rgba[i] = (x * 2) as u8;
                rgba[i + 1] = (y * 3) as u8;
                rgba[i + 2] = ((x + y) % 256) as u8;
                rgba[i + 3] = 255;
            }
        }

        let q80 = encode_rgba_jpeg_bytes(w, h, &rgba, 80);
        let q95 = encode_rgba_jpeg_bytes(w, h, &rgba, 95);

        // Assert starts with SOI (0xFF, 0xD8) + APP0 (0xFF, 0xE0)
        assert!(q80.starts_with(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(q95.starts_with(&[0xFF, 0xD8, 0xFF, 0xE0]));

        // Assert ends with EOI (0xFF, 0xD9)
        assert!(q80.ends_with(&[0xFF, 0xD9]));
        assert!(q95.ends_with(&[0xFF, 0xD9]));

        // Assert SOF0 contains big-endian height 77 and width 123
        fn find_sof0_dims(bytes: &[u8]) -> Option<(usize, usize)> {
            let mut i = 0;
            while i + 9 <= bytes.len() {
                if bytes[i] == 0xFF && bytes[i + 1] == 0xC0 {
                    let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as usize;
                    let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as usize;
                    return Some((w, h));
                }
                i += 1;
            }
            None
        }

        assert_eq!(find_sof0_dims(&q80), Some((123, 77)));
        assert_eq!(find_sof0_dims(&q95), Some((123, 77)));

        // Higher quality should yield larger compressed file
        assert!(
            q95.len() > q80.len(),
            "q95 len {} should be > q80 len {}",
            q95.len(),
            q80.len()
        );
    }

    #[test]
    fn render_som_image_supports_jpeg_and_custom_max_width() {
        let cap_id = cache_rgba_frame("lin", 1600, 900, vec![64u8; 1600 * 900 * 4]);
        let marks_json = r#"[{"id":"m1","source":"ax","x":50,"y":50,"w":100,"h":40}]"#;
        let win = Rect {
            x: 0.0,
            y: 0.0,
            w: 1600.0,
            h: 900.0,
        };

        let b64 = render_som_image_to_b64(&cap_id, marks_json, &win, ImageFormat::Jpeg, 80, 1280)
            .unwrap();
        let bytes = base64_decode(&b64).unwrap();

        assert!(bytes.starts_with(&[0xFF, 0xD8, 0xFF]));
        assert!(bytes.ends_with(&[0xFF, 0xD9]));

        let mut dims = None;
        let mut i = 0;
        while i + 9 <= bytes.len() {
            if bytes[i] == 0xFF && bytes[i + 1] == 0xC0 {
                let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as usize;
                let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as usize;
                dims = Some((w, h));
                break;
            }
            i += 1;
        }
        assert_eq!(dims, Some((1280, 720)));
    }
}

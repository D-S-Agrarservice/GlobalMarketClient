//! Pure-Rust `.webp` / `.png` / `.jpg` / `.dds` -> GIANTS-compliant `512x256` BC3 (`DXT5`) `.dds`
//! texture converter with 10 prebuilt mipmaps and deterministic `logos/logo_<hash>.dds` caching.
//!
//! Layout specification (matching `FS25_GlobalMarket` `GlobalMarketBranding.lua` & `GlobalMarketUI.xml`):
//! - Target canvas: `512 x 256` RGBA8 (`2:1` aspect ratio, power-of-two `2^9 x 2^8`), transparent `[0, 0, 0, 0]`.
//! - Aspect-preserving `Lanczos3` fit (premultiplied-alpha filtered to prevent dark alpha halos).
//! - Bottom-left placement (`x = 0`, `y = 256 - scaled_height`) so both wide `600x256` banners and
//!   square/round `512x512` crests sit flush above `marketStatusTitle` inside `marketCustomLogo` (`240x120`).
//! - Complete 10-level mipmap chain (`512x256` down to `1x1`) encoded as standard 128-byte header `DXT5` `.dds`.

use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{ColorType, ImageEncoder, ImageReader, Limits, Rgba, RgbaImage};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

static LOGO_STAGING_SEQ: AtomicU64 = AtomicU64::new(1);

/// Fixed power-of-two width (`2^9`) of generated custom market `.dds` logos.
pub const LOGO_CANVAS_WIDTH: u32 = 512;
/// Fixed power-of-two height (`2^8`) of generated custom market `.dds` logos (`2:1` aspect ratio).
pub const LOGO_CANVAS_HEIGHT: u32 = 256;
/// Full mipmap count from `512x256` down to `1x1` (`max(log2(512), log2(256)) + 1 = 10`).
pub const LOGO_MIPMAP_COUNT: u32 = 10;
/// Exact deterministic file size in bytes of a `512x256` 10-mipmap `DXT5` `.dds` file (`128` header + `174,800` data).
pub const EXPECTED_LOGO_DDS_BYTES: usize = 174_928;
/// Maximum accepted raw input logo file/response size (`8 MiB`).
pub const MAX_LOGO_RAW_BYTES: usize = 8 * 1024 * 1024;
/// Maximum accepted source image width or height (`4096 px`).
pub const MAX_LOGO_DIMENSION: u32 = 4096;

/// Built-in official D-S-Agrarservice logo (`src/assets/dsa-logo.png`), rendered in-game whenever
/// no custom Community/Logo token is active (both Official Global Market and Tier-2 Private Markets).
pub const DEFAULT_DSA_LOGO_PNG: &[u8] = include_bytes!("../../../src/assets/dsa-logo.png");
const DEFAULT_DSA_LOGO_SEED: &[u8] = b"builtin://dsa-default-logo-v1";

const DDS_MAGIC: &[u8; 4] = b"DDS ";
const DDS_HEADER_SIZE: u32 = 124;
const DDS_PIXELFORMAT_SIZE: u32 = 32;
// DDSD_CAPS (0x1) | DDSD_HEIGHT (0x2) | DDSD_WIDTH (0x4) | DDSD_PIXELFORMAT (0x1000)
// | DDSD_MIPMAPCOUNT (0x20000) | DDSD_LINEARSIZE (0x80000)
const DDSD_FLAGS_MIPMAPPED_LINEAR: u32 = 0x000A_1007;
const DDPF_FOURCC: u32 = 0x0000_0004;
const DDPF_RGB: u32 = 0x0000_0040;
const DDPF_ALPHAPIXELS: u32 = 0x0000_0001;
// DDSCAPS_COMPLEX (0x8) | DDSCAPS_TEXTURE (0x1000) | DDSCAPS_MIPMAP (0x400000)
const DDSCAPS_MIPMAPPED_TEXTURE: u32 = 0x0040_1008;

/// Computes a deterministic 12-character lowercase hex SHA-256 prefix for `logos/logo_<hash>.dds`.
pub fn logo_cache_hash12(seed: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"FS25_GM_Logo_DDS_v1:");
    hasher.update(seed);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(12);
    for &byte in &digest[..6] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Formats the relative `modSettings/FS<YY>_GlobalMarket/` path (`logos/logo_<12char_sha256>.dds`).
pub fn logo_rel_path_for_hash(hash12: &str) -> String {
    format!("logos/logo_{hash12}.dds")
}

/// Deterministic relative path (`logos/logo_<hash12>.dds`) for the built-in default DSA market logo.
pub fn default_dsa_logo_rel_path() -> String {
    logo_rel_path_for_hash(&logo_cache_hash12(DEFAULT_DSA_LOGO_SEED))
}

/// Ensures the built-in DSA logo (`src/assets/dsa-logo.png`) is converted once and cached as a
/// GIANTS-compliant `512x256` 10-mipmap `DXT5` `.dds` inside `logos_dir` without any server download.
pub fn ensure_default_dsa_logo_dds(staging_dir: &Path, logos_dir: &Path) -> Option<String> {
    let hash12 = logo_cache_hash12(DEFAULT_DSA_LOGO_SEED);
    let rel_logo = logo_rel_path_for_hash(&hash12);
    let target_dds = logos_dir.join(format!("logo_{hash12}.dds"));
    if is_valid_cached_logo_dds(&target_dds) {
        return Some(rel_logo);
    }
    convert_logo_bytes_to_dds_file(DEFAULT_DSA_LOGO_PNG, staging_dir, &target_dds)
        .ok()
        .map(|()| rel_logo)
}

/// Verifies whether `dds_path` already exists on disk as a complete, valid `512x256` 10-mip `DXT5` `.dds` texture.
pub fn is_valid_cached_logo_dds(dds_path: &Path) -> bool {
    let Ok(meta) = fs::metadata(dds_path) else {
        return false;
    };
    if !meta.is_file() || meta.len() != EXPECTED_LOGO_DDS_BYTES as u64 {
        return false;
    }

    let Ok(mut file) = File::open(dds_path) else {
        return false;
    };
    let mut hdr = [0u8; 128];
    if file.read_exact(&mut hdr).is_err() {
        return false;
    }

    &hdr[0..4] == DDS_MAGIC
        && u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) == DDS_HEADER_SIZE
        && u32::from_le_bytes([hdr[12], hdr[13], hdr[14], hdr[15]]) == LOGO_CANVAS_HEIGHT
        && u32::from_le_bytes([hdr[16], hdr[17], hdr[18], hdr[19]]) == LOGO_CANVAS_WIDTH
        && u32::from_le_bytes([hdr[28], hdr[29], hdr[30], hdr[31]]) == LOGO_MIPMAP_COUNT
        && &hdr[84..88] == b"DXT5"
}

/// Decodes `.webp`, `.png`, `.jpg`, or `.dds` bytes in memory, composes the aspect-preserved
/// bottom-left `512x256` canvas, encodes 10 `DXT5` mipmap levels, and writes atomically via `staging_dir`.
pub fn convert_logo_bytes_to_dds_file(
    raw_bytes: &[u8],
    staging_dir: &Path,
    target_dds_path: &Path,
) -> Result<(), String> {
    let dds_bytes = convert_logo_bytes_to_dds_bytes(raw_bytes)?;
    atomic_write_binary(staging_dir, target_dds_path, &dds_bytes)
}

/// Pure in-memory pipeline: raw `.webp` / `.png` / `.jpg` / `.dds` bytes -> `174,928`-byte `DXT5` `.dds` buffer.
pub fn convert_logo_bytes_to_dds_bytes(raw_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let decoded = decode_logo_rgba8(raw_bytes)?;
    let canvas = compose_bottom_left_logo_canvas(&decoded)?;
    encode_rgba8_to_dxt5_dds(&canvas)
}

/// Decodes `.webp`, `.png`, `.jpg`, or `.dds` image bytes into an `RgbaImage` with strict memory/dimension guards.
pub fn decode_logo_rgba8(raw_bytes: &[u8]) -> Result<RgbaImage, String> {
    if raw_bytes.is_empty() {
        return Err("Logo image data is empty.".to_string());
    }
    if raw_bytes.len() > MAX_LOGO_RAW_BYTES {
        return Err("Logo image exceeds 8 MB limit.".to_string());
    }

    if raw_bytes.len() >= 128 && &raw_bytes[0..4] == DDS_MAGIC {
        return decode_dds_to_rgba8(raw_bytes);
    }

    let mut reader = ImageReader::new(Cursor::new(raw_bytes))
        .with_guessed_format()
        .map_err(|e| format!("Failed to inspect image format: {e}"))?;

    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_LOGO_DIMENSION);
    limits.max_image_height = Some(MAX_LOGO_DIMENSION);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);

    let dyn_img = reader
        .decode()
        .map_err(|e| format!("Failed to decode logo image: {e}"))?;

    let (w, h) = (dyn_img.width(), dyn_img.height());
    if w == 0 || h == 0 || w > MAX_LOGO_DIMENSION || h > MAX_LOGO_DIMENSION {
        return Err(format!("Invalid logo dimensions ({w}x{h})."));
    }

    Ok(dyn_img.to_rgba8())
}

/// Proportionally scales `src` with premultiplied-alpha `Lanczos3` to fit within `512x256` without
/// distortion and composites it **bottom-left** (`x = 0`, `y = 256 - scaled_height`) on a transparent canvas.
pub fn compose_bottom_left_logo_canvas(src: &RgbaImage) -> Result<RgbaImage, String> {
    let (src_w, src_h) = src.dimensions();
    if src_w == 0 || src_h == 0 {
        return Err("Source logo has zero width or height.".to_string());
    }

    let scale = (LOGO_CANVAS_WIDTH as f64 / src_w as f64)
        .min(LOGO_CANVAS_HEIGHT as f64 / src_h as f64);
    let scaled_w = ((src_w as f64 * scale).round() as u32).clamp(1, LOGO_CANVAS_WIDTH);
    let scaled_h = ((src_h as f64 * scale).round() as u32).clamp(1, LOGO_CANVAS_HEIGHT);

    let resized = if scaled_w == src_w && scaled_h == src_h {
        src.clone()
    } else {
        resize_lanczos3_premultiplied(src, scaled_w, scaled_h)
    };

    let mut canvas = RgbaImage::from_pixel(
        LOGO_CANVAS_WIDTH,
        LOGO_CANVAS_HEIGHT,
        Rgba([0, 0, 0, 0]),
    );

    let y_offset = LOGO_CANVAS_HEIGHT - scaled_h;
    for y in 0..scaled_h {
        for x in 0..scaled_w {
            let px = *resized.get_pixel(x, y);
            canvas.put_pixel(x, y_offset + y, px);
        }
    }

    Ok(canvas)
}

/// Encodes an `RgbaImage` as PNG bytes in memory (used for Tauri WebView preview of `.dds` files in Mock mode).
pub fn encode_rgba8_to_png_bytes(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let encoder = PngEncoder::new(&mut out);
    encoder
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ColorType::Rgba8.into(),
        )
        .map_err(|e| format!("Failed to encode PNG preview: {e}"))?;
    Ok(out)
}

/// Encodes a `512x256` `RgbaImage` into a classic 128-byte header `DXT5` / `BC3` `.dds` buffer
/// containing all 10 mipmap levels (`512x256` down to `1x1`).
pub fn encode_rgba8_to_dxt5_dds(canvas: &RgbaImage) -> Result<Vec<u8>, String> {
    let (width, height) = canvas.dimensions();
    if width != LOGO_CANVAS_WIDTH || height != LOGO_CANVAS_HEIGHT {
        return Err(format!(
            "DDS encoder expects {}x{} canvas, got {width}x{height}.",
            LOGO_CANVAS_WIDTH, LOGO_CANVAS_HEIGHT
        ));
    }

    let mut out = Vec::with_capacity(EXPECTED_LOGO_DDS_BYTES);
    let top_linear_size = (width.div_ceil(4)) * (height.div_ceil(4)) * 16;
    write_dxt5_dds_header(&mut out, width, height, top_linear_size, LOGO_MIPMAP_COUNT);

    let mut current_mip = canvas.clone();
    for level in 0..LOGO_MIPMAP_COUNT {
        encode_mip_surface_dxt5(&current_mip, &mut out);
        if level + 1 < LOGO_MIPMAP_COUNT {
            current_mip = downsample_2x_alpha_weighted(&current_mip);
        }
    }

    debug_assert_eq!(out.len(), EXPECTED_LOGO_DDS_BYTES);
    Ok(out)
}

/// Decodes top-level mipmap `0` of a `DXT5`, `DXT1`, or uncompressed 32-bit RGBA/BGRA `.dds` buffer into `RgbaImage`.
pub fn decode_dds_to_rgba8(dds_bytes: &[u8]) -> Result<RgbaImage, String> {
    if dds_bytes.len() < 128 || &dds_bytes[0..4] != DDS_MAGIC {
        return Err("Invalid DDS header magic.".to_string());
    }

    let hdr_size = u32::from_le_bytes([dds_bytes[4], dds_bytes[5], dds_bytes[6], dds_bytes[7]]);
    if hdr_size != DDS_HEADER_SIZE {
        return Err(format!("Unsupported DDS header size: {hdr_size}."));
    }

    let height = u32::from_le_bytes([dds_bytes[12], dds_bytes[13], dds_bytes[14], dds_bytes[15]]);
    let width = u32::from_le_bytes([dds_bytes[16], dds_bytes[17], dds_bytes[18], dds_bytes[19]]);
    if width == 0 || height == 0 || width > MAX_LOGO_DIMENSION || height > MAX_LOGO_DIMENSION {
        return Err(format!("Invalid DDS dimensions ({width}x{height})."));
    }

    let pf_flags = u32::from_le_bytes([dds_bytes[80], dds_bytes[81], dds_bytes[82], dds_bytes[83]]);
    let four_cc = &dds_bytes[84..88];
    let data = &dds_bytes[128..];

    if (pf_flags & DDPF_FOURCC) != 0 {
        match four_cc {
            b"DXT5" => decode_dxt5_surface(data, width, height),
            b"DXT1" => decode_dxt1_surface(data, width, height),
            b"DX10" => {
                if data.len() < 20 {
                    return Err("Truncated DX10 DDS header.".to_string());
                }
                let dxgi_format = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                let payload = &data[20..];
                // DXGI_FORMAT_BC3_UNORM (77) / BC3_UNORM_SRGB (78), BC1_UNORM (71) / BC1_UNORM_SRGB (72)
                match dxgi_format {
                    77 | 78 => decode_dxt5_surface(payload, width, height),
                    71 | 72 => decode_dxt1_surface(payload, width, height),
                    _ => Err(format!("Unsupported DX10 DXGI format ({dxgi_format}). Please use WebP, PNG, JPG, or DXT5/BC3 DDS.")),
                }
            }
            _ => Err("Unsupported DDS FourCC compression. Please use WebP, PNG, JPG, or DXT5/BC3 DDS.".to_string()),
        }
    } else if (pf_flags & DDPF_RGB) != 0 {
        let bit_count = u32::from_le_bytes([dds_bytes[88], dds_bytes[89], dds_bytes[90], dds_bytes[91]]);
        let r_mask = u32::from_le_bytes([dds_bytes[92], dds_bytes[93], dds_bytes[94], dds_bytes[95]]);
        let b_mask = u32::from_le_bytes([dds_bytes[100], dds_bytes[101], dds_bytes[102], dds_bytes[103]]);
        let has_alpha = (pf_flags & DDPF_ALPHAPIXELS) != 0;
        if bit_count != 32 {
            return Err(format!("Unsupported uncompressed DDS bit depth ({bit_count})."));
        }
        let needed = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or_else(|| "DDS dimensions overflow.".to_string())?;
        if data.len() < needed {
            return Err("Truncated uncompressed DDS pixel buffer.".to_string());
        }
        let is_bgra = b_mask == 0x0000_00FF && r_mask == 0x00FF_0000;
        let mut img = RgbaImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let idx = ((y * width + x) as usize) * 4;
                let (r, g, b) = if is_bgra {
                    (data[idx + 2], data[idx + 1], data[idx])
                } else {
                    (data[idx], data[idx + 1], data[idx + 2])
                };
                let a = if has_alpha { data[idx + 3] } else { 255 };
                img.put_pixel(x, y, Rgba([r, g, b, a]));
            }
        }
        Ok(img)
    } else {
        Err("Unrecognized DDS pixel format.".to_string())
    }
}

fn write_dxt5_dds_header(
    out: &mut Vec<u8>,
    width: u32,
    height: u32,
    top_linear_size: u32,
    mip_count: u32,
) {
    out.extend_from_slice(DDS_MAGIC);
    out.extend_from_slice(&DDS_HEADER_SIZE.to_le_bytes());
    out.extend_from_slice(&DDSD_FLAGS_MIPMAPPED_LINEAR.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&top_linear_size.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes()); // dwDepth
    out.extend_from_slice(&mip_count.to_le_bytes());
    out.extend_from_slice(&[0u8; 44]); // dwReserved1[11]

    // DDS_PIXELFORMAT (32 bytes)
    out.extend_from_slice(&DDS_PIXELFORMAT_SIZE.to_le_bytes());
    out.extend_from_slice(&DDPF_FOURCC.to_le_bytes());
    out.extend_from_slice(b"DXT5");
    out.extend_from_slice(&[0u8; 20]); // RGBBitCount + R/G/B/A masks

    // dwCaps + dwCaps2..dwReserved2 (20 bytes)
    out.extend_from_slice(&DDSCAPS_MIPMAPPED_TEXTURE.to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
}

fn resize_lanczos3_premultiplied(src: &RgbaImage, dst_w: u32, dst_h: u32) -> RgbaImage {
    let (src_w, src_h) = src.dimensions();
    let mut premul = RgbaImage::new(src_w, src_h);
    for (x, y, px) in src.enumerate_pixels() {
        let [r, g, b, a] = px.0;
        let a32 = a as u32;
        let pr = ((r as u32 * a32 + 127) / 255) as u8;
        let pg = ((g as u32 * a32 + 127) / 255) as u8;
        let pb = ((b as u32 * a32 + 127) / 255) as u8;
        premul.put_pixel(x, y, Rgba([pr, pg, pb, a]));
    }

    let resized = image::imageops::resize(&premul, dst_w, dst_h, FilterType::Lanczos3);
    let mut out = RgbaImage::new(dst_w, dst_h);
    for (x, y, px) in resized.enumerate_pixels() {
        let [pr, pg, pb, a] = px.0;
        if a == 0 {
            out.put_pixel(x, y, Rgba([0, 0, 0, 0]));
        } else {
            let a32 = a as u32;
            let half = a32 / 2;
            let r = ((pr as u32 * 255 + half) / a32).min(255) as u8;
            let g = ((pg as u32 * 255 + half) / a32).min(255) as u8;
            let b = ((pb as u32 * 255 + half) / a32).min(255) as u8;
            out.put_pixel(x, y, Rgba([r, g, b, a]));
        }
    }
    out
}

fn downsample_2x_alpha_weighted(src: &RgbaImage) -> RgbaImage {
    let (w, h) = src.dimensions();
    let next_w = (w / 2).max(1);
    let next_h = (h / 2).max(1);
    let mut dst = RgbaImage::new(next_w, next_h);

    for dy in 0..next_h {
        let sy0 = (dy * 2).min(h - 1);
        let sy1 = (dy * 2 + 1).min(h - 1);
        for dx in 0..next_w {
            let sx0 = (dx * 2).min(w - 1);
            let sx1 = (dx * 2 + 1).min(w - 1);

            let samples = [
                src.get_pixel(sx0, sy0).0,
                src.get_pixel(sx1, sy0).0,
                src.get_pixel(sx0, sy1).0,
                src.get_pixel(sx1, sy1).0,
            ];

            let mut a_sum = 0_u32;
            let mut r_acc = 0_u32;
            let mut g_acc = 0_u32;
            let mut b_acc = 0_u32;

            for [r, g, b, a] in samples {
                let aw = a as u32;
                a_sum += aw;
                r_acc += r as u32 * aw;
                g_acc += g as u32 * aw;
                b_acc += b as u32 * aw;
            }

            let out_a = ((a_sum + 2) / 4) as u8;
            if a_sum == 0 {
                dst.put_pixel(dx, dy, Rgba([0, 0, 0, 0]));
            } else {
                let half = a_sum / 2;
                let out_r = ((r_acc + half) / a_sum).min(255) as u8;
                let out_g = ((g_acc + half) / a_sum).min(255) as u8;
                let out_b = ((b_acc + half) / a_sum).min(255) as u8;
                dst.put_pixel(dx, dy, Rgba([out_r, out_g, out_b, out_a]));
            }
        }
    }

    dst
}

fn encode_mip_surface_dxt5(img: &RgbaImage, out: &mut Vec<u8>) {
    let (w, h) = img.dimensions();
    let blocks_x = w.div_ceil(4);
    let blocks_y = h.div_ceil(4);

    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let mut tile = [[0u8; 4]; 16];
            for py in 0..4 {
                let sy = (by * 4 + py).min(h - 1);
                for px in 0..4 {
                    let sx = (bx * 4 + px).min(w - 1);
                    tile[(py * 4 + px) as usize] = img.get_pixel(sx, sy).0;
                }
            }
            let block = encode_bc3_block(&tile);
            out.extend_from_slice(&block);
        }
    }
}

fn encode_bc3_block(pixels: &[[u8; 4]; 16]) -> [u8; 16] {
    let mut alphas = [0u8; 16];
    let mut any_visible = false;
    for (i, px) in pixels.iter().enumerate() {
        alphas[i] = px[3];
        if px[3] > 0 {
            any_visible = true;
        }
    }

    if !any_visible {
        return [0u8; 16];
    }

    let alpha_bytes = encode_bc3_alpha_block(&alphas);
    let color_bytes = encode_bc1_opaque_color_block(pixels);

    let mut out = [0u8; 16];
    out[0..8].copy_from_slice(&alpha_bytes);
    out[8..16].copy_from_slice(&color_bytes);
    out
}

fn encode_bc3_alpha_block(alphas: &[u8; 16]) -> [u8; 8] {
    let mut min_a = 255u8;
    let mut max_a = 0u8;
    for &a in alphas {
        min_a = min_a.min(a);
        max_a = max_a.max(a);
    }

    if min_a == max_a {
        return [max_a, min_a, 0, 0, 0, 0, 0, 0];
    }

    // Mode A: 8-alpha interpolation (`alpha0 > alpha1`)
    let (err8, block8) = fit_alpha_endpoints(max_a, min_a, alphas);

    // Mode B: 6-alpha interpolation + exact `0` and `255` (`alpha0 <= alpha1`)
    let mut min_inner = 255u8;
    let mut max_inner = 0u8;
    let mut has_inner = false;
    for &a in alphas {
        if a > 0 && a < 255 {
            min_inner = min_inner.min(a);
            max_inner = max_inner.max(a);
            has_inner = true;
        }
    }
    let (a0_mode6, a1_mode6) = if has_inner {
        (min_inner, max_inner)
    } else {
        (0, 255)
    };
    let (err6, block6) = fit_alpha_endpoints(a0_mode6, a1_mode6, alphas);

    if err6 <= err8 {
        block6
    } else {
        block8
    }
}

fn fit_alpha_endpoints(a0: u8, a1: u8, alphas: &[u8; 16]) -> (u32, [u8; 8]) {
    let palette = build_bc3_alpha_palette(a0, a1);
    let mut total_err = 0_u32;
    let mut packed_indices = 0_u64;

    for (i, &target) in alphas.iter().enumerate() {
        let mut best_idx = 0_u64;
        let mut best_err = u32::MAX;
        for (idx, &cand) in palette.iter().enumerate() {
            let diff = (target as i32) - (cand as i32);
            let sq = (diff * diff) as u32;
            if sq < best_err {
                best_err = sq;
                best_idx = idx as u64;
                if sq == 0 {
                    break;
                }
            }
        }
        total_err += best_err;
        packed_indices |= best_idx << (3 * i);
    }

    let idx_le = packed_indices.to_le_bytes();
    let mut out = [0u8; 8];
    out[0] = a0;
    out[1] = a1;
    out[2..8].copy_from_slice(&idx_le[0..6]);
    (total_err, out)
}

fn build_bc3_alpha_palette(a0: u8, a1: u8) -> [u8; 8] {
    let p0 = a0 as u32;
    let p1 = a1 as u32;
    if a0 > a1 {
        [
            a0,
            a1,
            ((6 * p0 + p1 + 3) / 7) as u8,
            ((5 * p0 + 2 * p1 + 3) / 7) as u8,
            ((4 * p0 + 3 * p1 + 3) / 7) as u8,
            ((3 * p0 + 4 * p1 + 3) / 7) as u8,
            ((2 * p0 + 5 * p1 + 3) / 7) as u8,
            ((p0 + 6 * p1 + 3) / 7) as u8,
        ]
    } else {
        [
            a0,
            a1,
            ((4 * p0 + p1 + 2) / 5) as u8,
            ((3 * p0 + 2 * p1 + 2) / 5) as u8,
            ((2 * p0 + 3 * p1 + 2) / 5) as u8,
            ((p0 + 4 * p1 + 2) / 5) as u8,
            0,
            255,
        ]
    }
}

fn encode_bc1_opaque_color_block(pixels: &[[u8; 4]; 16]) -> [u8; 8] {
    // Compute alpha-weighted mean and bounding box across visible pixels (`alpha > 0`).
    let mut w_sum = 0.0_f32;
    let mut mean = [0.0_f32; 3];
    let mut min_rgb = [255u8; 3];
    let mut max_rgb = [0u8; 3];

    for px in pixels {
        if px[3] == 0 {
            continue;
        }
        let w = px[3] as f32;
        w_sum += w;
        for c in 0..3 {
            mean[c] += (px[c] as f32) * w;
            min_rgb[c] = min_rgb[c].min(px[c]);
            max_rgb[c] = max_rgb[c].max(px[c]);
        }
    }

    if min_rgb == max_rgb {
        let c = pack_rgb565(min_rgb[0], min_rgb[1], min_rgb[2]);
        let mut out = [0u8; 8];
        out[0..2].copy_from_slice(&c.to_le_bytes());
        out[2..4].copy_from_slice(&c.to_le_bytes());
        // All indices = 0 (`color0`) so GPU decode is unambiguous across all graphics APIs.
        return out;
    }

    let inv_w = 1.0 / w_sum.max(1.0);
    mean[0] *= inv_w;
    mean[1] *= inv_w;
    mean[2] *= inv_w;

    // 3x3 symmetric covariance matrix for principal color axis extraction.
    let mut cov = [0.0_f32; 6]; // xx, xy, xz, yy, yz, zz
    for px in pixels {
        if px[3] == 0 {
            continue;
        }
        let w = px[3] as f32;
        let dx = px[0] as f32 - mean[0];
        let dy = px[1] as f32 - mean[1];
        let dz = px[2] as f32 - mean[2];
        cov[0] += w * dx * dx;
        cov[1] += w * dx * dy;
        cov[2] += w * dx * dz;
        cov[3] += w * dy * dy;
        cov[4] += w * dy * dz;
        cov[5] += w * dz * dz;
    }

    // Power iteration initialized with bounding-box diagonal.
    let mut axis = [
        (max_rgb[0] as f32 - min_rgb[0] as f32).max(1e-3),
        (max_rgb[1] as f32 - min_rgb[1] as f32).max(1e-3),
        (max_rgb[2] as f32 - min_rgb[2] as f32).max(1e-3),
    ];
    for _ in 0..4 {
        let nx = cov[0] * axis[0] + cov[1] * axis[1] + cov[2] * axis[2];
        let ny = cov[1] * axis[0] + cov[3] * axis[1] + cov[4] * axis[2];
        let nz = cov[2] * axis[0] + cov[4] * axis[1] + cov[5] * axis[2];
        let norm = (nx * nx + ny * ny + nz * nz).sqrt();
        if norm > 1e-6 {
            axis = [nx / norm, ny / norm, nz / norm];
        }
    }

    let mut min_proj = f32::INFINITY;
    let mut max_proj = f32::NEG_INFINITY;
    let mut p_min = min_rgb;
    let mut p_max = max_rgb;

    for px in pixels {
        if px[3] == 0 {
            continue;
        }
        let proj = (px[0] as f32 - mean[0]) * axis[0]
            + (px[1] as f32 - mean[1]) * axis[1]
            + (px[2] as f32 - mean[2]) * axis[2];
        if proj < min_proj {
            min_proj = proj;
            p_min = [px[0], px[1], px[2]];
        }
        if proj > max_proj {
            max_proj = proj;
            p_max = [px[0], px[1], px[2]];
        }
    }

    let cand_pca = (
        pack_rgb565(p_max[0], p_max[1], p_max[2]),
        pack_rgb565(p_min[0], p_min[1], p_min[2]),
    );
    let cand_bbox = (
        pack_rgb565(max_rgb[0], max_rgb[1], max_rgb[2]),
        pack_rgb565(min_rgb[0], min_rgb[1], min_rgb[2]),
    );

    let (mut best_err, mut best_block) = evaluate_bc1_pair(cand_pca.0, cand_pca.1, pixels);
    let (bbox_err, bbox_block) = evaluate_bc1_pair(cand_bbox.0, cand_bbox.1, pixels);
    if bbox_err < best_err {
        best_err = bbox_err;
        best_block = bbox_block;
    }

    // Two least-squares endpoint refinement passes on the winning initial cluster assignment.
    let mut cur_indices = u32::from_le_bytes([
        best_block[4],
        best_block[5],
        best_block[6],
        best_block[7],
    ]);

    for _ in 0..2 {
        if let Some((ref_c0, ref_c1)) = refine_bc1_endpoints_lsq(pixels, cur_indices) {
            let (ref_err, ref_block) = evaluate_bc1_pair(ref_c0, ref_c1, pixels);
            if ref_err < best_err {
                best_err = ref_err;
                best_block = ref_block;
                cur_indices = u32::from_le_bytes([
                    best_block[4],
                    best_block[5],
                    best_block[6],
                    best_block[7],
                ]);
            } else {
                break;
            }
        } else {
            break;
        }
    }

    best_block
}

fn refine_bc1_endpoints_lsq(pixels: &[[u8; 4]; 16], indices: u32) -> Option<(u16, u16)> {
    // Palette weights for indices 0 -> 1.0, 1 -> 0.0, 2 -> 2/3, 3 -> 1/3
    const WEIGHT_A: [f32; 4] = [1.0, 0.0, 2.0 / 3.0, 1.0 / 3.0];
    let mut ata00 = 0.0_f32;
    let mut ata01 = 0.0_f32;
    let mut ata11 = 0.0_f32;
    let mut atb0 = [0.0_f32; 3];
    let mut atb1 = [0.0_f32; 3];

    for (i, px) in pixels.iter().enumerate() {
        if px[3] == 0 {
            continue;
        }
        let idx = ((indices >> (2 * i)) & 0x3) as usize;
        let wa = WEIGHT_A[idx];
        let wb = 1.0 - wa;
        let alpha_w = (px[3] as f32) / 255.0;

        ata00 += alpha_w * wa * wa;
        ata01 += alpha_w * wa * wb;
        ata11 += alpha_w * wb * wb;
        for c in 0..3 {
            let val = px[c] as f32;
            atb0[c] += alpha_w * wa * val;
            atb1[c] += alpha_w * wb * val;
        }
    }

    let det = ata00 * ata11 - ata01 * ata01;
    if det.abs() < 1e-4 {
        return None;
    }
    let inv_det = 1.0 / det;
    let mut e0 = [0u8; 3];
    let mut e1 = [0u8; 3];
    for c in 0..3 {
        let v0 = (ata11 * atb0[c] - ata01 * atb1[c]) * inv_det;
        let v1 = (ata00 * atb1[c] - ata01 * atb0[c]) * inv_det;
        e0[c] = v0.round().clamp(0.0, 255.0) as u8;
        e1[c] = v1.round().clamp(0.0, 255.0) as u8;
    }

    Some((
        pack_rgb565(e0[0], e0[1], e0[2]),
        pack_rgb565(e1[0], e1[1], e1[2]),
    ))
}

fn evaluate_bc1_pair(raw_c0: u16, raw_c1: u16, pixels: &[[u8; 4]; 16]) -> (u64, [u8; 8]) {
    if raw_c0 == raw_c1 {
        let rgb = unpack_rgb565(raw_c0);
        let mut total_err = 0_u64;
        for px in pixels {
            if px[3] == 0 {
                continue;
            }
            total_err += weighted_rgb_sq_err(px, rgb);
        }
        let mut out = [0u8; 8];
        out[0..2].copy_from_slice(&raw_c0.to_le_bytes());
        out[2..4].copy_from_slice(&raw_c1.to_le_bytes());
        return (total_err, out);
    }

    // Enforce `c0 > c1` for standard 4-color interpolation mode.
    let (c0, c1) = if raw_c0 > raw_c1 {
        (raw_c0, raw_c1)
    } else {
        (raw_c1, raw_c0)
    };

    let palette = build_bc1_four_color_palette(c0, c1);
    let mut total_err = 0_u64;
    let mut packed_indices = 0_u32;

    for (i, px) in pixels.iter().enumerate() {
        // Assign index 0 (`color0`) to invisible pixels so bilinear borders inherit clean foreground RGB.
        if px[3] == 0 {
            continue;
        }
        let mut best_idx = 0_u32;
        let mut best_err = u64::MAX;
        for (idx, &cand_rgb) in palette.iter().enumerate() {
            let err = weighted_rgb_sq_err(px, cand_rgb);
            if err < best_err {
                best_err = err;
                best_idx = idx as u32;
                if err == 0 {
                    break;
                }
            }
        }
        total_err += best_err;
        packed_indices |= best_idx << (2 * i);
    }

    let mut out = [0u8; 8];
    out[0..2].copy_from_slice(&c0.to_le_bytes());
    out[2..4].copy_from_slice(&c1.to_le_bytes());
    out[4..8].copy_from_slice(&packed_indices.to_le_bytes());
    (total_err, out)
}

#[inline]
fn weighted_rgb_sq_err(px: &[u8; 4], cand: [u8; 3]) -> u64 {
    let dr = (px[0] as i32) - (cand[0] as i32);
    let dg = (px[1] as i32) - (cand[1] as i32);
    let db = (px[2] as i32) - (cand[2] as i32);
    // Perceptual luminance weights (2:4:1) scaled by pixel alpha.
    let dist = (2 * dr * dr + 4 * dg * dg + db * db) as u64;
    dist * (px[3] as u64)
}

#[inline]
fn pack_rgb565(r: u8, g: u8, b: u8) -> u16 {
    let r5 = ((r as u32 * 31 + 127) / 255) as u16;
    let g6 = ((g as u32 * 63 + 127) / 255) as u16;
    let b5 = ((b as u32 * 31 + 127) / 255) as u16;
    (r5 << 11) | (g6 << 5) | b5
}

#[inline]
fn unpack_rgb565(c: u16) -> [u8; 3] {
    let r5 = ((c >> 11) & 0x1F) as u8;
    let g6 = ((c >> 5) & 0x3F) as u8;
    let b5 = (c & 0x1F) as u8;
    [
        (r5 << 3) | (r5 >> 2),
        (g6 << 2) | (g6 >> 4),
        (b5 << 3) | (b5 >> 2),
    ]
}

#[inline]
fn build_bc1_four_color_palette(c0: u16, c1: u16) -> [[u8; 3]; 4] {
    let p0 = unpack_rgb565(c0);
    let p1 = unpack_rgb565(c1);
    let p2 = [
        ((2 * p0[0] as u16 + p1[0] as u16 + 1) / 3) as u8,
        ((2 * p0[1] as u16 + p1[1] as u16 + 1) / 3) as u8,
        ((2 * p0[2] as u16 + p1[2] as u16 + 1) / 3) as u8,
    ];
    let p3 = [
        ((p0[0] as u16 + 2 * p1[0] as u16 + 1) / 3) as u8,
        ((p0[1] as u16 + 2 * p1[1] as u16 + 1) / 3) as u8,
        ((p0[2] as u16 + 2 * p1[2] as u16 + 1) / 3) as u8,
    ];
    [p0, p1, p2, p3]
}

fn decode_dxt5_surface(data: &[u8], width: u32, height: u32) -> Result<RgbaImage, String> {
    let blocks_x = width.div_ceil(4) as usize;
    let blocks_y = height.div_ceil(4) as usize;
    let required = blocks_x
        .checked_mul(blocks_y)
        .and_then(|n| n.checked_mul(16))
        .ok_or_else(|| "DDS block dimensions overflow.".to_string())?;
    if data.len() < required {
        return Err("Truncated DXT5 DDS surface data.".to_string());
    }

    let mut img = RgbaImage::new(width, height);
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let offset = (by * blocks_x + bx) * 16;
            let block = &data[offset..offset + 16];

            let a_palette = build_bc3_alpha_palette(block[0], block[1]);
            let mut a_bits_le = [0u8; 8];
            a_bits_le[0..6].copy_from_slice(&block[2..8]);
            let a_bits = u64::from_le_bytes(a_bits_le);

            let c0 = u16::from_le_bytes([block[8], block[9]]);
            let c1 = u16::from_le_bytes([block[10], block[11]]);
            let c_palette = build_bc1_four_color_palette(c0, c1);
            let c_bits = u32::from_le_bytes([block[12], block[13], block[14], block[15]]);

            for py in 0..4usize {
                let dy = (by * 4 + py) as u32;
                if dy >= height {
                    continue;
                }
                for px in 0..4usize {
                    let dx = (bx * 4 + px) as u32;
                    if dx >= width {
                        continue;
                    }
                    let i = py * 4 + px;
                    let a_idx = ((a_bits >> (3 * i)) & 0x7) as usize;
                    let c_idx = ((c_bits >> (2 * i)) & 0x3) as usize;
                    let rgb = c_palette[c_idx];
                    img.put_pixel(dx, dy, Rgba([rgb[0], rgb[1], rgb[2], a_palette[a_idx]]));
                }
            }
        }
    }

    Ok(img)
}

fn decode_dxt1_surface(data: &[u8], width: u32, height: u32) -> Result<RgbaImage, String> {
    let blocks_x = width.div_ceil(4) as usize;
    let blocks_y = height.div_ceil(4) as usize;
    let required = blocks_x
        .checked_mul(blocks_y)
        .and_then(|n| n.checked_mul(8))
        .ok_or_else(|| "DDS block dimensions overflow.".to_string())?;
    if data.len() < required {
        return Err("Truncated DXT1 DDS surface data.".to_string());
    }

    let mut img = RgbaImage::new(width, height);
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let offset = (by * blocks_x + bx) * 8;
            let block = &data[offset..offset + 8];
            let c0 = u16::from_le_bytes([block[0], block[1]]);
            let c1 = u16::from_le_bytes([block[2], block[3]]);
            let p0 = unpack_rgb565(c0);
            let p1 = unpack_rgb565(c1);
            let palette: [[u8; 4]; 4] = if c0 > c1 {
                let full = build_bc1_four_color_palette(c0, c1);
                [
                    [full[0][0], full[0][1], full[0][2], 255],
                    [full[1][0], full[1][1], full[1][2], 255],
                    [full[2][0], full[2][1], full[2][2], 255],
                    [full[3][0], full[3][1], full[3][2], 255],
                ]
            } else {
                [
                    [p0[0], p0[1], p0[2], 255],
                    [p1[0], p1[1], p1[2], 255],
                    [
                        ((p0[0] as u16 + p1[0] as u16) / 2) as u8,
                        ((p0[1] as u16 + p1[1] as u16) / 2) as u8,
                        ((p0[2] as u16 + p1[2] as u16) / 2) as u8,
                        255,
                    ],
                    [0, 0, 0, 0],
                ]
            };
            let c_bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);

            for py in 0..4usize {
                let dy = (by * 4 + py) as u32;
                if dy >= height {
                    continue;
                }
                for px in 0..4usize {
                    let dx = (bx * 4 + px) as u32;
                    if dx >= width {
                        continue;
                    }
                    let i = py * 4 + px;
                    let c_idx = ((c_bits >> (2 * i)) & 0x3) as usize;
                    img.put_pixel(dx, dy, Rgba(palette[c_idx]));
                }
            }
        }
    }

    Ok(img)
}

fn atomic_write_binary(staging_dir: &Path, target_path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::create_dir_all(staging_dir)
        .map_err(|e| format!("Failed to create staging directory: {e}"))?;
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create logo cache directory: {e}"))?;
    }

    let seq = LOGO_STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_path = staging_dir.join(format!("stage_logo_{}_{}.tmp", std::process::id(), seq));

    {
        let mut file = File::create(&tmp_path)
            .map_err(|e| format!("Failed to create temporary DDS file: {e}"))?;
        file.write_all(bytes)
            .map_err(|e| format!("Failed to write temporary DDS file: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("Failed to flush temporary DDS file: {e}"))?;
    }

    let mut last_err = String::new();
    for attempt in 0..5 {
        match fs::rename(&tmp_path, target_path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = e.to_string();
                if attempt < 4 {
                    thread::sleep(Duration::from_millis(40));
                }
            }
        }
    }

    let _ = fs::remove_file(&tmp_path);
    Err(format!(
        "Atomic rename of DDS logo to '{}' failed: {last_err}",
        target_path.display()
    ))
}

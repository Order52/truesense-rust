//! Painting a picture across the keys: the picture is scaled to the 22 x 6 key grid.
//!
//! Decoding a file (PNG, JPEG, ...) needs a decoder crate, which this layer does not carry: the
//! caller supplies an [`ImageDecoder`]. What is done here is the part that decides how the
//! picture looks on the keyboard: Pillow's LANCZOS resampling, reproduced bit for bit, so that
//! pictures paint exactly as they did in the Python edition.
#![forbid(unsafe_code)]

use std::path::Path;

use super::DesignError;
use crate::keymap::{COLUMNS, Color, LIT_CELLS, NUM_CELLS, ROWS};

/// The most pixels a picture may have (a hostile or accidental huge file must not exhaust the
/// memory of the process that paints it). Decoders should refuse bigger pictures before decoding.
pub const MAX_PIXELS: usize = 64 << 20;

/// A decoded picture: 8-bit red, green, blue per pixel, row by row. Transparency is ignored the
/// way Pillow's `convert("RGB")` ignores it: the colour channels are used as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raster {
    /// Pixels per row.
    pub width: usize,
    /// Number of rows.
    pub height: usize,
    /// `width * height * 3` bytes.
    pub rgb: Vec<u8>,
}

/// Reads a picture file into a [`Raster`]. The error text is shown to the user after
/// "cannot read the image: ".
pub trait ImageDecoder: Send + Sync {
    /// Decodes the file at `path`.
    fn decode(&self, path: &Path) -> Result<Raster, String>;
}

/// Paints the picture in `path` across the keys: 128 colours, cell = column * 6 + row, the last
/// two cells black.
pub fn image_cells(decoder: &dyn ImageDecoder, path: &Path) -> Result<Vec<Color>, DesignError> {
    let raster = decoder.decode(path).map_err(DesignError::Image)?;
    image_cells_from_raster(&raster)
}

/// [`image_cells`] for a picture that is already decoded.
pub fn image_cells_from_raster(raster: &Raster) -> Result<Vec<Color>, DesignError> {
    let pixels = raster.width.checked_mul(raster.height).filter(|p| (1..=MAX_PIXELS).contains(p));
    let Some(pixels) = pixels else {
        return Err(DesignError::Image(format!(
            "a {}x{} picture is not usable",
            raster.width, raster.height
        )));
    };
    if raster.rgb.len() != pixels * 3 {
        return Err(DesignError::Image(format!(
            "a {}x{} picture needs {} bytes, not {}",
            raster.width,
            raster.height,
            pixels * 3,
            raster.rgb.len()
        )));
    }
    let small = resize_lanczos(&raster.rgb, raster.width, raster.height, COLUMNS, ROWS);
    let mut cells = vec![[0u8; 3]; NUM_CELLS];
    for col in 0..COLUMNS {
        for row in 0..ROWS {
            let cell = col * ROWS + row;
            if cell < LIT_CELLS {
                let at = (row * COLUMNS + col) * 3;
                cells[cell] = [small[at], small[at + 1], small[at + 2]];
            }
        }
    }
    Ok(cells)
}

/// Pillow's LANCZOS resize (a = 3, the support widened by the shrink factor so a big picture is
/// averaged, not sampled), in the same 22-bit fixed point and with the same 8-bit intermediate
/// picture between the two passes. An axis whose size already fits is left alone, as Pillow does.
///
/// Which pass runs first matters for the last bit of a few colours. Pillow 13 (the one this was
/// measured against, over 4800 picture shapes, with no exception) runs the vertical pass first
/// exactly when the picture is taller than the grid and `2 * (width - columns) < height - rows`;
/// otherwise the horizontal pass goes first, which is what every older Pillow did always.
fn resize_lanczos(src: &[u8], w: usize, h: usize, out_w: usize, out_h: usize) -> Vec<u8> {
    let vertical_first = h > out_h && 2 * (w as i64 - out_w as i64) < h as i64 - out_h as i64;
    let horizontal =
        |tmp: Vec<u8>, h: usize| if w != out_w { resize_axis(&tmp, w, h, out_w, true) } else { tmp };
    let vertical =
        |tmp: Vec<u8>, w: usize| if h != out_h { resize_axis(&tmp, w, h, out_h, false) } else { tmp };
    if vertical_first {
        horizontal(vertical(src.to_vec(), w), out_h)
    } else {
        vertical(horizontal(src.to_vec(), h), out_w)
    }
}

const PRECISION_BITS: u32 = 22;

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        let x = x * std::f64::consts::PI;
        x.sin() / x
    }
}

fn lanczos(x: f64) -> f64 {
    if x > -3.0 && x < 3.0 { sinc(x) * sinc(x / 3.0) } else { 0.0 }
}

/// The weights of one output pixel: the first input pixel they apply to, and the fixed-point
/// coefficients.
struct Kernel {
    first: usize,
    coeffs: Vec<i64>,
}

fn kernels(in_size: usize, out_size: usize) -> Vec<Kernel> {
    let scale = in_size as f64 / out_size as f64;
    let filter_scale = scale.max(1.0);
    let support = 3.0 * filter_scale;
    (0..out_size)
        .map(|xx| {
            let center = (xx as f64 + 0.5) * scale;
            let ss = 1.0 / filter_scale;
            let lo = ((center - support + 0.5) as i64).max(0) as usize;
            let hi = (((center + support + 0.5) as i64).max(0) as usize).min(in_size);
            let count = hi.saturating_sub(lo);
            let mut weights: Vec<f64> =
                (0..count).map(|x| lanczos(((x + lo) as f64 - center + 0.5) * ss)).collect();
            let sum: f64 = weights.iter().sum();
            let coeffs = weights
                .iter_mut()
                .map(|w| {
                    if sum != 0.0 {
                        *w /= sum;
                    }
                    let f = *w * f64::from(1u32 << PRECISION_BITS);
                    // Pillow rounds half away from zero.
                    if *w < 0.0 { (f - 0.5) as i64 } else { (f + 0.5) as i64 }
                })
                .collect();
            Kernel { first: lo, coeffs }
        })
        .collect()
}

fn clip8(v: i64) -> u8 {
    // Clamped to a byte first, so the conversion cannot truncate.
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

/// Resamples one axis of a `w` x `h` picture to `out_size` pixels along it.
fn resize_axis(src: &[u8], w: usize, h: usize, out_size: usize, horizontal: bool) -> Vec<u8> {
    let half = 1i64 << (PRECISION_BITS - 1);
    if horizontal {
        let ks = kernels(w, out_size);
        let mut dst = vec![0u8; out_size * h * 3];
        for y in 0..h {
            for (xx, k) in ks.iter().enumerate() {
                for c in 0..3 {
                    let mut acc = half;
                    for (i, coef) in k.coeffs.iter().enumerate() {
                        acc += i64::from(src[(y * w + k.first + i) * 3 + c]) * coef;
                    }
                    dst[(y * out_size + xx) * 3 + c] = clip8(acc);
                }
            }
        }
        return dst;
    }
    let ks = kernels(h, out_size);
    let mut dst = vec![0u8; w * out_size * 3];
    for (yy, k) in ks.iter().enumerate() {
        for x in 0..w {
            for c in 0..3 {
                let mut acc = half;
                for (i, coef) in k.coeffs.iter().enumerate() {
                    acc += i64::from(src[((k.first + i) * w + x) * 3 + c]) * coef;
                }
                dst[(yy * w + x) * 3 + c] = clip8(acc);
            }
        }
    }
    dst
}

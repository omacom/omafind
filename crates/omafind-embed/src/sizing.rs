//! How the reference sizes an image for the vision encoder, so llama.cpp sees the same pixels.
//!
//! mtmd fixes the token budget per context and rescales only images outside its range, while the reference
//! (transformers' Gemma image processor) always fills the budget. So images are resized here, with Pillow's bicubic
//! filter bit for bit, and mtmd takes them as they are.

/// The vision soft-token budgets the model accepts.
pub const BUDGETS: [usize; 5] = [70, 140, 280, 560, 1120];

/// transformers' Gemma image processor: scale to fill `budget` soft tokens (each 3 × 3 patches of 16 px) keeping the
/// aspect ratio, each side rounded down to a multiple of 48 px. None for an image too thin to hold a patch.
pub fn target_size(height: usize, width: usize, budget: usize) -> Option<(usize, usize)> {
    let (patch, pooling) = (16usize, 3usize);
    let max_patches = budget * pooling * pooling;
    let factor = ((max_patches * patch * patch) as f64 / (height * width) as f64).sqrt();
    let side = pooling * patch;
    let mut th = (factor * height as f64 / side as f64).floor() as usize * side;
    let mut tw = (factor * width as f64 / side as f64).floor() as usize * side;
    if th == 0 && tw == 0 {
        return None;
    }
    let max_side = (max_patches / (pooling * pooling)) * side;
    if th == 0 {
        th = side;
        tw = (width / height * side).min(max_side);
    } else if tw == 0 {
        tw = side;
        th = (height / width * side).min(max_side);
    }
    Some((th, tw))
}

const PRECISION: i32 = 22;

fn filter(value: f64) -> f64 {
    let (x, a) = (value.abs(), -0.5);
    if x < 1.0 {
        ((a + 2.0) * x - (a + 3.0)) * x * x + 1.0
    } else if x < 2.0 {
        (((x - 5.0) * x + 8.0) * x - 4.0) * a
    } else {
        0.0
    }
}

/// For each output pixel: the first contributing input pixel, how many contribute, and their weights.
fn coefficients(in_size: usize, out_size: usize) -> (Vec<usize>, Vec<i32>, usize) {
    let scale = in_size as f64 / out_size as f64;
    let filter_scale = scale.max(1.0);
    let support = 2.0 * filter_scale;
    let size = support.ceil() as usize * 2 + 1;
    let mut bounds = vec![0usize; out_size * 2];
    let mut weights = vec![0i32; out_size * size];
    let mut row = vec![0f64; size];
    for out in 0..out_size {
        let center = (out as f64 + 0.5) * scale;
        let first = ((center - support + 0.5) as i64).max(0) as usize;
        let count = (((center + support + 0.5) as i64) as usize).min(in_size) - first;
        let mut total = 0.0;
        for (x, slot) in row.iter_mut().enumerate() {
            *slot = if x < count {
                filter(((x + first) as f64 - center + 0.5) / filter_scale)
            } else {
                0.0
            };
            total += *slot;
        }
        for x in 0..size {
            let weight = if x < count && total != 0.0 {
                row[x] / total
            } else {
                row[x]
            };
            let rounding = if weight < 0.0 { -0.5 } else { 0.5 };
            weights[out * size + x] = (weight * (1i64 << PRECISION) as f64 + rounding) as i32;
        }
        bounds[out * 2] = first;
        bounds[out * 2 + 1] = count;
    }
    (bounds, weights, size)
}

#[inline]
fn clip(value: i32) -> u8 {
    (value >> PRECISION).clamp(0, 255) as u8
}

fn horizontal(src: &[u8], in_width: usize, rows: usize, out_width: usize) -> Vec<u8> {
    let (bounds, weights, size) = coefficients(in_width, out_width);
    let mut out = vec![0u8; out_width * rows * 3];
    for y in 0..rows {
        for x in 0..out_width {
            let mut p = (y * in_width + bounds[x * 2]) * 3;
            let (mut r, mut g, mut b) = (
                1i32 << (PRECISION - 1),
                1i32 << (PRECISION - 1),
                1i32 << (PRECISION - 1),
            );
            for k in 0..bounds[x * 2 + 1] {
                let w = weights[x * size + k];
                r = r.wrapping_add((src[p] as i32).wrapping_mul(w));
                g = g.wrapping_add((src[p + 1] as i32).wrapping_mul(w));
                b = b.wrapping_add((src[p + 2] as i32).wrapping_mul(w));
                p += 3;
            }
            let o = (y * out_width + x) * 3;
            out[o] = clip(r);
            out[o + 1] = clip(g);
            out[o + 2] = clip(b);
        }
    }
    out
}

fn vertical(src: &[u8], in_height: usize, row_width: usize, out_height: usize) -> Vec<u8> {
    let (bounds, weights, size) = coefficients(in_height, out_height);
    let stride = row_width * 3;
    let mut out = vec![0u8; stride * out_height];
    let mut sums = vec![0i32; stride];
    for y in 0..out_height {
        sums.fill(1i32 << (PRECISION - 1));
        for k in 0..bounds[y * 2 + 1] {
            let row = &src[(bounds[y * 2] + k) * stride..][..stride];
            let w = weights[y * size + k];
            for i in 0..stride {
                sums[i] = sums[i].wrapping_add((row[i] as i32).wrapping_mul(w));
            }
        }
        for i in 0..stride {
            out[y * stride + i] = clip(sums[i]);
        }
    }
    out
}

/// Pillow's bicubic resampling of packed RGB (a = −0.5, the kernel widened when shrinking, 22-bit fixed point): what
/// the reference gets from PIL's Image.resize.
pub fn resize_bicubic(
    source: &[u8],
    width: usize,
    height: usize,
    to_width: usize,
    to_height: usize,
) -> Vec<u8> {
    debug_assert_eq!(source.len(), width * height * 3);
    match (to_width != width, to_height != height) {
        (true, true) => {
            let pass = horizontal(source, width, height, to_width);
            vertical(&pass, height, to_width, to_height)
        }
        (true, false) => horizontal(source, width, height, to_width),
        (false, true) => vertical(source, height, width, to_height),
        (false, false) => source.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_the_budget_in_multiples_of_48() {
        // 1024 × 768 at 140 tokens: 140 × 9 patches × 256 px² = 322,560 px² → factor 0.6404 → 492 × 656 → 480 × 624.
        assert_eq!(target_size(768, 1024, 140), Some((480, 624)));
        // 1440 × 900 at 280 tokens: factor 0.7055 → 635 × 1016 → 624 × 1008.
        assert_eq!(target_size(900, 1440, 280), Some((624, 1008)));
        // A 1 px square scales up to fill the budget: 384 × 384 (8 × 8 soft tokens = 64 ≤ 70).
        assert_eq!(target_size(1, 1, 70), Some((384, 384)));
        // A strip too thin for a patch gets one patch row, its width capped at the budget's side.
        assert_eq!(target_size(1, 10000, 70), Some((48, 70 * 48)));
    }

    #[test]
    fn identity_resize_copies() {
        let src: Vec<u8> = (0..4 * 4 * 3).map(|i| i as u8).collect();
        assert_eq!(resize_bicubic(&src, 4, 4, 4, 4), src);
    }

    #[test]
    fn flat_color_stays_flat() {
        let src = vec![200u8; 10 * 6 * 3];
        let out = resize_bicubic(&src, 10, 6, 5, 3);
        assert_eq!(out.len(), 5 * 3 * 3);
        assert!(out.iter().all(|&v| v == 200));
    }
}

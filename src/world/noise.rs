//! Deterministic Seed-Based Noise Engine (`Simplex`, `Perlin`, `FBm`, `Ridged`, and Tile Hashing).
//!
//! Guarantees 100% cross-platform, cross-thread determinism: identical [`WorldSeed`] and
//! coordinates always produce identical floating-point and integer values.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Global deterministic seed for procedural world generation.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorldSeed(pub u64);

impl Default for WorldSeed {
    fn default() -> Self {
        Self(0x5049_584C_535F_3236) // "PIXLS_26"
    }
}

impl WorldSeed {
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Derives an independent deterministic sub-seed for a specific noise layer (e.g. elevation, moisture, paths).
    #[must_use]
    pub const fn derive_layer(self, layer_salt: u64) -> u64 {
        splitmix64(self.0 ^ layer_salt.wrapping_mul(0x9E37_79B9_7F4A_7C15))
    }

    /// Deterministic 64-bit hash for a global `(x, y)` tile coordinate.
    #[must_use]
    pub const fn hash_tile(self, x: i32, y: i32, salt: u64) -> u64 {
        let ux = (x as i64 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        let uy = (y as i64 as u64).wrapping_mul(0x94D0_49BB_1331_11EB);
        splitmix64(self.0 ^ salt ^ ux ^ uy.rotate_left(32))
    }

    /// Deterministic uniform float in `[0.0, 1.0)` for a global `(x, y)` tile coordinate.
    #[must_use]
    pub fn hash_unit_f64(self, x: i32, y: i32, salt: u64) -> f64 {
        let h = self.hash_tile(x, y, salt);
        // Use top 53 bits for IEEE-754 double precision mantissa
        ((h >> 11) as f64) * (1.0 / ((1u64 << 53) as f64))
    }
}

/// SplitMix64 bijective 64-bit mixer.
#[must_use]
pub const fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// 12 standard 2D gradient directions on the unit circle / cube edges.
const GRADIENTS_2D: [(f64, f64); 12] = [
    (1.0, 1.0),
    (-1.0, 1.0),
    (1.0, -1.0),
    (-1.0, -1.0),
    (1.0, 0.0),
    (-1.0, 0.0),
    (0.0, 1.0),
    (0.0, -1.0),
    (1.0, 1.0),
    (-1.0, 1.0),
    (0.0, -1.0),
    (-1.0, -1.0),
];

/// Fast, deterministic 2D Noise Generator supporting Simplex, Perlin, FBm, and Ridged Multifractal noise.
#[derive(Debug, Clone)]
pub struct FastNoise2d {
    seed: u64,
    perm: [u8; 512],
    perm_mod12: [u8; 512],
}

impl FastNoise2d {
    /// Initializes a deterministic 2D noise permutation table from `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let mut p = [0u8; 256];
        for (i, slot) in p.iter_mut().enumerate() {
            *slot = i as u8;
        }

        // Fisher-Yates shuffle driven by deterministic SplitMix64 state
        let mut state = splitmix64(seed);
        for i in (1..256).rev() {
            state = splitmix64(state);
            let j = (state as usize) % (i + 1);
            p.swap(i, j);
        }

        let mut perm = [0u8; 512];
        let mut perm_mod12 = [0u8; 512];
        for i in 0..512 {
            let v = p[i & 255];
            perm[i] = v;
            perm_mod12[i] = v % 12;
        }

        Self {
            seed,
            perm,
            perm_mod12,
        }
    }

    /// Returns the seed used to construct this noise generator.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// Evaluates 2D Simplex Noise at `(x, y)`, returning a smooth value in `[-1.0, 1.0]`.
    #[must_use]
    pub fn simplex2d(&self, x: f64, y: f64) -> f64 {
        // Skewing and unskewing factors for 2D triangular simplex grid:
        // F2 = 0.5 * (sqrt(3.0) - 1.0)
        // G2 = (3.0 - sqrt(3.0)) / 6.0
        const F2: f64 = 0.366_025_403_784_438_6;
        const G2: f64 = 0.211_324_865_405_187_13;

        let s = (x + y) * F2;
        let i = (x + s).floor() as i32;
        let j = (y + s).floor() as i32;

        let t = (i + j) as f64 * G2;
        let x0 = x - (i as f64 - t);
        let y0 = y - (j as f64 - t);

        // Determine which simplex triangle we are in
        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };

        let x1 = x0 - i1 as f64 + G2;
        let y1 = y0 - j1 as f64 + G2;
        let x2 = x0 - 1.0 + 2.0 * G2;
        let y2 = y0 - 1.0 + 2.0 * G2;

        let ii = (i & 255) as usize;
        let jj = (j & 255) as usize;

        let gi0 = self.perm_mod12[ii + self.perm[jj] as usize] as usize;
        let gi1 = self.perm_mod12[ii + i1 + self.perm[jj + j1] as usize] as usize;
        let gi2 = self.perm_mod12[ii + 1 + self.perm[jj + 1] as usize] as usize;

        let n0 = Self::simplex_corner_contribution(gi0, x0, y0);
        let n1 = Self::simplex_corner_contribution(gi1, x1, y1);
        let n2 = Self::simplex_corner_contribution(gi2, x2, y2);

        // Scale to [-1.0, 1.0]
        (70.0 * (n0 + n1 + n2)).clamp(-1.0, 1.0)
    }

    #[inline]
    fn simplex_corner_contribution(gi: usize, x: f64, y: f64) -> f64 {
        let t = 0.5 - x * x - y * y;
        if t <= 0.0 {
            0.0
        } else {
            let t2 = t * t;
            let t4 = t2 * t2;
            let (gx, gy) = GRADIENTS_2D[gi];
            t4 * (gx * x + gy * y)
        }
    }

    /// Evaluates classic 2D Perlin Gradient Noise at `(x, y)`, returning a value in `[-1.0, 1.0]`.
    #[must_use]
    pub fn perlin2d(&self, x: f64, y: f64) -> f64 {
        let xi0 = x.floor() as i32;
        let yi0 = y.floor() as i32;
        let xf = x - xi0 as f64;
        let yf = y - yi0 as f64;

        let u = Self::quintic_fade(xf);
        let v = Self::quintic_fade(yf);

        let xi = (xi0 & 255) as usize;
        let yi = (yi0 & 255) as usize;

        let aa = self.perm_mod12[xi + self.perm[yi] as usize] as usize;
        let ab = self.perm_mod12[xi + self.perm[yi + 1] as usize] as usize;
        let ba = self.perm_mod12[xi + 1 + self.perm[yi] as usize] as usize;
        let bb = self.perm_mod12[xi + 1 + self.perm[yi + 1] as usize] as usize;

        let g_aa = Self::grad_dot(aa, xf, yf);
        let g_ba = Self::grad_dot(ba, xf - 1.0, yf);
        let g_ab = Self::grad_dot(ab, xf, yf - 1.0);
        let g_bb = Self::grad_dot(bb, xf - 1.0, yf - 1.0);

        let x1 = Self::lerp(g_aa, g_ba, u);
        let x2 = Self::lerp(g_ab, g_bb, u);

        (Self::lerp(x1, x2, v) * std::f64::consts::FRAC_1_SQRT_2).clamp(-1.0, 1.0)
    }

    /// Evaluates multi-octave Fractal Brownian Motion (FBm) using 2D Simplex Noise, normalized to `[-1.0, 1.0]`.
    #[must_use]
    pub fn fbm_simplex2d(
        &self,
        x: f64,
        y: f64,
        octaves: u32,
        frequency: f64,
        lacunarity: f64,
        gain: f64,
    ) -> f64 {
        let octaves = octaves.max(1);
        let mut total = 0.0;
        let mut amplitude = 1.0;
        let mut max_amplitude = 0.0;
        let mut freq = frequency;

        for oct in 0..octaves {
            // Offset each octave slightly to decorrelate axes
            let offset = (oct as f64) * 31.73;
            total += self.simplex2d(x * freq + offset, y * freq - offset) * amplitude;
            max_amplitude += amplitude;
            amplitude *= gain;
            freq *= lacunarity;
        }

        if max_amplitude > 0.0 {
            (total / max_amplitude).clamp(-1.0, 1.0)
        } else {
            0.0
        }
    }

    /// Evaluates multi-octave Ridged Multifractal Simplex Noise in `[0.0, 1.0]`.
    /// Sharp ridges near `1.0` occur along zero-crossings of the underlying simplex field.
    #[must_use]
    pub fn ridged_simplex2d(
        &self,
        x: f64,
        y: f64,
        octaves: u32,
        frequency: f64,
        lacunarity: f64,
        gain: f64,
    ) -> f64 {
        let octaves = octaves.max(1);
        let mut total = 0.0;
        let mut amplitude = 1.0;
        let mut max_amplitude = 0.0;
        let mut freq = frequency;

        for oct in 0..octaves {
            let offset = (oct as f64) * 19.41;
            let raw = self.simplex2d(x * freq + offset, y * freq + offset);
            let ridge = 1.0 - raw.abs();
            total += (ridge * ridge) * amplitude;
            max_amplitude += amplitude;
            amplitude *= gain;
            freq *= lacunarity;
        }

        if max_amplitude > 0.0 {
            (total / max_amplitude).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    #[inline]
    const fn quintic_fade(t: f64) -> f64 {
        t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
    }

    #[inline]
    const fn lerp(a: f64, b: f64, t: f64) -> f64 {
        a + t * (b - a)
    }

    #[inline]
    fn grad_dot(gi: usize, x: f64, y: f64) -> f64 {
        let (gx, gy) = GRADIENTS_2D[gi];
        gx * x + gy * y
    }
}

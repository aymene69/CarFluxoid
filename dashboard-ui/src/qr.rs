//! Self-contained, zero-dependency ISO/IEC 18004 QR Code encoder (Byte mode, ECC Level L, Versions 1..=6).
//! Renders directly to a crisp high-contrast `slint::Image` with a quiet zone for reliable iPhone scanning.

use slint::{Rgba8Pixel, SharedPixelBuffer};

struct VersionSpec {
    version: usize,
    size: usize,
    data_cw: usize,
    ecc_cw: usize,
    align_pos: &'static [usize],
}

// ISO/IEC 18004 Table 9 — ECC Level L (single block for Versions 1..=5, 2 blocks for Version 6 -> we use 1..=5 up to 106 bytes!)
const VERSIONS: [VersionSpec; 5] = [
    VersionSpec { version: 1, size: 21, data_cw: 19, ecc_cw: 7, align_pos: &[] },
    VersionSpec { version: 2, size: 25, data_cw: 34, ecc_cw: 10, align_pos: &[6, 18] },
    VersionSpec { version: 3, size: 29, data_cw: 55, ecc_cw: 15, align_pos: &[6, 22] },
    VersionSpec { version: 4, size: 33, data_cw: 80, ecc_cw: 20, align_pos: &[6, 26] },
    VersionSpec { version: 5, size: 37, data_cw: 108, ecc_cw: 26, align_pos: &[6, 30] },
];

fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut res = 0u8;
    for _ in 0..8 {
        if (b & 1) != 0 {
            res ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= 0x1D; // primitive poly x^8 + x^4 + x^3 + x^2 + 1
        }
        b >>= 1;
    }
    res
}

fn rs_generator(degree: usize) -> Vec<u8> {
    let mut poly = vec![1u8];
    let mut root = 1u8;
    for _ in 0..degree {
        let mut next = vec![0u8; poly.len() + 1];
        for (i, &c) in poly.iter().enumerate() {
            next[i] ^= c;
            next[i + 1] ^= gf_mul(c, root);
        }
        poly = next;
        root = gf_mul(root, 2);
    }
    poly
}

fn rs_remainder(data: &[u8], ecc_len: usize) -> Vec<u8> {
    let gen_poly = rs_generator(ecc_len);
    let mut rem = vec![0u8; ecc_len];
    for &b in data {
        let factor = b ^ rem[0];
        rem.rotate_left(1);
        rem[ecc_len - 1] = 0;
        for (r, &g) in rem.iter_mut().zip(gen_poly.iter().skip(1)) {
            *r ^= gf_mul(g, factor);
        }
    }
    rem
}

fn push_bits(bits: &mut Vec<bool>, val: u32, count: usize) {
    for i in (0..count).rev() {
        bits.push(((val >> i) & 1) != 0);
    }
}

pub struct QrMatrix {
    pub size: usize,
    pub modules: Vec<bool>, // true = dark
}

impl QrMatrix {
    pub fn encode(text: &str) -> Option<Self> {
        let raw = text.as_bytes();
        // Byte mode header = 4 bits mode (0100) + 8 bits length = 12 bits -> max payload = data_cw - 2 bytes
        let spec = VERSIONS.iter().find(|v| raw.len() + 2 <= v.data_cw)?;
        let mut bits = Vec::with_capacity(spec.data_cw * 8);
        push_bits(&mut bits, 0b0100, 4); // Byte mode
        push_bits(&mut bits, raw.len() as u32, 8);
        for &b in raw {
            push_bits(&mut bits, b as u32, 8);
        }
        let max_bits = spec.data_cw * 8;
        let term = (max_bits - bits.len()).min(4);
        push_bits(&mut bits, 0, term);
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        let mut codewords = Vec::with_capacity(spec.data_cw + spec.ecc_cw);
        for chunk in bits.chunks_exact(8) {
            let mut byte = 0u8;
            for &bit in chunk {
                byte = (byte << 1) | u8::from(bit);
            }
            codewords.push(byte);
        }
        let mut pad_idx = 0usize;
        while codewords.len() < spec.data_cw {
            codewords.push(if pad_idx % 2 == 0 { 0xEC } else { 0x11 });
            pad_idx += 1;
        }
        let ecc = rs_remainder(&codewords, spec.ecc_cw);
        codewords.extend_from_slice(&ecc);

        let n = spec.size;
        let mut mods = vec![false; n * n];
        let mut func = vec![false; n * n];

        let set_fn = |mods: &mut [bool], func: &mut [bool], x: isize, y: isize, dark: bool| {
            if x >= 0 && y >= 0 && (x as usize) < n && (y as usize) < n {
                let idx = (y as usize) * n + (x as usize);
                mods[idx] = dark;
                func[idx] = true;
            }
        };

        // 3 Finder patterns + separators
        for &(fx, fy) in &[(0isize, 0isize), (n as isize - 7, 0), (0, n as isize - 7)] {
            for dy in -1..=7 {
                for dx in -1..=7 {
                    let in_box = (0..=6).contains(&dx) && (0..=6).contains(&dy);
                    let dark = in_box
                        && (dx == 0
                            || dx == 6
                            || dy == 0
                            || dy == 6
                            || ((2..=4).contains(&dx) && (2..=4).contains(&dy)));
                    set_fn(&mut mods, &mut func, fx + dx, fy + dy, dark);
                }
            }
        }

        // Alignment patterns (Versions 2..=5)
        for &ay in spec.align_pos {
            for &ax in spec.align_pos {
                // Skip corners overlapping finders
                if (ax < 9 && ay < 9) || (ax < 9 && ay + 9 > n) || (ax + 9 > n && ay < 9) {
                    continue;
                }
                for dy in -2..=2isize {
                    for dx in -2..=2isize {
                        let dark = dx.abs().max(dy.abs()) != 1;
                        set_fn(&mut mods, &mut func, ax as isize + dx, ay as isize + dy, dark);
                    }
                }
            }
        }

        // Timing patterns
        for i in 8..(n - 8) {
            let dark = i % 2 == 0;
            set_fn(&mut mods, &mut func, i as isize, 6, dark);
            set_fn(&mut mods, &mut func, 6, i as isize, dark);
        }

        // Dark module & reserve format info areas
        set_fn(&mut mods, &mut func, 8, (n - 8) as isize, true);
        for i in 0..9 {
            if !func[8 * n + i] { func[8 * n + i] = true; }
            if !func[i * n + 8] { func[i * n + 8] = true; }
        }
        for i in 0..8 {
            func[8 * n + (n - 1 - i)] = true;
            func[(n - 1 - i) * n + 8] = true;
        }

        // Place data bits in upward/downward 2-column zig-zag
        let mut bit_idx = 0usize;
        let total_bits = codewords.len() * 8;
        let mut right = (n as isize) - 1;
        let mut upward = true;
        while right >= 1 {
            if right == 6 {
                right -= 1;
            }
            for step in 0..n {
                let y = if upward { n - 1 - step } else { step };
                for dx in 0..2 {
                    let x = (right - dx) as usize;
                    let idx = y * n + x;
                    if !func[idx] {
                        let bit = if bit_idx < total_bits {
                            let cw = codewords[bit_idx / 8];
                            ((cw >> (7 - (bit_idx % 8))) & 1) != 0
                        } else {
                            false
                        };
                        mods[idx] = bit;
                        bit_idx += 1;
                    }
                }
            }
            right -= 2;
            upward = !upward;
        }

        // Evaluate masks 0..8 and pick lowest penalty score (standard ISO 18004)
        let mut best_mods = mods.clone();
        let mut best_score = u32::MAX;
        for mask in 0..8u8 {
            let mut candidate = mods.clone();
            for y in 0..n {
                for x in 0..n {
                    let idx = y * n + x;
                    if !func[idx] && mask_bit(mask, x, y) {
                        candidate[idx] = !candidate[idx];
                    }
                }
            }
            write_format_bits(&mut candidate, n, mask);
            let score = penalty_score(&candidate, n);
            if score < best_score {
                best_score = score;
                best_mods = candidate;
            }
        }

        let _ = spec.version;
        Some(Self {
            size: n,
            modules: best_mods,
        })
    }

    pub fn to_slint_image(&self, scale: usize) -> slint::Image {
        let quiet = 3usize;
        let total_mods = self.size + quiet * 2;
        let px = total_mods * scale;
        let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(px as u32, px as u32);
        let slice = buf.make_mut_slice();
        let white = Rgba8Pixel { r: 255, g: 255, b: 255, a: 255 };
        let black = Rgba8Pixel { r: 10, g: 14, b: 22, a: 255 };
        slice.fill(white);

        for my in 0..self.size {
            for mx in 0..self.size {
                if self.modules[my * self.size + mx] {
                    let ox = (mx + quiet) * scale;
                    let oy = (my + quiet) * scale;
                    for dy in 0..scale {
                        let row = (oy + dy) * px;
                        for dx in 0..scale {
                            slice[row + ox + dx] = black;
                        }
                    }
                }
            }
        }
        slint::Image::from_rgba8(buf)
    }
}

fn mask_bit(mask: u8, x: usize, y: usize) -> bool {
    match mask {
        0 => (x + y) % 2 == 0,
        1 => y % 2 == 0,
        2 => x % 3 == 0,
        3 => (x + y) % 3 == 0,
        4 => ((y / 2) + (x / 3)) % 2 == 0,
        5 => ((x * y) % 2) + ((x * y) % 3) == 0,
        6 => (((x * y) % 2) + ((x * y) % 3)) % 2 == 0,
        _ => (((x + y) % 2) + ((x * y) % 3)) % 2 == 0,
    }
}

fn write_format_bits(mods: &mut [bool], n: usize, mask: u8) {
    // ECC Level L = 01 (2 bits) + mask (3 bits) -> BCH(15, 5) + XOR 0x5412
    let data = (0b01u32 << 3) | u32::from(mask & 7);
    let mut rem = data << 10;
    for i in (0..5).rev() {
        if (rem & (1 << (i + 10))) != 0 {
            rem ^= 0x537 << i;
        }
    }
    let bits = ((data << 10) | rem) ^ 0x5412;

    for i in 0..=5 {
        let dark = ((bits >> i) & 1) != 0;
        mods[8 * n + (n - 1 - i)] = dark;
        mods[i * n + 8] = dark;
    }
    mods[8 * n + (n - 7)] = ((bits >> 6) & 1) != 0;
    mods[7 * n + 8] = ((bits >> 6) & 1) != 0;
    mods[8 * n + (n - 8)] = ((bits >> 7) & 1) != 0;
    mods[8 * n + 8] = ((bits >> 7) & 1) != 0;
    mods[8 * n + 7] = ((bits >> 8) & 1) != 0;
    mods[(n - 7) * n + 8] = ((bits >> 8) & 1) != 0;
    for i in 9..15 {
        let dark = ((bits >> i) & 1) != 0;
        mods[8 * n + (14 - i)] = dark;
        mods[(n - 15 + i) * n + 8] = dark;
    }
}

fn penalty_score(mods: &[bool], n: usize) -> u32 {
    let mut score = 0u32;
    // Run-length horizontal & vertical
    for y in 0..n {
        let mut run = 1u32;
        for x in 1..n {
            if mods[y * n + x] == mods[y * n + x - 1] {
                run += 1;
                if run == 5 { score += 3; } else if run > 5 { score += 1; }
            } else {
                run = 1;
            }
        }
    }
    for x in 0..n {
        let mut run = 1u32;
        for y in 1..n {
            if mods[y * n + x] == mods[(y - 1) * n + x] {
                run += 1;
                if run == 5 { score += 3; } else if run > 5 { score += 1; }
            } else {
                run = 1;
            }
        }
    }
    // 2x2 blocks
    for y in 0..(n - 1) {
        for x in 0..(n - 1) {
            let c = mods[y * n + x];
            if c == mods[y * n + x + 1] && c == mods[(y + 1) * n + x] && c == mods[(y + 1) * n + x + 1] {
                score += 3;
            }
        }
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_wifi_and_url_qr() {
        let q1 = QrMatrix::encode("http://192.168.50.2").expect("url qr");
        assert_eq!(q1.size, 25);
        assert!(q1.modules[0]); // top-left finder corner
        let q2 = QrMatrix::encode("WIFI:T:WPA;S:CarFluxoid-Pi_AP;P:carfluxoid123;;").expect("wifi qr");
        assert!(q2.size >= 29);
    }
}

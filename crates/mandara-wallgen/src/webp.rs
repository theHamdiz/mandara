//! A lossless WebP (VP8L) encoder tuned for generated wallpapers.
//!
//! `image`'s encoder writes a fixed top predictor and plain Huffman codes,
//! with no LZ77 and no color cache, which is fine for photos but wastes
//! most of the size of a smooth, dithered gradient. This one follows the
//! VP8L format (RFC 9649) with the parts that matter for this content:
//!
//! * subtract-green, then the predictor transform with the best of the 14
//!   modes chosen per 16 × 16 block;
//! * LZ77 back references searched at the distances where generated
//!   images repeat: the previous pixel and row, and multiples of the
//!   dither tile's period, so the anti-banding pattern costs almost
//!   nothing;
//! * a color cache, and canonical Huffman codes limited to 15 bits.
//!
//! Output decodes with any WebP decoder (`image` round-trips it exactly in
//! the tests). Deterministic: no threads, no hashing of addresses.

/// Predictor blocks are `1 << PREDICTOR_BITS` pixels square.
const PREDICTOR_BITS: u32 = 4;
/// Color cache size: `1 << CACHE_BITS` entries.
const CACHE_BITS: u32 = 10;
/// Shortest back reference worth coding (pixels).
const MIN_MATCH: usize = 3;
/// Longest back reference VP8L allows.
const MAX_MATCH: usize = 4096;
/// Longest code length VP8L allows (and the code-length code's).
const MAX_CODE_LENGTH: u8 = 15;
const MAX_CL_LENGTH: u8 = 7;
/// Symbols in the green / length / cache alphabet before the cache.
const NUM_LITERALS: usize = 256;
const NUM_LENGTH_CODES: usize = 24;
const NUM_DISTANCE_CODES: usize = 40;
/// Distance codes 1 and 2 are the "plane codes" for straight up and
/// straight left; larger distances are coded as `distance + 120`.
const PLANE_CODES: usize = 120;
/// Order in which code-length-code lengths are written.
const CODE_LENGTH_ORDER: [usize; 19] = [17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
/// Multiplier of the color-cache hash (from the format).
const CACHE_HASH: u32 = 0x1E35_A7BD;
/// Horizontal and vertical repeat distances searched besides 1 and 2:
/// the dither matrix period and its multiples.
const PERIODS: [usize; 3] = [crate::dither::TILE / 2, crate::dither::TILE, crate::dither::TILE * 2];
/// Hash-chain matcher: table size, candidates tried per position, and the
/// farthest distance VP8L can code (2^20 − 120).
const HASH_BITS: u32 = 18;
const CHAIN_DEPTH: usize = 24;
const MAX_DISTANCE: usize = (1 << 20) - PLANE_CODES;
/// Lazy matching defers a match when the next one is longer by more than this.
const LAZY_MARGIN: usize = 1;

struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter { out: Vec::new(), acc: 0, n: 0 }
    }

    #[inline]
    fn bits(&mut self, value: u32, count: u32) {
        if count == 0 {
            return;
        }
        self.acc |= (value as u64 & ((1u64 << count) - 1)) << self.n;
        self.n += count;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

/// A prefix code: lengths and bit-reversed canonical codes per symbol.
struct Code {
    lengths: Vec<u8>,
    codes: Vec<u16>,
}

impl Code {
    #[inline]
    fn put(&self, w: &mut BitWriter, symbol: usize) {
        w.bits(self.codes[symbol] as u32, self.lengths[symbol] as u32);
    }
}

/// Code lengths (≤ `limit`) for `freqs`: Huffman, with frequencies
/// flattened until the deepest code fits. Deterministic tie-breaking.
fn code_lengths(freqs: &[u32], limit: u8) -> Vec<u8> {
    let mut f: Vec<u64> = freqs.iter().map(|&x| x as u64).collect();
    loop {
        let lengths = huffman(&f);
        if lengths.iter().all(|&l| l <= limit) {
            return lengths;
        }
        for v in f.iter_mut().filter(|v| **v > 0) {
            *v = (*v >> 1) | 1;
        }
    }
}

fn huffman(freqs: &[u64]) -> Vec<u8> {
    let mut lengths = vec![0u8; freqs.len()];
    let used: Vec<usize> = (0..freqs.len()).filter(|&i| freqs[i] > 0).collect();
    if used.len() <= 1 {
        for &i in &used {
            lengths[i] = 1;
        }
        return lengths;
    }
    // Nodes: leaves then internal; a heap of (freq, id) picks the two
    // lightest, ties by id (deterministic).
    let mut parent = vec![usize::MAX; used.len() * 2];
    let mut heap = std::collections::BinaryHeap::new();
    for (k, &i) in used.iter().enumerate() {
        heap.push(std::cmp::Reverse((freqs[i], k)));
    }
    let mut next = used.len();
    while heap.len() > 1 {
        let std::cmp::Reverse((fa, a)) = heap.pop().unwrap_or(std::cmp::Reverse((0, 0)));
        let std::cmp::Reverse((fb, b)) = heap.pop().unwrap_or(std::cmp::Reverse((0, 0)));
        parent[a] = next;
        parent[b] = next;
        heap.push(std::cmp::Reverse((fa + fb, next)));
        next += 1;
    }
    for (k, &i) in used.iter().enumerate() {
        let (mut d, mut n) = (0u8, k);
        while parent[n] != usize::MAX {
            n = parent[n];
            d += 1;
        }
        lengths[i] = d;
    }
    lengths
}

/// Canonical codes for `lengths`, bit-reversed for LSB-first writing.
fn canonical(lengths: Vec<u8>) -> Code {
    let max = *lengths.iter().max().unwrap_or(&0) as usize;
    let mut count = vec![0u16; max + 1];
    for &l in &lengths {
        if l > 0 {
            count[l as usize] += 1;
        }
    }
    let mut next = vec![0u16; max + 2];
    let mut code = 0u16;
    for bits in 1..=max {
        code = (code + count[bits - 1]) << 1;
        next[bits] = code;
    }
    let codes = lengths
        .iter()
        .map(|&l| {
            if l == 0 {
                return 0;
            }
            let c = next[l as usize];
            next[l as usize] += 1;
            c.reverse_bits() >> (16 - l as u32)
        })
        .collect();
    Code { lengths, codes }
}

/// Writes a prefix code for `freqs` and returns it.
fn write_code(w: &mut BitWriter, freqs: &[u32]) -> Code {
    let used: Vec<usize> = (0..freqs.len()).filter(|&i| freqs[i] > 0).collect();
    // Simple codes: one or two symbols, all below 256.
    if used.len() <= 2 && used.iter().all(|&s| s < 256) {
        let syms: Vec<usize> = if used.is_empty() { vec![0] } else { used.clone() };
        w.bits(1, 1);
        w.bits(syms.len() as u32 - 1, 1);
        if syms[0] < 2 {
            w.bits(0, 1);
            w.bits(syms[0] as u32, 1);
        } else {
            w.bits(1, 1);
            w.bits(syms[0] as u32, 8);
        }
        if syms.len() == 2 {
            w.bits(syms[1] as u32, 8);
        }
        let mut lengths = vec![0u8; freqs.len()];
        let mut codes = vec![0u16; freqs.len()];
        if syms.len() == 2 {
            lengths[syms[0]] = 1;
            lengths[syms[1]] = 1;
            codes[syms[1]] = 1;
        }
        return Code { lengths, codes };
    }
    let mut f = freqs.to_vec();
    if used.len() == 1 {
        // A normal code needs two symbols; add a never-used partner.
        f[if used[0] == 0 { 1 } else { 0 }] = 1;
    }
    let lengths = code_lengths(&f, MAX_CODE_LENGTH);
    w.bits(0, 1);
    // Run-length tokens of the lengths: 0–15 literal, 16 repeat the
    // previous length 3–6 times, 17 / 18 runs of zeros (3–10 / 11–138).
    let mut tokens: Vec<(u8, u8)> = Vec::new();
    let mut i = 0;
    let mut prev: Option<u8> = None;
    while i < lengths.len() {
        let l = lengths[i];
        let run = lengths[i..].iter().take_while(|&&x| x == l).count();
        if l == 0 && run >= 3 {
            let r = run.min(138);
            if r >= 11 { tokens.push((18, (r - 11) as u8)) } else { tokens.push((17, (r - 3) as u8)) }
            i += r;
            continue;
        }
        if prev == Some(l) && l != 0 && run >= 3 {
            let r = run.min(6);
            tokens.push((16, (r - 3) as u8));
            i += r;
            continue;
        }
        tokens.push((l, 0));
        prev = Some(l);
        i += 1;
    }
    let mut cl_freq = [0u32; 19];
    for &(s, _) in &tokens {
        cl_freq[s as usize] += 1;
    }
    if cl_freq.iter().filter(|&&c| c > 0).count() < 2 {
        let only = cl_freq.iter().position(|&c| c > 0).unwrap_or(0);
        cl_freq[if only == 0 { 1 } else { 0 }] = 1;
    }
    let cl = canonical(code_lengths(&cl_freq, MAX_CL_LENGTH));
    let count = (4..=19).rev().find(|&n| cl.lengths[CODE_LENGTH_ORDER[n - 1]] != 0).unwrap_or(4).max(4);
    w.bits(count as u32 - 4, 4);
    for &sym in &CODE_LENGTH_ORDER[..count] {
        w.bits(cl.lengths[sym] as u32, 3);
    }
    // All symbols are coded (no max_symbol).
    w.bits(0, 1);
    for (s, extra) in tokens {
        cl.put(w, s as usize);
        match s {
            16 => w.bits(extra as u32, 2),
            17 => w.bits(extra as u32, 3),
            18 => w.bits(extra as u32, 7),
            _ => {}
        }
    }
    canonical(lengths)
}

/// VP8L prefix coding of a length or distance (≥ 1): (prefix, extra
/// bit count, extra bits).
#[inline]
fn prefix(value: usize) -> (usize, u32, u32) {
    let d = value - 1;
    if d < 4 {
        return (d, 0, 0);
    }
    let high = usize::BITS - 1 - d.leading_zeros();
    let second = (d >> (high - 1)) & 1;
    let extra = high - 1;
    ((2 * high as usize) + second, extra, (d & ((1 << extra) - 1)) as u32)
}

#[derive(Clone, Copy)]
enum Token {
    Literal(u32),
    Cache(u32),
    Copy { len: u32, dist_code: u32 },
}

/// Finds back references: the fixed repeat distances first (previous
/// pixel and row, dither-tile periods), then a hash chain of pixel pairs.
struct Matcher<'a> {
    argb: &'a [u32],
    dists: Vec<usize>,
    head: Vec<usize>,
    chain: Vec<usize>,
    inserted: usize,
}

impl<'a> Matcher<'a> {
    fn new(argb: &'a [u32], width: usize) -> Self {
        let mut dists: Vec<usize> = vec![1, 2, width, width * 2];
        for p in PERIODS {
            dists.extend([p, p * width, p * width + p]);
        }
        dists.retain(|&d| d > 0);
        dists.sort_unstable();
        dists.dedup();
        Matcher { argb, dists, head: vec![usize::MAX; 1 << HASH_BITS], chain: vec![usize::MAX; argb.len()], inserted: 0 }
    }

    #[inline]
    fn hash(&self, i: usize) -> usize {
        let a = self.argb;
        let next = a.get(i + 1).copied().unwrap_or(0);
        ((a[i].wrapping_mul(0x9E37_79B1) ^ next.wrapping_mul(0x85EB_CA77)) >> (32 - HASH_BITS)) as usize
    }

    /// Indexes every position before `pos`.
    fn catch_up(&mut self, pos: usize) {
        while self.inserted < pos {
            let k = self.hash(self.inserted);
            self.chain[self.inserted] = self.head[k];
            self.head[k] = self.inserted;
            self.inserted += 1;
        }
    }

    #[inline]
    fn extend(&self, pos: usize, d: usize, limit: usize) -> usize {
        let a = self.argb;
        let mut l = 0;
        while l < limit && a[pos + l] == a[pos + l - d] {
            l += 1;
        }
        l
    }

    /// Longest match at `pos`: (length, distance).
    fn best(&mut self, pos: usize) -> (usize, usize) {
        self.catch_up(pos);
        let limit = (self.argb.len() - pos).min(MAX_MATCH);
        let (mut len, mut dist) = (0, 0);
        let consider = |d: usize, this: &Self, len: &mut usize, dist: &mut usize| {
            if d > pos || d > MAX_DISTANCE || *len >= limit {
                return;
            }
            // Quick reject: a longer match must also match at the current end.
            if *len > 0 && this.argb[pos + *len] != this.argb[pos + *len - d] {
                return;
            }
            let l = this.extend(pos, d, limit);
            if l > *len {
                *len = l;
                *dist = d;
            }
        };
        for i in 0..self.dists.len() {
            consider(self.dists[i], self, &mut len, &mut dist);
        }
        let mut cand = self.head[self.hash(pos)];
        let mut tries = 0;
        while cand != usize::MAX && tries < CHAIN_DEPTH && len < limit {
            consider(pos - cand, self, &mut len, &mut dist);
            cand = self.chain[cand];
            tries += 1;
        }
        (len, dist)
    }
}

/// LZ77 + color cache parse of `argb` (`width` wide), with one step of
/// lazy matching: a match is deferred by a literal when the next position
/// has a clearly longer one.
fn tokenize(argb: &[u32], width: usize, cache_bits: u32) -> Vec<Token> {
    let n = argb.len();
    let mut cache = vec![0u32; if cache_bits > 0 { 1 << cache_bits } else { 0 }];
    let hash = |c: u32| (c.wrapping_mul(CACHE_HASH) >> (32 - cache_bits)) as usize;
    let mut m = Matcher::new(argb, width);
    let mut tokens = Vec::with_capacity(n / 4);
    let mut pos = 0;
    let mut pending: Option<(usize, usize)> = None;
    while pos < n {
        let (len, dist) = pending.take().unwrap_or_else(|| m.best(pos));
        if len >= MIN_MATCH && len < MAX_MATCH && pos + 1 < n {
            let next = m.best(pos + 1);
            if next.0 > len + LAZY_MARGIN {
                pending = Some(next);
                // Emit this pixel as a literal, then take the longer match.
                push_literal(&mut tokens, &mut cache, cache_bits, &hash, argb[pos]);
                pos += 1;
                continue;
            }
        }
        if len >= MIN_MATCH {
            let dist_code = if dist == width { 1 } else if dist == 1 { 2 } else { dist + PLANE_CODES };
            tokens.push(Token::Copy { len: len as u32, dist_code: dist_code as u32 });
            if cache_bits > 0 {
                for &c in &argb[pos..pos + len] {
                    cache[hash(c)] = c;
                }
            }
            pos += len;
        } else {
            push_literal(&mut tokens, &mut cache, cache_bits, &hash, argb[pos]);
            pos += 1;
        }
    }
    tokens
}

#[inline]
fn push_literal(tokens: &mut Vec<Token>, cache: &mut [u32], cache_bits: u32, hash: &impl Fn(u32) -> usize, c: u32) {
    if cache_bits > 0 {
        let h = hash(c);
        if cache[h] == c {
            tokens.push(Token::Cache(h as u32));
            return;
        }
        cache[h] = c;
    }
    tokens.push(Token::Literal(c));
}

/// Writes an entropy-coded image (one Huffman group).
fn write_image(w: &mut BitWriter, argb: &[u32], width: usize, cache_bits: u32, is_main: bool) {
    let tokens = tokenize(argb, width, cache_bits);
    if cache_bits > 0 {
        w.bits(1, 1);
        w.bits(cache_bits, 4);
    } else {
        w.bits(0, 1);
    }
    if is_main {
        // No meta prefix codes: one group for the whole image.
        w.bits(0, 1);
    }
    let cache_size = if cache_bits > 0 { 1usize << cache_bits } else { 0 };
    let mut green = vec![0u32; NUM_LITERALS + NUM_LENGTH_CODES + cache_size];
    let (mut red, mut blue, mut alpha, mut dist) = (vec![0u32; 256], vec![0u32; 256], vec![0u32; 256], vec![0u32; NUM_DISTANCE_CODES]);
    for t in &tokens {
        match *t {
            Token::Literal(c) => {
                green[((c >> 8) & 0xFF) as usize] += 1;
                red[((c >> 16) & 0xFF) as usize] += 1;
                blue[(c & 0xFF) as usize] += 1;
                alpha[(c >> 24) as usize] += 1;
            }
            Token::Cache(i) => green[NUM_LITERALS + NUM_LENGTH_CODES + i as usize] += 1,
            Token::Copy { len, dist_code } => {
                green[NUM_LITERALS + prefix(len as usize).0] += 1;
                dist[prefix(dist_code as usize).0] += 1;
            }
        }
    }
    let g = write_code(w, &green);
    let r = write_code(w, &red);
    let b = write_code(w, &blue);
    let a = write_code(w, &alpha);
    let d = write_code(w, &dist);
    for t in tokens {
        match t {
            Token::Literal(c) => {
                g.put(w, ((c >> 8) & 0xFF) as usize);
                r.put(w, ((c >> 16) & 0xFF) as usize);
                b.put(w, (c & 0xFF) as usize);
                a.put(w, (c >> 24) as usize);
            }
            Token::Cache(i) => g.put(w, NUM_LITERALS + NUM_LENGTH_CODES + i as usize),
            Token::Copy { len, dist_code } => {
                let (p, nb, extra) = prefix(len as usize);
                g.put(w, NUM_LITERALS + p);
                w.bits(extra, nb);
                let (p, nb, extra) = prefix(dist_code as usize);
                d.put(w, p);
                w.bits(extra, nb);
            }
        }
    }
}

#[inline]
fn avg2(a: u32, b: u32) -> u32 {
    (((a ^ b) & 0xFEFE_FEFE) >> 1) + (a & b)
}

#[inline]
fn channels(c: u32) -> [i32; 4] {
    [(c >> 24) as i32, ((c >> 16) & 0xFF) as i32, ((c >> 8) & 0xFF) as i32, (c & 0xFF) as i32]
}

#[inline]
fn pack(ch: [i32; 4]) -> u32 {
    ((ch[0].clamp(0, 255) as u32) << 24) | ((ch[1].clamp(0, 255) as u32) << 16) | ((ch[2].clamp(0, 255) as u32) << 8) | ch[3].clamp(0, 255) as u32
}

/// VP8L predictor `mode` from left, top, top-left and top-right.
#[inline]
fn predict(mode: u32, l: u32, t: u32, tl: u32, tr: u32) -> u32 {
    match mode {
        0 => 0xFF00_0000,
        1 => l,
        2 => t,
        3 => tr,
        4 => tl,
        5 => avg2(avg2(l, tr), t),
        6 => avg2(l, tl),
        7 => avg2(l, t),
        8 => avg2(tl, t),
        9 => avg2(t, tr),
        10 => avg2(avg2(l, tl), avg2(t, tr)),
        11 => {
            let (cl, ct, ctl) = (channels(l), channels(t), channels(tl));
            let (mut pl, mut pt) = (0, 0);
            for i in 0..4 {
                let p = cl[i] + ct[i] - ctl[i];
                pl += (p - cl[i]).abs();
                pt += (p - ct[i]).abs();
            }
            if pl < pt { l } else { t }
        }
        12 => {
            let (cl, ct, ctl) = (channels(l), channels(t), channels(tl));
            pack([0, 1, 2, 3].map(|i| cl[i] + ct[i] - ctl[i]))
        }
        _ => {
            let (a, ctl) = (channels(avg2(l, t)), channels(tl));
            pack([0, 1, 2, 3].map(|i| a[i] + (a[i] - ctl[i]) / 2))
        }
    }
}

#[inline]
fn sub(a: u32, b: u32) -> u32 {
    let (a, b) = (a.to_be_bytes(), b.to_be_bytes());
    u32::from_be_bytes([a[0].wrapping_sub(b[0]), a[1].wrapping_sub(b[1]), a[2].wrapping_sub(b[2]), a[3].wrapping_sub(b[3])])
}

/// Signed size of a residual, per channel, summed (cheap entropy proxy).
#[inline]
fn residual_cost(r: u32) -> u32 {
    r.to_be_bytes().iter().map(|&b| (b as i8).unsigned_abs() as u32).sum()
}

/// Neighbours of pixel `i` for prediction, with the format's borders.
#[inline]
fn neighbours(px: &[u32], w: usize, i: usize) -> (u32, u32, u32, u32) {
    let x = i % w;
    let l = px[i - 1];
    let t = px[i - w];
    let tl = if x > 0 { px[i - w - 1] } else { t };
    // The top-right of the last column is the first pixel of this row.
    let tr = px[i - w + 1];
    (l, t, tl, tr)
}

/// Residuals of the predictor transform and the per-block mode image.
fn predictor_transform(px: &[u32], w: usize, h: usize) -> (Vec<u32>, Vec<u32>, usize) {
    let bs = 1usize << PREDICTOR_BITS;
    let (bw, bh) = (w.div_ceil(bs), h.div_ceil(bs));
    let mut modes = vec![0u32; bw * bh];
    for by in 0..bh {
        for bx in 0..bw {
            let mut best = (u64::MAX, 0u32);
            for mode in 0..14u32 {
                let mut cost = 0u64;
                for y in by * bs..((by + 1) * bs).min(h) {
                    if y == 0 {
                        continue;
                    }
                    for x in bx * bs..((bx + 1) * bs).min(w) {
                        if x == 0 {
                            continue;
                        }
                        let i = y * w + x;
                        let (l, t, tl, tr) = neighbours(px, w, i);
                        cost += residual_cost(sub(px[i], predict(mode, l, t, tl, tr))) as u64;
                    }
                }
                if cost < best.0 {
                    best = (cost, mode);
                }
            }
            modes[by * bw + bx] = best.1;
        }
    }
    let mut res = vec![0u32; px.len()];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let pred = if y == 0 {
                if x == 0 { 0xFF00_0000 } else { px[i - 1] }
            } else if x == 0 {
                px[i - w]
            } else {
                let (l, t, tl, tr) = neighbours(px, w, i);
                predict(modes[(y / bs) * bw + x / bs], l, t, tl, tr)
            };
            res[i] = sub(px[i], pred);
        }
    }
    // The mode image stores each mode in the green channel.
    let mode_img = modes.iter().map(|&m| 0xFF00_0000 | (m << 8)).collect();
    (res, mode_img, bw)
}

/// Encodes packed RGB as a lossless WebP file.
pub fn encode_rgb(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || w > 16384 || h > 16384 || rgb.len() != w * h * 3 {
        return Err(mandara_i18n::t("WebP images must be 1 to 16384 pixels on each side").into());
    }
    // ARGB with the green channel subtracted from red and blue.
    let px: Vec<u32> = rgb
        .chunks_exact(3)
        .map(|p| {
            let (r, g, b) = (p[0].wrapping_sub(p[1]), p[1], p[2].wrapping_sub(p[1]));
            0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
        })
        .collect();
    let (res, modes, mode_w) = predictor_transform(&px, w, h);
    drop(px);
    let mut bw = BitWriter::new();
    bw.bits(0x2F, 8);
    bw.bits(width - 1, 14);
    bw.bits(height - 1, 14);
    // No alpha; version 0.
    bw.bits(0, 1);
    bw.bits(0, 3);
    // Transforms in decoding-reverse order: subtract green, then predictor.
    bw.bits(1, 1);
    bw.bits(2, 2);
    bw.bits(1, 1);
    bw.bits(0, 2);
    bw.bits(PREDICTOR_BITS - 2, 3);
    write_image(&mut bw, &modes, mode_w, 0, false);
    bw.bits(0, 1);
    write_image(&mut bw, &res, w, CACHE_BITS, true);
    let body = bw.finish();
    let pad = body.len() & 1;
    let mut out = Vec::with_capacity(body.len() + 21);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((4 + 8 + body.len() + pad) as u32).to_le_bytes());
    out.extend_from_slice(b"WEBPVP8L");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    if pad == 1 {
        out.push(0);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(w: u32, h: u32, mut f: impl FnMut(u32, u32) -> [u8; 3]) {
        let mut rgb = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgb.extend_from_slice(&f(x, y));
            }
        }
        let bytes = encode_rgb(&rgb, w, h).unwrap();
        let back = image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP).unwrap().to_rgb8();
        assert_eq!(back.dimensions(), (w, h));
        assert!(back.as_raw() == &rgb, "{w}×{h} differs");
    }

    #[test]
    fn round_trips_exactly() {
        round_trip(1, 1, |_, _| [7, 8, 9]);
        round_trip(3, 2, |x, y| [x as u8 * 80, y as u8 * 90, 200]);
        round_trip(37, 23, |x, y| [(x * 7) as u8, (y * 11) as u8, ((x * y) % 256) as u8]);
        round_trip(64, 40, |_, _| [10, 20, 30]);
        let mut seed = 1u32;
        round_trip(50, 31, |_, _| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let b = seed.to_le_bytes();
            [b[1], b[2], b[3]]
        });
        // Dithered gradient, the content this encoder is built for.
        round_trip(300, 70, |x, y| {
            let v = 40.0 + x as f32 * 0.07 + y as f32 * 0.2 + crate::dither::threshold(x, y);
            [v as u8, (v * 1.3) as u8, (v * 0.6) as u8]
        });
    }

    #[test]
    fn prefix_codes_match_the_format() {
        assert_eq!(prefix(1), (0, 0, 0));
        assert_eq!(prefix(4), (3, 0, 0));
        assert_eq!(prefix(5), (4, 1, 0));
        assert_eq!(prefix(6), (4, 1, 1));
        assert_eq!(prefix(7), (5, 1, 0));
        assert_eq!(prefix(4096).0, 23);
        // Decode side: offset = (2 + (p & 1)) << extra, value = offset + bits + 1.
        for v in 1..5000usize {
            let (p, nb, e) = prefix(v);
            let back = if p < 4 { p + 1 } else { ((2 + (p & 1)) << nb) + e as usize + 1 };
            assert_eq!(back, v);
        }
    }

    #[test]
    fn lengths_are_limited() {
        let freqs: Vec<u32> = (0..40).map(|i| 1u32 << (i % 30)).collect();
        assert!(code_lengths(&freqs, 15).iter().all(|&l| l <= 15 && l > 0));
    }
}

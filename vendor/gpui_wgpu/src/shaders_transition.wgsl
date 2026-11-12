// Mandara patch 16: the transition primitive (`gpui::TransitionSprite`).
//
// One full-surface quad samples the outgoing (`t_from`) and incoming (`t_to`)
// images, plus an optional helper image (`t_aux`: subject mask in red,
// distance from the subject in green, distance from the nearest line in
// blue), and evaluates the transition per pixel. Nothing is tiled, so no
// seam can open between pieces. Every kind leaves exactly the incoming image
// at progress 1 and exactly the outgoing one at progress 0.
//
// A self-contained module: its own bind group (uniforms, three textures, a
// sampler, and a small tiling noise texture with its own sampler), built only
// when a frame first draws a transition.

struct TransitionUniforms {
    // Frame viewport (device px), unused, unused.
    viewport: vec4<f32>,
    // Primitive bounds: origin, size (device px).
    bounds: vec4<f32>,
    // Content mask rectangle: origin, size (device px).
    clip: vec4<f32>,
    // Atlas tiles: origin, size (texels).
    from_tile: vec4<f32>,
    to_tile: vec4<f32>,
    aux_tile: vec4<f32>,
    // 1 / texture size of `t_from` (xy) and `t_to` (zw), and of `t_aux` (xy).
    inv_from_to: vec4<f32>,
    inv_aux: vec4<f32>,
    // kind, flags, fill style, drawing order.
    mode: vec4<u32>,
    // seed, shard count, grid columns, grid rows.
    counts: vec4<u32>,
    // progress, seconds since the start, intensity, device px per logical px.
    timing: vec4<f32>,
    // origin (fraction of bounds), focus (fraction of bounds).
    points: vec4<f32>,
    // line width (device px), drawing share, unused, unused.
    sketch: vec4<f32>,
    line: vec4<f32>,
    glow: vec4<f32>,
    wash: vec4<f32>,
    // Shatter: per shard `a` = inverse 2x2 (row major), `b` = rest centre
    // (q space) and current centre, `c` = alpha, depth, spin, how far it
    // has gone (0..1), `d` = when it breaks loose (progress).
    cells: array<ShardCell, 128>,
    // Shatter: for each of 16 × 9 screen bins, a 128-bit set of the shards
    // that may cover it.
    bins: array<vec4<u32>, 144>,
}

struct ShardCell {
    a: vec4<f32>,
    b: vec4<f32>,
    c: vec4<f32>,
    d: vec4<f32>,
}

@group(0) @binding(0) var<uniform> u: TransitionUniforms;
@group(0) @binding(1) var t_from: texture_2d<f32>;
@group(0) @binding(2) var t_to: texture_2d<f32>;
@group(0) @binding(3) var t_aux: texture_2d<f32>;
@group(0) @binding(4) var s_linear: sampler;
@group(0) @binding(5) var t_noise: texture_2d<f32>;
@group(0) @binding(6) var s_noise: sampler;

const NOISE_SIZE: f32 = 256.0;
const BINS_X: f32 = 16.0;
// `transition::SHARD_LAST_DELAY`, and how long before it breaks loose a
// shard shows its crack.
const SHARD_LAST_DELAY: f32 = 0.38;
const SHARD_CRACK_LEAD: f32 = 0.13;
const BINS_Y: f32 = 9.0;

const KIND_FADE: u32 = 0u;
const KIND_WIPE: u32 = 1u;
const KIND_GROW: u32 = 2u;
const KIND_RIPPLE: u32 = 3u;
const KIND_TIDE: u32 = 4u;
const KIND_TEAR: u32 = 5u;
const KIND_GLITCH: u32 = 6u;
const KIND_SHATTER: u32 = 7u;
const KIND_DISSOLVE: u32 = 8u;
const KIND_SKETCH: u32 = 9u;

const FLAG_AUX: u32 = 1u;
const FLAG_SUBJECT: u32 = 2u;
const FLAG_RTL: u32 = 4u;
const FLAG_GLOW: u32 = 8u;

const ORDER_SUBJECT: u32 = 0u;
const ORDER_TOP_DOWN: u32 = 1u;
const ORDER_CENTER: u32 = 2u;

const FILL_FADE: u32 = 0u;
const FILL_BLEED: u32 = 1u;

const PI: f32 = 3.14159265;
const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

struct Varying {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_transition(@builtin(vertex_index) vertex_id: u32) -> Varying {
    let unit = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    // Rasterize only the clipped rectangle.
    let lo = max(u.bounds.xy, u.clip.xy);
    let hi = max(lo, min(u.bounds.xy + u.bounds.zw, u.clip.xy + u.clip.zw));
    let position = mix(lo, hi, unit);
    let device = position / u.viewport.xy * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    var out: Varying;
    out.position = vec4<f32>(device, 0.0, 1.0);
    return out;
}

// ── sampling ────────────────────────────────────────────────────────────

// `uv` in 0..1 of the bounds, clamped to the tile's texel centres.
fn tile_coord(uv: vec2<f32>, tile: vec4<f32>, inv: vec2<f32>) -> vec2<f32> {
    let texel = clamp(tile.xy + uv * tile.zw, tile.xy + vec2<f32>(0.5), tile.xy + tile.zw - vec2<f32>(0.5));
    return texel * inv;
}

fn from_rgb(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(t_from, s_linear, tile_coord(uv, u.from_tile, u.inv_from_to.xy), 0.0).rgb;
}

fn to_rgb(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(t_to, s_linear, tile_coord(uv, u.to_tile, u.inv_from_to.zw), 0.0).rgb;
}

fn aux(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(t_aux, s_linear, tile_coord(uv, u.aux_tile, u.inv_aux.xy), 0.0);
}

fn has(flag: u32) -> bool {
    return (u.mode.y & flag) != 0u;
}

// ── noise ───────────────────────────────────────────────────────────────

fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn hash21(p: vec2<i32>, seed: u32) -> f32 {
    let h = pcg(bitcast<u32>(p.x) * 1597334677u ^ pcg(bitcast<u32>(p.y) ^ seed));
    return f32(h >> 8u) / 16777216.0;
}

fn hash11(i: u32, seed: u32) -> f32 {
    return f32(pcg(i ^ pcg(seed)) >> 8u) / 16777216.0;
}

// Smooth value noise from one filtered sample of the noise texture: the
// lattice point plus a Hermite-shaped fraction lands between texel centres,
// so the hardware's bilinear filter interpolates smoothly. `seed` shifts
// the lattice and picks a channel.
fn value_noise(p: vec2<f32>, seed: u32) -> f32 {
    let h = pcg(seed);
    let q = p + vec2<f32>(f32(h & 255u), f32((h >> 8u) & 255u));
    let i = floor(q);
    let f = fract(q);
    let w = f * f * (3.0 - 2.0 * f);
    let n = textureSampleLevel(t_noise, s_noise, (i + w + 0.5) / NOISE_SIZE, 0.0);
    switch (h >> 16u) & 3u {
        case 0u: { return n.r; }
        case 1u: { return n.g; }
        case 2u: { return n.b; }
        default: { return n.a; }
    }
}

fn fbm(p: vec2<f32>, seed: u32, octaves: i32) -> f32 {
    var sum = 0.0;
    var amp = 0.5;
    var q = p;
    var norm = 0.0;
    for (var i = 0; i < octaves; i++) {
        sum += amp * value_noise(q, seed + u32(i) * 1013u);
        norm += amp;
        q = q * 2.03 + vec2<f32>(17.3, 9.1);
        amp *= 0.5;
    }
    return sum / norm;
}

// Hermite ramp from `a` to `b`, either way round (`smoothstep` is
// undefined when `a >= b` on some backends).
fn ramp(a: f32, b: f32, x: f32) -> f32 {
    let t = saturate((x - a) / (b - a));
    return t * t * (3.0 - 2.0 * t);
}

// Square (WGSL's `pow` is undefined for a negative base).
fn sq(x: f32) -> f32 {
    return x * x;
}

fn ease(t: f32) -> f32 {
    let x = saturate(t);
    return x * x * (3.0 - 2.0 * x);
}

fn ease_in(t: f32) -> f32 {
    let x = saturate(t);
    return x * x;
}

fn rot(a: f32) -> mat2x2<f32> {
    let c = cos(a);
    let s = sin(a);
    return mat2x2<f32>(c, s, -s, c);
}

// ── fragment ────────────────────────────────────────────────────────────

struct Frame {
    uv: vec2<f32>,
    // q space: y in 0..1 of the height, x in 0..aspect.
    q: vec2<f32>,
    aspect: f32,
    // One device pixel in q units.
    px: f32,
    t: f32,
    seed: u32,
    intensity: f32,
}

fn q_to_uv(q: vec2<f32>, f: Frame) -> vec2<f32> {
    return vec2<f32>(q.x / f.aspect, q.y);
}

@fragment
fn fs_transition(input: Varying) -> @location(0) vec4<f32> {
    let p = input.position.xy;
    if (any(p < u.clip.xy) || any(p > u.clip.xy + u.clip.zw)) {
        discard;
    }
    var f: Frame;
    f.uv = (p - u.bounds.xy) / max(u.bounds.zw, vec2<f32>(1.0));
    f.aspect = u.bounds.z / max(u.bounds.w, 1.0);
    f.q = vec2<f32>(f.uv.x * f.aspect, f.uv.y);
    f.px = 1.0 / max(u.bounds.w, 1.0);
    f.t = saturate(u.timing.x);
    f.seed = u.counts.x;
    f.intensity = saturate(u.timing.z);

    var rgb: vec3<f32>;
    switch u.mode.x {
        case KIND_WIPE: { rgb = wipe(f); }
        case KIND_GROW: { rgb = grow(f); }
        case KIND_RIPPLE: { rgb = ripple(f); }
        case KIND_TIDE: { rgb = tide(f); }
        case KIND_TEAR: { rgb = tear(f); }
        case KIND_GLITCH: { rgb = glitch(f); }
        case KIND_SHATTER: { rgb = shatter(f); }
        case KIND_DISSOLVE: { rgb = dissolve(f); }
        case KIND_SKETCH: { rgb = sketch(f); }
        default: { rgb = mix(from_rgb(f.uv), to_rgb(f.uv), ease(f.t)); }
    }
    // Ends are exact: the outgoing image at 0, the incoming one at 1.
    if (f.t <= 0.0) {
        rgb = from_rgb(f.uv);
    } else if (f.t >= 1.0) {
        rgb = to_rgb(f.uv);
    }
    return vec4<f32>(saturate(rgb), 1.0);
}

// ── Wipe: a soft edge sweeps across in the reading direction ─────────────

fn wipe(f: Frame) -> vec3<f32> {
    let soft = 0.035;
    var x = f.uv.x;
    if (has(FLAG_RTL)) {
        x = 1.0 - x;
    }
    let edge = ease(f.t) * (1.0 + soft) - soft;
    let k = ramp(edge + soft, edge, x);
    let old = from_rgb(f.uv);
    // A faint shade ahead of the edge.
    let shade = 1.0 - 0.18 * exp(-max(x - edge - soft, 0.0) / 0.02) * (1.0 - k) * (1.0 - f.t);
    return mix(old * shade, to_rgb(f.uv), k);
}

// ── Grow: a rounded window opens from the centre, the image settling ─────

fn grow(f: Frame) -> vec3<f32> {
    let e = ease(f.t);
    let half_size = vec2<f32>(f.aspect, 1.0) * 0.5 * (e * 1.08);
    let r = 0.06 * (1.0 - e) + 0.001;
    let d = abs(f.q - vec2<f32>(f.aspect * 0.5, 0.5)) - half_size + vec2<f32>(r);
    let sdf = length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0) - r;
    let inside = ramp(f.px * 1.5, -f.px * 1.5, sdf);
    // The new image settles from a slight zoom.
    let zoom = mix(1.06, 1.0, e);
    let new_uv = (f.uv - 0.5) / zoom + 0.5;
    let shadow = 1.0 - 0.35 * exp(-max(sdf, 0.0) / 0.025) * (1.0 - inside) * (1.0 - e);
    return mix(from_rgb(f.uv) * shadow, to_rgb(new_uv), inside);
}

// ── Ripple: a radial wave front refracts both images, a bright ring rides it

fn ripple(f: Frame) -> vec3<f32> {
    let o = vec2<f32>(u.points.x * f.aspect, u.points.y);
    let corners = max(max(length(o), length(o - vec2<f32>(f.aspect, 0.0))), max(length(o - vec2<f32>(0.0, 1.0)), length(o - vec2<f32>(f.aspect, 1.0))));
    let band = 0.09;
    let radius = ease(f.t) * (corners + band * 1.5);
    let delta = f.q - o;
    let d = length(delta);
    let dir = select(vec2<f32>(0.0, 1.0), delta / max(d, 1e-5), d > 1e-5);
    let w = d - radius;
    let fade = 1.0 - ease(f.t);
    let amp = 0.016 * (0.35 + 0.65 * f.intensity) * fade;
    // Rings concentrated around the front, trailing inside.
    let env = exp(-abs(w) / band * select(2.6, 1.4, w < 0.0));
    let k = 2.0 * PI / (band * 0.9);
    let wave = sin(w * k) * env;
    let slope = cos(w * k) * env;
    let offset = dir * amp * wave;
    let inside = ramp(f.px * 2.0, -f.px * 2.0, w);
    let old = from_rgb(q_to_uv(f.q + offset, f));
    let fresh = to_rgb(q_to_uv(f.q - offset * 0.6, f));
    var rgb = mix(old, fresh, inside);
    // Light catches the wave's slopes; the crest glows.
    rgb *= 1.0 + 0.16 * slope * fade;
    let crest = exp(-sq(w / (band * 0.18)));
    rgb += vec3<f32>(0.22) * crest * fade;
    return rgb;
}

// ── Tide: a noise-edged wave rises from the bottom with foam on its crest ─

fn tide(f: Frame) -> vec3<f32> {
    let amp = 0.035 + 0.035 * f.intensity;
    let foam_w = 0.05;
    let level = ease(f.t) * (1.0 + 2.0 * amp + foam_w + 0.02) - amp;
    let x = f.q.x;
    let swell = (fbm(vec2<f32>(x * 1.6 + u.timing.y * 0.25, 3.7), f.seed, 4) - 0.5) * 2.6 * amp
        + (value_noise(vec2<f32>(x * 14.0 - u.timing.y * 1.5, 1.1), f.seed + 5u) - 0.5) * amp * 0.35
        + sin(x * 7.0 + u.timing.y * 2.6) * amp * 0.3;
    let crest = level + swell;
    let height = 1.0 - f.q.y;
    // Below the crest: new image, refracted near the front.
    let under = crest - height;
    let wet = ramp(-f.px * 1.5, f.px * 1.5, under);
    let near = exp(-max(under, 0.0) / 0.06) * (1.0 - ease(f.t));
    let refract = vec2<f32>(sin(f.q.y * 70.0 + u.timing.y * 5.0), cos(x * 55.0 - u.timing.y * 3.0)) * 0.006 * near * f.intensity;
    var fresh = to_rgb(q_to_uv(f.q + refract, f));
    // A darker, wetter band right behind the foam.
    fresh *= 1.0 - 0.12 * near;
    // Foam: broken bubbles in a band under the crest.
    let foam_band = pow(1.0 - ramp(0.0, foam_w * (0.6 + 0.8 * value_noise(vec2<f32>(x * 6.0, 0.3), f.seed + 9u)), under), 1.6);
    let bubbles = fbm(f.q * vec2<f32>(46.0, 70.0) + vec2<f32>(u.timing.y * 0.8, 0.0), f.seed + 7u, 3);
    let foam = foam_band * (0.35 + 0.65 * ramp(0.32, 0.68, bubbles)) * wet;
    let lip = exp(-sq(under / (f.px * 3.0 + 0.002)));
    var rgb = mix(from_rgb(f.uv), fresh, wet);
    rgb = mix(rgb, vec3<f32>(0.95, 0.97, 0.98), saturate(foam * 0.85 + lip * 0.55));
    // The old image darkens a little under the wave's shadow.
    let shadow = exp(-max(-under, 0.0) / 0.03) * (1.0 - wet) * 0.25;
    return rgb * (1.0 - shadow);
}

// ── Tear: a jagged rip splits the old image; both halves peel and fall ───

fn tear_seam(y: f32, f: Frame) -> f32 {
    let big = (fbm(vec2<f32>(y * 2.6, 1.3), f.seed, 4) - 0.5) * 0.16;
    let fine = (value_noise(vec2<f32>(y * 48.0, 7.7), f.seed + 3u) - 0.5) * 0.018;
    let fibre = (value_noise(vec2<f32>(y * 260.0, 2.1), f.seed + 5u) - 0.5) * 0.005;
    return f.aspect * 0.5 + big + fine + fibre;
}

// Where `p` came from on half `side` (-1 left, +1 right) at time `t`, and
// how far it is inside that half (q units, negative outside).
struct Piece {
    q: vec2<f32>,
    inside: f32,
    // 1 where the rip has run (above its front), 0 where the sheet is whole.
    torn: f32,
    // Distance from the seam into the half (q units).
    seam_d: f32,
}

fn tear_piece(p: vec2<f32>, side: f32, f: Frame) -> Piece {
    let open = ease(f.t / 0.32);
    let fly = ease_in(saturate((f.t - 0.22) / 0.78));
    // Phase 2: the half slides out, turns and drops.
    let pivot = vec2<f32>(f.aspect * (0.5 + side * 0.25), 0.5);
    let shift = vec2<f32>(side * (f.aspect * 0.62 + 0.12), 0.18) * fly;
    let turn = side * (0.05 + 0.13 * f.intensity) * fly;
    var q = rot(-turn) * (p - pivot - shift) + pivot;
    // Phase 1: the rip runs down from the top and the halves bend apart
    // above its front, hinged there.
    let front = open * 1.15;
    let bend = side * (0.012 + 0.03 * f.intensity) * open * max(front - q.y, 0.0) / 1.15;
    q.x -= bend;
    var out: Piece;
    out.q = q;
    let seam = tear_seam(q.y, f);
    let to_seam = (q.x - seam) * side;
    let box = min(min(q.x, f.aspect - q.x), min(q.y, 1.0 - q.y));
    // Below the rip's front the sheet is whole: both halves reach a hair
    // past the seam so no line shows between them.
    let slack = select(0.0, f.px * 1.5, q.y > front);
    out.inside = min(to_seam + slack, box);
    out.seam_d = to_seam;
    out.torn = ramp(front + 0.005, front - 0.02, q.y);
    return out;
}

fn tear(f: Frame) -> vec3<f32> {
    let left = tear_piece(f.q, -1.0, f);
    let right = tear_piece(f.q, 1.0, f);
    let sep = ease(f.t / 0.32);
    var rgb = to_rgb(f.uv);
    // Drop shadows of both halves on the new image: a soft band just
    // outside each, widening as they lift.
    let lift = 0.004 + 0.012 * ease(f.t / 0.5);
    let blur = 0.004 + lift;
    let shade = max(ramp(-blur, blur * 0.3, left.inside + lift * 0.6), ramp(-blur, blur * 0.3, right.inside + lift * 0.6));
    rgb *= 1.0 - 0.5 * shade * sep * (1.0 - ramp(0.85, 1.0, f.t));
    for (var i = 0; i < 2; i++) {
        var piece = right;
        if (i == 0) {
            piece = left;
        }
        let side = select(1.0, -1.0, i == 0);
        let a = ramp(-f.px, f.px, piece.inside);
        if (a <= 0.0) {
            continue;
        }
        var col = from_rgb(q_to_uv(piece.q, f));
        let seam_d = piece.seam_d;
        // The torn edge: a paper-white rim of uneven width, with fibres,
        // shaded where the half curls away.
        let rim_w = (0.0025 + 0.006 * value_noise(vec2<f32>(piece.q.y * 34.0, side * 9.0), f.seed + 11u)) * sep;
        let fibres = value_noise(vec2<f32>(piece.q.y * 900.0, seam_d * 600.0), f.seed + 13u);
        let rim = (1.0 - ramp(rim_w * 0.6, rim_w + f.px, seam_d)) * step(0.0, rim_w - 0.0004) * piece.torn;
        let paper = vec3<f32>(0.96, 0.95, 0.92) * (0.86 + 0.14 * fibres);
        let curl = 1.0 - 0.22 * exp(-max(seam_d, 0.0) / 0.03) * sep * piece.torn;
        col = mix(col * curl, paper, rim);
        rgb = mix(rgb, col, a);
    }
    return rgb;
}

// ── Glitch: displaced scan bands, a brief RGB split, then it settles ─────
//
// Photosensitivity: luminance never pulses more than 3 times a second and
// by at most 6 %; each band switches from the old image to the new one
// once and never back.

fn glitch(f: Frame) -> vec3<f32> {
    let time = u.timing.y;
    let live = 1.0 - ramp(0.3, 0.85, f.t);
    let strong = 1.0 - ramp(0.22, 0.32, f.t);
    let step_i = u32(floor(time * 12.0));
    let amp = (0.02 + 0.06 * f.intensity) * live;
    // Bands of uneven height, re-cut each step.
    let rows = 18.0 + 22.0 * hash11(step_i, f.seed);
    let band = u32(floor(f.q.y * rows));
    let h = hash21(vec2<i32>(i32(band), i32(step_i)), f.seed + 17u);
    let strong_band = select(0.12, 1.0, h > 0.62);
    var dx = (hash21(vec2<i32>(i32(band), i32(step_i)), f.seed + 29u) - 0.5) * 2.0 * amp * strong_band;
    // A few corrupted blocks.
    let block = vec2<i32>(floor(f.q * vec2<f32>(24.0, 14.0)));
    if (hash21(block + vec2<i32>(i32(step_i) * 7, 0), f.seed + 41u) > 0.985) {
        dx += (hash21(block, f.seed + 43u) - 0.5) * 0.25 * live;
    }
    // Each band of a fixed grid switches to the new image once.
    let swap_rows = 26.0;
    let swap_band = i32(floor(f.q.y * swap_rows + value_noise(vec2<f32>(f.q.x * 3.0, 0.0), f.seed) * 0.6));
    let swap_at = 0.18 + 0.52 * hash21(vec2<i32>(swap_band, 3), f.seed + 53u);
    let shows_new = ramp(swap_at, swap_at + 0.06, f.t);
    let split = (0.002 + 0.006 * f.intensity) * strong + 0.0015 * live;
    let uv = q_to_uv(f.q + vec2<f32>(dx, 0.0), f);
    let du = vec2<f32>(split / f.aspect, 0.0);
    let old = vec3<f32>(from_rgb(uv + du).r, from_rgb(uv).g, from_rgb(uv - du).b);
    let fresh = vec3<f32>(to_rgb(uv + du).r, to_rgb(uv).g, to_rgb(uv - du).b);
    var rgb = mix(old, fresh, shows_new);
    // Faint scanlines while it is live.
    let scan = 1.0 - 0.035 * live * step(0.5, fract(f.q.y / (f.px * 3.0)));
    // At most three gentle pulses a second, only in the first 30 %.
    let slot = u32(floor(time * 3.0));
    let pulse = select(0.0, 1.0, hash11(slot, f.seed + 61u) > 0.45 && fract(time * 3.0) < 0.2) * strong;
    rgb *= scan * (1.0 + 0.06 * pulse);
    return rgb;
}

// ── Shatter: Voronoi shards burst from the origin, spin and fall ─────────
//
// The renderer moves the shards (`ShardCell`); here each pixel finds the
// shard whose inverse transform maps it back inside that shard's own
// Voronoi cell (nearest rest centre), the frontmost one where they overlap.

// The shard whose rest cell holds `q`: its index and how far `q` is inside
// it (half the gap between the nearest and second-nearest rest centres).
fn nearest_cell(q: vec2<f32>, f: Frame) -> vec2<f32> {
    let cols = i32(u.counts.z);
    let rows = i32(u.counts.w);
    let size = vec2<f32>(f.aspect / f32(cols), 1.0 / f32(rows));
    let cell = clamp(vec2<i32>(floor(q / size)), vec2<i32>(0), vec2<i32>(cols - 1, rows - 1));
    var best = 1e9;
    var second = 1e9;
    var best_i = 0;
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let c = cell + vec2<i32>(i, j);
            if (c.x < 0 || c.y < 0 || c.x >= cols || c.y >= rows) {
                continue;
            }
            let k = c.y * cols + c.x;
            let centre = u.cells[k].b.xy;
            let d = dot(q - centre, q - centre);
            if (d < best) {
                second = best;
                best = d;
                best_i = k;
            } else if (d < second) {
                second = d;
            }
        }
    }
    return vec2<f32>(f32(best_i), (sqrt(second) - sqrt(best)) * 0.5);
}

// A shard's pixels at its own point `q`: a glint on its rim that turns with
// it, a little darker as it tilts away, and a hairline crack along its edges
// just before it breaks loose.
fn shard_colour(cell: ShardCell, q: vec2<f32>, edge: f32, f: Frame) -> vec3<f32> {
    let moved = cell.c.w;
    var col = from_rgb(q_to_uv(q, f));
    let rim = 1.0 - ramp(f.px * 0.6, f.px * 2.5 + 0.0012, edge);
    let glint = pow(saturate(0.5 + 0.5 * cos(cell.c.z * 2.3 + 1.7)), 4.0);
    col = col * (1.0 - 0.22 * moved * (1.0 - glint));
    col = mix(col, vec3<f32>(0.92, 0.96, 1.0), rim * (0.18 + 0.7 * glint) * saturate(moved * 8.0));
    let crack = (1.0 - ramp(f.px * 0.3, f.px * 1.4, edge)) * saturate((f.t - cell.d.x + SHARD_CRACK_LEAD - 0.01) / 0.06) * (1.0 - saturate(moved * 10.0));
    return mix(col, vec3<f32>(0.97, 0.98, 1.0), crack * 0.75);
}

fn shatter(f: Frame) -> vec3<f32> {
    let count = i32(u.counts.y);
    var top_depth = -1.0;
    var top_rgb = vec3<f32>(0.0);
    var top_alpha = 0.0;
    let cell_size = 1.0 / f32(max(u.counts.w, 1u));
    let reach = cell_size * cell_size * 2.25;
    // Flying shards (only those the pixel's bin lists), frontmost first.
    let bin = vec2<u32>(clamp(f.uv * vec2<f32>(BINS_X, BINS_Y), vec2<f32>(0.0), vec2<f32>(BINS_X - 1.0, BINS_Y - 1.0)));
    let shard_set = u.bins[bin.y * u32(BINS_X) + bin.x];
    for (var word = 0u; word < 4u; word++) {
        var bits = shard_set[word];
        loop {
            if (bits == 0u) {
                break;
            }
            let k = i32(word * 32u + firstTrailingBit(bits));
            bits &= bits - 1u;
            if (k >= count) {
                break;
            }
            let cell = u.cells[k];
            if (cell.c.x <= 0.0 || cell.c.y <= top_depth) {
                continue;
            }
            let m = mat2x2<f32>(cell.a.x, cell.a.z, cell.a.y, cell.a.w);
            let q = m * (f.q - cell.b.zw) + cell.b.xy;
            let r = q - cell.b.xy;
            // Border cells end at the image edge.
            if (dot(r, r) > reach || q.x < 0.0 || q.y < 0.0 || q.x > f.aspect || q.y > 1.0) {
                continue;
            }
            let near = nearest_cell(q, f);
            if (i32(near.x) != k) {
                continue;
            }
            top_depth = cell.c.y;
            top_rgb = shard_colour(cell, q, near.y, f);
            top_alpha = cell.c.x;
        }
    }
    // Under them, the shards still in place. A shard breaks loose later the
    // farther its centre is from the origin (`transition::shards`), and its
    // centre is within about a cell of the pixel: far enough from the
    // break-up front, the pixel's shard is surely still whole and uncracked
    // (no lookup), or surely gone.
    if (top_alpha < 1.0) {
        let o = vec2<f32>(u.points.x * f.aspect, u.points.y);
        let far = max(max(length(o), length(o - vec2<f32>(f.aspect, 0.0))), max(length(o - vec2<f32>(0.0, 1.0)), length(o - vec2<f32>(f.aspect, 1.0))));
        let dist = length(f.q - o);
        let slack = cell_size * 1.6;
        let early = max(dist - slack, 0.0) / far * SHARD_LAST_DELAY * 0.92;
        let late = (dist + slack) / far * SHARD_LAST_DELAY * 0.92 + SHARD_LAST_DELAY * 0.08;
        var rest = vec3<f32>(0.0);
        var whole = false;
        if (f.t < early - SHARD_CRACK_LEAD) {
            rest = from_rgb(f.uv);
            whole = true;
        } else if (f.t <= late) {
            let near = nearest_cell(f.q, f);
            let k = i32(near.x);
            if (k < count && u.cells[k].c.w <= 0.0) {
                rest = shard_colour(u.cells[k], f.q, near.y, f);
                whole = true;
            }
        }
        if (whole) {
            top_rgb = mix(rest, top_rgb, top_alpha);
            top_alpha = 1.0;
        }
    }
    return mix(to_rgb(f.uv), top_rgb, top_alpha);
}

// ── Dissolve: a noise threshold burns the old image away, edge aglow ─────

fn dissolve(f: Frame) -> vec3<f32> {
    let n = ramp(0.18, 0.82, fbm(f.q * (2.4 + 1.6 * f.intensity) + vec2<f32>(f32(f.seed & 255u) * 0.37), f.seed, 5));
    let width = 0.07;
    let th = ease(f.t) * (1.0 + 3.0 * width) - width;
    let d = n - th;
    let revealed = ramp(0.004, -0.004, d);
    var old = from_rgb(f.uv);
    // Charring ahead of the burn.
    old *= 1.0 - 0.75 * (1.0 - ramp(0.0, width * 1.6, d)) * step(0.0, d);
    var rgb = mix(old, to_rgb(f.uv), revealed);
    let core = exp(-sq(d / (width * 0.22)));
    let halo = exp(-sq(d / (width * 0.9))) * 0.45;
    let embers = ramp(0.7, 0.95, value_noise(f.q * 140.0, f.seed + 71u)) * exp(-sq(d / (width * 0.6)));
    let glow = u.glow.rgb;
    rgb += glow * (core * 1.25 + halo + embers * 0.8) * (1.0 - ramp(0.9, 1.0, f.t));
    return rgb;
}

// ── Sketch: line art of the new image draws itself, then the image fills in

fn luma_to(uv: vec2<f32>) -> f32 {
    return dot(to_rgb(uv), LUMA);
}

// Edge strength of the new image at `step` device px: a Sobel operator from
// four bilinear taps on the diagonals (each tap averages a 2 × 2 block,
// which supplies Sobel's smoothing), scaled to Sobel's range.
fn sobel(uv: vec2<f32>, step_px: f32) -> f32 {
    let d = vec2<f32>(step_px, step_px) / u.bounds.zw;
    let tl = luma_to(uv + vec2<f32>(-d.x, -d.y));
    let tr = luma_to(uv + vec2<f32>(d.x, -d.y));
    let bl = luma_to(uv + vec2<f32>(-d.x, d.y));
    let br = luma_to(uv + vec2<f32>(d.x, d.y));
    let gx = (tr + br) - (tl + bl);
    let gy = (bl + br) - (tl + tr);
    return length(vec2<f32>(gx, gy)) * 2.0;
}

// When the line at `uv` is drawn, 0..1 of the drawing phase.
fn sketch_order(uv: vec2<f32>, subject: f32, f: Frame) -> f32 {
    let wobble = (value_noise(uv * 7.0, f.seed + 83u) - 0.5) * 0.08;
    switch u.mode.w {
        case ORDER_SUBJECT: {
            if (has(FLAG_SUBJECT)) {
                if (subject > 0.5) {
                    return 0.4 * saturate(uv.y + wobble * 0.5);
                }
                return 0.4 + 0.6 * saturate(aux(uv).g * 1.6 + wobble);
            }
            return saturate(uv.y + wobble);
        }
        case ORDER_TOP_DOWN: {
            return saturate(uv.y + wobble);
        }
        case ORDER_CENTER: {
            let d = length((uv - 0.5) * vec2<f32>(f.aspect, 1.0)) / length(vec2<f32>(f.aspect, 1.0) * 0.5);
            return saturate(d + wobble);
        }
        default: {
            let o = u.points.zw;
            let far = max(max(length((vec2<f32>(0.0) - o) * vec2<f32>(f.aspect, 1.0)), length((vec2<f32>(1.0, 0.0) - o) * vec2<f32>(f.aspect, 1.0))), max(length((vec2<f32>(0.0, 1.0) - o) * vec2<f32>(f.aspect, 1.0)), length((vec2<f32>(1.0) - o) * vec2<f32>(f.aspect, 1.0))));
            return saturate(length((uv - o) * vec2<f32>(f.aspect, 1.0)) / max(far, 1e-3) + wobble);
        }
    }
}

fn sketch(f: Frame) -> vec3<f32> {
    let share = clamp(u.sketch.y, 0.05, 0.95);
    let draw = saturate(f.t / share);
    let fill = saturate((f.t - share) / (1.0 - share));
    let lw = max(u.sketch.x, 0.5);
    let uv = f.uv;
    let a = select(vec4<f32>(0.0, 1.0, 0.5, 1.0), aux(uv), has(FLAG_AUX));
    let subject_on = has(FLAG_SUBJECT);
    let subject = select(0.0, a.r, subject_on);

    // The subject's contour, from the mask's 0.5 level (antialiased).
    var contour = 0.0;
    var contour_glow = 0.0;
    if (subject_on) {
        let e = vec2<f32>(1.0) / u.bounds.zw;
        let gx = aux(uv + vec2<f32>(e.x, 0.0)).r - aux(uv - vec2<f32>(e.x, 0.0)).r;
        let gy = aux(uv + vec2<f32>(0.0, e.y)).r - aux(uv - vec2<f32>(0.0, e.y)).r;
        let grad = max(length(vec2<f32>(gx, gy)) * 0.5, 1e-4);
        let dist = abs(subject - 0.5) / grad;
        let width = lw * 1.6;
        contour = 1.0 - ramp(width * 0.5, width * 0.5 + 1.2, dist);
        contour_glow = exp(-sq(dist / (width * 3.5)));
    }

    // The picture's own edges: two scales, so fine texture reads lighter
    // than real contours.
    let fine = sobel(uv, max(lw * 0.6, 1.0));
    // The broad gate: near a line of the helper image (its edges at half
    // size, specks blurred away), else the edges at three times the scale.
    var broad = 0.0;
    if (has(FLAG_AUX)) {
        broad = 1.0 - ramp(0.04, 0.14, a.b);
    } else {
        broad = ramp(0.1, 0.3, sobel(uv, max(lw * 1.8, 2.0)));
    }
    let edge = ramp(0.16, 0.42, fine) * broad;
    let lines = saturate(edge * select(0.75, 0.55, subject_on));

    // The drawing front: each line appears when the front reaches it,
    // brighter right at the pen.
    let order = sketch_order(uv, subject, f);
    let soft = 0.05;
    let shown = ramp(order - soft, order, draw * (1.0 + soft));
    let pen = exp(-sq((draw * (1.0 + soft) - order) / 0.025)) * (1.0 - fill);
    let ink_fade = 1.0 - ramp(0.35, 1.0, fill);
    let ink = saturate(max(contour, lines) * shown) * ink_fade;

    // Under the lines: the old image under a wash.
    let washed = mix(from_rgb(uv), u.wash.rgb, u.wash.a * ease(draw / 0.3));
    // The fill.
    var k = 0.0;
    let fresh = to_rgb(uv);
    var pigment = fresh;
    switch u.mode.z {
        case FILL_FADE: {
            k = ease(fill);
        }
        case FILL_BLEED: {
            // Colour bleeds out from the lines: noise-driven, with a darker,
            // wetter rim where the pigment pools.
            let n = fbm(f.q * 9.0, f.seed + 91u, 4);
            let grain = mix(n, fbm(f.q * 31.0, f.seed + 97u, 3), 0.35);
            // Wet paint spreads out from the lines in drawing order; its edge
            // wanders with the paper's grain.
            let wet = 0.45 * order + 0.55 * a.b + (grain - 0.5) * 0.35;
            let front = ease(fill) * 1.45 - 0.1;
            k = ramp(front + 0.04, front - 0.16, wet);
            let rim = exp(-sq((wet - front + 0.02) / 0.03)) * (1.0 - fill);
            pigment = mix(fresh, fresh * vec3<f32>(0.86, 0.84, 0.9), saturate(rim * 0.7));
            k = max(k, ease((fill - 0.8) / 0.2));
        }
        default: {
            // Along the strokes: the drawing order again, lines first.
            let along = order + a.b * 0.35;
            k = ramp(along + 0.06, along, fill * 1.4);
            k = max(k, ease((fill - 0.85) / 0.15));
        }
    }
    var rgb = mix(washed, pigment, saturate(k));
    // Lines over everything, with a soft glow under them.
    if (has(FLAG_GLOW)) {
        let halo = saturate(max(contour_glow * shown, lines * shown * 0.4)) * ink_fade;
        rgb = mix(rgb, u.glow.rgb, halo * 0.35 * u.glow.a);
    }
    rgb = mix(rgb, u.line.rgb, ink * u.line.a);
    rgb += u.line.rgb * pen * ink * 0.5;
    return rgb;
}

//! Mandara patch 16: the GPU half of `gpui::TransitionSprite`, a per-pixel
//! transition between two full-size images (a wallpaper change).
//!
//! The primitive is rare (a few seconds per wallpaper change, on one
//! surface), so nothing exists until a frame draws one: the shader module,
//! pipeline, uniform buffer and bind groups are built then, reused while
//! frames keep drawing transitions, and released [`IDLE_RELEASE`] after the
//! last one (or with the renderer). Every frame writes one uniform block
//! per transition; the bind groups (which name the three atlas textures)
//! are rebuilt only when a texture changes.

use bytemuck::{Pod, Zeroable};
use gpui::{AtlasTextureId, AtlasTile, TRANSITION_KIND_SHATTER, TRANSITION_MAX_SHARDS, TransitionParams, TransitionSprite};
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

/// The shader, its own module (`shaders_transition.wgsl`).
pub(crate) const TRANSITION_SHADER: &str = include_str!("shaders_transition.wgsl");

/// Transition resources are released this long after the last frame that
/// drew one.
pub(crate) const IDLE_RELEASE: Duration = Duration::from_millis(1500);

/// Most transitions drawn in one frame (one uniform slot each); further
/// ones in the same frame are skipped.
pub(crate) const MAX_PER_FRAME: usize = 4;

const MAX_SHARDS: usize = TRANSITION_MAX_SHARDS as usize;
/// Shatter's screen bins (the shader's `BINS_X` × `BINS_Y`): each lists the
/// shards that may cover it, so a pixel tests a handful, not all of them.
pub(crate) const BINS: (usize, usize) = (16, 9);
/// Side of the tiling noise texture (the shader's `NOISE_SIZE`).
pub(crate) const NOISE_SIZE: u32 = 256;

/// One shard of a Shatter, as the shader reads it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub(crate) struct ShardCell {
    /// Inverse of the shard's rotation and scale, row major.
    pub(crate) a: [f32; 4],
    /// Rest centre (q space) and current centre.
    pub(crate) b: [f32; 4],
    /// Alpha, depth, spin (radians), how far it has gone (0..1).
    pub(crate) c: [f32; 4],
    /// When it breaks loose (progress), unused ×3.
    pub(crate) d: [f32; 4],
}

/// The shader's `TransitionUniforms` (std140-compatible: only `vec4`s).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct TransitionUniforms {
    pub(crate) viewport: [f32; 4],
    pub(crate) bounds: [f32; 4],
    pub(crate) clip: [f32; 4],
    pub(crate) from_tile: [f32; 4],
    pub(crate) to_tile: [f32; 4],
    pub(crate) aux_tile: [f32; 4],
    pub(crate) inv_from_to: [f32; 4],
    pub(crate) inv_aux: [f32; 4],
    pub(crate) mode: [u32; 4],
    pub(crate) counts: [u32; 4],
    pub(crate) timing: [f32; 4],
    pub(crate) points: [f32; 4],
    pub(crate) sketch: [f32; 4],
    pub(crate) line: [f32; 4],
    pub(crate) glow: [f32; 4],
    pub(crate) wash: [f32; 4],
    pub(crate) cells: [ShardCell; MAX_SHARDS],
    pub(crate) bins: [[u32; 4]; BINS.0 * BINS.1],
}

impl TransitionUniforms {
    /// The block for `sprite` in a `viewport` (device px), with the atlas
    /// textures' sizes.
    pub(crate) fn new(sprite: &TransitionSprite, viewport: [f32; 2], sizes: [[f32; 2]; 3]) -> Self {
        let p = &sprite.params;
        let b = sprite.bounds;
        let m = sprite.content_mask.bounds;
        let tile = |t: &AtlasTile| [t.bounds.origin.x.0 as f32, t.bounds.origin.y.0 as f32, t.bounds.size.width.0 as f32, t.bounds.size.height.0 as f32];
        let inv = |s: [f32; 2]| [1.0 / s[0].max(1.0), 1.0 / s[1].max(1.0)];
        let (fi, ti, ai) = (inv(sizes[0]), inv(sizes[1]), inv(sizes[2]));
        let aspect = b.size.width.0 / b.size.height.0.max(1.0);
        let (shards, cells, bins) = if p.kind == TRANSITION_KIND_SHATTER { shards(p, aspect) } else { (0, [ShardCell::default(); MAX_SHARDS], [[0; 4]; BINS.0 * BINS.1]) };
        let [cols, rows] = shard_grid(p);
        Self {
            viewport: [viewport[0], viewport[1], 0.0, 0.0],
            bounds: [b.origin.x.0, b.origin.y.0, b.size.width.0, b.size.height.0],
            clip: [m.origin.x.0, m.origin.y.0, m.size.width.0, m.size.height.0],
            from_tile: tile(&sprite.from),
            to_tile: tile(&sprite.to),
            aux_tile: tile(&sprite.aux),
            inv_from_to: [fi[0], fi[1], ti[0], ti[1]],
            inv_aux: [ai[0], ai[1], 0.0, 0.0],
            mode: [p.kind, p.flags, p.fill_style, p.draw_order],
            counts: [p.seed, shards, cols, rows],
            timing: [p.progress.clamp(0.0, 1.0), p.time.max(0.0), p.intensity.clamp(0.0, 1.0), p.scale.max(0.01)],
            points: [p.origin[0], p.origin[1], p.focus[0], p.focus[1]],
            sketch: [p.line_width, p.draw_share, 0.0, 0.0],
            line: p.line,
            glow: p.glow,
            wash: p.wash,
            cells,
            bins,
        }
    }
}

/// A Shatter's grid, within bounds (at least 1 × 1, at most
/// [`TRANSITION_MAX_SHARDS`] cells).
pub(crate) fn shard_grid(p: &TransitionParams) -> [u32; 2] {
    let cols = p.grid[0].clamp(1, TRANSITION_MAX_SHARDS);
    let rows = p.grid[1].clamp(1, (TRANSITION_MAX_SHARDS / cols).max(1));
    [cols, rows]
}

/// A stable value in `0..1` for `(i, salt)` of transition `seed` (the same
/// hash family as the shader's `hash11`).
pub(crate) fn hash01(i: u32, seed: u32) -> f32 {
    fn pcg(v: u32) -> u32 {
        let state = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
        let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
        (word >> 22) ^ word
    }
    (pcg(i ^ pcg(seed)) >> 8) as f32 / 16_777_216.0
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The latest a shard breaks loose (progress): the rest of the time takes
/// it out.
const SHARD_LAST_DELAY: f32 = 0.38;

/// Every shard of a Shatter at `p.progress`: rest centres on a jittered
/// grid (q space: y in 0..1, x in 0..aspect), and for each the transform
/// that has moved it. A shard breaks loose later the farther it is from
/// the origin, bursts away from it, spins, grows a little as it flies at
/// the viewer, falls, and fades: gone by progress 1, untouched at 0.
pub(crate) fn shards(p: &TransitionParams, aspect: f32) -> (u32, [ShardCell; MAX_SHARDS], [[u32; 4]; BINS.0 * BINS.1]) {
    let mut cells = [ShardCell::default(); MAX_SHARDS];
    let mut bins = [[0u32; 4]; BINS.0 * BINS.1];
    let [cols, rows] = shard_grid(p);
    let (cw, ch) = (aspect / cols as f32, 1.0 / rows as f32);
    let origin = [p.origin[0] * aspect, p.origin[1]];
    let t = p.progress.clamp(0.0, 1.0);
    let intensity = p.intensity.clamp(0.0, 1.0);
    let corners = [[0.0, 0.0], [aspect, 0.0], [0.0, 1.0], [aspect, 1.0]];
    let far = corners.iter().map(|c| ((c[0] - origin[0]).powi(2) + (c[1] - origin[1]).powi(2)).sqrt()).fold(1e-3_f32, f32::max);
    let count = cols * rows;
    for k in 0..count {
        let (i, j) = (k % cols, k / cols);
        let jitter = |salt: u32| (hash01(k * 8 + salt, p.seed) - 0.5) * 0.8;
        let rest = [(i as f32 + 0.5 + jitter(0)) * cw, (j as f32 + 0.5 + jitter(1)) * ch];
        let (dx, dy) = (rest[0] - origin[0], rest[1] - origin[1]);
        let dist = (dx * dx + dy * dy).sqrt();
        let delay = dist / far * SHARD_LAST_DELAY * 0.92 + hash01(k * 8 + 2, p.seed) * SHARD_LAST_DELAY * 0.08;
        let u = ((t - delay) / (1.0 - SHARD_LAST_DELAY)).clamp(0.0, 1.0);
        let (ux, uy) = if dist > 1e-3 { (dx / dist, dy / dist) } else { (0.0, -1.0) };
        // A little sideways scatter of the direction.
        let wobble = (hash01(k * 8 + 3, p.seed) - 0.5) * 0.6;
        let (ux, uy) = (ux * wobble.cos() - uy * wobble.sin(), ux * wobble.sin() + uy * wobble.cos());
        let speed = (0.35 + 0.35 * hash01(k * 8 + 4, p.seed)) * (0.5 + 0.5 * intensity);
        let burst = speed * (1.0 - (1.0 - u) * (1.0 - u));
        let fall = 0.9 * u * u;
        let centre = [rest[0] + ux * burst, rest[1] + uy * burst + fall];
        let spin = (hash01(k * 8 + 5, p.seed) - 0.5) * 2.0 * (0.6 + 1.4 * intensity) * u;
        let scale = 1.0 + 0.3 * hash01(k * 8 + 6, p.seed) * smooth(u);
        // Inverse of rotate-then-scale: R(-spin) / scale.
        let (s, c) = spin.sin_cos();
        let inv = [c / scale, s / scale, -s / scale, c / scale];
        let alpha = 1.0 - smooth((u - 0.45) / 0.55);
        // Flying shards are in front of the ones still in place.
        let depth = hash01(k * 8 + 7, p.seed) * 0.5 + if u > 0.0 { 1.0 + u } else { 0.0 };
        cells[k as usize] = ShardCell { a: inv, b: [rest[0], rest[1], centre[0], centre[1]], c: [alpha, depth, spin, u], d: [delay, 0.0, 0.0, 0.0] };
        // Only flying shards are binned; the shader finds a resting one by
        // the pixel's own Voronoi cell.
        if alpha > 0.0 && u > 0.0 {
            // A shard reaches at most this far from its centre (jittered
            // Voronoi cells stay within 1.25 grid diagonals of their seed).
            let reach = (cw * cw + ch * ch).sqrt() * 1.25 * scale;
            bin_circle(&mut bins, k as usize, centre, reach, aspect);
        }
    }
    (count, cells, bins)
}

/// Marks shard `k` in every bin its circle (`centre`, `radius`, q space)
/// touches.
fn bin_circle(bins: &mut [[u32; 4]; BINS.0 * BINS.1], k: usize, centre: [f32; 2], radius: f32, aspect: f32) {
    let (bw, bh) = (aspect / BINS.0 as f32, 1.0 / BINS.1 as f32);
    let range = |c: f32, size: f32, n: usize| -> Option<(usize, usize)> {
        let lo = ((c - radius) / size).floor();
        let hi = ((c + radius) / size).floor();
        if hi < 0.0 || lo > (n - 1) as f32 {
            return None;
        }
        Some((lo.max(0.0) as usize, (hi as usize).min(n - 1)))
    };
    let (Some((x0, x1)), Some((y0, y1))) = (range(centre[0], bw, BINS.0), range(centre[1], bh, BINS.1)) else { return };
    for y in y0..=y1 {
        for x in x0..=x1 {
            bins[y * BINS.0 + x][k / 32] |= 1 << (k % 32);
        }
    }
}

/// The tiling noise texture's texels: four independent channels of white
/// noise (the shader filters them into smooth value noise).
pub(crate) fn noise_texels() -> Vec<u8> {
    let n = (NOISE_SIZE * NOISE_SIZE) as usize;
    let mut texels = Vec::with_capacity(n * 4);
    for i in 0..n as u32 * 4 {
        texels.push((hash01(i, 0x6e6f_6973) * 256.0) as u8);
    }
    texels
}

/// Which atlas textures a transition's bind group names, with their
/// generations (a texture id can be reused by a new texture).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct BindKey {
    pub(crate) ids: [AtlasTextureId; 3],
    pub(crate) generations: [u64; 3],
}

/// Everything a renderer holds while it draws transitions.
pub(crate) struct TransitionResources {
    pub(crate) pipeline: wgpu::RenderPipeline,
    pub(crate) layout: wgpu::BindGroupLayout,
    pub(crate) uniforms: wgpu::Buffer,
    /// Byte stride of one uniform slot (the block, aligned).
    pub(crate) stride: u64,
    pub(crate) bind_groups: [Option<(BindKey, wgpu::BindGroup)>; MAX_PER_FRAME],
    pub(crate) last_used: Instant,
    /// 256² RGBA noise (256 KiB) and its repeating sampler.
    pub(crate) noise: wgpu::TextureView,
    pub(crate) noise_sampler: wgpu::Sampler,
}

impl TransitionResources {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, texture_instances: bool) -> Self {
        // Mandara patch 18: native legacy GL needs the portable bit scan too.
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("transition_shader"), source: wgpu::ShaderSource::Wgsl(crate::compatibility::transition_shader(texture_instances)) });
        let size = std::mem::size_of::<TransitionUniforms>() as u64;
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("transition_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: NonZeroU64::new(size) },
                    count: None,
                },
                texture(1),
                texture(2),
                texture(3),
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                texture(5),
                wgpu::BindGroupLayoutEntry { binding: 6, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("transition_pipeline_layout"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("transitions"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_transition"), buffers: &[], compilation_options: wgpu::PipelineCompilationOptions::default() },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_transition"),
                // Opaque output: it replaces what is under it.
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let stride = size.next_multiple_of(u64::from(device.limits().min_uniform_buffer_offset_alignment).max(16));
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("transition_uniforms"), size: stride * MAX_PER_FRAME as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let extent = wgpu::Extent3d { width: NOISE_SIZE, height: NOISE_SIZE, depth_or_array_layers: 1 };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("transition_noise"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &noise_texels(),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(NOISE_SIZE * 4), rows_per_image: None },
            extent,
        );
        let noise = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let noise_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("transition_noise_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self { pipeline, layout, uniforms, stride, bind_groups: Default::default(), last_used: Instant::now(), noise, noise_sampler }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TRANSITION_KIND_FADE, TRANSITION_KIND_SKETCH};

    fn shatter(progress: f32) -> TransitionParams {
        TransitionParams { kind: TRANSITION_KIND_SHATTER, progress, origin: [0.5, 0.5], seed: 7, intensity: 1.0, grid: [13, 7], ..Default::default() }
    }

    #[test]
    fn uniform_block_is_vec4_packed() {
        // 16 vec4 headers plus 128 shards of 4 vec4 each.
        assert_eq!(std::mem::size_of::<ShardCell>(), 64);
        assert_eq!(std::mem::size_of::<TransitionUniforms>(), 16 * 16 + 128 * 64 + 144 * 16);
        // Within WebGL2's 16 KiB uniform block.
        assert!(std::mem::size_of::<TransitionUniforms>() <= 16 * 1024);
        assert_eq!(std::mem::align_of::<TransitionUniforms>(), 4);
    }

    #[test]
    fn shards_rest_at_the_start_and_are_gone_at_the_end() {
        let (n, start, bins) = shards(&shatter(0.0), 16.0 / 9.0);
        assert!(bins.iter().all(|b| *b == [0; 4]), "shards at rest need no bins");
        // A flying shard is in the bin around its current centre.
        let (m, mid, mid_bins) = shards(&shatter(0.3), 16.0 / 9.0);
        for (k, cell) in mid[..m as usize].iter().enumerate().filter(|(_, c)| c.c[3] > 0.0 && c.c[0] > 0.0) {
            let (bx, by) = (((cell.b[2] / (16.0 / 9.0)) * 16.0).clamp(0.0, 15.0) as usize, (cell.b[3] * 9.0).clamp(0.0, 8.0) as usize);
            assert_ne!(mid_bins[by * 16 + bx][k / 32] & (1 << (k % 32)), 0, "shard {k} is in its bin");
        }
        assert_eq!(n, 91);
        for cell in &start[..n as usize] {
            assert_eq!(cell.c[0], 1.0, "opaque at rest");
            assert_eq!([cell.b[0], cell.b[1]], [cell.b[2], cell.b[3]], "not moved");
            assert_eq!(cell.a, [1.0, 0.0, -0.0, 1.0], "identity transform");
            assert!(cell.d[0] <= SHARD_LAST_DELAY + 1e-6);
        }
        let (_, end, end_bins) = shards(&shatter(1.0), 16.0 / 9.0);
        assert!(end_bins.iter().all(|b| *b == [0; 4]), "gone shards are in no bin");
        assert!(end[..n as usize].iter().all(|c| c.c[0] == 0.0 && c.c[3] == 1.0), "every shard has left");
        // Untouched cells past the grid stay transparent.
        assert!(end[n as usize..].iter().all(|c| c.c[0] == 0.0));
    }

    #[test]
    fn shards_leave_from_the_origin_outward() {
        let p = shatter(0.25);
        let (n, cells, _) = shards(&p, 16.0 / 9.0);
        let origin = [0.5 * 16.0 / 9.0, 0.5];
        let dist = |c: &ShardCell| ((c.b[0] - origin[0]).powi(2) + (c.b[1] - origin[1]).powi(2)).sqrt();
        let nearest = cells[..n as usize].iter().min_by(|a, b| dist(a).total_cmp(&dist(b))).unwrap();
        let farthest = cells[..n as usize].iter().max_by(|a, b| dist(a).total_cmp(&dist(b))).unwrap();
        assert!(nearest.c[3] > 0.0, "the shards at the origin go first");
        assert_eq!(farthest.c[3], 0.0, "the far corners wait");
        // Same seed, same shards; another seed, other shards.
        assert_eq!(shards(&p, 16.0 / 9.0).1[3], cells[3]);
        assert_ne!(shards(&TransitionParams { seed: 8, ..p }, 16.0 / 9.0).1[3], cells[3]);
    }

    #[test]
    fn grids_stay_within_the_uniform_block() {
        let p = |grid| TransitionParams { kind: TRANSITION_KIND_SHATTER, grid, ..Default::default() };
        assert_eq!(shard_grid(&p([0, 0])), [1, 1]);
        assert_eq!(shard_grid(&p([500, 500])), [128, 1]);
        let [c, r] = shard_grid(&p([16, 16]));
        assert!(c * r <= TRANSITION_MAX_SHARDS);
        let (n, _, _) = shards(&p([16, 16]), 1.0);
        assert!(n <= TRANSITION_MAX_SHARDS);
    }

    #[test]
    fn other_kinds_carry_no_shards() {
        let sprite = TransitionSprite {
            order: 0,
            pad: 0,
            bounds: gpui::Bounds { origin: gpui::point(0.0.into(), 0.0.into()), size: gpui::size(64.0.into(), 32.0.into()) },
            content_mask: gpui::ContentMask::rect(gpui::Bounds { origin: gpui::point(0.0.into(), 0.0.into()), size: gpui::size(64.0.into(), 32.0.into()) }),
            from: tile(0),
            to: tile(1),
            aux: tile(1),
            params: TransitionParams { kind: TRANSITION_KIND_SKETCH, progress: 2.0, intensity: -1.0, ..Default::default() },
        };
        let u = TransitionUniforms::new(&sprite, [64.0, 32.0], [[64.0, 32.0]; 3]);
        assert_eq!(u.counts[1], 0);
        assert_eq!(u.timing[0], 1.0, "progress is clamped");
        assert_eq!(u.timing[2], 0.0, "intensity is clamped");
        assert_eq!(u.mode[0], TRANSITION_KIND_SKETCH);
        assert_eq!(u.inv_from_to, [1.0 / 64.0, 1.0 / 32.0, 1.0 / 64.0, 1.0 / 32.0]);
        let fade = TransitionSprite { params: TransitionParams { kind: TRANSITION_KIND_FADE, ..sprite.params }, ..sprite };
        assert_eq!(TransitionUniforms::new(&fade, [64.0, 32.0], [[64.0, 32.0]; 3]).mode[0], TRANSITION_KIND_FADE);
    }

    fn tile(index: u32) -> AtlasTile {
        AtlasTile {
            texture_id: AtlasTextureId { index, kind: gpui::AtlasTextureKind::Polychrome },
            tile_id: gpui::TileId(index),
            padding: 0,
            bounds: gpui::Bounds { origin: gpui::point(gpui::DevicePixels(0), gpui::DevicePixels(0)), size: gpui::size(gpui::DevicePixels(64), gpui::DevicePixels(32)) },
        }
    }
}

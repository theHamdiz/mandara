//! Mandara patch 18: match instance transport to the adapter's actual limits.

use std::borrow::Cow;

pub(crate) fn transition_shader(texture_instances: bool) -> Cow<'static, str> {
    let shader = crate::transition::TRANSITION_SHADER;
    if !texture_instances {
        return Cow::Borrowed(shader);
    }
    let helper = include_str!("shaders_transition_compat.wgsl");
    let mut source = String::with_capacity(shader.len() + helper.len());
    source.push_str(helper);
    for (index, part) in shader.split("firstTrailingBit(bits)").enumerate() {
        if index != 0 {
            source.push_str("first_shard_bit(bits)");
        }
        source.push_str(part);
    }
    Cow::Owned(source)
}

pub(crate) fn texture_instances(
    backend: wgpu::Backend,
    limits: &wgpu::Limits,
    flags: wgpu::DownlevelFlags,
) -> bool {
    backend == wgpu::Backend::Gl
        && (cfg!(target_family = "wasm")
            || !flags.contains(wgpu::DownlevelFlags::VERTEX_STORAGE)
            || !native_limits(limits).check_limits(limits))
}

pub(crate) fn required_limits(
    backend: wgpu::Backend,
    available: &wgpu::Limits,
    flags: wgpu::DownlevelFlags,
) -> wgpu::Limits {
    let base = if texture_instances(backend, available, flags) {
        wgpu::Limits::downlevel_webgl2_defaults()
    } else {
        return native_limits(available);
    };
    base.using_resolution(available.clone())
        .using_alignment(available.clone())
}

fn native_limits(available: &wgpu::Limits) -> wgpu::Limits {
    wgpu::Limits::downlevel_defaults()
        .using_resolution(available.clone())
        .using_alignment(available.clone())
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;

    #[test]
    fn only_legacy_transitions_replace_the_unavailable_bit_scan() {
        let normal = transition_shader(false);
        assert!(matches!(normal, Cow::Borrowed(_)));
        assert_eq!(normal, crate::transition::TRANSITION_SHADER);
        assert!(normal.contains("firstTrailingBit(bits)"));
        let legacy = transition_shader(true);
        assert!(!legacy.contains("firstTrailingBit("));
        assert!(legacy.contains("first_shard_bit(bits)"));
    }

    #[test]
    fn native_gl_without_vertex_storage_uses_texture_instances() {
        let limited = wgpu::Limits::downlevel_webgl2_defaults();
        assert!(texture_instances(
            wgpu::Backend::Gl,
            &limited,
            wgpu::DownlevelFlags::empty()
        ));
        let full = wgpu::Limits::downlevel_defaults();
        assert!(texture_instances(
            wgpu::Backend::Gl,
            &full,
            wgpu::DownlevelFlags::empty()
        ));
    }

    #[test]
    fn legacy_gl_device_requests_fit_its_available_limits() {
        let available = wgpu::Limits::downlevel_webgl2_defaults();
        let requested =
            required_limits(wgpu::Backend::Gl, &available, wgpu::DownlevelFlags::empty());
        assert!(requested.check_limits(&available));
        assert_eq!(requested.max_storage_buffers_per_shader_stage, 0);
    }

    #[test]
    fn gl_without_compute_support_can_use_the_render_only_transport() {
        let mut available = wgpu::Limits::downlevel_defaults();
        available.max_compute_workgroup_storage_size = 0;
        available.max_compute_invocations_per_workgroup = 0;
        available.max_compute_workgroup_size_x = 0;
        available.max_compute_workgroup_size_y = 0;
        available.max_compute_workgroup_size_z = 0;
        available.max_compute_workgroups_per_dimension = 0;
        assert!(texture_instances(
            wgpu::Backend::Gl,
            &available,
            wgpu::DownlevelFlags::VERTEX_STORAGE
        ));
        assert!(
            required_limits(
                wgpu::Backend::Gl,
                &available,
                wgpu::DownlevelFlags::VERTEX_STORAGE
            )
            .check_limits(&available)
        );
    }

    #[test]
    fn modern_backends_keep_their_existing_limits_and_storage_transport() {
        let available = wgpu::Limits::downlevel_defaults();
        for backend in [
            wgpu::Backend::Gl,
            wgpu::Backend::Vulkan,
            wgpu::Backend::Metal,
            wgpu::Backend::Dx12,
        ] {
            assert!(!texture_instances(
                backend,
                &available,
                wgpu::DownlevelFlags::VERTEX_STORAGE
            ));
            assert_eq!(
                required_limits(backend, &available, wgpu::DownlevelFlags::VERTEX_STORAGE),
                available
            );
        }
    }
}

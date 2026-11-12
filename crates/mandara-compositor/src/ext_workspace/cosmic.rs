//! COSMIC's window protocols, generated from the XML vendored in
//! `protocols/` (pop-os/cosmic-protocols; each file carries its own
//! permissive copyright notice). cosmic-comp offers no wlr foreign-toplevel
//! manager: windows are listed by `ext_foreign_toplevel_list_v1`, and
//! `zcosmic_toplevel_info_v1` adds their state, outputs and ext-workspace
//! membership; `zcosmic_toplevel_manager_v1` activates, closes, minimizes,
//! maximizes, fullscreens and moves them. COSMIC's own workspace protocol
//! is generated only because the toplevel protocols name its handle.

pub mod workspace {
    use wayland_client;
    use wayland_client::protocol::*;
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/cosmic-workspace-unstable-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("protocols/cosmic-workspace-unstable-v1.xml");
}

pub mod info {
    use super::workspace::*;
    use wayland_client;
    use wayland_client::protocol::*;
    use wayland_protocols::ext::foreign_toplevel_list::v1::client::*;
    use wayland_protocols::ext::workspace::v1::client::*;
    pub mod __interfaces {
        use super::super::workspace::__interfaces::*;
        use wayland_client::protocol::__interfaces::*;
        use wayland_protocols::ext::foreign_toplevel_list::v1::client::__interfaces::*;
        use wayland_protocols::ext::workspace::v1::client::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/cosmic-toplevel-info-unstable-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("protocols/cosmic-toplevel-info-unstable-v1.xml");
}

pub mod management {
    use super::info::*;
    use super::workspace::*;
    use wayland_client;
    use wayland_client::protocol::*;
    use wayland_protocols::ext::workspace::v1::client::*;
    pub mod __interfaces {
        use super::super::info::__interfaces::*;
        use super::super::workspace::__interfaces::*;
        use wayland_client::protocol::__interfaces::*;
        use wayland_protocols::ext::workspace::v1::client::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/cosmic-toplevel-management-unstable-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("protocols/cosmic-toplevel-management-unstable-v1.xml");
}

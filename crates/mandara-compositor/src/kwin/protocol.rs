//! KDE's version 4 virtual-desktop protocol adds per-output activation.
//! The released Plasma bindings still carry its older version 2 XML.

use wayland_client;

pub mod __interfaces {
    wayland_scanner::generate_interfaces!("protocols/org-kde-plasma-virtual-desktop.xml");
}

use self::__interfaces::*;
wayland_scanner::generate_client_code!("protocols/org-kde-plasma-virtual-desktop.xml");

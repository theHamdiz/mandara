//! Dev tool: drives the pointer of the session named by `WAYLAND_DISPLAY`
//! through `zwlr_virtual_pointer_v1` (see `mandara_services::vpointer`).
//!
//! ```text
//! cargo run -p mandara-services --example vpointer -- move  X Y
//! cargo run -p mandara-services --example vpointer -- click X Y [left|right|middle]
//! cargo run -p mandara-services --example vpointer -- drag  X1 Y1 X2 Y2
//! cargo run -p mandara-services --example vpointer -- flyby X Y MS   # touch X Y for MS ms, then leave
//! cargo run -p mandara-services --example vpointer -- scroll X Y STEPS # wheel notches (negative = up)
//! ```
//! `MANDARA_LAYOUT=WxH` sets the layout size (default 2560x1440);
//! `VPOINTER_HOLD_MS` / `VPOINTER_DRAG_HOLD_MS` hold the device / the drag.
//! For the headless sandbox use `cargo xtask sandbox pointer …` instead.

use mandara_services::vpointer::{self, Gesture, Options};
use wayland_client::Connection;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let gesture = Gesture::parse(&args)?;
    let conn = Connection::connect_to_env()?;
    vpointer::perform(&conn, &gesture, &Options::from_env())
}

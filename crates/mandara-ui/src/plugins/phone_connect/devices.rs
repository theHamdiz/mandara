//! Borrow the visible device list while keeping the daemon's order.

use mandara_services::phone_connect::Device;

pub(super) fn visible_devices(
    devices: &[Device],
    show_unpaired: bool,
) -> impl Iterator<Item = &Device> + Clone {
    devices
        .iter()
        .filter(move |device| device.paired || show_unpaired)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, paired: bool) -> Device {
        Device {
            id: id.into(),
            name: id.into(),
            kind: "phone".into(),
            reachable: true,
            paired,
            incoming_pair: false,
            battery: None,
            plugins: Vec::new(),
        }
    }

    /// Looking for an empty state must not consume the first visible device.
    #[test]
    fn visible_list_preserves_order_and_first_item_after_peek() {
        let devices = [
            device("unpaired", false),
            device("paired-a", true),
            device("paired-b", true),
        ];
        let mut visible = visible_devices(&devices, false).peekable();
        assert_eq!(visible.peek().unwrap().id, "paired-a");
        assert_eq!(
            visible.map(|device| device.id.as_str()).collect::<Vec<_>>(),
            ["paired-a", "paired-b"]
        );
        assert_eq!(visible_devices(&devices, true).count(), 3);
        assert!(std::ptr::eq(
            visible_devices(&devices, false).next().unwrap(),
            &devices[1]
        ));
        assert!(visible_devices(&devices[..1], false).next().is_none());
        assert!(visible_devices(&[], true).next().is_none());
    }
}

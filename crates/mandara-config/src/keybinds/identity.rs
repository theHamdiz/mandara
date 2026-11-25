//! Source revisions travel as hex strings, avoiding JSON number precision loss.

use serde::{Deserialize as _, Deserializer, Serializer, de::Error as _};

pub(super) fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(&format_args!("{value:016x}"))
}

pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.len() != 16 {
        return Err(D::Error::custom(
            "a shortcut revision is 16 hexadecimal digits",
        ));
    }
    u64::from_str_radix(&value, 16).map_err(D::Error::custom)
}

#[cfg(test)]
mod tests {
    #[test]
    fn source_ids_round_trip_without_losing_large_revision_bits() {
        let id = crate::keybinds::BindingId {
            file: "/tmp/config".into(),
            target: "/tmp/target".into(),
            fingerprint: u64::MAX,
            index: 0,
        };
        let value = serde_json::to_value(&id).unwrap();
        assert_eq!(value["fingerprint"], "ffffffffffffffff");
        assert_eq!(
            serde_json::from_value::<crate::keybinds::BindingId>(value).unwrap(),
            id
        );
    }
}

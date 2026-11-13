//! Attach rollback details without replacing the original write or check error.

use anyhow::Error;
use std::path::Path;

pub(super) fn failed(primary: Error, restore: Error, recovery: Option<&Path>) -> Error {
    let detail = match recovery {
        Some(path) => format!(
            "restoring the previous theme failed: {restore:#}; original settings are saved at {}",
            path.display()
        ),
        None => format!("restoring the previous theme failed: {restore:#}"),
    };
    primary.context(detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn rollback_context_keeps_the_original_error_type_and_recovery_location() {
        let error = failed(
            io::Error::new(
                io::ErrorKind::InvalidData,
                "native check rejected the theme",
            )
            .into(),
            io::Error::new(io::ErrorKind::PermissionDenied, "rollback write denied").into(),
            Some(Path::new("/private/config.mandara-backup")),
        );
        assert_eq!(
            error
                .root_cause()
                .downcast_ref::<io::Error>()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidData
        );
        let message = format!("{error:#}");
        assert!(message.contains("native check rejected the theme"));
        assert!(message.contains("rollback write denied"));
        assert!(message.contains("/private/config.mandara-backup"));
    }
}

#[cfg(test)]
mod tests {
    use crate::contracts::{AppErrorCode, CommandError, SafeParameterValue};
    use std::collections::BTreeMap;

    #[test]
    fn serializes_stable_command_error() {
        assert_eq!(
            serde_json::to_string(&AppErrorCode::StorageUnavailable).unwrap(),
            "\"storageUnavailable\""
        );
        let mut details = BTreeMap::new();
        details.insert(
            "clipboardRemovalCount".into(),
            SafeParameterValue::Number(12.into()),
        );
        let serialized = serde_json::to_value(
            CommandError::new(
                AppErrorCode::InvalidInput,
                "settings.storage.retentionConfirmationRequired",
                details,
                false,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(serialized["details"]["clipboardRemovalCount"].is_number());
        let boolean = serde_json::to_value(SafeParameterValue::Boolean(true)).unwrap();
        assert_eq!(boolean, true);
    }

    #[test]
    fn rejects_unregistered_sensitive_and_wrong_usage_details_before_serialization() {
        for (key, name, value) in [
            ("errors.conflict", "id", "item-1"),
            ("errors.ioFailure", "body", "secret"),
            ("errors.ioFailure", "token", "secret"),
            ("errors.conflict", "entityId", "C:\\Build\\release"),
            ("errors.conflict", "entityId", "\\\\server\\share\\release"),
            ("errors.conflict", "entityId", "/opt/build/release"),
            ("errors.unknown", "entityId", "item-1"),
            ("errors.ioFailure", "entityId", "item-1"),
            ("reminders.agent.status", "taskId", "item-1"),
        ] {
            let mut details = BTreeMap::new();
            details.insert(name.into(), SafeParameterValue::String(value.into()));
            assert!(
                CommandError::new(AppErrorCode::InvalidInput, key, details, false).is_err(),
                "{key}/{name} must be rejected"
            );
        }
        let mut valid = BTreeMap::new();
        valid.insert(
            "entityId".into(),
            SafeParameterValue::String("item-1".into()),
        );
        assert!(CommandError::new(AppErrorCode::Conflict, "errors.conflict", valid, true).is_ok());
    }
}

use crate::contracts::{
    CommandError, MessageParameterContract, MessageUsage, SafeMessageParameters, SafeParameterValue,
};

pub struct NativeMessageCatalog;

impl NativeMessageCatalog {
    pub fn render(
        language: &str,
        message_key: &str,
        parameters: SafeMessageParameters,
    ) -> Result<String, CommandError> {
        let usage = if message_key.starts_with("errors.")
            || message_key.starts_with("settings.")
            || message_key == "onboarding.consentRequired"
        {
            MessageUsage::CommandError
        } else if message_key.starts_with("services.") {
            MessageUsage::ServiceHealth
        } else if message_key.starts_with("reminders.") {
            MessageUsage::ReminderDisplay
        } else {
            MessageUsage::UiDisplay
        };
        MessageParameterContract::validate_for(usage, message_key, &parameters)?;
        let catalog: serde_json::Value = serde_json::from_str(include_str!(
            "../../src/shared/messageCatalog.json"
        ))
        .map_err(|_| {
            CommandError::with_detail(
                crate::contracts::AppErrorCode::IoFailure,
                "errors.ioFailure",
                "reasonCode",
                SafeParameterValue::String("catalogInvalid".into()),
                false,
            )
        })?;
        let mut text = catalog["messages"][message_key][language]
            .as_str()
            .ok_or_else(|| {
                CommandError::with_detail(
                    crate::contracts::AppErrorCode::IoFailure,
                    "errors.ioFailure",
                    "reasonCode",
                    SafeParameterValue::String("catalogMissing".into()),
                    false,
                )
            })?
            .to_string();
        for (name, value) in parameters {
            let raw = match value {
                SafeParameterValue::String(v) => v,
                SafeParameterValue::Number(v) => v.to_string(),
                SafeParameterValue::Boolean(v) => v.to_string(),
            };
            let rendered = match name.as_str() {
                "triggerStatus" => catalog["parameterEnums"]["triggerStatus"][&raw][language]
                    .as_str()
                    .unwrap_or(&raw)
                    .to_string(),
                "metric" => catalog["parameterEnums"]["metric"][&raw][language]
                    .as_str()
                    .unwrap_or(&raw)
                    .to_string(),
                _ => raw,
            };
            text = text.replace(&format!("{{{name}}}"), &rendered);
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use crate::contracts::{MessageParameterContract, SafeParameterValue};
    use crate::message_catalog::NativeMessageCatalog;
    use std::collections::BTreeMap;

    #[test]
    fn registry_native_catalog_and_parameters_are_total() {
        let parsed: serde_json::Value =
            serde_json::from_str(include_str!("../../src/shared/messageCatalog.json")).unwrap();
        let mut catalog_keys = parsed["messages"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        catalog_keys.sort();
        assert_eq!(catalog_keys, MessageParameterContract::message_keys());

        for key in MessageParameterContract::message_keys() {
            let parameters = fixture(&key);
            for language in ["zh-CN", "en-US"] {
                let rendered =
                    NativeMessageCatalog::render(language, &key, parameters.clone()).unwrap();
                assert!(!rendered.contains('{'));
            }
        }

        for language in ["zh-CN", "en-US"] {
            let agent = NativeMessageCatalog::render(
                language,
                "reminders.agent.status",
                fixture("reminders.agent.status"),
            )
            .unwrap();
            assert!(agent.contains("C:\\Build\\release"));
            assert!(agent.contains("\\\\server\\share\\release"));
            let todo = NativeMessageCatalog::render(
                language,
                "reminders.todo.due",
                fixture("reminders.todo.due"),
            )
            .unwrap();
            assert!(todo.contains("/opt/build/release"));
            let projected = NativeMessageCatalog::render(
                language,
                "reminders.agent.status",
                BTreeMap::from([
                    (
                        "agentName".into(),
                        SafeParameterValue::String("Codex".into()),
                    ),
                    (
                        "environment".into(),
                        SafeParameterValue::String("windows".into()),
                    ),
                    ("taskId".into(), SafeParameterValue::String("task-1".into())),
                    (
                        "taskTitle".into(),
                        SafeParameterValue::String("task-1".into()),
                    ),
                    (
                        "triggerStatus".into(),
                        SafeParameterValue::String("failed".into()),
                    ),
                ]),
            )
            .unwrap();
            assert!(projected.matches("task-1").count() >= 2);
        }
        let too_long = BTreeMap::from([(
            "todoTitle".into(),
            SafeParameterValue::String("x".repeat(513)),
        )]);
        assert!(NativeMessageCatalog::render("zh-CN", "reminders.todo.due", too_long).is_err());
        let control_text = BTreeMap::from([(
            "todoTitle".into(),
            SafeParameterValue::String("bad\ntext".into()),
        )]);
        assert!(NativeMessageCatalog::render("zh-CN", "reminders.todo.due", control_text).is_err());
        let error_path = BTreeMap::from([(
            "entityId".into(),
            SafeParameterValue::String("C:\\Build\\release".into()),
        )]);
        assert!(NativeMessageCatalog::render("zh-CN", "errors.conflict", error_path).is_err());
        let sensitive_service_pair =
            BTreeMap::from([("body".into(), SafeParameterValue::String("secret".into()))]);
        assert!(
            NativeMessageCatalog::render("zh-CN", "services.healthy", sensitive_service_pair)
                .is_err()
        );
    }

    fn fixture(key: &str) -> BTreeMap<String, SafeParameterValue> {
        let string = |value: &str| SafeParameterValue::String(value.into());
        match key {
            "settings.storage.retentionConfirmationRequired" => BTreeMap::from([
                (
                    "clipboardRemovalCount".into(),
                    SafeParameterValue::Number(12.into()),
                ),
                (
                    "notificationRemovalCount".into(),
                    SafeParameterValue::Number(4.into()),
                ),
            ]),
            "services.healthy" => BTreeMap::from([("serviceId".into(), string("clipboard"))]),
            "services.degraded" | "services.blocked" | "services.offline" => BTreeMap::from([
                ("serviceId".into(), string("clipboard")),
                ("reasonCode".into(), string("locked")),
            ]),
            "services.clipboard.locked" | "home.agents.more" => {
                BTreeMap::from([("count".into(), SafeParameterValue::Number(2.into()))])
            }
            "reminders.agent.status" => BTreeMap::from([
                ("agentName".into(), string("Codex")),
                ("environment".into(), string("windows")),
                ("taskId".into(), string("C:\\Build\\release")),
                ("taskTitle".into(), string("\\\\server\\share\\release")),
                ("triggerStatus".into(), string("failed")),
            ]),
            "reminders.todo.due" => {
                BTreeMap::from([("todoTitle".into(), string("/opt/build/release"))])
            }
            "reminders.monitor.threshold" => BTreeMap::from([
                ("metric".into(), string("networkReceive")),
                ("currentValue".into(), SafeParameterValue::Number(12.into())),
                (
                    "thresholdValue".into(),
                    SafeParameterValue::Number(10.into()),
                ),
            ]),
            "errors.notFound" | "errors.conflict" => {
                BTreeMap::from([("entityId".into(), string("item-1"))])
            }
            "errors.sourceUnavailable" | "errors.platformUnsupported" => BTreeMap::from([
                ("serviceId".into(), string("service")),
                ("reasonCode".into(), string("failed")),
            ]),
            "errors.integrationUnsupported" | "errors.integrationConfigInvalid" => {
                BTreeMap::from([
                    ("agentName".into(), string("Codex")),
                    ("environment".into(), string("windows")),
                    ("reasonCode".into(), string("failed")),
                ])
            }
            "errors.integrationNotInstalled" => BTreeMap::from([
                ("agentName".into(), string("Codex")),
                ("environment".into(), string("windows")),
            ]),
            key if key.starts_with("errors.") && key != "errors.serviceStopping" => {
                BTreeMap::from([("reasonCode".into(), string("failed"))])
            }
            _ => BTreeMap::new(),
        }
    }
}

use crate::contracts::{AppErrorCode, CommandError, SafeParameterValue};
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigFormat {
    JsonHooks,
    HermesYaml,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedHookFragment {
    pub event: String,
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeAction {
    Install,
    Uninstall,
}

pub fn merge_config(
    bytes: &[u8],
    format: ConfigFormat,
    owned: &[OwnedHookFragment],
    action: MergeAction,
) -> Result<(Vec<u8>, bool), CommandError> {
    match format {
        ConfigFormat::JsonHooks => merge_json(bytes, owned, action),
        ConfigFormat::HermesYaml => merge_yaml(bytes, owned, action),
    }
}

pub fn inspect_config(
    bytes: &[u8],
    format: ConfigFormat,
    owned: &[OwnedHookFragment],
) -> Result<bool, CommandError> {
    match format {
        ConfigFormat::JsonHooks => {
            let root: Value = serde_json::from_slice(bytes).map_err(|_| invalid("parse"))?;
            Ok(owned.iter().all(|fragment| json_has(&root, fragment)))
        }
        ConfigFormat::HermesYaml => {
            let root: serde_yaml::Value =
                serde_yaml::from_slice(bytes).map_err(|_| invalid("parse"))?;
            Ok(owned.iter().all(|fragment| yaml_has(&root, fragment)))
        }
    }
}

fn merge_json(
    bytes: &[u8],
    owned: &[OwnedHookFragment],
    action: MergeAction,
) -> Result<(Vec<u8>, bool), CommandError> {
    let mut root: Value = serde_json::from_slice(bytes).map_err(|_| invalid("parse"))?;
    let object = root.as_object_mut().ok_or_else(|| invalid("jsonRoot"))?;
    let hooks = match action {
        MergeAction::Install => object
            .entry("hooks")
            .or_insert_with(|| Value::Object(Map::new())),
        MergeAction::Uninstall => match object.get_mut("hooks") {
            Some(hooks) => hooks,
            None => return Ok((bytes.to_vec(), false)),
        },
    };
    let hooks = hooks.as_object_mut().ok_or_else(|| invalid("jsonHooks"))?;
    let mut changed = false;
    for fragment in owned {
        let event = match action {
            MergeAction::Install => hooks
                .entry(fragment.event.clone())
                .or_insert_with(|| Value::Array(Vec::new())),
            MergeAction::Uninstall => match hooks.get_mut(&fragment.event) {
                Some(event) => event,
                None => continue,
            },
        };
        let groups = event.as_array_mut().ok_or_else(|| invalid("jsonEvent"))?;
        if matches!(action, MergeAction::Install) {
            if !json_has_event(groups, fragment) {
                let mut repaired = false;
                for group in groups.iter_mut() {
                    let mut repaired_group = false;
                    if let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                        for handler in handlers.iter_mut() {
                            if handler.get("command").and_then(Value::as_str).is_some_and(
                                |command| same_managed_script(command, &fragment.command),
                            ) {
                                *handler = serde_json::json!({"type":"command","command":fragment.command});
                                repaired = true;
                                repaired_group = true;
                            }
                        }
                    }
                    if repaired_group {
                        group["matcher"] = Value::String("*".into());
                    }
                }
                if !repaired {
                    groups.push(serde_json::json!({"matcher":"*","hooks":[{"type":"command","command":fragment.command}]}));
                }
                changed = true;
            }
        } else {
            for group in groups.iter_mut() {
                if let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                    let before = handlers.len();
                    handlers.retain(|handler| {
                        handler.get("command").and_then(Value::as_str)
                            != Some(fragment.command.as_str())
                    });
                    changed |= before != handlers.len();
                }
            }
        }
    }
    Ok((
        if changed {
            serde_json::to_vec_pretty(&root).map_err(|_| invalid("serialize"))?
        } else {
            bytes.to_vec()
        },
        changed,
    ))
}

fn json_has(root: &Value, fragment: &OwnedHookFragment) -> bool {
    root.get("hooks")
        .and_then(Value::as_object)
        .and_then(|hooks| hooks.get(&fragment.event))
        .and_then(Value::as_array)
        .is_some_and(|groups| json_has_event(groups, fragment))
}
fn json_has_event(groups: &[Value], fragment: &OwnedHookFragment) -> bool {
    groups.iter().any(|group| {
        group.get("matcher").and_then(Value::as_str) == Some("*")
            && group
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|handlers| {
                    handlers.iter().any(|handler| {
                        handler.get("command").and_then(Value::as_str)
                            == Some(fragment.command.as_str())
                            && handler.get("type").and_then(Value::as_str) == Some("command")
                            && handler.as_object().is_some_and(|object| object.len() == 2)
                    })
                })
    })
}

fn merge_yaml(
    bytes: &[u8],
    owned: &[OwnedHookFragment],
    action: MergeAction,
) -> Result<(Vec<u8>, bool), CommandError> {
    let mut root: serde_yaml::Value =
        serde_yaml::from_slice(bytes).map_err(|_| invalid("parse"))?;
    let mapping = root.as_mapping_mut().ok_or_else(|| invalid("yamlRoot"))?;
    let hooks_key = serde_yaml::Value::String("hooks".into());
    let hooks = match action {
        MergeAction::Install => mapping
            .entry(hooks_key.clone())
            .or_insert_with(|| serde_yaml::Value::Mapping(Default::default())),
        MergeAction::Uninstall => match mapping.get_mut(&hooks_key) {
            Some(hooks) => hooks,
            None => return Ok((bytes.to_vec(), false)),
        },
    };
    let hooks = hooks.as_mapping_mut().ok_or_else(|| invalid("yamlHooks"))?;
    let mut changed = false;
    for fragment in owned {
        let event_key = serde_yaml::Value::String(fragment.event.clone());
        let entries = match action {
            MergeAction::Install => hooks
                .entry(event_key.clone())
                .or_insert_with(|| serde_yaml::Value::Sequence(Vec::new())),
            MergeAction::Uninstall => match hooks.get_mut(&event_key) {
                Some(entries) => entries,
                None => continue,
            },
        };
        let entries = entries
            .as_sequence_mut()
            .ok_or_else(|| invalid("yamlEvent"))?;
        if matches!(action, MergeAction::Install) {
            if !yaml_entries_has(entries, fragment) {
                let mut repaired = false;
                for entry in entries.iter_mut() {
                    if entry
                        .as_mapping()
                        .and_then(|m| m.get(serde_yaml::Value::String("command".into())))
                        .and_then(serde_yaml::Value::as_str)
                        .is_some_and(|command| same_managed_script(command, &fragment.command))
                    {
                        *entry = serde_yaml::to_value(
                            serde_json::json!({"command": fragment.command, "timeout": 5}),
                        )
                        .map_err(|_| invalid("serialize"))?;
                        repaired = true;
                    }
                }
                if !repaired {
                    entries.push(
                        serde_yaml::to_value(
                            serde_json::json!({"command": fragment.command, "timeout": 5}),
                        )
                        .map_err(|_| invalid("serialize"))?,
                    );
                }
                changed = true;
            }
        } else {
            let before = entries.len();
            entries.retain(|entry| {
                entry
                    .as_mapping()
                    .and_then(|m| m.get(serde_yaml::Value::String("command".into())))
                    .and_then(serde_yaml::Value::as_str)
                    != Some(fragment.command.as_str())
            });
            changed |= before != entries.len();
        }
    }
    Ok((
        if changed {
            serde_yaml::to_string(&root)
                .map_err(|_| invalid("serialize"))?
                .into_bytes()
        } else {
            bytes.to_vec()
        },
        changed,
    ))
}
fn yaml_has(root: &serde_yaml::Value, fragment: &OwnedHookFragment) -> bool {
    root.as_mapping()
        .and_then(|m| m.get(serde_yaml::Value::String("hooks".into())))
        .and_then(serde_yaml::Value::as_mapping)
        .and_then(|m| m.get(serde_yaml::Value::String(fragment.event.clone())))
        .and_then(serde_yaml::Value::as_sequence)
        .is_some_and(|entries| yaml_entries_has(entries, fragment))
}
fn yaml_entries_has(entries: &[serde_yaml::Value], fragment: &OwnedHookFragment) -> bool {
    entries.iter().any(|entry| {
        entry.as_mapping().is_some_and(|mapping| mapping.len() == 2)
            && entry
                .as_mapping()
                .and_then(|m| m.get(serde_yaml::Value::String("command".into())))
                .and_then(serde_yaml::Value::as_str)
                == Some(fragment.command.as_str())
            && entry
                .as_mapping()
                .and_then(|m| m.get(serde_yaml::Value::String("timeout".into())))
                .and_then(serde_yaml::Value::as_i64)
                == Some(5)
    })
}
fn same_managed_script(candidate: &str, owned: &str) -> bool {
    same_aisland_managed_script(candidate, owned)
}

pub(crate) fn same_aisland_managed_script(candidate: &str, owned: &str) -> bool {
    managed_script_identity(candidate)
        .zip(managed_script_identity(owned))
        .is_some_and(|(candidate, owned)| {
            candidate == owned || migrated_app_data_script_identity(candidate) == owned
        })
}

fn migrated_app_data_script_identity(identity: &str) -> String {
    identity
        .replace(
            "\\com.aisland\\agent-hooks\\",
            "\\com.aisland.app\\agent-hooks\\",
        )
        .replace("/com.aisland/agent-hooks/", "/com.aisland.app/agent-hooks/")
}

fn managed_script_identity(command: &str) -> Option<&str> {
    [".ps1", ".sh"].into_iter().find_map(|extension| {
        command
            .find(extension)
            .map(|index| &command[..index + extension.len()])
    })
}
fn invalid(reason: &str) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::IntegrationConfigInvalid,
        "errors.invalidInput",
        "reasonCode",
        SafeParameterValue::String(reason.into()),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hooks() -> Vec<OwnedHookFragment> {
        vec![OwnedHookFragment {
            event: "Stop".into(),
            command: "C:/aisland/codex.ps1".into(),
        }]
    }
    #[test]
    fn json_merge_is_idempotent_and_uninstall_keeps_empty_containers() {
        let source =
            br#"{"unknown":7,"hooks":{"Stop":[],"Other":[{"hooks":[{"command":"user"}]}]}}"#;
        let (installed, changed) = merge_config(
            source,
            ConfigFormat::JsonHooks,
            &hooks(),
            MergeAction::Install,
        )
        .unwrap();
        assert!(changed);
        assert!(inspect_config(&installed, ConfigFormat::JsonHooks, &hooks()).unwrap());
        assert_eq!(
            merge_config(
                &installed,
                ConfigFormat::JsonHooks,
                &hooks(),
                MergeAction::Install
            )
            .unwrap(),
            (installed.clone(), false)
        );
        let (removed, changed) = merge_config(
            &installed,
            ConfigFormat::JsonHooks,
            &hooks(),
            MergeAction::Uninstall,
        )
        .unwrap();
        assert!(changed);
        let root: Value = serde_json::from_slice(&removed).unwrap();
        assert!(root["hooks"]["Stop"].is_array());
        assert_eq!(root["unknown"], 7);
    }
    #[test]
    fn yaml_merge_preserves_unrelated_values_and_exact_removal() {
        let source = b"root: &keep value\nhooks:\n  post_llm_call:\n    - command: user\n      timeout: 8\noutbound: keep\n";
        let owned = vec![OwnedHookFragment {
            event: "post_llm_call".into(),
            command: "/home/a/.local/share/aisland/h.sh".into(),
        }];
        let (installed, _) = merge_config(
            source,
            ConfigFormat::HermesYaml,
            &owned,
            MergeAction::Install,
        )
        .unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_slice(&installed).unwrap();
        assert_eq!(parsed["root"], "value");
        assert_eq!(parsed["outbound"], "keep");
        let (removed, _) = merge_config(
            &installed,
            ConfigFormat::HermesYaml,
            &owned,
            MergeAction::Uninstall,
        )
        .unwrap();
        assert!(
            serde_yaml::from_slice::<serde_yaml::Value>(&removed).unwrap()["hooks"]
                ["post_llm_call"]
                .is_sequence()
        );
    }

    #[test]
    fn uninstall_never_creates_missing_json_or_yaml_containers() {
        let owned = hooks();
        let json = br#"{"unrelated":true}"#;
        assert_eq!(
            merge_config(
                json,
                ConfigFormat::JsonHooks,
                &owned,
                MergeAction::Uninstall
            )
            .unwrap(),
            (json.to_vec(), false)
        );
        let yaml = b"unrelated: true\n";
        assert_eq!(
            merge_config(
                yaml,
                ConfigFormat::HermesYaml,
                &owned,
                MergeAction::Uninstall
            )
            .unwrap(),
            (yaml.to_vec(), false)
        );

        let mixed_owned = vec![
            owned[0].clone(),
            OwnedHookFragment {
                event: "Missing".into(),
                command: "C:/aisland/missing.ps1".into(),
            },
        ];
        let mixed = br#"{"hooks":{"Stop":[{"matcher":"*","hooks":[{"type":"command","command":"C:/aisland/codex.ps1"}]}]}}"#;
        let (removed, changed) = merge_config(
            mixed,
            ConfigFormat::JsonHooks,
            &mixed_owned,
            MergeAction::Uninstall,
        )
        .unwrap();
        assert!(changed);
        let parsed = serde_json::from_slice::<Value>(&removed).unwrap();
        assert!(parsed["hooks"].get("Missing").is_none());
    }

    #[test]
    fn repair_replaces_a_drifted_owned_handler_without_removing_user_handlers() {
        let owned = hooks();
        let source = br#"{"hooks":{"Stop":[{"matcher":"*","hooks":[{"type":"command","command":"C:/aisland/codex.ps1","extra":"drift"},{"type":"command","command":"user"}]}]}}"#;
        let (repaired, changed) = merge_config(
            source,
            ConfigFormat::JsonHooks,
            &owned,
            MergeAction::Install,
        )
        .unwrap();
        assert!(changed);
        let handlers =
            &serde_json::from_slice::<Value>(&repaired).unwrap()["hooks"]["Stop"][0]["hooks"];
        assert_eq!(handlers.as_array().unwrap().len(), 2);
        assert_eq!(
            handlers[0],
            serde_json::json!({"type":"command","command":"C:/aisland/codex.ps1"})
        );
        assert_eq!(handlers[1]["command"], "user");
    }

    #[test]
    fn repair_recognizes_owned_script_with_drifted_arguments_and_repairs_in_place() {
        let owned = hooks();
        let source = br#"{"hooks":{"Stop":[{"matcher":"drifted","hooks":[{"type":"command","command":"C:/aisland/codex.ps1 --wrong-args","extra":"drift"},{"type":"command","command":"user"}]}]}}"#;

        let (repaired, changed) = merge_config(
            source,
            ConfigFormat::JsonHooks,
            &owned,
            MergeAction::Install,
        )
        .unwrap();

        assert!(changed);
        let parsed = serde_json::from_slice::<Value>(&repaired).unwrap();
        let group = &parsed["hooks"]["Stop"][0];
        assert_eq!(group["matcher"], "*");
        let handlers = group["hooks"].as_array().unwrap();
        assert_eq!(handlers.len(), 2);
        assert_eq!(
            handlers[0],
            serde_json::json!({"type":"command","command":"C:/aisland/codex.ps1"})
        );
        assert_eq!(handlers[1]["command"], "user");
    }

    #[test]
    fn repair_migrates_the_legacy_aisland_app_data_path_in_place() {
        let current = r#"powershell.exe -NoProfile -File "C:\Users\Alice\AppData\Roaming\com.aisland.app\agent-hooks\hermes-windows.ps1" -OutputPath current"#;
        let legacy = r#"powershell.exe -NoProfile -File "C:\Users\Alice\AppData\Roaming\com.aisland\agent-hooks\hermes-windows.ps1" -OutputPath legacy"#;
        let owned = vec![OwnedHookFragment {
            event: "post_llm_call".into(),
            command: current.into(),
        }];

        let yaml_source = format!(
            "hooks:\n  post_llm_call:\n    - command: '{}'\n      timeout: 5\n    - command: user-command\n      timeout: 8\n",
            legacy
        );
        let (yaml_repaired, changed) = merge_config(
            yaml_source.as_bytes(),
            ConfigFormat::HermesYaml,
            &owned,
            MergeAction::Install,
        )
        .unwrap();
        assert!(changed);
        let yaml_text = String::from_utf8(yaml_repaired).unwrap();
        assert!(yaml_text.contains(current));
        assert!(!yaml_text.contains(legacy));
        assert!(yaml_text.contains("user-command"));

        let json_source = serde_json::to_vec(&serde_json::json!({
            "hooks": {
                "post_llm_call": [{
                    "matcher": "*",
                    "hooks": [
                        {"type": "command", "command": legacy},
                        {"type": "command", "command": "user-command"}
                    ]
                }]
            }
        }))
        .unwrap();
        let (json_repaired, changed) = merge_config(
            &json_source,
            ConfigFormat::JsonHooks,
            &owned,
            MergeAction::Install,
        )
        .unwrap();
        assert!(changed);
        let json: serde_json::Value = serde_json::from_slice(&json_repaired).unwrap();
        let commands = json["hooks"]["post_llm_call"][0]["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|handler| handler["command"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(commands, vec![current, "user-command"]);
    }
}

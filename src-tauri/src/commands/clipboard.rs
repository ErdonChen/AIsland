use crate::contracts::{
    ClearResult, ClipboardAssetPayload, ClipboardItem, CommandError, DeleteResult,
    ListClipboardItemsInput,
};
use crate::domain::clipboard::ClipboardListKind;
use crate::services::{clipboard_service::ClipboardService, AppServices};
use std::sync::Arc;
use uuid::Uuid;

#[tauri::command(rename = "listClipboardItems", rename_all = "camelCase")]
pub fn list_clipboard_items(
    query: String,
    content_kind: ClipboardListKind,
    limit: u32,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Vec<ClipboardItem>, CommandError> {
    list_clipboard_items_with_service(query, content_kind, limit, services.clipboard.as_ref())
}

#[tauri::command(rename = "copyClipboardItem", rename_all = "camelCase")]
pub fn copy_clipboard_item(
    id: Uuid,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<ClipboardItem, CommandError> {
    copy_clipboard_item_with_service(id, services.clipboard.as_ref())
}

#[tauri::command(rename = "setClipboardPinned", rename_all = "camelCase")]
pub fn set_clipboard_pinned(
    id: Uuid,
    pinned: bool,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<ClipboardItem, CommandError> {
    set_clipboard_pinned_with_service(id, pinned, services.clipboard.as_ref(), now_millis())
}

#[tauri::command(rename = "deleteClipboardItem", rename_all = "camelCase")]
pub fn delete_clipboard_item(
    id: Uuid,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<DeleteResult, CommandError> {
    delete_clipboard_item_with_service(id, services.clipboard.as_ref(), now_millis())
}

#[tauri::command(rename = "clearClipboardHistory", rename_all = "camelCase")]
pub fn clear_clipboard_history(
    keep_pinned: bool,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<ClearResult, CommandError> {
    clear_clipboard_history_with_service(keep_pinned, services.clipboard.as_ref(), now_millis())
}

#[tauri::command(rename = "getClipboardAsset", rename_all = "camelCase")]
pub fn get_clipboard_asset(
    asset_id: Uuid,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<ClipboardAssetPayload, CommandError> {
    get_clipboard_asset_with_service(asset_id, services.clipboard.as_ref())
}

fn list_clipboard_items_with_service(
    query: String,
    content_kind: ClipboardListKind,
    limit: u32,
    service: &ClipboardService,
) -> Result<Vec<ClipboardItem>, CommandError> {
    service.list_items(ListClipboardItemsInput {
        query,
        content_kind,
        limit: i64::from(limit),
    })
}

fn copy_clipboard_item_with_service(
    id: Uuid,
    service: &ClipboardService,
) -> Result<ClipboardItem, CommandError> {
    service.copy_item(id)
}

fn set_clipboard_pinned_with_service(
    id: Uuid,
    pinned: bool,
    service: &ClipboardService,
    changed_at: i64,
) -> Result<ClipboardItem, CommandError> {
    service.set_pinned(id, pinned, changed_at)
}

fn delete_clipboard_item_with_service(
    id: Uuid,
    service: &ClipboardService,
    changed_at: i64,
) -> Result<DeleteResult, CommandError> {
    service.delete_item(id, changed_at)
}

fn clear_clipboard_history_with_service(
    keep_pinned: bool,
    service: &ClipboardService,
    changed_at: i64,
) -> Result<ClearResult, CommandError> {
    service.clear_history(keep_pinned, changed_at)
}

fn get_clipboard_asset_with_service(
    asset_id: Uuid,
    service: &ClipboardService,
) -> Result<ClipboardAssetPayload, CommandError> {
    service.asset_payload(asset_id)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        clear_clipboard_history, clear_clipboard_history_with_service, copy_clipboard_item,
        copy_clipboard_item_with_service, delete_clipboard_item,
        delete_clipboard_item_with_service, get_clipboard_asset, get_clipboard_asset_with_service,
        list_clipboard_items, list_clipboard_items_with_service, set_clipboard_pinned,
        set_clipboard_pinned_with_service,
    };
    use crate::contracts::{
        AppErrorCode, ClipboardAssetMimeType, ClipboardContentKindFilter, CommandError,
        SafeMessageParameters,
    };
    use crate::domain::clipboard::{BootstrapClipboardRetentionPolicy, CapturedClipboardContent};
    use crate::repositories::clipboard::ClipboardRepository;
    use crate::repositories::diagnostics::DiagnosticsRepository;
    use crate::repositories::service_health::ServiceHealthRepository;
    use crate::services::clipboard_assets::ClipboardAssetStore;
    use crate::services::clipboard_listener::{
        ClipboardReadError, ClipboardSource, ClipboardSourceFactory,
    };
    use crate::services::clipboard_service::ClipboardService;
    use crate::services::EventEmitterPort;
    use crate::storage::Storage;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, PartialEq, Eq)]
    enum WriteStep {
        Text(String),
        Rgba {
            width: u32,
            height: u32,
            bytes: Vec<u8>,
        },
    }

    struct NoopSource {
        writes: Arc<Mutex<Vec<WriteStep>>>,
    }
    impl ClipboardSource for NoopSource {
        fn read(
            &mut self,
        ) -> Result<Option<crate::domain::clipboard::CapturedClipboardContent>, ClipboardReadError>
        {
            Ok(None)
        }
        fn write_text(&mut self, value: &str) -> Result<(), crate::contracts::CommandError> {
            self.writes
                .lock()
                .unwrap()
                .push(WriteStep::Text(value.into()));
            Ok(())
        }
        fn write_png(&mut self, png: &[u8]) -> Result<(), crate::contracts::CommandError> {
            let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
                .unwrap()
                .to_rgba8();
            self.writes.lock().unwrap().push(WriteStep::Rgba {
                width: image.width(),
                height: image.height(),
                bytes: image.into_raw(),
            });
            Ok(())
        }
    }

    struct NoopFactory {
        writes: Arc<Mutex<Vec<WriteStep>>>,
        opens: Arc<AtomicUsize>,
    }
    impl ClipboardSourceFactory for NoopFactory {
        fn open(&self) -> Result<Box<dyn ClipboardSource>, crate::contracts::CommandError> {
            self.opens.fetch_add(1, Ordering::AcqRel);
            Ok(Box::new(NoopSource {
                writes: self.writes.clone(),
            }))
        }
    }

    struct RecordingEmitter {
        repository: ClipboardRepository,
        events: Mutex<Vec<(&'static str, serde_json::Value)>>,
        expect_deleted: Mutex<Option<String>>,
        expect_asset_present: Mutex<Option<std::path::PathBuf>>,
        reject: AtomicBool,
    }
    impl EventEmitterPort for RecordingEmitter {
        fn emit(
            &self,
            name: &'static str,
            payload: serde_json::Value,
        ) -> Result<(), crate::contracts::CommandError> {
            if let Some(id) = self.expect_deleted.lock().unwrap().take() {
                assert_eq!(payload["entityId"], id);
                assert_eq!(
                    self.repository
                        .get(uuid::Uuid::parse_str(&id).unwrap())
                        .unwrap_err()
                        .code,
                    AppErrorCode::NotFound,
                    "delete event preceded commit"
                );
            }
            if let Some(path) = self.expect_asset_present.lock().unwrap().take() {
                assert!(
                    std::fs::exists(path).unwrap(),
                    "owned cleanup preceded the wake hint"
                );
            }
            if self.reject.load(Ordering::Acquire) {
                return Err(CommandError {
                    code: AppErrorCode::SourceUnavailable,
                    message_key: "errors.sourceUnavailable".into(),
                    details: SafeMessageParameters::new(),
                    retryable: false,
                });
            }
            self.events.lock().unwrap().push((name, payload));
            Ok(())
        }
    }

    struct Fixture {
        service: ClipboardService,
        repository: ClipboardRepository,
        diagnostics: DiagnosticsRepository,
        emitter: Arc<RecordingEmitter>,
        writes: Arc<Mutex<Vec<WriteStep>>>,
        opens: Arc<AtomicUsize>,
        asset_root: std::path::PathBuf,
    }

    fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap().keep();
        let storage = Arc::new(Storage::open(&directory).unwrap());
        let repository =
            ClipboardRepository::new(storage.clone(), Arc::new(BootstrapClipboardRetentionPolicy));
        let diagnostics = DiagnosticsRepository::new(storage.clone());
        let writes = Arc::new(Mutex::new(Vec::new()));
        let opens = Arc::new(AtomicUsize::new(0));
        let emitter = Arc::new(RecordingEmitter {
            repository: repository.clone(),
            events: Mutex::new(Vec::new()),
            expect_deleted: Mutex::new(None),
            expect_asset_present: Mutex::new(None),
            reject: AtomicBool::new(false),
        });
        let service = ClipboardService::new(
            repository.clone(),
            ClipboardAssetStore::new(&directory).unwrap(),
            Arc::new(NoopFactory {
                writes: writes.clone(),
                opens: opens.clone(),
            }),
            ServiceHealthRepository::new(storage.clone()),
            diagnostics.clone(),
            emitter.clone(),
        );
        Fixture {
            service,
            repository,
            diagnostics,
            emitter,
            writes,
            opens,
            asset_root: directory.join("clipboard-assets"),
        }
    }

    fn capture_text(fixture: &Fixture, text: &str, now: i64) -> crate::contracts::ClipboardItem {
        fixture
            .service
            .capture(
                Some("notepad.exe".into()),
                CapturedClipboardContent::Text {
                    text: text.into(),
                    sha256: String::new(),
                    byte_size: text.len() as u64,
                },
                now,
            )
            .unwrap()
            .item
    }

    fn capture_image(
        fixture: &Fixture,
        rgba: &[u8],
        now: i64,
    ) -> (crate::contracts::ClipboardItem, Vec<u8>) {
        let png = crate::services::clipboard_listener::encode_rgba_png(1, 1, rgba).unwrap();
        let item = fixture
            .service
            .capture(
                Some("paint.exe".into()),
                CapturedClipboardContent::Image {
                    byte_size: png.len() as u64,
                    png: png.clone(),
                    sha256: String::new(),
                    width: 1,
                    height: 1,
                },
                now,
            )
            .unwrap()
            .item;
        (item, png)
    }

    #[test]
    fn list_command_uses_the_real_clipboard_repository_boundary() {
        let fixture = fixture();
        assert_eq!(
            list_clipboard_items_with_service(
                String::new(),
                ClipboardContentKindFilter::All,
                500,
                &fixture.service,
            )
            .unwrap(),
            Vec::new()
        );
    }

    #[test]
    fn canonical_manifest_locks_six_commands_and_matching_source_attributes() {
        let _ = list_clipboard_items;
        let _ = copy_clipboard_item;
        let _ = set_clipboard_pinned;
        let _ = delete_clipboard_item;
        let _ = clear_clipboard_history;
        let _ = get_clipboard_asset;
        assert_eq!(
            crate::commands::CLIPBOARD_COMMAND_NAMES,
            [
                "listClipboardItems",
                "copyClipboardItem",
                "setClipboardPinned",
                "deleteClipboardItem",
                "clearClipboardHistory",
                "getClipboardAsset",
            ]
        );
        let source = include_str!("clipboard.rs");
        for wire_name in crate::commands::CLIPBOARD_COMMAND_NAMES {
            let attribute =
                format!("#[tauri::command(rename = \"{wire_name}\", rename_all = \"camelCase\")]");
            assert_eq!(
                source.matches(&attribute).count(),
                1,
                "wire drift for {wire_name}"
            );
        }
    }

    #[test]
    fn copy_text_and_image_then_listener_echo_updates_the_same_ids() {
        let fixture = fixture();
        let text = capture_text(&fixture, "copy me", 10);
        copy_clipboard_item_with_service(
            uuid::Uuid::parse_str(&text.id).unwrap(),
            &fixture.service,
        )
        .unwrap();
        assert_eq!(capture_text(&fixture, "copy me", 20).id, text.id);

        let rgba = [7, 8, 9, 128];
        let (image, png) = capture_image(&fixture, &rgba, 30);
        copy_clipboard_item_with_service(
            uuid::Uuid::parse_str(&image.id).unwrap(),
            &fixture.service,
        )
        .unwrap();
        let echoed_image = fixture
            .service
            .capture(
                None,
                CapturedClipboardContent::Image {
                    byte_size: png.len() as u64,
                    png,
                    sha256: String::new(),
                    width: 1,
                    height: 1,
                },
                40,
            )
            .unwrap()
            .item;
        assert_eq!(echoed_image.id, image.id);
        assert_eq!(
            *fixture.writes.lock().unwrap(),
            vec![
                WriteStep::Text("copy me".into()),
                WriteStep::Rgba {
                    width: 1,
                    height: 1,
                    bytes: rgba.to_vec(),
                }
            ]
        );
        assert_eq!(
            fixture
                .service
                .list_items(crate::contracts::ListClipboardItemsInput {
                    query: String::new(),
                    content_kind: ClipboardContentKindFilter::All,
                    limit: 500,
                })
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn invalid_or_missing_image_asset_returns_io_failure_and_keeps_the_row() {
        for missing in [false, true] {
            let fixture = fixture();
            let (item, _) = capture_image(&fixture, &[1, 2, 3, 255], 50);
            let asset_id = uuid::Uuid::parse_str(item.asset_id.as_deref().unwrap()).unwrap();
            let asset = fixture.repository.get_asset(asset_id).unwrap();
            let path = fixture.asset_root.join(asset.asset_name);
            if missing {
                std::fs::remove_file(path).unwrap();
            } else {
                std::fs::write(path, b"not a png").unwrap();
            }
            let error = copy_clipboard_item_with_service(
                uuid::Uuid::parse_str(&item.id).unwrap(),
                &fixture.service,
            )
            .unwrap_err();
            assert_eq!(error.code, AppErrorCode::IoFailure);
            let asset_error =
                get_clipboard_asset_with_service(asset_id, &fixture.service).unwrap_err();
            assert_eq!(asset_error.code, AppErrorCode::IoFailure);
            assert_eq!(
                fixture
                    .repository
                    .get(uuid::Uuid::parse_str(&item.id).unwrap())
                    .unwrap(),
                item
            );
            assert!(fixture.writes.lock().unwrap().is_empty());
            assert_eq!(fixture.opens.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn asset_payload_is_padded_base64_and_delete_commits_then_cleans() {
        use base64::Engine;

        let fixture = fixture();
        let (item, png) = capture_image(&fixture, &[4, 5, 6, 255], 60);
        let asset_id = uuid::Uuid::parse_str(item.asset_id.as_deref().unwrap()).unwrap();
        let asset = fixture.repository.get_asset(asset_id).unwrap();
        let asset_path = fixture.asset_root.join(&asset.asset_name);
        let payload = get_clipboard_asset_with_service(asset_id, &fixture.service).unwrap();
        assert_eq!(payload.mime_type, ClipboardAssetMimeType::ImagePng);
        assert_eq!(payload.base64.len() % 4, 0);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(payload.base64)
                .unwrap(),
            png
        );

        *fixture.emitter.expect_deleted.lock().unwrap() = Some(item.id.clone());
        *fixture.emitter.expect_asset_present.lock().unwrap() = Some(asset_path.clone());
        let result = delete_clipboard_item_with_service(
            uuid::Uuid::parse_str(&item.id).unwrap(),
            &fixture.service,
            61,
        )
        .unwrap();
        assert_eq!(result.id, item.id);
        assert!(!std::fs::exists(asset_path).unwrap());
        assert_eq!(
            fixture.emitter.events.lock().unwrap().as_slice(),
            [(
                "clipboardChanged",
                serde_json::json!({ "entityId": item.id, "changedAt": 61 })
            )]
        );
    }

    #[test]
    fn clear_respects_pins_returns_exact_count_and_emits_one_operation_hint() {
        let fixture = fixture();
        let pinned = capture_text(&fixture, "keep", 70);
        set_clipboard_pinned_with_service(
            uuid::Uuid::parse_str(&pinned.id).unwrap(),
            true,
            &fixture.service,
            71,
        )
        .unwrap();
        let (removed, _) = capture_image(&fixture, &[8, 9, 10, 255], 72);
        let removed_asset = fixture
            .repository
            .get_asset(uuid::Uuid::parse_str(removed.asset_id.as_deref().unwrap()).unwrap())
            .unwrap();
        fixture.emitter.events.lock().unwrap().clear();
        let removed_path = fixture.asset_root.join(&removed_asset.asset_name);
        *fixture.emitter.expect_asset_present.lock().unwrap() = Some(removed_path.clone());

        let result = clear_clipboard_history_with_service(true, &fixture.service, 73).unwrap();
        assert_eq!(result.removed_count, 1);
        let remaining = fixture
            .service
            .list_items(crate::contracts::ListClipboardItemsInput {
                query: String::new(),
                content_kind: ClipboardContentKindFilter::All,
                limit: 500,
            })
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, pinned.id);
        assert!(!std::fs::exists(removed_path).unwrap());
        let events = fixture.emitter.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "clipboardChanged");
        assert_eq!(events[0].1["changedAt"], 73);
        let operation_id = events[0].1["entityId"].as_str().unwrap();
        assert_eq!(
            uuid::Uuid::parse_str(operation_id)
                .unwrap()
                .get_version_num(),
            4
        );
    }

    #[test]
    fn action_emit_failure_preserves_commit_and_records_only_entity_id() {
        let fixture = fixture();
        let item = capture_text(&fixture, "pin", 80);
        fixture.emitter.reject.store(true, Ordering::Release);
        let pinned = set_clipboard_pinned_with_service(
            uuid::Uuid::parse_str(&item.id).unwrap(),
            true,
            &fixture.service,
            81,
        )
        .unwrap();
        assert!(pinned.pinned);
        let diagnostics = fixture.diagnostics.list(10).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "events.clipboardChangedEmitFailed");
        assert_eq!(
            diagnostics[0].parameters,
            std::collections::BTreeMap::from([(
                "entityId".into(),
                crate::contracts::SafeParameterValue::String(item.id),
            )])
        );
    }
}

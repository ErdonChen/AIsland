use crate::contracts::{
    AppErrorCode, CommandError, PendingReminderNavigation, ReminderAlertGroup, ReminderDelivery,
    ReminderSound, ReminderSourceKind, SafeMessageParameters,
};
use crate::repositories::reminders::ReminderRepository;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::{Arc, Weak};
use tauri::{Emitter, Manager};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReminderChannelName {
    Sound,
    Toast,
    Window,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelFailure {
    pub code: &'static str,
}

#[async_trait::async_trait]
pub trait ReminderChannel: Send + Sync {
    fn name(&self) -> ReminderChannelName;
    async fn deliver(&self, delivery: &ReminderDelivery) -> Result<(), ChannelFailure>;

    async fn deliver_cancellable(
        &self,
        delivery: &ReminderDelivery,
        _: DeliveryCancellation,
    ) -> Result<(), ChannelFailure> {
        self.deliver(delivery).await
    }
}

#[derive(Clone, Default)]
pub struct DeliveryCancellation(Arc<AtomicBool>);

impl DeliveryCancellation {
    fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

pub trait NotificationHealthPort: Send + Sync {
    fn record(&self, healthy: bool, reason_code: Option<&str>);
}

pub struct RepositoryNotificationHealthPort(
    pub crate::repositories::service_health::ServiceHealthRepository,
);

impl NotificationHealthPort for RepositoryNotificationHealthPort {
    fn record(&self, healthy: bool, reason_code: Option<&str>) {
        let _ = self.0.upsert(&crate::contracts::ServiceHealthSnapshot {
            service_id: "notifications".into(),
            state: if healthy {
                crate::contracts::ServiceHealthState::Healthy
            } else {
                crate::contracts::ServiceHealthState::Degraded
            },
            message_key: if healthy {
                "services.healthy".into()
            } else {
                "services.degraded".into()
            },
            parameters: if healthy {
                SafeMessageParameters::from([(
                    "serviceId".into(),
                    crate::contracts::SafeParameterValue::String("notifications".into()),
                )])
            } else {
                SafeMessageParameters::from([
                    (
                        "serviceId".into(),
                        crate::contracts::SafeParameterValue::String("notifications".into()),
                    ),
                    (
                        "reasonCode".into(),
                        crate::contracts::SafeParameterValue::String(
                            reason_code.unwrap_or("toastShowFailed").into(),
                        ),
                    ),
                ])
            },
            checked_at: crate::services::now_millis(),
        });
    }
}

pub trait ToastActivationHandler: Send + Sync {
    fn activate(&self, delivery_id: &str);
}

#[derive(Default)]
pub struct ToastActivationPort {
    // The native callback must never keep AppServices alive after shutdown.  AppServices owns the
    // router for its accepted lifetime; this port is only a non-owning dispatch endpoint.
    handler: Mutex<Option<Weak<dyn ToastActivationHandler>>>,
}

impl ToastActivationPort {
    pub fn install_once<T: ToastActivationHandler + 'static>(&self, handler: &Arc<T>) -> bool {
        let mut slot = self.handler.lock().expect("toast activation lock poisoned");
        if slot.is_some() {
            return false;
        }
        let handler: Arc<dyn ToastActivationHandler> = handler.clone();
        *slot = Some(Arc::downgrade(&handler));
        true
    }

    pub fn uninstall(&self) {
        self.handler
            .lock()
            .expect("toast activation lock poisoned")
            .take();
    }

    pub fn dispatch_uuid_only(&self, argument: &str) {
        if uuid::Uuid::parse_str(argument).is_err() {
            return;
        }
        if let Some(handler) = self
            .handler
            .lock()
            .expect("toast activation lock poisoned")
            .as_ref()
            .and_then(Weak::upgrade)
        {
            handler.activate(argument);
        }
    }
}

// The in-process WinRT event and the COM local-server callback intentionally share this one
// narrow parser.  Cold start therefore cannot bypass the durable UUID activation workflow.
fn dispatch_cold_start_activation(port: &ToastActivationPort, argument: &str) {
    port.dispatch_uuid_only(argument);
}

fn local_server_command(executable: &str) -> String {
    format!("\"{executable}\"")
}

#[cfg(windows)]
const TOAST_ACTIVATOR_CLSID: windows::core::GUID =
    windows::core::GUID::from_u128(0x8a3824c5_5a7d_4d59_bf04_2c19c43b6f9a);

// Windows invokes this COM object when a toast is clicked while the process is not running.
// Do not put command parsing here: it must take the exact same UUID-only route as a warm click.
#[cfg(windows)]
#[windows::core::implement(windows::Win32::UI::Notifications::INotificationActivationCallback)]
struct ColdStartNotificationActivator {
    activation: Arc<ToastActivationPort>,
}

#[cfg(windows)]
impl windows::Win32::UI::Notifications::INotificationActivationCallback_Impl
    for ColdStartNotificationActivator_Impl
{
    fn Activate(
        &self,
        _: &windows::core::PCWSTR,
        invoked_args: &windows::core::PCWSTR,
        _: *const windows::Win32::UI::Notifications::NOTIFICATION_USER_INPUT_DATA,
        _: u32,
    ) -> windows::core::Result<()> {
        // PCWSTR originates at COM; conversion owns a Rust string before the callback returns.
        let argument = unsafe { invoked_args.to_string()? };
        dispatch_cold_start_activation(&self.activation, &argument);
        Ok(())
    }
}

#[cfg(windows)]
#[windows::core::implement(windows::Win32::System::Com::IClassFactory)]
struct ColdStartNotificationClassFactory {
    activation: Arc<ToastActivationPort>,
}

#[cfg(windows)]
impl windows::Win32::System::Com::IClassFactory_Impl for ColdStartNotificationClassFactory_Impl {
    fn CreateInstance(
        &self,
        _: windows::core::Ref<'_, windows::core::IUnknown>,
        riid: *const windows::core::GUID,
        object: *mut *mut core::ffi::c_void,
    ) -> windows::core::Result<()> {
        use windows::core::Interface;
        use windows::Win32::UI::Notifications::INotificationActivationCallback;
        let callback: INotificationActivationCallback = ColdStartNotificationActivator {
            activation: self.activation.clone(),
        }
        .into();
        unsafe { callback.query(riid, object).ok() }
    }

    fn LockServer(&self, _: windows::core::BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}

#[cfg(windows)]
pub struct ColdStartActivationRegistration {
    // The test registration is intentionally inert.  Production registrations own the COM
    // cookie and revoke it during shutdown.
    cookie: Option<u32>,
}

#[cfg(windows)]
impl Drop for ColdStartActivationRegistration {
    fn drop(&mut self) {
        if let Some(cookie) = self.cookie {
            unsafe {
                let _ = windows::Win32::System::Com::CoRevokeClassObject(cookie);
            }
        }
    }
}

#[cfg(windows)]
impl ColdStartActivationRegistration {
    #[cfg(test)]
    fn for_test() -> Self {
        Self { cookie: None }
    }
}

#[cfg(windows)]
trait ColdStartRegistrationPort: Send + Sync {
    fn current_executable(&self) -> Result<String, ChannelFailure>;
    fn write_local_server32(&self, command: &str) -> Result<(), ChannelFailure>;
    fn install_shortcut(
        &self,
        executable: &str,
        aumid: &str,
        clsid: &str,
    ) -> Result<(), ChannelFailure>;
    fn register_class(
        &self,
        activation: Arc<ToastActivationPort>,
    ) -> Result<ColdStartActivationRegistration, ChannelFailure>;
}

#[cfg(windows)]
const TOAST_AUMID: &str = "com.aisland.app";
#[cfg(windows)]
const TOAST_ACTIVATOR_CLSID_TEXT: &str = "{8A3824C5-5A7D-4D59-BF04-2C19C43B6F9A}";

#[cfg(windows)]
fn register_windows_cold_start_activation_with(
    port: &dyn ColdStartRegistrationPort,
    activation: Arc<ToastActivationPort>,
) -> Result<ColdStartActivationRegistration, ChannelFailure> {
    let executable = port.current_executable()?;
    port.write_local_server32(&local_server_command(&executable))?;
    port.install_shortcut(&executable, TOAST_AUMID, TOAST_ACTIVATOR_CLSID_TEXT)?;
    port.register_class(activation)
}

#[cfg(windows)]
struct WindowsColdStartRegistrationPort;

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

#[cfg(windows)]
struct OwnedPropVariant(windows::Win32::System::Com::StructuredStorage::PROPVARIANT);

#[cfg(windows)]
impl OwnedPropVariant {
    fn string(value: &str) -> Result<Self, ChannelFailure> {
        use windows::core::PWSTR;
        use windows::Win32::System::Com::CoTaskMemAlloc;
        use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
        use windows::Win32::System::Variant::VT_LPWSTR;

        let value = wide(value);
        let byte_count =
            value
                .len()
                .checked_mul(std::mem::size_of::<u16>())
                .ok_or(ChannelFailure {
                    code: "toastRegistrationFailed",
                })?;
        let allocation = unsafe { CoTaskMemAlloc(byte_count) };
        if allocation.is_null() {
            return Err(ChannelFailure {
                code: "toastRegistrationFailed",
            });
        }

        unsafe {
            std::ptr::copy_nonoverlapping(value.as_ptr(), allocation.cast::<u16>(), value.len());
        }
        let mut variant = PROPVARIANT::default();
        unsafe {
            (*variant.Anonymous.Anonymous).vt = VT_LPWSTR;
            (*variant.Anonymous.Anonymous).Anonymous.pwszVal = PWSTR(allocation.cast());
        }
        Ok(Self(variant))
    }
}

#[cfg(windows)]
impl Drop for OwnedPropVariant {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::System::Com::StructuredStorage::PropVariantClear(&mut self.0);
        }
    }
}

#[cfg(windows)]
impl ColdStartRegistrationPort for WindowsColdStartRegistrationPort {
    fn current_executable(&self) -> Result<String, ChannelFailure> {
        std::env::current_exe()
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|_| ChannelFailure {
                code: "toastRegistrationFailed",
            })
    }

    fn write_local_server32(&self, command: &str) -> Result<(), ChannelFailure> {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::ERROR_SUCCESS;
        use windows::Win32::System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
            REG_OPTION_NON_VOLATILE, REG_SZ,
        };

        let key_path =
            wide("Software\\Classes\\CLSID\\{8A3824C5-5A7D-4D59-BF04-2C19C43B6F9A}\\LocalServer32");
        let mut key = HKEY::default();
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(key_path.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut key,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(ChannelFailure {
                code: "toastRegistrationFailed",
            });
        }
        let command = wide(command);
        let bytes =
            unsafe { std::slice::from_raw_parts(command.as_ptr().cast::<u8>(), command.len() * 2) };
        let status = unsafe { RegSetValueExW(key, PCWSTR::null(), None, REG_SZ, Some(bytes)) };
        unsafe {
            let _ = RegCloseKey(key);
        }
        if status != ERROR_SUCCESS {
            return Err(ChannelFailure {
                code: "toastRegistrationFailed",
            });
        }
        Ok(())
    }

    fn install_shortcut(
        &self,
        executable: &str,
        aumid: &str,
        clsid: &str,
    ) -> Result<(), ChannelFailure> {
        use windows::core::{Interface, PCWSTR};
        use windows::Win32::Storage::EnhancedStorage::{
            PKEY_AppUserModel_ID, PKEY_AppUserModel_ToastActivatorCLSID,
        };
        use windows::Win32::System::Com::StructuredStorage::InitPropVariantFromCLSID;
        use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
        use windows::Win32::UI::Shell::IShellLinkW;
        use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

        let app_data = std::env::var("APPDATA").map_err(|_| ChannelFailure {
            code: "toastRegistrationFailed",
        })?;
        let folder =
            std::path::Path::new(&app_data).join("Microsoft\\Windows\\Start Menu\\Programs");
        std::fs::create_dir_all(&folder).map_err(|_| ChannelFailure {
            code: "toastRegistrationFailed",
        })?;
        let shortcut = folder.join("AIsland.lnk");
        let exe_wide = wide(executable);
        let shortcut_wide = wide(&shortcut.to_string_lossy());
        let shell_link = windows::core::GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
        let link: IShellLinkW = unsafe {
            CoCreateInstance(&shell_link, None, CLSCTX_INPROC_SERVER)
        }
        .map_err(|_| ChannelFailure {
            code: "toastRegistrationFailed",
        })?;
        unsafe { link.SetPath(PCWSTR(exe_wide.as_ptr())) }.map_err(|_| ChannelFailure {
            code: "toastRegistrationFailed",
        })?;
        let store: IPropertyStore = link.cast().map_err(|_| ChannelFailure {
            code: "toastRegistrationFailed",
        })?;
        let aumid_value = OwnedPropVariant::string(aumid)?;
        // The port boundary keeps the CLSID observable to the test seam; the Windows adapter
        // rejects any accidental mismatch rather than registering a shortcut for another class.
        if clsid != TOAST_ACTIVATOR_CLSID_TEXT {
            return Err(ChannelFailure {
                code: "toastRegistrationFailed",
            });
        }
        unsafe {
            store
                .SetValue(&PKEY_AppUserModel_ID, &aumid_value.0)
                .map_err(|_| ChannelFailure {
                    code: "toastRegistrationFailed",
                })?;
            let clsid_value = OwnedPropVariant(
                InitPropVariantFromCLSID(&TOAST_ACTIVATOR_CLSID).map_err(|_| ChannelFailure {
                    code: "toastRegistrationFailed",
                })?,
            );
            store
                .SetValue(&PKEY_AppUserModel_ToastActivatorCLSID, &clsid_value.0)
                .map_err(|_| ChannelFailure {
                    code: "toastRegistrationFailed",
                })?;
            store.Commit().map_err(|_| ChannelFailure {
                code: "toastRegistrationFailed",
            })?;
        }
        let persist: IPersistFile = link.cast().map_err(|_| ChannelFailure {
            code: "toastRegistrationFailed",
        })?;
        unsafe { persist.Save(PCWSTR(shortcut_wide.as_ptr()), true) }.map_err(|_| ChannelFailure {
            code: "toastRegistrationFailed",
        })
    }

    fn register_class(
        &self,
        activation: Arc<ToastActivationPort>,
    ) -> Result<ColdStartActivationRegistration, ChannelFailure> {
        use windows::Win32::System::Com::{
            CoRegisterClassObject, IClassFactory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE,
        };
        let factory: IClassFactory = ColdStartNotificationClassFactory { activation }.into();
        let cookie = unsafe {
            CoRegisterClassObject(
                &TOAST_ACTIVATOR_CLSID,
                &factory,
                CLSCTX_LOCAL_SERVER,
                REGCLS_MULTIPLEUSE,
            )
        }
        .map_err(|_| ChannelFailure {
            code: "toastRegistrationFailed",
        })?;
        Ok(ColdStartActivationRegistration {
            cookie: Some(cookie),
        })
    }
}

#[cfg(windows)]
pub fn register_windows_cold_start_activation(
    activation: Arc<ToastActivationPort>,
) -> Result<ColdStartActivationRegistration, ChannelFailure> {
    // Tauri calls setup on its already-STA UI thread.  Do not increment COM apartment state here:
    // this registration owns only the class cookie and has no matching apartment lifetime.
    let port = WindowsColdStartRegistrationPort;
    // This helper is also the test seam: a ready registration is returned only after the
    // registry command, AUMID/CLSID shortcut, and COM class registration all succeed.
    register_windows_cold_start_activation_with(&port, activation)
}

pub struct UnavailableReminderChannel(pub ReminderChannelName);

#[async_trait::async_trait]
impl ReminderChannel for UnavailableReminderChannel {
    fn name(&self) -> ReminderChannelName {
        self.0
    }
    async fn deliver(&self, _: &ReminderDelivery) -> Result<(), ChannelFailure> {
        Err(ChannelFailure {
            code: match self.0 {
                ReminderChannelName::Sound => "soundDeviceUnavailable",
                ReminderChannelName::Toast => "toastUnavailable",
                ReminderChannelName::Window => "alertWindowUnavailable",
            },
        })
    }
}

pub trait MainWindowPort: Send + Sync {
    fn show_main(&self) -> Result<(), ChannelFailure>;
}

pub trait ReminderNavigationEmitter: Send + Sync {
    fn emit_navigation(&self, navigation: &PendingReminderNavigation)
        -> Result<(), ChannelFailure>;
}

pub trait AlertWindowPort: Send + Sync {
    fn emit_reminder(&self, group: &ReminderAlertGroup) -> Result<(), ChannelFailure>;
    fn show(&self) -> Result<(), ChannelFailure>;
    fn focus_after_user_activation(&self) -> Result<(), ChannelFailure>;
}

pub struct AlertWindowReminderChannel {
    alert_window: Arc<dyn AlertWindowPort>,
    repository: ReminderRepository,
}

/// Native audio is deliberately a narrow port: no decoder/device error, filename, or path is
/// allowed to cross the channel persistence boundary.
pub struct RodioReminderChannel {
    local_player: Arc<dyn LocalSoundPort>,
}

const MAX_LOCAL_FILE_PLAYBACK: std::time::Duration = std::time::Duration::from_secs(30);

trait LocalSoundPort: Send + Sync {
    fn play(
        &self,
        sound: &ReminderSound,
        deadline: std::time::Instant,
        cancellation: &DeliveryCancellation,
    ) -> Result<(), ChannelFailure>;
}

struct SystemLocalSoundPort;

fn check_local_sound_stop(
    deadline: std::time::Instant,
    cancellation: &DeliveryCancellation,
) -> Result<(), ChannelFailure> {
    if cancellation.is_cancelled() {
        return Err(ChannelFailure {
            code: "soundPlayCancelled",
        });
    }
    if std::time::Instant::now() >= deadline {
        return Err(ChannelFailure {
            code: "soundPlayTimedOut",
        });
    }
    Ok(())
}

fn wait_for_local_sound_completion(
    maximum: std::time::Duration,
    complete: impl FnMut() -> bool,
    stop: impl FnMut(),
) -> Result<(), ChannelFailure> {
    wait_for_local_sound_completion_until(
        std::time::Instant::now() + maximum,
        &DeliveryCancellation::default(),
        complete,
        stop,
    )
}

fn wait_for_local_sound_completion_until(
    deadline: std::time::Instant,
    cancellation: &DeliveryCancellation,
    mut complete: impl FnMut() -> bool,
    mut stop: impl FnMut(),
) -> Result<(), ChannelFailure> {
    loop {
        if complete() {
            return Ok(());
        }
        if cancellation.is_cancelled() {
            stop();
            return Err(ChannelFailure {
                code: "soundPlayCancelled",
            });
        }
        if std::time::Instant::now() >= deadline {
            stop();
            return Err(ChannelFailure {
                code: "soundPlayTimedOut",
            });
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

impl LocalSoundPort for SystemLocalSoundPort {
    fn play(
        &self,
        sound: &ReminderSound,
        deadline: std::time::Instant,
        cancellation: &DeliveryCancellation,
    ) -> Result<(), ChannelFailure> {
        check_local_sound_stop(deadline, cancellation)?;
        match sound {
            ReminderSound::None => Ok(()),
            ReminderSound::Builtin { .. } => play_system_notification_sound(),
            ReminderSound::LocalFile { canonical_path } => {
                let file = std::fs::File::open(canonical_path).map_err(|_| ChannelFailure {
                    code: "soundOpenFailed",
                })?;
                check_local_sound_stop(deadline, cancellation)?;
                let stream =
                    rodio::DeviceSinkBuilder::open_default_sink().map_err(|_| ChannelFailure {
                        code: "soundDeviceUnavailable",
                    })?;
                check_local_sound_stop(deadline, cancellation)?;
                let sink =
                    rodio::play(stream.mixer(), std::io::BufReader::new(file)).map_err(|_| {
                        ChannelFailure {
                            code: "soundDecodeFailed",
                        }
                    })?;
                check_local_sound_stop(deadline, cancellation)?;
                let _stream = stream;
                wait_for_local_sound_completion_until(
                    deadline,
                    cancellation,
                    || sink.empty(),
                    || sink.stop(),
                )
            }
        }
    }
}

pub struct WindowsToastReminderChannel {
    aumid: String,
    activation: Arc<ToastActivationPort>,
    health: Arc<dyn NotificationHealthPort>,
    registration: Arc<ToastRegistrationState>,
}

#[derive(Default)]
pub struct ToastRegistrationState {
    ready: AtomicBool,
}

impl ToastRegistrationState {
    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::Release);
    }

    pub fn mark_unavailable(&self) {
        self.ready.store(false, Ordering::Release);
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
}

impl WindowsToastReminderChannel {
    pub fn new(activation: Arc<ToastActivationPort>) -> Self {
        // Stable packaged identity: Tag/Group are the durable delivery UUID so retries replace
        // the prior toast instead of stacking a second notification.
        Self {
            aumid: "com.aisland.app".into(),
            activation,
            health: Arc::new(NoopNotificationHealthPort),
            registration: Arc::new(ToastRegistrationState::default()),
        }
    }

    pub fn with_health(
        activation: Arc<ToastActivationPort>,
        health: Arc<dyn NotificationHealthPort>,
    ) -> Self {
        Self::with_health_and_registration(
            activation,
            health,
            Arc::new(ToastRegistrationState::default()),
        )
    }

    pub fn with_health_and_registration(
        activation: Arc<ToastActivationPort>,
        health: Arc<dyn NotificationHealthPort>,
        registration: Arc<ToastRegistrationState>,
    ) -> Self {
        Self {
            aumid: "com.aisland.app".into(),
            activation,
            health,
            registration,
        }
    }
}

struct NoopNotificationHealthPort;
impl NotificationHealthPort for NoopNotificationHealthPort {
    fn record(&self, _: bool, _: Option<&str>) {}
}

#[cfg(windows)]
fn show_windows_toast(
    aumid: &str,
    delivery: &ReminderDelivery,
    activation: Arc<ToastActivationPort>,
) -> Result<(), ChannelFailure> {
    use windows::core::{Interface, HSTRING};
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{
        ToastActivatedEventArgs, ToastNotification, ToastNotificationManager,
    };

    let id = HSTRING::from(&delivery.id);
    let xml = XmlDocument::new().map_err(|_| ChannelFailure {
        code: "toastUnavailable",
    })?;
    let language = match crate::native_locale() {
        Ok(crate::contracts::Locale::ZhCn) => "zh-CN",
        Ok(crate::contracts::Locale::EnUs) => "en-US",
        Err(_) => "zh-CN",
    };
    let (title, body) = toast_text(language, delivery)?;
    // The activation payload is UUID-only; display text is from the committed catalog locale.
    xml.LoadXml(&HSTRING::from(toast_xml(delivery, &title, &body)))
        .map_err(|_| ChannelFailure {
            code: "toastShowFailed",
        })?;
    let toast = ToastNotification::CreateToastNotification(&xml).map_err(|_| ChannelFailure {
        code: "toastShowFailed",
    })?;
    let on_activated = TypedEventHandler::<ToastNotification, windows::core::IInspectable>::new(
        move |_, arguments| {
            if let Some(arguments) = arguments.as_ref() {
                if let Ok(arguments) = arguments.cast::<ToastActivatedEventArgs>() {
                    if let Ok(argument) = arguments.Arguments() {
                        activation.dispatch_uuid_only(&argument.to_string());
                    }
                }
            }
            Ok(())
        },
    );
    toast.Activated(&on_activated).map_err(|_| ChannelFailure {
        code: "toastShowFailed",
    })?;
    toast.SetTag(&id).map_err(|_| ChannelFailure {
        code: "toastShowFailed",
    })?;
    toast.SetGroup(&id).map_err(|_| ChannelFailure {
        code: "toastShowFailed",
    })?;
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(aumid))
        .map_err(|_| ChannelFailure {
            code: "toastUnavailable",
        })?;
    notifier.Show(&toast).map_err(|_| ChannelFailure {
        code: "toastShowFailed",
    })
}

fn toast_text(
    language: &str,
    delivery: &ReminderDelivery,
) -> Result<(String, String), ChannelFailure> {
    let title = match language {
        "zh-CN" => "AIsland 提醒",
        "en-US" => "AIsland reminder",
        _ => {
            return Err(ChannelFailure {
                code: "toastShowFailed",
            })
        }
    };
    let body = crate::message_catalog::NativeMessageCatalog::render(
        language,
        &delivery.message_key,
        delivery.message_parameters.clone(),
    )
    .map_err(|_| ChannelFailure {
        code: "toastShowFailed",
    })?;
    Ok((title.into(), body))
}

fn toast_xml(delivery: &ReminderDelivery, title: &str, body: &str) -> String {
    // Windows Toasts must not choose a sound independently: ReminderSound is the sole channel
    // owner, preventing `None` from sounding and builtin sound from playing twice.
    format!(
        "<toast launch=\"{}\"><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual><audio silent=\"true\"/></toast>",
        delivery.id,
        xml_escape(title),
        xml_escape(body),
    )
}

fn notification_default_sound_alias() -> &'static str {
    "Notification.Default"
}

#[cfg(windows)]
fn notification_default_sound_flags() -> u32 {
    use windows::Win32::Media::Audio::{SND_ALIAS, SND_ASYNC, SND_SYSTEM};

    SND_ALIAS.0 | SND_ASYNC.0 | SND_SYSTEM.0
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(not(windows))]
fn show_windows_toast(
    _: &str,
    _: &ReminderDelivery,
    _: Arc<ToastActivationPort>,
) -> Result<(), ChannelFailure> {
    Err(ChannelFailure {
        code: "toastUnavailable",
    })
}

#[async_trait::async_trait]
impl ReminderChannel for WindowsToastReminderChannel {
    fn name(&self) -> ReminderChannelName {
        ReminderChannelName::Toast
    }

    async fn deliver(&self, delivery: &ReminderDelivery) -> Result<(), ChannelFailure> {
        if !self.registration.is_ready() {
            let error = ChannelFailure {
                code: "toastRegistrationFailed",
            };
            self.health.record(false, Some(error.code));
            return Err(error);
        }
        let result = show_windows_toast(&self.aumid, delivery, self.activation.clone());
        match &result {
            Ok(()) => self.health.record(true, None),
            Err(error) => self.health.record(false, Some(error.code)),
        }
        result
    }
}

impl RodioReminderChannel {
    fn permits() -> &'static Arc<tokio::sync::Semaphore> {
        static PERMITS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
        PERMITS.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
    }

    #[cfg(test)]
    fn with_local_player(local_player: Arc<dyn LocalSoundPort>) -> Self {
        Self { local_player }
    }
}

impl Default for RodioReminderChannel {
    fn default() -> Self {
        Self {
            local_player: Arc::new(SystemLocalSoundPort),
        }
    }
}

#[async_trait::async_trait]
impl ReminderChannel for RodioReminderChannel {
    fn name(&self) -> ReminderChannelName {
        ReminderChannelName::Sound
    }

    async fn deliver(&self, delivery: &ReminderDelivery) -> Result<(), ChannelFailure> {
        self.deliver_cancellable(delivery, DeliveryCancellation::default())
            .await
    }

    async fn deliver_cancellable(
        &self,
        delivery: &ReminderDelivery,
        cancellation: DeliveryCancellation,
    ) -> Result<(), ChannelFailure> {
        let permit = Self::permits()
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ChannelFailure {
                code: "soundPlayFailed",
            })?;
        let sound = delivery.sound.clone();
        // The budget begins before the blocking task is submitted, not after opening/decoding.
        let deadline = std::time::Instant::now() + MAX_LOCAL_FILE_PLAYBACK;
        let player = self.local_player.clone();
        let blocking_cancellation = cancellation.clone();
        let mut join = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            player.play(&sound, deadline, &blocking_cancellation)
        });
        // Cancellation/deadline signals the owned player and then joins it.  The handle is never
        // dropped, so a runtime shutdown cannot leave an orphaned spawn_blocking playback task.
        tokio::select! {
            result = &mut join => result.map_err(|_| ChannelFailure { code: "soundPlayFailed" })?,
            () = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                cancellation.cancel();
                join.await.map_err(|_| ChannelFailure { code: "soundPlayFailed" })?
            }
        }
    }
}

#[cfg(windows)]
fn play_system_notification_sound() -> Result<(), ChannelFailure> {
    use windows::core::PCWSTR;
    use windows::Win32::Media::Audio::{PlaySoundW, SND_FLAGS};

    let alias: Vec<u16> = notification_default_sound_alias()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    if unsafe {
        PlaySoundW(
            PCWSTR(alias.as_ptr()),
            None,
            SND_FLAGS(notification_default_sound_flags()),
        )
    }
    .as_bool()
    {
        Ok(())
    } else {
        Err(ChannelFailure {
            code: "soundDeviceUnavailable",
        })
    }
}

#[cfg(not(windows))]
fn play_system_notification_sound() -> Result<(), ChannelFailure> {
    Err(ChannelFailure {
        code: "soundDeviceUnavailable",
    })
}

impl AlertWindowReminderChannel {
    pub fn new(alert_window: Arc<dyn AlertWindowPort>, repository: ReminderRepository) -> Self {
        Self {
            alert_window,
            repository,
        }
    }

    pub fn user_activated(&self) -> Result<(), ChannelFailure> {
        self.alert_window.focus_after_user_activation()
    }
}

#[async_trait::async_trait]
impl ReminderChannel for AlertWindowReminderChannel {
    fn name(&self) -> ReminderChannelName {
        ReminderChannelName::Window
    }

    async fn deliver(&self, _delivery: &ReminderDelivery) -> Result<(), ChannelFailure> {
        // Standalone alert windows are retired. Agent activity belongs to its Agent card;
        // system and AIsland history remain available through Toast and Notification Center.
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToastActivationResult {
    NavigateHome(PendingReminderNavigation),
    UnknownContext(PendingReminderNavigation),
}

/// Application-owned activation routing.  The native Toast callback is deliberately limited to
/// a UUID, while this router performs the durable write before touching either UI port.
pub struct ToastActivationRouter {
    service: Arc<ReminderChannelService>,
    main_window: Arc<dyn MainWindowPort>,
    emitter: Arc<dyn ReminderNavigationEmitter>,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl ToastActivationRouter {
    pub fn new(
        service: Arc<ReminderChannelService>,
        main_window: Arc<dyn MainWindowPort>,
        emitter: Arc<dyn ReminderNavigationEmitter>,
        now: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            service,
            main_window,
            emitter,
            now,
        }
    }
}

impl ToastActivationHandler for ToastActivationRouter {
    fn activate(&self, delivery_id: &str) {
        if let Err(error) = self.service.handle_toast_activation_with(
            delivery_id,
            (self.now)(),
            self.main_window.as_ref(),
            self.emitter.as_ref(),
        ) {
            log::error!(
                target: "aisland::reminders",
                "toast_activation status=failed error={}",
                error.message_key
            );
        }
    }
}

pub struct ReminderChannelService {
    sound: Arc<dyn ReminderChannel>,
    toast: Arc<dyn ReminderChannel>,
    window: Arc<dyn ReminderChannel>,
    repository: ReminderRepository,
    result_persistence: Arc<dyn ChannelResultPersistence>,
    wake_tx: tokio::sync::mpsc::Sender<(String, i64)>,
    wake_overflow: AtomicBool,
    persistence_halted: AtomicBool,
    attempted: Mutex<HashSet<(String, i64, &'static str)>>,
}

pub struct ReminderChannelWorker {
    service: Arc<ReminderChannelService>,
    wake_rx: tokio::sync::mpsc::Receiver<(String, i64)>,
}

trait ChannelResultPersistence: Send + Sync {
    fn persist(
        &self,
        delivery: &ReminderDelivery,
        channel: &str,
        succeeded: bool,
        error_code: Option<&str>,
        now: i64,
    ) -> Result<(), CommandError>;
}

struct RepositoryChannelResultPersistence {
    repository: ReminderRepository,
}

impl ChannelResultPersistence for RepositoryChannelResultPersistence {
    fn persist(
        &self,
        delivery: &ReminderDelivery,
        channel: &str,
        succeeded: bool,
        error_code: Option<&str>,
        now: i64,
    ) -> Result<(), CommandError> {
        self.repository.persist_channel_result(
            &delivery.id,
            delivery.dispatch_seq,
            channel,
            succeeded,
            error_code,
            now,
        )
    }
}

pub const REMINDER_NAVIGATION_REQUESTED: &str = "reminderNavigationRequested";
pub const REMINDER_ALERT_READY: &str = "reminderAlertReady";

pub struct TauriAlertWindowPort {
    app: tauri::AppHandle,
}

impl TauriAlertWindowPort {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl AlertWindowPort for TauriAlertWindowPort {
    fn emit_reminder(&self, group: &ReminderAlertGroup) -> Result<(), ChannelFailure> {
        self.app
            .emit_to("reminder-alert", REMINDER_ALERT_READY, group)
            .map_err(|_| ChannelFailure {
                code: "alertEmitFailed",
            })
    }

    fn show(&self) -> Result<(), ChannelFailure> {
        self.app
            .get_webview_window("reminder-alert")
            .ok_or(ChannelFailure {
                code: "alertWindowUnavailable",
            })?
            .show()
            .map_err(|_| ChannelFailure {
                code: "alertShowFailed",
            })
    }

    fn focus_after_user_activation(&self) -> Result<(), ChannelFailure> {
        self.app
            .get_webview_window("reminder-alert")
            .ok_or(ChannelFailure {
                code: "alertWindowUnavailable",
            })?
            .set_focus()
            .map_err(|_| ChannelFailure {
                code: "alertFocusFailed",
            })
    }
}

impl ReminderChannelService {
    pub fn new(
        sound: Arc<dyn ReminderChannel>,
        toast: Arc<dyn ReminderChannel>,
        window: Arc<dyn ReminderChannel>,
        repository: ReminderRepository,
    ) -> (Arc<Self>, ReminderChannelWorker) {
        let result_persistence = Arc::new(RepositoryChannelResultPersistence {
            repository: repository.clone(),
        });
        Self::new_with_result_persistence(sound, toast, window, repository, result_persistence)
    }

    fn new_with_result_persistence(
        sound: Arc<dyn ReminderChannel>,
        toast: Arc<dyn ReminderChannel>,
        window: Arc<dyn ReminderChannel>,
        repository: ReminderRepository,
        result_persistence: Arc<dyn ChannelResultPersistence>,
    ) -> (Arc<Self>, ReminderChannelWorker) {
        let (wake_tx, wake_rx) = tokio::sync::mpsc::channel(64);
        let service = Arc::new(Self {
            sound,
            toast,
            window,
            repository,
            result_persistence,
            wake_tx,
            wake_overflow: AtomicBool::new(false),
            persistence_halted: AtomicBool::new(false),
            attempted: Mutex::new(HashSet::new()),
        });
        (service.clone(), ReminderChannelWorker { service, wake_rx })
    }

    pub fn wake(&self, delivery_id: impl Into<String>, dispatch_seq: i64) {
        if self.is_halted() {
            return;
        }
        match self.wake_tx.try_send((delivery_id.into(), dispatch_seq)) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                // A hint is only an optimization.  Coalesce saturation into one durable rescan
                // instead of silently losing the final dispatched delivery.
                self.wake_overflow.store(true, Ordering::Release);
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {}
        }
    }

    pub fn handle_toast_activation(
        &self,
        delivery_id: &str,
        activated_at: i64,
    ) -> Result<PendingReminderNavigation, CommandError> {
        let delivery = self
            .repository
            .dispatched_delivery_by_id(delivery_id)?
            .ok_or_else(not_found)?;
        let pending = PendingReminderNavigation {
            sequence: delivery.dispatch_seq,
            delivery_id: delivery.id,
            source_kind: delivery.source_kind,
            source_entity_id: delivery.source_entity_id,
        };
        self.repository
            .persist_pending_navigation(&pending, activated_at)
    }

    pub fn handle_toast_activation_with(
        &self,
        delivery_id: &str,
        activated_at: i64,
        main_window: &dyn MainWindowPort,
        emitter: &dyn ReminderNavigationEmitter,
    ) -> Result<ToastActivationResult, CommandError> {
        let pending = self.handle_toast_activation(delivery_id, activated_at)?;
        if pending.source_kind != ReminderSourceKind::Agent {
            return Ok(ToastActivationResult::UnknownContext(pending));
        }
        main_window
            .show_main()
            .map_err(|failure| activation_failure(failure.code))?;
        emitter
            .emit_navigation(&pending)
            .map_err(|failure| activation_failure(failure.code))?;
        Ok(ToastActivationResult::NavigateHome(pending))
    }

    pub async fn deliver_pending_channels(&self, delivery_id: &str, dispatch_seq: i64) {
        if self.is_halted() {
            return;
        }
        self.deliver_pending_channels_with_cancellation(
            delivery_id,
            dispatch_seq,
            DeliveryCancellation::default(),
        )
        .await;
    }

    async fn deliver_pending_channels_with_cancellation(
        &self,
        delivery_id: &str,
        dispatch_seq: i64,
        cancellation: DeliveryCancellation,
    ) {
        if self.is_halted() {
            return;
        }
        let Some(delivery) = self
            .repository
            .dispatched_delivery(delivery_id, dispatch_seq)
            .unwrap_or(None)
        else {
            return;
        };
        let ((), (), ()) = tokio::join!(
            self.deliver_if_pending(self.sound.as_ref(), &delivery, cancellation.clone()),
            self.deliver_if_pending(self.toast.as_ref(), &delivery, cancellation.clone()),
            self.deliver_if_pending(self.window.as_ref(), &delivery, cancellation),
        );
    }

    async fn deliver_if_pending(
        &self,
        channel: &dyn ReminderChannel,
        delivery: &ReminderDelivery,
        cancellation: DeliveryCancellation,
    ) {
        if self.is_halted() {
            return;
        }
        let channel_name = channel_name(channel);
        let attempt_key = (delivery.id.clone(), delivery.dispatch_seq, channel_name);
        let unseen = self
            .attempted
            .lock()
            .expect("channel attempt lock poisoned")
            .insert(attempt_key.clone());
        if !unseen {
            return;
        }
        if self
            .repository
            .is_channel_pending(&delivery.id, delivery.dispatch_seq, channel_name)
            .unwrap_or(false)
        {
            match self.deliver_one(channel, delivery, cancellation).await {
                Ok(()) => {
                    self.attempted
                        .lock()
                        .expect("channel attempt lock poisoned")
                        .remove(&attempt_key);
                }
                Err(()) => {
                    // A durable result is unknown.  Preserve this key for restart recovery and
                    // stop accepting new deliveries rather than letting it grow without bound.
                    self.persistence_halted.store(true, Ordering::Release);
                }
            }
        } else {
            // No OS call is still terminal for this transient key: the durable row is no longer
            // pending, so retaining it would only leak memory across deliveries.
            self.attempted
                .lock()
                .expect("channel attempt lock poisoned")
                .remove(&attempt_key);
        }
    }

    async fn deliver_one(
        &self,
        channel: &dyn ReminderChannel,
        delivery: &ReminderDelivery,
        cancellation: DeliveryCancellation,
    ) -> Result<(), ()> {
        let (name, result) = match channel.name() {
            ReminderChannelName::Sound => (
                "sound",
                channel.deliver_cancellable(delivery, cancellation).await,
            ),
            ReminderChannelName::Toast => (
                "toast",
                channel.deliver_cancellable(delivery, cancellation).await,
            ),
            ReminderChannelName::Window => (
                "window",
                channel.deliver_cancellable(delivery, cancellation).await,
            ),
        };
        let (succeeded, error_code) = match result {
            Ok(()) => (true, None),
            Err(failure) => (false, Some(safe_channel_code(name, failure.code))),
        };
        self.result_persistence
            .persist(
                delivery,
                name,
                succeeded,
                error_code,
                crate::services::now_millis(),
            )
            .map_err(|_| ())
    }

    fn is_halted(&self) -> bool {
        self.persistence_halted.load(Ordering::Acquire)
    }
}

fn safe_channel_code(channel: &str, code: &'static str) -> &'static str {
    match channel {
        "sound" => match code {
            "soundOpenFailed"
            | "soundDecodeFailed"
            | "soundDeviceUnavailable"
            | "soundPlayFailed" => code,
            _ => "soundPlayFailed",
        },
        "toast" => match code {
            "toastUnavailable" | "toastShowFailed" => code,
            _ => "toastShowFailed",
        },
        "window" => match code {
            "alertEmitFailed" | "alertWindowUnavailable" | "alertShowFailed" => code,
            _ => "alertShowFailed",
        },
        _ => "channelFailed",
    }
}

impl ReminderChannelWorker {
    pub async fn run(mut self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        if *shutdown.borrow() {
            return;
        }
        if !self.recover_pending_channels(&mut shutdown).await {
            return;
        }
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { return; }
                }
                hint = self.wake_rx.recv() => {
                    let Some((delivery_id, dispatch_seq)) = hint else { return; };
                    if !self.deliver_until_shutdown(&delivery_id, dispatch_seq, &mut shutdown).await {
                        return;
                    }
                    if self.service.is_halted() {
                        return;
                    }
                    if self.service.wake_overflow.swap(false, Ordering::AcqRel)
                        && !self.recover_pending_channels(&mut shutdown).await
                    {
                        return;
                    }
                }
            }
        }
    }

    async fn recover_pending_channels(
        &self,
        shutdown: &mut tokio::sync::watch::Receiver<bool>,
    ) -> bool {
        for delivery in self
            .service
            .repository
            .dispatched_with_pending_channels()
            .unwrap_or_default()
        {
            if !self
                .deliver_until_shutdown(&delivery.id, delivery.dispatch_seq, shutdown)
                .await
            {
                return false;
            }
            if self.service.is_halted() {
                return false;
            }
        }
        !*shutdown.borrow()
    }

    async fn deliver_until_shutdown(
        &self,
        delivery_id: &str,
        dispatch_seq: i64,
        shutdown: &mut tokio::sync::watch::Receiver<bool>,
    ) -> bool {
        let cancellation = DeliveryCancellation::default();
        let delivery = self.service.deliver_pending_channels_with_cancellation(
            delivery_id,
            dispatch_seq,
            cancellation.clone(),
        );
        tokio::pin!(delivery);
        loop {
            if *shutdown.borrow() {
                cancellation.cancel();
                delivery.await;
                return false;
            }
            tokio::select! {
                () = &mut delivery => return true,
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        cancellation.cancel();
                        delivery.await;
                        return false;
                    }
                }
            }
        }
    }
}

fn channel_name(channel: &dyn ReminderChannel) -> &'static str {
    match channel.name() {
        ReminderChannelName::Sound => "sound",
        ReminderChannelName::Toast => "toast",
        ReminderChannelName::Window => "window",
    }
}

fn not_found() -> CommandError {
    CommandError {
        code: AppErrorCode::NotFound,
        message_key: "errors.notFound".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

fn activation_failure(code: &'static str) -> CommandError {
    CommandError {
        code: AppErrorCode::SourceUnavailable,
        message_key: "errors.sourceUnavailable".into(),
        details: SafeMessageParameters::from([(
            "reasonCode".into(),
            crate::contracts::SafeParameterValue::String(code.into()),
        )]),
        retryable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{
        AgentEnvironment, AgentId, AgentTriggerStatus, BuiltinReminderSoundId, ReminderSound,
        ReminderSourceContext, ReminderSourceKind, SafeParameterValue, SaveReminderRuleInput,
    };
    use crate::domain::reminders::{EnqueueOutcome, NewReminderDelivery};
    use crate::storage::Storage;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    struct FakeChannel {
        name: ReminderChannelName,
        result: Result<(), ChannelFailure>,
        calls: AtomicUsize,
    }

    #[derive(Default)]
    struct RecordingActivation {
        calls: Mutex<Vec<String>>,
    }

    #[derive(Default)]
    struct RecordingNotificationHealth {
        calls: Mutex<Vec<(bool, Option<String>)>>,
    }

    impl NotificationHealthPort for RecordingNotificationHealth {
        fn record(&self, healthy: bool, reason_code: Option<&str>) {
            self.calls
                .lock()
                .unwrap()
                .push((healthy, reason_code.map(str::to_owned)));
        }
    }

    impl ToastActivationHandler for RecordingActivation {
        fn activate(&self, delivery_id: &str) {
            self.calls.lock().unwrap().push(delivery_id.into());
        }
    }

    struct PersistCheckingMainWindow {
        storage: Arc<Storage>,
        calls: AtomicUsize,
        fail: bool,
    }

    struct FakeAlertWindow {
        emitted: AtomicUsize,
        shown: AtomicUsize,
        focused: AtomicUsize,
        groups: Mutex<Vec<ReminderAlertGroup>>,
    }

    impl AlertWindowPort for FakeAlertWindow {
        fn emit_reminder(&self, group: &ReminderAlertGroup) -> Result<(), ChannelFailure> {
            self.emitted.fetch_add(1, Ordering::AcqRel);
            self.groups.lock().unwrap().push(group.clone());
            Ok(())
        }

        fn show(&self) -> Result<(), ChannelFailure> {
            self.shown.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }

        fn focus_after_user_activation(&self) -> Result<(), ChannelFailure> {
            self.focused.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    impl MainWindowPort for PersistCheckingMainWindow {
        fn show_main(&self) -> Result<(), ChannelFailure> {
            let persisted = self.storage.with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM app_settings WHERE key = 'navigation.reminder.pending'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(Into::into)
            }).unwrap();
            assert_eq!(persisted, 1, "navigation must persist before main is shown");
            self.calls.fetch_add(1, Ordering::AcqRel);
            if self.fail {
                Err(ChannelFailure { code: "showFailed" })
            } else {
                Ok(())
            }
        }
    }

    struct PersistCheckingEmitter {
        storage: Arc<Storage>,
        calls: AtomicUsize,
        fail: bool,
    }

    impl ReminderNavigationEmitter for PersistCheckingEmitter {
        fn emit_navigation(
            &self,
            navigation: &PendingReminderNavigation,
        ) -> Result<(), ChannelFailure> {
            let persisted = self.storage.with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT value_json FROM app_settings WHERE key = 'navigation.reminder.pending'",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(Into::into)
            }).unwrap();
            assert!(persisted.contains(&navigation.delivery_id));
            self.calls.fetch_add(1, Ordering::AcqRel);
            if self.fail {
                Err(ChannelFailure { code: "emitFailed" })
            } else {
                Ok(())
            }
        }
    }

    impl FakeChannel {
        fn new(name: ReminderChannelName, result: Result<(), ChannelFailure>) -> Self {
            Self {
                name,
                result,
                calls: AtomicUsize::new(0),
            }
        }
    }

    struct FailingChannelResultPersistence {
        calls: AtomicUsize,
    }

    impl ChannelResultPersistence for FailingChannelResultPersistence {
        fn persist(
            &self,
            _: &ReminderDelivery,
            _: &str,
            _: bool,
            _: Option<&str>,
            _: i64,
        ) -> Result<(), CommandError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Err(not_found())
        }
    }

    #[async_trait::async_trait]
    impl ReminderChannel for FakeChannel {
        fn name(&self) -> ReminderChannelName {
            self.name
        }

        async fn deliver(&self, _delivery: &ReminderDelivery) -> Result<(), ChannelFailure> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.result.clone()
        }
    }

    struct BlockingChannel {
        name: ReminderChannelName,
        started: tokio::sync::Notify,
        release: tokio::sync::Notify,
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ReminderChannel for BlockingChannel {
        fn name(&self) -> ReminderChannelName {
            self.name
        }

        async fn deliver(&self, _: &ReminderDelivery) -> Result<(), ChannelFailure> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.started.notify_waiters();
            self.release.notified().await;
            Ok(())
        }
    }

    fn repository() -> (TempDir, Arc<Storage>, ReminderRepository) {
        let temp = TempDir::new().unwrap();
        let storage = Arc::new(Storage::open(temp.path()).unwrap());
        let repository = ReminderRepository::new(storage.clone());
        (temp, storage, repository)
    }

    fn delivery(repository: &ReminderRepository) -> ReminderDelivery {
        delivery_with_rule(repository, None)
    }

    fn delivery_with_rule(
        repository: &ReminderRepository,
        rule_id: Option<uuid::Uuid>,
    ) -> ReminderDelivery {
        let request = NewReminderDelivery {
            dedupe_key: format!("channel-red-{}", uuid::Uuid::new_v4()),
            rule_id,
            source_kind: ReminderSourceKind::Agent,
            source_entity_id: "agent:rule:codex:windows:task:completed".into(),
            message_key: "reminders.agent.status".into(),
            message_parameters: BTreeMap::from([
                (
                    "agentName".into(),
                    SafeParameterValue::String("Codex".into()),
                ),
                (
                    "environment".into(),
                    SafeParameterValue::String("windows".into()),
                ),
                ("taskId".into(), SafeParameterValue::String("task".into())),
                (
                    "taskTitle".into(),
                    SafeParameterValue::String("task".into()),
                ),
                (
                    "triggerStatus".into(),
                    SafeParameterValue::String("completed".into()),
                ),
            ]),
            source_context: ReminderSourceContext::Agent {
                agent_id: AgentId::Codex,
                environment: AgentEnvironment::Windows,
                task_id: "task".into(),
                task_title: Some("task".into()),
                trigger_status: AgentTriggerStatus::Completed,
                source_event_id: "event".into(),
                source_occurred_at: 10,
            },
            source_occurred_at: 10,
            sound: ReminderSound::Builtin {
                sound_id: BuiltinReminderSoundId::SystemNotification,
            },
            toast_enabled: true,
            window_enabled: true,
            due_at: 10,
        };
        let EnqueueOutcome::Inserted(_) = repository.enqueue(request, 10).unwrap() else {
            panic!("fixture delivery must insert")
        };
        repository.claim_due(10, 1).unwrap().pop().unwrap()
    }

    #[test]
    fn toast_activation_port_accepts_only_uuid_arguments_and_installs_once() {
        let port = ToastActivationPort::default();
        let first = Arc::new(RecordingActivation::default());
        let second = Arc::new(RecordingActivation::default());
        assert!(port.install_once(&first));
        assert!(!port.install_once(&second));
        port.dispatch_uuid_only("not-a-delivery");
        let id = uuid::Uuid::new_v4().to_string();
        port.dispatch_uuid_only(&id);
        assert_eq!(*first.calls.lock().unwrap(), vec![id]);
        assert!(second.calls.lock().unwrap().is_empty());
    }

    // RED: the native callback port formerly retained the router strongly, creating an
    // AppServices -> port -> router -> channel-service lifetime cycle.
    #[test]
    fn toast_activation_port_does_not_retain_a_dropped_handler() {
        let port = ToastActivationPort::default();
        let handler = Arc::new(RecordingActivation::default());
        let original_strong_count = Arc::strong_count(&handler);
        assert!(port.install_once(&handler));
        assert_eq!(Arc::strong_count(&handler), original_strong_count);
        let id = uuid::Uuid::new_v4().to_string();
        port.dispatch_uuid_only(&id);
        assert_eq!(*handler.calls.lock().unwrap(), vec![id.clone()]);

        drop(handler);
        port.dispatch_uuid_only(&id);
        // Dispatch after the owning service/router has gone away is deliberately a no-op.
    }

    #[test]
    fn cold_start_activation_uses_the_same_uuid_only_router() {
        let port = ToastActivationPort::default();
        let handler = Arc::new(RecordingActivation::default());
        assert!(port.install_once(&handler));
        let id = uuid::Uuid::new_v4().to_string();
        dispatch_cold_start_activation(&port, &id);
        dispatch_cold_start_activation(&port, "not-a-delivery");
        assert_eq!(*handler.calls.lock().unwrap(), vec![id]);
    }

    #[test]
    fn local_server_command_quotes_a_space_containing_executable_without_backslashes() {
        assert_eq!(
            local_server_command(r"C:\Program Files\AIsland\aisland.exe"),
            r#""C:\Program Files\AIsland\aisland.exe""#
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_shortcut_installation_keeps_property_memory_owned_until_commit() {
        let app_data = tempfile::tempdir().expect("temporary APPDATA");
        let status = std::process::Command::new(
            std::env::current_exe().expect("current Rust test executable"),
        )
        .args([
            "--exact",
            "services::reminder_channels::tests::windows_shortcut_installation_child",
            "--nocapture",
        ])
        .env("APPDATA", app_data.path())
        .env("AISLAND_TEST_SHORTCUT_CHILD", "1")
        .status()
        .expect("shortcut installation child starts");

        assert!(
            status.success(),
            "the real Shell property-store operation must not corrupt the child heap: {status}"
        );
        assert!(
            app_data
                .path()
                .join("Microsoft/Windows/Start Menu/Programs/AIsland.lnk")
                .is_file(),
            "the isolated shortcut must be committed"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_shortcut_installation_child() {
        if std::env::var_os("AISLAND_TEST_SHORTCUT_CHILD").is_none() {
            return;
        }

        use windows::Win32::System::Com::{
            CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
        };

        struct ComApartment;
        impl Drop for ComApartment {
            fn drop(&mut self) {
                unsafe { CoUninitialize() };
            }
        }

        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .expect("test child initializes one STA");
        let _apartment = ComApartment;
        let executable = std::env::current_exe()
            .expect("current test executable")
            .to_string_lossy()
            .into_owned();
        WindowsColdStartRegistrationPort
            .install_shortcut(&executable, TOAST_AUMID, TOAST_ACTIVATOR_CLSID_TEXT)
            .expect("isolated Windows shortcut installation");
    }

    #[cfg(windows)]
    #[test]
    fn cold_start_registration_commits_ready_only_after_registry_shortcut_and_class_registration() {
        struct RecordingRegistrationPort {
            calls: Mutex<Vec<String>>,
            fail_class_registration: bool,
        }
        impl ColdStartRegistrationPort for RecordingRegistrationPort {
            fn current_executable(&self) -> Result<String, ChannelFailure> {
                self.calls.lock().unwrap().push("currentExe".into());
                Ok(r"C:\Program Files\AIsland\aisland.exe".into())
            }

            fn write_local_server32(&self, command: &str) -> Result<(), ChannelFailure> {
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("registry:{command}"));
                Ok(())
            }
            fn install_shortcut(
                &self,
                executable: &str,
                aumid: &str,
                clsid: &str,
            ) -> Result<(), ChannelFailure> {
                assert_eq!(executable, r"C:\Program Files\AIsland\aisland.exe");
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("shortcut:{aumid}:{clsid}"));
                Ok(())
            }
            fn register_class(
                &self,
                _: Arc<ToastActivationPort>,
            ) -> Result<ColdStartActivationRegistration, ChannelFailure> {
                self.calls.lock().unwrap().push("class".into());
                if self.fail_class_registration {
                    Err(ChannelFailure {
                        code: "toastRegistrationFailed",
                    })
                } else {
                    Ok(ColdStartActivationRegistration::for_test())
                }
            }
        }

        let port = RecordingRegistrationPort {
            calls: Mutex::new(Vec::new()),
            fail_class_registration: false,
        };
        register_windows_cold_start_activation_with(
            &port,
            Arc::new(ToastActivationPort::default()),
        )
        .unwrap();
        assert_eq!(
            *port.calls.lock().unwrap(),
            vec![
                String::from("currentExe"),
                String::from(r#"registry:"C:\Program Files\AIsland\aisland.exe""#),
                String::from("shortcut:com.aisland.app:{8A3824C5-5A7D-4D59-BF04-2C19C43B6F9A}"),
                String::from("class"),
            ],
        );

        let failed = RecordingRegistrationPort {
            calls: Mutex::new(Vec::new()),
            fail_class_registration: true,
        };
        let failure = match register_windows_cold_start_activation_with(
            &failed,
            Arc::new(ToastActivationPort::default()),
        ) {
            Ok(_) => panic!("class registration failure must not return a ready registration"),
            Err(failure) => failure,
        };
        assert_eq!(failure.code, "toastRegistrationFailed");
        assert_eq!(
            *failed.calls.lock().unwrap(),
            vec![
                String::from("currentExe"),
                String::from(r#"registry:"C:\Program Files\AIsland\aisland.exe""#),
                String::from("shortcut:com.aisland.app:{8A3824C5-5A7D-4D59-BF04-2C19C43B6F9A}"),
                String::from("class"),
            ],
        );
    }

    #[test]
    fn toast_text_uses_the_committed_locale_for_title_and_body() {
        let (_temp, _storage, repository) = repository();
        let delivery = delivery(&repository);
        let zh = toast_text("zh-CN", &delivery).unwrap();
        let en = toast_text("en-US", &delivery).unwrap();
        assert_eq!(zh.0, "AIsland 提醒");
        assert_eq!(en.0, "AIsland reminder");
        assert_ne!(zh.1, en.1);
        assert!(zh.1.contains("Codex"));
        assert!(en.1.contains("Codex"));
    }

    #[test]
    fn toast_xml_uses_the_schema_valid_silent_audio_child() {
        let (_temp, _storage, repository) = repository();
        let delivery = delivery(&repository);
        let xml = toast_xml(&delivery, "AIsland reminder", "Body");

        assert!(xml.starts_with(&format!("<toast launch=\"{}\">", delivery.id)));
        assert!(xml.contains("<text>AIsland reminder</text><text>Body</text>"));
        assert!(xml.contains("</visual><audio silent=\"true\"/></toast>"));
    }

    #[test]
    fn builtin_system_sound_uses_the_windows_notification_default_alias() {
        assert_eq!(notification_default_sound_alias(), "Notification.Default");
    }

    #[cfg(windows)]
    #[test]
    fn builtin_system_sound_flags_join_the_system_notification_session() {
        use windows::Win32::Media::Audio::SND_SYSTEM;

        assert_ne!(notification_default_sound_flags() & SND_SYSTEM.0, 0);
    }

    // Post-implementation regression coverage: both the native-toast failure path and a later
    // successful delivery must leave one durable, safe notification-health snapshot.
    #[test]
    fn notification_health_persists_degraded_failure_then_healthy_success() {
        let (_temp, storage, _reminders) = repository();
        let health = crate::repositories::service_health::ServiceHealthRepository::new(storage);
        let port = RepositoryNotificationHealthPort(health.clone());
        port.record(false, Some("toastShowFailed"));
        let degraded = health
            .list()
            .unwrap()
            .into_iter()
            .find(|snapshot| snapshot.service_id == "notifications")
            .unwrap();
        assert_eq!(
            degraded.state,
            crate::contracts::ServiceHealthState::Degraded
        );
        assert_eq!(degraded.message_key, "services.degraded");
        assert_eq!(
            degraded.parameters.get("reasonCode"),
            Some(&SafeParameterValue::String("toastShowFailed".into()))
        );

        port.record(true, None);
        let healthy = health.list().unwrap().pop().unwrap();
        assert_eq!(healthy.state, crate::contracts::ServiceHealthState::Healthy);
        assert_eq!(healthy.message_key, "services.healthy");
        assert!(!healthy.parameters.contains_key("reasonCode"));
    }

    #[tokio::test]
    async fn unregistered_toast_refuses_show_and_never_marks_notification_health_healthy() {
        let (_temp, _storage, repository) = repository();
        let delivery = delivery(&repository);
        let health = Arc::new(RecordingNotificationHealth::default());
        let registration = Arc::new(ToastRegistrationState::default());
        let channel = WindowsToastReminderChannel::with_health_and_registration(
            Arc::new(ToastActivationPort::default()),
            health.clone(),
            registration,
        );

        let error = channel.deliver(&delivery).await.unwrap_err();

        assert_eq!(error.code, "toastRegistrationFailed");
        assert_eq!(
            *health.calls.lock().unwrap(),
            vec![(false, Some("toastRegistrationFailed".into()))]
        );
    }

    // The production callback is the public seam between WinRT activation and the durable
    // navigation workflow.  It must use the same UUID-only port as the native Toast and retain
    // the record even if the later UI emission fails.
    #[test]
    fn toast_activation_callback_persists_before_show_and_emit() {
        let (_temp, storage, repository) = repository();
        let delivery = delivery(&repository);
        let (service, _worker) = ReminderChannelService::new(
            Arc::new(FakeChannel::new(ReminderChannelName::Sound, Ok(()))),
            Arc::new(FakeChannel::new(ReminderChannelName::Toast, Ok(()))),
            Arc::new(FakeChannel::new(ReminderChannelName::Window, Ok(()))),
            repository,
        );
        let main = Arc::new(PersistCheckingMainWindow {
            storage: storage.clone(),
            calls: AtomicUsize::new(0),
            fail: false,
        });
        let emitter = Arc::new(PersistCheckingEmitter {
            storage: storage.clone(),
            calls: AtomicUsize::new(0),
            fail: false,
        });
        let port = ToastActivationPort::default();

        let router = Arc::new(ToastActivationRouter::new(
            service,
            main.clone(),
            emitter.clone(),
            Arc::new(|| 20),
        ));
        assert!(port.install_once(&router));
        port.dispatch_uuid_only(&delivery.id);

        assert_eq!(main.calls.load(Ordering::Acquire), 1);
        assert_eq!(emitter.calls.load(Ordering::Acquire), 1);
        let persisted = storage.with_connection(|connection| connection.query_row(
            "SELECT value_json FROM app_settings WHERE key = 'navigation.reminder.pending'", [], |row| row.get::<_, String>(0)
        ).map_err(Into::into)).unwrap();
        assert!(persisted.contains(&delivery.id));
    }

    #[tokio::test]
    async fn sound_failure_is_persisted_without_blocking_toast_or_window() {
        let (_temp, storage, repository) = repository();
        let delivery = delivery(&repository);
        let sound = Arc::new(FakeChannel::new(
            ReminderChannelName::Sound,
            Err(ChannelFailure {
                code: "soundDeviceUnavailable",
            }),
        ));
        let toast = Arc::new(FakeChannel::new(ReminderChannelName::Toast, Ok(())));
        let window = Arc::new(FakeChannel::new(ReminderChannelName::Window, Ok(())));
        let (service, _worker) =
            ReminderChannelService::new(sound.clone(), toast.clone(), window.clone(), repository);

        service
            .deliver_pending_channels(&delivery.id, delivery.dispatch_seq)
            .await;

        assert_eq!(sound.calls.load(Ordering::Acquire), 1);
        assert_eq!(toast.calls.load(Ordering::Acquire), 1);
        assert_eq!(window.calls.load(Ordering::Acquire), 1);
        let states = storage.with_connection(|connection| {
            connection.query_row(
                "SELECT sound_state, sound_error_code, toast_state, window_state FROM reminder_deliveries WHERE id = ?1",
                [&delivery.id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)),
            ).map_err(Into::into)
        }).unwrap();
        assert_eq!(
            states,
            (
                "failed".into(),
                Some("soundDeviceUnavailable".into()),
                "succeeded".into(),
                "succeeded".into()
            )
        );
    }

    #[tokio::test]
    async fn a_blocking_sound_does_not_delay_toast_or_window_delivery() {
        let (_temp, _storage, repository) = repository();
        let delivery = delivery(&repository);
        let sound = Arc::new(BlockingChannel {
            name: ReminderChannelName::Sound,
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            calls: AtomicUsize::new(0),
        });
        let toast = Arc::new(FakeChannel::new(ReminderChannelName::Toast, Ok(())));
        let window = Arc::new(FakeChannel::new(ReminderChannelName::Window, Ok(())));
        let (service, _worker) =
            ReminderChannelService::new(sound.clone(), toast.clone(), window.clone(), repository);
        let delivery_id = delivery.id.clone();
        let dispatch_seq = delivery.dispatch_seq;
        let task = tokio::spawn(async move {
            service
                .deliver_pending_channels(&delivery_id, dispatch_seq)
                .await;
        });

        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            sound.started.notified(),
        )
        .await
        .expect("sound delivery must start");
        tokio::time::timeout(std::time::Duration::from_millis(100), async {
            while toast.calls.load(Ordering::Acquire) == 0
                || window.calls.load(Ordering::Acquire) == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("toast and window must run while sound is still blocked");
        sound.release.notify_waiters();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn persisted_channel_results_release_the_transient_attempt_deduplication_keys() {
        let (_temp, _storage, repository) = repository();
        let delivery = delivery(&repository);
        let (service, _worker) = ReminderChannelService::new(
            Arc::new(FakeChannel::new(ReminderChannelName::Sound, Ok(()))),
            Arc::new(FakeChannel::new(ReminderChannelName::Toast, Ok(()))),
            Arc::new(FakeChannel::new(ReminderChannelName::Window, Ok(()))),
            repository,
        );

        service
            .deliver_pending_channels(&delivery.id, delivery.dispatch_seq)
            .await;

        assert!(service.attempted.lock().unwrap().is_empty());
    }

    // RED: after SQLite result persistence fails, retaining each key while continuing every new
    // delivery makes `attempted` grow without bound and repeats OS side effects indefinitely.
    #[tokio::test]
    async fn persistence_failure_halts_the_worker_before_new_delivery_attempts_grow_the_set() {
        let (_temp, _storage, repository) = repository();
        let first = delivery(&repository);
        let second = delivery(&repository);
        let sound = Arc::new(FakeChannel::new(ReminderChannelName::Sound, Ok(())));
        let toast = Arc::new(FakeChannel::new(ReminderChannelName::Toast, Ok(())));
        let window = Arc::new(FakeChannel::new(ReminderChannelName::Window, Ok(())));
        let persistence = Arc::new(FailingChannelResultPersistence {
            calls: AtomicUsize::new(0),
        });
        let (service, worker) = ReminderChannelService::new_with_result_persistence(
            sound.clone(),
            toast.clone(),
            window.clone(),
            repository,
            persistence.clone(),
        );
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let worker_join = tokio::spawn(worker.run(shutdown_rx));

        tokio::time::timeout(std::time::Duration::from_millis(250), async {
            while persistence.calls.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the first dispatched delivery must attempt durable result persistence");
        tokio::time::timeout(std::time::Duration::from_millis(250), worker_join)
            .await
            .expect("persistence failure must halt the channel worker")
            .unwrap();

        let first_os_calls = sound.calls.load(Ordering::Acquire)
            + toast.calls.load(Ordering::Acquire)
            + window.calls.load(Ordering::Acquire);
        assert!(first_os_calls > 0 && first_os_calls <= 3);
        assert!(service.attempted.lock().unwrap().len() <= 3);
        assert!(service.is_halted());

        service.wake(second.id, second.dispatch_seq);
        tokio::task::yield_now().await;
        assert_eq!(
            sound.calls.load(Ordering::Acquire)
                + toast.calls.load(Ordering::Acquire)
                + window.calls.load(Ordering::Acquire),
            first_os_calls,
            "halted service must not start new OS attempts after durable persistence failure"
        );
        shutdown_tx.send_replace(true);
        drop(first);
    }

    #[test]
    fn local_file_playback_timeout_stops_the_sink_before_returning() {
        let stopped = AtomicUsize::new(0);

        let result = wait_for_local_sound_completion(
            std::time::Duration::ZERO,
            || false,
            || {
                stopped.fetch_add(1, Ordering::AcqRel);
            },
        );

        assert_eq!(
            result,
            Err(ChannelFailure {
                code: "soundPlayTimedOut"
            })
        );
        assert_eq!(stopped.load(Ordering::Acquire), 1);
    }

    // RED: native backends must never persist an OS error or an audio file path; channel
    // diagnostics are intentionally limited to the fixed public sound failure vocabulary.
    #[tokio::test]
    async fn sound_failure_is_reduced_to_a_safe_fixed_code_without_a_path() {
        let (_temp, storage, repository) = repository();
        let delivery = delivery(&repository);
        let sound = Arc::new(FakeChannel::new(
            ReminderChannelName::Sound,
            Err(ChannelFailure {
                code: "C:\\\\private\\\\reminders\\\\secret.wav",
            }),
        ));
        let toast = Arc::new(FakeChannel::new(ReminderChannelName::Toast, Ok(())));
        let window = Arc::new(FakeChannel::new(ReminderChannelName::Window, Ok(())));
        let (service, _worker) = ReminderChannelService::new(sound, toast, window, repository);

        service
            .deliver_pending_channels(&delivery.id, delivery.dispatch_seq)
            .await;

        let error_code = storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT sound_error_code FROM reminder_deliveries WHERE id = ?1",
                        [&delivery.id],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(error_code.as_deref(), Some("soundPlayFailed"));
    }

    #[tokio::test]
    async fn worker_recovers_only_persisted_pending_channels_and_stale_hints_do_not_attempt() {
        let (_temp, _storage, repository) = repository();
        let delivery = delivery(&repository);
        let sound = Arc::new(FakeChannel::new(ReminderChannelName::Sound, Ok(())));
        let toast = Arc::new(FakeChannel::new(ReminderChannelName::Toast, Ok(())));
        let window = Arc::new(FakeChannel::new(ReminderChannelName::Window, Ok(())));
        let (service, worker) = ReminderChannelService::new(
            sound.clone(),
            toast.clone(),
            window.clone(),
            repository.clone(),
        );
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let join = tokio::spawn(worker.run(shutdown_rx));
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if sound.calls.load(Ordering::Acquire) == 1
                    && toast.calls.load(Ordering::Acquire) == 1
                    && window.calls.load(Ordering::Acquire) == 1
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("worker must recover the dispatched row");

        service.wake(delivery.id.clone(), delivery.dispatch_seq);
        tokio::task::yield_now().await;
        assert_eq!(sound.calls.load(Ordering::Acquire), 1);
        assert_eq!(toast.calls.load(Ordering::Acquire), 1);
        assert_eq!(window.calls.load(Ordering::Acquire), 1);
        shutdown_tx.send(true).unwrap();
        join.await.unwrap();
    }

    #[tokio::test]
    async fn overflowed_hint_triggers_a_coalesced_persisted_rescan() {
        let (_temp, storage, repository) = repository();
        let sound = Arc::new(BlockingChannel {
            name: ReminderChannelName::Sound,
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            calls: AtomicUsize::new(0),
        });
        let toast = Arc::new(FakeChannel::new(ReminderChannelName::Toast, Ok(())));
        let window = Arc::new(FakeChannel::new(ReminderChannelName::Window, Ok(())));
        let (service, worker) =
            ReminderChannelService::new(sound.clone(), toast, window, repository.clone());
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let worker_join = tokio::spawn(worker.run(shutdown_rx));

        let blocker = delivery(&repository);
        service.wake(blocker.id.clone(), blocker.dispatch_seq);
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            sound.started.notified(),
        )
        .await
        .expect("first delivery must block the worker");

        let target = delivery(&repository);
        for _ in 0..64 {
            service.wake(uuid::Uuid::new_v4().to_string(), 1);
        }
        service.wake(target.id.clone(), target.dispatch_seq);
        sound.release.notify_waiters();

        tokio::time::timeout(std::time::Duration::from_millis(500), async {
            loop {
                let toast_state = storage
                    .with_connection(|connection| {
                        connection
                            .query_row(
                                "SELECT toast_state FROM reminder_deliveries WHERE id = ?1",
                                [&target.id],
                                |row| row.get::<_, String>(0),
                            )
                            .map_err(Into::into)
                    })
                    .unwrap();
                if toast_state == "succeeded" {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the dropped hint must be recovered from persisted pending rows");

        sound.release.notify_waiters();
        shutdown_tx.send(true).unwrap();
        worker_join.await.unwrap();
    }

    // Break caught: shutdown must be observed before recovery begins, otherwise a process that
    // is already stopping can play sound/show a Toast while the shutdown owner waits to join it.
    #[tokio::test]
    async fn channel_worker_skips_recovery_when_shutdown_already_started() {
        let (_temp, _storage, repository) = repository();
        let delivery = delivery(&repository);
        let sound = Arc::new(FakeChannel::new(ReminderChannelName::Sound, Ok(())));
        let toast = Arc::new(FakeChannel::new(ReminderChannelName::Toast, Ok(())));
        let window = Arc::new(FakeChannel::new(ReminderChannelName::Window, Ok(())));
        let (_service, worker) =
            ReminderChannelService::new(sound.clone(), toast.clone(), window.clone(), repository);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        shutdown_tx.send_replace(true);

        worker.run(shutdown_rx).await;

        assert_eq!(
            sound.calls.load(Ordering::Acquire),
            0,
            "delivery {} must remain durable for restart",
            delivery.id
        );
        assert_eq!(toast.calls.load(Ordering::Acquire), 0);
        assert_eq!(window.calls.load(Ordering::Acquire), 0);
    }

    // RED: dropping the recovery future used to drop Rodio's JoinHandle while the blocking
    // playback thread kept running.  Shutdown must signal the in-flight local player, wait for
    // its blocking task to exit, and only then let the worker finish.
    #[tokio::test]
    async fn worker_shutdown_cancels_and_joins_an_inflight_local_sound_playback() {
        struct BlockingLocalPlayer {
            started: tokio::sync::Notify,
            stopped: AtomicBool,
        }
        impl LocalSoundPort for BlockingLocalPlayer {
            fn play(
                &self,
                _: &ReminderSound,
                _: std::time::Instant,
                cancellation: &DeliveryCancellation,
            ) -> Result<(), ChannelFailure> {
                self.started.notify_waiters();
                while !cancellation.is_cancelled() {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                self.stopped.store(true, Ordering::Release);
                Ok(())
            }
        }

        let (_temp, _storage, repository) = repository();
        let mut delivery = delivery(&repository);
        delivery.sound = ReminderSound::LocalFile {
            canonical_path: "test-controlled-local-sound".into(),
        };
        let local_player = Arc::new(BlockingLocalPlayer {
            started: tokio::sync::Notify::new(),
            stopped: AtomicBool::new(false),
        });
        let (service, worker) = ReminderChannelService::new(
            Arc::new(RodioReminderChannel::with_local_player(
                local_player.clone(),
            )),
            Arc::new(FakeChannel::new(ReminderChannelName::Toast, Ok(()))),
            Arc::new(FakeChannel::new(ReminderChannelName::Window, Ok(()))),
            repository,
        );
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let worker_join = tokio::spawn(worker.run(shutdown_rx));

        tokio::time::timeout(
            std::time::Duration::from_millis(250),
            local_player.started.notified(),
        )
        .await
        .expect("local playback must enter the blocking player during recovery");
        shutdown_tx.send_replace(true);
        tokio::time::timeout(std::time::Duration::from_millis(250), worker_join)
            .await
            .expect("worker shutdown must join the cancelled blocking playback")
            .unwrap();
        assert!(local_player.stopped.load(Ordering::Acquire));
        drop(service);
    }

    #[test]
    fn toast_activation_persists_navigation_before_any_window_or_event_side_effect() {
        let (_temp, storage, repository) = repository();
        let delivery = delivery(&repository);
        let sound = Arc::new(FakeChannel::new(ReminderChannelName::Sound, Ok(())));
        let toast = Arc::new(FakeChannel::new(ReminderChannelName::Toast, Ok(())));
        let window = Arc::new(FakeChannel::new(ReminderChannelName::Window, Ok(())));
        let (service, _worker) = ReminderChannelService::new(sound, toast, window, repository);

        let navigation = service.handle_toast_activation(&delivery.id, 20).unwrap();

        assert_eq!(navigation.sequence, delivery.dispatch_seq);
        let persisted =
            storage.with_connection(|connection| {
                connection.query_row(
                "SELECT value_json FROM app_settings WHERE key = 'navigation.reminder.pending'",
                [],
                |row| row.get::<_, String>(0),
            ).map_err(Into::into)
            });
        let persisted =
            persisted.expect("activation must persist before window/event side effects");
        assert!(persisted.contains(&delivery.id));
    }

    // Break caught: a Toast click must durably save navigation before either showing the main
    // window or emitting Home navigation; later side-effect failure leaves that durable record.
    #[test]
    fn agent_toast_activation_persists_before_show_and_emit_and_remains_recoverable_on_failure() {
        let (_temp, storage, repository) = repository();
        let delivery = delivery(&repository);
        let (service, _worker) = ReminderChannelService::new(
            Arc::new(FakeChannel::new(ReminderChannelName::Sound, Ok(()))),
            Arc::new(FakeChannel::new(ReminderChannelName::Toast, Ok(()))),
            Arc::new(FakeChannel::new(ReminderChannelName::Window, Ok(()))),
            repository,
        );
        let main = PersistCheckingMainWindow {
            storage: storage.clone(),
            calls: AtomicUsize::new(0),
            fail: false,
        };
        let emitter = PersistCheckingEmitter {
            storage: storage.clone(),
            calls: AtomicUsize::new(0),
            fail: true,
        };

        let error = service
            .handle_toast_activation_with(&delivery.id, 20, &main, &emitter)
            .unwrap_err();

        assert_eq!(error.code, AppErrorCode::SourceUnavailable);
        assert_eq!(main.calls.load(Ordering::Acquire), 1);
        assert_eq!(emitter.calls.load(Ordering::Acquire), 1);
        let persisted = storage.with_connection(|connection| connection.query_row(
            "SELECT value_json FROM app_settings WHERE key = 'navigation.reminder.pending'", [], |row| row.get::<_, String>(0)
        ).map_err(Into::into)).unwrap();
        assert!(persisted.contains(&delivery.id));
    }

    // Break caught: activation is keyed by the durable delivery UUID, not by a channel that
    // happens to still be pending after the Toast was displayed.
    #[test]
    fn toast_click_routes_a_fully_delivered_dispatched_row_by_uuid() {
        let (_temp, storage, repository) = repository();
        let delivery = delivery(&repository);
        let (service, _worker) = ReminderChannelService::new(
            Arc::new(FakeChannel::new(ReminderChannelName::Sound, Ok(()))),
            Arc::new(FakeChannel::new(ReminderChannelName::Toast, Ok(()))),
            Arc::new(FakeChannel::new(ReminderChannelName::Window, Ok(()))),
            repository,
        );
        tauri::async_runtime::block_on(
            service.deliver_pending_channels(&delivery.id, delivery.dispatch_seq),
        );
        let main = PersistCheckingMainWindow {
            storage: storage.clone(),
            calls: AtomicUsize::new(0),
            fail: false,
        };
        let emitter = PersistCheckingEmitter {
            storage,
            calls: AtomicUsize::new(0),
            fail: false,
        };

        let route = service
            .handle_toast_activation_with(&delivery.id, 20, &main, &emitter)
            .expect("a Toast click must route its durable dispatched delivery");

        assert!(matches!(route, ToastActivationResult::NavigateHome(_)));
        assert_eq!(main.calls.load(Ordering::Acquire), 1);
        assert_eq!(emitter.calls.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn standalone_alert_window_is_retired_for_every_reminder_source() {
        let (_temp, _storage, repository) = repository();
        let rule = repository
            .save_rule(
                SaveReminderRuleInput {
                    id: None,
                    agent_ids: vec![AgentId::Codex],
                    trigger_statuses: vec![AgentTriggerStatus::Completed],
                    enabled: true,
                    delay_seconds: 0,
                    sound: ReminderSound::None,
                    toast_enabled: true,
                    window_enabled: true,
                    expected_revision: None,
                },
                9,
            )
            .unwrap();
        let delivery =
            delivery_with_rule(&repository, Some(uuid::Uuid::parse_str(&rule.id).unwrap()));
        let port = Arc::new(FakeAlertWindow {
            emitted: AtomicUsize::new(0),
            shown: AtomicUsize::new(0),
            focused: AtomicUsize::new(0),
            groups: Mutex::new(Vec::new()),
        });
        let channel = AlertWindowReminderChannel::new(port.clone(), repository);

        channel.deliver(&delivery).await.unwrap();
        let mut non_agent_delivery = delivery;
        non_agent_delivery.source_kind = ReminderSourceKind::Monitor;
        channel.deliver(&non_agent_delivery).await.unwrap();

        assert_eq!(port.emitted.load(Ordering::Acquire), 0);
        assert_eq!(port.shown.load(Ordering::Acquire), 0);
        assert_eq!(port.focused.load(Ordering::Acquire), 0);
        let groups = port.groups.lock().unwrap();
        assert!(groups.is_empty());
    }
}

use crate::contracts::{AppErrorCode, CommandError, SafeMessageParameters};
use crate::domain::clipboard::CapturedClipboardContent;
use std::borrow::Cow;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub trait ClipboardSource: Send {
    fn read(&mut self) -> Result<Option<CapturedClipboardContent>, ClipboardReadError>;
    fn write_text(&mut self, value: &str) -> Result<(), CommandError>;
    fn write_png(&mut self, png: &[u8]) -> Result<(), CommandError>;
}

pub trait ClipboardSourceFactory: Send + Sync {
    fn open(&self) -> Result<Box<dyn ClipboardSource>, CommandError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardReadError {
    Occupied,
    Unavailable { reason_code: String },
}

pub trait ClipboardRetrySleeper: Send + Sync {
    fn sleep(&self, duration: Duration);
}

pub struct SystemClipboardRetrySleeper;

impl ClipboardRetrySleeper for SystemClipboardRetrySleeper {
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

pub enum ClipboardReadOutcome {
    Content(Option<CapturedClipboardContent>),
    Locked { count: u32 },
    Failed { reason_code: String },
}

pub fn read_with_locked_retry(
    source: &mut dyn ClipboardSource,
    sleeper: &dyn ClipboardRetrySleeper,
) -> ClipboardReadOutcome {
    const DELAYS: [Duration; 3] = [
        Duration::from_millis(25),
        Duration::from_millis(50),
        Duration::from_millis(100),
    ];
    for (index, delay) in DELAYS.into_iter().enumerate() {
        match source.read() {
            Ok(content) => return ClipboardReadOutcome::Content(content),
            Err(ClipboardReadError::Occupied) => {
                sleeper.sleep(delay);
                if index == DELAYS.len() - 1 {
                    return ClipboardReadOutcome::Locked { count: 3 };
                }
            }
            Err(ClipboardReadError::Unavailable { reason_code }) => {
                return ClipboardReadOutcome::Failed { reason_code };
            }
        }
    }
    unreachable!("the bounded retry loop always returns")
}

pub trait ThreadQuitPort: Send + Sync {
    fn post_quit(&self, thread_id: u32) -> Result<(), CommandError>;
}

struct ClipboardListenerStopState {
    thread_id: Option<u32>,
    quit_posted: bool,
}

#[derive(Clone)]
pub struct ClipboardListenerStopSignal {
    cancelled: Arc<AtomicBool>,
    state: Arc<Mutex<ClipboardListenerStopState>>,
    quit: Arc<dyn ThreadQuitPort>,
}

impl ClipboardListenerStopSignal {
    fn new(quit: Arc<dyn ThreadQuitPort>) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            state: Arc::new(Mutex::new(ClipboardListenerStopState {
                thread_id: None,
                quit_posted: false,
            })),
            quit,
        }
    }

    fn bind_thread(&self, thread_id: u32) {
        let mut state = self.state.lock().expect("clipboard stop state poisoned");
        state.thread_id = Some(thread_id);
    }

    fn mark_exited(&self) {
        let mut state = self.state.lock().expect("clipboard stop state poisoned");
        state.thread_id = None;
    }

    pub fn request_stop(&self) -> Result<(), CommandError> {
        self.cancelled.store(true, Ordering::Release);
        let mut state = self.state.lock().expect("clipboard stop state poisoned");
        let Some(thread_id) = state.thread_id else {
            return Ok(());
        };
        if state.quit_posted {
            return Ok(());
        }
        state.quit_posted = true;
        if let Err(error) = self.quit.post_quit(thread_id) {
            state.quit_posted = false;
            return Err(error);
        }
        Ok(())
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub(crate) fn cancelled_flag(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }
}

pub struct ClipboardListenerHandle {
    join: Option<JoinHandle<()>>,
    stop_signal: ClipboardListenerStopSignal,
    #[cfg(windows)]
    window_handle: usize,
}

impl ClipboardListenerHandle {
    pub(crate) fn from_parts(
        thread_id: u32,
        join: JoinHandle<()>,
        quit: Arc<dyn ThreadQuitPort>,
    ) -> Self {
        let stop_signal = ClipboardListenerStopSignal::new(quit);
        stop_signal.bind_thread(thread_id);
        Self {
            join: Some(join),
            stop_signal,
            #[cfg(windows)]
            window_handle: 0,
        }
    }

    fn from_listener_thread(
        join: JoinHandle<()>,
        stop_signal: ClipboardListenerStopSignal,
        #[cfg(windows)] window_handle: usize,
    ) -> Self {
        Self {
            join: Some(join),
            stop_signal,
            #[cfg(windows)]
            window_handle,
        }
    }

    pub fn stop(&mut self) -> Result<(), CommandError> {
        if let Some(join) = self.join.take() {
            if !join.is_finished() {
                if let Err(error) = self.stop_signal.request_stop() {
                    self.join = Some(join);
                    return Err(error);
                }
            }
            join.join().map_err(|_| source_unavailable())?;
            self.stop_signal.mark_exited();
        }
        Ok(())
    }

    pub fn stop_signal(&self) -> ClipboardListenerStopSignal {
        self.stop_signal.clone()
    }

    #[cfg(all(test, windows))]
    fn window_handle(&self) -> windows::Win32::Foundation::HWND {
        windows::Win32::Foundation::HWND(self.window_handle as *mut std::ffi::c_void)
    }
}

impl Drop for ClipboardListenerHandle {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub fn encode_rgba_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, CommandError> {
    let image =
        image::RgbaImage::from_raw(width, height, rgba.to_vec()).ok_or_else(invalid_input)?;
    let mut output = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut output, image::ImageFormat::Png)
        .map_err(|_| invalid_input())?;
    Ok(output.into_inner())
}

pub struct ArboardClipboardSourceFactory;

impl ClipboardSourceFactory for ArboardClipboardSourceFactory {
    fn open(&self) -> Result<Box<dyn ClipboardSource>, CommandError> {
        let clipboard = arboard::Clipboard::new().map_err(|_| source_unavailable())?;
        Ok(Box::new(ArboardClipboardSource {
            clipboard,
            #[cfg(windows)]
            reader: Arc::new(SystemWindowsClipboardReader),
        }))
    }
}

struct ArboardClipboardSource {
    clipboard: arboard::Clipboard,
    #[cfg(windows)]
    reader: Arc<dyn WindowsClipboardReadPort>,
}

impl ClipboardSource for ArboardClipboardSource {
    fn read(&mut self) -> Result<Option<CapturedClipboardContent>, ClipboardReadError> {
        #[cfg(windows)]
        {
            return self.reader.read_once();
        }
        #[cfg(not(windows))]
        match self.clipboard.get_text() {
            Ok(text) => {
                let byte_size = text.len() as u64;
                let sha256 = crate::domain::clipboard::validate_text_capture(&text)
                    .map(|(sha256, _)| sha256)
                    .unwrap_or_default();
                Ok(Some(CapturedClipboardContent::Text {
                    text,
                    sha256,
                    byte_size,
                }))
            }
            Err(arboard::Error::ClipboardOccupied) => Err(ClipboardReadError::Occupied),
            Err(arboard::Error::ContentNotAvailable) => self.read_image(),
            Err(_) => Err(ClipboardReadError::Unavailable {
                reason_code: "textReadFailed".into(),
            }),
        }
    }

    fn write_text(&mut self, value: &str) -> Result<(), CommandError> {
        self.clipboard
            .set_text(value)
            .map_err(|_| source_unavailable())
    }

    fn write_png(&mut self, png: &[u8]) -> Result<(), CommandError> {
        let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .map_err(|_| invalid_input())?
            .to_rgba8();
        self.clipboard
            .set_image(arboard::ImageData {
                width: image.width() as usize,
                height: image.height() as usize,
                bytes: Cow::Owned(image.into_raw()),
            })
            .map_err(|_| source_unavailable())
    }
}

#[cfg(windows)]
trait WindowsClipboardReadPort: Send + Sync {
    fn read_once(&self) -> Result<Option<CapturedClipboardContent>, ClipboardReadError>;
}

#[cfg(windows)]
struct SystemWindowsClipboardReader;

#[cfg(windows)]
enum RawWindowsClipboardContent {
    Text(String),
    Png(Vec<u8>),
    DibV5(Vec<u8>),
    None,
}

#[cfg(windows)]
impl WindowsClipboardReadPort for SystemWindowsClipboardReader {
    fn read_once(&self) -> Result<Option<CapturedClipboardContent>, ClipboardReadError> {
        match read_windows_clipboard_raw_once()? {
            RawWindowsClipboardContent::Text(text) => Ok(Some(captured_text(text))),
            RawWindowsClipboardContent::Png(png) => decode_png_capture(&png),
            RawWindowsClipboardContent::DibV5(dibv5) => decode_dibv5_capture(dibv5),
            RawWindowsClipboardContent::None => Ok(None),
        }
    }
}

#[cfg(windows)]
fn read_windows_clipboard_raw_once() -> Result<RawWindowsClipboardContent, ClipboardReadError> {
    use clipboard_win::{formats, is_format_avail, Clipboard, Getter};

    let _clipboard = Clipboard::new().map_err(|_| ClipboardReadError::Occupied)?;
    if is_format_avail(u32::from(&formats::Unicode)) {
        let mut text = String::new();
        formats::Unicode.read_clipboard(&mut text).map_err(|_| {
            ClipboardReadError::Unavailable {
                reason_code: "textReadFailed".into(),
            }
        })?;
        return Ok(RawWindowsClipboardContent::Text(text));
    }
    if let Some(format) = clipboard_win::register_format("PNG") {
        let format = format.get();
        if is_format_avail(format) {
            let mut png = Vec::new();
            clipboard_win::raw::get_vec(format, &mut png).map_err(|_| {
                ClipboardReadError::Unavailable {
                    reason_code: "imageReadFailed".into(),
                }
            })?;
            return Ok(RawWindowsClipboardContent::Png(png));
        }
    }
    if is_format_avail(clipboard_win::formats::CF_DIBV5) {
        let mut dibv5 = Vec::new();
        clipboard_win::raw::get_vec(clipboard_win::formats::CF_DIBV5, &mut dibv5).map_err(
            |_| ClipboardReadError::Unavailable {
                reason_code: "imageReadFailed".into(),
            },
        )?;
        return Ok(RawWindowsClipboardContent::DibV5(dibv5));
    }
    Ok(RawWindowsClipboardContent::None)
}

#[cfg(windows)]
fn decode_png_capture(png: &[u8]) -> Result<Option<CapturedClipboardContent>, ClipboardReadError> {
    use image::ImageDecoder;

    let decoder = image::codecs::png::PngDecoder::new(Cursor::new(png)).map_err(|_| {
        ClipboardReadError::Unavailable {
            reason_code: "imageReadFailed".into(),
        }
    })?;
    let (width, height) = decoder.dimensions();
    if let Some(oversized) = oversized_image_capture(width, height) {
        return Ok(Some(oversized));
    }
    let image = image::DynamicImage::from_decoder(decoder).map_err(|_| {
        ClipboardReadError::Unavailable {
            reason_code: "imageReadFailed".into(),
        }
    })?;
    captured_image(width, height, image.into_rgba8().into_raw())
}

#[cfg(windows)]
fn decode_dibv5_capture(
    dibv5: Vec<u8>,
) -> Result<Option<CapturedClipboardContent>, ClipboardReadError> {
    use windows::Win32::Graphics::Gdi::{BITMAPV5HEADER, BI_BITFIELDS, BI_RGB};

    if dibv5.len() < std::mem::size_of::<BITMAPV5HEADER>() {
        return Err(image_read_failed());
    }
    let header = unsafe { std::ptr::read_unaligned(dibv5.as_ptr().cast::<BITMAPV5HEADER>()) };
    let header_size = usize::try_from(header.bV5Size).map_err(|_| image_read_failed())?;
    if header_size < std::mem::size_of::<BITMAPV5HEADER>() || header_size > dibv5.len() {
        return Err(image_read_failed());
    }
    let width = u32::try_from(header.bV5Width).map_err(|_| image_read_failed())?;
    let height = header.bV5Height.unsigned_abs();
    if let Some(oversized) = oversized_image_capture(width, height) {
        return Ok(Some(oversized));
    }
    if header.bV5Planes != 1 {
        return Err(image_read_failed());
    }
    if header.bV5BitCount != 32
        || (header.bV5Compression != BI_RGB && header.bV5Compression != BI_BITFIELDS)
    {
        return decode_other_dibv5_capture(dibv5, width, height);
    }
    let row_bytes = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(image_read_failed)?;
    let pixel_bytes = row_bytes
        .checked_mul(usize::try_from(height).map_err(|_| image_read_failed())?)
        .ok_or_else(image_read_failed)?;
    let end = header_size
        .checked_add(pixel_bytes)
        .ok_or_else(image_read_failed)?;
    if end > dibv5.len() {
        return Err(image_read_failed());
    }

    let red_mask = nonzero_mask(header.bV5RedMask, 0x00ff_0000);
    let green_mask = nonzero_mask(header.bV5GreenMask, 0x0000_ff00);
    let blue_mask = nonzero_mask(header.bV5BlueMask, 0x0000_00ff);
    let alpha_mask = header.bV5AlphaMask;
    let mut rgba = vec![0; pixel_bytes];
    for output_row in 0..usize::try_from(height).map_err(|_| image_read_failed())? {
        let source_row = if header.bV5Height > 0 {
            usize::try_from(height).map_err(|_| image_read_failed())? - 1 - output_row
        } else {
            output_row
        };
        for column in 0..usize::try_from(width).map_err(|_| image_read_failed())? {
            let source = header_size + source_row * row_bytes + column * 4;
            let pixel = u32::from_le_bytes(
                dibv5[source..source + 4]
                    .try_into()
                    .map_err(|_| image_read_failed())?,
            );
            let target = output_row * row_bytes + column * 4;
            rgba[target] = masked_channel(pixel, red_mask, 0);
            rgba[target + 1] = masked_channel(pixel, green_mask, 0);
            rgba[target + 2] = masked_channel(pixel, blue_mask, 0);
            rgba[target + 3] = masked_channel(pixel, alpha_mask, 255);
        }
    }
    captured_image(width, height, rgba)
}

#[cfg(windows)]
fn decode_other_dibv5_capture(
    dibv5: Vec<u8>,
    expected_width: u32,
    expected_height: u32,
) -> Result<Option<CapturedClipboardContent>, ClipboardReadError> {
    use image::ImageDecoder;

    let decoder = image::codecs::bmp::BmpDecoder::new_without_file_header(Cursor::new(dibv5))
        .map_err(|_| image_read_failed())?;
    if decoder.dimensions() != (expected_width, expected_height) {
        return Err(image_read_failed());
    }
    let image = image::DynamicImage::from_decoder(decoder).map_err(|_| image_read_failed())?;
    captured_image(
        expected_width,
        expected_height,
        image.into_rgba8().into_raw(),
    )
}

#[cfg(windows)]
fn nonzero_mask(mask: u32, fallback: u32) -> u32 {
    if mask == 0 {
        fallback
    } else {
        mask
    }
}

#[cfg(windows)]
fn masked_channel(pixel: u32, mask: u32, default: u8) -> u8 {
    if mask == 0 {
        return default;
    }
    let shift = mask.trailing_zeros();
    let maximum = mask >> shift;
    let value = (pixel & mask) >> shift;
    ((u64::from(value) * 255) / u64::from(maximum)) as u8
}

#[cfg(windows)]
fn image_read_failed() -> ClipboardReadError {
    ClipboardReadError::Unavailable {
        reason_code: "imageReadFailed".into(),
    }
}

fn oversized_image_capture(width: u32, height: u32) -> Option<CapturedClipboardContent> {
    let rgba_bytes = u64::from(width)
        .checked_mul(u64::from(height))?
        .checked_mul(4)?;
    if width == 0
        || height == 0
        || width > crate::domain::clipboard::MAX_IMAGE_DIMENSION
        || height > crate::domain::clipboard::MAX_IMAGE_DIMENSION
        || rgba_bytes > crate::domain::clipboard::MAX_IMAGE_RGBA_BYTES as u64
    {
        return Some(CapturedClipboardContent::Image {
            png: Vec::new(),
            sha256: String::new(),
            width,
            height,
            byte_size: rgba_bytes,
        });
    }
    None
}

fn captured_text(text: String) -> CapturedClipboardContent {
    let byte_size = text.len() as u64;
    let sha256 = crate::domain::clipboard::validate_text_capture(&text)
        .map(|(sha256, _)| sha256)
        .unwrap_or_default();
    CapturedClipboardContent::Text {
        text,
        sha256,
        byte_size,
    }
}

fn captured_image(
    width: u32,
    height: u32,
    rgba: Vec<u8>,
) -> Result<Option<CapturedClipboardContent>, ClipboardReadError> {
    if let Some(oversized) = oversized_image_capture(width, height) {
        return Ok(Some(oversized));
    }
    if rgba.len() > crate::domain::clipboard::MAX_IMAGE_RGBA_BYTES {
        return Ok(Some(CapturedClipboardContent::Image {
            png: Vec::new(),
            sha256: String::new(),
            width,
            height,
            byte_size: rgba.len() as u64,
        }));
    }
    let png =
        encode_rgba_png(width, height, &rgba).map_err(|_| ClipboardReadError::Unavailable {
            reason_code: "imageEncodeFailed".into(),
        })?;
    let (sha256, byte_size) =
        crate::domain::clipboard::validate_image_capture(width, height, rgba.len(), &png)
            .unwrap_or_else(|_| (String::new(), png.len() as u64));
    Ok(Some(CapturedClipboardContent::Image {
        png,
        sha256,
        width,
        height,
        byte_size,
    }))
}

impl ArboardClipboardSource {
    #[cfg(not(windows))]
    fn read_image(&mut self) -> Result<Option<CapturedClipboardContent>, ClipboardReadError> {
        let image = match self.clipboard.get_image() {
            Ok(image) => image,
            Err(arboard::Error::ClipboardOccupied) => return Err(ClipboardReadError::Occupied),
            Err(arboard::Error::ContentNotAvailable) => return Ok(None),
            Err(_) => {
                return Err(ClipboardReadError::Unavailable {
                    reason_code: "imageReadFailed".into(),
                });
            }
        };
        let width = u32::try_from(image.width).map_err(|_| ClipboardReadError::Unavailable {
            reason_code: "captureTooLarge".into(),
        })?;
        let height = u32::try_from(image.height).map_err(|_| ClipboardReadError::Unavailable {
            reason_code: "captureTooLarge".into(),
        })?;
        captured_image(width, height, image.bytes.into_owned())
    }
}

#[cfg(windows)]
pub struct WindowsThreadQuit;

#[cfg(windows)]
impl ThreadQuitPort for WindowsThreadQuit {
    fn post_quit(&self, thread_id: u32) -> Result<(), CommandError> {
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                thread_id,
                windows::Win32::UI::WindowsAndMessaging::WM_QUIT,
                windows::Win32::Foundation::WPARAM(0),
                windows::Win32::Foundation::LPARAM(0),
            )
        }
        .map_err(|_| source_unavailable())
    }
}

#[cfg(windows)]
thread_local! {
    static CLIPBOARD_CALLBACK: std::cell::RefCell<Option<Box<dyn FnMut()>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(windows)]
unsafe extern "system" fn clipboard_window_proc(
    hwnd: windows::Win32::Foundation::HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    if message == windows::Win32::UI::WindowsAndMessaging::WM_CLIPBOARDUPDATE {
        let mut callback = CLIPBOARD_CALLBACK.with(|slot| slot.borrow_mut().take());
        if let Some(callback) = callback.as_mut() {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback));
            if outcome.is_err() {
                unsafe { windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(1) };
                return windows::Win32::Foundation::LRESULT(0);
            }
        }
        CLIPBOARD_CALLBACK.with(|slot| *slot.borrow_mut() = callback);
        return windows::Win32::Foundation::LRESULT(0);
    }
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

#[cfg(windows)]
pub fn start_message_listener(
    initialize: impl FnOnce(Arc<AtomicBool>) -> Result<Box<dyn FnMut()>, CommandError> + Send + 'static,
) -> Result<ClipboardListenerHandle, CommandError> {
    use windows::core::w;
    use windows::Win32::System::DataExchange::{
        AddClipboardFormatListener, RemoveClipboardFormatListener,
    };
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, RegisterClassW,
        TranslateMessage, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
    };

    let stop_signal = ClipboardListenerStopSignal::new(Arc::new(WindowsThreadQuit));
    let listener_stop_signal = stop_signal.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let join = std::thread::spawn(move || {
        let thread_id = unsafe { GetCurrentThreadId() };
        listener_stop_signal.bind_thread(thread_id);
        let callback = match initialize(listener_stop_signal.cancelled_flag()) {
            Ok(callback) => callback,
            Err(error) => {
                let _ = ready_tx.send(Err(error));
                listener_stop_signal.mark_exited();
                return;
            }
        };
        CLIPBOARD_CALLBACK.with(|slot| *slot.borrow_mut() = Some(callback));
        let class_name = w!("AIslandClipboardMessageWindow");
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(clipboard_window_proc),
            lpszClassName: class_name,
            ..Default::default()
        };
        unsafe {
            RegisterClassW(&window_class);
        }
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                w!("AIslandClipboardMessageWindow"),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        };
        let Ok(window) = window else {
            let _ = ready_tx.send(Err(source_unavailable()));
            CLIPBOARD_CALLBACK.with(|slot| *slot.borrow_mut() = None);
            listener_stop_signal.mark_exited();
            return;
        };
        if unsafe { AddClipboardFormatListener(window) }.is_err() {
            let _ = ready_tx.send(Err(source_unavailable()));
            let _ = unsafe { DestroyWindow(window) };
            CLIPBOARD_CALLBACK.with(|slot| *slot.borrow_mut() = None);
            listener_stop_signal.mark_exited();
            return;
        }
        let _ = ready_tx.send(Ok((thread_id, window.0 as usize)));
        let mut message = MSG::default();
        loop {
            let result = unsafe { GetMessageW(&mut message, None, 0, 0) }.0;
            if result <= 0 {
                break;
            }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let _ = unsafe { RemoveClipboardFormatListener(window) };
        let _ = unsafe { DestroyWindow(window) };
        CLIPBOARD_CALLBACK.with(|slot| *slot.borrow_mut() = None);
        listener_stop_signal.mark_exited();
    });
    let (_thread_id, window_handle) = ready_rx.recv().map_err(|_| source_unavailable())??;
    Ok(ClipboardListenerHandle::from_listener_thread(
        join,
        stop_signal,
        window_handle,
    ))
}

#[cfg(windows)]
pub fn foreground_process_basename() -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, MAX_PATH};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    let window = unsafe { GetForegroundWindow() };
    if window.0.is_null() {
        return None;
    }
    let mut process_id = 0_u32;
    unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) };
    if process_id == 0 {
        return None;
    }
    let process =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }.ok()?;
    let mut buffer = vec![0_u16; MAX_PATH as usize];
    let mut length = buffer.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    result.ok()?;
    let path = String::from_utf16(&buffer[..length as usize]).ok()?;
    std::path::Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

fn invalid_input() -> CommandError {
    CommandError {
        code: AppErrorCode::InvalidInput,
        message_key: "errors.invalidInput".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

fn source_unavailable() -> CommandError {
    CommandError {
        code: AppErrorCode::SourceUnavailable,
        message_key: "errors.sourceUnavailable".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        encode_rgba_png, read_with_locked_retry, ClipboardListenerHandle, ClipboardReadError,
        ClipboardReadOutcome, ClipboardRetrySleeper, ClipboardSource, ThreadQuitPort,
    };
    use crate::contracts::{CommandError, SafeMessageParameters};
    use crate::domain::clipboard::CapturedClipboardContent;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct LockedSource {
        remaining: usize,
    }

    impl ClipboardSource for LockedSource {
        fn read(&mut self) -> Result<Option<CapturedClipboardContent>, ClipboardReadError> {
            if self.remaining > 0 {
                self.remaining -= 1;
                Err(ClipboardReadError::Occupied)
            } else {
                Ok(None)
            }
        }

        fn write_text(&mut self, _value: &str) -> Result<(), CommandError> {
            Ok(())
        }

        fn write_png(&mut self, _png: &[u8]) -> Result<(), CommandError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingSleeper(Mutex<Vec<Duration>>);

    impl ClipboardRetrySleeper for RecordingSleeper {
        fn sleep(&self, duration: Duration) {
            self.0.lock().unwrap().push(duration);
        }
    }

    #[test]
    fn occupied_reads_use_exact_bounded_delays_then_report_three_locks() {
        let mut source = LockedSource { remaining: 3 };
        let sleeper = RecordingSleeper::default();
        let outcome = read_with_locked_retry(&mut source, &sleeper);
        assert!(matches!(outcome, ClipboardReadOutcome::Locked { count: 3 }));
        assert_eq!(
            sleeper.0.lock().unwrap().as_slice(),
            [
                Duration::from_millis(25),
                Duration::from_millis(50),
                Duration::from_millis(100),
            ]
        );
    }

    struct FailedSource {
        reads: usize,
    }

    impl ClipboardSource for FailedSource {
        fn read(&mut self) -> Result<Option<CapturedClipboardContent>, ClipboardReadError> {
            self.reads += 1;
            Err(ClipboardReadError::Unavailable {
                reason_code: "readFailed".into(),
            })
        }

        fn write_text(&mut self, _value: &str) -> Result<(), CommandError> {
            Ok(())
        }

        fn write_png(&mut self, _png: &[u8]) -> Result<(), CommandError> {
            Ok(())
        }
    }

    #[test]
    fn unavailable_reads_are_not_retried_or_slept() {
        let mut source = FailedSource { reads: 0 };
        let sleeper = RecordingSleeper::default();
        let outcome = read_with_locked_retry(&mut source, &sleeper);
        assert!(matches!(
            outcome,
            ClipboardReadOutcome::Failed { ref reason_code } if reason_code == "readFailed"
        ));
        assert_eq!(source.reads, 1);
        assert!(sleeper.0.lock().unwrap().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn production_source_delegates_each_outer_attempt_to_one_windows_open() {
        struct OccupiedOnce(std::sync::atomic::AtomicUsize);

        impl super::WindowsClipboardReadPort for OccupiedOnce {
            fn read_once(
                &self,
            ) -> Result<
                Option<crate::domain::clipboard::CapturedClipboardContent>,
                ClipboardReadError,
            > {
                self.0.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                Err(ClipboardReadError::Occupied)
            }
        }

        let reader = Arc::new(OccupiedOnce(std::sync::atomic::AtomicUsize::new(0)));
        let mut source = super::ArboardClipboardSource {
            clipboard: arboard::Clipboard::new().unwrap(),
            reader: reader.clone(),
        };
        assert!(matches!(source.read(), Err(ClipboardReadError::Occupied)));
        assert_eq!(reader.0.load(std::sync::atomic::Ordering::Acquire), 1);
    }

    #[cfg(windows)]
    #[test]
    fn png_only_and_dibv5_alpha_fixtures_decode_losslessly() {
        let rgba = [
            255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 64, 255, 255, 255, 0,
        ];
        let png = encode_rgba_png(2, 2, &rgba).unwrap();
        let decoded_png = super::decode_png_capture(&png).unwrap().unwrap();
        assert_captured_rgba(decoded_png, 2, 2, &rgba);

        let decoded_dibv5 = super::decode_dibv5_capture(dibv5_fixture(2, 2, &rgba))
            .unwrap()
            .unwrap();
        assert_captured_rgba(decoded_dibv5, 2, 2, &rgba);
    }

    #[cfg(windows)]
    #[test]
    fn dibv5_24_bit_bi_rgb_with_dword_padding_decodes_losslessly() {
        let rgba = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let decoded = super::decode_dibv5_capture(dibv5_24_bit_fixture(2, 2, &rgba))
            .unwrap()
            .unwrap();
        assert_captured_rgba(decoded, 2, 2, &rgba);
    }

    #[cfg(windows)]
    #[test]
    fn oversized_dibv5_is_rejected_from_its_header_before_pixel_allocation() {
        let capture = super::decode_dibv5_capture(dibv5_fixture(8_193, 1, &[]))
            .unwrap()
            .unwrap();
        match capture {
            CapturedClipboardContent::Image {
                png,
                width,
                height,
                byte_size,
                ..
            } => {
                assert!(png.is_empty());
                assert_eq!((width, height), (8_193, 1));
                assert_eq!(byte_size, 8_193 * 4);
            }
            CapturedClipboardContent::Text { .. } => panic!("expected oversized image"),
        }
    }

    #[cfg(windows)]
    fn assert_captured_rgba(
        capture: CapturedClipboardContent,
        width: u32,
        height: u32,
        expected: &[u8],
    ) {
        match capture {
            CapturedClipboardContent::Image {
                png,
                width: actual_width,
                height: actual_height,
                ..
            } => {
                assert_eq!((actual_width, actual_height), (width, height));
                assert_eq!(
                    image::load_from_memory_with_format(&png, image::ImageFormat::Png)
                        .unwrap()
                        .to_rgba8()
                        .as_raw(),
                    expected
                );
            }
            CapturedClipboardContent::Text { .. } => panic!("expected image capture"),
        }
    }

    #[cfg(windows)]
    fn dibv5_fixture(width: i32, height: i32, rgba: &[u8]) -> Vec<u8> {
        use windows::Win32::Graphics::Gdi::{BITMAPV5HEADER, BI_BITFIELDS};

        let header = BITMAPV5HEADER {
            bV5Size: std::mem::size_of::<BITMAPV5HEADER>() as u32,
            bV5Width: width,
            bV5Height: height,
            bV5Planes: 1,
            bV5BitCount: 32,
            bV5Compression: BI_BITFIELDS,
            bV5SizeImage: rgba.len() as u32,
            bV5RedMask: 0x00ff0000,
            bV5GreenMask: 0x0000ff00,
            bV5BlueMask: 0x000000ff,
            bV5AlphaMask: 0xff000000,
            bV5CSType: 0x7352_4742,
            bV5Intent: 4,
            ..Default::default()
        };
        let mut data = unsafe {
            std::slice::from_raw_parts(
                (&header as *const BITMAPV5HEADER).cast::<u8>(),
                std::mem::size_of::<BITMAPV5HEADER>(),
            )
            .to_vec()
        };
        if width > 0 && height > 0 {
            let row_bytes = width as usize * 4;
            for row in rgba.chunks_exact(row_bytes).rev() {
                for pixel in row.chunks_exact(4) {
                    data.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                }
            }
        }
        data
    }

    #[cfg(windows)]
    fn dibv5_24_bit_fixture(width: i32, height: i32, rgba: &[u8]) -> Vec<u8> {
        use windows::Win32::Graphics::Gdi::{BITMAPV5HEADER, BI_RGB};

        let row_pixels = width as usize * 3;
        let row_bytes = (row_pixels + 3) & !3;
        let header = BITMAPV5HEADER {
            bV5Size: std::mem::size_of::<BITMAPV5HEADER>() as u32,
            bV5Width: width,
            bV5Height: height,
            bV5Planes: 1,
            bV5BitCount: 24,
            bV5Compression: BI_RGB,
            bV5SizeImage: (row_bytes * height.unsigned_abs() as usize) as u32,
            bV5CSType: 0x7352_4742,
            bV5Intent: 4,
            ..Default::default()
        };
        let mut data = unsafe {
            std::slice::from_raw_parts(
                (&header as *const BITMAPV5HEADER).cast::<u8>(),
                std::mem::size_of::<BITMAPV5HEADER>(),
            )
            .to_vec()
        };
        for row in rgba.chunks_exact(width as usize * 4).rev() {
            for pixel in row.chunks_exact(4) {
                data.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
            }
            data.resize(data.len() + (row_bytes - row_pixels), 0);
        }
        data
    }

    struct RecordingQuit {
        calls: Mutex<Vec<u32>>,
        release: Mutex<Option<std::sync::mpsc::SyncSender<()>>>,
    }

    impl ThreadQuitPort for RecordingQuit {
        fn post_quit(&self, thread_id: u32) -> Result<(), CommandError> {
            self.calls.lock().unwrap().push(thread_id);
            if let Some(release) = self.release.lock().unwrap().take() {
                release.send(()).unwrap();
            }
            Ok(())
        }
    }

    #[test]
    fn stop_posts_one_quit_and_joins_once() {
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let quit = Arc::new(RecordingQuit {
            calls: Mutex::new(Vec::new()),
            release: Mutex::new(Some(release_tx)),
        });
        let join = std::thread::spawn(move || release_rx.recv().unwrap());
        let mut handle = ClipboardListenerHandle::from_parts(42, join, quit.clone());
        let stop_signal = handle.stop_signal();
        stop_signal.request_stop().unwrap();
        handle.stop().unwrap();
        stop_signal.request_stop().unwrap();
        handle.stop().unwrap();
        assert_eq!(quit.calls.lock().unwrap().as_slice(), [42]);
        assert!(stop_signal.is_cancelled());
    }

    #[test]
    fn rgba_fixture_encodes_to_a_reopenable_lossless_png() {
        let rgba = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 128,
        ];
        let png = encode_rgba_png(2, 2, &rgba).unwrap();
        let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
        assert_eq!(decoded.as_raw(), &rgba);
    }

    #[cfg(windows)]
    #[test]
    fn message_only_listener_starts_and_stops_without_polling() {
        let mut listener = super::start_message_listener(|_| Ok(Box::new(|| {}))).unwrap();
        listener.stop().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn message_only_listener_dispatches_clipboard_updates_to_the_callback() {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLIPBOARDUPDATE};

        let (called_tx, called_rx) = std::sync::mpsc::sync_channel(1);
        let mut listener = super::start_message_listener(move |_| {
            Ok(Box::new(move || {
                called_tx.send(()).unwrap();
            }))
        })
        .unwrap();
        let window = listener.window_handle();
        unsafe {
            PostMessageW(Some(window), WM_CLIPBOARDUPDATE, WPARAM(0), LPARAM(0)).unwrap();
        }

        called_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the real message loop dropped WM_CLIPBOARDUPDATE");
        listener.stop().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn clipboard_source_initializer_runs_on_the_dedicated_listener_thread() {
        let caller = std::thread::current().id();
        let (initialized_tx, initialized_rx) = std::sync::mpsc::sync_channel(1);
        let mut listener = super::start_message_listener(move |_| {
            initialized_tx.send(std::thread::current().id()).unwrap();
            Ok(Box::new(|| {}))
        })
        .unwrap();

        assert_ne!(initialized_rx.recv().unwrap(), caller);
        listener.stop().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn panicking_clipboard_callback_is_contained_and_stops_the_listener() {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLIPBOARDUPDATE};

        let mut listener = super::start_message_listener(|_| {
            Ok(Box::new(|| panic!("clipboard callback panic fixture")))
        })
        .unwrap();
        let window = listener.window_handle();
        unsafe {
            PostMessageW(Some(window), WM_CLIPBOARDUPDATE, WPARAM(0), LPARAM(0)).unwrap();
        }

        listener.stop().unwrap();
    }

    fn _safe_error_fixture() -> CommandError {
        CommandError {
            code: crate::contracts::AppErrorCode::SourceUnavailable,
            message_key: "errors.sourceUnavailable".into(),
            details: SafeMessageParameters::new(),
            retryable: false,
        }
    }
}

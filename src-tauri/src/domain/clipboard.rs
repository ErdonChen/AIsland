use crate::contracts::{
    AppErrorCode, ClipboardContentKind, ClipboardContentKindFilter, ClipboardItem, CommandError,
    SafeMessageParameters,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const MAX_TEXT_BYTES: usize = 1_048_576;
pub const MAX_IMAGE_DIMENSION: u32 = 8_192;
pub const MAX_IMAGE_RGBA_BYTES: usize = 33_554_432;
pub const MAX_IMAGE_PNG_BYTES: usize = 20_971_520;
pub const MAX_UNPINNED_ITEMS: u32 = 500;

pub type ClipboardListKind = ClipboardContentKindFilter;

pub enum CapturedClipboardContent {
    Text {
        text: String,
        sha256: String,
        byte_size: u64,
    },
    Image {
        png: Vec<u8>,
        sha256: String,
        width: u32,
        height: u32,
        byte_size: u64,
    },
}

pub struct CaptureOutcome {
    pub item: ClipboardItem,
    pub inserted: bool,
    pub removed_asset_names: Vec<String>,
}

pub struct NewClipboardAsset {
    pub id: Uuid,
    pub asset_name: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub byte_size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardAssetRecord {
    pub id: Uuid,
    pub clipboard_item_id: Uuid,
    pub asset_name: String,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
    pub byte_size: u64,
}

pub trait ClipboardRetentionPolicy: Send + Sync {
    fn unpinned_limit(&self) -> Result<u32, CommandError>;
}

pub struct BootstrapClipboardRetentionPolicy;

impl ClipboardRetentionPolicy for BootstrapClipboardRetentionPolicy {
    fn unpinned_limit(&self) -> Result<u32, CommandError> {
        Ok(MAX_UNPINNED_ITEMS)
    }
}

pub fn validate_text_capture(text: &str) -> Result<(String, u64), CommandError> {
    let bytes = text.as_bytes();
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(invalid_input());
    }
    Ok((sha256_hex(bytes), bytes.len() as u64))
}

pub fn validate_image_capture(
    width: u32,
    height: u32,
    rgba_bytes: usize,
    png: &[u8],
) -> Result<(String, u64), CommandError> {
    if width == 0
        || height == 0
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || rgba_bytes == 0
        || rgba_bytes > MAX_IMAGE_RGBA_BYTES
        || png.is_empty()
        || png.len() > MAX_IMAGE_PNG_BYTES
    {
        return Err(invalid_input());
    }
    Ok((sha256_hex(png), png.len() as u64))
}

pub fn validate_sha256_hex(value: &str) -> Result<(), CommandError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(invalid_input())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn content_kind_name(kind: ClipboardContentKind) -> &'static str {
    match kind {
        ClipboardContentKind::Text => "text",
        ClipboardContentKind::Image => "image",
    }
}

fn invalid_input() -> CommandError {
    CommandError {
        code: AppErrorCode::InvalidInput,
        message_key: "errors.invalidInput".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::{validate_image_capture, validate_text_capture, MAX_TEXT_BYTES};
    use crate::contracts::AppErrorCode;

    const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn text_capture_hashes_utf8_bytes_and_enforces_the_byte_boundary() {
        assert_eq!(
            validate_text_capture("abc").unwrap(),
            (SHA256_ABC.into(), 3)
        );

        let exact = "🧊".repeat(MAX_TEXT_BYTES / "🧊".len());
        assert_eq!(
            validate_text_capture(&exact).unwrap().1,
            MAX_TEXT_BYTES as u64
        );
        assert_eq!(
            validate_text_capture(&(exact + "🧊")).unwrap_err().code,
            AppErrorCode::InvalidInput
        );
    }

    #[test]
    fn image_capture_hashes_the_encoded_png_bytes() {
        assert_eq!(
            validate_image_capture(1, 1, 4, b"abc").unwrap(),
            (SHA256_ABC.into(), 3)
        );
    }
}

//! Cất API token/key của trợ lý AI trong Windows Credential Manager (Trình quản lý thông tin
//! xác thực), không ghi vào config.toml: Windows mã hóa theo tài khoản người dùng, và key
//! không lộ ra nếu ai đó mở hay chia sẻ file cấu hình.
//! Mỗi nhà cung cấp một mục riêng, đổi qua lại không mất key của bên kia.
//! Xem/xóa bằng tay: Control Panel > Credential Manager > Windows Credentials > "DynamicIsland/…".

use windows::{
    Win32::Security::Credentials::{
        CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW,
        CredFree, CredReadW, CredWriteW,
    },
    core::{HSTRING, PWSTR},
};

use crate::config::AiProvider;

/// (tên mục trong Credential Manager, tên người dùng hiện ở đó)
fn target(provider: AiProvider) -> (&'static str, &'static str) {
    match provider {
        AiProvider::Cloudflare => ("DynamicIsland/CloudflareAIToken", "Cloudflare API token"),
        AiProvider::Claude => ("DynamicIsland/ClaudeAPIKey", "Claude API key"),
    }
}

/// Token/key đã lưu của nhà cung cấp (None nếu chưa có)
pub fn load_token(provider: AiProvider) -> Option<String> {
    let (name, _) = target(provider);
    unsafe {
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        CredReadW(&HSTRING::from(name), CRED_TYPE_GENERIC, None, &mut cred).ok()?;
        let c = &*cred;
        let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
        // Lưu dạng UTF-16 như các app Windows khác
        let wide: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| u16::from_le_bytes(b))
            .collect();
        CredFree(cred.cast());
        let token = String::from_utf16(&wide).ok()?;
        (!token.trim().is_empty()).then_some(token)
    }
}

/// Lưu (hoặc thay) token/key của nhà cung cấp
pub fn save_token(provider: AiProvider, token: &str) -> windows::core::Result<()> {
    let (name, user) = target(provider);
    let mut blob: Vec<u8> = token
        .trim()
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut target: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let mut user: Vec<u16> = user.encode_utf16().chain([0]).collect();
    let cred = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user.as_mut_ptr()),
        ..Default::default()
    };
    unsafe { CredWriteW(&cred, 0) }
}

/// Xóa token/key của nhà cung cấp (không có thì thôi)
pub fn delete_token(provider: AiProvider) {
    let (name, _) = target(provider);
    unsafe {
        let _ = CredDeleteW(&HSTRING::from(name), CRED_TYPE_GENERIC, None);
    }
}

/// Nhà cung cấp này đã có token/key chưa
pub fn has_token(provider: AiProvider) -> bool {
    load_token(provider).is_some()
}

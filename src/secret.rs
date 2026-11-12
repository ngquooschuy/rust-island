//! Cất API token của Cloudflare trong Windows Credential Manager (Trình quản lý thông tin
//! xác thực), không ghi vào config.toml: Windows mã hóa theo tài khoản người dùng, và token
//! không lộ ra nếu ai đó mở hay chia sẻ file cấu hình.
//! Xem/xóa bằng tay: Control Panel > Credential Manager > Windows Credentials > "DynamicIsland/…".

use windows::{
    Win32::Security::Credentials::{
        CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW,
        CredFree, CredReadW, CredWriteW,
    },
    core::{HSTRING, PWSTR},
};

/// Tên mục trong Credential Manager
const TARGET: &str = "DynamicIsland/CloudflareAIToken";

/// Token đã lưu (None nếu chưa có)
pub fn load_token() -> Option<String> {
    unsafe {
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        CredReadW(&HSTRING::from(TARGET), CRED_TYPE_GENERIC, None, &mut cred).ok()?;
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

/// Lưu (hoặc thay) token
pub fn save_token(token: &str) -> windows::core::Result<()> {
    let mut blob: Vec<u8> = token
        .trim()
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut target: Vec<u16> = TARGET.encode_utf16().chain([0]).collect();
    let mut user: Vec<u16> = "Cloudflare API token".encode_utf16().chain([0]).collect();
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

/// Xóa token (không có thì thôi)
pub fn delete_token() {
    unsafe {
        let _ = CredDeleteW(&HSTRING::from(TARGET), CRED_TYPE_GENERIC, None);
    }
}

/// Đã có token chưa
pub fn has_token() -> bool {
    load_token().is_some()
}

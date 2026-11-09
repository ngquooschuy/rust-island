//! Tự khởi động cùng Windows.
//! Dùng lệnh `reg` có sẵn của Windows để ghi vào khóa Run của người dùng hiện tại.

use std::os::windows::process::CommandExt;
use std::process::Command;

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const APP_NAME: &str = "DynamicIsland";
const CREATE_NO_WINDOW: u32 = 0x0800_0000; // không bật cửa sổ console khi chạy lệnh

fn reg(args: &[&str]) -> bool {
    Command::new("reg")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn is_enabled() -> bool {
    reg(&["query", RUN_KEY, "/v", APP_NAME])
}

pub fn set(enable: bool) {
    if enable {
        if let Ok(exe) = std::env::current_exe() {
            // Bọc đường dẫn trong dấu ngoặc kép để an toàn khi có khoảng trắng
            let value = format!("\"{}\"", exe.display());
            reg(&[
                "add", RUN_KEY, "/v", APP_NAME, "/t", "REG_SZ", "/d", &value, "/f",
            ]);
        }
    } else {
        reg(&["delete", RUN_KEY, "/v", APP_NAME, "/f"]);
    }
}

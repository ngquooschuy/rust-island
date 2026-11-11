//! Nhúng vào file .exe:
//! - manifest để control của Windows (nút, ô đánh dấu, thanh trượt trong cửa sổ Cài đặt)
//!   dùng giao diện hiện đại thay vì kiểu Windows 95;
//! - icon của app (assets/app.rc → assets/icon.ico), biên dịch bằng rc.exe của Windows SDK.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// rc.exe mới nhất trong Windows SDK (hoặc đường dẫn đặt trong biến môi trường RC)
fn find_rc() -> Option<PathBuf> {
    if let Some(rc) = env::var_os("RC") {
        return Some(PathBuf::from(rc));
    }
    let host = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };
    let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut found: Vec<PathBuf> = std::fs::read_dir(kits)
        .ok()?
        .flatten()
        .map(|e| e.path().join(host).join("rc.exe"))
        .filter(|p| p.exists())
        .collect();
    // Thư mục đặt tên theo phiên bản SDK (10.0.26100.0...): sắp xếp rồi lấy bản mới nhất
    found.sort();
    found.pop()
}

fn embed_icon() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("app.res");
    let Some(rc) = find_rc() else {
        println!("cargo:warning=Không tìm thấy rc.exe (Windows SDK): file .exe sẽ không có icon");
        return;
    };
    let status = Command::new(&rc)
        .args(["/nologo", "/fo"])
        .arg(&out)
        .arg("assets/app.rc")
        .status();
    match status {
        Ok(s) if s.success() => {
            // link.exe nhận thẳng file .res như một đầu vào
            println!("cargo:rustc-link-arg-bins={}", out.display());
        }
        _ => println!(
            "cargo:warning=rc.exe không biên dịch được assets/app.rc: file .exe sẽ không có icon"
        ),
    }
}

fn main() {
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTDEPENDENCY:type='win32' \
             name='Microsoft.Windows.Common-Controls' version='6.0.0.0' \
             processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
        embed_icon();
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-env-changed=RC");
}

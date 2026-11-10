//! Nhúng manifest vào file .exe để control của Windows (nút, ô đánh dấu, thanh trượt
//! trong cửa sổ Cài đặt) dùng giao diện hiện đại thay vì kiểu Windows 95.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTDEPENDENCY:type='win32' \
             name='Microsoft.Windows.Common-Controls' version='6.0.0.0' \
             processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
    println!("cargo:rerun-if-changed=build.rs");
}

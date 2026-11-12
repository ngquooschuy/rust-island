//! HTTP nhỏ gọn bằng WinHTTP có sẵn trong Windows: GET cho thời tiết và lời bài hát,
//! POST JSON cho trợ lý AI.
//!
//! Không dùng HttpClient của WinRT: với ứng dụng desktop không đóng gói nó báo
//! lỗi chứng chỉ 0x80072F8F, trong khi WinHTTP dùng chung tầng TLS với cả hệ thống.

use std::ffi::c_void;

use windows::{
    Win32::Networking::WinHttp::*,
    core::{HSTRING, PCWSTR, w},
};

/// Chờ trợ lý AI trả lời tối đa bao lâu (mili giây)
const AI_RECEIVE_TIMEOUT_MS: i32 = 90_000;

/// Handle của WinHTTP, tự đóng khi ra khỏi phạm vi (RAII, giống `File` của Rust)
struct Handle(*mut c_void);

impl Handle {
    /// WinHTTP trả về con trỏ null khi lỗi; lý do nằm ở GetLastError
    fn new(raw: *mut c_void) -> Result<Self, String> {
        if raw.is_null() {
            Err(windows::core::Error::from_thread().to_string())
        } else {
            Ok(Self(raw))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

/// Một phiên WinHTTP; mở một lần rồi dùng cho nhiều request trên cùng luồng
pub struct Session(Handle);

/// Kết quả một request: mã trạng thái và nội dung (đã đọc hết)
pub struct Response {
    pub status: u32,
    pub body: String,
}

impl Response {
    /// Nội dung nếu máy chủ trả 200, ngược lại là lỗi
    pub fn ok(self) -> Result<String, String> {
        if self.status == 200 {
            Ok(self.body)
        } else {
            Err(format!("máy chủ trả về HTTP {}", self.status))
        }
    }
}

impl Session {
    pub fn open() -> Result<Self, String> {
        let handle = Handle::new(unsafe {
            WinHttpOpen(
                // User-Agent: LRCLIB đề nghị ứng dụng tự giới thiệu tên và trang chủ
                w!("DynamicIsland/0.1 (https://github.com/ngquooschuy/rust-island)"),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, // tự dùng proxy của hệ thống nếu có
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            )
        })?;
        // Phân giải tên, kết nối, gửi, nhận (mili giây)
        unsafe {
            let _ = WinHttpSetTimeouts(handle.0, 10_000, 10_000, 10_000, 15_000);
        }
        Ok(Self(handle))
    }

    /// GET một địa chỉ https; trả về cả khi máy chủ báo lỗi (404...) để bên gọi tự xử lý
    pub fn get(&self, url: &str) -> Result<Response, String> {
        self.send(w!("GET"), url, "", &[], None)
    }

    /// POST nội dung JSON kèm header Authorization. AI cần thời gian "suy nghĩ" nên chờ
    /// câu trả lời lâu hơn GET thường
    pub fn post_json(
        &self,
        url: &str,
        authorization: &str,
        json: &str,
    ) -> Result<Response, String> {
        let headers =
            format!("Content-Type: application/json\r\nAuthorization: {authorization}\r\n");
        self.send(
            w!("POST"),
            url,
            &headers,
            json.as_bytes(),
            Some(AI_RECEIVE_TIMEOUT_MS),
        )
    }

    /// Gửi một request https; `headers` dạng "Tên: giá trị\r\n..." (rỗng = không thêm)
    fn send(
        &self,
        method: PCWSTR,
        url: &str,
        headers: &str,
        body: &[u8],
        receive_timeout_ms: Option<i32>,
    ) -> Result<Response, String> {
        let (host, path) = split_url(url).ok_or("URL không hợp lệ")?;
        let err = |e: windows::core::Error| e.to_string();
        unsafe {
            // Thứ tự khai báo quan trọng: Rust drop ngược lại, nên request đóng trước connect
            let connect = Handle::new(WinHttpConnect(
                (self.0).0,
                &HSTRING::from(host),
                INTERNET_DEFAULT_HTTPS_PORT,
                0,
            ))?;
            let request = Handle::new(WinHttpOpenRequest(
                connect.0,
                method,
                &HSTRING::from(path),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ))?;
            if let Some(ms) = receive_timeout_ms {
                let _ = WinHttpSetTimeouts(request.0, 10_000, 10_000, 30_000, ms);
            }
            let headers: Vec<u16> = headers.encode_utf16().collect();
            let headers = (!headers.is_empty()).then_some(headers.as_slice());
            let length = body.len() as u32;
            let data = (!body.is_empty()).then_some(body.as_ptr().cast::<c_void>());
            WinHttpSendRequest(request.0, headers, data, length, length, 0).map_err(err)?;
            WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(err)?;

            let mut status = 0u32;
            let mut size = std::mem::size_of::<u32>() as u32;
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some((&raw mut status).cast()),
                &mut size,
                std::ptr::null_mut(),
            )
            .map_err(err)?;

            // Đọc thân phản hồi từng khúc cho tới khi hết
            let mut body = Vec::new();
            loop {
                let mut available = 0u32;
                WinHttpQueryDataAvailable(request.0, &mut available).map_err(err)?;
                if available == 0 {
                    break;
                }
                let start = body.len();
                body.resize(start + available as usize, 0u8);
                let mut read = 0u32;
                WinHttpReadData(
                    request.0,
                    body[start..].as_mut_ptr().cast(),
                    available,
                    &mut read,
                )
                .map_err(err)?;
                body.truncate(start + read as usize);
            }
            let body = String::from_utf8(body).map_err(|e| e.to_string())?;
            Ok(Response { status, body })
        }
    }
}

/// Mã hóa chuỗi để đặt vào URL: "Hà Nội" → "H%C3%A0%20N%E1%BB%99i"
pub fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// "https://host/path?query" → ("host", "/path?query")
fn split_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("https://")?;
    let slash = rest.find('/')?;
    Some(rest.split_at(slash))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_handles_vietnamese_and_spaces() {
        assert_eq!(percent_encode("Hà Nội"), "H%C3%A0%20N%E1%BB%99i");
        assert_eq!(percent_encode("Da-Nang_1.~"), "Da-Nang_1.~");
        assert_eq!(percent_encode("a,b&c"), "a%2Cb%26c");
    }

    #[test]
    fn split_url_separates_host_and_path() {
        assert_eq!(
            split_url("https://api.open-meteo.com/v1/forecast?a=1"),
            Some(("api.open-meteo.com", "/v1/forecast?a=1"))
        );
        assert_eq!(split_url("http://example.com/"), None); // chỉ nhận https
        assert_eq!(split_url("https://example.com"), None); // thiếu đường dẫn
    }

    #[test]
    fn non_200_response_is_an_error() {
        let r = Response {
            status: 404,
            body: String::new(),
        };
        assert_eq!(r.ok().unwrap_err(), "máy chủ trả về HTTP 404");
    }
}

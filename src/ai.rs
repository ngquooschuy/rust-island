//! Trợ lý AI cho Tra cứu nhanh: gọi Workers AI của Cloudflare (model gpt-oss của OpenAI).
//!
//! Người dùng tự cấu hình Account ID (trong config.toml) và API token (cất trong Windows
//! Credential Manager, xem secret.rs). Câu hỏi và vài lượt chat gần nhất được gửi tới
//! `https://api.cloudflare.com/client/v4/accounts/<ID>/ai/run/<model>`.
//!
//! Tài liệu của Cloudflare không mô tả dạng kết quả của gpt-oss, nên `parse_reply` nhận cả
//! ba dạng hay gặp: `result.response`, `result.choices[0].message.content` (kiểu Chat
//! Completions) và `result.output[]` (kiểu Responses API của OpenAI).

use serde::Serialize;
use serde_json::Value;

use crate::http::Session;

/// Các model chọn được: (mã model, tên hiện trong Cài đặt)
pub const MODELS: [(&str, &str); 2] = [
    ("@cf/openai/gpt-oss-20b", "GPT-OSS 20B (nhanh, rẻ)"),
    ("@cf/openai/gpt-oss-120b", "GPT-OSS 120B (thông minh hơn)"),
];

/// Lời dặn mặc định cho trợ lý (người dùng sửa được trong Cài đặt)
pub const DEFAULT_SYSTEM_PROMPT: &str = "Bạn là trợ lý thân thiện trên Dynamic Island của Windows. \
Trả lời ngắn gọn, rõ ràng, bằng ngôn ngữ người dùng đang dùng (mặc định tiếng Việt). \
Khi được hỏi một từ hoặc cụm từ, giải thích nghĩa trong 2–4 câu, có ví dụ nếu cần. \
Không dùng bảng hay định dạng Markdown phức tạp.";

/// gpt-oss "suy nghĩ" trước khi trả lời và phần đó cũng tính vào giới hạn: mặc định của
/// Cloudflare (256) dễ làm câu trả lời bị cụt, nên đặt cao hơn
const MAX_TOKENS: u32 = 2048;
/// Chỉ gửi kèm chừng này lượt hỏi-đáp gần nhất (đủ để hiểu ngữ cảnh, không tốn token)
pub const CONTEXT_TURNS: usize = 6;

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Message {
    pub role: &'static str,
    pub content: String,
}

impl Message {
    pub fn user(text: &str) -> Self {
        Self {
            role: "user",
            content: text.to_string(),
        }
    }

    pub fn assistant(text: &str) -> Self {
        Self {
            role: "assistant",
            content: text.to_string(),
        }
    }
}

#[derive(Serialize)]
struct Request {
    messages: Vec<Message>,
    max_tokens: u32,
}

/// Thông tin cần để gọi AI
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub account_id: String,
    pub token: String,
    pub model: String,
    pub system_prompt: String,
}

pub fn url(account_id: &str, model: &str) -> String {
    format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/ai/run/{model}",
        account_id.trim()
    )
}

/// Thân request: lời dặn hệ thống rồi tới các lượt chat (lượt cuối là câu đang hỏi)
pub fn request_body(system_prompt: &str, chat: &[Message]) -> String {
    let mut messages = Vec::with_capacity(chat.len() + 1);
    if !system_prompt.trim().is_empty() {
        messages.push(Message {
            role: "system",
            content: system_prompt.trim().to_string(),
        });
    }
    messages.extend_from_slice(chat);
    serde_json::to_string(&Request {
        messages,
        max_tokens: MAX_TOKENS,
    })
    .unwrap_or_default()
}

/// Lỗi Cloudflare gửi kèm: {"success":false,"errors":[{"code":..,"message":".."}]}
fn api_errors(v: &Value) -> Option<String> {
    let errors = v.get("errors")?.as_array()?;
    let messages: Vec<&str> = errors
        .iter()
        .filter_map(|e| e.get("message").and_then(Value::as_str))
        .collect();
    (!messages.is_empty()).then(|| messages.join("; "))
}

/// Nội dung kiểu chuỗi, hoặc mảng các phần {type, text}
fn content_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => {
            let text: Vec<&str> = parts
                .iter()
                .filter(|p| {
                    // Bỏ phần "suy nghĩ" của model, chỉ lấy câu trả lời
                    let kind = p.get("type").and_then(Value::as_str).unwrap_or("text");
                    !kind.contains("reasoning")
                })
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect();
            (!text.is_empty()).then(|| text.join(""))
        }
        _ => None,
    }
}

/// Lấy câu trả lời từ phần `result`
fn reply_from_result(result: &Value) -> Option<String> {
    if let Some(s) = result.as_str() {
        return Some(s.to_string());
    }
    // Kiểu cũ của Workers AI
    if let Some(s) = result.get("response").and_then(Value::as_str) {
        return Some(s.to_string());
    }
    // Kiểu Chat Completions
    if let Some(content) = result
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        && let Some(text) = content_text(content)
    {
        return Some(text);
    }
    // Kiểu Responses API: output[] gồm mục "reasoning" (bỏ qua) và mục "message"
    if let Some(output) = result.get("output").and_then(Value::as_array) {
        let text: Vec<String> = output
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
            .filter_map(|item| item.get("content").and_then(content_text))
            .collect();
        if !text.is_empty() {
            return Some(text.join("\n"));
        }
    }
    result
        .get("output_text")
        .and_then(Value::as_str)
        .map(String::from)
}

/// Câu trả lời trong JSON Cloudflare trả về (HTTP 200), hoặc lỗi dễ hiểu
pub fn parse_reply(body: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "AI trả về dữ liệu lạ".to_string())?;
    if v.get("success").and_then(Value::as_bool) == Some(false) {
        return Err(api_errors(&v).unwrap_or_else(|| "AI báo lỗi".to_string()));
    }
    let result = v.get("result").unwrap_or(&v);
    let text = reply_from_result(result).ok_or("Không đọc được câu trả lời của AI")?;
    let text = plain_text(&text);
    if text.trim().is_empty() {
        Err("AI không trả lời (thử hỏi lại, hoặc chọn model 120B)".to_string())
    } else {
        Ok(text)
    }
}

/// Lời nhắn cho mã lỗi HTTP thường gặp
fn http_error(status: u32, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| api_errors(&v));
    let hint = match status {
        400 => "Yêu cầu không hợp lệ",
        401 | 403 => "Token Cloudflare sai hoặc chưa có quyền Workers AI",
        404 => "Sai Account ID hoặc tên model",
        429 => "Gọi AI quá nhiều, đợi một lát rồi thử lại",
        500..=599 => "Máy chủ Cloudflare đang lỗi, thử lại sau",
        _ => "AI báo lỗi",
    };
    match detail {
        Some(d) => format!("{hint} (HTTP {status}: {d})"),
        None => format!("{hint} (HTTP {status})"),
    }
}

/// Chỉ bản debug: ISLAND_FAKE_AI=1 thì trả lời giả sau 1,5 giây, để thử giao diện
/// mà không cần tài khoản Cloudflare
pub fn fake() -> bool {
    cfg!(debug_assertions) && std::env::var_os("ISLAND_FAKE_AI").is_some()
}

/// Gửi câu hỏi (lượt cuối của `chat`) và chờ câu trả lời. Chạy trên luồng nền
pub fn ask(settings: &Settings, chat: &[Message]) -> Result<String, String> {
    if fake() {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let question = chat.last().map_or("", |m| m.content.as_str());
        return Ok(plain_text(&format!(
            "Câu trả lời thử cho «{question}».\n\n**Pizza** ngon vì lớp đế nướng giòn, \
             phô mai tan chảy béo ngậy và sốt cà chua chua ngọt hòa quyện với nhau.\n\
             - Nướng ở lò rất nóng nên vỏ giòn mà ruột vẫn mềm\n\
             - Vị umami của cà chua và phô mai làm ta muốn ăn thêm"
        )));
    }
    if settings.account_id.trim().is_empty() || settings.token.trim().is_empty() {
        return Err("Chưa cấu hình AI: mở Cài đặt > Modules để nhập Account ID và token".into());
    }
    let session = Session::open().map_err(|e| format!("Không kết nối được: {e}"))?;
    let body = request_body(&settings.system_prompt, chat);
    let auth = format!("Bearer {}", settings.token.trim());
    let response = session
        .post_json(&url(&settings.account_id, &settings.model), &auth, &body)
        .map_err(|e| format!("Không kết nối được tới Cloudflare: {e}"))?;
    if response.status != 200 {
        return Err(http_error(response.status, &response.body));
    }
    parse_reply(&response.body)
}

/// Bỏ ký hiệu Markdown để hiện chữ thường trên island:
/// "**đậm**" → "đậm", "# Tiêu đề" → "Tiêu đề", "- ý" / "* ý" → "• ý", bỏ dòng ```
pub fn plain_text(markdown: &str) -> String {
    let mut lines = Vec::new();
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            continue;
        }
        let indent = &line[..line.len() - trimmed.len()];
        let body = trimmed.trim_start_matches('#');
        let body = if body.len() != trimmed.len() {
            body.trim_start()
        } else {
            trimmed
        };
        let body = match body.strip_prefix("- ").or_else(|| body.strip_prefix("* ")) {
            Some(rest) => format!("• {rest}"),
            None => body.to_string(),
        };
        let body = body.replace("**", "").replace("__", "").replace('`', "");
        lines.push(format!("{indent}{body}"));
    }
    // Gộp nhiều dòng trống liền nhau thành một
    let mut out = String::new();
    let mut blank = false;
    for line in lines {
        let is_blank = line.trim().is_empty();
        if is_blank && blank {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line.trim_end());
        blank = is_blank;
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_uses_account_and_model() {
        assert_eq!(
            url(" abc123 ", "@cf/openai/gpt-oss-120b"),
            "https://api.cloudflare.com/client/v4/accounts/abc123/ai/run/@cf/openai/gpt-oss-120b"
        );
    }

    #[test]
    fn body_has_system_prompt_then_chat() {
        let body = request_body(
            "Bạn là trợ lý",
            &[
                Message::user("Hi"),
                Message::assistant("Chào"),
                Message::user("\"pizza\"?"),
            ],
        );
        let v: Value = serde_json::from_str(&body).unwrap();
        let m = v["messages"].as_array().unwrap();
        assert_eq!(m.len(), 4);
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[0]["content"], "Bạn là trợ lý");
        assert_eq!(m[3]["role"], "user");
        assert_eq!(m[3]["content"], "\"pizza\"?");
        assert_eq!(v["max_tokens"], MAX_TOKENS);
        // Lời dặn trống thì không gửi
        let v: Value = serde_json::from_str(&request_body("  ", &[Message::user("x")])).unwrap();
        assert_eq!(v["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn reads_the_three_known_reply_shapes() {
        let old = r#"{"result":{"response":"Xin chào"},"success":true,"errors":[]}"#;
        assert_eq!(parse_reply(old).unwrap(), "Xin chào");

        let chat = r#"{"result":{"choices":[{"index":0,"message":{"role":"assistant",
            "content":"Pizza ngon vì **phô mai**.","reasoning_content":"..."}}]},"success":true}"#;
        assert_eq!(parse_reply(chat).unwrap(), "Pizza ngon vì phô mai.");

        let responses = r#"{"result":{"output":[
            {"type":"reasoning","content":[{"type":"reasoning_text","text":"nghĩ..."}]},
            {"type":"message","role":"assistant","content":[{"type":"output_text","text":"Câu trả lời"}]}
        ]},"success":true}"#;
        assert_eq!(parse_reply(responses).unwrap(), "Câu trả lời");
    }

    #[test]
    fn errors_are_readable() {
        let failed = r#"{"result":null,"success":false,"errors":[{"code":10000,"message":"Authentication error"}]}"#;
        assert_eq!(parse_reply(failed).unwrap_err(), "Authentication error");
        assert!(parse_reply("<html>").is_err());
        let empty = r#"{"result":{"response":"  "},"success":true}"#;
        assert!(parse_reply(empty).unwrap_err().contains("không trả lời"));
        assert!(http_error(401, failed).starts_with("Token Cloudflare sai"));
        assert!(http_error(401, failed).contains("Authentication error"));
        assert_eq!(
            http_error(404, ""),
            "Sai Account ID hoặc tên model (HTTP 404)"
        );
    }

    #[test]
    fn missing_settings_fail_before_any_network_call() {
        let s = Settings {
            account_id: String::new(),
            token: "t".into(),
            model: MODELS[0].0.into(),
            system_prompt: String::new(),
        };
        assert!(
            ask(&s, &[Message::user("hi")])
                .unwrap_err()
                .contains("Chưa cấu hình AI")
        );
    }

    #[test]
    fn markdown_becomes_plain_text() {
        let md = "## Pizza\n\n\n**Pizza** là món `Ý`.\n- Đế giòn\n* Phô mai\n```\ncode\n```\n";
        assert_eq!(
            plain_text(md),
            "Pizza\n\nPizza là món Ý.\n• Đế giòn\n• Phô mai\ncode"
        );
        // Dấu # giữa câu (vd. "C#") giữ nguyên
        assert_eq!(plain_text("Học C# dễ"), "Học C# dễ");
    }
}

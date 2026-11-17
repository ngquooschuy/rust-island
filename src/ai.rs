//! Trợ lý AI, gọi qua một trong hai nhà cung cấp (chọn trong Cài đặt):
//! - Workers AI của Cloudflare (model gpt-oss của OpenAI): các lượt chat được gửi tới
//!   `https://api.cloudflare.com/client/v4/accounts/<ID>/ai/run/<model>`.
//! - Claude của Anthropic: `POST <base_url>/v1/messages`, với `base_url` là API chính chủ
//!   (`https://api.anthropic.com`) hoặc một gateway cùng kiểu API.
//!
//! Account ID, Base URL, model nằm trong config.toml; token/key cất trong Windows Credential
//! Manager (xem secret.rs).
//!
//! Tài liệu của Cloudflare không mô tả dạng kết quả của gpt-oss, nên `parse_reply` nhận cả
//! ba dạng hay gặp: `result.response`, `result.choices[0].message.content` (kiểu Chat
//! Completions) và `result.output[]` (kiểu Responses API của OpenAI).

use serde::Serialize;
use serde_json::Value;

use crate::config::{AiConfig, AiProvider};
use crate::http::Session;

/// Các model chọn được: (mã model, tên hiện trong Cài đặt)
pub const MODELS: [(&str, &str); 2] = [
    ("@cf/openai/gpt-oss-20b", "GPT-OSS 20B (nhanh, rẻ)"),
    ("@cf/openai/gpt-oss-120b", "GPT-OSS 120B (thông minh hơn)"),
];

/// Model Claude chọn được: (mã model, tên hiện trong Cài đặt). Cái đầu tiên là mặc định
pub const CLAUDE_MODELS: [(&str, &str); 3] = [
    ("claude-haiku-5-5", "Claude Haiku 5.5 (nhanh, rẻ)"),
    ("claude-sonnet-5-5", "Claude Sonnet 5.5"),
    ("claude-opus-5-5", "Claude Opus 5.5 (thông minh nhất)"),
];

/// API chính chủ của Anthropic; dùng gateway thì đổi thành địa chỉ của gateway
pub const CLAUDE_DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
/// Phiên bản API gửi trong header `anthropic-version`. Dùng bản 2023-06-01 vì gateway bên
/// thứ ba nào cũng nhận; API chính chủ vẫn nhận bản này
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Câu trả lời của Claude gồm cả phần suy nghĩ (luôn bật trên Haiku 5.5), nên chừa rộng
const CLAUDE_MAX_TOKENS: u32 = 4096;
/// Các việc hiện có (đoán tên bài, kiểm tra kết nối) đều ngắn: nghĩ ít cho nhanh và rẻ
const CLAUDE_EFFORT: &str = "low";

/// Lời dặn mặc định cho trợ lý (người dùng sửa được trong Cài đặt)
pub const DEFAULT_SYSTEM_PROMPT: &str = "Bạn là trợ lý thân thiện trên Dynamic Island của Windows. \
Trả lời ngắn gọn, rõ ràng, bằng ngôn ngữ người dùng đang dùng (mặc định tiếng Việt). \
Khi được hỏi một từ hoặc cụm từ, giải thích nghĩa trong 2–4 câu, có ví dụ nếu cần. \
Không dùng bảng hay định dạng Markdown phức tạp.";

/// gpt-oss "suy nghĩ" trước khi trả lời và phần đó cũng tính vào giới hạn: mặc định của
/// Cloudflare (256) dễ làm câu trả lời bị cụt, nên đặt cao hơn
const MAX_TOKENS: u32 = 2048;

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

    /// Câu AI đã trả lời, để gửi kèm khi chat nhiều lượt (hiện chưa có chỗ nào dùng)
    #[allow(dead_code)]
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

/// Gọi AI qua đâu
#[derive(Clone, Debug, PartialEq)]
pub enum Endpoint {
    Cloudflare { account_id: String },
    Claude { base_url: String },
}

impl Endpoint {
    /// Nhà cung cấp đang chọn trong cấu hình, kèm model của nhà cung cấp đó
    pub fn from_config(c: &AiConfig) -> (Self, String) {
        match c.provider {
            AiProvider::Cloudflare => (
                Self::Cloudflare {
                    account_id: c.account_id.trim().to_string(),
                },
                c.model.clone(),
            ),
            AiProvider::Claude => (
                Self::Claude {
                    base_url: c.base_url.trim().to_string(),
                },
                c.claude_model.clone(),
            ),
        }
    }

    pub fn provider(&self) -> AiProvider {
        match self {
            Self::Cloudflare { .. } => AiProvider::Cloudflare,
            Self::Claude { .. } => AiProvider::Claude,
        }
    }

    /// Đã nhập Account ID (Cloudflare) / Base URL (Claude) chưa
    pub fn is_set_up(&self) -> bool {
        match self {
            Self::Cloudflare { account_id } => !account_id.trim().is_empty(),
            Self::Claude { base_url } => !base_url.trim().is_empty(),
        }
    }
}

/// Thông tin cần để gọi AI
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub endpoint: Endpoint,
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

/// Câu trả lời trong JSON Cloudflare trả về (HTTP 200), giữ nguyên chữ AI viết, hoặc lỗi dễ hiểu
pub fn parse_reply(body: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "AI trả về dữ liệu lạ".to_string())?;
    if v.get("success").and_then(Value::as_bool) == Some(false) {
        return Err(api_errors(&v).unwrap_or_else(|| "AI báo lỗi".to_string()));
    }
    let result = v.get("result").unwrap_or(&v);
    let text = reply_from_result(result).ok_or("Không đọc được câu trả lời của AI")?;
    if plain_text(&text).trim().is_empty() {
        Err("AI không trả lời (thử hỏi lại, hoặc chọn model 120B)".to_string())
    } else {
        Ok(text)
    }
}

/// Địa chỉ gọi Claude: `<base_url>/v1/messages` (Base URL có sẵn đuôi /v1 cũng được)
pub fn claude_url(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    }
}

/// Thân request kiểu Messages API của Anthropic: lời dặn nằm ở trường `system` riêng
pub fn claude_body(model: &str, system_prompt: &str, chat: &[Message]) -> String {
    let mut body = serde_json::json!({
        "model": model.trim(),
        "max_tokens": CLAUDE_MAX_TOKENS,
        "messages": chat,
        "output_config": { "effort": CLAUDE_EFFORT },
    });
    if !system_prompt.trim().is_empty() {
        body["system"] = Value::from(system_prompt.trim());
    }
    body.to_string()
}

/// Lời nhắn lỗi Anthropic gửi kèm: {"type":"error","error":{"type":..,"message":".."}}
fn claude_error_message(v: &Value) -> Option<String> {
    v.get("error")?.get("message")?.as_str().map(str::to_string)
}

/// Câu trả lời của Claude (HTTP 200): nối các khối chữ trong `content[]`, bỏ khối suy nghĩ
pub fn parse_claude_reply(body: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "AI trả về dữ liệu lạ".to_string())?;
    if v.get("type").and_then(Value::as_str) == Some("error") {
        return Err(claude_error_message(&v).unwrap_or_else(|| "AI báo lỗi".to_string()));
    }
    if v.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
        return Err("Claude từ chối trả lời câu này".to_string());
    }
    let text: Vec<&str> = v
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    let text = text.join("");
    if text.trim().is_empty() {
        Err("AI không trả lời (thử hỏi lại)".to_string())
    } else {
        Ok(text)
    }
}

/// Lời nhắn cho mã lỗi HTTP thường gặp khi gọi Claude (API chính chủ hoặc gateway)
fn claude_http_error(status: u32, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| claude_error_message(&v));
    let hint = match status {
        400 => "Yêu cầu không hợp lệ",
        401 | 403 => "API key sai hoặc không có quyền",
        404 => "Sai Base URL hoặc tên model",
        429 => "Gọi AI quá nhiều, đợi một lát rồi thử lại",
        529 => "Claude đang quá tải, thử lại sau",
        500..=599 => "Máy chủ AI đang lỗi, thử lại sau",
        _ => "AI báo lỗi",
    };
    match detail {
        Some(d) => format!("{hint} (HTTP {status}: {d})"),
        None => format!("{hint} (HTTP {status})"),
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

/// Chỉ bản debug: ISLAND_FAKE_AI=1 thì trả lời giả sau 1,5 giây, để thử mà không gọi Cloudflare
pub fn fake() -> bool {
    cfg!(debug_assertions) && std::env::var_os("ISLAND_FAKE_AI").is_some()
}

/// Gửi câu hỏi (lượt cuối của `chat`) và chờ câu trả lời, đã bỏ ký hiệu Markdown để hiện
/// lên màn hình. Chạy trên luồng nền
pub fn ask(settings: &Settings, chat: &[Message]) -> Result<String, String> {
    ask_raw(settings, chat).map(|text| plain_text(&text))
}

/// Như `ask` nhưng giữ nguyên chữ AI viết (để đọc JSON trong câu trả lời)
pub fn ask_raw(settings: &Settings, chat: &[Message]) -> Result<String, String> {
    if fake() {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let question = chat.last().map_or("", |m| m.content.as_str());
        return Ok(format!(
            "Câu trả lời thử cho «{question}».\n\n**Pizza** ngon vì lớp đế nướng giòn, \
             phô mai tan chảy béo ngậy và sốt cà chua chua ngọt hòa quyện với nhau.\n\
             - Nướng ở lò rất nóng nên vỏ giòn mà ruột vẫn mềm\n\
             - Vị umami của cà chua và phô mai làm ta muốn ăn thêm"
        ));
    }
    let token = settings.token.trim();
    if !settings.endpoint.is_set_up() || token.is_empty() {
        let need = match settings.endpoint {
            Endpoint::Cloudflare { .. } => "Account ID và token",
            Endpoint::Claude { .. } => "Base URL và API key",
        };
        return Err(format!(
            "Chưa cấu hình AI: mở Cài đặt > Trợ lý AI để nhập {need}"
        ));
    }
    let session = Session::open().map_err(|e| format!("Không kết nối được: {e}"))?;
    match &settings.endpoint {
        Endpoint::Cloudflare { account_id } => {
            let body = request_body(&settings.system_prompt, chat);
            let auth = format!("Bearer {token}");
            let response = session
                .post_json(
                    &url(account_id, &settings.model),
                    &[("Authorization", &auth)],
                    &body,
                )
                .map_err(|e| format!("Không kết nối được tới Cloudflare: {e}"))?;
            if response.status != 200 {
                return Err(http_error(response.status, &response.body));
            }
            parse_reply(&response.body)
        }
        Endpoint::Claude { base_url } => {
            let body = claude_body(&settings.model, &settings.system_prompt, chat);
            let response = session
                .post_json(
                    &claude_url(base_url),
                    &[
                        ("x-api-key", token),
                        ("anthropic-version", ANTHROPIC_VERSION),
                    ],
                    &body,
                )
                .map_err(|e| format!("Không kết nối được tới {base_url}: {e}"))?;
            if response.status != 200 {
                return Err(claude_http_error(response.status, &response.body));
            }
            parse_claude_reply(&response.body)
        }
    }
}

/// Bỏ ký hiệu Markdown để hiện chữ thường (trong Cài đặt, trên island):
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
        // Giữ nguyên chữ AI viết (để đọc JSON); `ask` mới bỏ Markdown khi hiện lên màn hình
        assert_eq!(parse_reply(chat).unwrap(), "Pizza ngon vì **phô mai**.");
        assert_eq!(
            plain_text(&parse_reply(chat).unwrap()),
            "Pizza ngon vì phô mai."
        );

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
            endpoint: Endpoint::Cloudflare {
                account_id: String::new(),
            },
            token: "t".into(),
            model: MODELS[0].0.into(),
            system_prompt: String::new(),
        };
        assert!(
            ask(&s, &[Message::user("hi")])
                .unwrap_err()
                .contains("Chưa cấu hình AI")
        );
        // Claude: thiếu key thì báo cần API key, không gọi mạng
        let s = Settings {
            endpoint: Endpoint::Claude {
                base_url: CLAUDE_DEFAULT_BASE_URL.into(),
            },
            token: " ".into(),
            model: CLAUDE_MODELS[0].0.into(),
            system_prompt: String::new(),
        };
        assert!(
            ask(&s, &[Message::user("hi")])
                .unwrap_err()
                .contains("API key")
        );
    }

    #[test]
    fn claude_url_accepts_base_with_or_without_v1() {
        let want = "https://gw.example/v1/messages";
        assert_eq!(claude_url("https://gw.example"), want);
        assert_eq!(claude_url(" https://gw.example/ "), want);
        assert_eq!(claude_url("https://gw.example/v1/"), want);
    }

    #[test]
    fn claude_body_puts_system_prompt_apart_and_thinks_little() {
        let body: Value = serde_json::from_str(&claude_body(
            "claude-haiku-5-5",
            "Trả lời ngắn",
            &[Message::user("Xin chào")],
        ))
        .unwrap();
        assert_eq!(body["model"], "claude-haiku-5-5");
        assert_eq!(body["system"], "Trả lời ngắn");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "Xin chào");
        assert_eq!(body["output_config"]["effort"], "low");
        let no_system: Value =
            serde_json::from_str(&claude_body("m", "  ", &[Message::user("x")])).unwrap();
        assert!(no_system.get("system").is_none());
    }

    #[test]
    fn reads_claude_replies_and_errors() {
        let ok = r#"{"id":"msg_1","type":"message","role":"assistant","content":[
            {"type":"thinking","thinking":""},
            {"type":"text","text":"Xin "},{"type":"text","text":"chào"}],
            "stop_reason":"end_turn"}"#;
        assert_eq!(parse_claude_reply(ok).unwrap(), "Xin chào");
        let refused = r#"{"type":"message","content":[],"stop_reason":"refusal"}"#;
        assert!(parse_claude_reply(refused).unwrap_err().contains("từ chối"));
        let error = r#"{"type":"error","error":{"type":"not_found_error","message":"model: x"}}"#;
        assert_eq!(parse_claude_reply(error).unwrap_err(), "model: x");
        assert_eq!(
            claude_http_error(
                401,
                r#"{"type":"error","error":{"message":"invalid x-api-key"}}"#
            ),
            "API key sai hoặc không có quyền (HTTP 401: invalid x-api-key)"
        );
        assert_eq!(
            claude_http_error(404, "<html>"),
            "Sai Base URL hoặc tên model (HTTP 404)"
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

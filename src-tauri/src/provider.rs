use crate::model::{Profile, Protocol};
use futures_util::StreamExt;
use reqwest::{Client, RequestBuilder, Url};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub fn validate_profile(profile: &Profile) -> Result<(), String> {
    if profile.provider == "chatgpt"
        && (profile.protocol != Protocol::Responses || profile.base_url != crate::chatgpt::RESOURCE)
    {
        return Err("ChatGPT 订阅凭据只能用于 OpenAI 官方 Responses 接口。".into());
    }
    if profile.name.trim().is_empty() || profile.model.trim().is_empty() {
        return Err("请填写配置名称和模型 ID".into());
    }
    let url = Url::parse(profile.base_url.trim())
        .map_err(|_| "接口地址无效，请填写完整的 http:// 或 https:// 地址")?;
    if !["https", "http"].contains(&url.scheme())
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("接口地址应为不含用户名、密码、查询参数或片段的 HTTP(S) Base URL".into());
    }
    if profile.base_url.contains('{') || profile.base_url.contains('}') {
        return Err("请将接口地址中的占位符替换为实际地区或工作空间 ID".into());
    }
    if !(15..=600).contains(&profile.timeout_seconds) {
        return Err("超时应设为 15 至 600 秒".into());
    }
    Ok(())
}

fn build_request(
    client: &Client,
    profile: &Profile,
    key: &str,
    instruction: &str,
    text: &str,
    probe: bool,
) -> Result<RequestBuilder, String> {
    validate_profile(profile)?;
    let base = profile.base_url.trim().trim_end_matches('/');
    let (url, body) = match profile.protocol {
        Protocol::Responses if profile.provider == "chatgpt" => (
            format!("{base}/responses"),
            json!({ "model": profile.model, "instructions": instruction, "input": [{ "role": "user", "content": text }], "stream": true, "store": false }),
        ),
        Protocol::Responses => (
            format!("{base}/responses"),
            json!({ "model": profile.model, "instructions": instruction, "input": text, "stream": true, "store": false, "max_output_tokens": if probe { 64 } else { 16384 } }),
        ),
        Protocol::ChatCompletions => (
            format!("{base}/chat/completions"),
            json!({ "model": profile.model, "messages": [{ "role": "system", "content": instruction }, { "role": "user", "content": text }], "stream": true }),
        ),
        Protocol::Anthropic => (
            format!("{base}/messages"),
            json!({ "model": profile.model, "system": instruction, "messages": [{ "role": "user", "content": text }], "max_tokens": if probe { 64 } else { 8192 }, "stream": true }),
        ),
        Protocol::Gemini => {
            let mut url = Url::parse(&format!("{base}/")).map_err(|_| "接口地址无效")?;
            url.path_segments_mut()
                .map_err(|_| "接口地址无效")?
                .pop_if_empty()
                .push("models")
                .push(&format!(
                    "{}:streamGenerateContent",
                    profile.model.trim_start_matches("models/")
                ));
            url.set_query(Some("alt=sse"));
            (
                url.to_string(),
                json!({ "systemInstruction": { "parts": [{ "text": instruction }] }, "contents": [{ "role": "user", "parts": [{ "text": text }] }], "generationConfig": { "maxOutputTokens": if probe { 64 } else { 16384 } } }),
            )
        }
    };
    let mut request = client
        .post(url)
        .header("Accept", "text/event-stream")
        .json(&body);
    if !key.is_empty() {
        request = match profile.protocol {
            Protocol::Anthropic => request.header("x-api-key", key),
            Protocol::Gemini => request.header("x-goog-api-key", key),
            _ => request.bearer_auth(key),
        };
    }
    if profile.protocol == Protocol::Anthropic {
        request = request.header("anthropic-version", "2023-06-01");
    }
    Ok(request)
}

pub async fn translate(
    profile: &Profile,
    key: &str,
    instruction: &str,
    text: &str,
    probe: bool,
    cancel: CancellationToken,
    mut on_text: impl FnMut(&str) + Send,
) -> Result<String, String> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(profile.timeout_seconds))
        // Never forward credentials to a redirect destination.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "无法初始化网络连接")?;
    let request = build_request(&client, profile, key, instruction, text, probe)?;
    let response = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err("请求已取消".into()),
        response = request.send() => response.map_err(network_error)?,
    };
    if !response.status().is_success() {
        let status = response.status().as_u16();
        if profile.provider == "chatgpt" {
            let value = response.json::<Value>().await.unwrap_or(Value::Null);
            return Err(chatgpt_api_error(status, &value));
        }
        return Err(status_error(status));
    }
    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    let mut parser = ProtocolParser::new(profile.protocol.clone());
    let mut result = String::new();
    loop {
        let item = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err("请求已取消".into()),
            item = stream.next() => item,
        };
        let ended = item.is_none();
        let events = match item {
            Some(bytes) => decoder.feed(&bytes.map_err(network_error)?)?,
            None => decoder.finish()?,
        };
        for event in events {
            for delta in parser.process(&event)? {
                if cancel.is_cancelled() {
                    return Err("请求已取消".into());
                }
                result.push_str(&delta);
                on_text(&delta);
            }
        }
        if parser.finished {
            break;
        }
        if ended {
            return Err("连接已结束，但模型没有确认完成。已保留部分译文，请重试。".into());
        }
    }
    if parser.truncated {
        return Err("达到模型输出限制，译文未完成。请分段输入后重试。".into());
    }
    if result.trim().is_empty() {
        return Err("模型未返回译文，请检查模型、提示词或内容限制。".into());
    }
    Ok(result)
}

fn network_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "请求超时，已保留部分译文；可调整超时后重试。".into()
    } else if error.is_connect() {
        "无法连接模型服务，请检查接口地址、网络及系统代理环境变量。".into()
    } else {
        "网络连接中断，已保留部分译文，请重试。".into()
    }
}
fn status_error(status: u16) -> String {
    let explanation = match status {
        400 | 422 => "请求被拒绝，请检查模型 ID、输入长度与协议配置",
        401 | 403 => "认证失败或没有模型权限，请检查 API Key 和服务开通状态",
        404 => "接口或模型不存在，请检查 Base URL 与模型 ID",
        408 | 504 => "模型服务响应超时，请重试",
        429 => "请求频率或账户额度受限，请稍后重试并检查账户",
        300..=399 => "接口发生重定向，请直接填写最终服务地址",
        500..=599 => "模型服务暂时不可用，请稍后重试",
        _ => "模型服务拒绝请求，请检查配置",
    };
    format!("HTTP {status} · {explanation}")
}

pub fn chatgpt_api_error(status: u16, body: &Value) -> String {
    let error = if body["response"]["error"].is_object() {
        &body["response"]["error"]
    } else if body["error"].is_object() {
        &body["error"]
    } else {
        body
    };
    let code = error["code"].as_str().unwrap_or_default();
    let message = match code {
        "subscription_sharing_usage_limit_exceeded" => {
            "ChatGPT 套餐或此应用的使用额度已达到上限。请打开「管理订阅额度」查看，稍后手动重试。"
        }
        "subscription_sharing_user_not_eligible" => {
            "当前账户、工作空间或所在地区不支持在此应用中使用 ChatGPT 套餐。请检查账户权限。"
        }
        "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable" => {
            "ChatGPT 套餐额度服务暂时不可用，请稍后重试；登录凭据已保留。"
        }
        "subscription_sharing_unsupported_capability" => {
            "ChatGPT 套餐接口不支持当前请求能力或模型，请刷新模型列表后选择可用模型。"
        }
        "subscription_sharing_route_not_supported" => {
            "ChatGPT 套餐不支持此接口路径，请使用官方 Responses 接口。"
        }
        "subscription_sharing_invalid_user" => "ChatGPT 账户授权无法验证，请在模型设置中重新登录。",
        "chatpass_v2_scope_not_authorized" | "chatpass_v2_invalid_authorization_context" => {
            "ChatGPT 套餐使用权限未授权，请在设置中启用订阅额度。"
        }
        _ => match status {
            401 => "ChatGPT 登录或订阅授权无效，请在模型设置中重新登录。",
            403 => "ChatGPT 套餐访问被拒绝，请检查账户、工作空间及地区权限。",
            429 => "ChatGPT 请求频率或应用额度受限，请打开「管理订阅额度」查看并稍后重试。",
            503 => "ChatGPT 订阅服务暂不可用或尚未对该账户开放，请稍后重试。",
            _ => "ChatGPT 订阅请求未完成，请检查网络、模型与账户权限后重试。",
        },
    };
    // Never reflect server messages, URLs or token-shaped data into the renderer or logs.
    let diagnostic =
        if code.starts_with("subscription_sharing_") || code.starts_with("chatpass_v2_") {
            code.chars()
                .filter(|c| c.is_ascii_lowercase() || *c == '_' || c.is_ascii_digit())
                .take(100)
                .collect::<String>()
        } else {
            String::new()
        };
    if diagnostic.is_empty() {
        format!("{message}（HTTP {status}）")
    } else {
        format!("{message}\n{diagnostic}")
    }
}

#[derive(Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
    data: Vec<String>,
}
impl SseDecoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<String>, String> {
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() > 8 * 1024 * 1024 {
            return Err("接口返回的数据格式异常".into());
        }
        let mut events = vec![];
        while let Some(index) = self.buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<_> = self.buffer.drain(..=index).collect();
            let line = std::str::from_utf8(&line[..line.len() - 1])
                .map_err(|_| "接口返回了无效的 UTF-8 数据")?
                .trim_end_matches('\r');
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(self.data.join("\n"));
                    self.data.clear();
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data
                    .push(data.strip_prefix(' ').unwrap_or(data).to_owned());
            }
        }
        Ok(events)
    }
    pub fn finish(&mut self) -> Result<Vec<String>, String> {
        self.feed(b"\n\n")
    }
}

struct ProtocolParser {
    protocol: Protocol,
    finished: bool,
    truncated: bool,
}
impl ProtocolParser {
    fn new(protocol: Protocol) -> Self {
        Self {
            protocol,
            finished: false,
            truncated: false,
        }
    }
    fn process(&mut self, data: &str) -> Result<Vec<String>, String> {
        if data == "[DONE]" && self.protocol == Protocol::ChatCompletions {
            self.finished = true;
            return Ok(vec![]);
        }
        let value: Value =
            serde_json::from_str(data).map_err(|_| "模型返回的数据格式异常，请检查所选协议")?;
        if value.get("error").is_some() || value["type"] == "error" {
            if value["error"]["code"]
                .as_str()
                .or_else(|| value["code"].as_str())
                .is_some_and(|c| {
                    c.starts_with("subscription_sharing_") || c.starts_with("chatpass_v2_")
                })
            {
                return Err(chatgpt_api_error(0, &value));
            }
            return Err("模型在生成过程中报告错误，已保留部分译文，请重试。".into());
        }
        let mut deltas = vec![];
        let mut push = |text: &Value| {
            if let Some(text) = text.as_str() {
                if !text.is_empty() {
                    deltas.push(text.to_owned());
                }
            }
        };
        match self.protocol {
            Protocol::Responses => match value["type"].as_str().unwrap_or_default() {
                "response.output_text.delta" => push(&value["delta"]),
                "response.completed" => self.finished = true,
                "response.incomplete" => {
                    self.finished = true;
                    self.truncated = true;
                }
                "response.failed" => {
                    if value["response"]["error"]["code"]
                        .as_str()
                        .is_some_and(|c| {
                            c.starts_with("subscription_sharing_") || c.starts_with("chatpass_v2_")
                        })
                    {
                        return Err(chatgpt_api_error(0, &value));
                    }
                    return Err("模型生成失败，已保留部分译文，请重试。".into());
                }
                "response.refusal.delta" => return Err("模型拒绝翻译这段内容。".into()),
                _ => {}
            },
            Protocol::ChatCompletions => {
                let choice = &value["choices"][0];
                push(&choice["delta"]["content"]);
                match choice["finish_reason"].as_str() {
                    Some("length") => {
                        self.truncated = true;
                        self.finished = true;
                    }
                    Some("content_filter") => return Err("内容被模型服务过滤。".into()),
                    Some("tool_calls" | "function_call") => {
                        return Err("模型返回了工具调用，请使用文本生成模型。".into())
                    }
                    _ => {}
                }
            }
            Protocol::Anthropic => match value["type"].as_str().unwrap_or_default() {
                "content_block_delta" if value["delta"]["type"] == "text_delta" => {
                    push(&value["delta"]["text"])
                }
                "message_delta" => {
                    if value["delta"]["stop_reason"] == "max_tokens" {
                        self.truncated = true;
                    }
                    if value["delta"]["stop_reason"] == "refusal" {
                        return Err("模型拒绝翻译这段内容。".into());
                    }
                }
                "message_stop" => self.finished = true,
                _ => {}
            },
            Protocol::Gemini => {
                if value["promptFeedback"]["blockReason"].is_string() {
                    return Err("输入被模型服务过滤。".into());
                }
                let candidate = &value["candidates"][0];
                if let Some(parts) = candidate["content"]["parts"].as_array() {
                    for part in parts {
                        if part["thought"] != true {
                            push(&part["text"]);
                        }
                    }
                }
                match candidate["finishReason"].as_str() {
                    Some("STOP") => self.finished = true,
                    Some("MAX_TOKENS") => {
                        self.finished = true;
                        self.truncated = true;
                    }
                    Some(_) => return Err("模型因内容限制或生成错误停止，译文未完成。".into()),
                    None => {}
                }
            }
        }
        Ok(deltas)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subscription_request_uses_supported_fields_and_pins_credentials_to_openai() {
        let mut profile = test_profile(Protocol::Responses, crate::chatgpt::RESOURCE.into());
        profile.provider = "chatgpt".into();
        let request = build_request(
            &Client::new(),
            &profile,
            "fake-oauth-token",
            "translate-only",
            "原文",
            false,
        )
        .unwrap()
        .build()
        .unwrap();
        let body: Value =
            serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"], "原文");
        assert_eq!(body["instructions"], "translate-only");
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        for key in [
            "max_output_tokens",
            "temperature",
            "conversation",
            "previous_response_id",
            "metadata",
            "tools",
        ] {
            assert!(body.get(key).is_none(), "{key}");
        }
        profile.base_url = "https://example.invalid/v1".into();
        assert!(build_request(
            &Client::new(),
            &profile,
            "fake-oauth-token",
            "i",
            "t",
            false
        )
        .is_err());
    }
    #[test]
    fn subscription_limit_after_delta_is_failure_with_an_actionable_message() {
        let mut parser = ProtocolParser::new(Protocol::Responses);
        assert_eq!(
            parser
                .process(r#"{"type":"response.output_text.delta","delta":"partial"}"#)
                .unwrap(),
            vec!["partial"]
        );
        let error = parser.process(r#"{"type":"response.failed","response":{"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"do not reflect fake-secret"}}}"#).unwrap_err();
        assert!(error.contains("管理订阅额度"));
        assert!(!error.contains("fake-secret"));
        assert!(!parser.finished);
        assert!(chatgpt_api_error(
            403,
            &json!({"error":{"code":"subscription_sharing_user_not_eligible"}})
        )
        .contains("不支持"));
        assert!(chatgpt_api_error(503, &json!({"detail":"sensitive"})).contains("暂不可用"));
        assert!(!chatgpt_api_error(503, &json!({"detail":"sensitive"})).contains("sensitive"));
    }
    #[test]
    fn utf8_survives_every_possible_chunk_boundary() {
        let sse = "event: delta\r\ndata: {\"text\":\"中文🙂\"}\r\n\r\n: ping\n\ndata: [DONE]\n\n"
            .as_bytes();
        for cut in 0..=sse.len() {
            let mut decoder = SseDecoder::default();
            let mut events = decoder.feed(&sse[..cut]).unwrap();
            events.extend(decoder.feed(&sse[cut..]).unwrap());
            assert_eq!(events, vec!["{\"text\":\"中文🙂\"}", "[DONE]"]);
        }
    }
    #[test]
    fn protocol_completion_and_truncation() {
        let cases = [
            (
                Protocol::Responses,
                r#"{"type":"response.output_text.delta","delta":"译文"}"#,
                r#"{"type":"response.completed"}"#,
            ),
            (
                Protocol::ChatCompletions,
                r#"{"choices":[{"delta":{"content":"译文"}}]}"#,
                "[DONE]",
            ),
            (
                Protocol::Anthropic,
                r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"译文"}}"#,
                r#"{"type":"message_stop"}"#,
            ),
            (
                Protocol::Gemini,
                r#"{"candidates":[{"content":{"parts":[{"text":"译文"}]}}]}"#,
                r#"{"candidates":[{"finishReason":"STOP"}]}"#,
            ),
        ];
        for (protocol, delta, end) in cases {
            let mut p = ProtocolParser::new(protocol);
            assert_eq!(p.process(delta).unwrap(), vec!["译文"]);
            p.process(end).unwrap();
            assert!(p.finished);
        }
        let mut p = ProtocolParser::new(Protocol::Responses);
        p.process(r#"{"type":"response.incomplete"}"#).unwrap();
        assert!(p.truncated);
        let mut p = ProtocolParser::new(Protocol::ChatCompletions);
        assert!(p
            .process(r#"{"error":{"message":"secret"}}"#)
            .unwrap_err()
            .find("secret")
            .is_none());
    }
    #[test]
    fn request_wire_shapes_keep_source_separate() {
        for protocol in [
            Protocol::Responses,
            Protocol::ChatCompletions,
            Protocol::Anthropic,
            Protocol::Gemini,
        ] {
            let profile = Profile {
                id: "test".into(),
                name: "test".into(),
                provider: "custom".into(),
                protocol: protocol.clone(),
                base_url: "https://example.com/prefix/v1".into(),
                model: "model".into(),
                timeout_seconds: 30,
                has_key: true,
            };
            let req = build_request(
                &Client::new(),
                &profile,
                "test-key",
                "instruction",
                "{{unexpanded}} 原文",
                false,
            )
            .unwrap()
            .build()
            .unwrap();
            assert!(req.url().path().starts_with("/prefix/v1/"));
            assert!(!req.url().as_str().contains("test-key"));
            let body: Value =
                serde_json::from_slice(req.body().unwrap().as_bytes().unwrap()).unwrap();
            let source = match protocol {
                Protocol::Responses => &body["input"],
                Protocol::ChatCompletions => &body["messages"][1]["content"],
                Protocol::Anthropic => &body["messages"][0]["content"],
                Protocol::Gemini => &body["contents"][0]["parts"][0]["text"],
            };
            assert_eq!(source, "{{unexpanded}} 原文");
        }
    }

    async fn mock_server(
        status: u16,
        body: String,
        stall: bool,
    ) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![];
            let mut buffer = [0; 2048];
            loop {
                let count = socket.read(&mut buffer).await.unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
                if let Some(boundary) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..boundary]).to_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .unwrap_or("0")
                        .trim()
                        .parse()
                        .unwrap();
                    if request.len() >= boundary + 4 + length {
                        break;
                    }
                }
            }
            if stall {
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            let response = format!("HTTP/1.1 {status} Test\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            for chunk in response.as_bytes().chunks(2) {
                if socket.write_all(chunk).await.is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            String::from_utf8(request).unwrap()
        });
        (format!("http://{address}/v1"), task)
    }
    fn test_profile(protocol: Protocol, base_url: String) -> Profile {
        Profile {
            id: "test".into(),
            name: "test".into(),
            provider: "custom".into(),
            protocol,
            base_url,
            model: "model".into(),
            timeout_seconds: 15,
            has_key: true,
        }
    }
    #[tokio::test]
    async fn all_protocols_translate_through_real_fragmented_http() {
        let cases = [
            (Protocol::Responses, "data: {\"type\":\"response.output_text.delta\",\"delta\":\"你好🙂\"}\n\ndata: {\"type\":\"response.completed\"}\n\n", "authorization: Bearer fake-key"),
            (Protocol::ChatCompletions, "data: {\"choices\":[{\"delta\":{\"content\":\"你好🙂\"}}]}\n\ndata: [DONE]\n\n", "authorization: Bearer fake-key"),
            (Protocol::Anthropic, "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"你好🙂\"}}\n\ndata: {\"type\":\"message_stop\"}\n\n", "x-api-key: fake-key"),
            (Protocol::Gemini, "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"你好🙂\"}]},\"finishReason\":\"STOP\"}]}\n\n", "x-goog-api-key: fake-key"),
        ];
        for (protocol, sse, header) in cases {
            let (base, task) = mock_server(200, sse.into(), false).await;
            let profile = test_profile(protocol, base);
            let mut chunks = String::new();
            let output = translate(
                &profile,
                "fake-key",
                "instruction",
                "Hello",
                false,
                CancellationToken::new(),
                |t| chunks.push_str(t),
            )
            .await
            .unwrap();
            assert_eq!(output, "你好🙂");
            assert_eq!(chunks, output);
            let request = task.await.unwrap();
            assert!(request.contains(header));
            assert!(!request.lines().next().unwrap().contains("fake-key"));
        }
    }
    #[tokio::test]
    async fn errors_and_disconnects_are_not_successful_translations() {
        for status in [401, 429, 503] {
            let (base, task) =
                mock_server(status, "sensitive provider response".into(), false).await;
            let error = translate(
                &test_profile(Protocol::Responses, base),
                "",
                "i",
                "t",
                false,
                CancellationToken::new(),
                |_| {},
            )
            .await
            .unwrap_err();
            assert!(error.contains(&status.to_string()));
            assert!(!error.contains("sensitive"));
            task.await.unwrap();
        }
        let (base, task) = mock_server(
            200,
            "data: {\"choices\":[{\"delta\":{\"content\":\"部分\"}}]}\n\n".into(),
            false,
        )
        .await;
        let mut partial = String::new();
        assert!(translate(
            &test_profile(Protocol::ChatCompletions, base),
            "",
            "i",
            "t",
            false,
            CancellationToken::new(),
            |t| partial.push_str(t)
        )
        .await
        .is_err());
        assert_eq!(partial, "部分");
        task.await.unwrap();
    }
    #[tokio::test]
    async fn cancel_interrupts_waiting_for_headers() {
        let (base, task) = mock_server(200, "".into(), true).await;
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            trigger.cancel();
        });
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            translate(
                &test_profile(Protocol::Responses, base),
                "",
                "i",
                "t",
                false,
                cancel,
                |_| {},
            ),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err(), "请求已取消");
        task.abort();
    }
}

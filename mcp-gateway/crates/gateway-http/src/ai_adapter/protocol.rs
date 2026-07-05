use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::session::AiToolDef;

// ── OpenAI 兼容协议格式 ──

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenAiRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub struct OpenAiMessage {
    pub role: OpenAiRole,
    #[serde(default)]
    pub content: Option<Value>,
    #[serde(default)]
    pub tool_calls: Option<Vec<OpenAiToolCallDeser>>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub struct OpenAiToolCallDeser {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: OpenAiFunctionCallDeser,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub struct OpenAiFunctionCallDeser {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub struct OpenAiToolDef {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAiFunctionDef,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OpenAiFunctionDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub parameters: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub struct OpenAiChatRequest {
    #[serde(default)]
    pub model: Option<String>,
    pub messages: Vec<OpenAiMessage>,
    #[serde(default)]
    pub tools: Vec<OpenAiToolDef>,
    #[serde(default)]
    pub stream: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub struct OpenAiChatResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub struct OpenAiChoice {
    pub index: u32,
    pub message: OpenAiResponseMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub struct OpenAiResponseMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OpenAiToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: OpenAiFunctionCall,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OpenAiFunctionCall {
    pub name: String,
    pub arguments: String,
}

// ── Anthropic 协议格式 ──

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub struct AnthropicRequest {
    #[serde(default)]
    pub model: Option<String>,
    pub max_tokens: u32,
    pub messages: Vec<AnthropicMessage>,
    #[serde(default)]
    pub system: Option<Value>,
    #[serde(default)]
    pub tools: Vec<AnthropicToolDef>,
    #[serde(default)]
    pub stream: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AnthropicToolDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub struct AnthropicResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub response_type: String,
    pub role: String,
    pub model: String,
    pub content: Vec<AnthropicContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(dead_code)]
pub enum AnthropicContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

// ── 协议解析工具函数 ──

/// 从 OpenAI 请求中提取系统提示词
pub fn extract_openai_system_prompt(messages: &[OpenAiMessage]) -> String {
    messages
        .iter()
        .filter(|m| matches!(m.role, OpenAiRole::System))
        .filter_map(|m| m.content.as_ref())
        .map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Ensure input_schema has "type": "object" at the top level, as required by JSON Schema / MCP.
pub(crate) fn ensure_input_schema_type_object(mut schema: Value) -> Value {
    if let Some(obj) = schema.as_object_mut() {
        if !obj.contains_key("type") {
            obj.insert("type".to_string(), Value::String("object".to_string()));
        }
    }
    schema
}

/// 从 OpenAI 请求中提取工具定义
pub fn extract_openai_tools(tools: &[OpenAiToolDef]) -> Vec<AiToolDef> {
    tools
        .iter()
        .map(|t| AiToolDef {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            input_schema: ensure_input_schema_type_object(t.function.parameters.clone()),
            enabled: true,
        })
        .collect()
}

/// 从 Anthropic 请求中提取系统提示词
pub fn extract_anthropic_system_prompt(system: Option<&Value>) -> String {
    match system {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| {
                v.as_object()
                    .and_then(|o| o.get("text"))
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// 从 Anthropic 请求中提取工具定义
pub fn extract_anthropic_tools(tools: &[AnthropicToolDef]) -> Vec<AiToolDef> {
    tools
        .iter()
        .map(|t| AiToolDef {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: ensure_input_schema_type_object(t.input_schema.clone()),
            enabled: true,
        })
        .collect()
}

fn chat_plus_adapter_debugger_tool_definition(os: &str, now: &str, cfg: &BuiltinToolsConfig) -> Value {
    json!({
            "name": BuiltinTool::ChatPlusAdapterDebugger.name(),
            "description": render_builtin_tool_description(BuiltinTool::ChatPlusAdapterDebugger, os, now, cfg.task_planning, cfg.read_file),
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["exec"],
                "properties": {
                    "exec": {
                        "type": "string",
                        "description": "Documentation read or Chrome CDP debugging action. First call must read the complete builtin://chat-plus-adapter-debugger/SKILL.md. Then use `capture start`, `network search <filter>`, `network get <request-id>`, `network perf`, or documented raw CDP commands such as `netclear`, `net`, `netget`, `html`, `snap`, and `evalraw`."
                    },
                    "timeoutMs": {
                        "type": "integer",
                        "minimum": 1000,
                        "description": "Optional CDP command timeout in milliseconds."
                    },
                    "skillToken": {
                        "type": "string",
                        "description": "Required for every non-documentation action. First read the complete builtin://chat-plus-adapter-debugger/SKILL.md without skillToken, then use the returned skillToken; do not use regex or partial reads to fetch only the token. Calls without the correct token fail and must be retried."
                    }
                }
            }
    })
}

impl SkillsService {
    async fn handle_builtin_chat_plus_adapter_debugger(
        &self,
        config: &GatewayConfig,
        args: BuiltinShellArgs,
        planning_scope: &str,
    ) -> Result<ToolResult, AppError> {
        let command_preview = args.exec.trim().to_string();
        if command_preview.is_empty() {
            return Err(AppError::BadRequest("exec cannot be empty".to_string()));
        }

        if let Some((doc_tool, matched_path)) = builtin_skill_doc_read(&command_preview) {
            return Ok(builtin_skill_doc_result(
                doc_tool,
                &command_preview,
                matched_path,
                builtin_skill_token(doc_tool),
                Self::planning_enabled(config),
            ));
        }

        if let Some(result) = validate_skill_token_result(
            BuiltinTool::ChatPlusAdapterDebugger.name(),
            &builtin_skill_token(BuiltinTool::ChatPlusAdapterDebugger),
            args.skill_token.as_deref(),
        ) {
            return Ok(result);
        }

        let Some(debug_command) = parse_chat_plus_debug_command(&command_preview)? else {
            return Ok(tool_error(
                format!(
                    "{} supports documentation reads and Chrome CDP debugging actions. Use `capture start`, `network search <filter>`, `network get <request-id>`, or a documented CDP command after reading {}.",
                    BuiltinTool::ChatPlusAdapterDebugger.name(),
                    builtin_skill_uri(BuiltinTool::ChatPlusAdapterDebugger)
                ),
                json!({
                    "status": "error",
                    "tool": BuiltinTool::ChatPlusAdapterDebugger.name(),
                    "exec": command_preview,
                    "nextStep": "Use one of: capture start, capture clear, network search <filter>, network get <request-id>, network perf, netclear, net, netget, perfnet, html, snap, evalraw"
                }),
            ));
        };

        if let Some(result) = self
            .check_planning_gate(
                config,
                planning_scope,
                BuiltinTool::ChatPlusAdapterDebugger,
                args.planning_id.as_deref(),
            )
            .await
        {
            return Ok(result);
        }

        match debug_command {
            ChatPlusDebugCommand::Cdp {
                command,
                structured_command,
            } => {
                self.execute_builtin_chrome_cdp_command(
                    config,
                    BuiltinTool::ChatPlusAdapterDebugger.name(),
                    &command,
                    &structured_command,
                    args.timeout_ms,
                    planning_scope,
                    args.planning_id.as_deref(),
                    BuiltinTool::ChatPlusAdapterDebugger,
                )
                .await
            }
            ChatPlusDebugCommand::CaptureStart | ChatPlusDebugCommand::CaptureClear => {
                self.execute_builtin_chrome_cdp_command(
                    config,
                    BuiltinTool::ChatPlusAdapterDebugger.name(),
                    "netclear",
                    &command_preview,
                    args.timeout_ms,
                    planning_scope,
                    args.planning_id.as_deref(),
                    BuiltinTool::ChatPlusAdapterDebugger,
                )
                .await
            }
        }
    }
}

#[derive(Debug)]
enum ChatPlusDebugCommand {
    Cdp {
        command: String,
        structured_command: String,
    },
    CaptureStart,
    CaptureClear,
}

fn parse_chat_plus_debug_command(command: &str) -> Result<Option<ChatPlusDebugCommand>, AppError> {
    let tokens = split_shell_tokens(command);
    let Some(first) = tokens.first() else {
        return Ok(None);
    };
    let first = first.to_ascii_lowercase();

    match first.as_str() {
        "capture" => parse_chat_plus_capture_command(&tokens),
        "network" => parse_chat_plus_network_command(&tokens),
        "launch" | "open" | "list" | "ls" | "netclear" | "network-clear" | "net" | "netget"
        | "network-get" | "perfnet" | "html" | "snap" | "snapshot" | "evalraw" | "eval"
        | "shot" | "screenshot" | "nav" | "navigate" | "click" | "clickxy" | "type" | "loadall"
        | "stop" => Ok(Some(ChatPlusDebugCommand::Cdp {
            command: cdp_command_from_parts(&tokens)?,
            structured_command: command.to_string(),
        })),
        _ => Ok(None),
    }
}

fn parse_chat_plus_capture_command(
    tokens: &[String],
) -> Result<Option<ChatPlusDebugCommand>, AppError> {
    let Some(action) = tokens.get(1).map(|value| value.to_ascii_lowercase()) else {
        return Ok(None);
    };
    match action.as_str() {
        "start" | "install" => Ok(Some(ChatPlusDebugCommand::CaptureStart)),
        "clear" | "reset" => Ok(Some(ChatPlusDebugCommand::CaptureClear)),
        "list" | "search" => Ok(Some(ChatPlusDebugCommand::Cdp {
            command: cdp_command_from_parts_with_prefix("net", &tokens[2..])?,
            structured_command: cdp_command_from_parts(tokens)?,
        })),
        "get" => {
            if tokens.len() < 3 {
                return Err(AppError::BadRequest(
                    "capture get requires a CDP request id".to_string(),
                ));
            }
            Ok(Some(ChatPlusDebugCommand::Cdp {
                command: cdp_command_from_parts_with_prefix("netget", &tokens[2..])?,
                structured_command: cdp_command_from_parts(tokens)?,
            }))
        }
        "perf" | "performance" => Ok(Some(ChatPlusDebugCommand::Cdp {
            command: cdp_command_from_parts_with_prefix("perfnet", &tokens[2..])?,
            structured_command: cdp_command_from_parts(tokens)?,
        })),
        _ => Ok(None),
    }
}

fn parse_chat_plus_network_command(
    tokens: &[String],
) -> Result<Option<ChatPlusDebugCommand>, AppError> {
    let Some(action) = tokens.get(1).map(|value| value.to_ascii_lowercase()) else {
        return Ok(Some(ChatPlusDebugCommand::Cdp {
            command: "net".to_string(),
            structured_command: "network".to_string(),
        }));
    };
    match action.as_str() {
        "clear" | "start" | "reset" => Ok(Some(ChatPlusDebugCommand::Cdp {
            command: cdp_command_from_parts_with_prefix("netclear", &tokens[2..])?,
            structured_command: cdp_command_from_parts(tokens)?,
        })),
        "list" | "search" => Ok(Some(ChatPlusDebugCommand::Cdp {
            command: cdp_command_from_parts_with_prefix("net", &tokens[2..])?,
            structured_command: cdp_command_from_parts(tokens)?,
        })),
        "get" => {
            if tokens.len() < 3 {
                return Err(AppError::BadRequest(
                    "network get requires a CDP request id".to_string(),
                ));
            }
            Ok(Some(ChatPlusDebugCommand::Cdp {
                command: cdp_command_from_parts_with_prefix("netget", &tokens[2..])?,
                structured_command: cdp_command_from_parts(tokens)?,
            }))
        }
        "perf" | "performance" => Ok(Some(ChatPlusDebugCommand::Cdp {
            command: cdp_command_from_parts_with_prefix("perfnet", &tokens[2..])?,
            structured_command: cdp_command_from_parts(tokens)?,
        })),
        _ => Ok(Some(ChatPlusDebugCommand::Cdp {
            command: cdp_command_from_parts_with_prefix("net", &tokens[1..])?,
            structured_command: cdp_command_from_parts(tokens)?,
        })),
    }
}


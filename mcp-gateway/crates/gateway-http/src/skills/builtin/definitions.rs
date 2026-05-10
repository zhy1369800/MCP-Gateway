type BuiltinToolDefinitionFn = fn(&str, &str, &BuiltinToolsConfig) -> Value;

fn builtin_tool_definitions(os: &str, now: &str, cfg: &BuiltinToolsConfig) -> Vec<Value> {
    let enabled: Vec<BuiltinTool> = builtin_tools(cfg);
    let builders: [(BuiltinTool, BuiltinToolDefinitionFn); 7] = [
        (BuiltinTool::ReadFile, read_file_tool_definition),
        (BuiltinTool::ShellCommand, shell_command_tool_definition),
        (BuiltinTool::MultiEditFile, multi_edit_file_tool_definition),
        (BuiltinTool::TaskPlanning, task_planning_tool_definition),
        (BuiltinTool::ChromeCdp, chrome_cdp_tool_definition),
        (
            BuiltinTool::ChatPlusAdapterDebugger,
            chat_plus_adapter_debugger_tool_definition,
        ),
        (BuiltinTool::OfficeCli, office_cli_tool_definition),
    ];
    let mut defs = builders
        .into_iter()
        .filter(|(tool, _)| enabled.contains(tool))
        .map(|(_, build)| build(os, now, cfg))
        .collect::<Vec<_>>();

    if cfg.task_planning {
        for def in &mut defs {
            add_planning_gate_schema(def);
        }
    }
    defs
}

fn add_planning_gate_schema(def: &mut Value) {
    let Some(name) = def.get("name").and_then(Value::as_str) else {
        return;
    };
    if name == BuiltinTool::TaskPlanning.name() {
        return;
    }
    let Some(properties) = def
        .get_mut("inputSchema")
        .and_then(|schema| schema.get_mut("properties"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    properties.insert(
        "planningId".to_string(),
        json!({
            "type": "string",
            "description": "Required for non-documentation calls when task-planning is enabled. Use the planningId returned by task-planning update."
        }),
    );
}

fn render_builtin_tool_description(
    tool: BuiltinTool,
    os: &str,
    now: &str,
    planning_enabled: bool,
    read_file_enabled: bool,
) -> String {
    let frontmatter = builtin_skill_frontmatter(tool);
    let skill_uri = builtin_skill_uri(tool);
    let skill_root_uri = builtin_skill_uri_root(tool);
    let shell_read_cmd = if cfg!(target_os = "windows") {
        format!("Get-Content -Raw {skill_uri}")
    } else {
        format!("cat {skill_uri}")
    };
    let read_file_doc = format!("`read_file` with `path`: `{skill_uri}` and no `skillToken`");
    let doc_read_hint = if read_file_enabled && tool != BuiltinTool::ReadFile {
        read_file_doc.clone()
    } else {
        format!("shell `exec`: `{shell_read_cmd}`")
    };
    let read_requirement = match tool {
        BuiltinTool::ReadFile => {
            format!("The only acceptable first call to this tool is a documentation-read call that reads the complete SKILL.md and does not require `skillToken`. Suggested arguments: `{{\"path\":\"{skill_uri}\"}}`.")
        }
        BuiltinTool::ShellCommand => {
            format!("The only acceptable first call to this tool is a documentation-read call that reads the complete SKILL.md and does not require `skillToken`. Suggested `exec`: `{shell_read_cmd}`.")
        }
        BuiltinTool::MultiEditFile => {
            format!("Before calling `{}`, read the complete SKILL.md; this read does not require `skillToken`. Preferred documentation read: {doc_read_hint}.", tool.name())
        }
        BuiltinTool::TaskPlanning
        | BuiltinTool::ChromeCdp
        | BuiltinTool::ChatPlusAdapterDebugger
        | BuiltinTool::OfficeCli => {
            format!("The only acceptable first call to this tool is a documentation-read call that reads the complete SKILL.md and does not require `skillToken`. Preferred documentation read: {doc_read_hint}.")
        }
    };
    let frontmatter_block = if frontmatter.block.trim().is_empty() {
        "none".to_string()
    } else {
        format!("---\n{}\n---", frontmatter.block.trim())
    };

    let mut description = format!(
        "Bundled skill: {}.\nMANDATORY BEFORE USE: this tool description is only a short discovery summary, not the operating instructions. Before using this bundled skill for any real action, you MUST first read its full SKILL.md. Do not infer safe usage from this description alone; skipping SKILL.md can cause incorrect or dangerous tool use. {read_requirement} The SKILL.md response includes the required `skillToken` only inside the returned markdown content. You must obtain it by reading the complete SKILL.md document; this SKILL.md read is the one call that does not need `skillToken`. Do not use regex, grep, Select-String, line ranges, or other partial-read tricks to fetch only the token. Every later non-documentation call to this skill MUST include that exact `skillToken` argument or the gateway will reject the call; a rejected call fails and must be retried with the correct token. The gateway serves bundled SKILL.md reads from embedded content, so this direct documentation read does not require a workspace `cwd`.\nCurrent OS: {os}.\nCurrent datetime: {now}.\nSkill URI: {skill_root_uri}.\nSKILL.md URI: {skill_uri}.\nFront matter summary:\nname: {}\ndescription: {}\nmetadata: {}\nFront matter raw (YAML):\n{}",
        frontmatter.name,
        frontmatter.name,
        if frontmatter.description.trim().is_empty() {
            "none"
        } else {
            frontmatter.description.trim()
        },
        if frontmatter.metadata.trim().is_empty() {
            "none"
        } else {
            frontmatter.metadata.trim()
        },
        frontmatter_block
    );
    if planning_enabled && tool != BuiltinTool::TaskPlanning {
        description.push_str("\n\n");
        description.push_str(planning_gate_instructions());
    }
    description
}

fn builtin_skill_frontmatter(tool: BuiltinTool) -> ParsedFrontmatter {
    parse_frontmatter_content(builtin_skill_md_content(tool), &builtin_skill_uri(tool))
        .unwrap_or_default()
}

fn builtin_skill_md_content(tool: BuiltinTool) -> &'static str {
    match tool {
        BuiltinTool::ReadFile => BUILTIN_READ_FILE_SKILL_MD,
        BuiltinTool::ShellCommand => BUILTIN_SHELL_COMMAND_SKILL_MD,
        BuiltinTool::MultiEditFile => BUILTIN_MULTI_EDIT_FILE_SKILL_MD,
        BuiltinTool::TaskPlanning => BUILTIN_TASK_PLANNING_SKILL_MD,
        BuiltinTool::ChromeCdp => BUILTIN_CHROME_CDP_SKILL_MD,
        BuiltinTool::ChatPlusAdapterDebugger => BUILTIN_CHAT_PLUS_ADAPTER_DEBUGGER_SKILL_MD,
        BuiltinTool::OfficeCli => BUILTIN_OFFICECLI_SKILL_MD,
    }
}

fn builtin_skills_root_uri() -> &'static str {
    "builtin://"
}

fn builtin_skill_uri_root(tool: BuiltinTool) -> String {
    format!("builtin://{}", tool.name())
}

fn builtin_skill_uri(tool: BuiltinTool) -> String {
    format!("builtin://{}/SKILL.md", tool.name())
}

fn builtin_skill_doc_read(command: &str) -> Option<(BuiltinTool, String)> {
    let tokens = split_shell_tokens(command);
    let (program, args) = tokens.split_first()?;
    let normalized_program = normalize_command_token(program);
    if !matches!(
        normalized_program.as_str(),
        "cat" | "type" | "get-content" | "gc"
    ) {
        return None;
    }

    args.iter().find_map(|arg| builtin_skill_doc_arg(arg))
}

fn builtin_skill_doc_result(
    tool: BuiltinTool,
    command: &str,
    matched_path: String,
    token: String,
    planning_enabled: bool,
) -> ToolResult {
    let mut text = render_builtin_skill_md(tool, planning_enabled);
    text.push_str(&format!(
        "\n\n[skillToken]\nUse this exact skillToken for subsequent non-documentation calls to `{}`: {}\n",
        tool.name(),
        token
    ));
    tool_success(
        text,
        json!({
            "status": "completed",
            "tool": BuiltinTool::ShellCommand.name(),
            "command": command,
            "builtinSkill": tool.name(),
            "path": matched_path,
            "docSource": "embedded"
        }),
    )
}

fn builtin_skill_read_doc_result(
    tool: BuiltinTool,
    matched_path: String,
    token: String,
    planning_enabled: bool,
) -> ToolResult {
    let mut text = render_builtin_skill_md(tool, planning_enabled);
    text.push_str(&format!(
        "\n\n[skillToken]\nUse this exact skillToken for subsequent non-documentation calls to `{}`: {}\n",
        tool.name(),
        token
    ));
    tool_success(
        text.clone(),
        json!({
            "status": "completed",
            "tool": BuiltinTool::ReadFile.name(),
            "builtinSkill": tool.name(),
            "path": matched_path,
            "docSource": "embedded",
            "content": text
        }),
    )
}

fn render_builtin_skill_md(tool: BuiltinTool, planning_enabled: bool) -> String {
    let mut content = builtin_skill_md_content(tool).to_string();
    if planning_enabled && tool != BuiltinTool::TaskPlanning {
        content.push_str("\n\n");
        content.push_str(planning_gate_instructions());
    }
    content
}


fn skill_doc_result(
    tool_name: &str,
    skill: &str,
    command: &str,
    path: String,
    content: String,
    token: String,
) -> ToolResult {
    let mut text = content;
    text.push_str(&format!(
        "\n\n[skillToken]\nUse this exact skillToken for subsequent non-documentation calls to `{tool_name}`: {token}\n"
    ));
    tool_success(
        text,
        json!({
            "status": "completed",
            "tool": tool_name,
            "skill": skill,
            "command": command,
            "path": path,
            "docSource": "file"
        }),
    )
}

fn validate_skill_token_result(
    tool_name: &str,
    expected_token: &str,
    provided: Option<&str>,
) -> Option<ToolResult> {
    match provided.map(str::trim).filter(|value| !value.is_empty()) {
        Some(provided) if provided == expected_token => None,
        Some(_) => Some(skill_token_error(
            tool_name,
            "invalid skillToken for this skill",
        )),
        None => Some(skill_token_error(
            tool_name,
            "missing skillToken for this skill",
        )),
    }
}

fn skill_token_error(tool_name: &str, message: &str) -> ToolResult {
    tool_error(
        format!(
            "{message}. This call failed and must be retried with the correct token. Read the complete SKILL.md first; that documentation-read call does not require `skillToken`. Then retry `{tool_name}` with the returned `skillToken` argument. Do not use regex, grep, Select-String, line ranges, or partial reads to fetch only the token."
        ),
        json!({
            "status": "error",
            "code": "SkillTokenRequired",
            "tool": tool_name,
            "message": message,
            "requiredArgument": "skillToken",
            "nextStep": "This call failed. Read the complete corresponding SKILL.md with the documented first-call command; that SKILL.md read does not require skillToken. Then retry with the returned skillToken. Do not use regex, grep, Select-String, line ranges, or partial reads to fetch only the token."
        }),
    )
}

fn builtin_skill_token(tool: BuiltinTool) -> String {
    skill_token_from_content(builtin_skill_md_content(tool))
}

fn external_skill_token(skill: &DiscoveredSkill) -> Result<String, AppError> {
    let skill_md_path = skill.path.join("SKILL.md");
    let content = std::fs::read_to_string(skill_md_path)?;
    Ok(skill_token_from_content(&content))
}

fn skill_token_from_content(content: &str) -> String {
    // Stable FNV-1a hash: enough for a short gate token without adding a crypto dependency.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}").chars().take(6).collect()
}

fn is_external_skill_doc_read_command(command: &str, skill: &DiscoveredSkill) -> bool {
    let tokens = split_shell_tokens(command);
    let Some((program, args)) = tokens.split_first() else {
        return false;
    };
    let normalized_program = normalize_command_token(program);
    if !matches!(
        normalized_program.as_str(),
        "cat" | "type" | "get-content" | "gc"
    ) {
        return false;
    }

    let skill_md = normalize_root_path(skill.path.join("SKILL.md"));
    args.iter().any(|arg| {
        let candidate = strip_matching_quotes(arg)
            .trim()
            .trim_end_matches(';')
            .trim();
        if candidate.is_empty() || candidate.starts_with('-') {
            return false;
        }
        let path = PathBuf::from(candidate);
        let resolved = if path.is_absolute() {
            normalize_root_path(path)
        } else {
            normalize_root_path(skill.path.join(path))
        };
        resolved == skill_md
    })
}

fn builtin_skill_doc_arg(arg: &str) -> Option<(BuiltinTool, String)> {
    let candidate = strip_matching_quotes(arg)
        .trim()
        .trim_end_matches(';')
        .trim();
    if candidate.is_empty() || candidate.starts_with('-') {
        return None;
    }

    // Iterate over ALL builtin tools regardless of enabled/disabled state.
    // Reading documentation should always be allowed without skillToken.
    const ALL_BUILTIN_TOOLS: &[BuiltinTool] = &[
        BuiltinTool::ReadFile,
        BuiltinTool::ShellCommand,
        BuiltinTool::MultiEditFile,
        BuiltinTool::TaskPlanning,
        BuiltinTool::ChromeCdp,
        BuiltinTool::ChatPlusAdapterDebugger,
        BuiltinTool::OfficeCli,
    ];

    for &tool in ALL_BUILTIN_TOOLS {
        let uri = builtin_skill_uri(tool);
        if candidate.eq_ignore_ascii_case(&uri) {
            return Some((tool, uri));
        }
    }

    None
}


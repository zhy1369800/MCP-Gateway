type BuiltinToolDefinitionFn = fn(&str, &str, &BuiltinToolsConfig) -> Value;

fn builtin_tool_definitions(os: &str, now: &str, cfg: &BuiltinToolsConfig) -> Vec<Value> {
    let enabled: Vec<BuiltinTool> = builtin_tools(cfg);
    let mut defs = BuiltinTool::ALL
        .iter()
        .copied()
        .filter(|tool| enabled.contains(tool))
        .map(|tool| tool.definition_builder()(os, now, cfg))
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
    _read_file_enabled: bool,
) -> String {
    let frontmatter = builtin_skill_frontmatter(tool);
    let skill_root_uri = builtin_skill_uri_root(tool);
    let self_read = format!("call `{}` with `{{\"readSkill\":true}}`", tool.name());
    let read_requirement = format!(
        "The only acceptable first call to this tool is a documentation-read call that reads this tool's complete SKILL.md and does not require `skillToken`: {self_read}."
    );
    let frontmatter_block = if frontmatter.block.trim().is_empty() {
        "none".to_string()
    } else {
        format!("---\n{}\n---", frontmatter.block.trim())
    };

    let mut description = format!(
        "Bundled skill: {}.\nMANDATORY BEFORE USE: this tool description is only a short discovery summary, not the operating instructions. Before using this bundled skill for any real action, you MUST first read its full SKILL.md through this same tool. Do not infer safe usage from this description alone; skipping SKILL.md can cause incorrect or dangerous tool use. {read_requirement} The SKILL.md response includes the required `skillToken` only inside the returned markdown content. You must obtain it by reading the complete SKILL.md document through this same tool; this SKILL.md read is the one call that does not need `skillToken`. Do not use regex, grep, Select-String, line ranges, or other partial-read tricks to fetch only the token. Every later non-documentation call to this skill MUST include that exact `skillToken` argument or the gateway will reject the call; a rejected call fails and must be retried with the correct token. The gateway serves bundled SKILL.md reads from embedded content, so this direct documentation read does not require a workspace `cwd` or any other builtin tool.\nCurrent OS: {os}.\nCurrent datetime: {now}.\nSkill URI: {skill_root_uri}.\nFront matter summary:\nname: {}\ndescription: {}\nmetadata: {}\nFront matter raw (YAML):\n{}",
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
        BuiltinTool::CodeGraph => BUILTIN_CODEGRAPH_SKILL_MD,
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

fn builtin_skill_self_doc_result(tool: BuiltinTool, token: String, planning_enabled: bool) -> ToolResult {
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
            "tool": tool.name(),
            "builtinSkill": tool.name(),
            "path": builtin_skill_uri(tool),
            "docSource": "embedded",
            "readSkill": true,
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
            "{message}. This call failed and must be retried with the correct token. First call `{tool_name}` with `{{\"readSkill\":true}}`; that documentation-read call does not require `skillToken`. Then retry `{tool_name}` with the returned `skillToken` argument. Do not use regex, grep, Select-String, line ranges, or partial reads to fetch only the token."
        ),
        json!({
            "status": "error",
            "code": "SkillTokenRequired",
            "tool": tool_name,
            "message": message,
            "requiredArgument": "skillToken",
            "nextStep": format!("This call failed. First call `{tool_name}` with {{\"readSkill\":true}}; that SKILL.md read does not require skillToken. Then retry with the returned skillToken. Do not use regex, grep, Select-String, line ranges, or partial reads to fetch only the token.")
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

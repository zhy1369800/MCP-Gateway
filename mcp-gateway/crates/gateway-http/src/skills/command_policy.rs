fn roots_signature(roots: &[String]) -> String {
    let mut normalized = roots
        .iter()
        .map(|root| root.trim())
        .filter(|root| !root.is_empty())
        .map(|root| {
            normalize_lexical_path(Path::new(root))
                .to_string_lossy()
                .to_string()
        })
        .collect::<Vec<_>>();
    normalized.sort_by_key(|item| item.to_ascii_lowercase());
    normalized.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    normalized.join("\u{001f}")
}

async fn execute_skill_command(
    command: &mut Command,
    timeout_ms: u64,
    max_output_bytes: usize,
    disable_truncation: bool,
    stdout_emitter: Option<SkillStreamEmitter>,
    stderr_emitter: Option<SkillStreamEmitter>,
) -> Result<SkillCommandExecution, AppError> {
    let mut child = command
        .spawn()
        .map_err(|error| AppError::Upstream(format!("failed to execute command: {error}")))?;
    if let Some(pid) = child.id() {
        let _ = assign_child_to_gateway_job(pid);
    }
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Internal("missing stdout from skill command".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::Internal("missing stderr from skill command".to_string()))?;

    let stdout_state = Arc::new(Mutex::new(StreamCaptureState::default()));
    let stderr_state = Arc::new(Mutex::new(StreamCaptureState::default()));

    let stdout_task = tokio::spawn(capture_stream_output(
        stdout,
        stdout_state.clone(),
        max_output_bytes,
        disable_truncation,
        stdout_emitter,
    ));
    let stderr_task = tokio::spawn(capture_stream_output(
        stderr,
        stderr_state.clone(),
        max_output_bytes,
        disable_truncation,
        stderr_emitter,
    ));

    let status = match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait()).await {
        Ok(wait_result) => wait_result
            .map_err(|error| AppError::Upstream(format!("failed to execute command: {error}")))?,
        Err(_) => {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            stdout_task.abort();
            stderr_task.abort();
            let stdout = snapshot_stream_output(&stdout_state);
            let stderr = snapshot_stream_output(&stderr_state);
            return Ok(SkillCommandExecution {
                status: None,
                stdout,
                stderr,
                timed_out: true,
            });
        }
    };

    stdout_task
        .await
        .map_err(|error| AppError::Internal(format!("stdout capture join error: {error}")))??;
    stderr_task
        .await
        .map_err(|error| AppError::Internal(format!("stderr capture join error: {error}")))??;
    let stdout = snapshot_stream_output(&stdout_state);
    let stderr = snapshot_stream_output(&stderr_state);

    Ok(SkillCommandExecution {
        status: Some(status),
        stdout,
        stderr,
        timed_out: false,
    })
}

async fn capture_stream_output<R>(
    mut reader: R,
    shared_state: Arc<Mutex<StreamCaptureState>>,
    max_output_bytes: usize,
    disable_truncation: bool,
    emitter: Option<SkillStreamEmitter>,
) -> Result<(), AppError>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut chunk = [0_u8; 8192];

    loop {
        let read = reader.read(&mut chunk).await.map_err(|error| {
            AppError::Upstream(format!("failed to read command output: {error}"))
        })?;
        if read == 0 {
            break;
        }
        if let Some(emitter) = &emitter {
            emitter
                .emit(String::from_utf8_lossy(&chunk[..read]).to_string())
                .await;
        }

        let mut state = match shared_state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        if disable_truncation {
            state.bytes.extend_from_slice(&chunk[..read]);
            continue;
        }

        if state.bytes.len() < max_output_bytes {
            let available = max_output_bytes - state.bytes.len();
            let take = available.min(read);
            state.bytes.extend_from_slice(&chunk[..take]);
            if take < read {
                state.truncated = true;
            }
        } else {
            state.truncated = true;
        }
    }

    Ok(())
}

fn snapshot_stream_output(shared_state: &Arc<Mutex<StreamCaptureState>>) -> StreamCapturedOutput {
    let state = match shared_state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    StreamCapturedOutput {
        text: String::from_utf8_lossy(&state.bytes).to_string(),
        truncated: state.truncated,
    }
}

fn evaluate_policy(
    skills: &SkillsConfig,
    program: &str,
    command_args: &[String],
    raw_command: &str,
    script_path: &Path,
    script_text: Option<&str>,
) -> PolicyDecision {
    let invocations = collect_command_invocations(program, command_args, raw_command, script_text);
    for invocation in &invocations {
        for rule in &skills.policy.rules {
            if rule_matches(rule, invocation) {
                let base_reason = if rule.reason.trim().is_empty() {
                    "matched command rule".to_string()
                } else {
                    rule.reason.trim().to_string()
                };
                let reason = format!(
                    "{} (rule: {}, source: {})",
                    base_reason, rule.id, invocation.source
                );
                return action_to_decision(&rule.action, reason, rule.reason_key.clone());
            }
        }
    }

    if skills.policy.path_guard.enabled && !skills.policy.path_guard.whitelist_dirs.is_empty() {
        let whitelist = skills
            .policy
            .path_guard
            .whitelist_dirs
            .iter()
            .map(PathBuf::from)
            .map(normalize_root_path)
            .collect::<Vec<_>>();

        if let Some((token, source, resolved)) =
            find_outside_whitelist_path(script_path, command_args, script_text, &whitelist)
        {
            let reason = format!(
                "path '{}' resolved to '{}' is outside whitelist (source: {})",
                token,
                resolved.to_string_lossy(),
                source
            );
            return action_to_decision(
                &skills.policy.path_guard.on_violation,
                reason,
                "path_outside_allowed_dir".to_string(),
            );
        }
    }

    action_to_decision(
        &skills.policy.default_action,
        "matched default policy".to_string(),
        "default_policy".to_string(),
    )
}

fn action_to_decision(
    action: &SkillPolicyAction,
    reason: String,
    reason_key: String,
) -> PolicyDecision {
    match action {
        SkillPolicyAction::Allow => PolicyDecision::Allow,
        SkillPolicyAction::Confirm => PolicyDecision::Confirm { reason, reason_key },
        SkillPolicyAction::Deny => PolicyDecision::Deny(reason),
    }
}

fn collect_command_invocations(
    program: &str,
    command_args: &[String],
    raw_command: &str,
    script_text: Option<&str>,
) -> Vec<CommandInvocation> {
    let mut list = Vec::new();
    let mut runtime_tokens = Vec::with_capacity(command_args.len() + 1);
    runtime_tokens.push(normalize_command_token(program));
    runtime_tokens.extend(command_args.iter().map(|item| item.to_ascii_lowercase()));
    list.push(CommandInvocation {
        raw: std::iter::once(program.to_string())
            .chain(command_args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" "),
        tokens: runtime_tokens,
        source: "runtime".to_string(),
    });

    for (index, segment) in split_command_segments(raw_command).into_iter().enumerate() {
        let mut tokens = split_shell_tokens(&segment);
        if tokens.is_empty() {
            continue;
        }
        if tokens[0] == "&" {
            tokens.remove(0);
        }
        if tokens.is_empty() {
            continue;
        }

        let mut normalized = Vec::with_capacity(tokens.len());
        normalized.push(normalize_command_token(&tokens[0]));
        normalized.extend(tokens.iter().skip(1).map(|item| item.to_ascii_lowercase()));

        list.push(CommandInvocation {
            raw: segment,
            tokens: normalized,
            source: format!("runtime:segment:{}", index + 1),
        });
    }

    if let Some(script) = script_text {
        for (line_no, line) in script.lines().enumerate().take(300) {
            let trimmed = line.trim();
            if trimmed.is_empty() || is_comment_line(trimmed) {
                continue;
            }

            let mut tokens = split_shell_tokens(trimmed);
            if tokens.is_empty() {
                continue;
            }
            if tokens[0] == "&" {
                tokens.remove(0);
            }
            if tokens.is_empty() {
                continue;
            }

            let mut normalized = Vec::with_capacity(tokens.len());
            normalized.push(normalize_command_token(&tokens[0]));
            normalized.extend(tokens.iter().skip(1).map(|item| item.to_ascii_lowercase()));

            list.push(CommandInvocation {
                raw: trimmed.to_string(),
                tokens: normalized,
                source: format!("script:{}", line_no + 1),
            });
        }
    }

    list
}

fn split_command_segments(raw: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = raw.chars().peekable();

    while let Some(ch) = chars.next() {
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
            current.push(ch);
            continue;
        }

        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                current.push(ch);
            }
            ';' => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    segments.push(trimmed.to_string());
                }
                current.clear();
            }
            '|' => {
                if chars.peek().copied() == Some('|') {
                    let _ = chars.next();
                }
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    segments.push(trimmed.to_string());
                }
                current.clear();
            }
            '&' => {
                if chars.peek().copied() == Some('&') {
                    let _ = chars.next();
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        segments.push(trimmed.to_string());
                    }
                    current.clear();
                } else {
                    current.push(ch);
                }
            }
            _ => current.push(ch),
        }
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        segments.push(trimmed.to_string());
    }

    segments
}

fn rule_matches(rule: &SkillCommandRule, invocation: &CommandInvocation) -> bool {
    if !rule.command_tree.is_empty() {
        if invocation.tokens.len() < rule.command_tree.len() {
            return false;
        }
        for (idx, node) in rule.command_tree.iter().enumerate() {
            if invocation.tokens[idx] != *node {
                return false;
            }
        }
    }

    if !rule.contains.is_empty() {
        let raw = invocation.raw.to_ascii_lowercase();
        for needle in &rule.contains {
            let matched_in_tokens = invocation.tokens.iter().any(|token| token.contains(needle));
            if !matched_in_tokens && !raw.contains(needle) {
                return false;
            }
        }
    }

    true
}

fn find_outside_whitelist_path(
    script_path: &Path,
    command_args: &[String],
    script_text: Option<&str>,
    whitelist: &[PathBuf],
) -> Option<(String, String, PathBuf)> {
    let script_file = normalize_root_path(script_path.to_path_buf());
    let script_dir = script_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    for (token, source) in collect_path_candidates(command_args, script_text) {
        let resolved = resolve_candidate_path(&script_dir, &token);
        if resolved == script_file {
            continue;
        }
        let allowed = whitelist
            .iter()
            .any(|root| path_is_within_root(&resolved, root));
        if !allowed {
            return Some((token, source, resolved));
        }
    }
    None
}

fn collect_path_candidates(
    command_args: &[String],
    script_text: Option<&str>,
) -> Vec<(String, String)> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    for (index, arg) in command_args.iter().enumerate() {
        for token in split_shell_tokens(arg) {
            let cleaned = strip_matching_quotes(&token);
            if is_path_like(cleaned) && seen.insert(cleaned.to_string()) {
                candidates.push((cleaned.to_string(), format!("arg:{index}")));
            }
        }
    }

    if let Some(script) = script_text {
        for (line_no, line) in script.lines().enumerate().take(300) {
            let trimmed = line.trim();
            if trimmed.is_empty() || is_comment_line(trimmed) {
                continue;
            }
            let tokens = split_shell_tokens(trimmed);
            for token in tokens.into_iter().skip(1) {
                let cleaned = strip_matching_quotes(&token);
                if is_path_like(cleaned) && seen.insert(cleaned.to_string()) {
                    candidates.push((cleaned.to_string(), format!("script:{}", line_no + 1)));
                }
            }
        }
    }

    candidates
}

fn resolve_candidate_path(script_dir: &Path, token: &str) -> PathBuf {
    let expanded = expand_home_path(token);
    let raw = PathBuf::from(expanded);
    let absolute = if raw.is_absolute() {
        raw
    } else {
        script_dir.join(raw)
    };
    normalize_root_path(absolute)
}

fn resolve_builtin_cwd(
    tool: BuiltinTool,
    skills: &SkillsConfig,
    cwd: Option<&str>,
) -> Result<PathBuf, ToolResult> {
    let allowed_roots = configured_allowed_dir_paths(skills);
    let allowed_dirs = allowed_roots
        .iter()
        .map(|path| normalize_display_path(path))
        .collect::<Vec<_>>();
    if allowed_roots.is_empty() {
        return Err(cwd_error_result(
            tool,
            "skills requires at least one allowed directory",
            cwd,
            None,
            allowed_dirs,
        ));
    }

    let selected = if let Some(cwd) = cwd.map(str::trim).filter(|value| !value.is_empty()) {
        PathBuf::from(cwd)
    } else {
        match allowed_roots.as_slice() {
            [only] => only.clone(),
            _ => {
                let message = "cwd is required because multiple allowed directories are configured; ask the user which directory to operate in";
                return Err(cwd_error_result(tool, message, cwd, None, allowed_dirs));
            }
        }
    };
    let normalized = normalize_root_path(selected);
    if !allowed_roots
        .iter()
        .any(|root| path_is_within_root(&normalized, root))
    {
        let message = "cwd must be inside one configured allowed directory";
        return Err(cwd_error_result(
            tool,
            message,
            cwd,
            Some(&normalized),
            allowed_dirs,
        ));
    }

    if !normalized.exists() || !normalized.is_dir() {
        let message = format!(
            "working directory must be an existing directory: {}",
            normalized.to_string_lossy()
        );
        return Err(cwd_error_result(
            tool,
            &message,
            cwd,
            Some(&normalized),
            allowed_dirs,
        ));
    }

    Ok(normalized)
}

fn cwd_error_result(
    tool: BuiltinTool,
    message: &str,
    cwd: Option<&str>,
    resolved_cwd: Option<&Path>,
    allowed_dirs: Vec<String>,
) -> ToolResult {
    let requested_cwd = cwd
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let resolved_cwd = resolved_cwd.map(normalize_display_path);
    tool_error(
        format!(
            "{message}\nAllowed directories:\n{}",
            allowed_dirs_text(&allowed_dirs)
        ),
        json!({
            "status": "error",
            "code": "InvalidCwd",
            "message": message,
            "tool": tool.name(),
            "requestedCwd": requested_cwd,
            "resolvedCwd": resolved_cwd,
            "allowedDirectories": allowed_dirs,
            "nextStep": "Ask the user which allowed directory should be used as cwd, then retry with cwd set to that directory or a child directory."
        }),
    )
}

fn allowed_dirs_text(allowed_dirs: &[String]) -> String {
    if allowed_dirs.is_empty() {
        return "- <none configured>".to_string();
    }
    allowed_dirs
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn configured_allowed_dir_paths(skills: &SkillsConfig) -> Vec<PathBuf> {
    skills
        .policy
        .path_guard
        .whitelist_dirs
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| normalize_root_path(PathBuf::from(value)))
        .collect()
}

fn evaluate_paths_policy(skills: &SkillsConfig, paths: &[PathBuf]) -> PolicyDecision {
    if !skills.policy.path_guard.enabled {
        return PolicyDecision::Allow;
    }

    let whitelist = skills
        .policy
        .path_guard
        .whitelist_dirs
        .iter()
        .map(PathBuf::from)
        .map(normalize_root_path)
        .collect::<Vec<_>>();
    if whitelist.is_empty() {
        return PolicyDecision::Deny("skills allowed directories are empty".to_string());
    }

    for path in paths {
        let resolved = normalize_root_path(path.clone());
        let allowed = whitelist
            .iter()
            .any(|root| path_is_within_root(&resolved, root));
        if !allowed {
            let reason = format!(
                "path '{}' is outside allowed directories",
                resolved.to_string_lossy()
            );
            return action_to_decision(
                &skills.policy.path_guard.on_violation,
                reason,
                "path_outside_allowed_dir".to_string(),
            );
        }
    }

    PolicyDecision::Allow
}

fn resolve_file_operation_path(cwd: &Path, raw: &str) -> Result<PathBuf, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest(
            "file operation path cannot be empty".to_string(),
        ));
    }
    let expanded = expand_home_path(trimmed);
    let path = PathBuf::from(expanded);
    let absolute = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    Ok(normalize_root_path(absolute))
}


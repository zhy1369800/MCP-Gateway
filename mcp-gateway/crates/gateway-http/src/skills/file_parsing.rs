fn truncate_preview(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut out = input.chars().take(max_chars).collect::<String>();
    out.push_str("\n[preview truncated]");
    out
}

fn normalize_root_path(path: PathBuf) -> PathBuf {
    let lexical = normalize_lexical_path(&path);
    let normalized = if lexical.exists() {
        match std::fs::canonicalize(&lexical) {
            Ok(value) => value,
            Err(_) => lexical,
        }
    } else {
        normalize_existing_ancestor_path(&lexical).unwrap_or(lexical)
    };
    normalize_windows_verbatim_path(normalize_lexical_path(&normalized))
}

fn normalize_lexical_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn normalize_existing_ancestor_path(path: &Path) -> Option<PathBuf> {
    let mut ancestor = path;
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        suffix.push(ancestor.file_name()?.to_os_string());
        ancestor = ancestor.parent()?;
    }

    let mut normalized = std::fs::canonicalize(ancestor)
        .unwrap_or_else(|_| normalize_lexical_path(ancestor));
    for component in suffix.iter().rev() {
        normalized.push(component);
    }
    Some(normalized)
}

#[cfg(target_os = "windows")]
fn normalize_windows_verbatim_path(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy().to_string();
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = raw.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path
}

#[cfg(not(target_os = "windows"))]
fn normalize_windows_verbatim_path(path: PathBuf) -> PathBuf {
    path
}

fn path_is_within_root(path: &Path, root: &Path) -> bool {
    let path = normalize_root_path(path.to_path_buf());
    let root = normalize_root_path(root.to_path_buf());

    #[cfg(target_os = "windows")]
    {
        let path_components = path_case_folded_components(&path);
        let root_components = path_case_folded_components(&root);
        path_components.starts_with(&root_components)
    }

    #[cfg(not(target_os = "windows"))]
    {
        path.starts_with(root)
    }
}

#[cfg(target_os = "windows")]
fn path_case_folded_components(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect()
}

fn expand_home_path(token: &str) -> String {
    if let Some(rest) = token.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    if token == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().to_string();
        }
    }
    token.to_string()
}

fn normalize_command_token(token: &str) -> String {
    let cleaned = strip_matching_quotes(token);
    Path::new(cleaned)
        .file_name()
        .and_then(|item| item.to_str())
        .unwrap_or(cleaned)
        .to_ascii_lowercase()
}

fn is_comment_line(line: &str) -> bool {
    line.starts_with('#')
        || line.starts_with("//")
        || line.starts_with("::")
        || line
            .get(0..3)
            .map(|prefix| prefix.eq_ignore_ascii_case("rem"))
            .unwrap_or(false)
}

fn split_shell_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for ch in line.chars() {
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }

        match ch {
            '\'' | '"' => quote = Some(ch),
            ' ' | '\t' => {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn strip_matching_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn is_path_like(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    if token.starts_with('-') {
        return false;
    }
    if token.contains("://") {
        return false;
    }
    if token.starts_with("~/")
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with('\\')
        || token.starts_with('/')
        || token.contains('\\')
        || token.contains('/')
    {
        return true;
    }
    let bytes = token.as_bytes();
    bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn should_disable_output_truncation(program: &str, command_args: &[String]) -> bool {
    let normalized_program = normalize_command_token(program);
    let is_markdown_reader = matches!(
        normalized_program.as_str(),
        "cat" | "type" | "get-content" | "gc"
    );
    if !is_markdown_reader {
        return false;
    }

    command_args.iter().any(|arg| {
        let candidate = strip_matching_quotes(arg).trim();
        if candidate.is_empty() || candidate.starts_with('-') {
            return false;
        }

        let file_name = Path::new(candidate)
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or(candidate);
        let lowered = file_name.to_ascii_lowercase();
        lowered == "skill.md" || lowered.ends_with(".md") || lowered.ends_with(".markdown")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn prepend_path_entries_puts_bundled_paths_first() {
        let bundled = PathBuf::from("/app/tools");
        let existing_a = PathBuf::from("/usr/bin");
        let existing_b = PathBuf::from("/bin");
        let existing_path = env::join_paths([existing_a.clone(), existing_b.clone()])
            .expect("join existing test path");

        let updated = prepend_path_entries(
            std::slice::from_ref(&bundled),
            Some(existing_path.as_os_str()),
        )
        .expect("join updated test path");
        let paths = env::split_paths(&updated).collect::<Vec<_>>();

        assert_eq!(paths, vec![bundled, existing_a, existing_b]);
    }

    #[test]
    fn command_tree_rule_can_trigger_confirmation() {
        let skills = SkillsConfig::default();
        let raw = "sh script.sh";
        let decision = evaluate_policy(
            &skills,
            "sh",
            &[String::from("script.sh")],
            raw,
            Path::new("scripts/script.sh"),
            Some("rm test.txt"),
        );

        match decision {
            PolicyDecision::Confirm { reason, reason_key } => {
                assert!(reason.contains("confirm-rm"));
                assert_eq!(reason_key, "file_deletion");
            }
            _ => panic!("expected confirm decision"),
        }
    }

    #[test]
    fn path_guard_blocks_outside_whitelist() {
        let mut skills = SkillsConfig::default();
        let base = std::env::current_dir().expect("cwd");
        let allowed = base.join("allowed-zone");
        let blocked = base.join("outside-zone").join("target.txt");

        skills.policy.path_guard.enabled = true;
        skills.policy.path_guard.whitelist_dirs = vec![allowed.to_string_lossy().to_string()];
        skills.policy.path_guard.on_violation = SkillPolicyAction::Deny;
        skills.policy.rules.clear();

        let raw = format!("python tool.py {}", blocked.to_string_lossy());
        let decision = evaluate_policy(
            &skills,
            "python",
            &[
                String::from("tool.py"),
                blocked.to_string_lossy().to_string(),
            ],
            &raw,
            &allowed.join("scripts").join("tool.py"),
            None,
        );

        match decision {
            PolicyDecision::Deny(reason) => {
                assert!(reason.contains("outside whitelist"));
            }
            _ => panic!("expected deny decision"),
        }
    }

    #[test]
    fn path_guard_allows_nonexistent_descendant_under_allowed_root() {
        let temp_root = std::env::temp_dir().join(format!("mcp-gateway-{}", Uuid::new_v4()));
        let allowed = temp_root.join("allowed");
        fs::create_dir_all(&allowed).expect("create allowed test dir");
        let target = allowed.join("child").join("new-file.txt");

        let mut skills = SkillsConfig::default();
        skills.policy.path_guard.enabled = true;
        skills.policy.path_guard.whitelist_dirs = vec![allowed.to_string_lossy().to_string()];
        skills.policy.path_guard.on_violation = SkillPolicyAction::Deny;
        skills.policy.rules.clear();
        skills.policy.default_action = SkillPolicyAction::Allow;

        let raw = format!("New-Item {}", target.to_string_lossy());
        let decision = evaluate_policy(
            &skills,
            "New-Item",
            &[target.to_string_lossy().to_string()],
            &raw,
            &allowed,
            None,
        );

        match decision {
            PolicyDecision::Allow => {}
            other => panic!("expected allow decision for descendant path, got {other:?}"),
        }

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn path_guard_does_not_treat_sibling_prefix_as_descendant() {
        let temp_root = std::env::temp_dir().join(format!("mcp-gateway-{}", Uuid::new_v4()));
        let allowed = temp_root.join("allowed");
        let sibling = temp_root.join("allowed-sibling").join("target.txt");
        fs::create_dir_all(&allowed).expect("create allowed test dir");

        let mut skills = SkillsConfig::default();
        skills.policy.path_guard.enabled = true;
        skills.policy.path_guard.whitelist_dirs = vec![allowed.to_string_lossy().to_string()];
        skills.policy.path_guard.on_violation = SkillPolicyAction::Deny;
        skills.policy.rules.clear();
        skills.policy.default_action = SkillPolicyAction::Allow;

        let raw = format!("New-Item {}", sibling.to_string_lossy());
        let decision = evaluate_policy(
            &skills,
            "New-Item",
            &[sibling.to_string_lossy().to_string()],
            &raw,
            &allowed,
            None,
        );

        match decision {
            PolicyDecision::Deny(reason) => {
                assert!(reason.contains("outside whitelist"));
            }
            other => panic!("expected deny decision for sibling path, got {other:?}"),
        }

        let _ = fs::remove_dir_all(temp_root);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn path_guard_allows_windows_child_with_different_drive_case() {
        let temp_root = std::env::temp_dir().join(format!("mcp-gateway-{}", Uuid::new_v4()));
        let allowed = temp_root.join("allowed");
        fs::create_dir_all(&allowed).expect("create allowed test dir");

        let allowed_text = allowed.to_string_lossy().to_string();
        let target_text = flip_windows_drive_case(&allowed_text) + r"\child\new-file.txt";

        let mut skills = SkillsConfig::default();
        skills.policy.path_guard.enabled = true;
        skills.policy.path_guard.whitelist_dirs = vec![allowed_text];
        skills.policy.path_guard.on_violation = SkillPolicyAction::Deny;
        skills.policy.rules.clear();
        skills.policy.default_action = SkillPolicyAction::Allow;

        let raw = format!("New-Item {target_text}");
        let decision = evaluate_policy(
            &skills,
            "New-Item",
            &[target_text],
            &raw,
            &allowed,
            None,
        );

        match decision {
            PolicyDecision::Allow => {}
            other => panic!("expected allow decision for case-varied child path, got {other:?}"),
        }

        let _ = fs::remove_dir_all(temp_root);
    }

    #[cfg(target_os = "windows")]
    fn flip_windows_drive_case(path: &str) -> String {
        let mut chars = path.chars();
        let Some(first) = chars.next() else {
            return path.to_string();
        };
        let Some(':') = chars.next() else {
            return path.to_string();
        };
        let flipped = if first.is_ascii_lowercase() {
            first.to_ascii_uppercase()
        } else {
            first.to_ascii_lowercase()
        };
        format!("{flipped}:{}", chars.collect::<String>())
    }

    #[test]
    fn builtin_cwd_outside_allowed_dirs_returns_choices() {
        let temp_root = std::env::temp_dir().join(format!("mcp-gateway-{}", Uuid::new_v4()));
        let allowed = temp_root.join("allowed");
        let outside = temp_root.join("outside");
        fs::create_dir_all(&allowed).expect("create allowed test dir");
        fs::create_dir_all(&outside).expect("create outside test dir");

        let mut skills = SkillsConfig::default();
        skills.policy.path_guard.enabled = false;
        skills.policy.path_guard.whitelist_dirs = vec![allowed.to_string_lossy().to_string()];
        skills.policy.path_guard.on_violation = SkillPolicyAction::Allow;

        let outside_cwd = outside.to_string_lossy().to_string();
        let result = resolve_builtin_cwd(BuiltinTool::ShellCommand, &skills, Some(&outside_cwd))
            .expect_err("outside cwd should be rejected");

        assert!(result.is_error);
        assert_eq!(result.structured["code"], "InvalidCwd");
        assert_eq!(
            result.structured["message"],
            "cwd must be inside one configured allowed directory"
        );
        assert_eq!(
            result.structured["allowedDirectories"][0],
            normalize_display_path(&normalize_root_path(allowed.clone()))
        );
        assert_eq!(
            result.structured["resolvedCwd"],
            normalize_display_path(&normalize_root_path(outside))
        );
        assert!(result.text.contains("Allowed directories:"));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn builtin_cwd_missing_with_multiple_allowed_dirs_returns_choices() {
        let temp_root = std::env::temp_dir().join(format!("mcp-gateway-{}", Uuid::new_v4()));
        let first = temp_root.join("first");
        let second = temp_root.join("second");
        fs::create_dir_all(&first).expect("create first test dir");
        fs::create_dir_all(&second).expect("create second test dir");

        let mut skills = SkillsConfig::default();
        skills.policy.path_guard.whitelist_dirs = vec![
            first.to_string_lossy().to_string(),
            second.to_string_lossy().to_string(),
        ];

        let result = resolve_builtin_cwd(BuiltinTool::ShellCommand, &skills, None)
            .expect_err("ambiguous cwd should be rejected");

        assert!(result.is_error);
        assert_eq!(result.structured["code"], "InvalidCwd");
        assert_eq!(
            result.structured["message"],
            "cwd is required because multiple allowed directories are configured; ask the user which directory to operate in"
        );
        assert_eq!(
            result.structured["allowedDirectories"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(result.structured["requestedCwd"].is_null());

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn default_policy_allow_when_no_match() {
        let mut skills = SkillsConfig::default();
        skills.policy.rules.clear();
        skills.policy.path_guard.enabled = false;
        skills.policy.default_action = SkillPolicyAction::Allow;

        let raw = "python safe.py --help";
        let decision = evaluate_policy(
            &skills,
            "python",
            &[String::from("safe.py"), String::from("--help")],
            raw,
            Path::new("scripts/safe.py"),
            Some("print('ok')"),
        );

        match decision {
            PolicyDecision::Allow => {}
            _ => panic!("expected allow decision"),
        }
    }

    #[test]
    fn default_text_write_commands_require_confirmation() {
        let skills = SkillsConfig::default();
        let raw = "Set-Content -Path note.txt -Value hello";
        let decision = evaluate_policy(
            &skills,
            "Set-Content",
            &[
                String::from("-Path"),
                String::from("note.txt"),
                String::from("-Value"),
                String::from("hello"),
            ],
            raw,
            Path::new("scripts/write.ps1"),
            None,
        );

        match decision {
            PolicyDecision::Confirm { reason, reason_key } => {
                assert!(reason.contains("confirm-set-content"));
                assert_eq!(reason_key, "text_editing");
            }
            _ => panic!("expected confirm decision"),
        }
    }

    #[test]
    fn policy_denied_text_explains_gateway_rejection() {
        let text = mcp_gateway_policy_denied_text("matched default policy");

        assert!(text.contains("MCP Gateway"));
        assert!(text.contains("已拒绝此命令"));
        assert!(text.contains("confirm"));
        assert!(text.contains("删除或禁用"));
    }

    #[test]
    fn path_guard_denied_text_explains_allowed_directory_boundary() {
        let text =
            mcp_gateway_policy_denied_text("path 'D:/outside/file.txt' is outside allowed directories");

        assert!(text.contains("可访问目录"));
        assert!(text.contains("越界访问策略"));
        assert!(text.contains("越界动作"));
        assert!(text.contains("加入“可访问目录”"));

        let help = mcp_gateway_policy_denied_help("path 'D:/outside/file.txt' is outside allowed directories");
        assert_eq!(help["decisionScope"], "this_request_only");
        assert!(help["suggestedActions"]
            .as_array()
            .expect("suggested actions")
            .iter()
            .any(|item| item == "add_trusted_directory_to_allowed_directories"));
    }

    #[test]
    fn confirmation_rejected_text_explains_one_time_scope() {
        let result = confirmation_rejected_result("shell_command", "confirm-1", false);

        assert!(result.is_error);
        assert!(result.text.contains("拒绝本次"));
        assert!(result.text.contains("只针对本次请求"));
        assert_eq!(result.structured["reason"], "user_rejected");
        assert_eq!(result.structured["decisionScope"], "this_request_only");
        assert_eq!(
            result.structured["decisionAppliesOnlyToCurrentRequest"],
            true
        );
    }

    #[test]
    fn confirmation_timeout_text_explains_retry_hold() {
        let result = confirmation_rejected_result("shell_command", "confirm-2", true);

        assert!(result.is_error);
        assert!(result.text.contains("60 秒"));
        assert!(result.text.contains("120 秒"));
        assert!(result.text.contains("只针对本次请求"));
        assert_eq!(result.structured["reason"], "timeout");
        assert_eq!(result.structured["decisionTimeoutSeconds"], 60);
        assert_eq!(result.structured["timeoutRetryHoldSeconds"], 120);
    }

    #[test]
    fn tool_args_require_exec_not_legacy_cmd() {
        let args = decode_tool_args::<BuiltinShellArgs>(&json!({
            "exec": "Get-ChildItem -Name",
            "cwd": "D:/workspace"
        }))
        .expect("exec argument should decode");
        assert_eq!(args.exec, "Get-ChildItem -Name");

        let error = decode_tool_args::<BuiltinShellArgs>(&json!({
            "cmd": "Get-ChildItem -Name",
            "cwd": "D:/workspace"
        }))
        .expect_err("legacy cmd argument should not decode");
        assert!(error.message().contains("missing field `exec`"));
    }

    #[test]
    fn shell_wrapper_chain_is_blocked_by_default_rules() {
        let skills = SkillsConfig::default();
        let raw = "bash -lc \"rm -rf /tmp/demo\"";
        let decision = evaluate_policy(
            &skills,
            "bash",
            &[String::from("-lc"), String::from("rm -rf /tmp/demo")],
            raw,
            Path::new("scripts/safe.sh"),
            None,
        );

        match decision {
            PolicyDecision::Deny(reason) => {
                assert!(reason.contains("deny-bash-lc"));
            }
            _ => panic!("expected deny decision"),
        }
    }

    #[tokio::test]
    async fn confirmation_wait_returns_approved_when_user_approves() {
        let service = SkillsService::new();
        let confirmation = match service
            .create_confirmation(
                "skill",
                "cmd",
                &[String::from("cmd")],
                "cmd",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created, got {other:?}"),
        };
        let confirmation_id = confirmation.id.clone();
        let service_for_approve = service.clone();
        let id_for_approve = confirmation_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = service_for_approve
                .approve_confirmation(&id_for_approve)
                .await;
        });

        let outcome = service
            .wait_for_confirmation_decision(
                &confirmation_id,
                Duration::from_secs(2),
                Duration::from_millis(10),
            )
            .await;
        match outcome {
            ConfirmationWaitOutcome::Approved => {}
            _ => panic!("expected approved outcome"),
        }
    }

    #[tokio::test]
    async fn create_confirmation_creates_distinct_requests_for_same_command() {
        let service = SkillsService::new();
        let first = match service
            .create_confirmation(
                "skill",
                "cmd-repeat",
                &[String::from("cmd-repeat")],
                "cmd-repeat",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created, got {other:?}"),
        };
        // 同指纹第二次调用 → Reused，复用同一条记录
        let second = match service
            .create_confirmation(
                "skill",
                "cmd-repeat",
                &[String::from("cmd-repeat")],
                "cmd-repeat",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Reused(c) => c,
            other => panic!("expected Reused, got {other:?}"),
        };
        assert_eq!(first.id, second.id);
        assert_eq!(first.status, ConfirmationStatus::Pending);
        assert_eq!(second.status, ConfirmationStatus::Pending);
    }

    #[tokio::test]
    async fn confirmation_wait_returns_rejected_when_user_rejects() {
        let service = SkillsService::new();
        let confirmation = match service
            .create_confirmation(
                "skill",
                "cmd-reject-wait",
                &[String::from("cmd-reject-wait")],
                "cmd-reject-wait",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created, got {other:?}"),
        };
        let confirmation_id = confirmation.id.clone();
        let service_for_reject = service.clone();
        let id_for_reject = confirmation_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = service_for_reject.reject_confirmation(&id_for_reject).await;
        });

        let outcome = service
            .wait_for_confirmation_decision(
                &confirmation_id,
                Duration::from_secs(2),
                Duration::from_millis(10),
            )
            .await;
        match outcome {
            ConfirmationWaitOutcome::Rejected => {}
            _ => panic!("expected rejected outcome"),
        }
    }

    #[tokio::test]
    async fn rejecting_one_confirmation_rejects_pending_duplicates() {
        let service = SkillsService::new();
        // 第一次：新建
        let first = match service
            .create_confirmation(
                "skill",
                "cmd-dup",
                &[String::from("cmd-dup")],
                "cmd-dup",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created for first, got {other:?}"),
        };
        // 第二次同指纹：复用
        let second = match service
            .create_confirmation(
                "skill",
                "cmd-dup",
                &[String::from("cmd-dup")],
                "cmd-dup",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Reused(c) => c,
            other => panic!("expected Reused for second, got {other:?}"),
        };
        assert_eq!(
            first.id, second.id,
            "same fingerprint should reuse the same entry"
        );
        let _ = service
            .reject_confirmation(&first.id)
            .await
            .expect("reject first");

        let second_outcome = service
            .wait_for_confirmation_decision(
                &second.id,
                Duration::from_secs(1),
                Duration::from_millis(10),
            )
            .await;
        match second_outcome {
            ConfirmationWaitOutcome::Rejected => {}
            _ => panic!("expected duplicate request to be rejected"),
        }
    }

    #[tokio::test]
    async fn concurrent_create_confirmation_requests_are_distinct() {
        let service = SkillsService::new();
        let s1 = service.clone();
        let s2 = service.clone();

        let one = tokio::spawn(async move {
            s1.create_confirmation(
                "skill",
                "cmd-dedupe",
                &[String::from("cmd-dedupe")],
                "cmd-dedupe",
                "need approval",
            )
            .await
        });
        let two = tokio::spawn(async move {
            s2.create_confirmation(
                "skill",
                "cmd-dedupe",
                &[String::from("cmd-dedupe")],
                "cmd-dedupe",
                "need approval",
            )
            .await
        });

        let first = one.await.expect("first join");
        let second = two.await.expect("second join");
        // 并发同指纹：一个 Created，另一个 Reused，二者复用同一条记录
        let first_id = match &first {
            CreateConfirmationResult::Created(c) | CreateConfirmationResult::Reused(c) => {
                c.id.clone()
            }
            CreateConfirmationResult::AlreadyTimedOut(id) => id.clone(),
        };
        let second_id = match &second {
            CreateConfirmationResult::Created(c) | CreateConfirmationResult::Reused(c) => {
                c.id.clone()
            }
            CreateConfirmationResult::AlreadyTimedOut(id) => id.clone(),
        };
        assert_eq!(
            first_id, second_id,
            "concurrent same-fingerprint calls should share one entry"
        );
    }

    #[tokio::test]
    async fn confirmation_wait_times_out_when_not_confirmed() {
        let service = SkillsService::new();
        let confirmation = match service
            .create_confirmation(
                "skill",
                "cmd-timeout",
                &[String::from("cmd-timeout")],
                "cmd-timeout",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created, got {other:?}"),
        };

        let outcome = service
            .wait_for_confirmation_decision(
                &confirmation.id,
                Duration::from_millis(40),
                Duration::from_millis(10),
            )
            .await;
        match outcome {
            ConfirmationWaitOutcome::TimedOut => {}
            _ => panic!("expected timed out outcome"),
        }
        let pending = service.list_pending_confirmations().await;
        assert!(pending.iter().all(|item| item.id != confirmation.id));
    }

    #[tokio::test]
    async fn rejected_confirmation_then_same_command_still_creates_new_request() {
        let service = SkillsService::new();
        let first = match service
            .create_confirmation(
                "skill",
                "cmd-reject",
                &[String::from("cmd-reject")],
                "cmd-reject",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created, got {other:?}"),
        };
        // 用户手动拒绝（timed_out=false）→ 允许重新发起
        let _ = service.reject_confirmation(&first.id).await;

        let reused = match service
            .create_confirmation(
                "skill",
                "cmd-reject",
                &[String::from("cmd-reject")],
                "cmd-reject",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created (new entry after user reject), got {other:?}"),
        };
        assert_ne!(reused.id, first.id);
        assert_eq!(reused.status, ConfirmationStatus::Pending);

        let outcome = service
            .wait_for_confirmation_decision(
                &first.id,
                Duration::from_secs(1),
                Duration::from_millis(10),
            )
            .await;
        match outcome {
            ConfirmationWaitOutcome::Rejected => {}
            _ => panic!("expected rejected outcome"),
        }
    }

    #[tokio::test]
    async fn approved_confirmation_then_same_command_still_creates_new_request() {
        let service = SkillsService::new();
        let first = match service
            .create_confirmation(
                "skill",
                "cmd-approve",
                &[String::from("cmd-approve")],
                "cmd-approve",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created, got {other:?}"),
        };
        let _ = service.approve_confirmation(&first.id).await;

        let reused = match service
            .create_confirmation(
                "skill",
                "cmd-approve",
                &[String::from("cmd-approve")],
                "cmd-approve",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created (new entry after approve), got {other:?}"),
        };
        assert_ne!(reused.id, first.id);
        assert_eq!(reused.status, ConfirmationStatus::Pending);

        let outcome = service
            .wait_for_confirmation_decision(
                &first.id,
                Duration::from_secs(1),
                Duration::from_millis(10),
            )
            .await;
        match outcome {
            ConfirmationWaitOutcome::Approved => {}
            _ => panic!("expected approved outcome"),
        }
    }

    #[test]
    fn chained_remove_item_requires_confirmation() {
        let skills = SkillsConfig::default();
        let raw = "Set-Location 'D:/Code_Save/demo'; Remove-Item -Force package-lock.json -ErrorAction SilentlyContinue; npm install";
        let tokens = split_shell_tokens(raw);
        let decision = evaluate_policy(
            &skills,
            &tokens[0],
            &tokens[1..],
            raw,
            Path::new("scripts/safe.ps1"),
            None,
        );

        match decision {
            PolicyDecision::Confirm { reason, reason_key } => {
                assert!(reason.contains("confirm-remove-item"));
                assert_eq!(reason_key, "powershell_deletion");
            }
            _ => panic!("expected confirm decision"),
        }
    }

    #[test]
    fn markdown_read_commands_disable_output_truncation() {
        assert!(should_disable_output_truncation(
            "Get-Content",
            &[
                String::from("-Raw"),
                String::from("D:/skills/demo/SKILL.md")
            ]
        ));
        assert!(should_disable_output_truncation(
            "cat",
            &[String::from("./references/weather_info.md")]
        ));
        assert!(!should_disable_output_truncation(
            "python",
            &[String::from("scripts/tool.py")]
        ));
    }

    #[test]
    fn command_output_text_formats_stdout_and_stderr() {
        assert_eq!(command_output_text("hello\n", ""), "hello");
        assert_eq!(command_output_text("", "warn\n"), "warn");
        assert_eq!(
            command_output_text("ok\n", "warn\n"),
            "ok\n\n[stderr]\nwarn"
        );
    }

    #[test]
    fn command_failure_text_includes_exit_code() {
        assert_eq!(
            command_failure_text(2, "", ""),
            "command finished with non-zero exit code (2) and no output"
        );
        assert_eq!(
            command_failure_text(2, "oops\n", ""),
            "command finished with non-zero exit code (2).\noops"
        );
    }

    #[test]
    fn command_timeout_text_includes_last_output() {
        assert_eq!(
            command_timeout_text(60_000, "", ""),
            "command timed out after 60000ms and produced no output"
        );
        assert_eq!(
            command_timeout_text(60_000, "waiting for input\n", ""),
            "command timed out after 60000ms.\nLast output:\nwaiting for input"
        );
    }

    #[tokio::test]
    async fn execute_skill_command_timeout_returns_last_output() {
        let command_text = if cfg!(target_os = "windows") {
            "Write-Output 'waiting for input'; Start-Sleep -Seconds 5"
        } else {
            "printf 'waiting for input\\n'; sleep 5"
        };
        let timeout_ms = if cfg!(target_os = "windows") {
            800
        } else {
            250
        };
        let (runner, runner_args) = shell_command_for_current_os(command_text);
        let mut command = Command::new(&runner);
        command
            .args(&runner_args)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        configure_skill_command(&mut command);

        let error = execute_skill_command(&mut command, timeout_ms, 4096, false, None, None)
            .await
            .expect_err("command should time out");

        match error {
            AppError::Upstream(message) => {
                assert!(message.contains(&format!("command timed out after {timeout_ms}ms")));
                assert!(message.contains("Last output:"));
                assert!(message.contains("waiting for input"));
            }
            other => panic!("expected upstream timeout error, got {other:?}"),
        }
    }

    #[test]
    fn discover_skills_sync_finds_root_and_nested_skill_directories() {
        let sandbox = std::env::temp_dir().join(format!("gateway-skills-{}", Uuid::new_v4()));
        let root_skill_dir = sandbox.join("root-skill");
        let nested_skill_dir = root_skill_dir.join("nested").join("child-skill");
        std::fs::create_dir_all(&nested_skill_dir).expect("create test directories");
        std::fs::write(
            root_skill_dir.join("SKILL.md"),
            "---\nname: root-skill\n---\n# Root\n",
        )
        .expect("write root SKILL.md");
        std::fs::write(
            nested_skill_dir.join("SKILL.md"),
            "---\nname: child-skill\n---\n# Child\n",
        )
        .expect("write nested SKILL.md");

        let roots = vec![root_skill_dir.to_string_lossy().to_string()];
        let discovered = discover_skills_sync(&roots).expect("discover skills");
        let names = discovered
            .iter()
            .map(|skill| skill.skill.clone())
            .collect::<HashSet<_>>();

        assert!(names.contains("root-skill"));
        assert!(names.contains("child-skill"));

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn parse_frontmatter_fields_reads_yaml_mapping_and_metadata() {
        let sandbox = std::env::temp_dir().join(format!("gateway-frontmatter-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let skill_md = sandbox.join("SKILL.md");
        std::fs::write(
            &skill_md,
            "---\nname: demo-skill\ndescription: Demo description\nmetadata:\n  tags:\n    - remotion\n    - video\n  image:\n    format: png\n---\n# Demo\n",
        )
        .expect("write SKILL.md");

        let parsed = parse_frontmatter_fields(&skill_md).expect("parse frontmatter");
        assert_eq!(parsed.name, "demo-skill");
        assert_eq!(parsed.description, "Demo description");
        assert!(parsed
            .metadata
            .contains("\"tags\":[\"remotion\",\"video\"]"));
        assert!(parsed.metadata.contains("\"image\":{\"format\":\"png\"}"));
        assert!(parsed.block.contains("metadata:"));

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn builtin_tool_definitions_include_shell_command_description() {
        let os = current_os_label();
        let now = Utc::now().to_rfc3339();
        let tools = builtin_tool_definitions(os, &now, &BuiltinToolsConfig::default());
        let shell_tool = tools
            .iter()
            .find(|item| item.get("name").and_then(Value::as_str) == Some("shell_command"))
            .expect("shell command tool exists");
        let shell_description = shell_tool
            .get("description")
            .and_then(Value::as_str)
            .expect("shell description");
        assert!(shell_description.contains("builtin://shell_command/SKILL.md"));
        assert!(shell_description.contains("MANDATORY BEFORE USE"));
        assert!(shell_description.contains("you MUST first read its full SKILL.md"));
        assert!(shell_description.contains("returned markdown content"));
        assert!(shell_description.contains("partial-read tricks"));
        assert!(shell_description.contains("prefer rg or rg --files"));
        assert!(shell_description.contains("node_modules"));
        assert!(!shell_description.contains("structuredContent.skillToken"));
        assert!(shell_description.contains("Front matter summary:"));
        assert!(shell_description.contains("SKILL.md URI:"));
        assert!(!shell_description.contains("Prefer fast discovery commands"));

        let names: Vec<&str> = tools
            .iter()
            .filter_map(|item| item.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            vec![
                "read_file",
                "shell_command",
                "multi_edit_file",
                "task-planning",
                "chrome-cdp",
                "chat-plus-adapter-debugger"
            ]
        );
    }

    #[test]
    fn builtin_skill_docs_are_served_from_embedded_skill_md() {
        let (tool, path) =
            builtin_skill_doc_read("Get-Content -Raw builtin://shell_command/SKILL.md")
                .expect("shell doc read");
        let shell = builtin_skill_doc_result(tool, "doc", path, "abc123".to_string(), false);
        assert!(!shell.is_error);
        assert!(shell.text.contains("# Shell Command"));
        assert!(shell
            .text
            .contains("## Global Search And Discovery Priority"));
        assert!(shell
            .text
            .contains("## Project And Workflow Navigation With Ripgrep"));
        assert!(shell.text.contains("rg --files"));
        assert!(shell.text.contains("Do not use `Get-ChildItem -Recurse`"));
        assert!(shell.text.contains("abc123"));
        assert_eq!(
            shell.structured.get("builtinSkill").and_then(Value::as_str),
            Some("shell_command")
        );
        assert_eq!(
            shell.structured.get("docSource").and_then(Value::as_str),
            Some("embedded")
        );
        assert!(shell.structured.get("skillToken").is_none());

        let (tool, path) = builtin_skill_doc_read("cat builtin://multi_edit_file/SKILL.md")
            .expect("multi_edit_file doc read");
        let multi_edit = builtin_skill_doc_result(tool, "doc", path, "fed456".to_string(), false);
        assert!(!multi_edit.is_error);
        assert!(multi_edit.text.contains("# Multi Edit File"));
        assert!(multi_edit.text.contains("\"edits\""));
        assert!(multi_edit.text.contains("fed456"));
        assert!(multi_edit.structured.get("skillToken").is_none());

        let (tool, path) = builtin_skill_doc_read("cat builtin://task-planning/SKILL.md")
            .expect("task-planning doc read");
        let planning = builtin_skill_doc_result(tool, "doc", path, "plan123".to_string(), true);
        assert!(!planning.is_error);
        assert!(planning.text.contains("# Task Planning"));
        assert!(planning.text.contains("planningId"));
        assert!(planning.text.contains("plan123"));
        assert!(planning.structured.get("skillToken").is_none());

        let (tool, path) = builtin_skill_doc_read("cat builtin://chrome-cdp/SKILL.md")
            .expect("chrome-cdp doc read");
        let cdp = builtin_skill_doc_result(tool, "doc", path, "987abc".to_string(), false);
        assert!(!cdp.is_error);
        assert!(cdp.text.contains("# Chrome CDP"));
        assert!(cdp.text.contains("Chrome DevTools Protocol over WebSocket"));
        assert!(cdp.text.contains("netclear"));
        assert!(cdp.text.contains("CDP_PROFILE_MODE=persistent"));

        let (tool, path) = builtin_skill_doc_read(
            "Get-Content -Raw builtin://chat-plus-adapter-debugger/SKILL.md",
        )
        .expect("chat-plus adapter debugger doc read");
        let adapter = builtin_skill_doc_result(tool, "doc", path, "654fed".to_string(), false);
        assert!(!adapter.is_error);
        assert!(adapter.text.contains("# Chat Plus Adapter Debugger"));
        assert!(adapter.text.contains("decorateBubbles"));
        assert!(adapter.text.contains("capture start"));
        assert!(adapter.text.contains("network get <request-id>"));
        assert!(!adapter.text.contains("recorder-command.mjs"));
        assert!(!adapter
            .text
            .contains("mcp-gateway/crates/gateway-http/builtin-skills"));
        assert!(adapter.structured.get("runtimeAssets").is_none());
        assert_eq!(
            adapter.structured.get("docSource").and_then(Value::as_str),
            Some("embedded")
        );
    }

    #[test]
    fn external_skill_docs_put_token_only_in_markdown_content() {
        let result = skill_doc_result(
            "demo_skill",
            "demo",
            "cat SKILL.md",
            "D:/skills/demo/SKILL.md".to_string(),
            "# Demo Skill\n".to_string(),
            "tok123".to_string(),
        );

        assert!(!result.is_error);
        assert!(result.text.contains("# Demo Skill"));
        assert!(result.text.contains("[skillToken]"));
        assert!(result.text.contains("tok123"));
        assert!(result.structured.get("skillToken").is_none());
        assert_eq!(
            result.structured.get("docSource").and_then(Value::as_str),
            Some("file")
        );
    }

    #[test]
    fn non_documentation_calls_require_skill_md_hash_token() {
        let token = builtin_skill_token(BuiltinTool::MultiEditFile);
        assert_eq!(token.len(), 6);
        assert_eq!(
            token,
            skill_token_from_content(BUILTIN_MULTI_EDIT_FILE_SKILL_MD)
        );

        let missing = validate_skill_token_result(BuiltinTool::MultiEditFile.name(), &token, None)
            .expect("missing token should be rejected");
        assert!(missing.is_error);
        assert_eq!(
            missing.structured.get("code").and_then(Value::as_str),
            Some("SkillTokenRequired")
        );

        let invalid =
            validate_skill_token_result(BuiltinTool::MultiEditFile.name(), &token, Some("bad000"))
                .expect("invalid token should be rejected");
        assert!(invalid.text.contains("invalid skillToken"));

        let accepted =
            validate_skill_token_result(BuiltinTool::MultiEditFile.name(), &token, Some(&token));
        assert!(accepted.is_none());
    }

    #[test]
    fn builtin_chrome_cdp_command_parser_accepts_cdp_forms() {
        assert_eq!(
            parse_builtin_chrome_cdp_args("open https://example.com").expect("parse short"),
            vec!["open", "https://example.com"]
        );
        assert_eq!(
            parse_builtin_chrome_cdp_args("node cdp.mjs net api/chat").expect("parse node"),
            vec!["net", "api/chat"]
        );
        assert_eq!(
            parse_builtin_chrome_cdp_args("netget 123 --full").expect("parse netget"),
            vec!["netget", "123", "--full"]
        );
        assert!(parse_builtin_chrome_cdp_args("npm install unrelated-package").is_err());
        assert!(parse_builtin_chrome_cdp_args("npm install").is_err());
    }

    #[test]
    fn chat_plus_debug_command_parser_accepts_cdp_network_aliases() {
        match parse_chat_plus_debug_command("capture start").expect("parse capture") {
            Some(ChatPlusDebugCommand::CaptureStart) => {}
            other => panic!("unexpected capture command: {other:?}"),
        }

        match parse_chat_plus_debug_command("network search api chat").expect("parse search") {
            Some(ChatPlusDebugCommand::Cdp { command, .. }) => {
                assert_eq!(command, "net api chat");
            }
            other => panic!("unexpected network search command: {other:?}"),
        }

        match parse_chat_plus_debug_command("network get 123 --full").expect("parse get") {
            Some(ChatPlusDebugCommand::Cdp { command, .. }) => {
                assert_eq!(command, "netget 123 --full");
            }
            other => panic!("unexpected network get command: {other:?}"),
        }

        assert!(parse_chat_plus_debug_command("unsupported action")
            .expect("parse unsupported")
            .is_none());
    }

    #[test]
    fn file_edit_model_delta_omits_full_file_contents() {
        let delta = FileEditDelta {
            exact: true,
            changes: vec![FileEditChange::Update {
                path: "src/lib.rs".to_string(),
                move_path: None,
                old_content: "old\ncontent\n".to_string(),
                new_content: "new\ncontent\n".to_string(),
                overwritten_move_content: None,
            }],
        };

        let model_delta = edit_delta_for_model(&delta);
        let serialized = serde_json::to_string(&model_delta).expect("serialize model delta");
        assert!(serialized.contains("oldContentBytes"));
        assert!(serialized.contains("newContentBytes"));
        assert!(!serialized.contains("old\\ncontent"));
        assert!(!serialized.contains("new\\ncontent"));
    }

    #[test]
    fn multi_edit_file_applies_multiple_edits_with_single_write() {
        let sandbox = std::env::temp_dir().join(format!("gateway-multi-edit-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let target = sandbox.join("update.txt");
        std::fs::write(&target, "alpha\nbeta\nbeta\ngamma\n").expect("write update");

        let args = MultiEditFileArgs {
            path: Some("update.txt".to_string()),
            cwd: None,
            skill_token: None,
            planning_id: None,
            files: Vec::new(),
            operations: Vec::new(),
            edits: vec![
                MultiEditFileEdit {
                    old_string: "alpha".to_string(),
                    new_string: "ALPHA".to_string(),
                    replace_all: false,
                    start_line: None,
                },
                MultiEditFileEdit {
                    old_string: "beta".to_string(),
                    new_string: "BETA".to_string(),
                    replace_all: false,
                    start_line: Some(3),
                },
            ],
        };

        let operations = normalize_multi_edit_operations(&args).expect("normalize operations");
        let outcome = apply_multi_edit_file(&sandbox, &operations).expect("apply multi edit");
        assert!(outcome
            .summary
            .modified
            .iter()
            .any(|path| Path::new(path).ends_with("update.txt")));
        assert_eq!(
            std::fs::read_to_string(&target).expect("read update"),
            "ALPHA\nbeta\nBETA\ngamma\n"
        );
        match outcome.delta.changes.as_slice() {
            [FileEditChange::Update {
                path,
                old_content,
                new_content,
                ..
            }] => {
                assert!(Path::new(path).ends_with("update.txt"));
                assert_eq!(old_content, "alpha\nbeta\nbeta\ngamma\n");
                assert_eq!(new_content, "ALPHA\nbeta\nBETA\ngamma\n");
            }
            other => panic!("unexpected delta: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn multi_edit_file_rejects_ambiguous_edit_without_writing() {
        let sandbox = std::env::temp_dir().join(format!("gateway-multi-edit-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let target = sandbox.join("update.txt");
        std::fs::write(&target, "alpha\nbeta\nbeta\n").expect("write update");

        let args = MultiEditFileArgs {
            path: Some("update.txt".to_string()),
            cwd: None,
            skill_token: None,
            planning_id: None,
            files: Vec::new(),
            operations: Vec::new(),
            edits: vec![MultiEditFileEdit {
                old_string: "beta".to_string(),
                new_string: "BETA".to_string(),
                replace_all: false,
                start_line: None,
            }],
        };

        let operations = normalize_multi_edit_operations(&args).expect("normalize operations");
        let failure =
            apply_multi_edit_file(&sandbox, &operations).expect_err("ambiguous edit should fail");
        assert!(failure.message.contains("found 2 matches"));
        assert_eq!(
            std::fs::read_to_string(&target).expect("read update"),
            "alpha\nbeta\nbeta\n"
        );

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn multi_edit_file_preserves_crlf_line_endings() {
        let sandbox = std::env::temp_dir().join(format!("gateway-multi-edit-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let target = sandbox.join("update.txt");
        std::fs::write(&target, "alpha\r\nbeta\r\n").expect("write update");

        let args = MultiEditFileArgs {
            path: Some("update.txt".to_string()),
            cwd: None,
            skill_token: None,
            planning_id: None,
            files: Vec::new(),
            operations: Vec::new(),
            edits: vec![MultiEditFileEdit {
                old_string: "alpha\nbeta".to_string(),
                new_string: "ALPHA\nBETA".to_string(),
                replace_all: false,
                start_line: None,
            }],
        };

        let operations = normalize_multi_edit_operations(&args).expect("normalize operations");
        apply_multi_edit_file(&sandbox, &operations).expect("apply multi edit");
        assert_eq!(
            std::fs::read_to_string(&target).expect("read update"),
            "ALPHA\r\nBETA\r\n"
        );

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn multi_edit_file_warns_when_delimiters_become_unbalanced() {
        let sandbox = std::env::temp_dir().join(format!("gateway-multi-edit-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let target = sandbox.join("lib.rs");
        std::fs::write(&target, "fn outer() {\n    value();\n}\n").expect("write update");

        let args = MultiEditFileArgs {
            path: Some("lib.rs".to_string()),
            cwd: None,
            skill_token: None,
            planning_id: None,
            files: Vec::new(),
            operations: Vec::new(),
            edits: vec![MultiEditFileEdit {
                old_string: "    value();\n}".to_string(),
                new_string: "    value();".to_string(),
                replace_all: false,
                start_line: None,
            }],
        };

        let operations = normalize_multi_edit_operations(&args).expect("normalize operations");
        let outcome = apply_multi_edit_file(&sandbox, &operations).expect("apply multi edit");
        assert!(outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("unbalanced delimiters")));

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn multi_edit_file_applies_multi_file_create_delete_and_move_operations() {
        let sandbox = std::env::temp_dir().join(format!("gateway-multi-ops-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        std::fs::write(sandbox.join("a.txt"), "alpha\n").expect("write a");
        std::fs::write(sandbox.join("delete.txt"), "remove\n").expect("write delete");
        std::fs::write(sandbox.join("old.txt"), "move me\n").expect("write move");

        let operations = vec![
            MultiEditFileOperation::Edit {
                path: "a.txt".to_string(),
                edits: vec![MultiEditFileEdit {
                    old_string: "alpha".to_string(),
                    new_string: "ALPHA".to_string(),
                    replace_all: false,
                    start_line: None,
                }],
            },
            MultiEditFileOperation::Create {
                path: "created.txt".to_string(),
                content: "created\n".to_string(),
                overwrite: false,
            },
            MultiEditFileOperation::Delete {
                path: "delete.txt".to_string(),
            },
            MultiEditFileOperation::Move {
                from: "old.txt".to_string(),
                to: "new.txt".to_string(),
                overwrite: false,
            },
        ];

        let outcome = apply_multi_edit_file(&sandbox, &operations).expect("apply operations");
        assert_eq!(
            std::fs::read_to_string(sandbox.join("a.txt")).expect("read a"),
            "ALPHA\n"
        );
        assert_eq!(
            std::fs::read_to_string(sandbox.join("created.txt")).expect("read created"),
            "created\n"
        );
        assert!(!sandbox.join("delete.txt").exists());
        assert!(!sandbox.join("old.txt").exists());
        assert_eq!(
            std::fs::read_to_string(sandbox.join("new.txt")).expect("read moved"),
            "move me\n"
        );
        assert_eq!(outcome.delta.changes.len(), 4);
        assert_eq!(outcome.summary.added.len(), 1);
        assert_eq!(outcome.summary.deleted.len(), 1);
        assert_eq!(outcome.summary.moved.len(), 1);

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn multi_edit_file_validates_all_operations_before_writing() {
        let sandbox = std::env::temp_dir().join(format!("gateway-multi-ops-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        std::fs::write(sandbox.join("a.txt"), "alpha\n").expect("write a");

        let operations = vec![
            MultiEditFileOperation::Create {
                path: "created.txt".to_string(),
                content: "created\n".to_string(),
                overwrite: false,
            },
            MultiEditFileOperation::Edit {
                path: "a.txt".to_string(),
                edits: vec![MultiEditFileEdit {
                    old_string: "missing".to_string(),
                    new_string: "MISSING".to_string(),
                    replace_all: false,
                    start_line: None,
                }],
            },
        ];

        let failure =
            apply_multi_edit_file(&sandbox, &operations).expect_err("second operation should fail");
        assert!(failure.message.contains("old_string not found"));
        assert!(!sandbox.join("created.txt").exists());
        assert_eq!(
            std::fs::read_to_string(sandbox.join("a.txt")).expect("read a"),
            "alpha\n"
        );

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn disabled_builtin_tool_not_in_tool_definitions() {
        let os = "Windows";
        let now = "2024-01-01T00:00:00Z";
        let cfg = BuiltinToolsConfig {
            read_file: true,
            shell_command: false,
            multi_edit_file: true,
            task_planning: true,
            chrome_cdp: true,
            chat_plus_adapter_debugger: true,
            office_cli: false,
            office_cli_path: None,
        };
        let tools = builtin_tool_definitions(os, now, &cfg);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect();
        assert!(!names.contains(&"shell_command"));
        assert!(names.contains(&"multi_edit_file"));
    }

    #[test]
    fn all_disabled_except_one_returns_single_tool() {
        let os = "Windows";
        let now = "2024-01-01T00:00:00Z";
        let cfg = BuiltinToolsConfig {
            read_file: false,
            shell_command: false,
            multi_edit_file: false,
            task_planning: false,
            chrome_cdp: true,
            chat_plus_adapter_debugger: false,
            office_cli: false,
            office_cli_path: None,
        };
        let tools = builtin_tool_definitions(os, now, &cfg);
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].get("name").and_then(Value::as_str),
            Some("chrome-cdp")
        );
    }

    #[test]
    fn disabled_tool_rejected_in_execute_builtin_tool() {
        let all_enabled = BuiltinToolsConfig::default();
        assert_eq!(builtin_tools(&all_enabled).len(), 7);

        let all_disabled = BuiltinToolsConfig {
            read_file: false,
            shell_command: false,
            multi_edit_file: false,
            task_planning: false,
            chrome_cdp: false,
            chat_plus_adapter_debugger: false,
            office_cli: false,
            office_cli_path: None,
        };
        assert_eq!(builtin_tools(&all_disabled).len(), 0);
    }

    #[tokio::test]
    async fn read_file_returns_line_window_every_time() {
        let service = SkillsService::new();
        let sandbox = std::env::temp_dir().join(format!("gateway-read-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let target = sandbox.join("sample.txt");
        std::fs::write(&target, "one\ntwo\nthree\nfour\n").expect("write sample");

        let mut config = GatewayConfig::default();
        config.skills.builtin_tools.task_planning = false;
        config.skills.policy.path_guard.enabled = true;
        config.skills.policy.path_guard.whitelist_dirs = vec![normalize_display_path(
            &normalize_root_path(sandbox.clone()),
        )];

        let token = builtin_skill_token(BuiltinTool::ReadFile);
        let args = json!({
            "path": "sample.txt",
            "cwd": normalize_display_path(&sandbox),
            "offset": 2,
            "limit": 2,
            "skillToken": token
        });

        let first = service
            .execute_builtin_tool(&config, BuiltinTool::ReadFile, args.clone(), "scope:read")
            .await
            .expect("first read");
        assert!(!first.is_error);
        assert_eq!(first.structured["startLine"], 2);
        assert_eq!(first.structured["endLine"], 3);
        assert_eq!(first.structured["numLines"], 2);
        assert!(first.text.contains("2\ttwo"));
        assert!(first.text.contains("3\tthree"));

        let second = service
            .execute_builtin_tool(&config, BuiltinTool::ReadFile, args, "scope:read")
            .await
            .expect("second read");
        assert!(!second.is_error);
        assert_eq!(second.structured["startLine"], 2);
        assert_eq!(second.structured["endLine"], 3);
        assert_eq!(second.structured["numLines"], 2);
        assert!(second.text.contains("2\ttwo"));
        assert!(second.text.contains("3\tthree"));
        assert!(second.structured["content"]
            .as_str()
            .is_some_and(|content| content.contains("2\ttwo")));

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[tokio::test]
    async fn read_file_rejects_binary_files() {
        let service = SkillsService::new();
        let sandbox = std::env::temp_dir().join(format!("gateway-read-bin-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        std::fs::write(sandbox.join("sample.bin"), b"abc\0def").expect("write binary");

        let mut config = GatewayConfig::default();
        config.skills.builtin_tools.task_planning = false;
        config.skills.policy.path_guard.enabled = true;
        config.skills.policy.path_guard.whitelist_dirs = vec![normalize_display_path(
            &normalize_root_path(sandbox.clone()),
        )];

        let result = service
            .execute_builtin_tool(
                &config,
                BuiltinTool::ReadFile,
                json!({
                    "path": "sample.bin",
                    "cwd": normalize_display_path(&sandbox),
                    "skillToken": builtin_skill_token(BuiltinTool::ReadFile)
                }),
                "scope:read-bin",
            )
            .await
            .expect("read result");
        assert!(result.is_error);
        assert!(result.text.contains("binary"));

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[tokio::test]
    async fn task_planning_update_returns_stable_content_derived_id() {
        let service = SkillsService::new();
        let config = GatewayConfig::default();
        let token = builtin_skill_token(BuiltinTool::TaskPlanning);
        let args = json!({
            "action": "update",
            "explanation": "test",
            "plan": [
                {"step": "Inspect", "status": "completed"},
                {"step": "Implement", "status": "in_progress"}
            ],
            "skillToken": token
        });

        let first = service
            .execute_builtin_tool(
                &config,
                BuiltinTool::TaskPlanning,
                args.clone(),
                "session:a",
            )
            .await
            .expect("first planning update");
        let second = service
            .execute_builtin_tool(&config, BuiltinTool::TaskPlanning, args, "session:a")
            .await
            .expect("second planning update");
        let planning_id = first.structured["planning"]["planningId"]
            .as_str()
            .expect("planning id")
            .to_string();
        let status_update = service
            .execute_builtin_tool(
                &config,
                BuiltinTool::TaskPlanning,
                json!({
                    "action": "set_status",
                    "planningId": planning_id.clone(),
                    "status": "completed",
                    "skillToken": token
                }),
                "session:a",
            )
            .await
            .expect("completed planning update");

        let first_plan = &first.structured["planning"];
        let second_plan = &second.structured["planning"];
        let completed_plan = &status_update.structured["planning"];
        assert_eq!(first_plan["planningId"], second_plan["planningId"]);
        assert_eq!(first_plan["planningId"], completed_plan["planningId"]);
        assert_eq!(completed_plan["active"], false);
        assert_eq!(completed_plan["completed"], true);
    }

    #[tokio::test]
    async fn planning_gate_allows_reusing_id_across_tool_calls_until_completed() {
        let service = SkillsService::new();
        let config = GatewayConfig::default();
        let scope = "session:gate";

        let missing = service
            .check_planning_gate(&config, scope, BuiltinTool::ShellCommand, None)
            .await
            .expect("missing planning should be blocked");
        assert!(missing.is_error);
        assert_eq!(missing.structured["reason"], "missing planningId");

        let token = builtin_skill_token(BuiltinTool::TaskPlanning);
        let updated = service
            .execute_builtin_tool(
                &config,
                BuiltinTool::TaskPlanning,
                json!({
                    "action": "update",
                    "plan": [
                        {"step": "Inspect", "status": "in_progress"},
                        {"step": "Verify", "status": "pending"}
                    ],
                    "skillToken": token
                }),
                scope,
            )
            .await
            .expect("planning update");
        let planning_id = updated.structured["planning"]["planningId"]
            .as_str()
            .expect("planning id")
            .to_string();

        assert!(service
            .check_planning_gate(
                &config,
                scope,
                BuiltinTool::ShellCommand,
                Some(&planning_id),
            )
            .await
            .is_none());

        assert!(service
            .check_planning_gate(
                &config,
                scope,
                BuiltinTool::ShellCommand,
                Some(&planning_id),
            )
            .await
            .is_none());

        assert!(service
            .check_planning_gate(
                &config,
                "session:changed",
                BuiltinTool::ShellCommand,
                Some(&planning_id),
            )
            .await
            .is_none());

        let planning_reminder = service
            .planning_success_hints(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::ShellCommand,
                Some("Get-ChildItem"),
            )
            .await
            .planning_reminder
            .expect("planning reminder");
        assert!(planning_reminder.contains("Current plan item #1"));
        assert!(planning_reminder.contains("\"action\":\"set_status\""));

        let reminder = tool_success_with_planning_reminder(
            "done".to_string(),
            json!({"status": "completed"}),
            PlanningSuccessHints {
                planning_reminder: Some(planning_reminder),
                shell_command_reminder: None,
            },
        );
        assert_eq!(reminder.text, "done");
        assert!(reminder.structured.get("planningUpdateRequired").is_none());
        assert!(reminder.structured["planningReminder"].is_string());
        assert!(reminder.structured.get("shellCommandReminder").is_none());

        let second_shell = service
            .planning_success_hints(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::ShellCommand,
                Some("Get-ChildItem"),
            )
            .await;
        assert!(second_shell.shell_command_reminder.is_none());
        let third_shell = service
            .planning_success_hints(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::ShellCommand,
                Some("Get-ChildItem"),
            )
            .await;
        assert!(third_shell.shell_command_reminder.is_some());
        let reset_by_rg = service
            .planning_success_hints(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::ShellCommand,
                Some("rg --files"),
            )
            .await;
        assert!(reset_by_rg.shell_command_reminder.is_none());
        let after_rg_shell = service
            .planning_success_hints(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::ShellCommand,
                Some("Get-ChildItem"),
            )
            .await;
        assert!(after_rg_shell.shell_command_reminder.is_none());
        let reset_by_edit = service
            .planning_success_hints(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::MultiEditFile,
                None,
            )
            .await;
        assert!(reset_by_edit.shell_command_reminder.is_none());

        assert!(service
            .planning_edit_failure_reminder(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::MultiEditFile,
            )
            .await
            .is_none());
        assert!(service
            .planning_edit_failure_reminder(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::MultiEditFile,
            )
            .await
            .is_none());
        let edit_failure_reminder = service
            .planning_edit_failure_reminder(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::MultiEditFile,
            )
            .await
            .expect("third edit failure reminder");
        assert!(edit_failure_reminder.contains("multi_edit_file has failed 3 times"));
        let edit_error = tool_error_with_edit_failure_reminder(
            "failed".to_string(),
            json!({"status": "failed"}),
            Some(edit_failure_reminder),
        );
        assert!(edit_error.structured["editFailureReminder"].is_string());

        let completed_first = service
            .execute_builtin_tool(
                &config,
                BuiltinTool::TaskPlanning,
                json!({
                    "action": "set_status",
                    "planningId": planning_id.clone(),
                    "status": "completed",
                    "skillToken": token
                }),
                scope,
            )
            .await
            .expect("complete current item");
        assert_eq!(
            completed_first.structured["planning"]["nextItem"]["item"],
            2
        );
        assert_eq!(
            completed_first.structured["planning"]["plan"][1]["status"],
            "in_progress"
        );
        assert!(service
            .check_planning_gate(
                &config,
                scope,
                BuiltinTool::ShellCommand,
                Some(&planning_id),
            )
            .await
            .is_none());

        service
            .execute_builtin_tool(
                &config,
                BuiltinTool::TaskPlanning,
                json!({
                    "action": "set_status",
                    "planningId": planning_id.clone(),
                    "status": "completed",
                    "skillToken": token
                }),
                scope,
            )
            .await
            .expect("complete final item");
        let closed = service
            .check_planning_gate(
                &config,
                scope,
                BuiltinTool::ShellCommand,
                Some(&planning_id),
            )
            .await
            .expect("completed planning id should be closed");
        assert_eq!(closed.structured["reason"], "unknown planningId");
    }
}

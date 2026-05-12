fn discover_skills_sync(roots: &[String]) -> Result<Vec<DiscoveredSkill>, AppError> {
    let mut discovered = Vec::new();
    let mut seen_skill_dirs = HashSet::new();

    for root in roots {
        let root_path = PathBuf::from(root);
        if !root_path.is_dir() {
            continue;
        }

        let mut stack = vec![root_path.clone()];
        let mut seen_dirs = HashSet::new();

        while let Some(current_dir) = stack.pop() {
            let canonical_dir =
                std::fs::canonicalize(&current_dir).unwrap_or_else(|_| current_dir.clone());
            if !seen_dirs.insert(canonical_dir.clone()) {
                continue;
            }

            register_skill_directory(
                &root_path,
                &canonical_dir,
                &mut seen_skill_dirs,
                &mut discovered,
            );

            let entries = match std::fs::read_dir(&canonical_dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };

            for entry in entries {
                let Ok(entry) = entry else {
                    continue;
                };
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_dir() || file_type.is_symlink() {
                    continue;
                }
                stack.push(entry.path());
            }
        }
    }

    discovered.sort_by_key(|entry| entry.skill.to_lowercase());
    Ok(discovered)
}

fn register_skill_directory(
    root_path: &Path,
    dir_path: &Path,
    seen_skill_dirs: &mut HashSet<PathBuf>,
    discovered: &mut Vec<DiscoveredSkill>,
) {
    let skill_md = dir_path.join("SKILL.md");
    if !skill_md.is_file() {
        return;
    }

    let canonical_skill_dir = std::fs::canonicalize(dir_path).unwrap_or_else(|_| dir_path.into());
    if !seen_skill_dirs.insert(canonical_skill_dir.clone()) {
        return;
    }

    let dir_name = canonical_skill_dir
        .file_name()
        .and_then(OsStr::to_str)
        .map(str::to_string)
        .unwrap_or_else(|| canonical_skill_dir.to_string_lossy().to_string());
    let parsed_frontmatter = parse_frontmatter_fields(&skill_md).unwrap_or_default();

    discovered.push(DiscoveredSkill {
        skill: dir_name.clone(),
        frontmatter_name: parsed_frontmatter.name,
        description: parsed_frontmatter.description,
        frontmatter_metadata: parsed_frontmatter.metadata,
        frontmatter_block: parsed_frontmatter.block,
        root: root_path.to_path_buf(),
        has_scripts: canonical_skill_dir.join("scripts").is_dir(),
        path: canonical_skill_dir,
    });
}

fn parse_frontmatter_fields(skill_md_path: &Path) -> Result<ParsedFrontmatter, AppError> {
    let content = std::fs::read_to_string(skill_md_path)?;
    parse_frontmatter_content(&content, &skill_md_path.display().to_string())
}

fn parse_frontmatter_content(content: &str, source: &str) -> Result<ParsedFrontmatter, AppError> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Ok(ParsedFrontmatter::default());
    }

    let mut frontmatter_lines = Vec::new();
    let mut has_closing = false;
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" || trimmed == "..." {
            has_closing = true;
            break;
        }
        frontmatter_lines.push(line.to_string());
    }
    if !has_closing {
        return Ok(ParsedFrontmatter::default());
    }

    let raw = frontmatter_lines.join("\n").trim().to_string();
    if raw.trim().is_empty() {
        return Ok(ParsedFrontmatter::default());
    }

    let frontmatter: Value = serde_yaml::from_str(&raw).map_err(|error| {
        AppError::BadRequest(format!("invalid YAML frontmatter in {source}: {error}"))
    })?;
    let frontmatter_obj = frontmatter.as_object().ok_or_else(|| {
        AppError::BadRequest(format!("frontmatter in {source} must be a YAML mapping"))
    })?;

    let name = frontmatter_obj
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let description = frontmatter_obj
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let metadata = frontmatter_obj
        .get("metadata")
        .map(frontmatter_value_summary)
        .unwrap_or_else(|| "none".to_string());
    Ok(ParsedFrontmatter {
        name,
        description,
        metadata,
        block: raw,
    })
}

fn frontmatter_value_summary(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::String(text) => text.to_string(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "unserializable".to_string()),
    }
}

fn shell_command_for_current_os(exec: &str) -> (String, Vec<String>) {
    if cfg!(target_os = "windows") {
        let runner = "powershell".to_string();
        let args = vec![
            "-NoProfile".to_string(),
            "-ExecutionPolicy".to_string(),
            "Bypass".to_string(),
            "-Command".to_string(),
            exec.to_string(),
        ];
        wrap_windows_powershell_command_for_utf8(&runner, &args).unwrap_or((runner, args))
    } else {
        ("sh".to_string(), vec!["-lc".to_string(), exec.to_string()])
    }
}


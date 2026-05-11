fn skill_display_name(skill: &DiscoveredSkill) -> &str {
    let frontmatter_name = skill.frontmatter_name.trim();
    if frontmatter_name.is_empty() {
        skill.skill.trim()
    } else {
        frontmatter_name
    }
}

fn sanitize_tool_name(raw: &str) -> String {
    let mut out = String::new();
    let mut last_separator = false;

    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_separator = false;
            continue;
        }
        if matches!(ch, '-' | '_') {
            out.push(ch);
            last_separator = false;
            continue;
        }
        if !last_separator {
            out.push('_');
            last_separator = true;
        }
    }

    let trimmed = out.trim_matches('_').trim_matches('-').to_string();
    if trimmed.is_empty() {
        "skill".to_string()
    } else {
        trimmed
    }
}

fn render_skill_tool_description(skill: &DiscoveredSkill, os: &str, now: &str) -> String {
    let meta_description = if skill.description.trim().is_empty() {
        format!("Skill instructions for {}", skill.skill)
    } else {
        skill.description.trim().to_string()
    };
    let frontmatter_block = if skill.frontmatter_block.trim().is_empty() {
        "none".to_string()
    } else {
        format!("---\n{}\n---", skill.frontmatter_block.trim())
    };
    let skill_path = normalize_display_path(&skill.path);
    format!(
        "MANDATORY BEFORE USE: this tool description is only a short discovery summary, not the operating instructions. Before using this skill for any real action, you MUST first call this skill tool with `exec` that reads the full SKILL.md from the skill path below. Do not infer safe usage from this description alone; skipping SKILL.md can cause incorrect or dangerous tool use. The only acceptable first call is a documentation-read call that reads the complete SKILL.md without `skillToken`, such as `cat {skill_path}/SKILL.md` or `Get-Content -Raw {skill_path}/SKILL.md`. The SKILL.md response includes the required `skillToken` only inside the returned markdown content. You must obtain it by reading the complete SKILL.md document; this SKILL.md read is the one call that does not need `skillToken`. Do not use regex, grep, Select-String, line ranges, or other partial-read tricks to fetch only the token. Every later non-documentation call to this skill MUST include that exact `skillToken` argument or the gateway will reject the call; a rejected call fails and must be retried with the correct token.\nThe `exec` value should be one shell command string used either to read markdown files or run scripts after SKILL.md has been read.\nCurrent OS: {os}.\nCurrent datetime: {now}.\nSkill path: {skill_path}.\nFront matter summary:\nname: {}\ndescription: {}\nmetadata: {}\nFront matter raw (YAML):\n{}",
        skill_display_name(skill),
        meta_description,
        if skill.frontmatter_metadata.trim().is_empty() {
            "none"
        } else {
            skill.frontmatter_metadata.trim()
        },
        frontmatter_block
    )
}

fn current_os_label() -> &'static str {
    if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        "Unknown"
    }
}

fn normalize_display_path(path: &Path) -> String {
    let raw = path.to_string_lossy().to_string();
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = raw.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    raw
}

/// Derive a key suitable for the per-path file lock table. On Windows the
/// file system is case-insensitive, so we normalize to lowercase to prevent
/// two spellings of the same path from bypassing each other's locks.
fn file_lock_key(path: &Path) -> String {
    let display = normalize_display_path(path);
    if cfg!(windows) {
        display.to_lowercase()
    } else {
        display
    }
}


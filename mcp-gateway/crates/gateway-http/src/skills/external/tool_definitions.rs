fn external_skill_tool_definitions(skills: &[DiscoveredSkill]) -> Value {
    let bindings = build_skill_tool_bindings(skills);
    let now = Utc::now().to_rfc3339();
    let os = current_os_label();
    let tools: Vec<Value> = bindings
        .into_iter()
        .map(|(tool_name, skill)| {
            let description = render_skill_tool_description(skill, os, &now);
            json!({
                "name": tool_name,
                "description": description,
                "inputSchema": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["exec"],
                    "properties": {
                        "exec": {
                            "type": "string",
                            "description": "Shell command string for this skill."
                        },
                        "skillToken": {
                            "type": "string",
                            "description": "Required skill token."
                        },

                    }
                }
            })
        })
        .collect();
    Value::Array(tools)
}


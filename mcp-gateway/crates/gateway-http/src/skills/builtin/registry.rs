fn builtin_tools(cfg: &BuiltinToolsConfig) -> Vec<BuiltinTool> {
    BuiltinTool::ALL
        .iter()
        .copied()
        .filter(|tool| tool.is_enabled(cfg))
        .collect()
}
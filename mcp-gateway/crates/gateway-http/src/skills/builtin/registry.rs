fn builtin_tools(cfg: &BuiltinToolsConfig) -> Vec<BuiltinTool> {
    let mut tools = Vec::with_capacity(7);
    if cfg.read_file {
        tools.push(BuiltinTool::ReadFile);
    }
    if cfg.shell_command {
        tools.push(BuiltinTool::ShellCommand);
    }
    if cfg.multi_edit_file {
        tools.push(BuiltinTool::MultiEditFile);
    }
    if cfg.task_planning {
        tools.push(BuiltinTool::TaskPlanning);
    }
    if cfg.chrome_cdp {
        tools.push(BuiltinTool::ChromeCdp);
    }
    if cfg.chat_plus_adapter_debugger {
        tools.push(BuiltinTool::ChatPlusAdapterDebugger);
    }
    if cfg.office_cli {
        tools.push(BuiltinTool::OfficeCli);
    }
    tools
}


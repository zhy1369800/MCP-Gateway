impl BuiltinTool {
    pub(crate) const ALL: &'static [BuiltinTool] = &[
        BuiltinTool::ReadFile,
        BuiltinTool::ShellCommand,
        BuiltinTool::MultiEditFile,
        BuiltinTool::TaskPlanning,
        BuiltinTool::ChromeCdp,
        BuiltinTool::ChatPlusAdapterDebugger,
        BuiltinTool::OfficeCli,
    ];

    pub(crate) fn is_enabled(self, cfg: &BuiltinToolsConfig) -> bool {
        match self {
            BuiltinTool::ReadFile => cfg.read_file,
            BuiltinTool::ShellCommand => cfg.shell_command,
            BuiltinTool::MultiEditFile => cfg.multi_edit_file,
            BuiltinTool::TaskPlanning => cfg.task_planning,
            BuiltinTool::ChromeCdp => cfg.chrome_cdp,
            BuiltinTool::ChatPlusAdapterDebugger => cfg.chat_plus_adapter_debugger,
            BuiltinTool::OfficeCli => cfg.office_cli,
        }
    }

    pub(crate) fn definition_builder(self) -> BuiltinToolDefinitionFn {
        match self {
            BuiltinTool::ReadFile => read_file_tool_definition,
            BuiltinTool::ShellCommand => shell_command_tool_definition,
            BuiltinTool::MultiEditFile => multi_edit_file_tool_definition,
            BuiltinTool::TaskPlanning => task_planning_tool_definition,
            BuiltinTool::ChromeCdp => chrome_cdp_tool_definition,
            BuiltinTool::ChatPlusAdapterDebugger => chat_plus_adapter_debugger_tool_definition,
            BuiltinTool::OfficeCli => office_cli_tool_definition,
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            value
                if value.eq_ignore_ascii_case(Self::ReadFile.name())
                    || value.eq_ignore_ascii_case("Read") =>
            {
                Some(Self::ReadFile)
            }
            value if value.eq_ignore_ascii_case(Self::ShellCommand.name()) => {
                Some(Self::ShellCommand)
            }
            value if value.eq_ignore_ascii_case(Self::MultiEditFile.name()) => {
                Some(Self::MultiEditFile)
            }
            value if value.eq_ignore_ascii_case(Self::TaskPlanning.name()) => {
                Some(Self::TaskPlanning)
            }
            value if value.eq_ignore_ascii_case(Self::ChromeCdp.name()) => Some(Self::ChromeCdp),
            value if value.eq_ignore_ascii_case(Self::ChatPlusAdapterDebugger.name()) => {
                Some(Self::ChatPlusAdapterDebugger)
            }
            value if value.eq_ignore_ascii_case(Self::OfficeCli.name()) => Some(Self::OfficeCli),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::ReadFile => "read_file",
            Self::ShellCommand => "shell_command",
            Self::MultiEditFile => "multi_edit_file",
            Self::TaskPlanning => "task-planning",
            Self::ChromeCdp => "chrome-cdp",
            Self::ChatPlusAdapterDebugger => "chat-plus-adapter-debugger",
            Self::OfficeCli => "officecli",
        }
    }
}
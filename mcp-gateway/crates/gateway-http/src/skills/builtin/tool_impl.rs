impl BuiltinTool {
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

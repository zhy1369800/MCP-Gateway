impl SkillsService {
    async fn execute_builtin_tool(
        &self,
        config: &GatewayConfig,
        tool: BuiltinTool,
        arguments: Value,
        planning_scope: &str,
    ) -> Result<ToolResult, AppError> {
        if !builtin_tools(&config.skills.builtin_tools).contains(&tool) {
            return Err(AppError::BadRequest(format!(
                "builtin tool {} is disabled by configuration",
                tool.name()
            )));
        }
        match tool {
            BuiltinTool::ReadFile => {
                let args = decode_tool_args::<ReadFileArgs>(&arguments)?;
                self.handle_builtin_read_file(config, args, planning_scope)
                    .await
            }
            BuiltinTool::ShellCommand => {
                let args = decode_tool_args::<BuiltinShellArgs>(&arguments)?;
                self.handle_builtin_shell_command(config, args, planning_scope)
                    .await
            }
            BuiltinTool::MultiEditFile => {
                let args = decode_tool_args::<MultiEditFileArgs>(&arguments)?;
                self.handle_builtin_multi_edit_file(config, args, planning_scope)
                    .await
            }
            BuiltinTool::TaskPlanning => {
                let args = decode_tool_args::<TaskPlanningArgs>(&arguments)?;
                self.handle_builtin_task_planning(args, planning_scope)
                    .await
            }
            BuiltinTool::ChromeCdp => {
                let args = decode_tool_args::<BuiltinShellArgs>(&arguments)?;
                self.handle_builtin_chrome_cdp(config, args, planning_scope)
                    .await
            }
            BuiltinTool::ChatPlusAdapterDebugger => {
                let args = decode_tool_args::<BuiltinShellArgs>(&arguments)?;
                self.handle_builtin_chat_plus_adapter_debugger(config, args, planning_scope)
                    .await
            }
            BuiltinTool::OfficeCli => {
                let args = decode_tool_args::<BuiltinShellArgs>(&arguments)?;
                self.handle_builtin_office_cli(config, args, planning_scope)
                    .await
            }
        }
    }
}

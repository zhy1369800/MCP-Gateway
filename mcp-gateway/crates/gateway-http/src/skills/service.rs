impl SkillsService {
    const CONFIRMATION_DECISION_TIMEOUT: Duration = Duration::from_secs(60);
    const CONFIRMATION_STALE_PENDING_WINDOW: Duration = Duration::from_secs(75);
    const CONFIRMATION_RESOLVED_RETENTION_WINDOW: Duration = Duration::from_secs(120);
    const SKILL_DISCOVERY_CACHE_TTL: Duration = Duration::from_secs(3);
    const MAX_TOOL_EVENTS: usize = 500;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_skills_server(&self, config: &GatewayConfig, server_name: &str) -> bool {
        if config.skills.server_name == server_name || config.skills.builtin_server_name == server_name {
            return true;
        }
        // Check if server_name matches any skill group name (format: __groupName__)
        config.skills.root_groups.iter().any(|g| {
            !g.name.is_empty() && format!("__{}__", g.name) == server_name
        })
    }

    pub async fn handle_mcp_request(
        &self,
        config: &GatewayConfig,
        request: Value,
        session_id: Option<&str>,
        server_name: &str,
    ) -> Value {
        let Some(object) = request.as_object() else {
            return jsonrpc_error(Value::Null, -32600, "invalid request payload", None);
        };

        let id = object.get("id").cloned().unwrap_or(Value::Null);
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            return jsonrpc_error(id, -32600, "missing jsonrpc method", None);
        };

        match method {
            "initialize" => jsonrpc_result(
                id,
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {
                            "listChanged": false
                        }
                    },
                    "serverInfo": {
                        "name": "mcp-gateway-skills",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            ),
            "ping" | "notifications/initialized" => jsonrpc_result(id, json!({"ok": true})),
            "tools/list" => {
                let is_builtin = server_name == config.skills.builtin_server_name;
                let (discovered, summaries) = if is_builtin {
                    (Vec::new(), Vec::new())
                } else {
                    let discovered = match self.discover_skills_for_server(config, server_name).await {
                        Ok(skills) => skills,
                        Err(error) => {
                            return jsonrpc_error(
                                id,
                                -32603,
                                "failed to discover skills",
                                Some(json!({"detail": error.to_string()})),
                            );
                        }
                    };
                    let summaries = summarize_discovered_skills(&discovered);
                    (discovered, summaries)
                };
                let tools = if is_builtin {
                    let mut defs = builtin_tool_definitions(
                        std::env::consts::OS,
                        &Utc::now().to_rfc3339(),
                        &config.skills.builtin_tools,
                    );
                    // If officecli is enabled in config but binary is not callable, hide it
                    if config.skills.builtin_tools.office_cli && !check_officecli_available(&config.skills.builtin_tools) {
                        defs.retain(|def| {
                            def.get("name").and_then(Value::as_str)
                                != Some(BuiltinTool::OfficeCli.name())
                        });
                    }
                    Value::Array(defs)
                } else {
                    external_skill_tool_definitions(&discovered)
                };
                jsonrpc_result(
                    id,
                    json!({
                        "tools": tools,
                        "skills": summaries
                    }),
                )
            }
            "tools/call" => {
                let is_builtin = server_name == config.skills.builtin_server_name;
                let params = object.get("params").cloned().unwrap_or(Value::Null);
                let tool_params: ToolCallParams = match serde_json::from_value(params) {
                    Ok(value) => value,
                    Err(error) => {
                        return jsonrpc_error(
                            id,
                            -32602,
                            "invalid tool call params",
                            Some(json!({"detail": error.to_string()})),
                        );
                    }
                };

                let planning_scope = planning_scope_key(session_id);
                let result = match self
                    .execute_tool_call(config, tool_params, &planning_scope, is_builtin, server_name)
                    .await
                {
                    Ok(output) => output,
                    Err(error) => error_to_tool_result(error),
                };
                jsonrpc_result(
                    id,
                    json!({
                        "isError": result.is_error,
                        "content": [
                            {
                                "type": "text",
                                "text": result.text
                            }
                        ],
                        "structuredContent": result.structured
                    }),
                )
            }
            _ => jsonrpc_error(id, -32601, "method not found", None),
        }
    }

    pub async fn list_pending_confirmations(&self) -> Vec<SkillConfirmation> {
        let now = Utc::now();
        let mut guard = self.confirmations.write().await;
        Self::prune_confirmations_locked(&mut guard, now);
        let mut list = guard
            .values()
            .filter(|entry| entry.record.status == ConfirmationStatus::Pending)
            .map(|entry| entry.record.clone())
            .collect::<Vec<_>>();
        list.sort_by_key(|entry| std::cmp::Reverse(entry.created_at));
        list
    }

    pub async fn approve_confirmation(&self, id: &str) -> Result<SkillConfirmation, AppError> {
        let now = Utc::now();
        let mut guard = self.confirmations.write().await;
        Self::prune_confirmations_locked(&mut guard, now);
        let Some(entry) = guard.get_mut(id) else {
            return Err(AppError::NotFound("confirmation not found".to_string()));
        };
        let notify = entry.notify.clone();
        match entry.record.status {
            ConfirmationStatus::Pending => {}
            ConfirmationStatus::Approved => {
                return Err(AppError::Conflict(
                    "confirmation already approved".to_string(),
                ));
            }
            ConfirmationStatus::Rejected => {
                return Err(AppError::Conflict(
                    "confirmation already rejected".to_string(),
                ));
            }
        }
        entry.record.status = ConfirmationStatus::Approved;
        entry.record.updated_at = now;
        entry.timed_out = false;
        let updated = entry.record.clone();
        notify.notify_one();
        Ok(updated)
    }

    pub async fn reject_confirmation(&self, id: &str) -> Result<SkillConfirmation, AppError> {
        let now = Utc::now();
        let mut guard = self.confirmations.write().await;
        Self::prune_confirmations_locked(&mut guard, now);
        let Some(target) = guard.get(id).map(|entry| entry.record.clone()) else {
            return Err(AppError::NotFound("confirmation not found".to_string()));
        };
        match target.status {
            ConfirmationStatus::Pending => {}
            ConfirmationStatus::Approved => {
                return Err(AppError::Conflict(
                    "confirmation already approved".to_string(),
                ));
            }
            ConfirmationStatus::Rejected => {
                return Err(AppError::Conflict(
                    "confirmation already rejected".to_string(),
                ));
            }
        }
        let mut notifies = Vec::new();
        for entry in guard.values_mut() {
            if entry.record.status != ConfirmationStatus::Pending {
                continue;
            }
            if Self::is_same_confirmation_signature(&entry.record, &target) {
                entry.record.status = ConfirmationStatus::Rejected;
                entry.record.updated_at = now;
                entry.timed_out = false;
                notifies.push(entry.notify.clone());
            }
        }
        for notify in notifies {
            notify.notify_one();
        }

        guard
            .get(id)
            .map(|entry| entry.record.clone())
            .ok_or_else(|| AppError::NotFound("confirmation not found".to_string()))
    }

    pub async fn list_skills_for_admin(
        &self,
        config: &GatewayConfig,
    ) -> Result<Vec<SkillSummary>, AppError> {
        let discovered = self.discover_skills(&config.skills).await?;
        let mut summaries = summarize_builtin_skills(&config.skills.builtin_tools);
        summaries.extend(summarize_discovered_skills(&discovered));
        Ok(summaries)
    }

    pub async fn list_tool_events(&self, after: Option<u64>) -> Vec<SkillToolEvent> {
        let after = after.unwrap_or(0);
        let guard = self.events.read().await;
        guard
            .events
            .iter()
            .filter(|event| event.seq > after)
            .cloned()
            .collect()
    }

    async fn record_tool_event(&self, mut event: SkillToolEvent) {
        let mut guard = self.events.write().await;
        guard.next_seq = guard.next_seq.saturating_add(1);
        event.seq = guard.next_seq;
        guard.events.push_back(event);
        while guard.events.len() > Self::MAX_TOOL_EVENTS {
            guard.events.pop_front();
        }
    }

    async fn record_tool_event_data(
        &self,
        call_id: &str,
        tool: &str,
        kind: &str,
        data: SkillToolEventData,
    ) {
        self.record_tool_event(SkillToolEvent {
            seq: 0,
            timestamp: Utc::now(),
            call_id: call_id.to_string(),
            tool: tool.to_string(),
            kind: kind.to_string(),
            cwd: data.cwd,
            preview: data.preview,
            text: data.text,
            status: data.status,
            exit_code: data.exit_code,
            duration_ms: data.duration_ms,
            affected_paths: data.affected_paths,
            changes: data.changes,
            delta: data.delta,
            warnings: data.warnings,
        })
        .await;
    }

    /// Acquire an async lock for each path in `paths`, returning one owned
    /// guard per unique path. Locks are acquired in a deterministic key order
    /// (lowercase on Windows to avoid case-insensitive aliasing) so concurrent
    /// callers can't deadlock against each other.
    async fn acquire_file_locks(
        &self,
        paths: &[PathBuf],
    ) -> Vec<tokio::sync::OwnedMutexGuard<()>> {
        let mut keys: Vec<String> = paths
            .iter()
            .map(|p| file_lock_key(p))
            .collect();
        keys.sort();
        keys.dedup();

        let mutexes: Vec<Arc<tokio::sync::Mutex<()>>> = {
            let mut table = self.file_locks.lock().expect("file_locks mutex poisoned");
            keys.iter()
                .map(|key| {
                    table
                        .entry(key.clone())
                        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                        .clone()
                })
                .collect()
        };

        let mut guards = Vec::with_capacity(mutexes.len());
        for mutex in mutexes {
            guards.push(mutex.lock_owned().await);
        }
        guards
    }

    async fn execute_tool_call(
        &self,
        config: &GatewayConfig,
        params: ToolCallParams,
        planning_scope: &str,
        is_builtin_endpoint: bool,
        server_name: &str,
    ) -> Result<ToolResult, AppError> {
        if let Some(tool) = BuiltinTool::from_name(&params.name) {
            if is_builtin_endpoint {
                return self
                    .execute_builtin_tool(config, tool, params.arguments, planning_scope)
                    .await;
            }
            return Err(AppError::BadRequest(format!(
                "built-in tool {} is only available on the built-in skills endpoint",
                params.name
            )));
        }

        if is_builtin_endpoint {
            return Err(AppError::BadRequest(format!(
                "unknown tool name: {}",
                params.name
            )));
        }

        let skills = self.discover_skills_for_server(config, server_name).await?;
        let bindings = build_skill_tool_bindings(&skills);
        let Some((tool_name, skill)) = bindings
            .iter()
            .find(|(tool_name, _)| tool_name.eq_ignore_ascii_case(params.name.as_str()))
            .map(|(tool_name, skill)| (tool_name.clone(), (*skill).clone()))
        else {
            return Err(AppError::BadRequest(format!(
                "unknown tool name: {}",
                params.name
            )));
        };

        let args = decode_tool_args::<SkillCommandArgs>(&params.arguments)?;
        self.handle_skill_command(config, &tool_name, &skill, args)
            .await
    }

}

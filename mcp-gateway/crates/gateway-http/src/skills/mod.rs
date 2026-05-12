use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use gateway_core::{
    assign_child_to_gateway_job, wrap_windows_powershell_command_for_utf8, AppError,
    BuiltinToolsConfig, ErrorCode, GatewayConfig, SkillCommandRule, SkillPolicyAction,
    SkillsConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::{Notify, RwLock};
use utoipa::ToSchema;
use uuid::Uuid;

include!("types.rs");
include!("service.rs");
include!("planning.rs");
include!("builtin/handlers.rs");
include!("builtin/read_file.rs");
include!("builtin/shell_command.rs");
include!("builtin/multi_edit_file.rs");
include!("builtin/task_planning.rs");
include!("builtin/chrome_cdp.rs");
include!("builtin/chat_plus_adapter_debugger.rs");
include!("builtin/office_cli.rs");
include!("external/handler.rs");
include!("confirmations.rs");
include!("results.rs");
include!("builtin/registry.rs");
include!("external/tool_definitions.rs");
include!("builtin/definitions.rs");
include!("external/bindings.rs");
include!("builtin/tool_impl.rs");
include!("external/description.rs");
include!("jsonrpc.rs");
include!("external/discovery.rs");
include!("command_policy.rs");
include!("file_parsing.rs");

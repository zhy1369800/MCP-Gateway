import { invoke } from "@tauri-apps/api/core";
import type {
  GatewayConfig,
  ServerConfig,
  ServerAuthState,
  ServerConnectivityTestResult,
  SkillCommandRule,
  SkillDirectoryValidation,
} from "./types";

export async function loadLocalConfig(): Promise<GatewayConfig> {
  return invoke<GatewayConfig>("load_local_config");
}

export async function saveLocalConfig(config: GatewayConfig): Promise<void> {
  await invoke("save_local_config", { config });
}

export async function getConfigPath(): Promise<string> {
  return invoke<string>("get_config_path");
}

export async function getDefaultSkillRules(): Promise<SkillCommandRule[]> {
  return invoke<SkillCommandRule[]>("get_default_skill_rules");
}

export async function pickFolderDialog(startDir?: string): Promise<string | null> {
  return invoke<string | null>("pick_folder_dialog", { startDir: startDir ?? null });
}

export async function validateSkillDirectory(path: string): Promise<SkillDirectoryValidation> {
  return invoke<SkillDirectoryValidation>("validate_skill_directory", { path });
}

export async function focusMainWindowForSkillConfirmation(): Promise<void> {
  await invoke("focus_main_window_for_skill_confirmation");
}

export async function testMcpServerLocal(server: ServerConfig): Promise<ServerConnectivityTestResult> {
  return invoke<ServerConnectivityTestResult>("test_mcp_server_local", { server });
}

export async function getServerAuthStateLocal(server: ServerConfig): Promise<ServerAuthState> {
  return invoke<ServerAuthState>("get_server_auth_state_local", { server });
}

export async function clearServerAuthLocal(server: ServerConfig): Promise<ServerAuthState> {
  return invoke<ServerAuthState>("clear_server_auth_local", { server });
}

export async function reauthorizeServerLocal(server: ServerConfig): Promise<ServerConnectivityTestResult> {
  return invoke<ServerConnectivityTestResult>("reauthorize_server_local", { server });
}


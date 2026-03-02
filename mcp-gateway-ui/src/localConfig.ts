import { invoke } from "@tauri-apps/api/core";
import type { GatewayConfig, SkillDirectoryValidation } from "./types";

export async function loadLocalConfig(): Promise<GatewayConfig> {
  return invoke<GatewayConfig>("load_local_config");
}

export async function saveLocalConfig(config: GatewayConfig): Promise<void> {
  await invoke("save_local_config", { config });
}

export async function getConfigPath(): Promise<string> {
  return invoke<string>("get_config_path");
}

export async function pickFolderDialog(startDir?: string): Promise<string | null> {
  return invoke<string | null>("pick_folder_dialog", { startDir: startDir ?? null });
}

export async function validateSkillDirectory(path: string): Promise<SkillDirectoryValidation> {
  return invoke<SkillDirectoryValidation>("validate_skill_directory", { path });
}


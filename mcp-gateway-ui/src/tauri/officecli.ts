import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface OfficeCliCheckResult {
  installed: boolean;
  version?: string | null;
  /** 真实解析到的二进制绝对路径，用于回写 config */
  path?: string | null;
  error?: string | null;
}

export interface OfficeCliInstallResult {
  ok: boolean;
  installedPath?: string | null;
  version?: string | null;
  error?: string | null;
  /** 兜底：GitHub releases 页面 URL */
  releaseUrl: string;
  assetName: string;
}

export interface OfficeCliProgress {
  downloaded: number;
  total: number;
  percent: number;
}

/** 检测 officecli：可选传已保存路径/目录。稳定性核心。 */
export function checkOfficeCli(path?: string): Promise<OfficeCliCheckResult> {
  return invoke<OfficeCliCheckResult>("officecli_check", { path });
}

/** 触发真实下载安装，进度通过 listenOfficeCliProgress 订阅 */
export function installOfficeCli(): Promise<OfficeCliInstallResult> {
  return invoke<OfficeCliInstallResult>("officecli_install");
}

/** 删除我们自己管理的安装目录（不动老脚本目录，不改 PATH） */
export function uninstallOfficeCli(): Promise<void> {
  return invoke("officecli_uninstall");
}

/** 用默认浏览器打开 GitHub releases 页面（下载失败兜底） */
export function openOfficeCliReleases(): Promise<void> {
  return invoke("officecli_open_releases");
}

/** 订阅下载进度。返回取消订阅函数。 */
export async function listenOfficeCliProgress(
  handler: (p: OfficeCliProgress) => void
): Promise<UnlistenFn> {
  return listen<OfficeCliProgress>("officecli://progress", (event) => {
    handler(event.payload);
  });
}


/** Returns the default managed install path for officecli binary. */
export function getOfficeCliDefaultPath(): Promise<string> {
  return invoke<string>("officecli_default_path");
}
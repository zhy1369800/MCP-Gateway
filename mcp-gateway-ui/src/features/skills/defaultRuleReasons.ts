import type { Lang } from "../../i18n";

type RuleReason = {
  en: string;
  zh: string;
};

export type ParsedSkillReason = {
  text: string;
  raw: string;
  baseReason: string;
  ruleId: string | null;
  source: string | null;
  reasonKey: string | null;
  isLocalizedDefault: boolean;
};

const reasonByKey: Record<string, RuleReason> = {
  root_destructive_deletion: {
    en: "This command may recursively delete the root directory and is blocked by default.",
    zh: "该命令可能递归删除根目录，已默认拦截。",
  },
  drive_root_recursive_deletion: {
    en: "This command may recursively delete a drive root and is blocked by default.",
    zh: "该命令可能递归删除磁盘根目录，已默认拦截。",
  },
  privilege_escalation: {
    en: "This command may gain higher system privileges and is blocked by default.",
    zh: "该命令可能获取更高系统权限，已默认拦截。",
  },
  user_switching: {
    en: "This command switches the system user and is blocked by default.",
    zh: "该命令会切换系统用户，已默认拦截。",
  },
  permission_modification: {
    en: "This command changes file permissions and is blocked by default.",
    zh: "该命令会修改文件权限，已默认拦截。",
  },
  ownership_modification: {
    en: "This command changes file ownership and is blocked by default.",
    zh: "该命令会修改文件所有者，已默认拦截。",
  },
  group_ownership_modification: {
    en: "This command changes file group ownership and is blocked by default.",
    zh: "该命令会修改文件所属用户组，已默认拦截。",
  },
  windows_ownership_takeover: {
    en: "This command takes Windows file ownership and is blocked by default.",
    zh: "该命令会接管 Windows 文件所有权，已默认拦截。",
  },
  windows_acl_modification: {
    en: "This command changes Windows access control permissions and is blocked by default.",
    zh: "该命令会修改 Windows 访问控制权限，已默认拦截。",
  },
  registry_modification: {
    en: "This command changes the Windows registry and is blocked by default.",
    zh: "该命令会修改 Windows 注册表，已默认拦截。",
  },
  boot_configuration: {
    en: "This command changes system boot configuration and is blocked by default.",
    zh: "该命令会修改系统启动配置，已默认拦截。",
  },
  network_configuration: {
    en: "This command views or changes network configuration and needs your confirmation.",
    zh: "该命令会查看或修改网络配置，执行前需要你确认。",
  },
  routing_configuration: {
    en: "This command views or changes network routes and needs your confirmation.",
    zh: "该命令会查看或修改网络路由，执行前需要你确认。",
  },
  process_termination: {
    en: "This command terminates running processes and needs your confirmation.",
    zh: "该命令会结束正在运行的进程，执行前需要你确认。",
  },
  disk_partition: {
    en: "This command operates on disk partitions and is blocked by default.",
    zh: "该命令会操作磁盘分区，已默认拦截。",
  },
  disk_formatting: {
    en: "This command formats disks and is blocked by default.",
    zh: "该命令会格式化磁盘，已默认拦截。",
  },
  download_or_certificate: {
    en: "This command may download files or manipulate certificates and is blocked by default.",
    zh: "该命令可能下载文件或操作证书，已默认拦截。",
  },
  background_transfer: {
    en: "This command may transfer files in the background and is blocked by default.",
    zh: "该命令可能在后台传输文件，已默认拦截。",
  },
  installer_execution: {
    en: "This command runs an installer and needs your confirmation.",
    zh: "该命令会运行安装程序，执行前需要你确认。",
  },
  binary_registration: {
    en: "This command registers system components and is blocked by default.",
    zh: "该命令会注册系统组件，已默认拦截。",
  },
  dynamic_library_execution: {
    en: "This command runs dynamic library code directly and is blocked by default.",
    zh: "该命令会直接运行动态库代码，已默认拦截。",
  },
  task_scheduler: {
    en: "This command changes scheduled tasks and is blocked by default.",
    zh: "该命令会修改计划任务，已默认拦截。",
  },
  service_controller: {
    en: "This command manages system services and needs your confirmation.",
    zh: "该命令会管理系统服务，执行前需要你确认。",
  },
  package_manager: {
    en: "This command installs, upgrades, or removes software packages and needs your confirmation.",
    zh: "该命令会安装、升级或移除软件包，执行前需要你确认。",
  },
  firewall: {
    en: "This command changes firewall rules and is blocked by default.",
    zh: "该命令会修改防火墙规则，已默认拦截。",
  },
  raw_disk_write: {
    en: "This command may write directly to a disk device and is blocked by default.",
    zh: "该命令可能直接写入磁盘设备，已默认拦截。",
  },
  filesystem_creation: {
    en: "This command creates a filesystem and is blocked by default.",
    zh: "该命令会创建文件系统，已默认拦截。",
  },
  mount: {
    en: "This command mounts a disk or directory and needs your confirmation.",
    zh: "该命令会挂载磁盘或目录，执行前需要你确认。",
  },
  unmount: {
    en: "This command unmounts a disk or directory and needs your confirmation.",
    zh: "该命令会卸载磁盘或目录，执行前需要你确认。",
  },
  file_attribute_modification: {
    en: "This command changes file attributes and is blocked by default.",
    zh: "该命令会修改文件属性，已默认拦截。",
  },
  acl_modification: {
    en: "This command changes access control lists and is blocked by default.",
    zh: "该命令会修改访问控制列表，已默认拦截。",
  },
  user_management: {
    en: "This command manages system users and is blocked by default.",
    zh: "该命令会管理系统用户，已默认拦截。",
  },
  group_management: {
    en: "This command manages system groups and is blocked by default.",
    zh: "该命令会管理系统用户组，已默认拦截。",
  },
  macos_service_controller: {
    en: "This command manages macOS services and needs your confirmation.",
    zh: "该命令会管理 macOS 服务，执行前需要你确认。",
  },
  macos_preferences: {
    en: "This command changes macOS preferences and needs your confirmation.",
    zh: "该命令会修改 macOS 偏好设置，执行前需要你确认。",
  },
  macos_security_policy: {
    en: "This command changes macOS security policy and is blocked by default.",
    zh: "该命令会修改 macOS 安全策略，已默认拦截。",
  },
  macos_sip: {
    en: "This command changes macOS System Integrity Protection and is blocked by default.",
    zh: "该命令会修改 macOS 系统完整性保护，已默认拦截。",
  },
  macos_keychain: {
    en: "This command accesses or changes the macOS keychain and is blocked by default.",
    zh: "该命令会访问或修改 macOS 钥匙串，已默认拦截。",
  },
  applescript_execution: {
    en: "This command runs AppleScript and needs your confirmation.",
    zh: "该命令会执行 AppleScript，执行前需要你确认。",
  },
  disk_utility: {
    en: "This command operates on macOS Disk Utility and is blocked by default.",
    zh: "该命令会操作 macOS 磁盘工具，已默认拦截。",
  },
  nested_shell: {
    en: "This command launches another command-line environment and is blocked by default.",
    zh: "该命令会启动新的命令行环境，已默认拦截。",
  },
  shell_wrapper: {
    en: "This command runs dynamic commands through a shell wrapper and is blocked by default.",
    zh: "该命令会通过 shell 包装器执行动态命令，已默认拦截。",
  },
  dynamic_command_execution: {
    en: "This command dynamically executes composed commands and is blocked by default.",
    zh: "该命令会动态执行拼接的命令，已默认拦截。",
  },
  remote_command_execution: {
    en: "This command may execute commands on a remote machine and is blocked by default.",
    zh: "该命令可能在远程机器上执行命令，已默认拦截。",
  },
  network_download: {
    en: "This command accesses the network or downloads content and needs your confirmation.",
    zh: "该命令会访问外部网络或下载内容，执行前需要你确认。",
  },
  text_editing: {
    en: "This command modifies file content and needs your confirmation.",
    zh: "该命令会修改文件内容，执行前需要你确认。",
  },
  file_deletion: {
    en: "This command deletes files and needs your confirmation.",
    zh: "该命令会删除文件，执行前需要你确认。",
  },
  directory_deletion: {
    en: "This command deletes directories and needs your confirmation.",
    zh: "该命令会删除目录，执行前需要你确认。",
  },
  powershell_deletion: {
    en: "This command deletes content through PowerShell and needs your confirmation.",
    zh: "该命令会通过 PowerShell 删除内容，执行前需要你确认。",
  },
  file_unlink: {
    en: "This command removes a file link or deletes a file and needs your confirmation.",
    zh: "该命令会移除文件链接或删除文件，执行前需要你确认。",
  },
  file_move: {
    en: "This command moves files or directories and needs your confirmation.",
    zh: "该命令会移动文件或目录，执行前需要你确认。",
  },
  file_copy: {
    en: "This command copies files or directories and needs your confirmation.",
    zh: "该命令会复制文件或目录，执行前需要你确认。",
  },
  file_rename: {
    en: "This command renames files or directories and needs your confirmation.",
    zh: "该命令会重命名文件或目录，执行前需要你确认。",
  },
  file_or_directory_creation: {
    en: "This command creates files or directories and needs your confirmation.",
    zh: "该命令会创建文件或目录，执行前需要你确认。",
  },
  directory_creation: {
    en: "This command creates directories and needs your confirmation.",
    zh: "该命令会创建目录，执行前需要你确认。",
  },
  file_touch: {
    en: "This command creates files or updates timestamps and needs your confirmation.",
    zh: "该命令会创建文件或更新时间戳，执行前需要你确认。",
  },
  path_outside_allowed_dir: {
    en: "This operation tries to access a path outside the allowed directories. Approve only if you trust this one-time access.",
    zh: "该操作会访问允许目录外的路径。只有确认本次访问可信时，才允许本次执行。",
  },
  default_policy: {
    en: "This command matched the default policy.",
    zh: "该命令未命中特定规则，按默认策略处理。",
  },
  legacy_confirm_keyword: {
    en: "This command matched a legacy confirmation keyword.",
    zh: "该命令命中了旧版待确认关键词。",
  },
  legacy_deny_keyword: {
    en: "This command matched a legacy deny keyword.",
    zh: "该命令命中了旧版拒绝关键词。",
  },
};

const fallbackReasonByEn: Record<string, RuleReason> = {
  "Network download command is blocked": {
    en: "This command accesses the network or downloads content and is blocked by default.",
    zh: "该命令会访问外部网络或下载内容，已默认拦截。",
  },
  "matched command rule": {
    en: "This command matched a policy rule.",
    zh: "该命令命中了策略规则。",
  },
  "matched default policy": reasonByKey.default_policy,
};

function localizePathGuardReason(reason: string, lang: Lang): string | null {
  const resolvedMatch = reason.match(/^path '(.+)' resolved to '(.+)' is outside whitelist$/);
  if (resolvedMatch) {
    if (lang === "en") {
      return `This operation tries to access a path outside the allowed directories: ${resolvedMatch[2]}`;
    }
    return `该操作会访问允许目录外的路径：${resolvedMatch[2]}`;
  }

  const directMatch = reason.match(/^path '(.+)' is outside allowed directories$/);
  if (!directMatch) return null;
  if (lang === "en") return reason;
  return `该操作会访问允许目录外的路径：${directMatch[1]}`;
}

function getReasonText(reasonKey: string | null | undefined, baseReason: string, lang: Lang) {
  const pathGuardReason = localizePathGuardReason(baseReason, lang);
  if (pathGuardReason) return { text: pathGuardReason, localized: lang === "zh" };

  const byKey = reasonKey ? reasonByKey[reasonKey] : undefined;
  if (byKey) return { text: byKey[lang], localized: true };

  const byReason = fallbackReasonByEn[baseReason];
  if (byReason) return { text: byReason[lang], localized: true };

  return { text: baseReason, localized: false };
}

export function parseSkillReason(rawReason: string, lang: Lang, reasonKey?: string | null): ParsedSkillReason {
  const raw = rawReason.trim();
  const ruleMatch = raw.match(/^(.*)\s+\(rule:\s*([^,]+),\s*source:\s*([^)]+)\)$/);
  const sourceMatch = ruleMatch ? null : raw.match(/^(.*)\s+\(source:\s*([^)]+)\)$/);
  const baseReason = (ruleMatch?.[1] ?? sourceMatch?.[1] ?? raw).trim();
  const ruleId = ruleMatch?.[2]?.trim() ?? null;
  const source = ruleMatch?.[3]?.trim() ?? sourceMatch?.[2]?.trim() ?? null;
  const normalizedKey = reasonKey?.trim() || null;
  const localized = getReasonText(normalizedKey, baseReason, lang);

  return {
    text: localized.text || raw,
    raw,
    baseReason,
    ruleId,
    source,
    reasonKey: normalizedKey,
    isLocalizedDefault: localized.localized,
  };
}

export function localizeSkillRuleReason(reason: string, lang: Lang, reasonKey?: string | null): string {
  return getReasonText(reasonKey, reason.trim(), lang).text || reason;
}

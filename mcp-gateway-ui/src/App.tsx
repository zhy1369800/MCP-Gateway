import { useState, useEffect, useCallback, useMemo, useRef, type PointerEvent as ReactPointerEvent } from "react";
import {
  Play,
  Square,
  Copy,
  Check,
  Code2,
  BookOpenText,
  AlignLeft,
  FileText,
  Terminal,
  FilePenLine,
  ListChecks,
  Chrome,
  Bug,
  Eye,
  List,
  Languages,
  Save,
  Newspaper,
  Github,
  Send,
  Info,
  Settings as SettingsIcon,
  Wrench,
  Plug,
  Pencil,
  RotateCcw,
  Download,
  FolderOpen,
  Plus,
  Trash2,
} from "lucide-react";
import { open } from "@tauri-apps/plugin-shell";
import { getGatewayStatus, startGateway, stopGateway, type GatewayProcessStatus } from "./gatewayRuntime";
import {
  clearServerAuthLocal,
  detectLocalRuntimes,
  setMainWindowTitle,
  getServerAuthStateLocal,
  loadLocalConfig,
  openConfigFileLocal,
  resetDefaultConfigLocal,
  saveLocalConfig,
  getConfigPath,
  getDefaultSkillRules,
  reauthorizeServerLocal,
  scanSkillDirectories,
  testMcpServerLocal,
  pickFolderDialog,
  validateSkillDirectory,
  focusMainWindowForSkillConfirmation,
} from "./localConfig";
import { ApiClient } from "./api";
import {
  checkOfficeCli,
  installOfficeCli,
  listenOfficeCliProgress,
  getOfficeCliDefaultPath,
  type OfficeCliInstallResult,
} from "./tauri/officecli";
import { usePolling, type PollOutcome } from "./hooks/usePolling";
import type {
  GatewayConfig,
  LocalRuntimeSummary,
  ServerConfig,
  ServerAuthState,
  SkillCommandRule,
  SkillPolicyAction,
  ActivePlan,
  SkillConfirmation,
  ServerConnectivityTestResult,
  SkillsConfig,
  SkillGroupEntry,
  BuiltinToolsConfig,
} from "./types";
import { useT, type Lang, type TKey } from "./i18n";
import JsonEditor from "./components/JsonEditor";
import { useUpdateCheck } from "./hooks/useUpdateCheck";
import { UpdateBanner } from "./components/UpdateBanner";
import { formatTime } from "./utils/display";
import { ConfirmDialog } from "./components/ConfirmDialog";
import { SkillConfirmations, SkillConfirmationPopup } from "./components/SkillConfirmations";
import { SkillDirectoryListEditor } from "./components/SkillDirectoryListEditor";
import { SkillGroupsEditor } from "./components/SkillGroupsEditor";
import { SkillPolicyRulesEditor } from "./components/SkillPolicyRulesEditor";
import { ServerRow } from "./components/ServerRow";
import { jsonToServers } from "./utils/serverConfig";
import {
  buildServersJson,
  createEditableConfigFingerprint,
  createEditableConfigSnapshot,
  createMcpClientEntryJson,
  moveArrayItem,
  stripIndexKeyedEntries,
  type EndpointTransportType,
  type ServerDragTarget,
} from "./utils/configSnapshot";
import { runtimeDisplayValue, terminalEncodingDisplayValue } from "./utils/display";
import {
  createEmptySkillRuleForm,
  createSkillRuleId,
  BUILTIN_SKILL_SERVER_NAME,
  ensureSkillsConfig,
  formToRule,
  isSkillRuleFormValid,
  normalizeSkillsForSubmit,
  parseRulesJson,
  ruleToForm,
  type SkillRuleFormState,
} from "./features/skills/skillRules";
import { isConfirmationAlreadyResolvedError } from "./features/skills/confirmations";
import {
  createSkillDirectoryItem,
  createSkillGroup,
  skillDirectoryStatusFromResult,
  type SkillDirKind,
  type SkillDirectoryItem,
  type SkillGroup,
} from "./features/skills/directories";
import {
  asErrorMessage,
  authStateTone,
  createEmptyAuthState,
  serverTestKey,
  type ServerTestState,
  type ServerTestStatus,
} from "./features/servers/serverStatus";

const SERVER_LIST_GAP_PX = 16;

// 版本号由 Vite 在编译时注入（CI 时来自 git tag，本地开发时来自 package.json）
const CURRENT_VERSION = import.meta.env.VITE_APP_VERSION as string;
const BLOG_URL = "https://blog.aiguicai.com";
const BILIBILI_URL = "https://space.bilibili.com/228928896?spm_id_from=333.1007.0.0";
const GITHUB_URL = "https://github.com/510myRday/MCP-Gateway";
const TG_GROUP_URL = "https://t.me/+vq8WByYtPoQ1MjA1";
const QQ_GROUP_NUMBER = "1090461840";
// 最稳方式：填入群分享得到的官方邀请链接（含 k / idkey 等参数）
// 例如：https://qm.qq.com/cgi-bin/qm/qr?k=xxxx 或 https://shang.qq.com/wpa/qunwpa?idkey=xxxx
const QQ_GROUP_INVITE_URL = "https://qm.qq.com/q/rsa3XRgFe8";
const QQ_GROUP_PROTOCOL_LINKS = [
  `tencent://GroupProfile?groupid=${QQ_GROUP_NUMBER}`,
  `mqqapi://card/show_pslcard?src_type=internal&version=1&uin=${QQ_GROUP_NUMBER}&card_type=group&source=qrcode`,
] as const;

function QqLogoIcon({ size = 14 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 448 512"
      fill="currentColor"
      aria-hidden="true"
    >
      <path d="M433.754 420.445c-11.526 1.393-44.86-52.741-44.86-52.741 0 31.345-16.136 72.247-51.051 101.786 16.842 5.192 54.843 19.167 45.803 34.421-7.316 12.343-125.51 7.881-159.632 4.037-34.122 3.844-152.316 8.306-159.632-4.037-9.045-15.25 28.918-29.214 45.783-34.415-34.92-29.539-51.059-70.445-51.059-101.792 0 0-33.334 54.134-44.859 52.741-5.37-.65-12.424-29.644 9.347-99.704 10.261-33.024 21.995-60.478 40.144-105.779C60.683 98.063 108.982.006 224 0c113.737.006 163.156 96.133 160.264 214.963 18.118 45.223 29.912 72.85 40.144 105.778 21.768 70.06 14.716 99.053 9.346 99.704z" />
    </svg>
  );
}

function BilibiliLogoIcon({ size = 15 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M8 5 5.5 2.5" />
      <path d="m16 5 2.5-2.5" />
      <rect x="3.5" y="5" width="17" height="15" rx="3.5" />
      <path d="M8.7 11.3h.01" />
      <path d="M15.3 11.3h.01" />
      <path d="M9 15.1c1.45.9 4.55.9 6 0" />
    </svg>
  );
}

function OfficeLogoIcon({ size = 15 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <rect x="4" y="4" width="16" height="16" rx="2" />
      <path d="M8 8h2l2 4-2 4H8" />
      <path d="M14 8v8" />
      <path d="M14 8h2" />
      <path d="M14 12h1.5" />
      <path d="M14 16h2" />
    </svg>
  );
}

// ── 工具函数：将文本中的 URL 渲染为可点击链接 ──
function renderTextWithLinks(text: string, onLinkClick: (url: string) => void) {
  const urlRegex = /(https?:\/\/[^\s)]+)/g;
  const parts = text.split(urlRegex);
  return parts.map((part, i) =>
    urlRegex.test(part) ? (
      <a key={i} href="#" className="officecli-error-link" onClick={(e) => { e.preventDefault(); onLinkClick(part); }}>{part}</a>
    ) : (
      <span key={i}>{part}</span>
    )
  );
}

// ── 主 App ────────────────────────────────────────────────────────
function App() {
  const [lang, setLang] = useState<Lang>(() =>
    (localStorage.getItem("mcp-lang") as Lang) ?? "zh"
  );
  const t = useT(lang);
  const updateInfo = useUpdateCheck(CURRENT_VERSION);
  const [dismissedVersion, setDismissedVersion] = useState(
    () => localStorage.getItem("mcp-update-dismissed") ?? ""
  );
  const updateDismissed =
    !!updateInfo?.latestVersion && updateInfo.latestVersion === dismissedVersion;
  const hasAvailableUpdate = !!updateInfo?.hasUpdate;
  const toggleLang = () => {
    const next: Lang = lang === "zh" ? "en" : "zh";
    setLang(next);
    localStorage.setItem("mcp-lang", next);
  };

  // ── 导航状态 ──
  const [activeSection, setActiveSection] = useState<"info" | "settings" | "builtin" | "externalMcp" | "externalSkill">("info");

  const [servers, setServers] = useState<ServerConfig[]>([]);
  const [listen, setListen] = useState("127.0.0.1:8765");
  const [apiPrefix, setApiPrefix] = useState("/api/v2");
  const [ssePath, setSsePath] = useState("/api/v2/sse");
  const [httpPath, setHttpPath] = useState("/api/v2/mcp");
  const [adminToken, setAdminToken] = useState("");
  const [mcpToken, setMcpToken] = useState("");
  const [defaultSkillRules, setDefaultSkillRules] = useState<SkillCommandRule[]>([]);
  const [skills, setSkills] = useState<SkillsConfig>(() => ensureSkillsConfig(undefined, []));
  const [skillGroups, setSkillGroups] = useState<SkillGroup[]>([]);
  const [skillWhitelistItems, setSkillWhitelistItems] = useState<SkillDirectoryItem[]>([]);
  const [skillsRulesDraft, setSkillsRulesDraft] = useState("[]");
  const [skillsRulesError, setSkillsRulesError] = useState<string | null>(null);
  const [skillsRulesAdvancedOpen, setSkillsRulesAdvancedOpen] = useState(false);
  const [skillRuleFormOpen, setSkillRuleFormOpen] = useState(false);
  const [editingSkillRuleId, setEditingSkillRuleId] = useState<string | null>(null);
  const [skillRuleForm, setSkillRuleForm] = useState<SkillRuleFormState>(() => createEmptySkillRuleForm());
  const [skillPending, setSkillPending] = useState<SkillConfirmation[]>([]);
  const [activeSkillPopupId, setActiveSkillPopupId] = useState<string | null>(null);
  const [skillActionBusy, setSkillActionBusy] = useState<Set<string>>(new Set());
  const [status, setStatus] = useState<GatewayProcessStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [autoTestingServers, setAutoTestingServers] = useState(false);
  const [serverTestStates, setServerTestStates] = useState<Record<string, ServerTestState>>({});
  const [serverAuthStates, setServerAuthStates] = useState<Record<string, ServerAuthState>>({});
  const [configLoaded, setConfigLoaded] = useState(false);
  const [localRuntimeSummary, setLocalRuntimeSummary] = useState<LocalRuntimeSummary | null>(null);
  const [localRuntimeDetectFailed, setLocalRuntimeDetectFailed] = useState(false);
  const [serversMode, setServersMode] = useState<"visual" | "json">("visual");
  const [jsonText, setJsonText] = useState("{}");
  const [jsonError, setJsonError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveSuccess, setSaveSuccess] = useState(false);
  const [savedConfigFingerprint, setSavedConfigFingerprint] = useState("");
  const [configPath, setConfigPath] = useState<string>("");
  const [serverUiIds, setServerUiIds] = useState<string[]>([]);
  const [draggedServerIndex, setDraggedServerIndex] = useState<number | null>(null);
  const [serverDropTarget, setServerDropTarget] = useState<ServerDragTarget | null>(null);
  const [draggedServerOffsetY, setDraggedServerOffsetY] = useState(0);
  const [draggedServerHeight, setDraggedServerHeight] = useState(0);
  // 删除确认弹窗状态
  const [deleteConfirm, setDeleteConfirm] = useState<{ open: boolean; index: number; name: string }>({
    open: false, index: -1, name: ""
  });
  const [resetConfigConfirmOpen, setResetConfigConfirmOpen] = useState(false);
  const [resetSkillRulesConfirmOpen, setResetSkillRulesConfirmOpen] = useState(false);
  const [skillDirDeleteConfirm, setSkillDirDeleteConfirm] = useState<{
    open: boolean;
    kind: SkillDirKind;
    id: string;
  }>({
    open: false, kind: "roots", id: ""
  });
  const [skillGroupDeleteConfirm, setSkillGroupDeleteConfirm] = useState<{
    open: boolean;
    groupId: string;
    groupName: string;
  }>({ open: false, groupId: "", groupName: "" });

  const [officeCliState, setOfficeCliState] = useState<{
    installed: boolean;
    version?: string;
    error?: string;
    installing?: boolean;
    checking?: boolean;
    releaseUrl?: string;
    /** 本次检测真实命中的绝对路径（不是 config 里可能过时的值） */
    path?: string;
  } | null>(null);
  const [showOfficeCliPath, setShowOfficeCliPath] = useState(false);
  const [officeCliCustomPath, setOfficeCliCustomPath] = useState("");
  const [installProgress, setInstallProgress] = useState(0);
  const [showReinstallConfirm, setShowReinstallConfirm] = useState(false);
  const [showShellEnvPopover, setShowShellEnvPopover] = useState(false);
  const [shellEnvDraftKey, setShellEnvDraftKey] = useState("");
  const [shellEnvDraftValue, setShellEnvDraftValue] = useState("");
  const [showPlansModal, setShowPlansModal] = useState(false);
  const [activePlans, setActivePlans] = useState<ActivePlan[]>([]);
  const [plansLoading, setPlansLoading] = useState(false);
  const [deletingPlanId, setDeletingPlanId] = useState<string | null>(null);
  const [planDeleteConfirmId, setPlanDeleteConfirmId] = useState<string | null>(null);
  const [planDeleteError, setPlanDeleteError] = useState<string | null>(null);

  /** 安装成功后跳过 useEffect 重复 check 的标记 */
  const skipNextOfficeCliCheckRef = useRef(false);

  const knownPendingIdsRef = useRef<Set<string>>(new Set());
  const dismissedPopupIdsRef = useRef<Set<string>>(new Set());
  const pendingFetchSeqRef = useRef(0);
  const serverUiIdSeqRef = useRef(0);
  const dragPointerIdRef = useRef<number | null>(null);
  const dragStartClientYRef = useRef(0);
  const dragFrameRef = useRef<number | null>(null);
  const latestPointerClientYRef = useRef<number | null>(null);
  const serverBlockRefs = useRef<Record<string, HTMLDivElement | null>>({});
  const serverDropTargetRef = useRef<ServerDragTarget | null>(null);
  const apiClient = useMemo(
    () => new ApiClient(listen, adminToken, apiPrefix),
    [adminToken, apiPrefix, listen],
  );

  const fetchPlans = useCallback(
    async ({ silent = false }: { silent?: boolean } = {}) => {
      if (!apiClient) return;
      if (!silent) setPlansLoading(true);
      try {
        const plans = await apiClient.fetchActivePlans();
        setActivePlans(plans);
      } catch {
        if (!silent) setActivePlans([]);
      } finally {
        if (!silent) setPlansLoading(false);
      }
    },
    [apiClient],
  );

  const handleOpenPlans = useCallback(() => {
    setShowPlansModal(true);
    setPlanDeleteConfirmId(null);
    setPlanDeleteError(null);
    fetchPlans();
  }, [fetchPlans]);

  const handleRefreshPlans = useCallback(() => {
    fetchPlans({ silent: true });
  }, [fetchPlans]);

  const togglePlanDeleteConfirm = useCallback((planningId: string) => {
    setPlanDeleteError(null);
    setPlanDeleteConfirmId((prev) => (prev === planningId ? null : planningId));
  }, []);

  const cancelPlanDeleteConfirm = useCallback(() => {
    setPlanDeleteError(null);
    setPlanDeleteConfirmId(null);
  }, []);

  const handleConfirmDeletePlan = useCallback(
    async (planningId: string) => {
      if (!apiClient) return;
      if (deletingPlanId) return;
      setPlanDeleteError(null);
      setDeletingPlanId(planningId);
      try {
        await apiClient.deleteActivePlan(planningId);
        setActivePlans((prev) => prev.filter((p) => p.planningId !== planningId));
        setPlanDeleteConfirmId(null);
        fetchPlans({ silent: true });
      } catch (error) {
        setPlanDeleteError(error instanceof Error ? error.message : String(error));
      } finally {
        setDeletingPlanId(null);
      }
    },
    [apiClient, deletingPlanId, fetchPlans],
  );

  useEffect(() => {
    if (!showPlansModal) return;
    const interval = window.setInterval(() => {
      fetchPlans({ silent: true });
    }, 5000);
    return () => window.clearInterval(interval);
  }, [showPlansModal, fetchPlans]);

  useEffect(() => {
    if (!showPlansModal) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setShowPlansModal(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [showPlansModal]);
  const createServerUiId = useCallback(() => `server-ui-${serverUiIdSeqRef.current++}`, []);
  const createServerUiIds = useCallback(
    (count: number) => Array.from({ length: count }, () => createServerUiId()),
    [createServerUiId],
  );
  const addServer = useCallback(() => {
    setServers((prev) => [{
      name: "", command: "npx", args: ["-y", ""],
      description: "", cwd: "", env: {}, lifecycle: null, stdioProtocol: "auto", enabled: true,
    }, ...prev]);
    setServerUiIds((prev) => [createServerUiId(), ...prev]);
    setServerTestStates({});
    setServerAuthStates({});
  }, [createServerUiId]);
  const draggedServerUiId = draggedServerIndex === null ? null : (serverUiIds[draggedServerIndex] ?? null);
  const previewServerUiIds = useMemo(() => {
    if (draggedServerIndex === null || !serverDropTarget) {
      return serverUiIds;
    }
    return moveArrayItem(serverUiIds, draggedServerIndex, serverDropTarget.index, serverDropTarget.position);
  }, [draggedServerIndex, serverDropTarget, serverUiIds]);
  const previewDraggedServerIndex = useMemo(() => {
    if (!draggedServerUiId) {
      return -1;
    }
    return previewServerUiIds.indexOf(draggedServerUiId);
  }, [draggedServerUiId, previewServerUiIds]);
  const serverShiftDistance = draggedServerHeight > 0
    ? draggedServerHeight + SERVER_LIST_GAP_PX
    : 0;

  // Built-in tool definitions. `requiresWhitelist` controls whether the toggle
  // is gated behind having at least one valid whitelist directory configured.
  // When adding a new tool, just append an entry here.
  const builtinToolDefs: {
    key: keyof BuiltinToolsConfig;
    name: string;
    icon: React.ReactNode;
    descKey: TKey;
    requiresWhitelist: boolean;
  }[] = [
    { key: "readFile", name: "read_file", icon: <FileText size={15} />, descKey: "builtInReadFileDesc", requiresWhitelist: true },
    { key: "shellCommand", name: "shell_command", icon: <Terminal size={15} />, descKey: "builtInShellDesc", requiresWhitelist: true },
    { key: "multiEditFile", name: "multi_edit_file", icon: <FilePenLine size={15} />, descKey: "builtInMultiEditDesc", requiresWhitelist: true },
    { key: "taskPlanning", name: "task-planning", icon: <ListChecks size={15} />, descKey: "builtInTaskPlanningDesc", requiresWhitelist: false },
    { key: "chromeCdp", name: "chrome-cdp", icon: <Chrome size={15} />, descKey: "builtInChromeCdpDesc", requiresWhitelist: false },
    { key: "chatPlusAdapterDebugger", name: "chat-plus-adapter-debugger", icon: <Bug size={15} />, descKey: "builtInChatPlusAdapterDesc", requiresWhitelist: false },
  ];

  const setWhitelistItemsAndSync = useCallback((nextItems: SkillDirectoryItem[]) => {
    setSkillWhitelistItems(nextItems);
    const nextDirs = nextItems.map((item) => item.path.trim()).filter((item) => item.length > 0);
    const hasAny = nextDirs.length > 0;
    setSkills((prev) => ({
      ...prev,
      builtinTools: hasAny ? prev.builtinTools : {
        ...prev.builtinTools,
        ...Object.fromEntries(builtinToolDefs.filter((td) => td.requiresWhitelist).map((td) => [td.key, false])),
        officeCli: false,
      },
      policy: {
        ...prev.policy,
        pathGuard: {
          ...prev.policy.pathGuard,
          whitelistDirs: nextDirs,
        },
      },
    }));
  }, []);

  const reloadLocalConfig = useCallback(async () => {
    const [cfg, builtinRules] = await Promise.all([loadLocalConfig(), getDefaultSkillRules().catch(() => [])]);
    setDefaultSkillRules(builtinRules);
    const nextServers = cfg.servers ?? [];
    const nextListen = cfg.listen || "127.0.0.1:8765";
    const nextApiPrefix = cfg.apiPrefix || "/api/v2";
    const nextSsePath = cfg.transport?.sse?.basePath || "/api/v2/sse";
    const nextHttpPath = cfg.transport?.streamableHttp?.basePath || "/api/v2/mcp";
    const nextAdminToken = cfg.security?.admin?.token ?? "";
    const nextMcpToken = cfg.security?.mcp?.token ?? "";

    setServers(nextServers);
    setServerUiIds(createServerUiIds(nextServers.length));
    setServerTestStates({});
    setServerAuthStates({});
    setAutoTestingServers(false);
    setListen(nextListen);
    setApiPrefix(nextApiPrefix);
    setSsePath(nextSsePath);
    setHttpPath(nextHttpPath);
    setAdminToken(nextAdminToken);
    setMcpToken(nextMcpToken);
    const nextSkills = ensureSkillsConfig(cfg.skills, builtinRules);
    setSkills(nextSkills);
    if (nextSkills.builtinTools.officeCliPath) { setOfficeCliCustomPath(nextSkills.builtinTools.officeCliPath); } else { getOfficeCliDefaultPath().then((p) => { setOfficeCliCustomPath(p); setSkills((prev) => ({ ...prev, builtinTools: { ...prev.builtinTools, officeCliPath: p } })); }).catch(() => {}); }
    const nextWhitelistItems = (nextSkills.policy.pathGuard.whitelistDirs.length > 0
      ? nextSkills.policy.pathGuard.whitelistDirs
      : [""]
    ).map((path) => ({
      ...createSkillDirectoryItem(path, true),
      status: "idle" as const,
    }));
    setSkillWhitelistItems(nextWhitelistItems);
    // Load skill groups from config, migrating legacy rootEntries into a default group
    let loadedGroups: SkillGroup[];
    if (Array.isArray(nextSkills.rootGroups) && nextSkills.rootGroups.length > 0) {
      loadedGroups = nextSkills.rootGroups.map((g) => {
        const entries = Array.isArray(g.rootEntries) && g.rootEntries.length > 0
          ? g.rootEntries
          : (g.roots ?? []).map((p) => ({ path: p, enabled: true }));
        return createSkillGroup(g.name, entries.map((e) => ({
          ...createSkillDirectoryItem(e.path, e.enabled),
          status: e.path.trim().length > 0 ? "checking" as const : "idle" as const,
        })));
      });
    } else {
      // Migrate: old config has rootEntries/roots but no rootGroups -> create default group
      const legacyEntries = Array.isArray(nextSkills.rootEntries) && nextSkills.rootEntries.length > 0
        ? nextSkills.rootEntries
        : nextSkills.roots.map((p) => ({ path: p, enabled: true }));
      const hasLegacyItems = legacyEntries.some((e) => e.path.trim().length > 0);
      if (hasLegacyItems) {
        loadedGroups = [createSkillGroup("skills", legacyEntries.map((e) => ({
          ...createSkillDirectoryItem(e.path, e.enabled),
          status: e.path.trim().length > 0 ? "checking" as const : "idle" as const,
        })))];
      } else {
        loadedGroups = [];
      }
    }
    setSkillGroups(loadedGroups);
    // Validate group items
    loadedGroups.forEach((group) => {
      group.items.forEach((item) => {
        if (item.path.trim().length > 0) {
          validateSkillDirectory(item.path.trim()).then((result) => {
            const status = skillDirectoryStatusFromResult(result);
            setSkillGroups((prev) => prev.map((g) =>
              g.id === group.id
                ? { ...g, items: g.items.map((i) => i.id === item.id ? { ...i, status, enabled: status === "valid" ? i.enabled : false } : i) }
                : g
            ));
          }).catch(() => {
            setSkillGroups((prev) => prev.map((g) =>
              g.id === group.id
                ? { ...g, items: g.items.map((i) => i.id === item.id ? { ...i, status: "error" as const, enabled: false } : i) }
                : g
            ));
          });
        }
      });
    });

    setSkillsRulesDraft(JSON.stringify(nextSkills.policy.rules, null, 2));
    setSkillsRulesError(null);
    setSkillsRulesAdvancedOpen(false);
    setSkillRuleFormOpen(false);
    setEditingSkillRuleId(null);
    setSkillRuleForm(createEmptySkillRuleForm());
    setJsonText(buildServersJson(nextServers));
    setJsonError(null);
    setServersMode("visual");
    const { officeCliPath: _ocpSaved, ...savedBuiltinTools } = nextSkills.builtinTools;
    setSavedConfigFingerprint(createEditableConfigFingerprint(createEditableConfigSnapshot({
      servers: nextServers,
      listen: nextListen,
      apiPrefix: nextApiPrefix,
      ssePath: nextSsePath,
      httpPath: nextHttpPath,
      adminToken: nextAdminToken,
      mcpToken: nextMcpToken,
      skills: { ...nextSkills, builtinTools: savedBuiltinTools as typeof nextSkills.builtinTools },
    })));
    setConfigLoaded(true);
  }, [createServerUiIds]);

  // ── 初始加载配置 ──
  useEffect(() => {
    reloadLocalConfig().catch((e) => setError(String(e)));
    getConfigPath().then(setConfigPath).catch(() => {});
  }, [reloadLocalConfig]);

  useEffect(() => {
    let cancelled = false;

    detectLocalRuntimes()
      .then((summary) => {
        if (cancelled) {
          return;
        }
        setLocalRuntimeSummary(summary);
        setLocalRuntimeDetectFailed(false);
      })
      .catch(() => {
        if (cancelled) {
          return;
        }
        setLocalRuntimeDetectFailed(true);
      });

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const baseTitle = `${t("windowTitle")} v${CURRENT_VERSION}`;
    const nextTitle = hasAvailableUpdate && updateInfo?.latestVersion
      ? `${baseTitle} ● ${t("updateAvailableShort")} ${updateInfo.latestVersion}`
      : baseTitle;

    document.title = nextTitle;
    setMainWindowTitle(nextTitle).catch(() => {});
  }, [hasAvailableUpdate, lang, t, updateInfo?.latestVersion]);

  const parsedJsonServers = useMemo(() => {
    if (serversMode !== "json") {
      return null;
    }
    try {
      return jsonToServers(JSON.parse(jsonText) as Record<string, unknown>);
    } catch {
      return null;
    }
  }, [jsonText, serversMode]);

  const switchToJson = () => {
    if (serversMode === "json") {
      setJsonError(null);
      return;
    }
    setJsonText(buildServersJson(servers));
    setJsonError(null);
    setServersMode("json");
  };
  const switchToVisual = () => {
    if (serversMode === "visual") {
      setJsonError(null);
      return;
    }
    if (!parsedJsonServers) {
      setJsonError(t("jsonParseError"));
      return;
    }
    setServers(parsedJsonServers);
    setServerUiIds(createServerUiIds(parsedJsonServers.length));
    setServerTestStates({});
    setServerAuthStates({});
    setDraggedServerIndex(null);
    setServerDropTarget(null);
    setJsonError(null);
    setServersMode("visual");
  };
  const formatServersJson = () => {
    try {
      setJsonText(JSON.stringify(JSON.parse(jsonText), null, 2));
      setJsonError(null);
    } catch {
      setJsonError(t("formatError"));
    }
  };

  // ── 进程状态轮询 ──
  const refreshStatus = useCallback(async (): Promise<PollOutcome> => {
    try {
      const current = await getGatewayStatus();
      setStatus(current);
      return current.running ? "active" : "idle";
    } catch {
      return "error";
    }
  }, []);
  usePolling(refreshStatus, {
    enabled: true,
    activeMs: 3000,
    idleMs: 10000,
    errorMs: 5000,
    immediate: true,
  });

  const running = !!status?.running;

  const refreshServerAuthStates = useCallback(async (): Promise<PollOutcome> => {
    if (servers.length === 0) {
      setServerAuthStates({});
      return "idle";
    }

    const results = await Promise.all(
      servers.map(async (server, index) => {
        const key = serverTestKey(index, server);
        try {
          const authState = await getServerAuthStateLocal(server);
          return [key, authState] as const;
        } catch {
          return [key, createEmptyAuthState()] as const;
        }
      }),
    );

    const nextState = Object.fromEntries(results);
    setServerAuthStates(nextState);
    const hasActiveAuth = Object.values(nextState).some((state) =>
      state.status === "starting"
      || state.status === "auth_pending"
      || state.status === "browser_opened"
      || state.status === "waiting_callback"
      || state.status === "authorized",
    );
    return hasActiveAuth ? "active" : "idle";
  }, [servers]);

  usePolling(refreshServerAuthStates, {
    enabled: configLoaded,
    activeMs: 2000,
    idleMs: 10000,
    errorMs: 5000,
    immediate: true,
  });

  const runServerConnectivityTest = useCallback(async (server: ServerConfig, index: number) => {
    const key = serverTestKey(index, server);
    if (!server.command.trim()) {
      setServerTestStates((prev) => ({
        ...prev,
        [key]: {
          status: "failed",
          message: t("serverTestMissingCommand"),
        },
      }));
      return;
    }

    setServerTestStates((prev) => ({
      ...prev,
      [key]: {
        status: "testing",
        message: "",
      },
    }));

    try {
      const result: ServerConnectivityTestResult = await testMcpServerLocal(server);
      const nextStatus: ServerTestStatus = result.ok
        ? "success"
        : authStateTone(result.auth) === "auth_required"
          ? "auth_required"
          : "failed";
      setServerTestStates((prev) => ({
        ...prev,
        [key]: {
          status: nextStatus,
          message: typeof result.message === "string" ? result.message : "",
          testedAt: typeof result.testedAt === "string" ? result.testedAt : new Date().toISOString(),
        },
      }));
      setServerAuthStates((prev) => ({
        ...prev,
        [key]: result.auth ?? createEmptyAuthState(),
      }));
    } catch (error) {
      setServerTestStates((prev) => ({
        ...prev,
        [key]: {
          status: "failed",
          message: asErrorMessage(error),
        },
      }));
    }
  }, [t]);

  const runServerReauthorize = useCallback(async (server: ServerConfig, index: number) => {
    const key = serverTestKey(index, server);
    setServerTestStates((prev) => ({
      ...prev,
      [key]: {
        status: "testing",
        message: "",
      },
    }));

    try {
      const result = await reauthorizeServerLocal(server);
      setServerAuthStates((prev) => ({
        ...prev,
        [key]: result.auth ?? createEmptyAuthState(),
      }));
      setServerTestStates((prev) => ({
        ...prev,
        [key]: {
          status: result.ok ? "success" : authStateTone(result.auth) === "auth_required" ? "auth_required" : "failed",
          message: typeof result.message === "string" ? result.message : "",
          testedAt: typeof result.testedAt === "string" ? result.testedAt : new Date().toISOString(),
        },
      }));
    } catch (error) {
      setServerTestStates((prev) => ({
        ...prev,
        [key]: {
          status: "failed",
          message: asErrorMessage(error),
        },
      }));
    }
  }, []);

  const runServerClearAuth = useCallback(async (server: ServerConfig, index: number) => {
    const key = serverTestKey(index, server);
    try {
      const nextAuthState = await clearServerAuthLocal(server);
      setServerAuthStates((prev) => ({
        ...prev,
        [key]: nextAuthState,
      }));
      setServerTestStates((prev) => ({
        ...prev,
        [key]: {
          status: "idle",
          message: "",
        },
      }));
    } catch (error) {
      setServerTestStates((prev) => ({
        ...prev,
        [key]: {
          status: "failed",
          message: asErrorMessage(error),
        },
      }));
    }
  }, []);

  const runEnabledServerConnectivityTests = useCallback(async (targetServers: ServerConfig[]) => {
    const candidates = targetServers
      .map((server, index) => ({ server, index }))
      .filter(({ server }) => server.enabled && server.command.trim().length > 0);
    if (candidates.length === 0) {
      return;
    }

    setAutoTestingServers(true);
    try {
      for (const item of candidates) {
        await runServerConnectivityTest(item.server, item.index);
      }
    } finally {
      setAutoTestingServers(false);
    }
  }, [runServerConnectivityTest]);

  const fetchSkillPending = useCallback(async (): Promise<PollOutcome> => {
    const fetchSeq = pendingFetchSeqRef.current + 1;
    pendingFetchSeqRef.current = fetchSeq;

    if (!running) {
      if (fetchSeq !== pendingFetchSeqRef.current) return "idle";
      setSkillPending([]);
      setActiveSkillPopupId(null);
      knownPendingIdsRef.current = new Set();
      dismissedPopupIdsRef.current = new Set();
      return "idle";
    }
    try {
      const pending = await apiClient.listSkillConfirmations();
      if (fetchSeq !== pendingFetchSeqRef.current) return "idle";
      const nextIds = new Set(pending.map((item) => item.id));
      const knownIds = knownPendingIdsRef.current;
      const newItems = pending.filter((item) => !knownIds.has(item.id));

      dismissedPopupIdsRef.current = new Set(
        Array.from(dismissedPopupIdsRef.current).filter((id) => nextIds.has(id)),
      );

      if (newItems.length > 0) {
        setActiveSection("settings");
        void focusMainWindowForSkillConfirmation().catch(() => {});
        const popupCandidate = newItems.find((item) => !dismissedPopupIdsRef.current.has(item.id)) ?? newItems[0];
        setActiveSkillPopupId(popupCandidate.id);
      }

      setSkillPending(pending);
      setActiveSkillPopupId((current) => {
        if (current && nextIds.has(current)) return current;
        const fallback = pending.find((item) => !dismissedPopupIdsRef.current.has(item.id));
        return fallback ? fallback.id : null;
      });
      knownPendingIdsRef.current = nextIds;
      return pending.length > 0 ? "active" : "idle";
    } catch (error) {
      if (fetchSeq !== pendingFetchSeqRef.current) return "idle";
      setError(String(error));
      return "error";
    }
  }, [apiClient, running]);

  usePolling(fetchSkillPending, {
    enabled: running,
    activeMs: 3000,
    idleMs: 10000,
    errorMs: 5000,
    immediate: true,
  });

  const updateConfirmation = useCallback(async (id: string, action: "approve" | "reject") => {
    setSkillActionBusy((prev) => {
      const next = new Set(prev);
      next.add(id);
      return next;
    });
    let success = false;
    try {
      if (action === "approve") {
        await apiClient.approveSkillConfirmation(id);
      } else {
        await apiClient.rejectSkillConfirmation(id);
      }
      await fetchSkillPending();
      success = true;
    } catch (error) {
      if (isConfirmationAlreadyResolvedError(error)) {
        await fetchSkillPending();
        success = true;
      } else {
        setError(String(error));
      }
    } finally {
      setSkillActionBusy((prev) => {
        const next = new Set(prev);
        next.delete(id);
        return next;
      });
    }
    return success;
  }, [apiClient, fetchSkillPending]);

  const handleSkillConfirmationAction = useCallback((id: string, action: "approve" | "reject") => {
    void (async () => {
      const ok = await updateConfirmation(id, action);
      if (!ok) return;
      dismissedPopupIdsRef.current = new Set(dismissedPopupIdsRef.current).add(id);
      setActiveSkillPopupId((current) => (current === id ? null : current));
    })();
  }, [updateConfirmation]);

  const deferSkillConfirmationPopup = useCallback((id: string) => {
    dismissedPopupIdsRef.current = new Set(dismissedPopupIdsRef.current).add(id);
    setActiveSkillPopupId((current) => (current === id ? null : current));
  }, []);

  const activeSkillPopupItem = useMemo(
    () => skillPending.find((item) => item.id === activeSkillPopupId) ?? null,
    [activeSkillPopupId, skillPending],
  );

  const onRulesDraftChange = (value: string) => {
    setSkillsRulesDraft(value);
    try {
      const parsed = parseRulesJson(value);
      setSkills((prev) => ({
        ...prev,
        policy: {
          ...prev.policy,
          rules: parsed,
        },
      }));
      setSkillsRulesError(null);
    } catch {
      setSkillsRulesError(t("skillsRulesJsonError"));
    }
  };

  const syncSkillRules = (rules: SkillCommandRule[]) => {
    setSkills((prev) => ({
      ...prev,
      policy: {
        ...prev.policy,
        rules,
      },
    }));
    setSkillsRulesDraft(JSON.stringify(rules, null, 2));
    setSkillsRulesError(null);
  };

  const startAddSkillRule = () => {
    setEditingSkillRuleId(null);
    setSkillRuleForm(createEmptySkillRuleForm());
    setSkillRuleFormOpen(true);
  };

  const editSkillRule = (rule: SkillCommandRule) => {
    setEditingSkillRuleId(rule.id);
    setSkillRuleForm(ruleToForm(rule));
    setSkillRuleFormOpen(true);
  };

  const cancelSkillRuleForm = () => {
    setEditingSkillRuleId(null);
    setSkillRuleForm(createEmptySkillRuleForm());
    setSkillRuleFormOpen(false);
  };

  const submitSkillRuleForm = () => {
    if (!isSkillRuleFormValid(skillRuleForm)) return;
    const nextRules = editingSkillRuleId
      ? skills.policy.rules.map((rule) =>
          rule.id === editingSkillRuleId ? formToRule(skillRuleForm, rule.id, rule) : rule
        )
      : [...skills.policy.rules, formToRule(skillRuleForm, createSkillRuleId(skills.policy.rules))];
    syncSkillRules(nextRules);
    cancelSkillRuleForm();
  };

  const copySkillRule = (rule: SkillCommandRule) => {
    const nextRule = {
      ...rule,
      id: createSkillRuleId(skills.policy.rules),
    };
    const index = skills.policy.rules.findIndex((item) => item.id === rule.id);
    const nextRules = [...skills.policy.rules];
    nextRules.splice(index >= 0 ? index + 1 : nextRules.length, 0, nextRule);
    syncSkillRules(nextRules);
  };

  const deleteSkillRule = (id: string) => {
    syncSkillRules(skills.policy.rules.filter((rule) => rule.id !== id));
    if (editingSkillRuleId === id) {
      cancelSkillRuleForm();
    }
  };

  const requestResetSkillRules = () => {
    setResetSkillRulesConfirmOpen(true);
  };

  const cancelResetSkillRules = () => {
    setResetSkillRulesConfirmOpen(false);
  };

  const confirmResetSkillRules = useCallback(async () => {
    setResetSkillRulesConfirmOpen(false);
    try {
      const rules = defaultSkillRules.length > 0
        ? defaultSkillRules
        : await getDefaultSkillRules();
      syncSkillRules(rules);
      setDefaultSkillRules(rules);
      cancelSkillRuleForm();
    } catch (error) {
      setError(asErrorMessage(error));
    }
  }, [defaultSkillRules]);

  const addWhitelistItem = () => {
    setWhitelistItemsAndSync([...skillWhitelistItems, createSkillDirectoryItem("", true)]);
  };

  const removeWhitelistItem = (id: string) => {
    const next = skillWhitelistItems.filter((item) => item.id !== id);
    setWhitelistItemsAndSync(next.length > 0 ? next : [createSkillDirectoryItem("", true)]);
  };

  const requestRemoveWhitelistItem = (id: string) => {
    setSkillDirDeleteConfirm({ open: true, kind: "whitelist", id });
  };

  const confirmSkillDirDelete = () => {
    const { kind, id } = skillDirDeleteConfirm;
    if (!id) {
      setSkillDirDeleteConfirm({ open: false, kind: "roots", id: "" });
      return;
    }

    if (kind === "whitelist") {
      removeWhitelistItem(id);
    }
    setSkillDirDeleteConfirm({ open: false, kind: "roots", id: "" });
  };

  const cancelSkillDirDelete = () => {
    setSkillDirDeleteConfirm({ open: false, kind: "roots", id: "" });
  };

  const updateWhitelistItemPath = (id: string, path: string) => {
    setWhitelistItemsAndSync(skillWhitelistItems.map((item) =>
      item.id === id ? { ...item, path, status: "idle" } : item
    ));
  };


  // ── Skill Group handlers ──
  const syncSkillGroupsToConfig = useCallback((groups: SkillGroup[]) => {
    const rootGroups: SkillGroupEntry[] = groups.map((group) => {
      const entries = group.items
        .map((item) => ({ path: item.path.trim(), enabled: item.enabled }))
        .filter((e) => e.path.length > 0);
      return {
        name: group.name,
        roots: entries.filter((e) => e.enabled).map((e) => e.path),
        rootEntries: entries,
      };
    });
    // Also flatten all group roots into the top-level roots/rootEntries for backend compatibility
    const allEntries = rootGroups.flatMap((g) => g.rootEntries ?? []);
    const allRoots = rootGroups.flatMap((g) => g.roots);
    setSkills((prev) => ({ ...prev, roots: allRoots, rootEntries: allEntries, rootGroups }));
  }, []);

  const setGroupsAndSync = useCallback((nextGroups: SkillGroup[]) => {
    setSkillGroups(nextGroups);
    syncSkillGroupsToConfig(nextGroups);
  }, [syncSkillGroupsToConfig]);

  const addSkillGroup = () => {
    const index = skillGroups.length + 1;
    const name = `skills${index > 1 ? index : ""}`;
    setGroupsAndSync([...skillGroups, createSkillGroup(name, [])]);
  };

  const removeSkillGroup = (groupId: string) => {
    const group = skillGroups.find((g) => g.id === groupId);
    if (!group) return;
    setSkillGroupDeleteConfirm({ open: true, groupId, groupName: group.name });
  };

  const confirmSkillGroupDelete = () => {
    const { groupId } = skillGroupDeleteConfirm;
    setGroupsAndSync(skillGroups.filter((g) => g.id !== groupId));
    setSkillGroupDeleteConfirm({ open: false, groupId: "", groupName: "" });
  };

  const cancelSkillGroupDelete = () => {
    setSkillGroupDeleteConfirm({ open: false, groupId: "", groupName: "" });
  };

  const renameSkillGroup = (groupId: string, name: string) => {
    // Don't allow empty group names - keep the old name if empty
    if (!name.trim()) return;
    setGroupsAndSync(skillGroups.map((g) => g.id === groupId ? { ...g, name: name.trim() } : g));
  };

  const addGroupItem = (groupId: string) => {
    setGroupsAndSync(skillGroups.map((g) =>
      g.id === groupId ? { ...g, items: [createSkillDirectoryItem("", false), ...g.items] } : g
    ));
  };

  const removeGroupItem = (groupId: string, itemId: string) => {
    setGroupsAndSync(skillGroups.map((g) =>
      g.id === groupId ? { ...g, items: g.items.filter((i) => i.id !== itemId) } : g
    ));
  };

  const updateGroupItemPath = (groupId: string, itemId: string, path: string) => {
    setGroupsAndSync(skillGroups.map((g) =>
      g.id === groupId
        ? { ...g, items: g.items.map((i) => i.id === itemId ? { ...i, path, status: "idle" as const, enabled: false } : i) }
        : g
    ));
  };

  // Helper: validate a group item path and update its status
  const runGroupItemValidation = (groupId: string, itemId: string, path: string) => {
    const normalized = path.trim();
    if (!normalized) return;
    setSkillGroups((prev) => prev.map((g) =>
      g.id === groupId
        ? { ...g, items: g.items.map((i) => i.id === itemId ? { ...i, status: "checking" as const } : i) }
        : g
    ));
    validateSkillDirectory(normalized).then((result) => {
      const status = skillDirectoryStatusFromResult(result);
      setSkillGroups((prev) => {
        const next = prev.map((g) =>
          g.id === groupId
            ? { ...g, items: g.items.map((i) => {
                if (i.id !== itemId || i.path.trim() !== normalized) return i;
                return { ...i, status, enabled: status === "valid" ? i.enabled : false };
              }) }
            : g
        );
        syncSkillGroupsToConfig(next);
        return next;
      });
    }).catch(() => {
      setSkillGroups((prev) => {
        const next = prev.map((g) =>
          g.id === groupId
            ? { ...g, items: g.items.map((i) => i.id === itemId ? { ...i, status: "error" as const, enabled: false } : i) }
            : g
        );
        syncSkillGroupsToConfig(next);
        return next;
      });
    });
  };

  const validateGroupItem = (groupId: string, itemId: string) => {
    const group = skillGroups.find((g) => g.id === groupId);
    const item = group?.items.find((i) => i.id === itemId);
    if (!item) return;
    runGroupItemValidation(groupId, itemId, item.path);
  };

  const browseGroupItem = async (groupId: string, itemId: string) => {
    const group = skillGroups.find((g) => g.id === groupId);
    const item = group?.items.find((i) => i.id === itemId);
    if (!item) return;
    try {
      const selected = await pickFolderDialog(item.path.trim() || undefined);
      if (!selected) return;
      setSkillGroups((prev) => {
        const next = prev.map((g) =>
          g.id === groupId
            ? { ...g, items: g.items.map((i) => i.id === itemId ? { ...i, path: selected, status: "checking" as const, enabled: false } : i) }
            : g
        );
        syncSkillGroupsToConfig(next);
        return next;
      });
      runGroupItemValidation(groupId, itemId, selected);
    } catch (error) {
      setError(String(error));
    }
  };

  const toggleGroupItemEnabled = (groupId: string, itemId: string) => {
    setGroupsAndSync(skillGroups.map((g) =>
      g.id === groupId
        ? { ...g, items: g.items.map((i) => {
            if (i.id !== itemId) return i;
            if (i.status !== "valid") return { ...i, enabled: false };
            return { ...i, enabled: !i.enabled };
          }) }
        : g
    ));
  };

  const importToGroup = async (groupId: string) => {
    try {
      const selected = await pickFolderDialog();
      if (!selected) return;
      const discovered = await scanSkillDirectories(selected);
      if (discovered.length === 0) {
        setError(t("noSkillRootsFound"));
        return;
      }
      const importedItems = discovered.map((item) => ({
        ...createSkillDirectoryItem(item.path, true),
        status: "valid" as const,
      }));
      setGroupsAndSync(skillGroups.map((g) => {
        if (g.id !== groupId) return g;
        const existingPaths = new Set(g.items.map((i) => i.path.trim().toLowerCase()));
        const newItems = importedItems.filter((i) => !existingPaths.has(i.path.trim().toLowerCase()));
        return { ...g, items: [...newItems, ...g.items] };
      }));
      setError(null);
    } catch (error) {
      setError(asErrorMessage(error));
    }
  };

  const browseWhitelistItem = async (id: string) => {
    const target = skillWhitelistItems.find((item) => item.id === id);
    if (!target) return;
    try {
      const selected = await pickFolderDialog(target.path.trim() || undefined);
      if (!selected) return;
      setWhitelistItemsAndSync(skillWhitelistItems.map((item) =>
        item.id === id ? { ...item, path: selected, status: "idle" } : item
      ));
    } catch (error) {
      setError(String(error));
    }
  };

  const resolveServers = (): ServerConfig[] | null => {
    let list: ServerConfig[];
    if (serversMode === "json") {
      if (!parsedJsonServers) {
        setJsonError(t("jsonParseErrorStart"));
        return null;
      }
      list = parsedJsonServers;
    } else {
      list = servers;
    }
    const valid = list.filter((s) => s.name.trim() && s.command.trim());
    if (list.length > 0 && valid.length === 0) {
      setError(t("allServersInvalid"));
      return null;
    }
    return valid;
  };

  const currentConfigFingerprint = useMemo(() => {
    if (serversMode === "json" && !parsedJsonServers) {
      return null;
    }
    const compareServers = serversMode === "json" ? parsedJsonServers : servers;

    const skillsForFingerprint = normalizeSkillsForSubmit(skills, defaultSkillRules);
    // officeCliPath is auto-persisted on verify/install, exclude from dirty check
    const { officeCliPath: _ocp, ...builtinToolsForFp } = skillsForFingerprint.builtinTools;
    return createEditableConfigFingerprint(createEditableConfigSnapshot({
      servers: compareServers ?? servers,
      listen,
      apiPrefix,
      ssePath,
      httpPath,
      adminToken,
      mcpToken,
      skills: { ...skillsForFingerprint, builtinTools: builtinToolsForFp as typeof skillsForFingerprint.builtinTools },
    }));
  }, [servers, serversMode, parsedJsonServers, listen, apiPrefix, ssePath, httpPath, adminToken, mcpToken, skills, defaultSkillRules]);

  const isConfigDirty = configLoaded
    && (currentConfigFingerprint === null || currentConfigFingerprint !== savedConfigFingerprint);
  const showRestartRequiredHint = running && isConfigDirty;

  const persistConfig = async (nextServers: ServerConfig[]) => {
    const snapshot = createEditableConfigSnapshot({
      servers: nextServers,
      listen,
      apiPrefix,
      ssePath,
      httpPath,
      adminToken,
      mcpToken,
      skills: normalizeSkillsForSubmit(skills, defaultSkillRules),
    });
    const cfg: GatewayConfig = await loadLocalConfig();
    cfg.servers = snapshot.servers;
    cfg.listen = snapshot.listen;
    cfg.apiPrefix = snapshot.apiPrefix;
    cfg.transport = snapshot.transport;
    cfg.security = snapshot.security;
    cfg.skills = snapshot.skills;
    await saveLocalConfig(cfg);
    // Exclude officeCliPath from fingerprint (auto-persisted separately)
    const { officeCliPath: _ocpPersist, ...persistBuiltinTools } = snapshot.skills.builtinTools;
    const fpSnapshot = { ...snapshot, skills: { ...snapshot.skills, builtinTools: persistBuiltinTools as typeof snapshot.skills.builtinTools } };
    return createEditableConfigFingerprint(fpSnapshot);
  };

  const ensureSkillsAccessReady = () => true;

  const handleStart = async () => {
    if (skillsRulesError) {
      setError(skillsRulesError);
      return;
    }
    if (!ensureSkillsAccessReady()) return;
    const nextServers = resolveServers();
    if (nextServers === null) return;
    setError(null); setBusy(true);
    try {
      const latestFingerprint = await persistConfig(nextServers);
      setSavedConfigFingerprint(latestFingerprint);
      await startGateway();
      await refreshStatus();
      await fetchSkillPending();
      void runEnabledServerConnectivityTests(nextServers);
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };

  const handleStop = async () => {
    setError(null); setBusy(true);
    try {
      await stopGateway();
      await refreshStatus();
      setSkillPending([]);
      setAutoTestingServers(false);
    }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };

  // ── 独立保存配置 ──
  const handleSave = async () => {
    if (skillsRulesError) {
      setError(skillsRulesError);
      return;
    }
    if (!ensureSkillsAccessReady()) return;
    const nextServers = resolveServers();
    if (nextServers === null) return;
    setError(null); setSaving(true); setSaveSuccess(false);
    try {
      const latestFingerprint = await persistConfig(nextServers);
      setSavedConfigFingerprint(latestFingerprint);
      setSaveSuccess(true);
      setTimeout(() => setSaveSuccess(false), 2000);
    } catch (e) { setError(String(e)); }
    finally { setSaving(false); }
  };

  // ── 删除确认逻辑 ──
  const requestDelete = (index: number, name: string) => {
    setDeleteConfirm({ open: true, index, name: name || `服务 ${index + 1}` });
  };
  const confirmDelete = () => {
    setServers((prev) => prev.filter((_, xi) => xi !== deleteConfirm.index));
    setServerTestStates({});
    setServerAuthStates({});
    setDeleteConfirm({ open: false, index: -1, name: "" });
    setServerUiIds((prev) => prev.filter((_, xi) => xi !== deleteConfirm.index));
    setJsonText(buildServersJson(servers.filter((_, xi) => xi !== deleteConfirm.index)));
  };
  const cancelDelete = () => {
    setDeleteConfirm({ open: false, index: -1, name: "" });
  };

  const resetServerDragState = useCallback(() => {
    if (dragFrameRef.current !== null) {
      window.cancelAnimationFrame(dragFrameRef.current);
      dragFrameRef.current = null;
    }
    dragPointerIdRef.current = null;
    dragStartClientYRef.current = 0;
    latestPointerClientYRef.current = null;
    serverDropTargetRef.current = null;
    setDraggedServerIndex(null);
    setServerDropTarget(null);
    setDraggedServerOffsetY(0);
    setDraggedServerHeight(0);
  }, []);

  const updateServerDropTarget = useCallback((nextTarget: ServerDragTarget | null) => {
    serverDropTargetRef.current = nextTarget;
    setServerDropTarget((current) =>
      current?.index === nextTarget?.index && current?.position === nextTarget?.position
        ? current
        : nextTarget,
    );
  }, []);

  const findServerDropTarget = useCallback((clientY: number, sourceIndex: number): ServerDragTarget | null => {
    const blocks = serverUiIds
      .map((id, index) => ({
        index,
        node: serverBlockRefs.current[id] ?? null,
      }))
      .filter((entry) => entry.index !== sourceIndex)
      .filter((entry): entry is { index: number; node: HTMLDivElement } => entry.node !== null);

    for (const block of blocks) {
      const rect = block.node.getBoundingClientRect();
      const midpoint = rect.top + rect.height / 2;
      if (clientY < midpoint) {
        return block.index === sourceIndex ? null : { index: block.index, position: "before" };
      }
    }

    const lastBlock = blocks.length > 0 ? blocks[blocks.length - 1] : null;
    if (!lastBlock || lastBlock.index === sourceIndex) {
      return null;
    }
    return { index: lastBlock.index, position: "after" };
  }, [serverUiIds]);

  const handleServerPointerDown = useCallback((event: ReactPointerEvent<HTMLDivElement>, index: number) => {
    if (event.button !== 0 || servers.length <= 1) {
      return;
    }

    event.preventDefault();
    const targetId = serverUiIds[index] ?? `server-${index}`;
    const blockRect = serverBlockRefs.current[targetId]?.getBoundingClientRect();
    dragPointerIdRef.current = event.pointerId;
    dragStartClientYRef.current = event.clientY;
    latestPointerClientYRef.current = event.clientY;
    setDraggedServerOffsetY(0);
    setDraggedServerHeight(blockRect?.height ?? 0);
    setDraggedServerIndex(index);
    window.getSelection()?.removeAllRanges();
    updateServerDropTarget(null);
  }, [serverUiIds, servers.length, updateServerDropTarget]);

  const updateDraggedServerPreview = useCallback((clientY: number, sourceIndex: number) => {
    setDraggedServerOffsetY(clientY - dragStartClientYRef.current);
    updateServerDropTarget(findServerDropTarget(clientY, sourceIndex));
  }, [findServerDropTarget, updateServerDropTarget]);

  const getServerBlockStyle = useCallback((index: number) => {
    if (draggedServerIndex === null) {
      return undefined;
    }

    if (index === draggedServerIndex) {
      return {
        transform: `translateY(${Math.round(draggedServerOffsetY)}px)`,
        transition: "none",
        zIndex: 4,
        pointerEvents: "none" as const,
      };
    }

    let translateY = 0;
    if (previewDraggedServerIndex > draggedServerIndex && index > draggedServerIndex && index <= previewDraggedServerIndex) {
      translateY = -serverShiftDistance;
    } else if (previewDraggedServerIndex >= 0 && previewDraggedServerIndex < draggedServerIndex && index >= previewDraggedServerIndex && index < draggedServerIndex) {
      translateY = serverShiftDistance;
    }

    return {
      transform: translateY === 0 ? undefined : `translateY(${Math.round(translateY)}px)`,
      transition: "transform 220ms cubic-bezier(0.22, 1, 0.36, 1), box-shadow 180ms ease, border-color 180ms ease, opacity 180ms ease",
      zIndex: 1,
    };
  }, [draggedServerIndex, draggedServerOffsetY, previewDraggedServerIndex, serverShiftDistance]);

  useEffect(() => {
    if (draggedServerIndex === null) {
      return;
    }

    const previousUserSelect = document.body.style.userSelect;
    const previousCursor = document.body.style.cursor;
    document.body.style.userSelect = "none";
    document.body.style.cursor = "grabbing";

    const finishPointerDrag = (clientY: number) => {
      if (dragFrameRef.current !== null) {
        window.cancelAnimationFrame(dragFrameRef.current);
        dragFrameRef.current = null;
      }
      updateDraggedServerPreview(clientY, draggedServerIndex);
      const target = findServerDropTarget(clientY, draggedServerIndex) ?? serverDropTargetRef.current;
      if (target) {
        const nextServers = moveArrayItem(servers, draggedServerIndex, target.index, target.position);
        const nextServerUiIds = moveArrayItem(serverUiIds, draggedServerIndex, target.index, target.position);
        setServers(nextServers);
        setServerUiIds(nextServerUiIds);
        setJsonText(buildServersJson(nextServers));
        setServerTestStates((prev) => stripIndexKeyedEntries(prev));
        setServerAuthStates((prev) => stripIndexKeyedEntries(prev));
      }
      resetServerDragState();
    };

    const handlePointerMove = (event: PointerEvent) => {
      if (dragPointerIdRef.current !== null && event.pointerId !== dragPointerIdRef.current) {
        return;
      }
      latestPointerClientYRef.current = event.clientY;
      if (dragFrameRef.current !== null) {
        return;
      }
      dragFrameRef.current = window.requestAnimationFrame(() => {
        dragFrameRef.current = null;
        const pointerY = latestPointerClientYRef.current;
        if (pointerY === null) {
          return;
        }
        updateDraggedServerPreview(pointerY, draggedServerIndex);
      });
    };

    const handlePointerUp = (event: PointerEvent) => {
      if (dragPointerIdRef.current !== null && event.pointerId !== dragPointerIdRef.current) {
        return;
      }
      finishPointerDrag(event.clientY);
    };

    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", handlePointerUp);
    window.addEventListener("pointercancel", handlePointerUp);

    return () => {
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("pointerup", handlePointerUp);
      window.removeEventListener("pointercancel", handlePointerUp);
      document.body.style.userSelect = previousUserSelect;
      document.body.style.cursor = previousCursor;
    };
  }, [draggedServerIndex, findServerDropTarget, resetServerDragState, serverUiIds, servers, updateDraggedServerPreview]);

  const baseUrl = listen.startsWith("http") ? listen : `http://${listen}`;
  const builtinSkillHttpUrl = `${baseUrl}${httpPath}/${BUILTIN_SKILL_SERVER_NAME}`;
  const builtinSkillSseUrl = `${baseUrl}${ssePath}/${BUILTIN_SKILL_SERVER_NAME}`;
  const hasValidWhitelistDir = skills.policy.pathGuard.whitelistDirs.some((d) => d.trim().length > 0);

  const runtimeLoading = !localRuntimeSummary && !localRuntimeDetectFailed;
  const systemDisplayValue = localRuntimeSummary?.system
    ? `${localRuntimeSummary.system.os} / ${localRuntimeSummary.system.arch}`
    : runtimeLoading
      ? t("runtimeChecking")
      : t("runtimeUnavailable");
  const runtimeCards = useMemo(() => ([
    {
      key: "system",
      label: t("runtimeSystem"),
      value: systemDisplayValue,
    },
    {
      key: "python",
      label: t("runtimePython"),
      value: runtimeDisplayValue(localRuntimeSummary?.python, runtimeLoading, localRuntimeDetectFailed, t),
    },
    {
      key: "node",
      label: t("runtimeNode"),
      value: runtimeDisplayValue(localRuntimeSummary?.node, runtimeLoading, localRuntimeDetectFailed, t),
    },
    {
      key: "uv",
      label: t("runtimeUv"),
      value: runtimeDisplayValue(localRuntimeSummary?.uv, runtimeLoading, localRuntimeDetectFailed, t),
    },
    {
      key: "terminal",
      label: t("runtimeTerminal"),
      value: terminalEncodingDisplayValue(
        localRuntimeSummary?.terminal,
        runtimeLoading,
        localRuntimeDetectFailed,
        t,
      ),
    },
  ]), [
    localRuntimeDetectFailed,
    localRuntimeSummary,
    runtimeLoading,
    systemDisplayValue,
    t,
  ]);

  // OfficeCLI detection — check the configured path (= input box value)
  useEffect(() => {
    if (skipNextOfficeCliCheckRef.current) {
      skipNextOfficeCliCheckRef.current = false;
      return;
    }
    const pathToCheck = skills.builtinTools.officeCliPath?.trim() || undefined;
    if (!pathToCheck) return;
    let cancelled = false;
    async function check() {
      setOfficeCliState((prev) => prev === null ? { installed: false, checking: true } : prev);
      try {
        const result = await checkOfficeCli(pathToCheck);
        if (cancelled) return;
        if (result.installed) {
          setOfficeCliState({
            installed: true,
            version: result.version ?? undefined,
            path: result.path ?? undefined,
          });
        } else {
          setOfficeCliState({ installed: false });
        }
      } catch {
        if (!cancelled) setOfficeCliState({ installed: false });
      }
    }
    check();
    return () => { cancelled = true; };
  }, [skills.builtinTools.officeCliPath]);

  const handleReinstallOfficeCli = async () => {
    setShowReinstallConfirm(false);
    await handleInstallOfficeCli();
  };

  const handleInstallOfficeCli = async () => {
    if (officeCliState?.installing) return; // 防并发
    setOfficeCliState({ installed: false, installing: true, error: undefined });
    setInstallProgress(0);

    // 订阅真实下载进度
    const unlisten = await listenOfficeCliProgress((p) => {
      setInstallProgress(p.percent);
    });

    let res: OfficeCliInstallResult;
    try {
      res = await installOfficeCli();
    } catch (e) {
      unlisten();
      setOfficeCliState({
        installed: false,
        installing: false,
        error: String(e),
        releaseUrl: "https://github.com/iOfficeAI/OfficeCLI/releases/latest",
      });
      return;
    }
    unlisten();

    if (!res.ok) {
      setOfficeCliState({
        installed: false,
        installing: false,
        error: res.error || t("builtInOfficeCliFailed"),
        releaseUrl: res.releaseUrl,
      });
      setShowOfficeCliPath(true); // 自动展开手动路径输入，方便用户手动装完后填
      return;
    }

    setInstallProgress(100);
    // 再跑一次 check 拿权威版本号；同时把真实路径回写 config
    const verify = await checkOfficeCli(res.installedPath ?? undefined);
    if (verify.installed) {
      const resolved = verify.path ?? res.installedPath ?? skills.builtinTools.officeCliPath ?? "";
      setOfficeCliState({
        installed: true,
        version: verify.version ?? res.version ?? undefined,
        path: resolved,
      });
      skipNextOfficeCliCheckRef.current = true;
      setOfficeCliCustomPath(resolved);
      setSkills((prev) => ({ ...prev, builtinTools: { ...prev.builtinTools, officeCliPath: resolved } }));
      try {
        const cfg = await loadLocalConfig();
        if (cfg.skills?.builtinTools) { cfg.skills.builtinTools.officeCliPath = resolved; }
        await saveLocalConfig(cfg);
      } catch { /* best-effort */ }
    } else {
      setOfficeCliState({
        installed: false,
        installing: false,
        error: t("builtInOfficeCliFailed"),
        releaseUrl: res.releaseUrl,
      });
      setShowOfficeCliPath(true);
    }
  };

  const handleVerifyOfficeCliPath = async () => {
    const p = officeCliCustomPath.trim();
    if (!p) return;
    // 点勾即落盘，不管检测成功与否
    skipNextOfficeCliCheckRef.current = true;
    setSkills((prev) => ({ ...prev, builtinTools: { ...prev.builtinTools, officeCliPath: p } }));
    try {
      const cfg = await loadLocalConfig();
      if (cfg.skills?.builtinTools) { cfg.skills.builtinTools.officeCliPath = p; }
      await saveLocalConfig(cfg);
    } catch { /* best-effort */ }
    // 然后执行检测
    setOfficeCliState(prev => ({ ...(prev || { installed: false }), installing: true, error: undefined }));
    try {
      const result = await checkOfficeCli(p);
      if (result.installed) {
        const resolved = result.path ?? p;
        if (resolved !== p) {
          setOfficeCliCustomPath(resolved);
          setSkills((prev) => ({ ...prev, builtinTools: { ...prev.builtinTools, officeCliPath: resolved } }));
          try {
            const cfg2 = await loadLocalConfig();
            if (cfg2.skills?.builtinTools) { cfg2.skills.builtinTools.officeCliPath = resolved; }
            await saveLocalConfig(cfg2);
          } catch { /* best-effort */ }
        }
        setOfficeCliState({
          installed: true,
          version: result.version ?? undefined,
          error: undefined,
          path: resolved,
        });
      } else {
        setOfficeCliState({
          installed: false,
          error: result.error || t("builtInOfficeCliNotInstalled"),
        });
      }
    } catch (e) {
      setOfficeCliState({ installed: false, error: String(e) });
    }
  };

  const handleCopy = async (name: string, type: EndpointTransportType, url: string, key: string) => {
    const snippet = createMcpClientEntryJson(name, type, url, mcpToken);
    await navigator.clipboard.writeText(snippet);
    setCopied(key);
    setTimeout(() => setCopied(null), 2000);
  };

  const handleOpenExternalLink = useCallback(async (url: string) => {
    try {
      await open(url);
    } catch (error) {
      setError(String(error));
    }
  }, []);

  const handleOpenLatestRelease = useCallback(async () => {
    if (!updateInfo?.releaseUrl) {
      return;
    }
    await handleOpenExternalLink(updateInfo.releaseUrl);
  }, [handleOpenExternalLink, updateInfo?.releaseUrl]);

  const handleOpenConfigFile = useCallback(async () => {
    try {
      await openConfigFileLocal();
    } catch (error) {
      setError(String(error));
    }
  }, []);

  const requestResetDefaultConfig = useCallback(() => {
    setResetConfigConfirmOpen(true);
  }, []);

  const cancelResetDefaultConfig = useCallback(() => {
    setResetConfigConfirmOpen(false);
  }, []);

  const confirmResetDefaultConfig = useCallback(async () => {
    setResetConfigConfirmOpen(false);
    setError(null);
    setSaving(true);
    setSaveSuccess(false);
    try {
      const nextPath = await resetDefaultConfigLocal();
      setConfigPath(nextPath);
      await reloadLocalConfig();
      setSaveSuccess(true);
      setTimeout(() => setSaveSuccess(false), 2000);
    } catch (error) {
      setError(String(error));
    } finally {
      setSaving(false);
    }
  }, [reloadLocalConfig]);

  const handleOpenQqGroup = useCallback(async () => {
    const inviteUrl = QQ_GROUP_INVITE_URL.trim();
    if (inviteUrl.length > 0) {
      try {
        await open(inviteUrl);
      } catch (error) {
        setError(String(error));
      }
      return;
    }

    for (const link of QQ_GROUP_PROTOCOL_LINKS) {
      try {
        await open(link);
        return;
      } catch {
        // keep trying next known protocol.
      }
    }
    setError(t("qqGroupFallbackHint").replace("{group}", QQ_GROUP_NUMBER));
  }, [t]);
  const versionChipTitle = hasAvailableUpdate && updateInfo?.latestVersion
    ? `${t("versionLabel")} v${CURRENT_VERSION} · ${t("updateAvailable")} ${updateInfo.latestVersion}`
    : `${t("versionLabel")} v${CURRENT_VERSION}`;

  return (
    <div className="app-root">

      {/* ── 版本更新 Banner ── */}
      {updateInfo && updateInfo.hasUpdate && !updateDismissed && (
        <UpdateBanner
          info={updateInfo}
          lang={lang}
          onDismiss={() => {
            if (updateInfo.latestVersion) {
              setDismissedVersion(updateInfo.latestVersion);
              localStorage.setItem("mcp-update-dismissed", updateInfo.latestVersion);
            }
          }}
        />
      )}

      <div className="app-shell">
        <aside className="sidebar" aria-label="Primary">
          <div className="sidebar-brand">
            <div className="brand-mark" aria-hidden>M</div>
            <div className="brand-copy">
              <div className="brand-title">{t("appTitle")}</div>
            </div>
          </div>

          <nav className="sidebar-nav">
            <button
              type="button"
              className={`sidebar-nav-item ${activeSection === "info" ? "active" : ""}`}
              onClick={() => setActiveSection("info")}
            >
              <Info size={16} />
              <span>{t("navBasicInfo")}</span>
            </button>
            <button
              type="button"
              className={`sidebar-nav-item ${activeSection === "settings" ? "active" : ""}`}
              onClick={() => setActiveSection("settings")}
            >
              <SettingsIcon size={16} />
              <span>{t("navSettings")}</span>
              {skillPending.length > 0 && <span className="sidebar-badge">{skillPending.length}</span>}
            </button>
            <button
              type="button"
              className={`sidebar-nav-item ${activeSection === "builtin" ? "active" : ""}`}
              onClick={() => setActiveSection("builtin")}
            >
              <Wrench size={16} />
              <span>{t("navBuiltinTools")}</span>
            </button>
            <button
              type="button"
              className={`sidebar-nav-item ${activeSection === "externalMcp" ? "active" : ""}`}
              onClick={() => setActiveSection("externalMcp")}
            >
              <Plug size={16} />
              <span>{t("navExternalMcp")}</span>
            </button>
            <button
              type="button"
              className={`sidebar-nav-item ${activeSection === "externalSkill" ? "active" : ""}`}
              onClick={() => setActiveSection("externalSkill")}
            >
              <BookOpenText size={16} />
              <span>{t("navExternalSkill")}</span>
            </button>
          </nav>

          <div className="sidebar-status">
            <span className={`status-dot ${running ? "running" : "stopped"}`} />
            <span>{running ? t("running") : t("stopped")}</span>
          </div>
        </aside>

        <div className="workspace">
          {/* ── 顶栏 ── */}
          <div className="topbar">
            <div className="topbar-right">
              {showRestartRequiredHint && (
                <span
                  className="topbar-restart-hint"
                  role="status"
                  aria-live="polite"
                  title={t("restartRequiredHint")}
                >
                  {t("restartRequiredHint")}
                </span>
              )}
              <button
                className={`topbar-icon-btn ${isConfigDirty ? "btn-save-dirty" : ""}`}
                onClick={handleSave}
                disabled={saving || !configLoaded}
                title={saving ? t("saving") : saveSuccess ? t("saveSuccess") : isConfigDirty ? t("saveConfigUnsaved") : t("saveConfig")}
                aria-label={saving ? t("saving") : saveSuccess ? t("saveSuccess") : isConfigDirty ? t("saveConfigUnsaved") : t("saveConfig")}
              >
                <Save size={15} />
                {isConfigDirty && !saving && (
                  <span
                    className="save-dirty-indicator"
                    aria-label={t("saveConfigUnsaved")}
                    title={t("saveConfigUnsaved")}
                  >
                    !
                  </span>
                )}
              </button>
              <button className="topbar-icon-btn" onClick={toggleLang} title={t("langToggle")} aria-label={t("langToggle")}>
                <Languages size={15} />
              </button>
              {!running ? (
                <button className="topbar-icon-btn topbar-run-btn" onClick={handleStart} disabled={busy || !configLoaded} title={busy ? t("starting") : t("start")} aria-label={busy ? t("starting") : t("start")}>
                  <Play size={15} />
                </button>
              ) : (
                <button className="topbar-icon-btn topbar-stop-btn" onClick={handleStop} disabled={busy} title={busy ? t("stopping") : t("stop")} aria-label={busy ? t("stopping") : t("stop")}>
                  <Square size={15} />
                </button>
              )}
            </div>
          </div>

          {/* ── 错误提示 ── */}
          {error && (
            <div className="alert alert-error app-alert">
              {error}
              <button className="alert-close" onClick={() => setError(null)}>x</button>
            </div>
          )}

          <div className="main-scroll">

        {activeSection === "info" && (
          <>
            <section className="config-section info-section">
              <div className="section-heading">{t("softwareIntroTitle")}</div>
              <p className="software-intro-text">{t("softwareIntroBody")}</p>
            </section>

            <section className="config-section info-section">
              <div className="section-heading">{t("runtimeEnvironment")}</div>
              <div className="runtime-line-list" role="status" aria-live="polite">
                {runtimeCards.map((item) => (
                  <div className="runtime-line-item" key={item.key}>
                    <span className="runtime-line-label">{item.label}</span>
                    <span className="runtime-line-value">{item.value}</span>
                  </div>
                ))}
              </div>
            </section>

            <section className="config-section info-section">
              <div className="section-heading">{t("filePaths")}</div>
              <div className="runtime-line-list">
                <div className="runtime-line-item runtime-line-item-with-action">
                  <span className="runtime-line-label">{t("configPath")}</span>
                  <code className="runtime-line-value runtime-path-value">
                    {configPath || t("runtimeChecking")}
                  </code>
                  <button
                    type="button"
                    className="runtime-line-action"
                    onClick={() => { void handleOpenConfigFile(); }}
                    disabled={!configPath}
                    title={t("openConfigFile")}
                    aria-label={t("openConfigFile")}
                  >
                    <Pencil size={14} />
                  </button>
                  <button
                    type="button"
                    className="runtime-line-action runtime-line-danger-action"
                    onClick={requestResetDefaultConfig}
                    disabled={!configLoaded || saving}
                    title={t("resetDefaultConfig")}
                    aria-label={t("resetDefaultConfig")}
                  >
                    <RotateCcw size={14} />
                  </button>
                </div>
                <div className="runtime-line-item">
                  <span className="runtime-line-label">{t("logPath")}</span>
                  <span className="runtime-line-value runtime-muted-value">{t("logPathPlaceholder")}</span>
                </div>
              </div>
            </section>
          </>
        )}

        {activeSection === "settings" && (
          <>
            {/* ── 网关设置 ── */}
            <section className="config-section">
              <div className="section-heading">{t("gatewaySettings")}</div>
              <div className="gateway-fields">
                <div className="gw-field">
                  <label className="field-label">{t("listenAddress")}</label>
                  <input className="form-input" placeholder="127.0.0.1:8765"
                    value={listen} onChange={(e) => setListen(e.target.value)} />
                </div>
                <div className="gw-field">
                  <label className="field-label">{t("ssePath")}</label>
                  <input className="form-input" placeholder="/api/v2/sse"
                    value={ssePath} onChange={(e) => setSsePath(e.target.value)} />
                </div>
                <div className="gw-field">
                  <label className="field-label">{t("httpStreamPath")}</label>
                  <input className="form-input" placeholder="/api/v2/mcp"
                    value={httpPath} onChange={(e) => setHttpPath(e.target.value)} />
                </div>
              </div>
            </section>

            {/* ── 安全配置 ── */}
            <section className="config-section">
              <div className="section-heading">{t("securityConfig")}</div>
              <div className="security-fields">
                <div className="gw-field">
                  <label className="field-label">
                    {t("adminToken")}
                    <span className="field-label-hint"> ({t("authHeaderHint")}，{t("adminTokenUsageHint")})</span>
                  </label>
                  <input className="form-input" type="password" placeholder={t("tokenPlaceholder")}
                    value={adminToken} onChange={(e) => setAdminToken(e.target.value)} />
                </div>
                <div className="gw-field">
                  <label className="field-label">
                    {t("mcpToken")}
                    <span className="field-label-hint"> ({t("authHeaderHint")}，{t("mcpTokenUsageHint")})</span>
                  </label>
                  <input className="form-input" type="password" placeholder={t("tokenPlaceholder")}
                    value={mcpToken} onChange={(e) => setMcpToken(e.target.value)} />
                </div>
              </div>
            </section>
          </>
        )}

        {activeSection === "externalMcp" && (
          <>
            <section className={`config-section ${serversMode === "json" ? "mcp-json-section" : ""}`}>
              <div className="section-heading-row mcp-servers-heading-row">
                <div className="section-heading-block">
                  <div className="section-heading">{t("mcpServers")}</div>
                  <div className="section-description">{t("mcpServersHint")}</div>
                </div>
                <div className="section-heading-actions">
                  <div className="mode-toggle">
                    <button className={`mode-btn ${serversMode === "visual" ? "active" : ""}`}
                      onClick={switchToVisual} title={t("visual")}>
                      <List size={13} /> {t("visual")}
                    </button>
                    <button className={`mode-btn ${serversMode === "json" ? "active" : ""}`}
                      onClick={switchToJson} title={t("json")}>
                      <Code2 size={13} /> {t("json")}
                    </button>
                  </div>
                  {serversMode === "visual" && (
                    <button className="btn btn-secondary btn-sm btn-add-server-heading" onClick={addServer}>
                      {t("addServer")}
                    </button>
                  )}
                  {serversMode === "json" && (
                    <button className="btn btn-secondary btn-sm btn-add-server-heading" onClick={formatServersJson}>
                      <AlignLeft size={13} /> {t("formatJson")}
                    </button>
                  )}
                </div>
              </div>

              {jsonError && (
                <div className="alert alert-error" style={{ marginBottom: 10 }}>{jsonError}</div>
              )}
              {autoTestingServers && (
                <div className="json-hint" style={{ marginBottom: 10 }}>{t("autoTestingServers")}</div>
              )}

              {serversMode === "visual" ? (
                <>
                  {servers.length === 0 ? (
                    <div className="empty-hint">{t("noServers")}</div>
                  ) : (
                    <div className="servers-list">
                      {servers.map((s, i) => (
                        <div
                          className={[
                            "server-block",
                            draggedServerIndex === i ? "server-block-dragging" : "",
                            serverDropTarget?.index === i ? `server-block-drop-${serverDropTarget.position}` : "",
                          ].filter(Boolean).join(" ")}
                          key={serverUiIds[i] ?? `server-${i}`}
                          style={getServerBlockStyle(i)}
                          ref={(node) => {
                            const key = serverUiIds[i] ?? `server-${i}`;
                            serverBlockRefs.current[key] = node;
                          }}
                        >
                          <div
                            className={`server-row-header ${servers.length > 1 ? "server-row-header-draggable" : ""}`}
                            onPointerDown={(event) => handleServerPointerDown(event, i)}
                            title={servers.length > 1 ? t("dragServerReorder") : undefined}
                            aria-grabbed={draggedServerIndex === i}
                          >
                            <span className="col-toggle" />
                            <span>{t("name")}</span>
                            <span>{t("command")}</span>
                            <span>{t("args")}</span>
                            <span />
                          </div>
                          <ServerRow server={s}
                            running={running}
                            baseUrl={baseUrl}
                            ssePath={ssePath}
                            httpPath={httpPath}
                            copied={copied}
                            onCopy={handleCopy}
                            testState={serverTestStates[serverTestKey(i, s)] ?? { status: "idle", message: "" }}
                            authState={serverAuthStates[serverTestKey(i, s)] ?? createEmptyAuthState()}
                            onTest={() => { void runServerConnectivityTest(s, i); }}
                            onReauthorize={() => { void runServerReauthorize(s, i); }}
                            onClearAuth={() => { void runServerClearAuth(s, i); }}
                            t={t}
                            onChange={(u) => {
                              setServers((prev) => prev.map((x, xi) => xi === i ? u : x));
                              setServerTestStates((prev) => {
                                const next = { ...prev };
                                delete next[serverTestKey(i, s)];
                                delete next[serverTestKey(i, u)];
                                return next;
                              });
                              setServerAuthStates((prev) => {
                                const next = { ...prev };
                                delete next[serverTestKey(i, s)];
                                delete next[serverTestKey(i, u)];
                                return next;
                              });
                            }}
                            onDelete={() => requestDelete(i, s.name)}
                          />
                        </div>
                      ))}
                    </div>
                  )}
                </>
              ) : (
                <div className="json-editor-wrap">
                  <div className="json-hint">{t("jsonHint")}</div>
                  <JsonEditor
                    value={jsonText}
                    onChange={(v) => { setJsonText(v); setJsonError(null); }}
                    placeholder={t("jsonHint")}
                  />
                </div>
              )}
            </section>
          </>
        )}

        {activeSection === "builtin" && (
          <>
            <section className="config-section skills-redesign-section">
              <div className="section-heading-row built-in-tools-heading-row">
                <div className="section-heading-block">
                  <div className="section-heading">{t("builtInToolsTitle")}</div>
                  <div className="section-description">{t("builtInToolsHint")}</div>
                </div>
              </div>

              {running && (
                <div className="skills-endpoints">
                  <div className="endpoint-item">
                    <span className="endpoint-label">{t("builtinSkillSseEndpoint")}</span>
                    <code className="endpoint-url">{builtinSkillSseUrl}</code>
                    <button className="btn-icon" title={t("copyBuiltinSkillSse")} onClick={() => handleCopy(BUILTIN_SKILL_SERVER_NAME, "sse", builtinSkillSseUrl, "builtin-skills-sse")}>
                      {copied === "builtin-skills-sse" ? <Check size={12} color="var(--accent-green)" /> : <Copy size={12} />}
                    </button>
                  </div>
                  <div className="endpoint-item">
                    <span className="endpoint-label">{t("builtinSkillHttpEndpoint")}</span>
                    <code className="endpoint-url">{builtinSkillHttpUrl}</code>
                    <button className="btn-icon" title={t("copyBuiltinSkillHttp")} onClick={() => handleCopy(BUILTIN_SKILL_SERVER_NAME, "streamable-http", builtinSkillHttpUrl, "builtin-skills-http")}>
                      {copied === "builtin-skills-http" ? <Check size={12} color="var(--accent-green)" /> : <Copy size={12} />}
                    </button>
                  </div>
                </div>
              )}

              <div className="skills-redesign">
                <div className="built-in-tools-panel">
                  <div className="built-in-tools-grid">
                    {builtinToolDefs.map((tool) => {
                      const isGated = tool.requiresWhitelist && !hasValidWhitelistDir;
                      const isOn = !isGated && skills.builtinTools[tool.key] === true;
                      const shellEnvCount = Object.keys(skills.builtinTools.shellEnv ?? {}).length;
                      return (
                        <div className="built-in-tool" key={tool.key}>
                          <button
                            className={`toggle-btn ${isOn ? "toggle-on" : "toggle-off"}`}
                            onClick={() => { if (isGated) return; setSkills((prev) => ({ ...prev, builtinTools: { ...prev.builtinTools, [tool.key]: !prev.builtinTools[tool.key] } })); }}
                            disabled={isGated}
                            title={isGated ? t("skillsWhitelistHint") : isOn ? t("enabledClick") : t("disabledClick")}
                            aria-label={`${isOn ? t("enabledClick") : t("disabledClick")} ${tool.name}`}
                            aria-pressed={isOn}
                          />
                          <div className="built-in-tool-icon">{tool.icon}</div>
                          <div className="built-in-tool-body">
                            <div className="built-in-tool-name">
                              {tool.name}
                              {tool.key === "taskPlanning" && (
                                <span className="shell-env-wrap">
                                  <button
                                    className={`btn-icon shell-env-btn${showPlansModal ? " active" : ""}`}
                                    title={t("planViewBtn")}
                                    onClick={handleOpenPlans}
                                  >
                                    <Eye size={13} />
                                  </button>
                                </span>
                              )}
                              {tool.key === "shellCommand" && (
                                <span className="shell-env-wrap">
                                  <button
                                    className={`btn-icon shell-env-btn${showShellEnvPopover ? " active" : ""}`}
                                    title={t("shellEnvBtn")}
                                    onClick={() => setShowShellEnvPopover(!showShellEnvPopover)}
                                  >
                                    <Code2 size={13} />
                                  </button>
                                  {shellEnvCount > 0 && !showShellEnvPopover && (
                                    <span className="shell-env-badge">{t("shellEnvCount").replace("{count}", String(shellEnvCount))}</span>
                                  )}
                                </span>
                              )}
                            </div>
                            {tool.key === "shellCommand" && showShellEnvPopover && (
                              <div className="shell-env-popover">
                                <div className="shell-env-header">
                                  <span className="shell-env-title">{t("shellEnvTitle")}</span>
                                </div>
                                <div className="shell-env-hint">{t("shellEnvHint")}</div>
                                <div className="shell-env-list">
                                  {Object.entries(skills.builtinTools.shellEnv ?? {}).map(([key, value]) => (
                                    <div className="shell-env-row" key={key}>
                                      <code className="shell-env-key">{key}</code>
                                      <input
                                        className="shell-env-value-input"
                                        type="text"
                                        value={value}
                                        onChange={(e) => {
                                          const next = { ...(skills.builtinTools.shellEnv ?? {}), [key]: e.target.value };
                                          setSkills((prev) => ({ ...prev, builtinTools: { ...prev.builtinTools, shellEnv: next } }));
                                        }}
                                      />
                                      <button
                                        className="btn-icon shell-env-delete-btn"
                                        title="Delete"
                                        onClick={() => {
                                          const next = { ...(skills.builtinTools.shellEnv ?? {}) };
                                          delete next[key];
                                          setSkills((prev) => ({ ...prev, builtinTools: { ...prev.builtinTools, shellEnv: next } }));
                                        }}
                                      >
                                        <Trash2 size={12} />
                                      </button>
                                    </div>
                                  ))}
                                  {shellEnvCount === 0 && (
                                    <div className="shell-env-empty">{t("shellEnvEmpty")}</div>
                                  )}
                                </div>
                                <div className="shell-env-add-row">
                                  <input
                                    className="shell-env-add-input shell-env-add-key"
                                    type="text"
                                    placeholder={t("shellEnvKeyPlaceholder")}
                                    value={shellEnvDraftKey}
                                    onChange={(e) => setShellEnvDraftKey(e.target.value)}
                                    onKeyDown={(e) => {
                                      if (e.key === "Enter" && shellEnvDraftKey.trim()) {
                                        const next = { ...(skills.builtinTools.shellEnv ?? {}), [shellEnvDraftKey.trim()]: shellEnvDraftValue };
                                        setSkills((prev) => ({ ...prev, builtinTools: { ...prev.builtinTools, shellEnv: next } }));
                                        setShellEnvDraftKey(""); setShellEnvDraftValue("");
                                      }
                                    }}
                                  />
                                  <input
                                    className="shell-env-add-input shell-env-add-value"
                                    type="text"
                                    placeholder={t("shellEnvValuePlaceholder")}
                                    value={shellEnvDraftValue}
                                    onChange={(e) => setShellEnvDraftValue(e.target.value)}
                                    onKeyDown={(e) => {
                                      if (e.key === "Enter" && shellEnvDraftKey.trim()) {
                                        const next = { ...(skills.builtinTools.shellEnv ?? {}), [shellEnvDraftKey.trim()]: shellEnvDraftValue };
                                        setSkills((prev) => ({ ...prev, builtinTools: { ...prev.builtinTools, shellEnv: next } }));
                                        setShellEnvDraftKey(""); setShellEnvDraftValue("");
                                      }
                                    }}
                                  />
                                  <button
                                    className="btn-icon shell-env-add-btn"
                                    title={t("shellEnvAdd")}
                                    disabled={!shellEnvDraftKey.trim()}
                                    onClick={() => {
                                      if (!shellEnvDraftKey.trim()) return;
                                      const next = { ...(skills.builtinTools.shellEnv ?? {}), [shellEnvDraftKey.trim()]: shellEnvDraftValue };
                                      setSkills((prev) => ({ ...prev, builtinTools: { ...prev.builtinTools, shellEnv: next } }));
                                      setShellEnvDraftKey(""); setShellEnvDraftValue("");
                                    }}
                                  >
                                    <Plus size={13} />
                                  </button>
                                </div>
                              </div>
                            )}
                            <div className="built-in-tool-desc">{t(tool.descKey)}</div>
                          </div>
                        </div>
                      );
                    })}
                    <div className={`built-in-tool officecli-tool${!officeCliState ? " officecli-checking" : ""}${officeCliState?.installed ? " officecli-ready" : ""}`}>
                      <button
                        className={`toggle-btn ${(!hasValidWhitelistDir || !officeCliState?.installed) ? "toggle-off" : skills.builtinTools.officeCli ? "toggle-on" : "toggle-off"}`}
                        onClick={() => {
                          if (!hasValidWhitelistDir || !officeCliState?.installed) return;
                          setSkills((prev) => ({ ...prev, builtinTools: { ...prev.builtinTools, officeCli: !prev.builtinTools.officeCli } }))
                        }}
                        title={!hasValidWhitelistDir ? t("skillsWhitelistHint") : officeCliState?.installed ? (skills.builtinTools.officeCli ? t("enabledClick") : t("disabledClick")) : t("builtInOfficeCliNotInstalled")}
                        aria-label={`${skills.builtinTools.officeCli ? t("enabledClick") : t("disabledClick")} officecli`}
                        aria-pressed={hasValidWhitelistDir && skills.builtinTools.officeCli}
                        disabled={!hasValidWhitelistDir || !officeCliState?.installed}
                      />
                      <div className="built-in-tool-icon"><OfficeLogoIcon size={15} /></div>
                      <div className="built-in-tool-body">
                        <div className="built-in-tool-name">
                          officecli
                          {officeCliState?.installed && (
                            <span className="officecli-badge ok" title={officeCliState.path}>
                              <span className="officecli-dot" />
                              {officeCliState.version && <>v{officeCliState.version}</>}
                            </span>
                          )}
                          {officeCliState?.installed && (
                            <span className="officecli-reinstall-wrap">
                              <button className="btn-icon officecli-reinstall-btn" title={t("builtInOfficeCliReinstall")} onClick={() => setShowReinstallConfirm(!showReinstallConfirm)}>
                                <RotateCcw size={13} />
                              </button>
                              <button
                                className={`btn-icon officecli-reinstall-btn${showOfficeCliPath ? " active" : ""}`}
                                title={t("builtInOfficeCliManualPath")}
                                onClick={() => setShowOfficeCliPath(!showOfficeCliPath)}
                              >
                                <FolderOpen size={13} />
                              </button>
                              {showReinstallConfirm && (
                                <span className="officecli-reinstall-popover">
                                  <span className="officecli-reinstall-popover-text">{t("builtInOfficeCliReinstallConfirm")}</span>
                                  <span className="officecli-reinstall-popover-actions">
                                    <button className="btn btn-secondary btn-sm" onClick={() => setShowReinstallConfirm(false)}>{t("cancel")}</button>
                                    <button className="btn-primary-sm" onClick={handleReinstallOfficeCli}>{t("confirm")}</button>
                                  </span>
                                </span>
                              )}
                            </span>
                          )}
                          {officeCliState && !officeCliState.installed && !officeCliState.installing && !officeCliState.checking && (
                            <span className="officecli-badge warn">
                              {t("builtInOfficeCliNotInstalled")}
                              <button className="btn-icon officecli-action-btn" title={t("builtInOfficeCliInstallBtn")} onClick={handleInstallOfficeCli} disabled={!!officeCliState?.installing}>
                                <Download size={13} />
                              </button>
                              <button
                                className={`btn-icon officecli-action-btn${showOfficeCliPath ? " active" : ""}`}
                                title={t("builtInOfficeCliManualPath")}
                                onClick={() => setShowOfficeCliPath(!showOfficeCliPath)}
                              >
                                <FolderOpen size={13} />
                              </button>
                            </span>
                          )}
                          {(!officeCliState || officeCliState?.checking) && (
                            <span className="officecli-badge muted">
                              {t("builtInOfficeCliChecking")}
                            </span>
                          )}
                          {officeCliState?.installing && (
                            <span className="officecli-badge muted">
                              {t("builtInOfficeCliInstalling")}
                              {installProgress > 0 && <> · {installProgress}%</>}
                            </span>
                          )}
                        </div>
                        <div className="built-in-tool-desc">{t("builtInOfficeCliDesc")}</div>
                        {officeCliState && !officeCliState.installing && showOfficeCliPath && (
                          <div className="officecli-path-row">
                            <Terminal size={13} className="officecli-path-icon" />
                            <input
                              className="officecli-path-input"
                              type="text"
                              placeholder={t("builtInOfficeCliPathPlaceholder")}
                              value={officeCliCustomPath}
                              onChange={(e) => setOfficeCliCustomPath(e.target.value)}
                              onKeyDown={(e) => { if (e.key === "Enter") handleVerifyOfficeCliPath(); }}
                            />
                            <button className="btn-icon officecli-action-btn" title={t("builtInOfficeCliVerifyBtn")} onClick={handleVerifyOfficeCliPath}>
                              <Check size={14} />
                            </button>
                          </div>
                        )}
                        {officeCliState?.error && (
                          <div className="officecli-error">
                            {renderTextWithLinks(officeCliState.error, (url) => { void handleOpenExternalLink(url); })}
                            {officeCliState.releaseUrl && (
                              <>
                                {" "}
                                <a
                                  className="officecli-error-link"
                                  href="#"
                                  onClick={(e) => { e.preventDefault(); void handleOpenExternalLink(officeCliState.releaseUrl!); }}
                                >
                                  {t("builtInOfficeCliOpenReleases")}
                                </a>
                              </>
                            )}
                          </div>
                        )}
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            </section>
          </>
        )}

        {activeSection === "externalSkill" && (
          <>
            <section className="config-section skills-redesign-section">
              <div className="section-heading-row skill-roots-heading-row">
                <div className="section-heading-block">
                  <div className="section-heading">{t("skillsConfig")}</div>
                  <div className="section-description">{t("skillsRootsHint")}</div>
                </div>
                <div className="section-heading-actions">
                  <button className="btn btn-secondary btn-sm btn-add-server-heading" onClick={addSkillGroup}>
                    {t("skillGroupAdd")}
                  </button>
                </div>
              </div>

              <div className="skills-redesign">
                <SkillGroupsEditor
                  groups={skillGroups}
                  onRemoveGroup={removeSkillGroup}
                  onRenameGroup={renameSkillGroup}
                  onAddItem={addGroupItem}
                  onRemoveItem={removeGroupItem}
                  onPathChange={updateGroupItemPath}
                  onValidate={validateGroupItem}
                  onBrowse={browseGroupItem}
                  onToggleEnabled={toggleGroupItemEnabled}
                  onImportToGroup={(gid) => { void importToGroup(gid); }}
                  onCopy={handleCopy}
                  copied={copied}
                  running={running}
                  baseUrl={baseUrl}
                  ssePath={ssePath}
                  httpPath={httpPath}
                  t={t}
                />
              </div>
            </section>
          </>
        )}

        {activeSection === "settings" && (
          <>
            {/* ── 执行配置 ── */}
            <section className="config-section">
              <div className="section-heading">{t("skillsExecution")}</div>
              <div className="skills-execution-config">
                <div className="gw-field">
                  <label className="field-label">{t("skillsExecutionTimeout")}</label>
                  <input
                    type="number"
                    className="form-input"
                    value={skills.execution.timeoutMs}
                    min={1000}
                    onChange={(e) =>
                      {
                        const val = parseInt(e.target.value, 10);
                        if (!isNaN(val) && val >= 1000) {
                          setSkills((prev) => ({
                            ...prev,
                            execution: { ...prev.execution, timeoutMs: val },
                          }));
                        }
                      }
                    }
                    placeholder="60000"
                  />
                  <span className="json-hint">{t("skillsExecutionTimeoutHint")}</span>
                </div>
                <div className="gw-field">
                  <label className="field-label">{t("skillsMaxOutputBytes")}</label>
                  <input
                    type="number"
                    className="form-input"
                    value={skills.execution.maxOutputBytes}
                    min={1024}
                    onChange={(e) =>
                      {
                        const val = parseInt(e.target.value, 10);
                        if (!isNaN(val) && val >= 1024) {
                          setSkills((prev) => ({
                            ...prev,
                            execution: { ...prev.execution, maxOutputBytes: val },
                          }));
                        }
                      }
                    }
                    placeholder="13107200"
                  />
                  <span className="json-hint">{t("skillsMaxOutputBytesHint")}</span>
                </div>
              </div>
            </section>

            <section className="config-section skills-redesign-section">
              <div className="section-heading">{t("skillsPathGuard")}</div>

              <div className="skills-redesign skills-path-guard-panel">
                <div className="skills-violation-panel">
                  <label className="field-label" htmlFor="skills-violation-action">{t("skillsViolationAction")}</label>
                  <select
                    id="skills-violation-action"
                    className="form-input skills-action-select"
                    value={skills.policy.pathGuard.onViolation}
                    onChange={(e) =>
                      setSkills((prev) => ({
                        ...prev,
                        policy: {
                          ...prev.policy,
                          pathGuard: { ...prev.policy.pathGuard, onViolation: e.target.value as SkillPolicyAction },
                        },
                      }))
                    }
                  >
                    <option value="allow">{t("policyAllow")}</option>
                    <option value="confirm">{t("policyConfirm")}</option>
                    <option value="deny">{t("policyDeny")}</option>
                  </select>
                </div>

                <SkillDirectoryListEditor
                  title={t("skillsWhitelistDirs")}
                  hint={t("skillsWhitelistHint")}
                  items={skillWhitelistItems}
                  onAdd={addWhitelistItem}
                  onRemove={requestRemoveWhitelistItem}
                  onPathChange={updateWhitelistItemPath}
                  onBrowse={browseWhitelistItem}
                  showValidation={false}
                  t={t}
                />
              </div>
            </section>

            {/* ── 策略规则 ── */}
            <section className="config-section">
              <div className="section-heading">{t("skillsRules")}</div>
              <SkillPolicyRulesEditor
                rules={skills.policy.rules}
                form={skillRuleForm}
                formOpen={skillRuleFormOpen}
                editingRuleId={editingSkillRuleId}
                advancedOpen={skillsRulesAdvancedOpen}
                jsonDraft={skillsRulesDraft}
                jsonError={skillsRulesError}
                onStartAdd={startAddSkillRule}
                onResetToDefault={requestResetSkillRules}
                onEdit={editSkillRule}
                onCopy={copySkillRule}
                onDelete={deleteSkillRule}
                onCancelForm={cancelSkillRuleForm}
                onSubmitForm={submitSkillRuleForm}
                onFormChange={(patch) => setSkillRuleForm((prev) => ({ ...prev, ...patch }))}
                onToggleAdvanced={() => setSkillsRulesAdvancedOpen((open) => !open)}
                onJsonChange={onRulesDraftChange}
                lang={lang}
                t={t}
              />
            </section>

            {/* ── 待确认命令 ── */}
            <section className="config-section">
              <div className="section-heading">{t("skillsPendingTitle")}</div>
              <SkillConfirmations
                pending={skillPending}
                busyIds={skillActionBusy}
                onApprove={(id) => { handleSkillConfirmationAction(id, "approve"); }}
                onReject={(id) => { handleSkillConfirmationAction(id, "reject"); }}
                lang={lang}
                t={t}
              />
            </section>
          </>
        )}

      </div>

      {/* ── 底部通知条：快捷入口 ── */}
      <div className="bottom-bar">
        <div className="bottom-bar-main" />
        <div className="bottom-bar-links" role="group" aria-label={t("quickLinks")}>
          <button
            type="button"
            className="bottom-link-btn"
            aria-label={t("openBlog")}
            title={t("openBlog")}
            onClick={() => { void handleOpenExternalLink(BLOG_URL); }}
          >
            <Newspaper size={15} />
          </button>
          <button
            type="button"
            className="bottom-link-btn"
            aria-label={t("openBilibiliHome")}
            title={t("openBilibiliHome")}
            onClick={() => { void handleOpenExternalLink(BILIBILI_URL); }}
          >
            <BilibiliLogoIcon size={15} />
          </button>
          <button
            type="button"
            className="bottom-link-btn"
            aria-label={t("openGithub")}
            title={t("openGithub")}
            onClick={() => { void handleOpenExternalLink(GITHUB_URL); }}
          >
            <Github size={15} />
          </button>
          <button
            type="button"
            className="bottom-link-btn"
            aria-label={t("openQqGroup")}
            title={t("openQqGroup")}
            onClick={() => { void handleOpenQqGroup(); }}
          >
            <QqLogoIcon size={14} />
          </button>
          <button
            type="button"
            className="bottom-link-btn"
            aria-label={t("openTelegramGroup")}
            title={t("openTelegramGroup")}
            onClick={() => { void handleOpenExternalLink(TG_GROUP_URL); }}
          >
            <Send size={14} />
          </button>
          {hasAvailableUpdate && updateInfo?.releaseUrl ? (
            <button
              type="button"
              className="bottom-version-chip bottom-version-chip-clickable"
              title={versionChipTitle}
              aria-label={versionChipTitle}
              onClick={() => { void handleOpenLatestRelease(); }}
            >
              <span className="bottom-version-chip-text">v{CURRENT_VERSION}</span>
              <span className="bottom-version-chip-dot" aria-hidden />
            </button>
          ) : (
            <span className="bottom-version-chip" title={versionChipTitle} aria-label={versionChipTitle}>
              <span className="bottom-version-chip-text">v{CURRENT_VERSION}</span>
            </span>
          )}
        </div>
      </div>
        </div>
      </div>

      <SkillConfirmationPopup
        open={!!activeSkillPopupItem}
        item={activeSkillPopupItem}
        busy={!!activeSkillPopupItem && skillActionBusy.has(activeSkillPopupItem.id)}
        onApprove={(id) => handleSkillConfirmationAction(id, "approve")}
        onReject={(id) => handleSkillConfirmationAction(id, "reject")}
        onLater={deferSkillConfirmationPopup}
        lang={lang}
        t={t}
      />

      {/* ── 删除确认弹窗 ── */}
      <ConfirmDialog
        open={deleteConfirm.open}
        title={t("confirmDeleteTitle")}
        message={t("confirmDeleteMsg").replace("{name}", deleteConfirm.name)}
        onCancel={cancelDelete}
        onConfirm={confirmDelete}
        t={t}
      />
      <ConfirmDialog
        open={skillDirDeleteConfirm.open}
        title={t("confirmDeleteTitle")}
        message={skillDirDeleteConfirm.kind === "roots"
          ? t("confirmDeleteSkillRootMsg")
          : t("confirmDeleteWhitelistDirMsg")}
        onCancel={cancelSkillDirDelete}
        onConfirm={confirmSkillDirDelete}
        t={t}
      />
      <ConfirmDialog
        open={resetConfigConfirmOpen}
        title={t("resetDefaultConfigTitle")}
        message={t("resetDefaultConfigMessage")}
        onCancel={cancelResetDefaultConfig}
        onConfirm={() => { void confirmResetDefaultConfig(); }}
        confirmText={t("confirmResetDefaultConfig")}
        t={t}
      />
      <ConfirmDialog
        open={resetSkillRulesConfirmOpen}
        title={t("resetSkillRulesTitle")}
        message={t("resetSkillRulesMessage")}
        onCancel={cancelResetSkillRules}
        onConfirm={() => { void confirmResetSkillRules(); }}
        confirmText={t("confirmResetSkillRules")}
        t={t}
      />
      <ConfirmDialog
        open={skillGroupDeleteConfirm.open}
        title={t("confirmDeleteTitle")}
        message={t("skillGroupDeleteConfirmMsg").replace("{name}", skillGroupDeleteConfirm.groupName || t("skillGroupDefault"))}
        onCancel={cancelSkillGroupDelete}
        onConfirm={confirmSkillGroupDelete}
        t={t}
      />

      {showPlansModal && (
        <div className="modal-overlay" onClick={() => setShowPlansModal(false)}>
          <div className="modal-content plans-modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-header">{t("planViewTitle")}</div>
            <div className="modal-body plans-modal-body">
              {plansLoading && activePlans.length === 0 && (
                <div className="shell-env-empty">{t("planViewLoading")}</div>
              )}
              {!plansLoading && activePlans.length === 0 && (
                <div className="shell-env-empty">{t("planViewEmpty")}</div>
              )}
              {activePlans.map((plan) => (
                <div key={plan.planningId} className="plan-card">
                  <div className="plan-card-header">
                    <code className="plan-id">{plan.planningId}</code>
                    <div className="plan-card-actions">
                      <span className="plan-updated">{formatTime(plan.updatedAt)}</span>
                      <span className="plan-delete-wrap">
                        <button
                          type="button"
                          className={`plan-delete-btn${planDeleteConfirmId === plan.planningId ? " active" : ""}`}
                          title={t("planDeleteTitle")}
                          aria-label={t("planDeleteTitle")}
                          aria-expanded={planDeleteConfirmId === plan.planningId}
                          disabled={deletingPlanId === plan.planningId}
                          onClick={() => togglePlanDeleteConfirm(plan.planningId)}
                        >
                          <Trash2 size={14} />
                        </button>
                        {planDeleteConfirmId === plan.planningId && (
                          <span
                            className="plan-delete-popover"
                            role="dialog"
                            aria-label={t("planDeleteTitle")}
                          >
                            <span className="plan-delete-popover-text">{t("planDeleteConfirm")}</span>
                            {planDeleteError && (
                              <span className="plan-delete-popover-error">{planDeleteError}</span>
                            )}
                            <span className="plan-delete-popover-actions">
                              <button
                                type="button"
                                className="btn btn-secondary btn-sm"
                                onClick={cancelPlanDeleteConfirm}
                                disabled={deletingPlanId === plan.planningId}
                              >
                                {t("cancel")}
                              </button>
                              <button
                                type="button"
                                className="btn btn-danger-sm"
                                onClick={() => handleConfirmDeletePlan(plan.planningId)}
                                disabled={deletingPlanId === plan.planningId}
                              >
                                {deletingPlanId === plan.planningId
                                  ? t("planViewLoading")
                                  : t("planDeleteConfirmBtn")}
                              </button>
                            </span>
                          </span>
                        )}
                      </span>
                    </div>
                  </div>
                  {plan.explanation && (
                    <div className="plan-explanation">{plan.explanation}</div>
                  )}
                  <div className="plan-progress">
                    <span className="plan-progress-text">{plan.completedSteps}/{plan.totalSteps}</span>
                    <div className="plan-progress-bar">
                      <div
                        className="plan-progress-fill"
                        style={{ width: `${plan.totalSteps > 0 ? (plan.completedSteps / plan.totalSteps) * 100 : 0}%` }}
                      />
                    </div>
                    {plan.pendingCount > 0 && (
                      <span className="plan-pending-pill">
                        {t("planPendingLabel").replace("{n}", String(plan.pendingCount))}
                      </span>
                    )}
                  </div>
                  <ul className="plan-step-list">
                    {plan.plan.map((item, i) => (
                      <li key={i} className={`plan-step plan-step-${item.status}`}>
                        <span className={`plan-step-dot plan-dot-${item.status}`} />
                        <span className="plan-step-text">{item.step}</span>
                        <span className={`plan-step-tag plan-tag-${item.status}`}>{t(`planStatus_${item.status}` as TKey) ?? item.status}</span>
                      </li>
                    ))}
                  </ul>
                </div>
              ))}
            </div>
            <div className="modal-footer plans-modal-footer">
              <span className="plans-auto-refresh-hint">{t("planAutoRefresh")}</span>
              <div className="plans-footer-actions">
                <button className="btn btn-secondary" onClick={() => setShowPlansModal(false)}>{t("planViewClose")}</button>
                <button className="btn btn-start" onClick={handleRefreshPlans} disabled={plansLoading}>{t("planViewRefresh")}</button>
              </div>
            </div>
          </div>
        </div>
      )}

    </div>
  );
}

export default App;

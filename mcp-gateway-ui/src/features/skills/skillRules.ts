import type { TFunction } from "../../i18n";
import type { SkillCommandRule, SkillPolicyAction, SkillRootEntry, SkillsConfig } from "../../types";
import { argsToStr, strToArgs } from "../../utils/serverConfig";

export function parseRulesJson(input: string): SkillCommandRule[] {
  const parsed = JSON.parse(input) as unknown;
  if (!Array.isArray(parsed)) {
    throw new Error("rules must be an array");
  }
  return parsed.map((entry, index) => {
    if (!entry || typeof entry !== "object") {
      throw new Error(`rule ${index + 1} must be object`);
    }
    const row = entry as Record<string, unknown>;
    const action = typeof row.action === "string" ? row.action : "allow";
    if (action !== "allow" && action !== "confirm" && action !== "deny") {
      throw new Error(`rule ${index + 1} action must be allow/confirm/deny`);
    }

    return {
      id: typeof row.id === "string" ? row.id : `rule-${index + 1}`,
      action: action as SkillPolicyAction,
      commandTree: Array.isArray(row.commandTree) ? row.commandTree.map((item) => String(item)) : [],
      contains: Array.isArray(row.contains) ? row.contains.map((item) => String(item)) : [],
      reason: typeof row.reason === "string" ? row.reason : "",
    };
  });
}

export type SkillRuleMatchType = "commandTree" | "contains" | "both";

export interface SkillRuleFormState {
  action: SkillPolicyAction;
  matchType: SkillRuleMatchType;
  commandPattern: string;
  containsPattern: string;
  reason: string;
}

export interface SkillRuleGroup {
  key: "deny" | "confirm";
  labelKey: "skillsRulesGroupDeny" | "skillsRulesGroupConfirm";
  hintKey: "skillsRulesGroupDenyHint" | "skillsRulesGroupConfirmHint";
  rules: SkillCommandRule[];
}

export function createEmptySkillRuleForm(): SkillRuleFormState {
  return {
    action: "confirm",
    matchType: "commandTree",
    commandPattern: "",
    containsPattern: "",
    reason: "",
  };
}

export function ruleToForm(rule: SkillCommandRule): SkillRuleFormState {
  const hasCommandTree = rule.commandTree.length > 0;
  const hasContains = rule.contains.length > 0;
  return {
    action: rule.action,
    matchType: hasCommandTree && hasContains ? "both" : hasCommandTree || !hasContains ? "commandTree" : "contains",
    commandPattern: argsToStr(rule.commandTree),
    containsPattern: rule.contains.join("\n"),
    reason: rule.reason,
  };
}

export function normalizeContainsInput(value: string): string[] {
  return value
    .split(/\r?\n|,/)
    .map((item) => item.trim())
    .filter((item) => item.length > 0);
}

export function formToRule(form: SkillRuleFormState, id: string): SkillCommandRule {
  const commandTree = form.matchType === "commandTree" || form.matchType === "both"
    ? strToArgs(form.commandPattern.trim())
    : [];
  const contains = form.matchType === "contains" || form.matchType === "both"
    ? normalizeContainsInput(form.containsPattern)
    : [];
  return {
    id,
    action: form.action,
    commandTree,
    contains,
    reason: form.reason.trim(),
  };
}

export function isSkillRuleFormValid(form: SkillRuleFormState): boolean {
  const hasCommandTree = strToArgs(form.commandPattern.trim()).length > 0;
  const hasContains = normalizeContainsInput(form.containsPattern).length > 0;
  if (form.matchType === "commandTree") return hasCommandTree;
  if (form.matchType === "contains") return hasContains;
  return hasCommandTree && hasContains;
}

export function createSkillRuleId(rules: SkillCommandRule[]): string {
  const used = new Set(rules.map((rule) => rule.id));
  let candidate = `custom-rule-${Date.now().toString(36)}`;
  let index = 2;
  while (used.has(candidate)) {
    candidate = `custom-rule-${Date.now().toString(36)}-${index}`;
    index += 1;
  }
  return candidate;
}

export function describeSkillRuleMatch(rule: SkillCommandRule, t: TFunction): string {
  const parts: string[] = [];
  if (rule.commandTree.length > 0) {
    parts.push(`${t("skillsRuleCommandTreeLabel")} ${argsToStr(rule.commandTree)}`);
  }
  if (rule.contains.length > 0) {
    parts.push(`${t("skillsRuleContainsLabel")} ${rule.contains.join(", ")}`);
  }
  return parts.length > 0 ? parts.join(" · ") : t("skillsRuleNoCondition");
}

export function skillRuleMatchesSearch(rule: SkillCommandRule, query: string, t: TFunction): boolean {
  const tokens = query
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter((item) => item.length > 0);

  if (tokens.length === 0) return true;

  const haystack = [
    rule.id,
    rule.action,
    rule.action === "allow" ? t("policyAllow") : rule.action === "confirm" ? t("policyConfirm") : t("policyDeny"),
    argsToStr(rule.commandTree),
    rule.commandTree.join(" "),
    rule.contains.join(" "),
    rule.reason,
    describeSkillRuleMatch(rule, t),
  ].join("\n").toLowerCase();

  return tokens.every((token) => haystack.includes(token));
}

export function groupSkillRules(rules: SkillCommandRule[]): SkillRuleGroup[] {
  return [
    {
      key: "deny",
      labelKey: "skillsRulesGroupDeny",
      hintKey: "skillsRulesGroupDenyHint",
      rules: rules.filter((rule) => rule.action === "deny"),
    },
    {
      key: "confirm",
      labelKey: "skillsRulesGroupConfirm",
      hintKey: "skillsRulesGroupConfirmHint",
      rules: rules.filter((rule) => rule.action === "confirm"),
    },
  ];
}


export function ensureSkillsConfig(
  raw: Partial<SkillsConfig> | undefined,
  fallbackRules: SkillCommandRule[] = [],
): SkillsConfig {
  const legacyPolicy = (raw?.policy as unknown as {
    confirmKeywords?: string[];
    denyKeywords?: string[];
  } | undefined);
  const normalizedRoots = Array.isArray(raw?.roots)
    ? raw.roots.map((item) => item.trim()).filter((item) => item.length > 0)
    : [];
  const parsedRootEntries = Array.isArray(raw?.rootEntries)
    ? raw.rootEntries
        .map((entry) => ({
          path: (entry?.path ?? "").trim(),
          enabled: entry?.enabled === true,
        }))
        .filter((entry) => entry.path.length > 0)
    : [];
  const rootEntries: SkillRootEntry[] = parsedRootEntries.length > 0
    ? [...parsedRootEntries]
    : normalizedRoots.map((path) => ({ path, enabled: true }));

  const knownPaths = new Set(rootEntries.map((entry) => entry.path.toLowerCase()));
  normalizedRoots.forEach((path) => {
    if (!knownPaths.has(path.toLowerCase())) {
      rootEntries.push({ path, enabled: true });
      knownPaths.add(path.toLowerCase());
    }
  });

  const hasLegacy = Array.isArray(legacyPolicy?.confirmKeywords) || Array.isArray(legacyPolicy?.denyKeywords);
  let rules = Array.isArray(raw?.policy?.rules) && raw.policy.rules.length > 0
    ? raw.policy.rules
    : fallbackRules;

  if ((!raw?.policy?.rules || raw.policy.rules.length === 0) && hasLegacy) {
    const legacyConfirm = legacyPolicy?.confirmKeywords ?? [];
    const legacyDeny = legacyPolicy?.denyKeywords ?? [];
    const legacyRules: SkillCommandRule[] = [];
    legacyDeny.forEach((item, index) => {
      if (item.trim().length > 0) {
        legacyRules.push({
          id: `legacy-deny-${index + 1}`,
          action: "deny",
          commandTree: [],
          contains: [item],
          reason: `Legacy deny keyword: ${item}`,
        });
      }
    });
    legacyConfirm.forEach((item, index) => {
      if (item.trim().length > 0) {
        legacyRules.push({
          id: `legacy-confirm-${index + 1}`,
          action: "confirm",
          commandTree: [],
          contains: [item],
          reason: `Legacy confirm keyword: ${item}`,
        });
      }
    });
    if (legacyRules.length > 0) {
      rules = legacyRules;
    }
  }

  return {
    serverName: raw?.serverName?.trim() || "__skills__",
    builtinServerName: raw?.builtinServerName?.trim() || "__builtin_skills__",
    roots: rootEntries.filter((entry) => entry.enabled).map((entry) => entry.path),
    rootEntries,
    policy: {
      defaultAction: raw?.policy?.defaultAction ?? "allow",
      rules: rules.map((rule) => ({
        id: (rule.id ?? "").trim(),
        action: rule.action ?? "allow",
        commandTree: Array.isArray(rule.commandTree)
          ? rule.commandTree.map((item) => item.trim()).filter((item) => item.length > 0)
          : [],
        contains: Array.isArray(rule.contains)
          ? rule.contains.map((item) => item.trim()).filter((item) => item.length > 0)
          : [],
        reason: (rule.reason ?? "").trim(),
      })),
      pathGuard: {
        enabled: raw?.policy?.pathGuard?.enabled ?? false,
        whitelistDirs: Array.isArray(raw?.policy?.pathGuard?.whitelistDirs)
          ? raw.policy.pathGuard.whitelistDirs.map((item) => item.trim()).filter((item) => item.length > 0)
          : [],
        onViolation: raw?.policy?.pathGuard?.onViolation ?? "allow",
      },
    },
    execution: {
      timeoutMs: raw?.execution?.timeoutMs ?? 60000,
      maxOutputBytes: raw?.execution?.maxOutputBytes ?? 131072,
    },
    builtinTools: {
      readFile: raw?.builtinTools?.readFile ?? true,
      shellCommand: raw?.builtinTools?.shellCommand ?? true,
      multiEditFile: raw?.builtinTools?.multiEditFile ?? true,
      taskPlanning: raw?.builtinTools?.taskPlanning ?? true,
      chromeCdp: raw?.builtinTools?.chromeCdp ?? true,
      chatPlusAdapterDebugger: raw?.builtinTools?.chatPlusAdapterDebugger ?? true,
    },
  };
}

export function normalizeSkillsForSubmit(input: SkillsConfig, fallbackRules: SkillCommandRule[]): SkillsConfig {
  const normalized = ensureSkillsConfig(input, fallbackRules);
  return {
    ...normalized,
    policy: {
      ...normalized.policy,
      pathGuard: {
        ...normalized.policy.pathGuard,
        enabled: true,
      },
    },
  };
}

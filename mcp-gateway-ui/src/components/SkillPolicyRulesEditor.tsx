import { useMemo, useState } from "react";
import { ChevronDown, ChevronRight, Copy, Pencil, Plus, RotateCcw, Save, Search, Trash2, X } from "lucide-react";
import type { Lang, TFunction } from "../i18n";
import type { SkillCommandRule, SkillPolicyAction } from "../types";
import { localizeSkillRuleReason } from "../features/skills/defaultRuleReasons";
import {
  describeSkillRuleMatch,
  groupSkillRules,
  isSkillRuleFormValid,
  skillRuleMatchesSearch,
  type SkillRuleFormState,
} from "../features/skills/skillRules";
import { argsToStr } from "../utils/serverConfig";

function renderSkillRuleMatch(rule: SkillCommandRule, t: TFunction) {
  const parts: Array<{ label: string; value: string }> = [];
  if (rule.commandTree.length > 0) {
    parts.push({ label: t("skillsRuleCommandTreeLabel"), value: argsToStr(rule.commandTree) });
  }
  if (rule.contains.length > 0) {
    parts.push({ label: t("skillsRuleContainsLabel"), value: rule.contains.join(", ") });
  }
  if (parts.length === 0) return t("skillsRuleNoCondition");

  return (
    <span className="skills-rule-condition-parts">
      {parts.map((part, index) => (
        <span className="skills-rule-condition-fragment" key={`${part.label}-${index}`}>
          {index > 0 && <span className="skills-rule-condition-join">{t("skillsRuleConditionAnd")}</span>}
          <span className="skills-rule-condition-term">
            <span className="skills-rule-condition-label">{part.label}</span>
            <span className="skills-rule-condition-value">{part.value}</span>
          </span>
        </span>
      ))}
    </span>
  );
}

export function SkillPolicyRulesEditor({
  rules,
  form,
  formOpen,
  editingRuleId,
  advancedOpen,
  jsonDraft,
  jsonError,
  onStartAdd,
  onResetToDefault,
  onEdit,
  onCopy,
  onDelete,
  onCancelForm,
  onSubmitForm,
  onFormChange,
  onToggleAdvanced,
  onJsonChange,
  lang,
  t,
}: {
  rules: SkillCommandRule[];
  form: SkillRuleFormState;
  formOpen: boolean;
  editingRuleId: string | null;
  advancedOpen: boolean;
  jsonDraft: string;
  jsonError: string | null;
  onStartAdd: () => void;
  onResetToDefault: () => void;
  onEdit: (rule: SkillCommandRule) => void;
  onCopy: (rule: SkillCommandRule) => void;
  onDelete: (id: string) => void;
  onCancelForm: () => void;
  onSubmitForm: () => void;
  onFormChange: (patch: Partial<SkillRuleFormState>) => void;
  onToggleAdvanced: () => void;
  onJsonChange: (value: string) => void;
  lang: Lang;
  t: TFunction;
}) {
  const [ruleSearch, setRuleSearch] = useState("");
  const normalizedRuleSearch = ruleSearch.trim();
  const filteredRules = useMemo(
    () => rules.filter((rule) => skillRuleMatchesSearch(rule, normalizedRuleSearch, t)),
    [normalizedRuleSearch, rules, t],
  );
  const groupedRules = groupSkillRules(filteredRules);
  const hasRuleSearch = normalizedRuleSearch.length > 0;
  const showCommandInput = form.matchType === "commandTree" || form.matchType === "both";
  const showContainsInput = form.matchType === "contains" || form.matchType === "both";
  const actionLabel = (action: SkillPolicyAction) => {
    if (action === "allow") return t("policyAllow");
    if (action === "confirm") return t("policyConfirm");
    return t("policyDeny");
  };

  return (
    <div className="skills-rules-manager">
      <div className="skills-rules-toolbar">
        <div className="field-label">
          {t("skillsRulesVisualTitle")}
          <span className="field-label-hint">（{t("skillsRulesVisualHint")}）</span>
        </div>
        <div className="skills-rules-toolbar-actions">
          <label className="skills-rules-search" aria-label={t("skillsRulesSearchLabel")}>
            <Search size={14} />
            <input
              value={ruleSearch}
              onChange={(event) => setRuleSearch(event.target.value)}
              placeholder={t("skillsRulesSearchPlaceholder")}
            />
            {hasRuleSearch && (
              <button
                className="skills-rules-search-clear"
                type="button"
                title={t("skillsRulesSearchClear")}
                onClick={() => setRuleSearch("")}
              >
                <X size={13} />
              </button>
            )}
          </label>
          <button
            className="btn-icon skills-rules-tool-btn"
            title={t("skillsRuleAdd")}
            aria-label={t("skillsRuleAdd")}
            onClick={onStartAdd}
          >
            <Plus size={14} />
          </button>
          <button
            className="btn-icon skills-rules-tool-btn"
            title={t("skillsRuleResetDefault")}
            aria-label={t("skillsRuleResetDefault")}
            onClick={onResetToDefault}
          >
            <RotateCcw size={14} />
          </button>
        </div>
      </div>

      {hasRuleSearch && (
        <div className="skills-rules-search-meta">
          {filteredRules.length === 0
            ? t("skillsRulesSearchNoResults")
            : t("skillsRulesSearchResults")
              .replace("{shown}", String(filteredRules.length))
              .replace("{total}", String(rules.length))}
        </div>
      )}

      {formOpen && (
        <div className="modal-overlay" onClick={onCancelForm}>
          <div className="modal-content skills-rule-modal" onClick={(event) => event.stopPropagation()}>
            <div className="modal-header">
              <div className="modal-title">
                {editingRuleId ? t("skillsRuleEditTitle") : t("skillsRuleAddTitle")}
              </div>
              <div className="skills-rule-modal-actions">
                <button
                  className="btn-icon skills-rule-modal-action"
                  title={t("skillsRuleSave")}
                  aria-label={t("skillsRuleSave")}
                  onClick={onSubmitForm}
                  disabled={!isSkillRuleFormValid(form)}
                >
                  <Save size={14} />
                </button>
                <button className="btn-icon skills-rule-modal-action" title={t("cancel")} aria-label={t("cancel")} onClick={onCancelForm}>
                  <X size={14} />
                </button>
              </div>
            </div>
            <div className="modal-body">
              <div className="skills-rule-form">
                <div className="gw-field">
                  <label className="field-label">{t("skillsRuleMatchTypeLabel")}</label>
                  <div className="skills-rule-segmented skills-rule-segmented-three" role="group" aria-label={t("skillsRuleMatchTypeLabel")}>
                    <button
                      className={`skills-rule-segment ${form.matchType === "commandTree" ? "active" : ""}`}
                      onClick={() => onFormChange({ matchType: "commandTree" })}
                    >
                      {t("skillsRuleMatchCommandTree")}
                    </button>
                    <button
                      className={`skills-rule-segment ${form.matchType === "contains" ? "active" : ""}`}
                      onClick={() => onFormChange({ matchType: "contains" })}
                    >
                      {t("skillsRuleMatchContains")}
                    </button>
                    <button
                      className={`skills-rule-segment ${form.matchType === "both" ? "active" : ""}`}
                      onClick={() => onFormChange({ matchType: "both" })}
                    >
                      {t("skillsRuleMatchBoth")}
                    </button>
                  </div>
                </div>

                {showCommandInput && (
                  <div className="gw-field">
                    <label className="field-label">{t("skillsRuleCommandInput")}</label>
                    <input
                      className="form-input"
                      value={form.commandPattern}
                      onChange={(event) => onFormChange({ commandPattern: event.target.value })}
                      placeholder={t("skillsRuleCommandPlaceholder")}
                    />
                    <span className="json-hint">{t("skillsRuleCommandHelp")}</span>
                  </div>
                )}

                {showContainsInput && (
                  <div className="gw-field">
                    <label className="field-label">{t("skillsRuleContainsInput")}</label>
                    <textarea
                      className="form-textarea skills-rule-pattern-textarea"
                      value={form.containsPattern}
                      onChange={(event) => onFormChange({ containsPattern: event.target.value })}
                      placeholder={t("skillsRuleContainsPlaceholder")}
                    />
                    <span className="json-hint">{t("skillsRuleContainsHelp")}</span>
                  </div>
                )}

                <div className="gw-field">
                  <label className="field-label">{t("skillsRuleReasonLabel")}</label>
                  <input
                    className="form-input"
                    value={form.reason}
                    onChange={(event) => onFormChange({ reason: event.target.value })}
                    placeholder={t("skillsRuleReasonPlaceholder")}
                  />
                </div>

                <div className="gw-field skills-rule-action-field">
                  <label className="field-label">{t("skillsRuleActionLabel")}</label>
                  <div className="skills-rule-segmented" role="group" aria-label={t("skillsRuleActionLabel")}>
                    {(["confirm", "deny"] as SkillPolicyAction[]).map((action) => (
                      <button
                        key={action}
                        className={`skills-rule-segment ${form.action === action ? "active" : ""} ${action}`}
                        onClick={() => onFormChange({ action })}
                      >
                        {actionLabel(action)}
                      </button>
                    ))}
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      )}

      <div className="skills-rule-groups">
        {groupedRules.map((group) => (
          <div className={`skills-rule-group ${group.key}`} key={group.key}>
            <div className="skills-rule-group-head">
              <div className="field-label">
                {t(group.labelKey)}
                <span className="field-label-hint">（{t(group.hintKey)}）</span>
              </div>
              <span className="skills-rule-count">{t("skillsRulesCount").replace("{count}", String(group.rules.length))}</span>
            </div>

            {group.rules.length === 0 ? (
              <div className="skills-rule-empty">
                {hasRuleSearch ? t("skillsRulesSearchGroupEmpty") : t("skillsRulesGroupEmpty")}
              </div>
            ) : (
              <div className="skills-rule-list">
                {group.rules.map((rule) => (
                  <div className="skills-rule-row" key={rule.id}>
                    <span className={`skills-rule-action ${rule.action}`}>{actionLabel(rule.action)}</span>
                    <div className="skills-rule-main">
                      <div className="skills-rule-condition" title={describeSkillRuleMatch(rule, t)}>
                        {renderSkillRuleMatch(rule, t)}
                      </div>
                      <div className="skills-rule-reason">{rule.reason ? localizeSkillRuleReason(rule.reason, lang, rule.reasonKey) : t("skillsRuleNoReason")}</div>
                    </div>
                    <div className="skills-rule-actions">
                      <button className="btn-icon" title={t("skillsRuleEdit")} onClick={() => onEdit(rule)}>
                        <Pencil size={13} />
                      </button>
                      <button className="btn-icon" title={t("skillsRuleCopy")} onClick={() => onCopy(rule)}>
                        <Copy size={13} />
                      </button>
                      <button className="btn-icon btn-danger-icon" title={t("skillsRuleDelete")} onClick={() => onDelete(rule.id)}>
                        <Trash2 size={13} />
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        ))}
      </div>

      <div className="skills-rules-advanced">
        <button className="skills-rules-advanced-toggle" onClick={onToggleAdvanced}>
          {advancedOpen ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          <span className="field-label">{t("skillsRulesAdvancedJson")}</span>
        </button>
        {advancedOpen && (
          <div className="skills-rules-advanced-body">
            <textarea
              className="form-textarea skills-rules-textarea"
              value={jsonDraft}
              onChange={(event) => onJsonChange(event.target.value)}
              placeholder={t("skillsRulesHint")}
            />
            <span className="json-hint">{t("skillsRulesAdvancedHint")}</span>
            {jsonError && <span className="skills-rules-error">{jsonError}</span>}
          </div>
        )}
      </div>
    </div>
  );
}


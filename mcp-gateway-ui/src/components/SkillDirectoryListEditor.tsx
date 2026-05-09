import { FolderOpen } from "lucide-react";
import type { TFunction } from "../i18n";
import type { SkillDirectoryItem, SkillDirStatus } from "../features/skills/directories";

export function SkillDirectoryListEditor({
  title,
  hint,
  items,
  onAdd,
  onRemove,
  onPathChange,
  onValidate,
  onBrowse,
  onToggleEnabled,
  enableToggle = false,
  showValidation = true,
  t,
}: {
  title: string;
  hint: string;
  items: SkillDirectoryItem[];
  onAdd: () => void;
  onRemove: (id: string) => void;
  onPathChange: (id: string, value: string) => void;
  onValidate?: (id: string) => void;
  onBrowse: (id: string) => void;
  onToggleEnabled?: (id: string) => void;
  enableToggle?: boolean;
  showValidation?: boolean;
  t: TFunction;
}) {
  const statusLabel = (status: SkillDirStatus): string => {
    if (status === "checking") return t("skillDirChecking");
    if (status === "valid") return t("skillDirValid");
    if (status === "invalid") return t("skillDirInvalid");
    if (status === "error") return t("skillDirError");
    return t("skillDirIdle");
  };

  return (
    <div className="skills-dir-panel">
      <div className="skills-dir-panel-head">
        <label className="field-label">{title}</label>
        <button className="btn-add-dir" title={t("addFolderPath")} onClick={onAdd}>+</button>
      </div>
      <div className="skills-dir-list">
        {items.map((item) => (
          <div className={`skills-dir-row ${showValidation ? "" : "no-validation"} ${enableToggle ? "with-toggle" : ""}`} key={item.id}>
            {enableToggle && (
              <button
                className={`toggle-btn skills-dir-toggle ${item.enabled ? "toggle-on" : "toggle-off"}`}
                disabled={item.status !== "valid"}
                onClick={() => onToggleEnabled?.(item.id)}
                title={item.status === "valid"
                  ? (item.enabled ? t("enabledClick") : t("disabledClick"))
                  : t("skillRootEnableBlocked")}
                aria-label={item.enabled ? t("enabledClick") : t("disabledClick")}
              />
            )}
            <input
              className="form-input skills-dir-input"
              value={item.path}
              placeholder={t("folderPathPlaceholder")}
              onChange={(e) => onPathChange(item.id, e.target.value)}
              onBlur={() => {
                if (showValidation && onValidate) {
                  onValidate(item.id);
                }
              }}
            />
            <button className="btn btn-secondary btn-sm skills-dir-browse" onClick={() => onBrowse(item.id)}>
              <FolderOpen size={13} />
              {t("browseFolder")}
            </button>
            {showValidation && (
              <>
                <span className={`skills-dir-dot ${item.status}`} aria-hidden />
                <span className={`skills-dir-status ${item.status}`}>{statusLabel(item.status)}</span>
              </>
            )}
            <button className="btn-icon btn-danger-icon skills-dir-remove" title={t("remove")} onClick={() => onRemove(item.id)}>
              ✕
            </button>
          </div>
        ))}
      </div>
      <span className="json-hint">{hint}</span>
    </div>
  );
}


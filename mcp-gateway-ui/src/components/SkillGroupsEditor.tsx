import { useState } from "react";
import { Check, ChevronDown, ChevronRight, Pencil, Trash2, Plus, FolderOpen, Copy } from "lucide-react";
import type { TFunction } from "../i18n";
import type { SkillDirStatus, SkillGroup } from "../features/skills/directories";

export function SkillGroupsEditor({
  groups,
  onRemoveGroup,
  onRenameGroup,
  onAddItem,
  onRemoveItem,
  onPathChange,
  onValidate,
  onBrowse,
  onToggleEnabled,
  onImportToGroup,
  onCopy,
  copied,
  running,
  baseUrl,
  ssePath,
  httpPath,
  t,
}: {
  groups: SkillGroup[];
  onRemoveGroup: (groupId: string) => void;
  onRenameGroup: (groupId: string, name: string) => void;
  onAddItem: (groupId: string) => void;
  onRemoveItem: (groupId: string, itemId: string) => void;
  onPathChange: (groupId: string, itemId: string, value: string) => void;
  onValidate: (groupId: string, itemId: string) => void;
  onBrowse: (groupId: string, itemId: string) => void;
  onToggleEnabled: (groupId: string, itemId: string) => void;
  onImportToGroup: (groupId: string) => void;
  onCopy: (name: string, type: "sse" | "streamable-http", url: string, key: string) => void;
  copied: string | null;
  running: boolean;
  baseUrl: string;
  ssePath: string;
  httpPath: string;
  t: TFunction;
}) {
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(new Set());
  const [editingGroupId, setEditingGroupId] = useState<string | null>(null);
  const [editingName, setEditingName] = useState("");


  const toggleCollapse = (groupId: string) => {
    setCollapsedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(groupId)) next.delete(groupId);
      else next.add(groupId);
      return next;
    });
  };

  const startRename = (group: SkillGroup) => {
    setEditingGroupId(group.id);
    setEditingName(group.name);
  };

  const commitRename = (groupId: string) => {
    const trimmed = editingName.trim();
    if (trimmed) {
      onRenameGroup(groupId, trimmed);
    }
    // If empty, don\'t commit - just cancel
    setEditingGroupId(null);
    setEditingName("");
  };

  const cancelRename = () => {
    setEditingGroupId(null);
    setEditingName("");
  };



  const statusLabel = (status: SkillDirStatus): string => {
    if (status === "checking") return t("skillDirChecking");
    if (status === "valid") return t("skillDirValid");
    if (status === "invalid") return t("skillDirInvalid");
    if (status === "error") return t("skillDirError");
    return t("skillDirIdle");
  };

  if (groups.length === 0) {
    return (
      <div className="skill-groups-editor">
        <div className="skill-group-empty">{t("skillGroupEmpty")}</div>
      </div>
    );
  }

  return (
    <div className="skill-groups-editor">
      {groups.map((group) => {
        const isCollapsed = collapsedGroups.has(group.id);
        const isEditing = editingGroupId === group.id;
        const validCount = group.items.filter((i) => i.status === "valid" && i.enabled).length;
        const mcpName = group.name ? `__${group.name}__` : "";
        const groupSseUrl = mcpName ? `${baseUrl}${ssePath}/${mcpName}` : "";
        const groupHttpUrl = mcpName ? `${baseUrl}${httpPath}/${mcpName}` : "";
        const nameEmpty = !group.name.trim();

        return (
          <div className={`skill-group-block${nameEmpty ? " skill-group-block-warning" : ""}`} key={group.id}>
            <div className="skill-group-header">
              <button
                className="skill-group-collapse-btn"
                onClick={() => toggleCollapse(group.id)}
                aria-label={isCollapsed ? t("skillGroupExpand") : t("skillGroupCollapse")}
              >
                {isCollapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
              </button>

              {isEditing ? (
                <input
                  className={`form-input skill-group-name-input${!editingName.trim() ? " input-warning" : ""}`}
                  value={editingName}
                  onChange={(e) => setEditingName(e.target.value)}
                  onBlur={() => commitRename(group.id)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") commitRename(group.id);
                    if (e.key === "Escape") cancelRename();
                  }}
                  placeholder={t("skillGroupNamePlaceholder")}
                  autoFocus
                />
              ) : (
                <span
                  className={`skill-group-name${nameEmpty ? " skill-group-name-warning" : ""}`}
                  onDoubleClick={() => startRename(group)}
                  title={mcpName ? `MCP: ${mcpName}` : t("skillGroupNamePlaceholder")}
                >
                  {group.name || t("skillGroupNamePlaceholder")}
                  {mcpName && <code className="skill-group-mcp-name">{mcpName}</code>}
                  <span className="skill-group-count">{validCount}/{group.items.length}</span>
                </span>
              )}

              <div className="skill-group-actions">
                {!isEditing && (
                  <button
                    className="btn-icon skill-group-action-btn"
                    title={t("skillGroupRename")}
                    onClick={() => startRename(group)}
                  >
                    <Pencil size={12} />
                  </button>
                )}
                <button
                  className="btn-icon skill-group-action-btn"
                  title={t("addSkillRootPath")}
                  onClick={() => onAddItem(group.id)}
                >
                  <Plus size={13} />
                </button>
                <button
                  className="btn-icon skill-group-action-btn"
                  title={t("importSkillRoots")}
                  onClick={() => onImportToGroup(group.id)}
                >
                  <FolderOpen size={12} />
                </button>
                <button
                  className="btn-icon btn-danger-icon skill-group-action-btn"
                  title={t("skillGroupDelete")}
                  onClick={() => onRemoveGroup(group.id)}
                >
                  <Trash2 size={12} />
                </button>
              </div>
            </div>

            {!isCollapsed && (
              <div className="skill-group-body">
                {running && mcpName && (
                  <div className="skills-endpoints skill-group-endpoints-row">
                    <div className="endpoint-item">
                      <span className="endpoint-label">SSE</span>
                      <code className="endpoint-url">{groupSseUrl}</code>
                      <button className="btn-icon" title={t("copySkillSse")} onClick={() => onCopy(mcpName, "sse", groupSseUrl, `${group.id}-sse`)}>
                        {copied === `${group.id}-sse` ? <Check size={11} color="var(--accent-green)" /> : <Copy size={11} />}
                      </button>
                    </div>
                    <div className="endpoint-item">
                      <span className="endpoint-label">HTTP</span>
                      <code className="endpoint-url">{groupHttpUrl}</code>
                      <button className="btn-icon" title={t("copySkillHttp")} onClick={() => onCopy(mcpName, "streamable-http", groupHttpUrl, `${group.id}-http`)}>
                        {copied === `${group.id}-http` ? <Check size={11} color="var(--accent-green)" /> : <Copy size={11} />}
                      </button>
                    </div>
                  </div>
                )}

                {nameEmpty && (
                  <div className="skill-group-name-warning-hint">{t("skillGroupNameRequired")}</div>
                )}

                <div className="skill-group-items">
                  {group.items.length === 0 && (
                    <div className="skill-group-empty">{t("skillGroupEmpty")}</div>
                  )}
                  {group.items.map((item) => (
                    <div className="skills-dir-row with-toggle" key={item.id}>
                      <button
                        className={`toggle-btn skills-dir-toggle ${item.enabled ? "toggle-on" : "toggle-off"}`}
                        disabled={item.status !== "valid"}
                        onClick={() => onToggleEnabled(group.id, item.id)}
                        title={item.status === "valid"
                          ? (item.enabled ? t("enabledClick") : t("disabledClick"))
                          : t("skillRootEnableBlocked")}
                        aria-label={item.enabled ? t("enabledClick") : t("disabledClick")}
                      />
                      <input
                        className="form-input skills-dir-input"
                        value={item.path}
                        placeholder={t("folderPathPlaceholder")}
                        onChange={(e) => onPathChange(group.id, item.id, e.target.value)}
                        onBlur={() => onValidate(group.id, item.id)}
                      />
                      <button className="btn btn-secondary btn-sm skills-dir-browse" onClick={() => onBrowse(group.id, item.id)}>
                        <FolderOpen size={13} />
                        {t("browseFolder")}
                      </button>
                      <span className={`skills-dir-dot ${item.status}`} aria-hidden />
                      <span className={`skills-dir-status ${item.status}`}>{statusLabel(item.status)}</span>
                      <button
                        className="btn-icon btn-danger-icon skills-dir-remove"
                        title={t("remove")}
                        onClick={() => onRemoveItem(group.id, item.id)}
                      >
                        ✕
                      </button>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

import type { TFunction } from "../i18n";
import type { SkillConfirmation } from "../types";
import { formatTime } from "../utils/display";

function skillPreviewLabel(kind: string | undefined, t: TFunction): string {
  if (kind === "edit") return t("editPreview");
  return t("commandPreview");
}

export function SkillConfirmations({ pending, busyIds, onApprove, onReject, t }: {
  pending: SkillConfirmation[];
  busyIds: Set<string>;
  onApprove: (id: string) => void;
  onReject: (id: string) => void;
  t: TFunction;
}) {
  if (pending.length === 0) {
    return <div className="empty-hint">{t("noSkillPending")}</div>;
  }

  return (
    <div className="skill-confirm-list">
      {pending.map((item) => {
        const busy = busyIds.has(item.id);
        const displayName = item.displayName.trim();
        const showDisplayName = displayName.length > 0 && displayName !== item.skill;
        return (
          <div className="skill-confirm-item" key={item.id}>
            <div className="skill-confirm-head">
              <div className="skill-confirm-meta">
                <span className="skill-chip">{item.skill}</span>
                {showDisplayName && <span className="skill-script">{displayName}</span>}
              </div>
              <div className="skill-confirm-actions">
                <button className="btn btn-secondary btn-sm" disabled={busy} onClick={() => onReject(item.id)}>
                  {t("reject")}
                </button>
                <button className="btn btn-start btn-sm" disabled={busy} onClick={() => onApprove(item.id)}>
                  {t("approve")}
                </button>
              </div>
            </div>
            <div className="skill-confirm-row">
              <span className="field-label">{skillPreviewLabel(item.kind, t)}</span>
              <code className="skill-command">{item.preview || item.rawCommand}</code>
            </div>
            {item.cwd && (
              <div className="skill-confirm-row">
                <span className="field-label">{t("cwd")}</span>
                <code className="skill-command">{item.cwd}</code>
              </div>
            )}
            {item.affectedPaths && item.affectedPaths.length > 0 && (
              <div className="skill-confirm-row">
                <span className="field-label">{t("affectedPaths")}</span>
                <code className="skill-command">{item.affectedPaths.join("\n")}</code>
              </div>
            )}
            <div className="skill-confirm-row">
              <span className="field-label">{t("confirmReason")}</span>
              <span>{item.reason}</span>
            </div>
            <div className="skill-confirm-row">
              <span className="field-label">{t("createdAt")}</span>
              <span>{formatTime(item.createdAt)}</span>
            </div>
          </div>
        );
      })}
    </div>
  );
}

export function SkillConfirmationPopup({
  open,
  item,
  busy,
  onApprove,
  onReject,
  onLater,
  t,
}: {
  open: boolean;
  item: SkillConfirmation | null;
  busy: boolean;
  onApprove: (id: string) => void;
  onReject: (id: string) => void;
  onLater: (id: string) => void;
  t: TFunction;
}) {
  if (!open || !item) return null;
  const displayName = item.displayName.trim();
  const showDisplayName = displayName.length > 0 && displayName !== item.skill;
  return (
    <div className="modal-overlay" onClick={() => onLater(item.id)}>
      <div className="modal-content" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">{t("skillsConfirmPopupTitle")}</div>
        <div className="modal-body">
          <div>{t("skillsConfirmPopupMsg")}</div>
          <div className="json-hint" style={{ marginTop: 8 }}>{t("skillsConfirmTimeoutHint")}</div>
          <div className="skill-confirm-meta" style={{ marginTop: 10 }}>
            <span className="skill-chip">{item.skill}</span>
            {showDisplayName && <span className="skill-script">{displayName}</span>}
          </div>
          <div className="skill-confirm-row" style={{ marginTop: 10 }}>
            <span className="field-label">{skillPreviewLabel(item.kind, t)}</span>
            <code className="skill-command">{item.preview || item.rawCommand}</code>
          </div>
          {item.cwd && (
            <div className="skill-confirm-row">
              <span className="field-label">{t("cwd")}</span>
              <code className="skill-command">{item.cwd}</code>
            </div>
          )}
          {item.affectedPaths && item.affectedPaths.length > 0 && (
            <div className="skill-confirm-row">
              <span className="field-label">{t("affectedPaths")}</span>
              <code className="skill-command">{item.affectedPaths.join("\n")}</code>
            </div>
          )}
          <div className="skill-confirm-row">
            <span className="field-label">{t("confirmReason")}</span>
            <span>{item.reason}</span>
          </div>
          <div className="skill-confirm-row">
            <span className="field-label">{t("createdAt")}</span>
            <span>{formatTime(item.createdAt)}</span>
          </div>
        </div>
        <div className="modal-footer">
          <button className="btn btn-secondary" disabled={busy} onClick={() => onLater(item.id)}>
            {t("decideLater")}
          </button>
          <button className="btn btn-secondary" disabled={busy} onClick={() => onReject(item.id)}>
            {t("reject")}
          </button>
          <button className="btn btn-start" disabled={busy} onClick={() => onApprove(item.id)}>
            {t("approve")}
          </button>
        </div>
      </div>
    </div>
  );
}


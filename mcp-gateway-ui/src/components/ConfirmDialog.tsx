import type { TFunction } from "../i18n";

export function ConfirmDialog({ open, title, message, onCancel, onConfirm, t }: {
  open: boolean;
  title: string;
  message: string;
  onCancel: () => void;
  onConfirm: () => void;
  t: TFunction;
}) {
  if (!open) return null;
  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal-content" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">{title}</div>
        <div className="modal-body">
          {message}
        </div>
        <div className="modal-footer">
          <button className="btn btn-secondary" onClick={onCancel}>{t("cancel")}</button>
          <button className="btn btn-danger" onClick={onConfirm}>{t("confirmDelete")}</button>
        </div>
      </div>
    </div>
  );
}



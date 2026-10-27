import { t } from "../i18n";
import { useAppStore } from "../store-context";

/**
 * The toasts of import and export (`store-interop.ts`): "Exported 12 pages to Notes.zip", "Saved Tasks.csv".
 * One line each with a button that dismisses it; an information toast goes by itself after 15 seconds, an
 * error stays until it is dismissed. Like the reminder toasts the container is always there, a `log` (a polite
 * live region), so that a screen reader announces what appears in it.
 */
export function Toasts() {
  const toasts = useAppStore((state) => state.toasts);
  const dismiss = useAppStore((state) => state.dismissToast);

  return (
    <div className="toasts" role="log" aria-label={t("toast.label")}>
      {toasts.map(({ id, text, kind, action }) => (
        <div key={id} className={kind === "error" ? "toast toast-error" : "toast"}>
          <p className="toast-text">{text}</p>
          {action !== undefined && (
            <button
              type="button"
              className="link-button toast-action"
              onClick={() => {
                dismiss(id);
                action.run();
              }}
            >
              {action.label}
            </button>
          )}
          <button
            type="button"
            className="link-button toast-dismiss"
            onClick={() => {
              dismiss(id);
            }}
          >
            {t("toast.dismiss")}
          </button>
        </div>
      ))}
    </div>
  );
}

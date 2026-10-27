import { t } from "../i18n";
import { useAppStore } from "../store-context";

/**
 * What the device log brought (`DevicesChanged`): "<name> was added to your account",
 * "<name> was removed", "Your keys were rotated". Shown once, above every screen, until
 * dismissed; later changes are added to it.
 */
export function DevicesNotice() {
  const notice = useAppStore((state) => state.devicesNotice);
  const dismiss = useAppStore((state) => state.dismissDevicesNotice);

  if (notice === null) {
    return null;
  }
  const lines = [
    ...notice.added.map((name) => t("devicesNotice.added", { name })),
    ...notice.removed.map((name) =>
      name === null ? t("devicesNotice.removedUnnamed") : t("devicesNotice.removed", { name }),
    ),
    ...(notice.rotated ? [t("devicesNotice.rotated")] : []),
  ];
  return (
    <aside className="banner devices-notice" aria-label={t("devicesNotice.label")}>
      <ul className="devices-notice-list" role="status">
        {lines.map((line, index) => (
          <li key={index}>{line}</li>
        ))}
      </ul>
      <button type="button" className="link-button" onClick={dismiss}>
        {t("devicesNotice.dismiss")}
      </button>
    </aside>
  );
}

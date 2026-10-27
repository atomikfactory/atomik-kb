import { toCoreError } from "@atomik-kb/core-client";

import { t } from "../i18n";
import { useAppStore } from "../store-context";
import { ConfirmDialog } from "./ConfirmDialog";

/**
 * Asks before turning sync off (Settings -> Sync and the fork banner): the device forgets
 * the server, the notes stay. A refusal of the core goes to `onError`; the dialog closes
 * either way.
 */
export function SyncDisconnectDialog({ onClose, onError }: { onClose: () => void; onError: (message: string) => void }) {
  const disconnectSync = useAppStore((state) => state.disconnectSync);

  return (
    <ConfirmDialog
      title={t("sync.disconnect.title")}
      body={t("sync.disconnect.body")}
      confirmLabel={t("sync.disconnect.confirm")}
      onConfirm={async () => {
        try {
          await disconnectSync();
        } catch (failure) {
          onError(t("sync.disconnect.error", { message: toCoreError(failure).message }));
        }
      }}
      onClose={onClose}
    />
  );
}

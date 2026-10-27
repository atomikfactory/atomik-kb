// Where the invitee's dialogs of Milestone 13 live: mounted once by the main layout, so that they show whichever way the
// session started. "Join a shared space" (a code) opens from the space list (`openJoinDialog`) and follows the session through
// to its end; the preview of an invitation link shows as soon as a link session is on, whether the person pasted the link or the
// operating system opened the app with it (`openInviteLink`, `store-deep-links.ts`). The session is the store's: read at boot, kept
// fresh by `InviteChanged`.

import { useAppStore } from "../store-context";
import { selectVisibleInvite } from "../store-sharing";
import { JoinPreviewDialog } from "./JoinPreviewDialog";
import { JoinSpaceDialog } from "./JoinSpaceDialog";

export function InviteSessionHost() {
  const invite = useAppStore(selectVisibleInvite);
  const joinOpen = useAppStore((state) => state.joinOpen);
  const closeJoinDialog = useAppStore((state) => state.closeJoinDialog);
  const dismissInvite = useAppStore((state) => state.dismissInvite);

  const link = invite?.flow === "Link" && invite.side === "Invitee" ? invite : null;
  const code = invite?.flow === "Code" && invite.side === "Invitee" ? invite : null;

  if (link !== null) {
    return <JoinPreviewDialog session={link} />;
  }
  if (joinOpen || code !== null) {
    return (
      <JoinSpaceDialog
        onClose={() => {
          closeJoinDialog();
          dismissInvite();
        }}
      />
    );
  }
  return null;
}

import type { SyncInfo, SyncState } from "@atomik-kb/core-client";

import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { useAppStore } from "../store-context";

/** The word for each state (the indicator's title, the status line of Settings -> Sync). */
export const SYNC_STATE_LABELS: Readonly<Record<SyncState, MessageKey>> = {
  Disabled: "sync.state.Disabled",
  Idle: "sync.state.Idle",
  Pushing: "sync.state.Pushing",
  Pulling: "sync.state.Pulling",
  Offline: "sync.state.Offline",
  Error: "sync.state.Error",
  Forked: "sync.state.Forked",
};

/** How a state looks: grey at rest, blue (animated) while moving data, amber offline, red when stuck. */
export type SyncTone = "idle" | "busy" | "offline" | "error";

const TONES: Readonly<Record<SyncState, SyncTone>> = {
  Disabled: "idle",
  Idle: "idle",
  Pushing: "busy",
  Pulling: "busy",
  Offline: "offline",
  Error: "error",
  Forked: "error",
};

export function syncTone(state: SyncState): SyncTone {
  return TONES[state];
}

/**
 * The state the indicator shows: `null` when sync is off (nothing to show), and `Forked`
 * for as long as a fork is detected, even while the driver still pulls, so that the dot
 * stays red until the user did something about it.
 */
export function indicatedState(sync: SyncInfo | null): SyncState | null {
  if (sync === null || !sync.configured || sync.state === "Disabled") {
    return null;
  }
  return sync.forkDetected ? "Forked" : sync.state;
}

/**
 * The dot next to the workspace name in the sidebar: hidden while sync is off; its title
 * names the state, and a click opens Settings.
 */
export function SyncIndicator() {
  const state = useAppStore((store) => indicatedState(store.sync));
  const navigate = useAppStore((store) => store.navigate);

  if (state === null) {
    return null;
  }
  const label = t("sync.indicator", { state: t(SYNC_STATE_LABELS[state]) });
  return (
    <button
      type="button"
      className={`sync-dot sync-tone-${syncTone(state)}`}
      title={label}
      aria-label={label}
      data-state={state}
      onClick={() => {
        navigate("settings");
      }}
    />
  );
}

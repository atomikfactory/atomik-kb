// "Use AI in this space" (Milestone 13, ADR-0047): this account's own switch, off by default for a shared space. Turning it on lets
// local AI on this account's devices read the space (the search index, the writing assistant, the chat and the MCP tools the person
// allows); it never changes what other members do on theirs, which the text beside it says.

import { toCoreError } from "@atomik-kb/core-client";
import type { SpaceId } from "@atomik-kb/core-client";
import { useId, useState } from "react";

import { t } from "../i18n";
import { spaceAiOn } from "../sharing";
import { useSpace } from "../sharing-hooks";
import { useAppStore } from "../store-context";

export function SpaceAiSwitch({ spaceId }: { spaceId: SpaceId }) {
  const space = useSpace(spaceId);
  const setSpaceAi = useAppStore((state) => state.setSpaceAi);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const hintId = useId();
  const on = spaceAiOn(space);

  async function change(enabled: boolean): Promise<void> {
    setError(null);
    setBusy(true);
    try {
      await setSpaceAi(spaceId, enabled);
    } catch (failure) {
      setError(t("sharing.ai.error", { message: toCoreError(failure).message }));
    }
    setBusy(false);
  }

  return (
    <section className="members-section" aria-labelledby={`ai-${spaceId}`}>
      <h3 className="members-heading" id={`ai-${spaceId}`}>
        {t("sharing.ai.title")}
      </h3>
      <label className="checkbox-label">
        <input
          type="checkbox"
          role="switch"
          checked={on}
          disabled={busy}
          aria-describedby={hintId}
          onChange={(event) => {
            void change(event.target.checked);
          }}
        />
        <span>{t("sharing.ai.label")}</span>
      </label>
      <p className="muted" id={hintId}>
        {t("sharing.ai.hint")} {t("sharing.ai.others")}
      </p>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

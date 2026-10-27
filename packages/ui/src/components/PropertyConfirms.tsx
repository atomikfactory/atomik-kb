// The questions the properties UI asks before something that cannot be undone: deleting a property
// (its values leave every page of the space) and deleting an option (it leaves every page that has
// it). The count of pages is asked of the core when the question opens and joins the text when it
// arrives. A refusal of the core goes to `onError`, which the owner shows.

import { toCoreError } from "@atomik-kb/core-client";
import type { PropertyDef, PropertyOption, SpaceId } from "@atomik-kb/core-client";
import { useEffect, useEffectEvent, useState } from "react";

import { t, tp } from "../i18n";
import { pairedPropertyOf } from "../property-format";
import { useAppStore } from "../store-context";
import { ConfirmDialog } from "./ConfirmDialog";

/** A count asked of the core when the component mounts: `null` until it arrives (and when it fails). */
export function useCount(load: () => Promise<number>): number | null {
  const [count, setCount] = useState<number | null>(null);
  const start = useEffectEvent(load);
  useEffect(() => {
    let current = true;
    start().then(
      (value) => {
        if (current) {
          setCount(value);
        }
      },
      () => undefined,
    );
    return () => {
      current = false;
    };
  }, []);
  return count;
}

export interface ConfirmProps {
  spaceId: SpaceId;
  /** Shows why the core refused. */
  onError: (message: string) => void;
  onClose: () => void;
  /** Where focus goes when the question closes (the control that led to it). */
  returnFocus?: Element | null;
}

/** "Delete the property ...?": its values go from every page of the space. */
export function DeletePropertyConfirm({
  spaceId,
  def,
  onError,
  onClose,
  returnFocus,
}: ConfirmProps & { def: PropertyDef }) {
  const deleteProperty = useAppStore((state) => state.deleteProperty);
  const countPagesWithProperty = useAppStore((state) => state.countPagesWithProperty);
  const schema = useAppStore((state) => state.propertySchemas[spaceId]);
  const count = useCount(() => countPagesWithProperty(spaceId, def.id));
  let body = t("properties.delete.body", { name: def.name });
  if (count !== null) {
    body = tp("properties.delete.bodyCount", count, { name: def.name });
  }
  // The other half of a two-way relation is kept, as a one-way relation.
  const paired = schema === undefined ? undefined : pairedPropertyOf(def, schema);
  if (paired !== undefined) {
    body = `${body} ${t("properties.delete.pair", { other: paired.name })}`;
  }
  return (
    <ConfirmDialog
      returnFocus={returnFocus}
      title={t("properties.delete.title", { name: def.name })}
      body={body}
      confirmLabel={t("properties.delete.confirm")}
      onConfirm={async () => {
        try {
          await deleteProperty(spaceId, def.id);
        } catch (failure) {
          onError(t("properties.error.delete", { message: toCoreError(failure).message }));
        }
      }}
      onClose={onClose}
    />
  );
}

/** "Delete the option ...?": it leaves every page that has it. */
export function DeleteOptionConfirm({
  spaceId,
  def,
  option,
  onError,
  onClose,
  returnFocus,
}: ConfirmProps & { def: PropertyDef; option: PropertyOption }) {
  const deletePropertyOption = useAppStore((state) => state.deletePropertyOption);
  const countPagesWithOption = useAppStore((state) => state.countPagesWithOption);
  const count = useCount(() => countPagesWithOption(spaceId, def.id, option.id));
  let body = t("properties.option.deleteBody", { name: option.name, property: def.name });
  if (count !== null) {
    body = tp("properties.option.deleteBodyCount", count, { name: option.name, property: def.name });
  }
  return (
    <ConfirmDialog
      returnFocus={returnFocus}
      title={t("properties.option.deleteTitle", { name: option.name })}
      body={body}
      confirmLabel={t("properties.option.deleteConfirm")}
      onConfirm={async () => {
        try {
          await deletePropertyOption(spaceId, def.id, option.id);
        } catch (failure) {
          onError(t("properties.error.delete", { message: toCoreError(failure).message }));
        }
      }}
      onClose={onClose}
    />
  );
}

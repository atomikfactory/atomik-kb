// The glue between the in-memory core and the formula engine (`./formula`, ADR-0055,
// `docs/architecture/formulas.md`): the schema the engine checks against, the refusal a formula that does not check
// deserves, and the names the move and the importers use. Pure functions; `MemoryProperties` builds the host that reads
// the pages.

import type { CoreError } from "./errors";
import { formulaReferencedIds } from "./formula";
import type { FormulaError, FormulaProperty, FormulaSchema } from "./formula";
import type { PropertyConfig } from "./generated/PropertyConfig";
import type { PropertyKind } from "./generated/PropertyKind";
import { ROLLUP_TITLE_TARGET, isRollupTargetKind, rollupFunctionAccepts } from "./rollup";
import { invalid } from "./validate";

/** What the schema of a space gives the engine: the fields of `PropertyDef` it needs. */
export interface FormulaSource {
  id: string;
  name: string;
  kind: PropertyKind;
  options: readonly { id: string; name: string }[];
  config?: PropertyConfig | undefined;
}

/**
 * The name the title has in the display forms the core writes and reads (the sidecar, a seed, an import): the
 * language-independent `title` of `prop("title")`. The core has no translations; the editor shows and accepts its own
 * localised label for the title on top of this. A property of the schema with the same name wins.
 */
export const FORMULA_TITLE_LABEL = "title";

/**
 * The formula engine's view of a property (`FormulaProperty`), given any other property by id. A rollup carries the
 * options of its target (`show_original` of a select, a multi-select or a status shows the names of the originals).
 */
export function formulaPropertyOf(source: FormulaSource, lookup: (id: string) => FormulaSource | undefined): FormulaProperty {
  const kindOf = (id: string): PropertyKind | undefined => lookup(id)?.kind;
  const config = source.config;
  const expression = source.kind === "formula" && config?.kind === "formula" ? config.expression : undefined;
  let rollup: FormulaProperty["rollup"];
  let optionsOf: FormulaSource = source;
  if (source.kind === "rollup" && config?.kind === "rollup") {
    // A rollup that no longer reads (its relation or target is gone) has none: the engine cannot read it.
    const target = config.target === ROLLUP_TITLE_TARGET ? null : kindOf(config.target);
    if (
      kindOf(config.relationPropertyId) === "relation" &&
      target !== undefined &&
      (target === null || isRollupTargetKind(target)) &&
      rollupFunctionAccepts(config.function, target ?? undefined)
    ) {
      rollup = { function: config.function, targetKind: target };
      optionsOf = (config.target === ROLLUP_TITLE_TARGET ? undefined : lookup(config.target)) ?? source;
    }
  }
  return {
    id: source.id,
    name: source.name,
    kind: source.kind,
    options: optionsOf.options.map((option) => ({ id: option.id, name: option.name })),
    ...(expression === undefined ? {} : { expression }),
    ...(rollup === undefined ? {} : { rollup }),
  };
}

/** The schema of a list of properties, by stored id, in the order given. */
export function formulaSchemaOf(sources: readonly FormulaSource[]): FormulaSchema {
  const byKey = new Map<string, FormulaSource>(sources.map((source) => [source.id, source]));
  const properties = sources.map((source) => formulaPropertyOf(source, (id) => byKey.get(id)));
  const byId = new Map(properties.map((property) => [property.id, property]));
  return {
    property: (id) => byId.get(id),
    properties: () => properties,
  };
}

/** `the formula is refused: code at start..end: message` (what the core answers, `FormulaError`'s display). */
export function formulaRefusal(error: FormulaError): CoreError {
  return invalid("config", `the formula is refused: ${error.code} at ${String(error.start)}..${String(error.end)}: ${error.message}`);
}

/** The ids a formula's stored expression names that are in `known` (the `prop` calls, in order, without repeats). */
export function formulaReads(expression: string, known: ReadonlySet<string>): string[] {
  return formulaReferencedIds(expression).filter((id) => known.has(id));
}

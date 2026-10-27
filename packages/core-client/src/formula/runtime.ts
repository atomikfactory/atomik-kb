// The state of one top-level evaluation: the host, the time, the step budget (shared by the nested formulas), the
// formulas being evaluated (the run-time cycle stack) and the compiled regular expressions.

import { compilePattern } from "./text";
import { MAX_FORMULA_STEPS } from "./limits";
import type { FormulaEnv, FormulaHost } from "./types";

/** Thrown when the evaluation goes over its step budget; the whole formula is then empty. */
export class BudgetExceeded extends Error {
  constructor() {
    super("formula step budget exceeded");
    this.name = "BudgetExceeded";
  }
}

export class Runtime {
  steps = 0;
  /** `page\0property` of the formulas being evaluated. */
  readonly active = new Set<string>();
  private readonly patterns = new Map<string, RegExp | null>();

  constructor(
    readonly host: FormulaHost,
    readonly env: FormulaEnv,
  ) {}

  /** One step: a node evaluated or a list item visited. */
  tick(): void {
    this.steps += 1;
    if (this.steps > MAX_FORMULA_STEPS) {
      throw new BudgetExceeded();
    }
  }

  /** The compiled pattern (once per pattern text), or `null` when it does not compile. */
  pattern(source: string): RegExp | null {
    if (this.patterns.has(source)) {
      return this.patterns.get(source) ?? null;
    }
    const compiled = compilePattern(source);
    this.patterns.set(source, compiled);
    return compiled;
  }
}

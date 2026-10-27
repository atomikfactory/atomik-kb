// The evaluator (section 4): runs a checked expression over a page. Empty is `null` and propagates as section 2.4 says;
// every step counts against the budget; relations reach one hop (4.3); a formula property read by another is evaluated
// on its page by the same evaluator (one budget, one cycle stack).

import type { FormulaResult } from "../generated/FormulaResult";
import type { PropertyValue } from "../generated/PropertyValue";
import { Checker } from "./check";
import type { Compiled } from "./check";
import { dateFromValue, dateToValue, localMs, makeDate } from "./dates";
import { Fail } from "./errors";
import { formatValue } from "./format";
import { STRICT_FUNCTIONS, runStrict } from "./functions";
import { FORMULA_TITLE_ID } from "./limits";
import type { BinNode, CallNode, Node, UnNode } from "./parser";
import { BudgetExceeded, Runtime } from "./runtime";
import type { Ty } from "./ty";
import type { FormulaEnv, FormulaHost, FormulaProperty } from "./types";
import { equalValues, finish, isDate, isPage, list, cmpText, normalize } from "./value";
import type { Value } from "./value";

interface RtScope {
  readonly name: string;
  readonly value: Value;
  readonly parent: RtScope | null;
}

/** Where an expression runs: the page, how far it is from the formula's own page, the variables, the `+` modes. */
interface Ctx {
  readonly page: string;
  /** 0 for the formula's page, 1 for a page of its relations. */
  readonly depth: number;
  readonly scope: RtScope | null;
  readonly plus: ReadonlySet<Node>;
}

const LAMBDA_FUNCTIONS: ReadonlySet<string> = new Set(["map", "filter", "find", "findIndex", "some", "every", "count"]);

function nth<T>(items: readonly T[], i: number): T {
  const item = items[i];
  if (item === undefined) {
    throw new Error("formula: missing argument");
  }
  return item;
}

function lookup(scope: RtScope | null, name: string): Value {
  for (let s = scope; s !== null; s = s.parent) {
    if (s.name === name) {
      return s.value;
    }
  }
  return null;
}

/** What a property of that type reads when it has nothing: `""`, `false`, `[]` or empty (a formula without a value). */
function unsetOf(ty: Ty): Value {
  if (ty.list) {
    return [];
  }
  if (ty.base === "text" || ty.base === null) {
    return "";
  }
  return ty.base === "boolean" ? false : null;
}

export class Evaluator {
  private readonly rt: Runtime;
  private readonly checker: Checker;
  private optionNames: Map<string, string> | undefined;

  constructor(host: FormulaHost, env: FormulaEnv) {
    this.rt = new Runtime(host, env);
    this.checker = new Checker(host);
  }

  /** Checks `stored` against the host's schema; throws a `Fail` when it does not check. */
  compile(stored: string): Compiled {
    return this.checker.checkText(stored);
  }

  /** The value of a checked expression on a page: raw (before section 4.4's normalisation). */
  run(compiled: Compiled, page: string): Value {
    return this.ev(compiled.ast, { page, depth: 0, scope: null, plus: compiled.plusText });
  }

  // --- reading properties ---

  private optionName(prop: FormulaProperty, id: string): string | undefined {
    const own = prop.options.find((o) => o.id === id);
    if (own !== undefined) {
      return own.name;
    }
    if (this.optionNames === undefined) {
      const all = new Map<string, string>();
      for (const other of this.rt.host.properties()) {
        for (const option of other.options) {
          if (!all.has(option.id)) {
            all.set(option.id, option.name);
          }
        }
      }
      this.optionNames = all;
    }
    return this.optionNames.get(id);
  }

  /** What one stored value of a rollup's `show_original` adds to the list (a list value is flattened). */
  private originals(prop: FormulaProperty, v: PropertyValue, out: Value[], depth: number): void {
    switch (v.kind) {
      case "text":
      case "url":
      case "email":
      case "phone":
        out.push(v.value);
        break;
      case "number":
        out.push(v.value);
        break;
      case "select":
      case "status": {
        const name = this.optionName(prop, v.value);
        if (name !== undefined) {
          out.push(name);
        }
        break;
      }
      case "multi_select":
        for (const id of v.value) {
          const name = this.optionName(prop, id);
          if (name !== undefined) {
            out.push(name);
          }
        }
        break;
      case "date": {
        const date = dateFromValue(v.value);
        if (date !== null) {
          out.push(date);
        }
        break;
      }
      case "checkbox":
        out.push(v.value);
        break;
      case "checklist":
        for (const item of v.value) {
          out.push(item.done);
        }
        break;
      case "place":
        out.push(v.value.name);
        break;
      case "person":
        for (const account of v.value) {
          out.push({ k: "person", id: account });
        }
        break;
      case "relation":
        for (const related of v.value) {
          out.push({ k: "page", id: related, hop: depth + 1 });
        }
        break;
      case "files":
        for (const attachment of v.value) {
          const name = this.rt.host.fileName(attachment);
          if (name !== undefined) {
            out.push(name);
          }
        }
        break;
      default:
        break;
    }
  }

  private readRollup(prop: FormulaProperty, stored: PropertyValue | undefined, depth: number): Value {
    const original = prop.rollup?.function === "show_original";
    if (stored?.kind !== "rollup") {
      return original ? [] : null;
    }
    const result = stored.value.result;
    switch (result.type) {
      case "number":
        return result.data;
      case "date":
        return dateFromValue(result.data);
      case "progress":
        return result.data.total === 0 ? null : result.data.done / result.data.total;
      case "values": {
        const out: Value[] = [];
        for (const item of result.data) {
          this.originals(prop, item, out, depth);
        }
        return out;
      }
    }
  }

  /** The value of a property of a page, as the table of section 2.3 says (`depth`: 0 for the formula's page). */
  private read(page: string, id: string, depth: number): Value {
    const host = this.rt.host;
    if (id === FORMULA_TITLE_ID) {
      return host.title(page) ?? "";
    }
    const prop = host.property(id);
    if (prop === undefined) {
      return null;
    }
    switch (prop.kind) {
      case "text": {
        const v = host.value(page, id);
        return v?.kind === "text" ? v.value : "";
      }
      case "url": {
        const v = host.value(page, id);
        return v?.kind === "url" ? v.value : "";
      }
      case "email": {
        const v = host.value(page, id);
        return v?.kind === "email" ? v.value : "";
      }
      case "phone": {
        const v = host.value(page, id);
        return v?.kind === "phone" ? v.value : "";
      }
      case "number": {
        const v = host.value(page, id);
        return v?.kind === "number" && Number.isFinite(v.value) ? v.value : null;
      }
      case "select": {
        const v = host.value(page, id);
        return v?.kind === "select" ? (this.optionName(prop, v.value) ?? "") : "";
      }
      case "status": {
        const v = host.value(page, id);
        return v?.kind === "status" ? (this.optionName(prop, v.value) ?? "") : "";
      }
      case "multi_select": {
        const v = host.value(page, id);
        const out: Value[] = [];
        if (v?.kind === "multi_select") {
          for (const option of v.value) {
            const name = this.optionName(prop, option);
            if (name !== undefined) {
              out.push(name);
            }
          }
        }
        return out;
      }
      case "date": {
        const v = host.value(page, id);
        return v?.kind === "date" ? dateFromValue(v.value) : null;
      }
      case "checkbox": {
        const v = host.value(page, id);
        return v?.kind === "checkbox" ? v.value : false;
      }
      case "files": {
        const v = host.value(page, id);
        const out: Value[] = [];
        if (v?.kind === "files") {
          for (const attachment of v.value) {
            const name = host.fileName(attachment);
            if (name !== undefined) {
              out.push(name);
            }
          }
        }
        return out;
      }
      case "checklist": {
        const v = host.value(page, id);
        return v?.kind === "checklist" ? v.value.map((item) => item.done) : [];
      }
      case "place": {
        const v = host.value(page, id);
        return v?.kind === "place" ? v.value.name : "";
      }
      case "person": {
        const v = host.value(page, id);
        return v?.kind === "person" ? v.value.map((account): Value => ({ k: "person", id: account })) : [];
      }
      case "relation":
        return host.related(page, id).map((related): Value => ({ k: "page", id: related, hop: depth + 1 }));
      case "rollup":
        return this.readRollup(prop, host.value(page, id), depth);
      case "created_time": {
        const ms = host.createdMs(page);
        return ms === undefined ? null : makeDate(localMs(ms, this.rt.env.offsetSeconds), true);
      }
      case "last_edited_time": {
        const ms = host.editedMs(page);
        return ms === undefined ? null : makeDate(localMs(ms, this.rt.env.offsetSeconds), true);
      }
      case "formula":
        return this.formulaValue(prop, page, depth);
      case "created_by":
      case "last_edited_by":
        return null;
    }
  }

  /**
   * The value of a formula property on a page: its result as section 4.4 makes it (no value reads as an unset property of
   * its type does). A formula already being evaluated on that page reads empty.
   */
  private formulaValue(prop: FormulaProperty, page: string, depth: number): Value {
    const key = `${page}\u0000${prop.id}`;
    if (this.rt.active.has(key)) {
      return null;
    }
    const compiled = this.checker.compiledOf(prop);
    if (compiled === undefined) {
      return null;
    }
    this.rt.active.add(key);
    try {
      const raw = this.ev(compiled.ast, { page, depth, scope: null, plus: compiled.plusText });
      return normalize(raw) ?? unsetOf(compiled.ty);
    } finally {
      this.rt.active.delete(key);
    }
  }

  // --- evaluation ---

  private ev(n: Node, c: Ctx): Value {
    this.rt.tick();
    switch (n.k) {
      case "num":
        return n.v;
      case "str":
        return n.v;
      case "bool":
        return n.v;
      case "var":
        return lookup(c.scope, n.name);
      case "list": {
        const out: Value[] = [];
        for (const item of n.items) {
          out.push(this.ev(item, c));
        }
        return finish(out);
      }
      case "un":
        return this.unary(n, c);
      case "cond":
        return this.ev(this.ev(n.test, c) === true ? n.yes : n.no, c);
      case "bin":
        return this.binary(n, c);
      case "call":
        return this.call(n, c);
    }
  }

  /** A chain of prefix operators is applied in a loop: it can be thousands of nodes deep. */
  private unary(n: UnNode, c: Ctx): Value {
    const chain: UnNode[] = [n];
    let inner: Node = n.arg;
    while (inner.k === "un") {
      this.rt.tick();
      chain.push(inner);
      inner = inner.arg;
    }
    let v = this.ev(inner, c);
    for (let i = chain.length - 1; i >= 0; i -= 1) {
      v = nth(chain, i).op === "-" ? (typeof v === "number" ? finish(-v) : null) : v !== true;
    }
    return v;
  }

  /** A chain of binary operators (`1 + 2 + 3 + …`) is run along its left side in a loop, for the same reason. */
  private binary(n: BinNode, c: Ctx): Value {
    const spine: BinNode[] = [n];
    let first: Node = n.left;
    while (first.k === "bin") {
      this.rt.tick();
      spine.push(first);
      first = first.left;
    }
    let left = this.ev(first, c);
    for (let i = spine.length - 1; i >= 0; i -= 1) {
      left = this.apply(nth(spine, i), left, c);
    }
    return left;
  }

  /** `left op right`, the value of the left side being known (the right side is evaluated only when it is needed). */
  private apply(n: BinNode, l: Value, c: Ctx): Value {
    if (n.op === "and") {
      return l === true && this.ev(n.right, c) === true;
    }
    if (n.op === "or") {
      return l === true || this.ev(n.right, c) === true;
    }
    const r = this.ev(n.right, c);
    switch (n.op) {
      case "+":
        if (c.plus.has(n)) {
          return finish(formatValue(l, this.rt.host) + formatValue(r, this.rt.host));
        }
        return typeof l === "number" && typeof r === "number" ? finish(l + r) : null;
      case "-":
        return typeof l === "number" && typeof r === "number" ? finish(l - r) : null;
      case "*":
        return typeof l === "number" && typeof r === "number" ? finish(l * r) : null;
      case "/":
        return typeof l === "number" && typeof r === "number" ? finish(l / r) : null;
      case "%":
        return typeof l === "number" && typeof r === "number" ? finish(l % r) : null;
      case "^":
        return typeof l === "number" && typeof r === "number" ? finish(Math.pow(l, r)) : null;
      case "==":
        return equalValues(l, r);
      case "!=":
        return !equalValues(l, r);
      case "<":
        return this.compare(l, r) < 0;
      case "<=":
        return this.compare(l, r) <= 0;
      case ">":
        return this.compare(l, r) > 0;
      case ">=":
        return this.compare(l, r) >= 0;
      default:
        return null;
    }
  }

  /** Orders two values of one type (number, text by code point, date by instant, boolean); `NaN` when one is empty. */
  private compare(l: Value, r: Value): number {
    if (typeof l === "number" && typeof r === "number") {
      return l < r ? -1 : l > r ? 1 : 0;
    }
    if (typeof l === "string" && typeof r === "string") {
      return cmpText(l, r);
    }
    if (typeof l === "boolean" && typeof r === "boolean") {
      return Number(l) - Number(r);
    }
    if (isDate(l) && isDate(r)) {
      return l.ms < r.ms ? -1 : l.ms > r.ms ? 1 : 0;
    }
    return Number.NaN;
  }

  private call(n: CallNode, c: Ctx): Value {
    switch (n.name) {
      case "if":
        return this.ev(this.ev(nth(n.args, 0), c) === true ? nth(n.args, 1) : nth(n.args, 2), c);
      case "ifs": {
        for (let i = 0; i + 1 < n.args.length; i += 2) {
          if (this.ev(nth(n.args, i), c) === true) {
            return this.ev(nth(n.args, i + 1), c);
          }
        }
        return this.ev(nth(n.args, n.args.length - 1), c);
      }
      case "and":
        return n.args.every((arg) => this.ev(arg, c) === true);
      case "or":
        return n.args.some((arg) => this.ev(arg, c) === true);
      case "let":
      case "lets": {
        let scope = c.scope;
        for (let i = 0; i + 1 < n.args.length; i += 2) {
          const name = nth(n.args, i);
          const value = this.ev(nth(n.args, i + 1), { ...c, scope });
          scope = { name: name.k === "var" ? name.name : "", value, parent: scope };
        }
        return this.ev(nth(n.args, n.args.length - 1), { ...c, scope });
      }
      case "prop":
        return this.propCall(n, c);
      case "id": {
        if (n.args.length === 0) {
          return c.page;
        }
        const page = this.ev(nth(n.args, 0), c);
        return isPage(page) ? page.id : null;
      }
      default:
        break;
    }
    if (LAMBDA_FUNCTIONS.has(n.name)) {
      return this.lambda(n, c);
    }
    const def = STRICT_FUNCTIONS.get(n.name);
    if (def === undefined) {
      return null;
    }
    return runStrict(
      def,
      n.args.map((arg) => this.ev(arg, c)),
      this.rt,
    );
  }

  private propCall(n: CallNode, c: Ctx): Value {
    const literal = nth(n.args, n.args.length - 1);
    if (literal.k !== "str") {
      return null;
    }
    if (n.args.length === 1) {
      return this.read(c.page, literal.v, c.depth);
    }
    const page = this.ev(nth(n.args, 0), c);
    if (!isPage(page) || page.hop > 1) {
      return null;
    }
    return this.read(page.id, literal.v, 1);
  }

  private lambda(n: CallNode, c: Ctx): Value {
    const items = list(this.ev(nth(n.args, 0), c));
    const body = n.args[1];
    if (body === undefined) {
      return items.length;
    }
    const each = (item: Value, index: number): Value => {
      this.rt.tick();
      const scope: RtScope = {
        name: "index",
        value: index,
        parent: { name: "current", value: item, parent: c.scope },
      };
      return this.ev(body, { ...c, scope });
    };
    switch (n.name) {
      case "map": {
        const out: Value[] = [];
        items.forEach((item, i) => {
          out.push(each(item, i));
        });
        return finish(out);
      }
      case "filter": {
        const out: Value[] = [];
        items.forEach((item, i) => {
          if (each(item, i) === true) {
            out.push(item);
          }
        });
        return out;
      }
      case "find": {
        for (let i = 0; i < items.length; i += 1) {
          const item = items[i] ?? null;
          if (each(item, i) === true) {
            return item;
          }
        }
        return null;
      }
      case "findIndex": {
        for (let i = 0; i < items.length; i += 1) {
          if (each(items[i] ?? null, i) === true) {
            return i;
          }
        }
        return -1;
      }
      case "some": {
        for (let i = 0; i < items.length; i += 1) {
          if (each(items[i] ?? null, i) === true) {
            return true;
          }
        }
        return false;
      }
      case "every": {
        for (let i = 0; i < items.length; i += 1) {
          if (each(items[i] ?? null, i) !== true) {
            return false;
          }
        }
        return true;
      }
      default: {
        let count = 0;
        items.forEach((item, i) => {
          if (each(item, i) === true) {
            count += 1;
          }
        });
        return count;
      }
    }
  }
}

/** A value as the `FormulaResult` of section 5 (after section 4.4's normalisation), or `null` for no value. */
export function toResult(v: Value): FormulaResult | null {
  const value = normalize(v);
  if (value === null) {
    return null;
  }
  return convert(value);
}

function convert(v: Value): FormulaResult | null {
  if (v === null) {
    return null;
  }
  if (typeof v === "number") {
    return { type: "number", data: v };
  }
  if (typeof v === "string") {
    return { type: "text", data: v };
  }
  if (typeof v === "boolean") {
    return { type: "boolean", data: v };
  }
  if (Array.isArray(v)) {
    const items: FormulaResult[] = [];
    for (const item of v) {
      const converted = convert(item);
      if (converted !== null) {
        items.push(converted);
      }
    }
    return items.length === 0 ? null : { type: "list", data: items };
  }
  switch (v.k) {
    case "date": {
      const date = dateToValue(v);
      return date === null ? null : { type: "date", data: date };
    }
    case "page":
      return { type: "page", data: v.id };
    case "person":
      return { type: "person", data: v.id };
  }
}

/** Evaluates a stored expression on a page; `null` when it does not check or its result is empty. */
export function evaluateStored(stored: string, page: string, host: FormulaHost, env: FormulaEnv): FormulaResult | null {
  try {
    const evaluator = new Evaluator(host, env);
    return toResult(evaluator.run(evaluator.compile(stored), page));
  } catch (error) {
    if (error instanceof Fail || error instanceof BudgetExceeded || error instanceof RangeError) {
      return null;
    }
    throw error;
  }
}

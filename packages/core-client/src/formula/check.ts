// The static checker (section 2.3 and 4.5): types every node (strictly), resolves names, arity, variables and
// properties, and refuses cycles among the formula properties that `prop` reaches (including through the formula being
// checked). Its answer, a `Compiled`, is what the evaluator runs: the tree plus which `+` are text concatenations.

import { Fail, failAt, makeError, stackError } from "./errors";
import { kindType } from "./kinds";
import { ARITY, ODD_ARGUMENTS } from "./names";
import { isParseError, parseCached } from "./parser";
import type { CallNode, Node } from "./parser";
import { STRICT_FUNCTIONS } from "./functions";
import { BOOLEAN, NUMBER, TEXT, describe, listOf, need, needList, scalar, typeFail, unify } from "./ty";
import type { Ty } from "./ty";
import type { FormulaError, FormulaProperty, FormulaSchema } from "./types";
import { FORMULA_TITLE_ID } from "./limits";

/** A checked expression. */
export interface Compiled {
  readonly ast: Node;
  readonly ty: Ty;
  /** The `+` nodes that concatenate text (the others add numbers). */
  readonly plusText: ReadonlySet<Node>;
}

interface Cx {
  readonly plusText: Set<Node>;
}

interface Scope {
  readonly name: string;
  readonly ty: Ty;
  readonly parent: Scope | null;
}

const LAMBDA_FUNCTIONS: ReadonlySet<string> = new Set(["map", "filter", "find", "findIndex", "some", "every", "count"]);

function nth<T>(items: readonly T[], i: number): T {
  const item = items[i];
  if (item === undefined) {
    throw new Error("formula: missing argument");
  }
  return item;
}

function lookup(scope: Scope | null, name: string): Ty | undefined {
  for (let s = scope; s !== null; s = s.parent) {
    if (s.name === name) {
      return s.ty;
    }
  }
  return undefined;
}

function isTextScalar(t: Ty): boolean {
  return !t.list && t.base === "text";
}

/** Checks expressions against a schema; one instance per check or evaluation (it remembers the formulas it reached). */
export class Checker {
  private readonly cache = new Map<string, Compiled | FormulaError>();
  /** Ids of the formula properties being checked, the formula under edit (`selfId`) first. */
  private readonly stack: string[] = [];

  constructor(
    readonly schema: FormulaSchema,
    selfId?: string,
  ) {
    if (selfId !== undefined) {
      this.stack.push(selfId);
    }
  }

  /** Parses and checks an expression text. Throws a `Fail`. */
  checkText(text: string): Compiled {
    const parsed = parseCached(text);
    if (isParseError(parsed)) {
      throw new Fail(parsed);
    }
    const cx: Cx = { plusText: new Set() };
    try {
      const ty = this.node(parsed.ast, cx, null);
      return { ast: parsed.ast, ty, plusText: cx.plusText };
    } catch (error) {
      if (error instanceof RangeError) {
        throw new Fail(stackError(Array.from(text).length));
      }
      throw error;
    }
  }

  /** The checked expression of a formula property, or `undefined` when it does not check. */
  compiledOf(prop: FormulaProperty): Compiled | undefined {
    try {
      return this.compiledProperty(prop, { s: 0, e: 0 });
    } catch (error) {
      if (error instanceof Fail) {
        return undefined;
      }
      throw error;
    }
  }

  private compiledProperty(prop: FormulaProperty, at: { s: number; e: number }): Compiled {
    const hit = this.cache.get(prop.id);
    if (hit !== undefined) {
      if ("code" in hit) {
        throw new Fail(this.moved(hit, prop, at));
      }
      return hit;
    }
    if (this.stack.includes(prop.id)) {
      failAt("cycle", `the formula "${prop.name}" depends on itself`, at.s, at.e, prop.name);
    }
    this.stack.push(prop.id);
    try {
      const compiled = this.checkText(prop.expression ?? "");
      this.cache.set(prop.id, compiled);
      return compiled;
    } catch (error) {
      if (error instanceof Fail) {
        this.cache.set(prop.id, error.error);
        throw new Fail(this.moved(error.error, prop, at));
      }
      throw error;
    } finally {
      this.stack.pop();
    }
  }

  /**
   * An error found in another formula, as the formula that reads it reports it: at the reference. A loop stays a `cycle`;
   * any other error makes the formula one that has no result type, which cannot be read (`unsupported_property`).
   */
  private moved(error: FormulaError, prop: FormulaProperty, at: { s: number; e: number }): FormulaError {
    if (error.code === "cycle") {
      return makeError("cycle", error.message, at.s, at.e, error.detail);
    }
    return makeError(
      "unsupported_property",
      `the formula "${prop.name}" has an error: ${error.message}`,
      at.s,
      at.e,
      prop.name,
    );
  }

  private propertyType(id: string, at: Node): Ty {
    if (id === FORMULA_TITLE_ID) {
      return TEXT;
    }
    const prop = this.schema.property(id);
    if (prop === undefined) {
      return failAt("unknown_property", `no property "${id}"`, at.s, at.e, id);
    }
    const kind = kindType(prop);
    if (kind === "unsupported") {
      return failAt("unsupported_property", `"${prop.name}" cannot be read by a formula`, at.s, at.e, prop.name);
    }
    if (kind === "formula") {
      return this.compiledProperty(prop, at).ty;
    }
    return kind;
  }

  private node(n: Node, cx: Cx, scope: Scope | null): Ty {
    switch (n.k) {
      case "num":
        return NUMBER;
      case "str":
        return TEXT;
      case "bool":
        return BOOLEAN;
      case "var": {
        const ty = lookup(scope, n.name);
        if (ty === undefined) {
          return failAt("unknown_variable", `unknown variable "${n.name}"`, n.s, n.e, n.name);
        }
        return ty;
      }
      case "list": {
        let base: Ty | null = null;
        for (const item of n.items) {
          const ty = this.node(item, cx, scope);
          if (ty.list) {
            typeFail(item, "any", ty);
          }
          if (base === null) {
            base = ty;
            continue;
          }
          const merged = unify(base, ty);
          if (merged === null) {
            typeFail(item, describe(base), ty);
          }
          base = merged;
        }
        return listOf(base?.base ?? null);
      }
      case "un":
        return this.unary(n, cx, scope);
      case "cond": {
        need(this.node(n.test, cx, scope), "boolean", n.test);
        const yes = this.node(n.yes, cx, scope);
        const no = this.node(n.no, cx, scope);
        const merged = unify(yes, no);
        if (merged === null) {
          return typeFail(n.no, describe(yes), no);
        }
        return merged;
      }
      case "bin":
        return this.binary(n, cx, scope);
      case "call":
        return this.call(n, cx, scope);
    }
  }

  /** A chain of prefix operators (`- - - x`, `not not x`) is walked in a loop: it can be thousands of nodes deep. */
  private unary(n: Extract<Node, { k: "un" }>, cx: Cx, scope: Scope | null): Ty {
    const chain: Extract<Node, { k: "un" }>[] = [];
    let inner: Node = n;
    while (inner.k === "un") {
      chain.push(inner);
      inner = inner.arg;
    }
    let ty = this.node(inner, cx, scope);
    for (let i = chain.length - 1; i >= 0; i -= 1) {
      const op = nth(chain, i);
      need(ty, op.op === "-" ? "number" : "boolean", op.arg);
      ty = op.op === "-" ? NUMBER : BOOLEAN;
    }
    return ty;
  }

  /** A chain of binary operators (`1 + 2 + 3 + …`) is walked along its left side in a loop, for the same reason. */
  private binary(n: Extract<Node, { k: "bin" }>, cx: Cx, scope: Scope | null): Ty {
    const spine: Extract<Node, { k: "bin" }>[] = [];
    let first: Node = n;
    while (first.k === "bin") {
      spine.push(first);
      first = first.left;
    }
    let left = this.node(first, cx, scope);
    for (let i = spine.length - 1; i >= 0; i -= 1) {
      const op = nth(spine, i);
      left = this.binaryType(op, left, this.node(op.right, cx, scope), cx);
    }
    return left;
  }

  /** The type of `left op right`, the types of both sides being known. */
  private binaryType(n: Extract<Node, { k: "bin" }>, left: Ty, right: Ty, cx: Cx): Ty {
    switch (n.op) {
      case "+":
        if (isTextScalar(left) || isTextScalar(right)) {
          cx.plusText.add(n);
          return TEXT;
        }
        need(left, "number", n.left);
        need(right, "number", n.right);
        return NUMBER;
      case "-":
      case "*":
      case "/":
      case "%":
      case "^":
        need(left, "number", n.left);
        need(right, "number", n.right);
        return NUMBER;
      case "==":
      case "!=":
        if (unify(left, right) === null) {
          typeFail(n.right, describe(left), right);
        }
        return BOOLEAN;
      case "<":
      case "<=":
      case ">":
      case ">=": {
        for (const [ty, node] of [
          [left, n.left],
          [right, n.right],
        ] as const) {
          if (ty.list || (ty.base !== null && ty.base !== "number" && ty.base !== "text" && ty.base !== "date" && ty.base !== "boolean")) {
            typeFail(node, "number", ty);
          }
        }
        if (unify(left, right) === null) {
          typeFail(n.right, describe(left), right);
        }
        return BOOLEAN;
      }
      case "and":
      case "or":
        need(left, "boolean", n.left);
        need(right, "boolean", n.right);
        return BOOLEAN;
    }
  }

  private call(n: CallNode, cx: Cx, scope: Scope | null): Ty {
    const arity = ARITY.get(n.name);
    if (arity === undefined) {
      return failAt("unknown_function", `unknown function "${n.name}"`, n.ns, n.ne, n.name);
    }
    const [min, max] = arity;
    const count = n.args.length;
    if (count < min || (max >= 0 && count > max) || (ODD_ARGUMENTS.has(n.name) && count % 2 === 0)) {
      return failAt("arity", `${n.name} takes ${arityText(min, max)} arguments, not ${String(count)}`, n.s, n.e, `${n.name}:${String(min)}:${String(max)}`);
    }
    if (n.name === "prop") {
      return this.propCall(n, cx, scope);
    }
    if (n.name === "let" || n.name === "lets") {
      return this.letCall(n, cx, scope);
    }
    if (n.name === "id") {
      if (count === 1) {
        need(this.node(nth(n.args, 0), cx, scope), "page", nth(n.args, 0));
      }
      return TEXT;
    }
    if (LAMBDA_FUNCTIONS.has(n.name)) {
      return this.lambdaCall(n, cx, scope);
    }
    const types = n.args.map((arg) => this.node(arg, cx, scope));
    switch (n.name) {
      case "if": {
        need(nth(types, 0), "boolean", nth(n.args, 0));
        const yes = nth(types, 1);
        const no = nth(types, 2);
        const merged = unify(yes, no);
        if (merged === null) {
          return typeFail(nth(n.args, 2), describe(yes), no);
        }
        return merged;
      }
      case "ifs": {
        let result: Ty | null = null;
        for (let i = 0; i < types.length; i += 1) {
          const ty = nth(types, i);
          const isCondition = i % 2 === 0 && i < types.length - 1;
          if (isCondition) {
            need(ty, "boolean", nth(n.args, i));
            continue;
          }
          if (result === null) {
            result = ty;
            continue;
          }
          const merged = unify(result, ty);
          if (merged === null) {
            return typeFail(nth(n.args, i), describe(result), ty);
          }
          result = merged;
        }
        return result ?? TEXT;
      }
      case "and":
      case "or":
        types.forEach((ty, i) => {
          need(ty, "boolean", nth(n.args, i));
        });
        return BOOLEAN;
      default: {
        const def = STRICT_FUNCTIONS.get(n.name);
        if (def === undefined) {
          return failAt("unknown_function", `unknown function "${n.name}"`, n.ns, n.ne, n.name);
        }
        return def.check(types, n);
      }
    }
  }

  private propCall(n: CallNode, cx: Cx, scope: Scope | null): Ty {
    const literal = nth(n.args, n.args.length - 1);
    if (n.args.length === 2) {
      const page = nth(n.args, 0);
      need(this.node(page, cx, scope), "page", page);
    }
    if (literal.k !== "str") {
      return failAt("syntax", "the argument of prop must be a text in quotes", literal.s, literal.e, "text literal");
    }
    return this.propertyType(literal.v, literal);
  }

  /** `let(name, value, body)` and `lets(n1, v1, n2, v2, …, body)`: each value sees the names before it. */
  private letCall(n: CallNode, cx: Cx, scope: Scope | null): Ty {
    let inner = scope;
    for (let i = 0; i + 1 < n.args.length; i += 2) {
      const name = nth(n.args, i);
      if (name.k !== "var") {
        return failAt("syntax", "a name is expected here (no quotes)", name.s, name.e, "identifier");
      }
      const ty = this.node(nth(n.args, i + 1), cx, inner);
      inner = { name: name.name, ty, parent: inner };
    }
    return this.node(nth(n.args, n.args.length - 1), cx, inner);
  }

  /** The list functions that take an expression: `current` is the item, `index` its position. */
  private lambdaCall(n: CallNode, cx: Cx, scope: Scope | null): Ty {
    const listNode = nth(n.args, 0);
    const listTy = needList(this.node(listNode, cx, scope), listNode);
    const body = n.args[1];
    if (body === undefined) {
      return NUMBER; // count(list)
    }
    const item = scalar(listTy.base);
    const inner: Scope = {
      name: "index",
      ty: NUMBER,
      parent: { name: "current", ty: item, parent: scope },
    };
    const ty = this.node(body, cx, inner);
    if (n.name === "map") {
      if (ty.list) {
        return typeFail(body, "any", ty);
      }
      return listOf(ty.base);
    }
    need(ty, "boolean", body);
    switch (n.name) {
      case "filter":
        return listTy;
      case "find":
        return item;
      case "some":
      case "every":
        return BOOLEAN;
      default:
        return NUMBER; // findIndex, count
    }
  }
}

function arityText(min: number, max: number): string {
  if (max < 0) {
    return `at least ${String(min)}`;
  }
  return min === max ? String(min) : `${String(min)} to ${String(max)}`;
}

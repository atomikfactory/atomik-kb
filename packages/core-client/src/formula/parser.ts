// The parser (section 2.2): recursive descent over the tokens of the lexer, lowest precedence first. Method calls
// `x.f(a)` become the call `f(x, a)`. Every node carries its range in code points (`s`, `e` exclusive).

import { Fail, failAt, stackError } from "./errors";
import { scan } from "./lexer";
import type { Tok } from "./lexer";
import { isReservedName } from "./names";
import { MAX_FORMULA_CHARS, MAX_FORMULA_DEPTH } from "./limits";
import type { FormulaError, ParsedFormula } from "./types";

interface NodeBase {
  readonly s: number;
  readonly e: number;
}
export interface NumNode extends NodeBase {
  readonly k: "num";
  readonly v: number;
}
export interface StrNode extends NodeBase {
  readonly k: "str";
  readonly v: string;
}
export interface BoolNode extends NodeBase {
  readonly k: "bool";
  readonly v: boolean;
}
export interface ListNode extends NodeBase {
  readonly k: "list";
  readonly items: readonly Node[];
}
export interface VarNode extends NodeBase {
  readonly k: "var";
  readonly name: string;
}
export interface CallNode extends NodeBase {
  readonly k: "call";
  readonly name: string;
  readonly args: readonly Node[];
  /** The range of the function's name (the method's name for `x.f()`). */
  readonly ns: number;
  readonly ne: number;
}
export type BinOp =
  | "+"
  | "-"
  | "*"
  | "/"
  | "%"
  | "^"
  | "=="
  | "!="
  | "<"
  | "<="
  | ">"
  | ">="
  | "and"
  | "or";
export interface BinNode extends NodeBase {
  readonly k: "bin";
  readonly op: BinOp;
  readonly left: Node;
  readonly right: Node;
}
export interface UnNode extends NodeBase {
  readonly k: "un";
  readonly op: "-" | "not";
  readonly arg: Node;
}
export interface CondNode extends NodeBase {
  readonly k: "cond";
  readonly test: Node;
  readonly yes: Node;
  readonly no: Node;
}
export type Node = NumNode | StrNode | BoolNode | ListNode | VarNode | CallNode | BinNode | UnNode | CondNode;

/** A parsed expression. */
export interface ParsedAst extends ParsedFormula {
  readonly ast: Node;
}

const COMPARISONS: ReadonlySet<string> = new Set(["==", "!=", "<", "<=", ">", ">="]);

class Parser {
  private pos = 0;
  private depth = 0;

  constructor(
    private readonly toks: readonly Tok[],
    private readonly length: number,
  ) {}

  parse(): Node {
    const node = this.expr();
    const t = this.peek();
    if (t.kind !== "eof") {
      this.unexpected(t, "end of text");
    }
    return node;
  }

  // --- tokens ---

  private peek(offset = 0): Tok {
    return this.toks[this.pos + offset] ?? { kind: "eof", start: this.length, end: this.length, text: "" };
  }

  private next(): Tok {
    const t = this.peek();
    this.pos += 1;
    return t;
  }

  private isPunct(text: string, offset = 0): boolean {
    const t = this.peek(offset);
    return t.kind === "punct" && t.text === text;
  }

  private isOp(...texts: string[]): boolean {
    const t = this.peek();
    return t.kind === "op" && texts.includes(t.text);
  }

  private isKeyword(...texts: string[]): boolean {
    const t = this.peek();
    return t.kind === "kw" && texts.includes(t.text);
  }

  private expectPunct(text: string): Tok {
    const t = this.peek();
    if (t.kind === "punct" && t.text === text) {
      this.pos += 1;
      return t;
    }
    return this.unexpected(t, `"${text}"`);
  }

  /** Raises the error of an unusable token: its own lexical error, or "unexpected" with what was expected. */
  private unexpected(t: Tok, expected: string): never {
    if (t.err) {
      throw new Fail(t.err);
    }
    const found = t.kind === "eof" ? "the end of the text" : `"${t.text}"`;
    return failAt("syntax", `expected ${expected}, found ${found}`, t.start, t.end, expected);
  }

  private enter(t: Tok): void {
    this.depth += 1;
    if (this.depth > MAX_FORMULA_DEPTH) {
      failAt("too_long", `nested more than ${MAX_FORMULA_DEPTH} levels`, t.start, t.end, "depth");
    }
  }

  private leave(): void {
    this.depth -= 1;
  }

  // --- grammar ---

  private expr(): Node {
    const test = this.or();
    if (this.isPunct("?")) {
      this.pos += 1;
      const yes = this.expr();
      this.expectPunct(":");
      const no = this.expr();
      return { k: "cond", test, yes, no, s: test.s, e: no.e };
    }
    return test;
  }

  private or(): Node {
    let left = this.and();
    while (this.isKeyword("or") || this.isOp("||")) {
      this.pos += 1;
      const right = this.and();
      left = { k: "bin", op: "or", left, right, s: left.s, e: right.e };
    }
    return left;
  }

  private and(): Node {
    let left = this.not();
    while (this.isKeyword("and") || this.isOp("&&")) {
      this.pos += 1;
      const right = this.not();
      left = { k: "bin", op: "and", left, right, s: left.s, e: right.e };
    }
    return left;
  }

  private not(): Node {
    const prefixes: Tok[] = [];
    while (this.isKeyword("not") || this.isOp("!")) {
      prefixes.push(this.next());
    }
    let node = this.compare();
    for (let i = prefixes.length - 1; i >= 0; i -= 1) {
      const t = prefixes[i];
      if (t !== undefined) {
        node = { k: "un", op: "not", arg: node, s: t.start, e: node.e };
      }
    }
    return node;
  }

  private compare(): Node {
    const left = this.sum();
    const t = this.peek();
    if (t.kind === "op" && COMPARISONS.has(t.text)) {
      this.pos += 1;
      const right = this.sum();
      return { k: "bin", op: t.text as BinOp, left, right, s: left.s, e: right.e };
    }
    return left;
  }

  private sum(): Node {
    let left = this.product();
    while (this.isOp("+", "-")) {
      const op = this.next().text as BinOp;
      const right = this.product();
      left = { k: "bin", op, left, right, s: left.s, e: right.e };
    }
    return left;
  }

  private product(): Node {
    let left = this.unary();
    while (this.isOp("*", "/", "%")) {
      const op = this.next().text as BinOp;
      const right = this.unary();
      left = { k: "bin", op, left, right, s: left.s, e: right.e };
    }
    return left;
  }

  private unary(): Node {
    const signs: Tok[] = [];
    while (this.isOp("-")) {
      signs.push(this.next());
    }
    let node = this.power();
    for (let i = signs.length - 1; i >= 0; i -= 1) {
      const t = signs[i];
      if (t !== undefined) {
        node = { k: "un", op: "-", arg: node, s: t.start, e: node.e };
      }
    }
    return node;
  }

  private power(): Node {
    const base = this.postfix();
    if (this.isOp("^")) {
      this.pos += 1;
      const exponent = this.unary();
      return { k: "bin", op: "^", left: base, right: exponent, s: base.s, e: exponent.e };
    }
    return base;
  }

  private postfix(): Node {
    let node = this.primary();
    while (this.isPunct(".")) {
      this.pos += 1;
      const name = this.peek();
      const nameOk = name.kind === "ident" || (name.kind === "kw" && ["and", "or", "not"].includes(name.text));
      if (!nameOk) {
        this.unexpected(name, "a function name");
      }
      this.pos += 1;
      const open = this.expectPunct("(");
      const { args, end } = this.callArguments(open);
      node = this.makeCall(name.text, [node, ...args], node.s, end, name.start, name.end);
    }
    return node;
  }

  private primary(): Node {
    const t = this.peek();
    switch (t.kind) {
      case "num":
        if (t.err) {
          throw new Fail(t.err);
        }
        this.pos += 1;
        return { k: "num", v: Number(t.value), s: t.start, e: t.end };
      case "str":
        if (t.err) {
          throw new Fail(t.err);
        }
        this.pos += 1;
        return { k: "str", v: String(t.value), s: t.start, e: t.end };
      case "kw":
        if (t.text === "true" || t.text === "false") {
          this.pos += 1;
          return { k: "bool", v: t.text === "true", s: t.start, e: t.end };
        }
        if ((t.text === "and" || t.text === "or") && this.isPunct("(", 1)) {
          this.pos += 1;
          return this.plainCall(t);
        }
        return this.unexpected(t, "an expression");
      case "ident":
        this.pos += 1;
        if (this.isPunct("(")) {
          return this.plainCall(t);
        }
        return { k: "var", name: t.text, s: t.start, e: t.end };
      case "punct":
        if (t.text === "(") {
          this.pos += 1;
          this.enter(t);
          const inner = this.expr();
          this.expectPunct(")");
          this.leave();
          return inner;
        }
        if (t.text === "[") {
          this.pos += 1;
          this.enter(t);
          const items: Node[] = [];
          if (!this.isPunct("]")) {
            for (;;) {
              items.push(this.expr());
              if (this.isPunct(",")) {
                this.pos += 1;
                continue;
              }
              break;
            }
          }
          const close = this.expectPunct("]");
          this.leave();
          return { k: "list", items, s: t.start, e: close.end };
        }
        return this.unexpected(t, "an expression");
      default:
        return this.unexpected(t, "an expression");
    }
  }

  /** `name(` has been consumed up to the name: parses the parenthesised arguments. */
  private plainCall(name: Tok): Node {
    const open = this.expectPunct("(");
    const { args, end } = this.callArguments(open);
    return this.makeCall(name.text, args, name.start, end, name.start, name.end);
  }

  /** After `(`: the arguments and the closing parenthesis. */
  private callArguments(open: Tok): { args: Node[]; end: number } {
    this.enter(open);
    const args: Node[] = [];
    if (!this.isPunct(")")) {
      for (;;) {
        args.push(this.expr());
        if (this.isPunct(",")) {
          this.pos += 1;
          continue;
        }
        break;
      }
    }
    const close = this.expectPunct(")");
    this.leave();
    return { args, end: close.end };
  }

  private makeCall(name: string, args: Node[], s: number, e: number, ns: number, ne: number): Node {
    validateCall(name, args, s, e);
    return { k: "call", name, args, s, e, ns, ne };
  }
}

/** The checks the grammar makes on the shape of a call: a `prop` argument is a text literal, a `let` name an identifier. */
function validateCall(name: string, args: readonly Node[], start: number, end: number): void {
  if (name === "prop") {
    if (args.length === 0 || args.length > 2) {
      failAt("syntax", "prop takes the name of a property in quotes", start, end, "text literal");
    }
    const last = args[args.length - 1];
    if (last !== undefined && last.k !== "str") {
      failAt("syntax", "the argument of prop must be a text in quotes", last.s, last.e, "text literal");
    }
  }
  if (name === "let" && args.length === 3) {
    checkLetName(args[0]);
  }
  if (name === "lets" && args.length >= 3 && args.length % 2 === 1) {
    for (let i = 0; i < args.length - 1; i += 2) {
      checkLetName(args[i]);
    }
  }
}

function checkLetName(arg: Node | undefined): void {
  if (arg === undefined) {
    return;
  }
  if (arg.k !== "var") {
    failAt("syntax", "a name is expected here (no quotes)", arg.s, arg.e, "identifier");
  }
  if (isReservedName(arg.name)) {
    failAt("syntax", `"${arg.name}" is a function or a reserved name and cannot be a variable`, arg.s, arg.e, arg.name);
  }
}

/** Parses an expression of any form. Throws a `Fail`. */
export function parseText(text: string): ParsedAst {
  const chars = Array.from(text);
  if (chars.length > MAX_FORMULA_CHARS) {
    failAt("too_long", `longer than ${MAX_FORMULA_CHARS} characters`, MAX_FORMULA_CHARS, chars.length, "length");
  }
  const toks: Tok[] = [];
  for (const t of scan(chars)) {
    if (t.kind !== "comment") {
      toks.push(t);
    } else if (t.err) {
      toks.push({ kind: "error", start: t.start, end: t.end, text: t.text, err: t.err });
    }
  }
  if (toks.length === 0) {
    failAt("syntax", "the expression is empty", 0, 0, "expression");
  }
  try {
    return { text, ast: new Parser(toks, chars.length).parse() };
  } catch (error) {
    if (error instanceof RangeError) {
      throw new Fail(stackError(chars.length));
    }
    throw error;
  }
}

const CACHE_LIMIT = 500;
const cache = new Map<string, ParsedAst | FormulaError>();

/** `parseText` with a cache by text (an expression is parsed again on every read); errors are values here. */
export function parseCached(text: string): ParsedAst | FormulaError {
  const hit = cache.get(text);
  if (hit !== undefined) {
    return hit;
  }
  let result: ParsedAst | FormulaError;
  try {
    result = parseText(text);
  } catch (error) {
    if (error instanceof Fail) {
      result = error.error;
    } else {
      throw error;
    }
  }
  if (cache.size >= CACHE_LIMIT) {
    cache.clear();
  }
  cache.set(text, result);
  return result;
}

/** Whether `parseCached` answered an error. */
export function isParseError(value: ParsedAst | FormulaError): value is FormulaError {
  return "code" in value;
}

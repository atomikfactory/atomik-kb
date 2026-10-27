// The in-memory twin of `kb_import::appflowy` (ADR-0039, `docs/architecture/appflowy-import.md`, 2 to 4),
// with REDUCED FIDELITY: it reads the JSON of an AppFlowy "Back up your workspace" archive and plans it
// like any other source (`./memory-import-plan`): the view tree, the spaces as planned space roots, text
// documents, databases as pages with a view block and one child page per row, the files they need and
// the images they keep on a server. Clean room (ADR-0010): the format is the one described in the design
// document, learnt from an archive; nothing here comes from AppFlowy's code.
//
// What is read: `metadata.json` and `relation_map.json` (the views: name, layout, parent, children, icon,
// `extra`), `collab_jsons/documents/<id>.json` and `collab_jsons/databases/<view id>.json`; the shell gives
// the binary paths (`local_files/<name>`). What is mapped: paragraphs, headings (1 to 3), bulleted,
// numbered and to-do lists with their nesting, quotes, callouts, toggles, code, dividers, equations, images
// and files (a `local_files` file is a needed file, an `http(s)` image a remote file), sub-page and inline
// database blocks as page links, link previews as links, outlines; in text, bold, italic, underline,
// strikethrough, code, links, page mentions, date mentions and inline equations. Databases: grid, board
// (grouped by the view's group field) and, as a table, calendars; fields as properties (text, number,
// date, select, multi-select, checkbox, URL, checklist, files, and the rest as text), rows as child pages
// with the row document found by its derived id. Not read (dropped with `UnsupportedContent`): tables,
// columns, AI meetings, colours, covers, icons other than emoji, filters and sorts, relations and views
// beyond the first of a database (a page linking to it).

import type { ImportWarning } from "./generated/ImportWarning";
import type { ImportWarningCode } from "./generated/ImportWarningCode";
import type { OptionColor } from "./generated/OptionColor";
import type { PropertyKind } from "./generated/PropertyKind";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { isEmoji, parseBool, parseNumber, titleOf } from "./memory-import-text";
import type { Cell, ImportPlan, PlannedCover, PlannedPage, PlannedValue, RemoteFile, Registry } from "./memory-import-plan";
import { uuidV5 } from "./uuid-v5";

/** What {@link planAppFlowy} reads. */
export interface AppFlowyInput {
  /** The text files by normalised path (the wrapping folder stripped). */
  texts: ReadonlyMap<string, string>;
  /** Binary paths: normalised to as given. */
  binaries: ReadonlyMap<string, string>;
  /** A normalised path as the shell gave it, for warnings. */
  shown: (path: string) => string;
  registry: Registry;
  maxPages: number;
}

type Rec = Record<string, unknown>;

function isRec(value: unknown): value is Rec {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function str(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function list(value: unknown): unknown[] {
  return Array.isArray(value) ? (value as unknown[]) : [];
}

function parseJson(text: string | undefined): unknown {
  if (text === undefined) {
    return undefined;
  }
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return undefined;
  }
}

/** A value that is JSON text or already the parsed value. */
function jsonOf(value: unknown): unknown {
  return typeof value === "string" ? parseJson(value) : value;
}

const RELATION_MAP = "relation_map.json";
const METADATA = "metadata.json";

/** Whether the files are an AppFlowy backup: a `relation_map.json` with a `views` object next to a `metadata.json` (4.1). */
export function isAppFlowyArchive(texts: ReadonlyMap<string, string>): boolean {
  if (!texts.has(METADATA)) {
    return false;
  }
  const map = parseJson(texts.get(RELATION_MAP));
  return isRec(map) && isRec(map.views);
}

/**
 * The folder prefix (`""` or `"Name/"`) under which an archive's root files are, when the source is an
 * AppFlowy backup (the root, after one wrapping folder, holds `relation_map.json` and `metadata.json`);
 * `undefined` for any other source. The shell uses it to read every `.json` file as text and to skip
 * `collab_objects/` (6).
 */
export function appFlowyPrefix(paths: readonly string[]): string | undefined {
  const first = paths[0]?.split("/")[0];
  const wrapped = first !== undefined && paths.every((path) => path.includes("/") && path.split("/")[0] === first);
  const prefix = wrapped ? `${first}/` : "";
  return paths.includes(`${prefix}${RELATION_MAP}`) && paths.includes(`${prefix}${METADATA}`) ? prefix : undefined;
}

// ----- the view tree --------------------------------------------------------------------------

interface View {
  id: string;
  name: string;
  layout: number;
  parent: string;
  children: string[];
  object: string;
  extra: Rec;
  icon: { ty: number; value: string } | undefined;
}

function readView(key: string, raw: unknown): View | undefined {
  if (!isRec(raw)) {
    return undefined;
  }
  const extra = jsonOf(raw.extra);
  const icon = isRec(raw.icon) ? raw.icon : undefined;
  return {
    id: str(raw.view_id) ?? key,
    name: str(raw.name) ?? "",
    layout: typeof raw.layout === "number" ? raw.layout : 0,
    parent: str(raw.parent_id) ?? "",
    children: list(raw.children).filter((child): child is string => typeof child === "string"),
    object: str(raw.collab_object_id) ?? "",
    extra: isRec(extra) ? extra : {},
    icon:
      icon !== undefined && typeof icon.ty === "number" && typeof icon.value === "string"
        ? { ty: icon.ty, value: icon.value }
        : undefined,
  };
}

type NodeKind = "space" | "folder" | "document" | "database" | "linked" | "row";

/** A page in the making. */
interface Node {
  key: number;
  kind: NodeKind;
  view: View | undefined;
  title: string;
  icon: string | undefined;
  /** The cover of the view (ADR-0040), when it maps to one. */
  cover: PlannedCover | undefined;
  parent: Node | null;
  children: Node[];
  blocks: JsonValue[];
  values: PlannedValue[];
  /** The path of the file the page is read from, for warnings. */
  path: string;
  /** A row's document, read once every page exists (its links resolve to them). */
  document: string | undefined;
}

const MAX_TITLE = 500;

/** What a view's `extra.cover` is, by its type: AppFlowy's cover types are none, colour, file and asset (a built-in picture). */
type CoverKind = "color" | "file" | "other";

function coverKind(type: string): CoverKind {
  switch (type.trim().toLowerCase().split(".").at(-1)) {
    case "1":
    case "color":
    case "purecolor":
      return "color";
    case "2":
    case "file":
    case "customimage":
    case "unsplashimage":
    case "unsplash":
      return "file";
    default:
      return "other";
  }
}

/**
 * The palette colour nearest to an `0xAARRGGBB` (or `#RRGGBB`) background colour, by hue (the Rust reader's `palette`
 * for backgrounds); `undefined` for transparent and near-white colours and for what is not a colour. Greys are `gray`.
 */
export function paletteBackground(argb: string): string | undefined {
  const text = argb.trim();
  const digits = /^(?:0x|#)/iu.test(text) ? text.replace(/^(?:0x|#)/iu, "") : text;
  if (!/^[0-9a-f]+$/iu.test(digits) || (digits.length !== 6 && digits.length !== 8)) {
    return undefined;
  }
  const value = Number.parseInt(digits, 16);
  const r = (value >> 16) & 0xff;
  const g = (value >> 8) & 0xff;
  const b = value & 0xff;
  const a = digits.length === 8 ? Math.floor(value / 2 ** 24) & 0xff : 0xff;
  if (a < 16) {
    return undefined;
  }
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const chroma = max - min;
  if (chroma < 24) {
    return max / 255 < 0.97 ? "gray" : undefined;
  }
  let hue: number;
  if (max === r) {
    hue = 60 * (((((g - b) / chroma) % 6) + 6) % 6);
  } else if (max === g) {
    hue = 60 * ((b - r) / chroma + 2);
  } else {
    hue = 60 * ((r - g) / chroma + 4);
  }
  const lightness = (max + min) / 510;
  if (hue < 12 || hue >= 348) {
    return "red";
  }
  if (hue < 45 && lightness < 0.35) {
    return "brown";
  }
  if (hue < 40) {
    return "orange";
  }
  if (hue < 70) {
    return "yellow";
  }
  if (hue < 165) {
    return "green";
  }
  if (hue < 255) {
    return "blue";
  }
  return hue < 300 ? "purple" : "pink";
}

// ----- delta text -----------------------------------------------------------------------------

type Obj = { [key: string]: JsonValue };

/** The kind of a value of an AppFlowy field (`field_type`) as a planned property, or `undefined` when it is not imported. */
function fieldKind(fieldType: number): PropertyKind | undefined {
  switch (fieldType) {
    case 1:
    case 13:
      return "number";
    case 2:
      return "date";
    case 3:
      return "select";
    case 4:
      return "multi_select";
    case 5:
      return "checkbox";
    case 6:
      return "url";
    case 7:
      return "checklist";
    case 8:
    case 9:
      return undefined;
    case 14:
      return "files";
    default:
      return "text";
  }
}

const OPTION_COLORS_BY_NAME: Record<string, OptionColor> = {
  purple: "purple",
  pink: "pink",
  lightpink: "pink",
  orange: "orange",
  yellow: "yellow",
  lime: "green",
  green: "green",
  aqua: "blue",
  blue: "blue",
};

const MAX_HEADING = 3;
const LIST_TYPES = new Set(["bulleted_list", "numbered_list", "todo_list"]);

/** A date (seconds since the epoch, or milliseconds) as `YYYY-MM-DD`, with the time of day when asked. */
function isoOf(stamp: unknown, withTime: boolean): string | undefined {
  const number = typeof stamp === "number" ? stamp : typeof stamp === "string" && /^-?\d+$/u.test(stamp.trim()) ? Number(stamp) : undefined;
  if (number === undefined || !Number.isFinite(number)) {
    return undefined;
  }
  const millis = Math.abs(number) > 1e11 ? number : number * 1000;
  const date = new Date(millis);
  if (Number.isNaN(date.getTime())) {
    return undefined;
  }
  const iso = date.toISOString();
  return withTime ? `${iso.slice(0, 10)} ${iso.slice(11, 16)}` : iso.slice(0, 10);
}

/** The reader of one archive. */
class Reader {
  readonly #input: AppFlowyInput;
  readonly warnings: ImportWarning[] = [];
  readonly #views = new Map<string, View>();
  readonly #nodes = new Map<string, Node>();
  readonly #rowDocuments = new Map<string, Node>();
  readonly #once = new Set<string>();
  /** The derived ids of the rows' documents: views with these ids are not pages. */
  readonly #documentIds = new Set<string>();
  /** The first view of each database holds its rows. */
  readonly #databaseOwner = new Map<string, Node>();
  readonly #placeholders = new Map<string, string>();
  readonly #localPlaceholders = new Map<string, string>();
  readonly #remoteKeys = new Map<string, string>();
  readonly neededFiles: string[] = [];
  readonly remoteFiles: RemoteFile[] = [];
  /** A needed file (given path) or remote file (key) to the key of the first node that uses it. */
  readonly #firstUser = new Map<string, number>();
  #nextKey = 0;

  constructor(input: AppFlowyInput) {
    this.#input = input;
  }

  get placeholders(): Map<string, string> {
    return this.#placeholders;
  }

  get firstUsers(): ReadonlyMap<string, number> {
    return this.#firstUser;
  }

  #warn(path: string, code: ImportWarningCode, detail?: string): void {
    const shown = this.#input.shown(path);
    this.warnings.push(detail === undefined ? { path: shown, code } : { path: shown, code, detail });
  }

  /** A warning that is given once for `key` (per page and type, or per import). */
  #warnOnce(key: string, path: string, code: ImportWarningCode, detail?: string): void {
    if (!this.#once.has(key)) {
      this.#once.add(key);
      this.#warn(path, code, detail);
    }
  }

  // ----- reading the archive ------------------------------------------------------------------

  read(): Node[] {
    const map = parseJson(this.#input.texts.get(RELATION_MAP));
    const views = isRec(map) && isRec(map.views) ? map.views : {};
    for (const [key, raw] of Object.entries(views)) {
      const view = readView(key, raw);
      if (view !== undefined && !this.#views.has(view.id)) {
        this.#views.set(view.id, view);
      }
    }
    const databases = this.#readDatabases();
    const roots: Node[] = [];
    const planned = new Set<string>();

    // The spaces first, then what no space reaches.
    const spaceViews = [...this.#views.values()].filter((view) => view.extra.is_space === true);
    for (const view of spaceViews) {
      planned.add(view.id);
    }
    for (const view of spaceViews) {
      const root = this.#makeNode("space", view, null);
      roots.push(root);
      this.#addChildren(root, view, planned, databases);
    }
    const left = [...this.#views.values()].filter((view) => !planned.has(view.id) && !this.#isRowDocument(view.id));
    const leftIds = new Set(left.map((view) => view.id));
    const tops = left.filter((view) => !leftIds.has(view.parent) || view.parent === view.id);
    if (left.length > 0) {
      const other = this.#folder("Other pages");
      roots.push(other);
      for (const view of [...tops, ...left]) {
        if (!planned.has(view.id)) {
          this.#addView(other, view, planned, databases);
        }
      }
    }
    return roots;
  }

  /** Every database JSON of the archive by view id; the derived ids of their row documents are remembered. */
  #readDatabases(): Map<string, Rec> {
    const found = new Map<string, Rec>();
    for (const view of this.#views.values()) {
      const parsed = parseJson(this.#input.texts.get(`collab_jsons/databases/${view.id}.json`));
      if (isRec(parsed)) {
        found.set(view.id, parsed);
        for (const row of list(parsed.rows)) {
          const id = isRec(row) ? str(row.id) : undefined;
          const document = id === undefined ? undefined : uuidV5(id, "document_id");
          if (document !== undefined) {
            this.#documentIds.add(document);
          }
        }
      }
    }
    return found;
  }

  #isRowDocument(viewId: string): boolean {
    return this.#documentIds.has(viewId) && !this.#nodes.has(viewId);
  }

  #makeNode(kind: NodeKind, view: View | undefined, parent: Node | null): Node {
    const title = titleOf(view?.name ?? "").slice(0, MAX_TITLE) || "Untitled";
    const node: Node = {
      key: this.#nextKey,
      kind,
      view,
      title,
      icon: view === undefined ? undefined : this.#iconOf(view, kind === "space"),
      cover: undefined,
      parent,
      children: [],
      blocks: [],
      values: [],
      path: RELATION_MAP,
      document: undefined,
    };
    this.#nextKey += 1;
    if (view !== undefined) {
      this.#nodes.set(view.id, node);
      if (view.extra.cover !== undefined && view.extra.cover !== null) {
        node.cover = this.#coverOf(view.extra.cover, node);
        if (node.cover === undefined) {
          this.#warnOnce("cover", RELATION_MAP, "unsupportedContent", "cover");
        }
      }
    }
    parent?.children.push(node);
    return node;
  }

  /**
   * The cover of a view (ADR-0040), when it can be one: a colour of AppFlowy's (by hue, to the nearest of the nine
   * palette colours), an image of the archive's `local_files` or an `https` image (a remote file, downloaded once by
   * the shell). AppFlowy's built-in covers and anything unreadable give none (the caller warns once per import).
   */
  #coverOf(raw: unknown, node: Node): PlannedCover | undefined {
    const parsed = jsonOf(raw);
    if (!isRec(parsed)) {
      return undefined;
    }
    const type = typeof parsed.type === "string" || typeof parsed.type === "number" ? String(parsed.type) : "";
    const value = (typeof parsed.value === "string" || typeof parsed.value === "number" ? String(parsed.value) : "").trim();
    if (value === "") {
      return undefined;
    }
    const kind = coverKind(type);
    if (kind === "color") {
      const name = paletteBackground(value);
      return name === undefined ? undefined : { kind: "named", value: `color:${name}` };
    }
    if (kind === "file") {
      const local = this.#localFile(value, node, true);
      if (local !== undefined) {
        return { kind: "file", path: local };
      }
      return /^https:\/\//iu.test(value) && value.length <= 2048 ? { kind: "file", path: this.#remoteKey(value, node) } : undefined;
    }
    return undefined;
  }

  #folder(title: string): Node {
    const node = this.#makeNode("folder", undefined, null);
    node.title = title;
    return node;
  }

  /** The emoji icon of a view (4.2); an icon of another kind is dropped, once per import. */
  #iconOf(view: View, space: boolean): string | undefined {
    const icon = view.icon;
    if (icon !== undefined && icon.value !== "") {
      if (icon.ty === 0 && isEmoji(icon.value)) {
        return icon.value;
      }
      this.#warnOnce("icon", RELATION_MAP, "unsupportedContent", "icon");
    }
    const spaceIcon = str(view.extra.space_icon);
    return space && spaceIcon !== undefined && isEmoji(spaceIcon) ? spaceIcon : undefined;
  }

  #addChildren(parent: Node, view: View, planned: Set<string>, databases: Map<string, Rec>): void {
    for (const id of view.children) {
      const child = this.#views.get(id);
      if (child !== undefined && !planned.has(id) && !this.#isRowDocument(id)) {
        this.#addView(parent, child, planned, databases);
      }
    }
  }

  /** One view and everything below it. */
  #addView(parent: Node, view: View, planned: Set<string>, databases: Map<string, Rec>): void {
    planned.add(view.id);
    const database = databases.get(view.id);
    const databaseId = str(view.extra.database_id);
    let node: Node;
    if (view.layout !== 0 && database !== undefined) {
      const first = databaseId === undefined ? undefined : this.#databaseOwner.get(databaseId);
      if (first === undefined) {
        node = this.#makeNode("database", view, parent);
        if (databaseId !== undefined) {
          this.#databaseOwner.set(databaseId, node);
        }
        this.#addRows(node, view, database);
      } else {
        node = this.#makeNode("linked", view, parent);
        node.blocks = [{ type: "pageLink", attrs: { pageId: `plan:${String(first.key)}`, fallback: first.title } }];
        this.#warn(`collab_jsons/databases/${view.id}.json`, "unsupportedContent", "linked view");
      }
    } else {
      node = this.#makeNode("document", view, parent);
    }
    this.#addChildren(node, view, planned, databases);
  }

  // ----- databases ----------------------------------------------------------------------------

  /** The fields of a database as `[id, name, field type, options]`, the primary one first. */
  #fieldsOf(database: Rec): { id: string; name: string; type: number; primary: boolean; options: Map<string, string>; colors: Map<string, OptionColor> }[] {
    const fields = list(database.fields).flatMap((raw) => {
      if (!isRec(raw) || str(raw.id) === undefined) {
        return [];
      }
      const type = typeof raw.field_type === "number" ? raw.field_type : 0;
      const typeOptions = isRec(raw.type_options) ? raw.type_options : {};
      const entry = typeOptions[String(type)];
      const content = isRec(entry) ? jsonOf(entry.content) : undefined;
      const options = new Map<string, string>();
      const colors = new Map<string, OptionColor>();
      for (const option of isRec(content) ? list(content.options) : []) {
        if (isRec(option) && str(option.id) !== undefined && str(option.name) !== undefined) {
          const name = str(option.name) ?? "";
          options.set(str(option.id) ?? "", name);
          const color = OPTION_COLORS_BY_NAME[(str(option.color) ?? "").toLowerCase()];
          if (color !== undefined) {
            colors.set(name, color);
          }
        }
      }
      return [{ id: str(raw.id) ?? "", name: titleOf(str(raw.name) ?? "") || "Untitled", type, primary: raw.is_primary === true, options, colors }];
    });
    const primary = fields.find((field) => field.primary) ?? fields[0];
    return primary === undefined ? [] : [{ ...primary, primary: true }, ...fields.filter((field) => field !== primary).map((field) => ({ ...field, primary: false }))];
  }

  #addRows(node: Node, view: View, database: Rec): void {
    const fields = this.#fieldsOf(database);
    const primary = fields.find((field) => field.primary);
    const rows = list(database.rows).filter(isRec);
    const byId = new Map(rows.map((row) => [str(row.id) ?? "", row]));
    // The view's order of rows first, then the rest in the order of the rows.
    const config = list(database.views).find((candidate) => isRec(candidate) && candidate.id === view.id);
    const viewConfig = isRec(config) ? config : list(database.views).find(isRec);
    const ordered: Rec[] = [];
    const seen = new Set<string>();
    for (const entry of list(viewConfig?.row_orders)) {
      const id = isRec(entry) ? str(entry.id) : undefined;
      const row = id === undefined ? undefined : byId.get(id);
      if (id !== undefined && row !== undefined && !seen.has(id)) {
        seen.add(id);
        ordered.push(row);
      }
    }
    for (const row of rows) {
      const id = str(row.id) ?? "";
      if (!seen.has(id)) {
        seen.add(id);
        ordered.push(row);
      }
    }
    // The properties: every field but the primary one (the title) and the computed ones.
    const properties = new Map<string, number>();
    for (const field of fields) {
      const kind = fieldKind(field.type);
      if (field.primary || kind === undefined) {
        continue;
      }
      if (field.type === 10) {
        this.#warnOnce(`relation:${field.name}`, `collab_jsons/databases/${view.id}.json`, "unsupportedContent", field.name);
        continue;
      }
      const at = this.#input.registry.add(field.name, kind, kind === "select" || kind === "multi_select" ? [...field.options.values()] : []);
      const planned = this.#input.registry.properties[at];
      for (const [name, color] of field.colors) {
        const option = planned?.options.find((candidate) => candidate.name === name);
        if (option !== undefined && option.color === undefined) {
          option.color = color;
        }
      }
      properties.set(field.id, at);
    }
    // The view block (4.3): a board grouped by the view's group field, a table for the rest.
    const layout = view.layout !== 0 ? view.layout : typeof viewConfig?.layout === "number" ? viewConfig.layout : 0;
    const settings = isRec(viewConfig?.field_settings) ? viewConfig.field_settings : {};
    const order = list(viewConfig?.field_orders).flatMap((entry) => (isRec(entry) && str(entry.id) !== undefined ? [str(entry.id) ?? ""] : []));
    const shown = (order.length > 0 ? order : fields.map((field) => field.id)).filter((id) => {
      const setting = settings[id];
      return properties.has(id) && !(isRec(setting) && setting.visibility === 2);
    });
    const attrs: Obj = {
      source: { kind: "children" },
      layout: layout === 2 ? "board" : "table",
      properties: shown.map((id) => `prop:${this.#input.registry.properties[properties.get(id) ?? -1]?.name ?? ""}`),
    };
    if (layout !== 1 && layout !== 2) {
      this.#warn(`collab_jsons/databases/${view.id}.json`, "unsupportedContent", layout === 3 ? "calendar" : String(layout));
    }
    if (layout === 2) {
      const group = list(viewConfig?.group_settings).find(isRec);
      const groupField = group === undefined ? undefined : str(group.field_id);
      const at = groupField === undefined ? undefined : properties.get(groupField);
      const name = at === undefined ? undefined : this.#input.registry.properties[at]?.name;
      if (name !== undefined) {
        attrs.groupBy = `prop:${name}`;
      }
    }
    if (list(viewConfig?.sorts).length > 0 || list(viewConfig?.filters).length > 0) {
      this.#warn(`collab_jsons/databases/${view.id}.json`, "unsupportedContent", "sort or filter");
    }
    node.blocks = [{ type: "pageList", attrs }];

    // The rows.
    const metas = isRec(database.row_metas) ? database.row_metas : {};
    for (const row of ordered) {
      const id = str(row.id) ?? "";
      const cells = isRec(row.cells) ? row.cells : {};
      const item = this.#makeNode("row", undefined, node);
      item.path = `collab_jsons/databases/${view.id}.json`;
      const titleCell = primary === undefined ? undefined : cells[primary.id];
      item.title = titleOf(isRec(titleCell) ? (str(titleCell.data) ?? "") : "").slice(0, MAX_TITLE) || "Untitled";
      const meta = metas[id];
      const icon = isRec(meta) ? str(meta.icon_url) : undefined;
      item.icon = icon !== undefined && isEmoji(icon) ? icon : undefined;
      for (const field of fields) {
        const at = properties.get(field.id);
        const cell = cells[field.id];
        if (at !== undefined && isRec(cell)) {
          this.#addValue(item, field, at, cell);
        }
      }
      const document = uuidV5(id, "document_id");
      if (document !== undefined) {
        this.#rowDocuments.set(document, item);
        const empty = isRec(meta) && meta.is_document_empty === true;
        const text = this.#input.texts.get(`collab_jsons/documents/${document}.json`);
        if (!empty && text !== undefined) {
          item.document = `collab_jsons/documents/${document}.json`;
        }
      }
    }
  }

  /** One cell of a row as a value of its planned property (4.3's table). */
  #addValue(row: Node, field: { name: string; type: number; options: Map<string, string> }, property: number, cell: Rec): void {
    const data = cell.data;
    const text = typeof data === "string" ? data : typeof data === "number" ? String(data) : "";
    const push = (value: Cell): void => {
      row.values.push({ property, cell: value });
    };
    const invalid = (): void => {
      this.#warn(row.path, "invalidValue", field.name);
    };
    if (text.trim() === "" && field.type !== 14) {
      return;
    }
    switch (field.type) {
      case 1:
      case 13: {
        const number = parseNumber(text);
        if (number === undefined) {
          invalid();
        } else {
          push({ text, number });
        }
        return;
      }
      case 2: {
        const iso = isoOf(data, cell.include_time === true);
        if (iso === undefined) {
          invalid();
        } else {
          push({ text: iso });
        }
        return;
      }
      case 3:
      case 4: {
        const names: string[] = [];
        for (const id of text.split(",").map((part) => part.trim()).filter((part) => part !== "")) {
          const name = field.options.get(id);
          if (name === undefined) {
            invalid();
            return;
          }
          names.push(name);
        }
        if (names.length > 0) {
          push(field.type === 3 ? { text: names[0] ?? "", items: [names[0] ?? ""] } : { text: names.join(", "), items: names });
        }
        return;
      }
      case 5: {
        const checked = parseBool(text) ?? (text === "1" ? true : text === "0" ? false : undefined);
        if (checked === undefined) {
          invalid();
        } else {
          push({ text, checked });
        }
        return;
      }
      case 6: {
        const object = jsonOf(data);
        const url = isRec(object) ? (str(object.data) ?? "") : text;
        if (url.trim() !== "") {
          push({ text: url });
        }
        return;
      }
      case 7: {
        const checklist = jsonOf(data);
        const options = isRec(checklist) ? list(checklist.options) : [];
        const selected = new Set(isRec(checklist) ? list(checklist.selected_option_ids) : []);
        const items = options.flatMap((option) =>
          isRec(option) && str(option.name) !== undefined ? [{ text: str(option.name) ?? "", done: selected.has(option.id) }] : [],
        );
        if (items.length > 0) {
          push({ text: items.map((item) => item.text).join(", "), checklist: items });
        }
        return;
      }
      case 14: {
        const files: string[] = [];
        for (const entry of list(jsonOf(data))) {
          const url = isRec(entry) ? (str(entry.url) ?? "") : "";
          const path = /^https?:\/\//iu.test(url) ? undefined : this.#localFile(url, row);
          if (path !== undefined) {
            files.push(path);
          } else if (url !== "") {
            this.#warnOnce(`media:${field.name}`, row.path, "unsupportedContent", field.name);
          }
        }
        if (files.length > 0) {
          push({ text: files.join(", "), files });
        }
        return;
      }
      default:
        push({ text });
    }
  }

  // ----- files --------------------------------------------------------------------------------

  /** The needed local file named by `url` (`<view id>/<name>` or `<name>`), as the shell gave its path; `undefined` when the archive lacks it. */
  #localFile(url: string, user: Node, quiet = false): string | undefined {
    const name = url.split("/").at(-1) ?? "";
    const normalised = `local_files/${name}`;
    const given = name === "" ? undefined : this.#input.binaries.get(normalised);
    if (given === undefined) {
      if (name !== "" && !quiet) {
        this.#warn(user.path, "missingFile", name);
      }
      return undefined;
    }
    if (!this.#firstUser.has(given)) {
      this.#firstUser.set(given, user.key);
      this.neededFiles.push(given);
    }
    return given;
  }

  /** The placeholder attachment id a block carries for a needed file (given path) or a remote file (key). */
  #placeholderFor(path: string, remote: boolean): string {
    const known = (remote ? this.#remoteKeys : this.#localPlaceholders).get(path);
    if (known !== undefined) {
      return known;
    }
    const n = this.#placeholders.size.toString(16).padStart(12, "0");
    const placeholder = remote ? `eeeeeeee-eeee-4eee-8eee-${n}` : `ffffffff-ffff-4fff-8fff-${n}`;
    (remote ? this.#remoteKeys : this.#localPlaceholders).set(path, placeholder);
    this.#placeholders.set(placeholder, path);
    return placeholder;
  }

  /** The key of a remote image: listed once per distinct URL, keyed `remote/<n>`. */
  #remoteKey(url: string, user: Node): string {
    let key = this.remoteFiles.find((file) => file.url === url)?.key;
    if (key === undefined) {
      key = `remote/${String(this.remoteFiles.length)}`;
      let host: string;
      try {
        host = new URL(url).hostname;
      } catch {
        host = "";
      }
      this.remoteFiles.push({ key, url, host });
      this.#firstUser.set(key, user.key);
    }
    return key;
  }

  /** A remote image's placeholder attachment id. */
  #remoteFile(url: string, user: Node): string {
    return this.#placeholderFor(this.#remoteKey(url, user), true);
  }

  // ----- documents ----------------------------------------------------------------------------

  /** The content of every document page, once all the pages exist (their keys resolve the links). */
  convertAll(nodes: readonly Node[]): void {
    for (const node of nodes) {
      if (node.document !== undefined) {
        node.path = node.document;
        node.blocks = this.#convertDocument(node, parseJson(this.#input.texts.get(node.document)), node.document);
      } else if (node.kind === "document" && node.view !== undefined) {
        const id = [node.view.object, node.view.id].find((candidate) => candidate !== "" && this.#input.texts.has(`collab_jsons/documents/${candidate}.json`));
        if (id === undefined) {
          this.#warn(`collab_jsons/documents/${node.view.object === "" ? node.view.id : node.view.object}.json`, "missingFile", node.view.name);
        } else {
          const path = `collab_jsons/documents/${id}.json`;
          node.path = path;
          node.blocks = this.#convertDocument(node, parseJson(this.#input.texts.get(path)), path);
        }
      } else if (node.kind === "database" && node.view !== undefined) {
        node.path = `collab_jsons/databases/${node.view.id}.json`;
      }
    }
  }

  #convertDocument(node: Node, raw: unknown, path: string): JsonValue[] {
    if (!isRec(raw) || !isRec(raw.blocks)) {
      this.#warn(path, "unsupportedContent", "document");
      return [];
    }
    const meta = isRec(raw.meta) ? raw.meta : {};
    const blocks = raw.blocks;
    const childrenMap = isRec(meta.children_map) ? meta.children_map : {};
    const textMap = isRec(meta.text_map) ? meta.text_map : {};
    const rootId = str(raw.page_id) ?? "";
    const rootBlock = isRec(blocks[rootId]) ? blocks[rootId] : Object.values(blocks).find((block): block is Rec => isRec(block) && block.ty === "page");
    if (rootBlock === undefined) {
      return [];
    }
    const warn = (code: ImportWarningCode, detail?: string): void => {
      this.#warn(path, code, detail);
    };
    const dataOf = (block: Rec): Rec => {
      const data = jsonOf(block.data);
      return isRec(data) ? data : {};
    };
    const childIds = (block: Rec): string[] => {
      const id = str(block.children);
      return id === undefined ? [] : list(childrenMap[id]).filter((child): child is string => typeof child === "string");
    };
    const deltaOf = (block: Rec): unknown[] => {
      const external = str(block.external_id);
      const fromMap = block.external_type === "text" && external !== undefined ? jsonOf(textMap[external]) : undefined;
      const delta = fromMap ?? dataOf(block).delta;
      return list(jsonOf(delta));
    };
    const plainOf = (block: Rec): string =>
      deltaOf(block)
        .map((op) => (isRec(op) && typeof op.insert === "string" ? op.insert : ""))
        .join("");
    const target = (viewId: string): Node | undefined => this.#nodes.get(viewId) ?? this.#rowDocuments.get(viewId);

    const inlines = (block: Rec): JsonValue[] => {
      const out: JsonValue[] = [];
      const ops = deltaOf(block);
      // A newline-only insert at the end is dropped (3).
      const lastOp = ops.at(-1);
      if (isRec(lastOp) && lastOp.insert === "\n") {
        ops.pop();
      }
      for (const op of ops) {
        if (!isRec(op) || typeof op.insert !== "string") {
          continue;
        }
        const attrs = isRec(op.attributes) ? op.attributes : {};
        const mention = isRec(attrs.mention) ? attrs.mention : undefined;
        if (mention !== undefined) {
          const kind = str(mention.type);
          if (kind === "page") {
            const page = target(str(mention.page_id) ?? "");
            if (page === undefined) {
              warn("unresolvedLink", str(mention.page_id));
            } else {
              out.push({ type: "pageRef", attrs: { pageId: `plan:${String(page.key)}`, fallback: page.title } });
            }
          } else if (kind === "date") {
            const date = isoOf(mention.date, false) ?? (typeof mention.date === "string" && /^\d{4}-\d{2}-\d{2}/u.test(mention.date) ? mention.date.slice(0, 10) : undefined);
            if (date !== undefined) {
              out.push({ type: "dateMention", attrs: { date } });
            }
          } else if (kind === "externalLink" && typeof mention.url === "string") {
            out.push(...textNodes(op.insert === "$" ? mention.url : op.insert, [linkMark(mention.url)]));
          } else if (kind === "person" && typeof mention.name === "string") {
            out.push(...textNodes(mention.name, []));
          }
          continue;
        }
        if (typeof attrs.formula === "string") {
          out.push({ type: "mathInline", attrs: { latex: attrs.formula } });
          continue;
        }
        const marks: JsonValue[] = [];
        if (attrs.bold === true) {
          marks.push({ type: "bold" });
        }
        if (attrs.italic === true) {
          marks.push({ type: "italic" });
        }
        if (attrs.underline === true) {
          marks.push({ type: "underline" });
        }
        if (attrs.strikethrough === true) {
          marks.push({ type: "strike" });
        }
        if (attrs.code === true) {
          marks.push({ type: "code" });
        }
        const href = str(attrs.href);
        if (href !== undefined) {
          const page = attrs.is_page_link === true ? target(href) : undefined;
          if (page !== undefined) {
            out.push({ type: "pageRef", attrs: { pageId: `plan:${String(page.key)}`, fallback: op.insert } });
            continue;
          }
          if (/^(?:https?:\/\/|mailto:|tel:)/iu.test(href)) {
            marks.push(linkMark(href));
          }
        }
        out.push(...textNodes(op.insert, marks));
      }
      return out;
    };

    const paragraph = (block: Rec): JsonValue => {
      const content = inlines(block);
      return content.length === 0 ? { type: "paragraph" } : { type: "paragraph", content };
    };

    const seenTypes = new Set<string>();
    const convert = (block: Rec): JsonValue[] => {
      const type = str(block.ty) ?? "";
      const data = dataOf(block);
      const children = (): JsonValue[] => convertList(childIds(block));
      switch (type) {
        case "paragraph":
          return [paragraph(block), ...children()];
        case "heading": {
          const level = typeof data.level === "number" ? Math.min(MAX_HEADING, Math.max(1, Math.trunc(data.level))) : 1;
          const content = inlines(block);
          return [{ type: "heading", attrs: { level }, ...(content.length === 0 ? {} : { content }) }, ...children()];
        }
        case "quote":
          return [{ type: "blockquote", content: [paragraph(block), ...children()] }];
        case "callout": {
          const icon = str(data.icon);
          return [
            {
              type: "callout",
              attrs: icon !== undefined && isEmoji(icon) ? { icon } : {},
              content: [paragraph(block), ...children()],
            },
          ];
        }
        case "toggle_list":
        case "toggle_heading": {
          const summary = inlines(block);
          const inner = children();
          // A toggle heading keeps its level, clamped as for headings; a toggle list is a plain toggle (ADR-0040).
          const level =
            type === "toggle_heading"
              ? typeof data.level === "number"
                ? Math.min(MAX_HEADING, Math.max(1, Math.trunc(data.level)))
                : 1
              : undefined;
          return [
            {
              type: "details",
              attrs: { open: data.collapsed !== true },
              content: [
                {
                  type: "detailsSummary",
                  ...(level === undefined ? {} : { attrs: { level } }),
                  ...(summary.length === 0 ? {} : { content: summary }),
                },
                { type: "detailsContent", content: inner.length === 0 ? [{ type: "paragraph" }] : inner },
              ],
            },
          ];
        }
        case "code": {
          const text = plainOf(block);
          const language = str(data.language);
          return [
            {
              type: "codeBlock",
              ...(language === undefined || language === "" ? {} : { attrs: { language: language.toLowerCase() } }),
              ...(text === "" ? {} : { content: [{ type: "text", text }] }),
            },
          ];
        }
        case "divider":
          return [{ type: "horizontalRule" }];
        case "math_equation": {
          const formula = str(data.formula) ?? "";
          return [{ type: "mathBlock", ...(formula === "" ? {} : { content: [{ type: "text", text: formula }] }) }];
        }
        case "outline":
          return [{ type: "tableOfContents" }];
        case "image":
        case "file":
          return [...this.#mediaBlock(node, path, type, data, warn)];
        case "sub_page":
        case "grid":
        case "board":
        case "calendar": {
          const viewId = str(data.view_id) ?? str(data.parent_id) ?? "";
          const page = target(viewId);
          if (page === undefined) {
            warn("unresolvedLink", viewId);
            return [];
          }
          return [{ type: "pageLink", attrs: { pageId: `plan:${String(page.key)}`, fallback: page.title } }];
        }
        case "link_preview":
        case "embed_link": {
          const url = str(data.url) ?? "";
          return /^https?:\/\//iu.test(url) ? [{ type: "paragraph", content: textNodes(url, [linkMark(url)]) }] : [];
        }
        case "page":
          return convertList(childIds(block));
        default: {
          // Anything else: its text as a paragraph, its children flattened, one warning per page and type.
          const key = `${String(node.key)}|${type}`;
          if (!seenTypes.has(key)) {
            seenTypes.add(key);
            warn("unsupportedContent", type);
          }
          const text = inlines(block);
          return [...(text.length === 0 ? [] : [{ type: "paragraph", content: text }]), ...children()];
        }
      }
    };

    const listItem = (block: Rec): JsonValue => {
      const type = str(block.ty);
      const body = [paragraph(block), ...convertList(childIds(block))];
      return type === "todo_list"
        ? { type: "taskItem", attrs: { checked: dataOf(block).checked === true }, content: body }
        : { type: "listItem", content: body };
    };

    const convertList = (ids: readonly string[]): JsonValue[] => {
      const out: JsonValue[] = [];
      let at = 0;
      while (at < ids.length) {
        const entry = blocks[ids[at] ?? ""];
        if (!isRec(entry)) {
          at += 1;
          continue;
        }
        const type = str(entry.ty) ?? "";
        if (!LIST_TYPES.has(type)) {
          out.push(...convert(entry));
          at += 1;
          continue;
        }
        // Consecutive items of one kind form one list.
        const items: Rec[] = [];
        while (at < ids.length) {
          const next = blocks[ids[at] ?? ""];
          if (!isRec(next) || next.ty !== type) {
            break;
          }
          items.push(next);
          at += 1;
        }
        const content = items.map(listItem);
        if (type === "bulleted_list") {
          out.push({ type: "bulletList", content });
        } else if (type === "todo_list") {
          out.push({ type: "taskList", content });
        } else {
          const start = dataOf(items[0] ?? {}).number;
          out.push({
            type: "orderedList",
            ...(typeof start === "number" && Number.isInteger(start) && start > 1 ? { attrs: { start } } : {}),
            content,
          });
        }
      }
      return out;
    };

    return convertList(childIds(rootBlock));
  }

  /** An `image` or `file` block (4.5): a needed local file, a remote image, a link, or nothing with a warning. */
  #mediaBlock(node: Node, path: string, type: string, data: Rec, warn: (code: ImportWarningCode, detail?: string) => void): JsonValue[] {
    const url = str(data.url) ?? "";
    const name = str(data.name) ?? "";
    if (/^https?:\/\//iu.test(url)) {
      if (type === "image") {
        const attrs: Obj = { attachmentId: this.#remoteFile(url, node), alt: name };
        const align = str(data.align);
        if (align === "left" || align === "center" || align === "right") {
          attrs.align = align;
        }
        if (name === "") {
          delete attrs.alt;
        }
        return [{ type: "image", attrs }];
      }
      return [{ type: "paragraph", content: textNodes(name === "" ? url : name, [linkMark(url)]) }];
    }
    if (url === "") {
      warn("missingFile", name);
      return [];
    }
    const given = this.#localFile(url, { ...node, path });
    if (given === undefined) {
      return [];
    }
    const attrs: Obj = { attachmentId: this.#placeholderFor(given, false) };
    const align = str(data.align);
    if (type === "image" && (align === "left" || align === "center" || align === "right")) {
      attrs.align = align;
    }
    return [{ type, attrs }];
  }
}

function linkMark(href: string): JsonValue {
  return { type: "link", attrs: { href } };
}

/** Text nodes for `text` with `marks`, a newline inside it a hard break; nothing for the empty text. */
function textNodes(text: string, marks: readonly JsonValue[]): JsonValue[] {
  const out: JsonValue[] = [];
  text.split("\n").forEach((line, at) => {
    if (at > 0) {
      out.push({ type: "hardBreak" });
    }
    if (line !== "") {
      out.push(marks.length === 0 ? { type: "text", text: line } : { type: "text", text: line, marks: [...marks] });
    }
  });
  return out;
}

/**
 * Plans an AppFlowy backup (see the module comment): the pages parents first (the spaces are the
 * `spaceRoots`, in order), the properties its databases need, the local files and the remote images it
 * uses, and what it left out. Never throws: what cannot be read is left out with a warning.
 */
export function planAppFlowy(input: AppFlowyInput): ImportPlan {
  const reader = new Reader(input);
  const roots = reader.read();
  const flat: Node[] = [];
  const visit = (node: Node): void => {
    flat.push(node);
    node.children.forEach(visit);
  };
  roots.forEach(visit);
  reader.convertAll(flat);

  let kept = flat;
  if (flat.length > input.maxPages) {
    kept = flat.slice(0, input.maxPages);
    reader.warnings.push({ path: input.shown(flat[input.maxPages]?.path ?? RELATION_MAP), code: "truncated" });
  }
  const at = new Map(kept.map((node, index) => [node, index]));
  const pages: PlannedPage[] = kept.map((node) => ({
    key: node.key,
    parent: node.parent === null ? null : (at.get(node.parent) ?? null),
    title: node.title,
    icon: node.icon,
    cover: node.cover,
    blocks: node.blocks,
    values: node.values,
    path: input.shown(node.path),
  }));
  const byKey = new Map(kept.map((node, index) => [node.key, index]));
  const fileUsers = new Map<string, number>();
  for (const [file, key] of reader.firstUsers) {
    const index = byKey.get(key);
    if (index !== undefined) {
      fileUsers.set(file, index);
    }
  }
  return {
    pages,
    properties: input.registry.properties,
    neededFiles: reader.neededFiles,
    placeholders: reader.placeholders,
    warnings: reader.warnings,
    spaceRoots: roots.flatMap((root) => (at.has(root) ? [at.get(root) ?? 0] : [])),
    remoteFiles: reader.remoteFiles,
    fileUsers,
  };
}

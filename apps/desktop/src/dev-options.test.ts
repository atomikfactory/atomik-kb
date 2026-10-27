import {
  AI_SETTINGS_SETTING,
  CHUNK_SIZE,
  CoreClient,
  CoreError,
  DOWNLOAD_POLICY_SETTING,
  FAKE_DRIVE_ACCOUNT,
  MemoryTransport,
  TAGS_PROPERTY_ID,
  demoImagePng,
  driveReasonOf,
  kitWordsFromEntropy,
} from "@atomik-kb/core-client";
import type { MemoryTransportOptions } from "@atomik-kb/core-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  DEMO_CREATED_PROPERTY_ID,
  DEMO_DUE_IN_PROPERTY_ID,
  DEMO_DUE_PROPERTY_ID,
  DEMO_EFFORT_PROPERTY_ID,
  DEMO_FILE_ID,
  DEMO_PDF_ID,
  DEMO_AUDIO_ID,
  DEMO_IMAGE_ID,
  DEMO_IMAGE_URL,
  DEMO_PRIORITY_PROPERTY_ID,
  DEMO_SCORE_PROPERTY_ID,
  DEMO_SPACES,
  DEMO_STATUS_PROPERTY_ID,
  DEMO_TASK_COUNT_PROPERTY_ID,
  DEMO_TASKS_PROPERTY_ID,
  DEV_AI_SETTINGS,
  DEV_COMMENT_THREADS,
  DEV_DRIVE_KIT_KEY,
  DEV_INDEX_STEP_MS,
  DEV_MCP_BRIDGE_PATH,
  DEV_NOTIFICATIONS,
  DEV_PAIRING_STEP_MS,
  DEV_PASSPHRASE,
  DEV_PRESENCE_PEERS,
  DEV_PRESENCE_STEP_MS,
  DEV_PROFILE_NAME,
  DEV_RECOVERY_KIT,
  DEV_SHARING_STEP_MS,
  DEV_SYNC_SERVER,
  DEV_TRANSFER_STEP_MS,
  commentsMode,
  devRemoteImages,
  inTwoMinutes,
  initialRouteFromQuery,
  localDay,
  optionsFromQuery,
  presenceMode,
  remoteDemoAttachments,
  seedMcpDemo,
  withDemoLinks,
  withoutAttachments,
} from "./dev-options";

const NOW = Date.UTC(2026, 9, 3, 12, 0, 0);

const options = (search: string) => optionsFromQuery(new URLSearchParams(search), NOW);

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("the demo's reminder", () => {
  it("is due between two and three minutes from now, on this machine's clock, across midnight too", () => {
    expect(inTwoMinutes(new Date(2026, 9, 7, 12, 0, 0))).toEqual({ date: "2026-10-07", time: "12:02" });
    expect(inTwoMinutes(new Date(2026, 9, 7, 12, 0, 1))).toEqual({ date: "2026-10-07", time: "12:03" });
    expect(inTwoMinutes(new Date(2026, 9, 7, 12, 0, 59))).toEqual({ date: "2026-10-07", time: "12:03" });
    expect(inTwoMinutes(new Date(2026, 9, 7, 23, 58, 30))).toEqual({ date: "2026-10-08", time: "00:01" });
    expect(inTwoMinutes(new Date(2026, 11, 31, 23, 59, 0))).toEqual({ date: "2027-01-01", time: "00:01" });
  });
});

describe("dev.html query parameters", () => {
  it("start without a workspace, so the onboarding shows", () => {
    expect(options("")).toMatchObject({ status: { platform: "windows" } });
    expect(options("").workspaceName).toBeUndefined();
    expect(options("").spaces).toBeUndefined();
  });

  it("?seed=demo brings a workspace with the demo spaces and pages", () => {
    const seeded = options("seed=demo");
    expect(seeded.workspaceName).toBe("Demo workspace");
    expect(seeded.spaces).toBe(DEMO_SPACES);
  });

  it("?seed=demo keeps a workspace name that was asked for, and other seeds are ignored", () => {
    expect(options("seed=demo&workspace=Research").workspaceName).toBe("Research");
    expect(options("seed=other").spaces).toBeUndefined();
    expect(options("seed=other").workspaceName).toBeUndefined();
  });

  it("?workspace alone gives a workspace without spaces", () => {
    const plain = options("workspace=Research");
    expect(plain.workspaceName).toBe("Research");
    expect(plain.spaces).toBeUndefined();
  });

  it("keeps the other parameters working next to the seed", () => {
    const combined = options("seed=demo&theme=dark&api=2&encrypted=1");
    expect(combined.settings).toEqual({ "ui.theme": "dark" });
    expect(combined.status).toMatchObject({ apiVersion: 2, dbEncrypted: true });
    expect(options("core=opening").coreState).toBe("opening");
    expect(options("core=failed").coreState).toMatchObject({ failed: { code: "KeystoreUnavailable" } });
  });

  it("?language stores the UI language next to the theme", () => {
    expect(options("language=fr").settings).toEqual({ "ui.language": "fr" });
    expect(options("theme=dark&language=es").settings).toEqual({ "ui.theme": "dark", "ui.language": "es" });
    expect(options("seed=demo").settings).toBeUndefined();
  });

  it("the demo pages have content, and their links show as backlinks across spaces", async () => {
    // A fixed clock (only `Date`: the transport's promises and timers stay real), so that the
    // demo's reminder "in two minutes" is the same minute for the seed and for the check below.
    vi.useFakeTimers({ now: new Date(2026, 9, 7, 23, 58, 30), toFake: ["Date"] });
    const client = new CoreClient(new MemoryTransport(await withDemoLinks(options("seed=demo"))));
    const ids = new Map<string, string>();
    for (const space of await client.spaceList()) {
      const visit = (nodes: Awaited<ReturnType<CoreClient["treeGet"]>>["roots"]): void => {
        for (const node of nodes) {
          ids.set(node.page.title, node.page.id);
          visit(node.children);
        }
      };
      visit((await client.treeGet(space.id)).roots);
    }
    const id = (title: string): string => ids.get(title) ?? "";

    const welcome = JSON.stringify((await client.pageContentGet(id("Welcome"))).content);
    for (const kind of ["heading", "bulletList", "orderedList", "taskList", "details", "callout", "blockquote", "codeBlock", "horizontalRule", "pageRef", "image", "file"]) {
      expect(welcome).toContain(`"${kind}"`);
    }
    expect(welcome).toContain('"language":"mermaid"'); // a diagram next to the Rust code block
    expect(welcome).toContain('"type":"columnList"'); // two columns side by side
    expect(welcome.match(/"type":"column"/g)).toHaveLength(2);
    expect(welcome).toContain('"mathBlock"'); // an equation after the diagram
    expect(welcome).toContain('frac{1}{3}'); // its LaTeX
    // Date mentions after the intro: today, and a week from today with a time (ids 91 and 92).
    expect(welcome).toContain(`"type":"dateMention","attrs":{"date":"${localDay()}","id":"de000000-0000-4000-8000-000000000091"}`);
    expect(welcome).toContain(
      `"type":"dateMention","attrs":{"date":"${localDay(7)}","id":"de000000-0000-4000-8000-000000000092","time":"10:00"}`,
    );
    // Reminders after them (ids 101 and 102): one due in two to three minutes, one a day before 9:00 in three days.
    const soon = inTwoMinutes();
    expect(welcome).toContain(
      `"type":"dateMention","attrs":{"date":"${soon.date}","id":"de000000-0000-4000-8000-000000000101","reminder":0,"time":"${soon.time}"}`,
    );
    expect(welcome).toContain(
      `"type":"dateMention","attrs":{"date":"${localDay(3)}","id":"de000000-0000-4000-8000-000000000102","reminder":1440}`,
    );
    expect(welcome.match(/"type":"dateMention"/g)).toHaveLength(4);
    // Inline equations in a paragraph of their own (ids 110 to 112): the sources, and none empty.
    expect(welcome).toContain('"type":"mathInline","attrs":{"id":"de000000-0000-4000-8000-000000000111","latex":"E=mc^2"}');
    expect(welcome).toContain('"id":"de000000-0000-4000-8000-000000000112"');
    expect(welcome.match(/"type":"mathInline"/g)).toHaveLength(2);
    // A table of contents right after the title, and a breadcrumb on top of the nested Roadmap page.
    const welcomeBlocks = (await client.pageContentGet(id("Welcome"))).content as { content: { type: string }[] };
    expect(welcomeBlocks.content.slice(0, 2).map((block) => block.type)).toEqual(["heading", "tableOfContents"]);
    const roadmapBlocks = (await client.pageContentGet(id("Roadmap"))).content as { content: { type: string }[] };
    expect(roadmapBlocks.content[0]?.type).toBe("breadcrumb");
    expect(roadmapBlocks.content[1]?.type).toBe("heading");
    // A card (a `pageLink` block) to Roadmap, after the paragraph with the chip.
    expect(welcome).toContain(`"type":"pageLink","attrs":{"id":"de000000-0000-4000-8000-000000000080","pageId":"${id("Roadmap")}"}`);
    // The chip in its paragraph and the card each count as a link: two backlinks from Welcome.
    expect((await client.backlinksGet(id("Roadmap"))).map((link) => link.title)).toEqual(["Welcome", "Welcome"]);
    expect((await client.backlinksGet(id("Handbook"))).map((link) => link.title)).toEqual(["Welcome"]);
    expect((await client.backlinksGet(id("Design notes"))).map((link) => link.title)).toEqual(["Roadmap"]);
    expect((await client.search("milestones")).map((hit) => hit.title)).toEqual(["Roadmap"]);
  });

  it("the demo's image and file exist in the first space and are used by Welcome", async () => {
    const memory = new MemoryTransport(await withDemoLinks(options("seed=demo")));
    const client = new CoreClient(memory);
    const [personal] = await client.spaceList();
    const listed = await client.attachmentList(personal?.id ?? "");
    expect(listed.map((info) => info.attachmentId).sort()).toEqual([DEMO_IMAGE_ID, DEMO_FILE_ID, DEMO_PDF_ID, DEMO_AUDIO_ID].sort());
    // Each is shown by a block of Welcome and listed by its Files property; the image is also in its gallery.
    expect(await client.attachmentGet(DEMO_IMAGE_ID)).toMatchObject({ mime: "image/png", references: 3 });
    expect(await client.attachmentGet(DEMO_FILE_ID)).toMatchObject({ name: "packing-list.txt", references: 2 });
    // The PDF is shown as a preview by a file block of Welcome (ADR-0031).
    expect(await client.attachmentGet(DEMO_PDF_ID)).toMatchObject({ name: "atomik-demo.pdf", mime: "application/pdf", references: 1 });
    expect(new TextDecoder().decode((await client.attachmentRead(DEMO_PDF_ID)).subarray(0, 5))).toBe("%PDF-");
    // The sound is played by a file block of Welcome (ADR-0032).
    expect(await client.attachmentGet(DEMO_AUDIO_ID)).toMatchObject({ name: "atomik-tone.wav", mime: "audio/wav", references: 1 });
    expect(new TextDecoder().decode((await client.attachmentRead(DEMO_AUDIO_ID)).subarray(0, 4))).toBe("RIFF");
    expect((await client.attachmentRead(DEMO_IMAGE_ID)).subarray(1, 4)).toEqual(new TextEncoder().encode("PNG"));
    expect(options("workspace=Research").attachments).toBeUndefined();
  });

  it("the demo's Personal space has tags and properties of several kinds, with values on several pages", async () => {
    const client = new CoreClient(new MemoryTransport(await withDemoLinks(options("seed=demo"))));
    const [personal, work] = await client.spaceList();
    const schema = await client.propertySchemaGet(personal?.id ?? "");
    expect(schema.map((def) => [def.name, def.kind])).toEqual([
      ["Tags", "multi_select"],
      ["Status", "status"],
      ["Due", "date"],
      ["Done", "checkbox"],
      ["Link", "url"],
      ["Owner email", "email"],
      ["Priority", "number"],
      ["Files", "files"],
      ["Launch checklist", "checklist"],
      ["Place", "place"],
      ["Created", "created_time"],
      ["Last edited by", "last_edited_by"],
      ["Tasks", "relation"],
      ["Milestone", "relation"],
      ["Task count", "rollup"],
      ["Effort", "rollup"],
      ["Score", "formula"],
      ["Due in", "formula"],
    ]);
    const tags = schema.find((def) => def.id === TAGS_PROPERTY_ID);
    expect(tags?.options.map((option) => [option.name, option.color])).toEqual([
      ["idea", "green"],
      ["project", "blue"],
      ["reading", "orange"],
    ]);
    // Work has the built-in Tags and the Person property "Owner" (M13 slice 2).
    expect((await client.propertySchemaGet(work?.id ?? "")).map((def) => def.name)).toEqual(["Tags", "Owner"]);

    const ids = new Map<string, string>();
    const visit = (nodes: Awaited<ReturnType<CoreClient["treeGet"]>>["roots"]): void => {
      for (const node of nodes) {
        ids.set(node.page.title, node.page.id);
        visit(node.children);
      }
    };
    visit((await client.treeGet(personal?.id ?? "")).roots);
    // A value as the panel shows it: the names of the options, the plain value otherwise.
    const valuesOf = async (title: string): Promise<Record<string, unknown>> => {
      const entries = (await client.pagePropertiesGet(ids.get(title) ?? "")).map(([id, value]): [string, unknown] => {
        const def = schema.find((candidate) => candidate.id === id);
        const named = (optionId: string): string | undefined => def?.options.find((option) => option.id === optionId)?.name;
        switch (value.kind) {
          case "multi_select":
            return [def?.name ?? id, value.value.map(named)];
          case "select":
          case "status":
            return [def?.name ?? id, [named(value.value)]];
          case "checklist":
            return [def?.name ?? id, value.value.map((item) => [item.text, item.done])];
          // Computed values depend on the clock and the device: the tests below leave them out.
          case "created_time":
          case "formula":
          case "last_edited_by":
          case "rollup":
            return [def?.name ?? id, "computed"];
          default:
            return [def?.name ?? id, value.value];
        }
      });
      return Object.fromEntries(entries.filter(([, value]) => value !== "computed"));
    };
    expect(await valuesOf("Welcome")).toEqual({
      Tags: ["idea"],
      Status: ["In progress"],
      Due: { date: localDay(7) },
      Link: "https://example.com/atomik-kb",
      "Owner email": "owner@example.com",
      Priority: 2,
      Files: [DEMO_IMAGE_ID, DEMO_FILE_ID],
      Place: { name: "Café de Flore, Paris", lat: 48.854, lon: 2.333 },
    });
    expect(await valuesOf("Roadmap")).toEqual({
      Tags: ["project"],
      Status: ["Not started"],
      Due: { date: localDay(30) },
      Done: false,
      Priority: 1,
      "Launch checklist": [
        ["Write the release notes", true],
        ["Test the installers", false],
        ["Announce it", false],
      ],
    });
    expect(await valuesOf("Reading list")).toEqual({
      Tags: ["reading", "idea"],
      Status: ["In progress"],
      Link: "https://example.com/books",
      Done: false,
      Priority: 3,
      Place: { name: "Public library" },
    });
    expect(await valuesOf("Journal")).toEqual({});
    // A search for a tag finds the pages, and the tag filter of the panel has pages to list.
    expect((await client.search("reading")).map((hit) => hit.title)).toContain("Reading list");
    expect(await client.pagesByProperty(personal?.id ?? "", TAGS_PROPERTY_ID, { type: "ContainsOption", data: tags?.options[0]?.id ?? "" })).toEqual([
      ids.get("Reading list"),
      ids.get("Welcome"),
    ]);
  });

  it("the demo's Projects page holds a list view of its children that shows Status and Due, soonest first", async () => {
    const client = new CoreClient(new MemoryTransport(await withDemoLinks(options("seed=demo"))));
    const [personal] = await client.spaceList();
    const schema = await client.propertySchemaGet(personal?.id ?? "");
    // The view names the two properties by ids the seed fixes.
    expect(schema.find((def) => def.name === "Status")?.id).toBe(DEMO_STATUS_PROPERTY_ID);
    expect(schema.find((def) => def.name === "Due")?.id).toBe(DEMO_DUE_PROPERTY_ID);

    const projects = (await client.treeGet(personal?.id ?? "")).roots.find((node) => node.page.title === "Projects");
    const content = (await client.pageContentGet(projects?.page.id ?? "")).content as {
      content: { type: string; attrs?: Record<string, unknown> }[];
    };
    // An intro, the list view and an empty paragraph below it: block ids 130, 131 and 132.
    expect(content.content.map((block) => [block.type, block.attrs?.id])).toEqual([
      ["paragraph", "de000000-0000-4000-8000-000000000130"],
      ["pageList", "de000000-0000-4000-8000-000000000131"],
      ["paragraph", "de000000-0000-4000-8000-000000000132"],
    ]);
    expect(content.content[1]?.attrs).toEqual({
      id: "de000000-0000-4000-8000-000000000131",
      source: { kind: "children" },
      layout: "list",
      properties: [DEMO_STATUS_PROPERTY_ID, DEMO_DUE_PROPERTY_ID],
      sort: { by: DEMO_DUE_PROPERTY_ID, dir: "asc" },
    });
    // Both children carry the two values the view shows, Garden's due day coming first.
    expect(projects?.children.map((node) => node.page.title)).toEqual(["atomik-kb", "Garden"]);
    for (const child of projects?.children ?? []) {
      // (The computed properties of the demo show on every page; they are not stored values.)
      const stored = (child.page.properties ?? []).filter(([, value]) => !["created_time", "last_edited_by", "rollup", "formula"].includes(value.kind));
      expect(stored.map(([id]) => id)).toEqual([DEMO_STATUS_PROPERTY_ID, DEMO_DUE_PROPERTY_ID]);
    }
    const dueOf = (title: string): string | undefined => {
      const value = projects?.children.find((node) => node.page.title === title)?.page.properties?.find(([id]) => id === DEMO_DUE_PROPERTY_ID)?.[1];
      return value?.kind === "date" ? value.value.date : undefined;
    };
    expect(dueOf("Garden")).toBe(localDay(3));
    expect(dueOf("atomik-kb")).toBe(localDay(14));
  });

  it("the demo has a two-way relation with rollups, and a table and a board over the pages under Tasks", async () => {
    const client = new CoreClient(new MemoryTransport(await withDemoLinks(options("seed=demo"))));
    const [personal] = await client.spaceList();
    const schema = await client.propertySchemaGet(personal?.id ?? "");
    const tasksDef = schema.find((def) => def.id === DEMO_TASKS_PROPERTY_ID);
    const back = schema.find((def) => def.name === "Milestone");
    expect(tasksDef?.config).toMatchObject({ kind: "relation", twoWay: true, pairedPropertyId: back?.id });
    expect(schema.find((def) => def.id === DEMO_TASK_COUNT_PROPERTY_ID)?.config).toMatchObject({ kind: "rollup", function: "count_all" });
    expect(schema.find((def) => def.id === DEMO_EFFORT_PROPERTY_ID)?.config).toMatchObject({ kind: "rollup", function: "sum" });

    const roots = (await client.treeGet(personal?.id ?? "")).roots;
    const milestones = roots.find((node) => node.page.title === "Release plan");
    const tasks = roots.find((node) => node.page.title === "Tasks");
    const beta = milestones?.children.find((node) => node.page.title === "Beta");
    const release = milestones?.children.find((node) => node.page.title === "Release");
    const valueOf = (node: typeof beta, id: string) => node?.page.properties?.find(([propertyId]) => propertyId === id)?.[1];
    // Beta: two tasks, effort 3 + 5; Release: three tasks, effort 1 + 2 + 3.
    expect(valueOf(beta, DEMO_TASKS_PROPERTY_ID)).toMatchObject({ kind: "relation", value: [expect.any(String), expect.any(String)] });
    expect(valueOf(beta, DEMO_TASK_COUNT_PROPERTY_ID)).toEqual({ kind: "rollup", value: { function: "count_all", result: { type: "number", data: 2 } } });
    expect(valueOf(beta, DEMO_EFFORT_PROPERTY_ID)).toEqual({ kind: "rollup", value: { function: "sum", result: { type: "number", data: 8 } } });
    expect(valueOf(release, DEMO_EFFORT_PROPERTY_ID)).toEqual({ kind: "rollup", value: { function: "sum", result: { type: "number", data: 6 } } });
    // The task that is in both milestones lists both through the paired property.
    const notes = tasks?.children.find((node) => node.page.title === "Write release notes");
    expect(valueOf(notes, back?.id ?? "")).toMatchObject({ kind: "relation", value: [expect.any(String), expect.any(String)] });

    const attrsOfViews = async (title: string) => {
      const page = roots.find((node) => node.page.title === title);
      const content = (await client.pageContentGet(page?.page.id ?? "")).content as { content: { type: string; attrs?: Record<string, unknown> }[] };
      return content.content.filter((block) => block.type === "pageList").map((block) => block.attrs);
    };
    const views = await attrsOfViews("Tasks");
    expect(views.map((attrs) => attrs?.layout)).toEqual(["table", "board"]);
    expect(views[1]?.groupBy).toBe(DEMO_STATUS_PROPERTY_ID);
    expect(views[0]?.properties).toContain(DEMO_CREATED_PROPERTY_ID);
    const milestoneViews = await attrsOfViews("Release plan");
    expect(milestoneViews.map((attrs) => attrs?.layout)).toEqual(["table"]);
    expect(milestoneViews[0]?.properties).toEqual([DEMO_STATUS_PROPERTY_ID, DEMO_TASKS_PROPERTY_ID, DEMO_TASK_COUNT_PROPERTY_ID, DEMO_EFFORT_PROPERTY_ID]);
    // Every property the views name is in the schema.
    for (const attrs of [...views, ...milestoneViews]) {
      for (const id of attrs?.properties as string[]) {
        expect(schema.some((def) => def.id === id), id).toBe(true);
      }
    }
  });

  it("the demo has two formulas: a score from the priority, shown in the table of Tasks, and the days to a due date", async () => {
    const client = new CoreClient(new MemoryTransport(await withDemoLinks(options("seed=demo"))));
    const [personal] = await client.spaceList();
    const schema = await client.propertySchemaGet(personal?.id ?? "");
    const due = DEMO_DUE_PROPERTY_ID;
    expect(schema.find((def) => def.id === DEMO_SCORE_PROPERTY_ID)).toMatchObject({
      name: "Score",
      kind: "formula",
      config: {
        kind: "formula",
        expression: `prop("${DEMO_PRIORITY_PROPERTY_ID}") * 10`,
        resultType: { base: "number", list: false },
      },
    });
    expect(schema.find((def) => def.id === DEMO_DUE_IN_PROPERTY_ID)).toMatchObject({
      name: "Due in",
      kind: "formula",
      config: {
        kind: "formula",
        expression: `if(empty(prop("${due}")), "", format(dateBetween(prop("${due}"), today(), "days")) + " days")`,
        resultType: { base: "text", list: false },
      },
    });
    const roots = (await client.treeGet(personal?.id ?? "")).roots;
    const tasks = roots.find((node) => node.page.title === "Tasks");
    const valueOf = (title: string, id: string) =>
      [...roots, ...roots.flatMap((node) => node.children)]
        .find((node) => node.page.title === title)
        ?.page.properties?.find(([propertyId]) => propertyId === id)?.[1];
    expect(tasks?.children.map((node) => node.page.title)).toContain("Write release notes");
    expect(valueOf("Write release notes", DEMO_SCORE_PROPERTY_ID)).toEqual({ kind: "formula", value: { type: "number", data: 30 } });
    expect(valueOf("Test the installers", DEMO_SCORE_PROPERTY_ID)).toEqual({ kind: "formula", value: { type: "number", data: 50 } });
    // A page with no priority has no score; a page due in a week says so, one with no due date says nothing.
    expect(valueOf("Release plan", DEMO_SCORE_PROPERTY_ID)).toBeUndefined();
    expect(valueOf("Welcome", DEMO_DUE_IN_PROPERTY_ID)).toEqual({ kind: "formula", value: { type: "text", data: "7 days" } });
    expect(valueOf("Tasks", DEMO_DUE_IN_PROPERTY_ID)).toBeUndefined();
    // The table of Tasks shows the score.
    const content = (await client.pageContentGet(tasks?.page.id ?? "")).content as { content: { type: string; attrs?: Record<string, unknown> }[] };
    const table = content.content.find((block) => block.type === "pageList" && block.attrs?.layout === "table");
    expect(table?.attrs?.properties).toContain(DEMO_SCORE_PROPERTY_ID);
  });

  it("?attachments=none hides the attachment part of the transport and keeps the rest", async () => {
    const memory = new MemoryTransport(await withDemoLinks(options("seed=demo")));
    const bare = new CoreClient(withoutAttachments(memory));
    expect(new CoreClient(memory).hasAttachments()).toBe(true);
    expect(bare.hasAttachments()).toBe(false);
    expect(bare.hasAppLock()).toBe(true);
    expect(bare.hasBackups()).toBe(true);
    expect((await bare.spaceList()).map((space) => space.name)).toEqual(["Personal", "Work", "Archive"]);
    expect((await bare.attachmentGet(DEMO_IMAGE_ID)).name).toBe("atomik.png");
    expect(bare.canImportUrl()).toBe(false); // no "Image from URL" either
  });

  it("every dev page can import the one canned link of 'Image from URL', and no other", async () => {
    expect(DEMO_IMAGE_URL).toBe("https://example.com/demo.png");
    // The in-memory core has no network: any other link goes to `fetch`, which here is offline.
    vi.stubGlobal("fetch", () => Promise.reject(new TypeError("offline")));
    for (const search of ["seed=demo", "attachments=downloading"]) {
      const memory = new MemoryTransport(await withDemoLinks(options(search)));
      const client = new CoreClient(memory);
      expect(client.canImportUrl(), search).toBe(true);
      const [space] = await client.spaceList();
      const info = await client.attachmentImportUrl(space?.id ?? "", DEMO_IMAGE_URL);
      expect(info, search).toMatchObject({ name: "demo.png", mime: "image/png", size: demoImagePng().length });
      expect(await client.attachmentRead(info.attachmentId), search).toEqual(demoImagePng());
      await expect(client.attachmentImportUrl(space?.id ?? "", "https://example.com/other.png"), search).rejects.toMatchObject({
        code: "InvalidInput",
        details: { field: "url", reason: "network" },
      });
    }
    expect([...devRemoteImages().keys()]).toEqual([DEMO_IMAGE_URL]);
  });

  it("other options are left alone by the demo links", async () => {
    const plain = options("workspace=Research");
    expect(await withDemoLinks(plain)).toBe(plain);
  });

  it("the demo data is a valid seed with nested pages in two spaces", async () => {
    const client = new CoreClient(new MemoryTransport(options("seed=demo")));
    const spaces = await client.spaceList();
    expect(spaces.map((space) => space.name)).toEqual(["Personal", "Work", "Archive"]);
    expect(spaces.map((space) => space.pageCount)).toEqual([17, 4, 0]);
    const personal = await client.treeGet(spaces[0]?.id ?? "");
    const projects = personal.roots.find((node) => node.page.title === "Projects");
    expect(projects?.children.map((node) => node.page.title)).toEqual(["atomik-kb", "Garden"]);
    expect(projects?.children[0]?.children).toHaveLength(2);
  });

  it("the demo's Personal space has two favourites, Welcome then Reading list, and the other spaces none", async () => {
    const client = new CoreClient(new MemoryTransport(options("seed=demo")));
    const [personal, work, archive] = await client.spaceList();
    const view = await client.treeGet(personal?.id ?? "");
    const titleOf = (id: string): string | undefined =>
      view.roots.find((node) => node.page.id === id)?.page.title;
    expect((view.favorites ?? []).map(titleOf)).toEqual(["Welcome", "Reading list"]);
    // Seeding leaves no undo history and no event.
    expect(view.undo).toEqual({ canUndo: false, canRedo: false });
    for (const space of [work, archive]) {
      expect((await client.treeGet(space?.id ?? "")).favorites).toBeUndefined();
    }
  });
});

describe("dev.html ?sync", () => {
  it("is off by default and ignores values it does not know", async () => {
    expect(options("").sync).toBeUndefined();
    expect(options("sync=other").sync).toBeUndefined();
    expect(options("sync=other").workspaceName).toBeUndefined();
    const info = await new CoreClient(new MemoryTransport(options("workspace=Research"))).syncStatus();
    expect(info).toMatchObject({ configured: false, state: "Disabled" });
  });

  it("starts connected to the documented server, in a workspace, in each state", async () => {
    const expected = { idle: "Idle", offline: "Offline", error: "Error", forked: "Forked" } as const;
    for (const [mode, state] of Object.entries(expected)) {
      const seeded = options(`sync=${mode}`);
      expect(seeded.workspaceName, mode).toBe("Demo workspace");
      expect(seeded.sync, mode).toEqual({ state: mode, serverUrl: DEV_SYNC_SERVER });
      const info = await new CoreClient(new MemoryTransport(seeded)).syncStatus();
      expect(info, mode).toMatchObject({ configured: true, serverUrl: DEV_SYNC_SERVER, state, forkDetected: mode === "forked" });
      expect(info.lastError !== undefined, mode).toBe(mode === "offline" || mode === "error");
    }
    expect(DEV_SYNC_SERVER).toBe("https://sync.example.com");
  });

  it("?sync=invite starts off, with a server that does not know the account", async () => {
    const invite = options("sync=invite");
    expect(invite.workspaceName).toBe("Demo workspace");
    expect(invite.sync).toEqual({ accountKnown: false });
    const client = new CoreClient(new MemoryTransport({ ...invite, syncStepMs: 1 }));
    expect(await client.syncStatus()).toMatchObject({ configured: false, state: "Disabled", needsInviteToken: false });
    await client.syncConfigure(DEV_SYNC_SERVER);
    await vi.waitFor(async () => {
      expect(await client.syncStatus()).toMatchObject({ state: "Error", needsInviteToken: true });
    });
    await client.syncConfigure(DEV_SYNC_SERVER, "invite-123");
    await vi.waitFor(async () => {
      expect(await client.syncStatus()).toMatchObject({ state: "Idle", needsInviteToken: false });
    });
  });

  it("works with the demo seed and keeps a workspace name that was asked for", async () => {
    expect(options("sync=offline&workspace=Research").workspaceName).toBe("Research");
    const memory = new MemoryTransport(await withDemoLinks(options("seed=demo&sync=forked")));
    const client = new CoreClient(memory);
    expect((await client.spaceList()).map((space) => space.name)).toEqual(["Personal", "Work", "Archive"]);
    expect((await client.syncStatus()).forkDetected).toBe(true);
  });
});

describe("dev.html ?devices", () => {
  it("is off by default and ignores values it does not know", () => {
    expect(options("").devices).toBeUndefined();
    expect(options("devices=other").devices).toBeUndefined();
    expect(options("devices=other").workspaceName).toBeUndefined();
    expect(initialRouteFromQuery(new URLSearchParams("devices=other"))).toBeUndefined();
  });

  it("pair-joiner and recover start on a fresh installation, whatever else asks for a workspace", async () => {
    for (const mode of ["pair-joiner", "recover"]) {
      const fresh = options(`devices=${mode}&seed=demo&security=lock&sync=idle&workspace=Research`);
      expect(fresh.workspaceName, mode).toBeUndefined();
      expect(fresh.spaces, mode).toBeUndefined();
      expect(fresh.security, mode).toBeUndefined();
      expect(fresh.sync, mode).toBeUndefined();
      expect(fresh.pairingStepMs, mode).toBe(DEV_PAIRING_STEP_MS);
      expect(initialRouteFromQuery(new URLSearchParams(`devices=${mode}`)), mode).toBeUndefined();
      const client = new CoreClient(new MemoryTransport(await withDemoLinks(fresh)));
      expect(await client.workspaceGet(), mode).toBeNull();
    }
  });

  it("the documented recovery kit recovers the account", async () => {
    const memory = new MemoryTransport({ ...options("devices=recover"), pairingStepMs: 1 });
    const client = new CoreClient(memory);
    await client.recoveryStart(DEV_SYNC_SERVER, DEV_RECOVERY_KIT);
    await vi.waitFor(async () => {
      expect((await client.recoveryStatus()).state).toBe("Done");
    });
    expect(await client.workspaceGet()).not.toBeNull();
  });

  it("pair-sponsor has the kit, sync on and another device, and opens on Settings", async () => {
    const sponsor = options("devices=pair-sponsor");
    expect(sponsor.workspaceName).toBe("Demo workspace");
    expect(initialRouteFromQuery(new URLSearchParams("devices=pair-sponsor"))).toBe("settings");
    const client = new CoreClient(new MemoryTransport(sponsor));
    expect((await client.syncStatus()).configured).toBe(true);
    expect(await client.deviceList()).toHaveLength(2);
    await expect(client.pairingStart("Sponsor", { code: "ABCD EFGH JKMN" })).resolves.toMatchObject({ role: "Sponsor" });
  });

  it("revoked and rotation seed the removed device and the pending rotation (with the app lock)", async () => {
    const revoked = await new CoreClient(new MemoryTransport(options("devices=revoked"))).securityStatus();
    expect(revoked).toMatchObject({ thisDeviceRevoked: true, rotationPending: false });

    const memory = new MemoryTransport(options("devices=rotation"));
    const client = new CoreClient(memory);
    expect(await client.securityStatus()).toMatchObject({ rotationPending: true, appLock: { enabled: true } });
    expect(await client.deviceList()).toHaveLength(3);
    await client.rotationApply(DEV_PASSPHRASE);
    expect((await client.securityStatus()).rotationPending).toBe(false);
  });

  it("works with the demo seed, whose links still resolve", async () => {
    const memory = new MemoryTransport(await withDemoLinks(options("seed=demo&devices=rotation")));
    const client = new CoreClient(memory);
    const roadmap = (await client.search("Roadmap", { scope: "Titles" }))[0]?.pageId ?? "";
    expect((await client.backlinksGet(roadmap)).map((link) => link.title)).toEqual(["Welcome", "Welcome"]); // its chip and its card
    expect((await client.securityStatus()).rotationPending).toBe(true);
  });
});

describe("dev.html ?security", () => {
  it("is off by default and ignores values it does not know", () => {
    expect(options("").security).toBeUndefined();
    expect(options("security=other").security).toBeUndefined();
    expect(options("security=other").workspaceName).toBeUndefined();
  });

  it("?security=kit starts with a recovery kit, in a workspace", async () => {
    const kit = options("security=kit");
    expect(kit.workspaceName).toBe("Demo workspace");
    expect(kit.security).toEqual({ kitCreatedAt: NOW });
    const status = await new CoreClient(new MemoryTransport(kit)).securityStatus();
    expect(status.recoveryKit).toEqual({ createdAt: NOW, stale: false });
    expect(status.appLock.enabled).toBe(false);
    expect(options("security=kit&workspace=Research").workspaceName).toBe("Research");
  });

  it("?security=lock turns the app lock on with the documented passphrase", async () => {
    expect(DEV_PASSPHRASE).toBe("correct horse battery");
    const memory = new MemoryTransport(options("security=lock"));
    await expect(memory.coreState()).resolves.toBe("ready");
    expect((await new CoreClient(memory).securityStatus()).appLock.enabled).toBe(true);
    await memory.lock();
    await memory.unlock(DEV_PASSPHRASE);
    await expect(memory.coreState()).resolves.toBe("ready");
  });

  it("?security=locked starts locked, and the passphrase unlocks", async () => {
    const memory = new MemoryTransport(options("security=locked"));
    await expect(memory.coreState()).resolves.toBe("locked");
    await memory.unlock(DEV_PASSPHRASE);
    await expect(memory.coreState()).resolves.toBe("ready");
  });

  it("?security=locked with ?core=opening is still opening (dev.tsx locks it after three seconds)", async () => {
    const opening = options("security=locked&core=opening");
    expect(opening.security).toEqual({ kitCreatedAt: NOW, appLockPassphrase: DEV_PASSPHRASE });
    const memory = new MemoryTransport(opening);
    await expect(memory.coreState()).resolves.toBe("opening");
    memory.setCoreState("locked");
    await memory.unlock(DEV_PASSPHRASE);
    await expect(memory.coreState()).resolves.toBe("ready");
  });

  it("works with the demo seed, whose links still resolve", async () => {
    const memory = new MemoryTransport(await withDemoLinks(options("seed=demo&security=locked")));
    await expect(memory.coreState()).resolves.toBe("locked");
    await memory.unlock(DEV_PASSPHRASE);
    const client = new CoreClient(memory);
    const roadmap = (await client.search("Roadmap", { scope: "Titles" }))[0]?.pageId ?? "";
    expect((await client.backlinksGet(roadmap)).map((link) => link.title)).toEqual(["Welcome", "Welcome"]); // its chip and its card
    expect((await client.securityStatus()).recoveryKit).toEqual({ createdAt: NOW, stale: false });
  });
});

describe("dev.html ?attachments=downloading|waiting|on-demand", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  /** The dev page's core for `search`, without the transfer timer (tests move chunks themselves). */
  async function seeded(search: string): Promise<{ memory: MemoryTransport; client: CoreClient }> {
    const seededOptions: MemoryTransportOptions = await withDemoLinks(options(search));
    const memory = new MemoryTransport({
      ...seededOptions,
      attachmentSync: { ...seededOptions.attachmentSync, transferStepMs: undefined },
    });
    return { memory, client: new CoreClient(memory) };
  }

  it("is off by default, for none and for values it does not know", () => {
    for (const search of ["", "seed=demo", "attachments=none", "attachments=other", "seed=demo&attachments=none"]) {
      expect(options(search).attachmentSync, search).toBeUndefined();
    }
    expect(options("attachments=other").spaces).toBeUndefined();
    expect(options("seed=demo&attachments=none").attachments).toHaveLength(4);
  });

  it("implies the demo, whose image and file become remote, with sync connected and a 1.5 s transfer step", () => {
    for (const mode of ["downloading", "waiting", "on-demand"]) {
      const seededOptions = options(`attachments=${mode}`);
      expect(seededOptions.spaces, mode).toBe(DEMO_SPACES);
      expect(seededOptions.workspaceName, mode).toBe("Demo workspace");
      expect(seededOptions.attachments, mode).toBeUndefined();
      expect(seededOptions.sync, mode).toEqual({ state: "idle", serverUrl: DEV_SYNC_SERVER });
      expect(seededOptions.attachmentSync?.transferStepMs, mode).toBe(DEV_TRANSFER_STEP_MS);
      expect(seededOptions.attachmentSync?.downloadPolicy, mode).toBe(mode === "on-demand" ? "on_demand" : undefined);
      const remote = seededOptions.attachmentSync?.remoteAttachments ?? [];
      expect(remote.map((seed) => [seed.attachmentId, seed.waiting]), mode).toEqual([
        [DEMO_IMAGE_ID, mode === "waiting"],
        [DEMO_FILE_ID, mode === "waiting"],
        [DEMO_PDF_ID, mode === "waiting"],
        [DEMO_AUDIO_ID, mode === "waiting"],
      ]);
    }
    expect(DEV_TRANSFER_STEP_MS).toBe(1500);
    // Other parameters keep their say.
    expect(options("attachments=waiting&sync=offline").sync).toEqual({ state: "offline", serverUrl: DEV_SYNC_SERVER });
    expect(options("attachments=waiting&workspace=Research").workspaceName).toBe("Research");
    expect(options("attachments=waiting&devices=pair-joiner").attachmentSync).toBeUndefined();
  });

  it("the remote image is the demo picture padded to two chunks, the file three chunks", () => {
    const [image, file] = remoteDemoAttachments(false);
    expect(image?.bytes.length).toBeGreaterThan(CHUNK_SIZE);
    expect(image?.bytes.length).toBeLessThan(2 * CHUNK_SIZE);
    expect(Array.from(image?.bytes.subarray(1, 4) ?? [])).toEqual([0x50, 0x4e, 0x47]);
    expect(file).toMatchObject({ name: "site-survey.pdf", mime: "application/pdf" });
    expect(Math.floor((file?.bytes.length ?? 0) / CHUNK_SIZE) + 1).toBe(3);
  });

  it("downloading: both are on their way and arrive chunk by chunk; the links still resolve", async () => {
    const { memory, client } = await seeded("attachments=downloading");
    expect(await client.attachmentGet(DEMO_IMAGE_ID)).toMatchObject({ localChunks: 0, chunkCount: 2, availability: "Downloading", references: 3 });
    expect(await client.attachmentGet(DEMO_FILE_ID)).toMatchObject({ localChunks: 0, chunkCount: 3, availability: "Downloading", references: 2 });
    expect(await client.syncStatus()).toMatchObject({ configured: true, state: "Idle", transfers: { uploads: 0, downloads: 7, waiting: 0 } });
    expect(await client.settingsGet(DOWNLOAD_POLICY_SETTING)).toBeNull();
    await expect(client.attachmentRead(DEMO_IMAGE_ID)).rejects.toThrow(CoreError);

    expect(memory.advanceTransfers(7)).toBe(7);
    expect((await client.attachmentRead(DEMO_IMAGE_ID)).subarray(1, 4)).toEqual(new TextEncoder().encode("PNG"));
    expect(await client.attachmentGet(DEMO_FILE_ID)).toMatchObject({ localChunks: 3, availability: "Complete" });
    const roadmap = (await client.search("Roadmap", { scope: "Titles" }))[0]?.pageId ?? "";
    expect((await client.backlinksGet(roadmap)).map((link) => link.title)).toEqual(["Welcome", "Welcome"]); // its chip and its card
  });

  it("waiting: both wait for the other device and never arrive", async () => {
    const { memory, client } = await seeded("attachments=waiting");
    for (const attachmentId of [DEMO_IMAGE_ID, DEMO_FILE_ID]) {
      expect((await client.attachmentGet(attachmentId)).availability).toBe("Waiting");
    }
    expect((await client.syncStatus()).transfers).toEqual({ uploads: 0, downloads: 0, waiting: 7 });
    expect(memory.advanceTransfers(10)).toBe(0);
  });

  it("on-demand: neither is downloaded until Download asks for one", async () => {
    const { memory, client } = await seeded("attachments=on-demand");
    expect(await client.settingsGet(DOWNLOAD_POLICY_SETTING)).toBe("on_demand");
    for (const attachmentId of [DEMO_IMAGE_ID, DEMO_FILE_ID]) {
      expect((await client.attachmentGet(attachmentId)).availability).toBe("NotDownloaded");
    }
    expect(memory.advanceTransfers()).toBe(0);
    expect((await client.attachmentDownload(DEMO_FILE_ID)).availability).toBe("Downloading");
    expect(memory.advanceTransfers(10)).toBe(3);
    expect((await client.attachmentGet(DEMO_FILE_ID)).availability).toBe("Complete");
    expect((await client.attachmentGet(DEMO_IMAGE_ID)).availability).toBe("NotDownloaded");
  });

  it("the page's timer completes the downloads by itself; the demo links' probe starts none", async () => {
    vi.useFakeTimers();
    const seededOptions = await withDemoLinks(options("attachments=downloading"));
    expect(vi.getTimerCount()).toBe(0);
    const client = new CoreClient(new MemoryTransport(seededOptions));
    expect(vi.getTimerCount()).toBe(1);
    vi.advanceTimersByTime(DEV_TRANSFER_STEP_MS);
    expect((await client.attachmentGet(DEMO_IMAGE_ID)).localChunks).toBe(1);
    vi.advanceTimersByTime(6 * DEV_TRANSFER_STEP_MS);
    expect((await client.attachmentGet(DEMO_IMAGE_ID)).availability).toBe("Complete");
    expect((await client.attachmentGet(DEMO_FILE_ID)).availability).toBe("Complete");
    expect((await client.attachmentGet(DEMO_PDF_ID)).availability).toBe("Complete");
    expect((await client.attachmentGet(DEMO_AUDIO_ID)).availability).toBe("Complete");
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("?ai= (ADR-0037)", () => {
  it("gives the transport no AI unless asked", () => {
    expect(options("seed=demo").ai).toBeUndefined();
    expect(options("ai=nonsense").ai).toBeUndefined();
    expect(options("aiOn=1").settings).toBeUndefined();
  });

  it("fake: a fake Ollama in a workspace, the index filling on a timer, AI itself still off", async () => {
    const opts = options("ai=fake");
    expect(opts.ai).toEqual({ mode: "fake", indexStepMs: DEV_INDEX_STEP_MS, bridgePath: DEV_MCP_BRIDGE_PATH });
    expect(opts.workspaceName).toBe("Demo workspace");
    expect(opts.settings?.[AI_SETTINGS_SETTING]).toBeUndefined();
    const client = new CoreClient(new MemoryTransport(opts));
    expect(client.hasAi()).toBe(true);
    expect(client.hasMcp()).toBe(true);
    expect((await client.aiStatus()).reachable).toBe(true);
    expect((await client.mcpStatus()).bridgePath).toBe(DEV_MCP_BRIDGE_PATH);
  });

  it("fake with aiOn=1: AI starts on with the fake models", () => {
    expect(options("ai=fake&aiOn=1").settings?.[AI_SETTINGS_SETTING]).toEqual(DEV_AI_SETTINGS);
    expect(DEV_AI_SETTINGS).toMatchObject({ enabled: true, chatModel: "qwen3:8b" });
  });

  it("ollama: the default endpoint, or the one aiUrl names; AI stays off until Settings turns it on", () => {
    expect(options("ai=ollama").ai).toMatchObject({ mode: "ollama", baseUrl: "http://127.0.0.1:11434" });
    expect(options("ai=ollama&aiUrl=http://192.168.1.9:11434").ai).toMatchObject({ mode: "ollama", baseUrl: "http://192.168.1.9:11434" });
    expect(options("ai=ollama&aiOn=1").settings).toBeUndefined();
  });

  it("keeps the workspace name it was given, and the demo seed", () => {
    const opts = options("seed=demo&workspace=Mine&ai=fake");
    expect(opts.workspaceName).toBe("Mine");
    expect(opts.spaces).toBe(DEMO_SPACES);
  });

  it("the demo links probe does not start the index timer", async () => {
    vi.useFakeTimers();
    await withDemoLinks(options("seed=demo&ai=fake&aiOn=1"));
    expect(vi.getTimerCount()).toBe(0);
  });

  it("?mcp=demo: two clients, read access to the first space and a few audited calls", async () => {
    const memory = new MemoryTransport(await withDemoLinks(options("seed=demo&ai=fake")));
    await seedMcpDemo(memory);
    const client = new CoreClient(memory);
    const clients = await client.mcpClientList();
    expect(clients.map((entry) => entry.name)).toEqual(["Claude Desktop", "Scripts"]);
    expect(clients[0]?.grants).toHaveLength(1);
    expect(clients[0]?.grants[0]).toMatchObject({ read: true, write: false });
    const tools = (await client.mcpAuditList()).map((row) => row.tool).reverse();
    expect(tools).toEqual(["list_spaces", "search", "read_page", "read_page"]);
    expect((await client.mcpAuditList())[0]?.outcome).toBe("denied");
  });

  it("withoutAttachments keeps the AI and MCP commands", () => {
    const memory = new MemoryTransport(options("ai=fake"));
    const transport = withoutAttachments(memory);
    expect(transport.ai).toBe(memory.ai);
    expect(transport.mcp).toBe(memory.mcp);
  });
});

describe("?dictation=fake (ADR-0053)", () => {
  it("is off without it, and for anything but fake", () => {
    expect(options("seed=demo").dictation).toBeUndefined();
    expect(options("dictation=nonsense").dictation).toBeUndefined();
    expect(options("dictation=real").dictation).toBeUndefined();
    expect(new MemoryTransport(options("seed=demo")).dictation).toBeUndefined();
    expect(new CoreClient(new MemoryTransport(options("seed=demo"))).hasDictation()).toBe(false);
  });

  it("gives the fake with its defaults, in a workspace", async () => {
    const opts = options("dictation=fake");
    expect(opts.dictation).toBe("fake");
    expect(opts.workspaceName).toBe("Demo workspace");
    const client = new CoreClient(new MemoryTransport(opts));
    expect(client.hasDictation()).toBe(true);
    const status = await client.dictation().status();
    expect(status.selected).toBeNull();
    expect(status.engines.map((engine) => [engine.engine, engine.installed])).toEqual([
      ["accurate", false],
      ["light", false],
      ["ollama", true],
    ]);
  });

  it("keeps the workspace that ?workspace= or ?seed= gave", () => {
    expect(options("dictation=fake&workspace=Mine").workspaceName).toBe("Mine");
    expect(options("dictation=fake&seed=demo").spaces).toBe(DEMO_SPACES);
  });

  it("starts with the models of &dictationInstalled= installed and the first chosen", async () => {
    const light = new CoreClient(new MemoryTransport(options("dictation=fake&dictationInstalled=light")));
    const status = await light.dictation().status();
    expect(status.selected).toBe("light");
    expect(status.engines.filter((engine) => engine.installed).map((engine) => engine.engine)).toEqual(["light", "ollama"]);

    const both = new CoreClient(new MemoryTransport(options("dictation=fake&dictationInstalled=accurate,light")));
    expect((await both.dictation().status()).selected).toBe("accurate");
    // What is not an engine of a download is left out; with none left, the defaults.
    expect(options("dictation=fake&dictationInstalled=ollama,nothing").dictation).toBe("fake");
  });

  it("works with ?ai= and the demo links", async () => {
    const memory = new MemoryTransport(await withDemoLinks(options("seed=demo&ai=fake&dictation=fake")));
    expect(memory.ai).toBeDefined();
    expect(memory.dictation).toBeDefined();
    const probeFree = new CoreClient(memory);
    expect(probeFree.hasAi()).toBe(true);
    expect(probeFree.hasDictation()).toBe(true);
  });

  it("withoutAttachments keeps it", () => {
    const memory = new MemoryTransport(options("dictation=fake"));
    expect(withoutAttachments(memory).dictation).toBe(memory.dictation);
    expect(withoutAttachments(new MemoryTransport(options("seed=demo"))).dictation).toBeUndefined();
  });
});

describe("?drive=fake (ADR-0059)", () => {
  it("is off without it, and for anything but fake", () => {
    expect(options("seed=demo").drive).toBeUndefined();
    expect(options("drive=nonsense").drive).toBeUndefined();
    expect(options("drive=real&driveConnected=1").drive).toBeUndefined();
    expect(new CoreClient(new MemoryTransport(options("seed=demo"))).hasDrive()).toBe(false);
  });

  it("gives the fake with its defaults and starts no workspace: alone it is onboarding", async () => {
    const opts = options("drive=fake");
    expect(opts.drive).toBe("fake");
    expect(opts.workspaceName).toBeUndefined();
    const client = new CoreClient(new MemoryTransport(opts));
    expect(client.hasDrive()).toBe(true);
    expect(await client.drive().status()).toEqual({ client: "builtIn", connected: false, needsReconnect: false });
    expect((await client.drive().connect()).account).toBe(FAKE_DRIVE_ACCOUNT);
  });

  it("keeps the workspace that ?workspace= or ?seed= gave, and works with the other fakes", async () => {
    expect(options("drive=fake&workspace=Mine").workspaceName).toBe("Mine");
    expect(options("drive=fake&seed=demo").spaces).toBe(DEMO_SPACES);
    const memory = new MemoryTransport(await withDemoLinks(options("seed=demo&ai=fake&dictation=fake&drive=fake")));
    expect(memory.drive).toBeDefined();
    expect(memory.ai).toBeDefined();
    expect(memory.dictation).toBeDefined();
  });

  it("starts connected, with a refused consent, or with no client, as &driveConnected=1, &driveConsent=no and &driveClient=none say", async () => {
    const connected = new CoreClient(new MemoryTransport(options("drive=fake&driveConnected=1")));
    expect(await connected.drive().status()).toMatchObject({ connected: true, account: FAKE_DRIVE_ACCOUNT });

    const refused = new CoreClient(new MemoryTransport(options("drive=fake&driveConsent=no")));
    expect(driveReasonOf(await refused.drive().connect().catch((error: unknown) => error))).toBe("consentDeclined");

    const none = new CoreClient(new MemoryTransport(options("drive=fake&driveClient=none")));
    expect((await none.drive().status()).client).toBe("none");
    expect(driveReasonOf(await none.drive().connect().catch((error: unknown) => error))).toBe("noClient");
  });

  it("fails the operations of &driveFail=, and leaves out pairs that are not both known", async () => {
    const client = new CoreClient(new MemoryTransport(options("drive=fake&driveConnected=1&driveFail=list:quota,deliver:reconnect,connect:nonsense,nothing:denied,x")));
    expect(driveReasonOf(await client.drive().listBackups().catch((error: unknown) => error))).toBe("quota");
    expect(driveReasonOf(await client.drive().connect().then(() => undefined, (error: unknown) => error))).toBeUndefined();
    expect(options("drive=fake&driveFail=bogus").drive).toBe("fake");
  });

  it("puts three restorable backups in the folder for &driveBackups=demo: the wizard's kit words restore them", async () => {
    const opts = options("drive=fake&driveConnected=1&driveBackups=demo");
    const client = new CoreClient(new MemoryTransport(opts));
    const list = await client.drive().listBackups();
    expect(list.map((file) => file.name)).toHaveLength(3);
    expect(list.every((file) => /^atomik-kb-auto-\d{4}-\d{2}-\d{2}-\d{4}\.atomikkb$/u.test(file.name))).toBe(true);
    // Newest first.
    expect([...list].sort((a, b) => b.modifiedAt - a.modifiedAt).map((file) => file.id)).toEqual(list.map((file) => file.id));
    const path = await client.drive().download(list[0]?.id ?? "", () => undefined);
    const words = (await kitWordsFromEntropy(DEV_DRIVE_KIT_KEY)).join(" ");
    await client.backupImport(path, words);
    expect((await client.spaceList()).map((space) => space.name)).toEqual(DEMO_SPACES.map((space) => space.name));
  });

  it("puts N files that are not backups for &driveBackups=N", async () => {
    const client = new CoreClient(new MemoryTransport(options("drive=fake&driveConnected=1&driveBackups=3")));
    const list = await client.drive().listBackups();
    expect(list).toHaveLength(3);
    const path = await client.drive().download(list[0]?.id ?? "", () => undefined);
    expect((await client.backupImport(path, "abandon").catch((error: unknown) => error)) instanceof CoreError).toBe(true);
    expect(options("drive=fake&driveBackups=0").drive).toBe("fake");
    expect(options("drive=fake&driveBackups=lots").drive).toBe("fake");
  });

  it("withoutAttachments keeps it and the folder listing", async () => {
    const memory = new MemoryTransport(options("drive=fake"));
    const transport = withoutAttachments(memory);
    expect(transport.drive).toBe(memory.drive);
    expect(await transport.backupFolderList?.("/backups")).toEqual({ folder: "/backups", backups: [] });
    expect(withoutAttachments(new MemoryTransport(options("seed=demo"))).drive).toBeUndefined();
  });
});

describe("?shared=1 and ?sharing= (Milestone 13)", () => {
  it("shares the demo's Work space with this account as its Owner, with the kit, sync and the demo seed", async () => {
    const opts = options("shared=1");
    expect(opts.spaces).toBe(DEMO_SPACES);
    expect(opts.security?.kitCreatedAt).toBe(NOW);
    expect(opts.sync).toEqual({ state: "idle", serverUrl: DEV_SYNC_SERVER });
    expect(opts.sharing).toMatchObject({ profileName: DEV_PROFILE_NAME, stepMs: DEV_SHARING_STEP_MS, spaces: [{ space: "Work" }] });
    const client = new CoreClient(new MemoryTransport(await withDemoLinks(opts)));
    const spaces = await client.spaceList();
    expect(spaces.map((space) => [space.name, space.shareState ?? "Personal"])).toEqual([
      ["Personal", "Personal"],
      ["Work", "Shared"],
      ["Archive", "Personal"],
    ]);
    const work = spaces.find((space) => space.name === "Work");
    expect(work).toMatchObject({ role: "owner", memberCount: 4, aiEnabled: false });
    const info = await client.spaceMembers(work?.id ?? "");
    expect(info.members.map((member) => [member.name, member.role])).toEqual([
      ["Maya", "owner"],
      ["Bob", "editor"],
      ["Carol", "commenter"],
      ["Dan", "viewer"],
    ]);
  });

  it("keeps the ids of the demo's spaces and pages: sharing draws none", async () => {
    const plain = new CoreClient(new MemoryTransport(await withDemoLinks(options("seed=demo"))));
    const shared = new CoreClient(new MemoryTransport(await withDemoLinks(options("seed=demo&shared=1"))));
    expect((await shared.spaceList()).map((space) => space.id)).toEqual((await plain.spaceList()).map((space) => space.id));
    const idsOf = async (client: CoreClient): Promise<string[]> =>
      (await client.treeGet((await client.spaceList())[1]?.id ?? "")).roots.map((node) => node.page.id);
    expect(await idsOf(shared)).toEqual(await idsOf(plain));
  });

  it("takes the role from the mode, and starts a read-only copy for removed and closed", async () => {
    for (const role of ["editor", "commenter", "viewer"] as const) {
      const client = new CoreClient(new MemoryTransport(await withDemoLinks(options(`sharing=${role}`))));
      const work = (await client.spaceList()).find((space) => space.name === "Work");
      expect(work, role).toMatchObject({ shareState: "Shared", role });
    }
    for (const [mode, reason] of [
      ["removed", "Removed"],
      ["closed", "Closed"],
    ] as const) {
      const client = new CoreClient(new MemoryTransport(await withDemoLinks(options(`sharing=${mode}`))));
      const work = (await client.spaceList()).find((space) => space.name === "Work");
      expect(work, mode).toMatchObject({ shareState: "Archived", archiveReason: reason });
    }
  });

  it("disputed: an incomplete snapshot, refused changes and an invitation; kit-notice: a device added with the kit", async () => {
    const disputed = new CoreClient(new MemoryTransport(await withDemoLinks(options("sharing=disputed"))));
    const work = (await disputed.spaceList()).find((space) => space.name === "Work");
    const info = await disputed.spaceMembers(work?.id ?? "");
    expect(info.disputes).toHaveLength(1);
    expect(info.refusedTotal).toBe(3);
    expect(info.invites).toHaveLength(1);
    const notice = new CoreClient(new MemoryTransport(await withDemoLinks(options("sharing=kit-notice"))));
    const noticeWork = (await notice.spaceList()).find((space) => space.name === "Work");
    expect((await notice.spaceMembers(noticeWork?.id ?? "")).members.find((member) => member.name === "Bob")?.viaKitAt).toBe(
      NOW - 2 * 86_400_000,
    );
  });

  it("starts the sessions: a link preview, a code invitee and a code inviter", async () => {
    vi.useFakeTimers();
    for (const [mode, expected] of [
      ["link-preview", { flow: "Link", side: "Invitee", state: "Previewing" }],
      ["code-invitee", { flow: "Code", side: "Invitee", state: "Confirming" }],
      ["code-inviter", { flow: "Code", side: "Inviter", state: "Waiting" }],
    ] as const) {
      const client = new CoreClient(new MemoryTransport(await withDemoLinks(options(`sharing=${mode}`))));
      expect((await client.inviteStatus()).state, mode).toBe("Starting");
      vi.advanceTimersByTime(DEV_SHARING_STEP_MS);
      expect(await client.inviteStatus(), mode).toMatchObject(expected);
    }
  });

  it("an unknown mode, or none, leaves every space personal", () => {
    expect(options("sharing=nonsense").sharing).toBeUndefined();
    expect(options("seed=demo").sharing).toBeUndefined();
    expect(options("shared=0").sharing).toBeUndefined();
  });

  it("starts the shared space with comment threads, a Person property and two rows in the inbox, and ?comments=none leaves them out", async () => {
    const opts = options("shared=1");
    expect(opts.sharing?.spaces?.[0]).toMatchObject({ space: "Work", comments: DEV_COMMENT_THREADS, notifications: DEV_NOTIFICATIONS });
    expect(options("shared=1&comments=none").sharing?.spaces?.[0]?.comments).toBeUndefined();
    expect(options("shared=1&comments=live").sharing?.spaces?.[0]?.comments).toBe(DEV_COMMENT_THREADS);

    const client = new CoreClient(new MemoryTransport(await withDemoLinks(opts)));
    const work = (await client.spaceList()).find((space) => space.name === "Work");
    const tree = await client.treeGet(work?.id ?? "");
    const handbook = tree.roots.find((node) => node.page.title === "Handbook");
    const meetings = tree.roots.find((node) => node.page.title === "Meetings");
    const threads = await client.commentList(handbook?.page.id ?? "");
    // One anchored to the words of the page (real cursors), one resolved on the page as a whole.
    expect(threads.map((thread) => [thread.resolved, thread.anchor?.quote])).toEqual([
      [true, undefined],
      [false, "How we work."],
    ]);
    expect(threads[1]?.anchor?.blockId).toBeDefined();
    expect(threads[1]?.anchor?.start).toBeDefined();
    expect(threads[1]?.comments.map((comment) => comment.authorName)).toEqual(["Carol", "Bob"]);
    const weekly = meetings?.children.find((node) => node.page.title === "Weekly sync");
    expect(await client.commentList(weekly?.page.id ?? "")).toHaveLength(1);
    // The inbox: one unread mention of this account, one read assignment.
    expect((await client.notificationList()).map((row) => [row.kind, row.actorName, row.read])).toEqual([
      ["CommentMention", "Bob", false],
      ["PersonAssigned", "Carol", true],
    ]);
    // The Person property of Work: this account owns Meetings.
    const owner = (await client.propertySchemaGet(work?.id ?? "")).find((def) => def.name === "Owner");
    expect(owner?.kind).toBe("person");
    const me = (await client.peopleMentionable(work?.id ?? "")).find((person) => person.isMe);
    expect((await client.pageGet(meetings?.page.id ?? "")).properties?.find(([id]) => id === owner?.id)?.[1]).toEqual({
      kind: "person",
      value: [me?.accountId],
    });
  });

  it("starts no comments without sharing: only the Person property of the demo (this account alone)", async () => {
    const client = new CoreClient(new MemoryTransport(await withDemoLinks(options("seed=demo"))));
    const work = (await client.spaceList()).find((space) => space.name === "Work");
    const meetings = (await client.treeGet(work?.id ?? "")).roots.find((node) => node.page.title === "Meetings");
    expect(await client.commentList(meetings?.page.id ?? "")).toEqual([]);
    expect(await client.notificationList()).toEqual([]);
    expect((await client.peopleMentionable(work?.id ?? "")).map((person) => person.isMe)).toEqual([true]);
  });

  it("reads ?comments= as none, live or neither", () => {
    expect(commentsMode(new URLSearchParams("comments=none"))).toBe("none");
    expect(commentsMode(new URLSearchParams("comments=live"))).toBe("live");
    expect(commentsMode(new URLSearchParams("comments=other"))).toBeNull();
    expect(commentsMode(new URLSearchParams(""))).toBeNull();
  });

  it("does not change the ?devices options that delete the workspace", () => {
    const opts = options("shared=1&devices=pair-joiner");
    expect(opts.workspaceName).toBeUndefined();
    expect(opts.spaces).toBeUndefined();
  });
});

describe("?presence=demo (Milestone 13, slice 3)", () => {
  it("reads ?presence= as demo or neither", () => {
    expect(presenceMode(new URLSearchParams("presence=demo"))).toBe("demo");
    expect(presenceMode(new URLSearchParams("presence=other"))).toBeNull();
    expect(presenceMode(new URLSearchParams(""))).toBeNull();
  });

  it("puts Bob, Carol and Dan on the pages of the shared Work space, one step every 1.2 seconds", () => {
    const opts = options("shared=1&presence=demo");
    expect(opts.presence).toEqual({ stepMs: DEV_PRESENCE_STEP_MS, peers: DEV_PRESENCE_PEERS });
    expect(DEV_PRESENCE_STEP_MS).toBe(1200);
    expect(DEV_PRESENCE_PEERS.map((peer) => peer.member)).toEqual(["Bob", "Carol", "Dan"]);
    // A moving cursor of a simulated member on the demo page: Bob edits Handbook.
    expect(DEV_PRESENCE_PEERS[0]?.route[0]).toMatchObject({ page: "Handbook", does: "edit" });
  });

  it("works for every role, and leaves presence out without sharing, for a read-only copy and for the sessions", () => {
    for (const mode of ["owner", "editor", "commenter", "viewer", "disputed", "kit-notice"]) {
      expect(options(`sharing=${mode}&presence=demo`).presence, mode).toBeDefined();
    }
    expect(options("seed=demo&presence=demo").presence).toBeUndefined();
    expect(options("shared=1").presence).toBeUndefined();
    expect(options("shared=1&presence=other").presence).toBeUndefined();
    for (const mode of ["removed", "closed", "link-preview", "code-invitee"]) {
      expect(options(`sharing=${mode}&presence=demo`).presence, mode).toBeUndefined();
    }
  });

  it("starts the people in place: avatars on Handbook for the page and for the space, and a caret in its text", async () => {
    const memory = new MemoryTransport(await withDemoLinks(options("shared=1&presence=demo")));
    const client = new CoreClient(memory);
    const work = (await client.spaceList()).find((space) => space.name === "Work");
    const handbook = (await client.treeGet(work?.id ?? "")).roots.find((node) => node.page.title === "Handbook");
    const list = await client.presenceList(work?.id ?? "");
    // Bob (join order 1, Editing) and Carol (2, Viewing) are on Handbook; Dan (3) is in Meetings.
    expect(list.map((info) => [info.name, info.color, info.state, info.pageId === handbook?.page.id])).toEqual([
      ["Bob", 1, "Editing", true],
      ["Carol", 2, "Viewing", true],
      ["Dan", 3, "Viewing", false],
    ]);
    const frames: string[] = [];
    memory.presence.subscribe((frame) => {
      if (frame.update.length > 0) {
        frames.push(frame.info.name);
      }
    });
    memory.presenceStep(2);
    expect(frames).toEqual(["Bob", "Bob"]);
  });

  it("does not disturb the ids of the demo's spaces and pages, and survives `?attachments=none`", async () => {
    const plain = new CoreClient(new MemoryTransport(await withDemoLinks(options("seed=demo&shared=1"))));
    const withPeople = new CoreClient(new MemoryTransport(await withDemoLinks(options("seed=demo&shared=1&presence=demo"))));
    expect((await withPeople.spaceList()).map((space) => space.id)).toEqual((await plain.spaceList()).map((space) => space.id));
    const memory = new MemoryTransport(await withDemoLinks(options("shared=1&presence=demo")));
    expect(withoutAttachments(memory).presence).toBe(memory.presence);
  });
});

# AI and dictation

All AI features are **optional and off by default**. They run on your own computer through
[Ollama](https://ollama.com), a separate program that you install and that runs language
models locally. atomik-kb has no cloud AI service and sends nothing to one.

## Turn on local AI

1. Install Ollama and download at least one chat model with it.
2. In atomik-kb open **Settings > AI**, turn on **Use AI features (Ollama)**.
3. The **Ollama address** defaults to a program on this same computer
   (`http://127.0.0.1:11434`). The panel shows **Connected**, the Ollama version and the number
   of models, or why it could not connect (**Check again** retries).
4. Under **Models** choose a **Chat and writing model**, and optionally an **Embedding
   model** (finds pages related to a question) and a **Transcription model** (a model that
   understands audio). **Think before answering** is available for models that can think.

If you point the address at another computer, the setting is marked **Remote: text leaves
this device**, and you are asked to confirm the change. Only do that for a machine you
control and trust.

### Semantic index

**Settings > AI > Semantic index** lets questions and the search find pages by meaning. It
builds an index on this device from your pages (shown as "N of M pages indexed"). The index
stays on this device and is never synced or backed up. **Rebuild** recreates it.

## What you can do

- **Writing assistant:** select text or use **Ask AI** in the block menu to **Continue writing,
  Improve writing, Fix spelling and grammar, Make shorter or longer, Simplify language,
  Change tone, Translate, Summarize, Explain this, Find action items**, or give your own
  instruction. The answer appears as a preview and is only applied when you choose
  **Replace** or **Insert below**; **Discard** drops it. Long texts are truncated before
  sending, and the app says so.
- **Ask AI chat:** the **Ask AI** panel (`Ctrl+J`) answers questions from your notes and
  shows the pages it used as **Sources**. Choose the scope: **This page**, **This space** or
  **All spaces**. You can copy an answer, **Insert into page** or **Save as page**.
- **Summarize page** and **Action items** from the block menu.
- **Record audio:** the `/Record audio` block records from your microphone, saves the
  recording as an encrypted attachment and, with a transcription model set, adds a
  transcript and a summary. Existing audio files can be transcribed from their menu
  (**Transcribe**; there is a size limit and it reports files with no speech).

AI never edits your notes without you choosing to apply its answer.

### AI in shared spaces

In a shared space AI is off for everyone until each member turns on **Use AI in this
space** on their own device. See [Sharing](sharing.md).

## Dictation (speech to text)

Type `/microphone` in a page and speak: each phrase is written at the cursor when you
pause. Speech is recognised on your computer and recordings are never stored or sent
anywhere. Choose and download an engine in **Settings > Dictation**:

| Engine | Notes |
|---|---|
| **Accurate** | NVIDIA Parakeet, 25 European languages; the most accurate, about 900 MB of memory while you dictate |
| **Light** | Moonshine, English only; small and quick |
| **Ollama** | The transcription model of your local AI; no download here |

Model files are downloaded by the app only when you ask (with a progress indicator and a
confirmation), or you can point to model files you already have (**Use model files...**).
The first time you use the microphone your system asks for permission; on Windows the
privacy setting that lets desktop apps use the microphone must also be on, and on macOS
the permission is under **System Settings > Privacy & Security > Microphone**.

The built-in engines come with the Windows installer, the Linux `.deb` package and the macOS app (Apple Silicon).
The **Ollama** engine works everywhere.

## MCP server (let other AI tools read your notes)

The optional **MCP server** lets AI tools on the same computer, such as Claude Desktop or
Claude Code, search and read your notes. In **Settings > AI > MCP server**:

1. Turn on **Run the MCP server**. It listens on this computer only (default port 47811;
   change it between 1024 and 65535).
2. **Add a client**: give it a name. You get a **key shown once**, plus a ready-made
   configuration for Claude Desktop and a command for Claude Code. They use a small
   bridge program (`kb-mcp-bridge`) installed next to the app.
3. Under **Permissions**, choose for each space whether the client may **Read** or also
   **Write**. A new client sees nothing until you allow it, and each grant asks you to
   confirm. **Read** lets the tool search, list and read pages; **Write** also lets it
   create pages and add to them.
4. **Activity** lists every call (time, client, tool, target, outcome). Deleting a client
   makes its key stop working immediately.

A tool you connect gets the same access the permission grants. Anything it reads is passed
to that tool, so only connect programs you trust, and grant the minimum.

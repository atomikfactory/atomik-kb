//! MCP clients, grants and the audit log, and the execution of MCP messages (ADR-0037,
//! `docs/architecture/ai.md` section 7): `McpCall` checks the token, hands the message to
//! `kb_mcp::handle` with the core as its `McpBackend` (grants checked and every tool call audited),
//! and answers the JSON-RPC reply. The tables are `kb-storage`'s `McpQueries`.
//!
//! - **Clients.** A token is `kbm_` and 43 base64url characters (32 bytes of the core's `Rng`),
//!   shown once; only its SHA-256 is stored. A client id is 32 lower-case hex characters. Grants are
//!   per space: write implies read, a grant of a space that does not exist is dropped, duplicates
//!   are merged. A client without a grant for a space cannot tell whether the space exists.
//! - **Audit.** Every `tools/call` leaves one row (also a refused one): time, client, tool, space
//!   and page, a summary (the query, the title of the page, or for a refused call the reason; never
//!   page text, at most 200 characters) and the outcome. If the row cannot be written the call fails:
//!   nothing is answered without a record.
//! - **Writes.** Pages made through MCP are created with `StructureDoc::create_pages`, whose commit
//!   origin the person's undo excludes (as an import's), and their content is converted with
//!   `kb-import`'s Markdown reading and appended to the page document with
//!   `PageDoc::append_portable`. Links to pages and images stay text.

use std::collections::HashSet;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use kb_import::{ImportOptions, Resolver, SourceFile};
use kb_mcp::{McpBackend, ToolCall, ToolResult};
use kb_model::{NewPage, StructureDoc};
use kb_storage::{DocQueries, McpAuditRow, McpClientRow, McpQueries, NewMcpAudit, UpdateOrigin};
use kb_types::ai::{
    McpAuditEntry, McpClientCreated, McpClientInfo, McpGrant, McpOutcome, McpReply,
};
use kb_types::{
    AccountId, ApiError, AttachmentId, ChangeSource, ErrorCode, Event, FormulaResult, MemberRole,
    OptionId, PageId, PageNode, PropertyDef, PropertyId, PropertyValue, Request, Response, RngExt,
    SearchHit, SearchScope, SpaceId, SpaceInfo,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::documents::map_model_error;
use crate::{Core, map_storage_error, validate};

/// Most characters of a client name.
const MAX_CLIENT_NAME_CHARS: usize = 60;
/// Largest message `McpCall` takes, in bytes (the shell's body limit).
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// How a token starts.
const TOKEN_PREFIX: &str = "kbm_";
/// Characters after the prefix: 32 bytes in base64url without padding.
const TOKEN_BODY_CHARS: usize = 43;
/// Most characters of an audit summary.
const AUDIT_SUMMARY_CHARS: usize = 200;
/// Most characters of a tool name in the audit log.
const AUDIT_TOOL_CHARS: usize = 64;
/// Rows `McpAuditList` answers when `limit` is left out.
const AUDIT_DEFAULT_LIMIT: u32 = 100;
/// Most rows `McpAuditList` answers.
const AUDIT_MAX_LIMIT: u32 = 500;
/// Most bytes of Markdown `read_page` answers.
const READ_PAGE_MAX_BYTES: usize = kb_mcp::tools::MAX_MARKDOWN_BYTES;
/// Most pages `list_pages` names.
const LIST_PAGES_MAX: usize = 200;
/// Most characters of a snippet in a search result.
const SNIPPET_CHARS: usize = 300;
/// Most characters of one property value in the properties line.
const PROPERTY_VALUE_CHARS: usize = 200;
/// Most characters of the properties line.
const PROPERTIES_LINE_CHARS: usize = 1_000;
/// The name of the file the Markdown of a write is read as (a name a heading never equals).
const MARKDOWN_PATH: &str = "mcp-markdown.md";
/// What a client without access hears from `list_spaces` and `search`.
const NO_SPACES: &str = "No spaces are shared with this client yet. The person can grant access \
in atomik-kb under Settings > AI > MCP.";

// ----- requests -------------------------------------------------------------------------------

impl Core {
    pub(crate) fn dispatch_mcp(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::McpClientList => Ok(Response::McpClients(self.mcp_client_list()?)),
            Request::McpClientCreate { name } => {
                Ok(Response::McpClientCreated(self.mcp_client_create(&name)?))
            }
            Request::McpClientGrantsSet { client_id, grants } => Ok(Response::McpClient(
                self.mcp_client_grants_set(&client_id, &grants)?,
            )),
            Request::McpClientDelete { client_id } => {
                self.mcp_client_delete(&client_id)?;
                Ok(Response::Ok)
            }
            Request::McpAuditList {
                client_id,
                before,
                limit,
            } => Ok(Response::McpAudit(self.mcp_audit_list(
                client_id.as_deref(),
                before,
                limit,
            )?)),
            Request::McpAuditClear => {
                self.db.mcp_audit_clear().map_err(map_storage_error)?;
                self.events.push(Event::McpChanged);
                Ok(Response::Ok)
            }
            Request::McpCall { token, message } => {
                Ok(Response::McpReply(self.mcp_call(&token, &message)?))
            }
            other => Err(ApiError::internal(format!(
                "request {} is not an MCP request",
                other.kind()
            ))),
        }
    }

    fn mcp_client_list(&self) -> Result<Vec<McpClientInfo>, ApiError> {
        let rows = self.db.mcp_clients_list().map_err(map_storage_error)?;
        Ok(rows
            .into_iter()
            .map(|row| self.mcp_client_info(row))
            .collect())
    }

    /// A client as the UI sees it: the grants of spaces that no longer exist are left out.
    fn mcp_client_info(&self, row: McpClientRow) -> McpClientInfo {
        // A space whose AI is off on this account's devices is not shared with any client
        // (ADR-0047): a grant that outlived the switch does not count.
        let grants = row
            .grants
            .into_iter()
            .filter(|grant| {
                self.docs.structures.contains_key(&grant.space_id)
                    && self.ai_space_enabled(grant.space_id)
            })
            .collect();
        McpClientInfo {
            id: row.id,
            name: row.name,
            grants,
            created_at: row.created_at,
            last_used_at: row.last_used_at,
        }
    }

    fn mcp_client_create(&mut self, name: &str) -> Result<McpClientCreated, ApiError> {
        let name = validate::display_name(name, MAX_CLIENT_NAME_CHARS, "name")?;
        let id = hex(&self.rng.random_array::<16>());
        let secret: [u8; 32] = self.rng.random_array();
        let token = format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(secret));
        let row = McpClientRow {
            id,
            name,
            token_hash: sha256(&token),
            grants: Vec::new(),
            created_at: self.clock.now_ms(),
            last_used_at: None,
        };
        self.db.mcp_client_insert(&row).map_err(map_storage_error)?;
        self.events.push(Event::McpChanged);
        Ok(McpClientCreated {
            client: self.mcp_client_info(row),
            token,
        })
    }

    fn mcp_client_grants_set(
        &mut self,
        client_id: &str,
        grants: &[McpGrant],
    ) -> Result<McpClientInfo, ApiError> {
        // A space whose AI is off cannot be granted (ADR-0047): its member opts in first.
        if grants.iter().any(|grant| {
            (grant.read || grant.write)
                && self.docs.structures.contains_key(&grant.space_id)
                && !self.ai_space_enabled(grant.space_id)
        }) {
            return Err(ApiError::invalid(
                "grants",
                "AI is off for a space in these grants: turn it on for the space first",
            ));
        }
        let grants = self.mcp_normalise_grants(grants);
        let found = self
            .db
            .mcp_client_grants_set(client_id, &grants)
            .map_err(map_storage_error)?;
        if !found {
            return Err(ApiError::not_found("client not found"));
        }
        self.events.push(Event::McpChanged);
        let row = self
            .db
            .mcp_client_get(client_id)
            .map_err(map_storage_error)?
            .ok_or_else(|| ApiError::not_found("client not found"))?;
        Ok(self.mcp_client_info(row))
    }

    /// Takes a space out of every client's grants (its AI went off, ADR-0047, or the space is gone).
    pub(crate) fn mcp_revoke_space(&mut self, space_id: SpaceId) -> Result<(), ApiError> {
        let mut changed = false;
        for client in self.db.mcp_clients_list().map_err(map_storage_error)? {
            if !client.grants.iter().any(|grant| grant.space_id == space_id) {
                continue;
            }
            let kept: Vec<McpGrant> = client
                .grants
                .iter()
                .filter(|grant| grant.space_id != space_id)
                .copied()
                .collect();
            self.db
                .mcp_client_grants_set(&client.id, &kept)
                .map_err(map_storage_error)?;
            changed = true;
        }
        if changed {
            self.events.push(Event::McpChanged);
        }
        Ok(())
    }

    /// Write implies read, grants of unknown spaces and grants of nothing are dropped, duplicates
    /// merge (first appearance keeps its place).
    fn mcp_normalise_grants(&self, grants: &[McpGrant]) -> Vec<McpGrant> {
        let mut merged: Vec<McpGrant> = Vec::new();
        for grant in grants {
            if !self.docs.structures.contains_key(&grant.space_id) {
                continue;
            }
            match merged.iter_mut().find(|m| m.space_id == grant.space_id) {
                Some(known) => {
                    known.read |= grant.read;
                    known.write |= grant.write;
                }
                None => merged.push(*grant),
            }
        }
        merged.retain(|grant| grant.read || grant.write);
        for grant in &mut merged {
            grant.read = true;
        }
        merged
    }

    fn mcp_client_delete(&mut self, client_id: &str) -> Result<(), ApiError> {
        let deleted = self
            .db
            .mcp_client_delete(client_id)
            .map_err(map_storage_error)?;
        if !deleted {
            return Err(ApiError::not_found("client not found"));
        }
        self.events.push(Event::McpChanged);
        Ok(())
    }

    fn mcp_audit_list(
        &self,
        client_id: Option<&str>,
        before: Option<i64>,
        limit: Option<u32>,
    ) -> Result<Vec<McpAuditEntry>, ApiError> {
        let limit = limit.unwrap_or(AUDIT_DEFAULT_LIMIT);
        if !(1..=AUDIT_MAX_LIMIT).contains(&limit) {
            return Err(ApiError::invalid(
                "limit",
                format!("must be from 1 to {AUDIT_MAX_LIMIT}"),
            ));
        }
        let rows = self
            .db
            .mcp_audit_list(client_id, before, limit)
            .map_err(map_storage_error)?;
        Ok(rows.into_iter().map(audit_entry).collect())
    }

    // ----- the call -----------------------------------------------------------------------------

    /// One message from the loopback server: the token is checked (`InvalidInput` for `token`),
    /// the message handled by `kb-mcp` with this core as the backend.
    fn mcp_call(&mut self, token: &str, message: &str) -> Result<McpReply, ApiError> {
        let client = self.mcp_authenticate(token)?;
        if message.len() > MAX_MESSAGE_BYTES {
            return Err(ApiError::invalid(
                "message",
                format!("must be at most {MAX_MESSAGE_BYTES} bytes"),
            ));
        }
        let now = self.clock.now_ms();
        self.db
            .mcp_client_touch(&client.id, now)
            .map_err(map_storage_error)?;
        let client = self.mcp_client_info(McpClientRow {
            last_used_at: Some(now),
            ..client
        });
        let mut backend = CoreBackend {
            core: self,
            client,
            failure: None,
        };
        let body = kb_mcp::handle(message, &mut backend);
        match backend.failure.take() {
            Some(err) => Err(err),
            None => Ok(McpReply { body }),
        }
    }

    /// The client a token belongs to. A token of the wrong shape is refused without a lookup.
    fn mcp_authenticate(&self, token: &str) -> Result<McpClientRow, ApiError> {
        let unknown = || ApiError::invalid("token", "unknown token");
        if !token_shape_ok(token) {
            return Err(unknown());
        }
        self.db
            .mcp_client_by_token_hash(&sha256(token))
            .map_err(map_storage_error)?
            .ok_or_else(unknown)
    }

    // ----- writes -------------------------------------------------------------------------------

    /// A page's Markdown (as `PageExportMarkdown` gives it), except that a reference to a page of
    /// a space that `readable` refuses is left as the id: a readable page does not tell the title
    /// of a page the client may not read.
    fn mcp_page_markdown(
        &mut self,
        page_id: PageId,
        space_id: SpaceId,
        readable: &dyn Fn(SpaceId) -> bool,
    ) -> Result<String, ApiError> {
        let transient = if self.docs.pages.contains_key(&page_id) {
            None
        } else {
            Some(self.load_or_create_page(page_id, space_id)?)
        };
        let model = match (&transient, self.docs.pages.get(&page_id)) {
            (Some(open), _) | (None, Some(open)) => &open.model,
            (None, None) => return Err(ApiError::internal("page document vanished")),
        };
        let db = &self.db;
        Ok(model.to_markdown(&|id| {
            let row = db.pages_index_get(id).ok().flatten()?;
            readable(row.space_id).then_some(row.title)
        }))
    }

    /// Markdown as portable page JSON, read as one file of `kb-import`: links to pages and images
    /// stay text, front matter and properties are dropped.
    fn mcp_markdown_content(&self, markdown: &str) -> Result<Value, ApiError> {
        let files = vec![SourceFile {
            path: MARKDOWN_PATH.to_owned(),
            text: markdown.to_owned(),
        }];
        let options = ImportOptions {
            source_name: None,
            max_pages: 1,
        };
        let plan = kb_import::plan(files, &[], &options);
        let page = plan
            .pages
            .first()
            .ok_or_else(|| ApiError::internal("the Markdown made no page"))?;
        let (content, _warnings) = kb_import::resolve(page, &NoLinks, self.rng.as_ref());
        Ok(content)
    }

    /// Creates a page (not an undo step) with its document, and appends `content` to it.
    fn mcp_create_page(
        &mut self,
        space_id: SpaceId,
        parent_id: Option<PageId>,
        title: &str,
        content: Option<&Value>,
    ) -> Result<PageId, ApiError> {
        let title = validate::page_title(title)?;
        // A client writes as this account: a Viewer's or Commenter's grant does not make it an
        // Editor (ADR-0043).
        self.structure(space_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        let page_id = PageId::generate(self.rng.as_ref());
        let now = self.clock.now_ms();
        self.structure_mut(space_id)?
            .model
            .create_pages(
                &[NewPage {
                    page_id,
                    parent: parent_id,
                    title: title.clone(),
                    icon: None,
                    cover: None,
                    cover_y: None,
                }],
                now,
            )
            .map_err(map_model_error)?;
        self.persist_structure(space_id, Some(&[page_id]), UpdateOrigin::Local)?;
        self.events.push(Event::StructureChanged { space_id });
        self.events.push(Event::SpacesChanged);
        self.text_after_create(page_id, &title)?;
        self.load_or_create_page(page_id, space_id)?;
        if let Some(content) = content {
            self.mcp_append_blocks(page_id, content)?;
        }
        Ok(page_id)
    }

    /// Appends the blocks of portable JSON to a page. A page that holds nothing but its first,
    /// empty paragraph loses that paragraph. Answers the blocks appended.
    fn mcp_append_blocks(&mut self, page_id: PageId, content: &Value) -> Result<usize, ApiError> {
        let space_id = self.space_of_page(page_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        let appended = {
            let open = self.open_page(page_id)?;
            let model = &open.model;
            let ids = model.block_ids();
            let only_blank = match ids.as_slice() {
                [only] => {
                    model.is_empty()
                        && model
                            .blocks()
                            .iter()
                            .any(|block| block.id == *only && block.kind == "paragraph")
                }
                _ => false,
            };
            let appended = model.append_portable(content).map_err(map_model_error)?;
            if appended > 0
                && only_blank
                && let Some(first) = ids.first()
            {
                model.delete_block(*first).map_err(map_model_error)?;
            }
            appended
        };
        if appended > 0 {
            self.persist_page(page_id, None, UpdateOrigin::Core, ChangeSource::Core)?;
            self.page_content_edited(page_id)?;
        }
        Ok(appended)
    }
}

/// What `kb_import::resolve` asks the core: nothing. Links to pages and attachments become text.
struct NoLinks;

impl Resolver for NoLinks {
    fn page(&self, _index: usize) -> Option<PageId> {
        None
    }
    fn page_by_title(&self, _title: &str) -> Option<PageId> {
        None
    }
    fn existing_page(&self, _page: PageId) -> bool {
        false
    }
    fn attachment(&self, _path: &str) -> Option<AttachmentId> {
        None
    }
    fn existing_attachment(&self, _attachment: AttachmentId) -> bool {
        false
    }
    fn property(&self, _name: &str) -> Option<PropertyId> {
        None
    }
    fn option(&self, _property: &str, _option: &str) -> Option<OptionId> {
        None
    }
}

// ----- the backend ----------------------------------------------------------------------------

/// The core as the backend of one MCP call for one client.
struct CoreBackend<'a> {
    core: &'a mut Core,
    client: McpClientInfo,
    /// A failure to write the audit row: the call is then failed as a whole.
    failure: Option<ApiError>,
}

/// What a tool run says about itself for the audit row.
#[derive(Default)]
struct Trail {
    space_id: Option<SpaceId>,
    page_id: Option<PageId>,
    summary: String,
}

/// Why a tool did not do its work; the text is what the model reads.
enum Failure {
    /// No grant.
    Denied(String),
    /// Bad arguments.
    Invalid(String),
    /// Anything else: unknown page, a page in the trash, a failure in the core.
    Error(String),
}

impl Failure {
    fn invalid(text: impl Into<String>) -> Self {
        Self::Invalid(text.into())
    }

    fn error(text: impl Into<String>) -> Self {
        Self::Error(text.into())
    }

    /// An error of the core as the model hears it: bad input and missing things as they are,
    /// everything else as a plain failure (no internals).
    fn from_api(err: &ApiError) -> Self {
        match err.code {
            ErrorCode::InvalidInput => Self::Invalid(err.message.clone()),
            ErrorCode::NotFound => Self::Error(err.message.clone()),
            _ => Self::Error("atomik-kb could not do that".to_owned()),
        }
    }
}

impl From<ApiError> for Failure {
    fn from(err: ApiError) -> Self {
        Self::from_api(&err)
    }
}

impl McpBackend for CoreBackend<'_> {
    fn call_tool(&mut self, call: ToolCall) -> ToolResult {
        let tool = call.tool_name();
        let mut trail = Trail::default();
        let (result, outcome) = match self.run(call, &mut trail) {
            Ok(text) => (ToolResult::ok(text), McpOutcome::Ok),
            Err(Failure::Denied(text)) => (ToolResult::error(text), McpOutcome::Denied),
            Err(Failure::Invalid(text)) => (ToolResult::error(text), McpOutcome::Invalid),
            Err(Failure::Error(text)) => (ToolResult::error(text), McpOutcome::Error),
        };
        self.audit(tool, &trail, outcome);
        result
    }

    fn invalid_call(&mut self, tool: &str, reason: &str) {
        let trail = Trail {
            summary: reason.to_owned(),
            ..Trail::default()
        };
        self.audit(tool, &trail, McpOutcome::Invalid);
    }
}

impl CoreBackend<'_> {
    /// Writes the audit row and announces it. A failure is kept: the call fails as a whole.
    fn audit(&mut self, tool: &str, trail: &Trail, outcome: McpOutcome) {
        let tool = if tool.is_empty() { "(none)" } else { tool };
        let row = NewMcpAudit {
            at: self.core.clock.now_ms(),
            client_id: self.client.id.clone(),
            client_name: self.client.name.clone(),
            tool: tool.chars().take(AUDIT_TOOL_CHARS).collect(),
            space_id: trail.space_id,
            page_id: trail.page_id,
            summary: clip(&one_line(&trail.summary), AUDIT_SUMMARY_CHARS),
            outcome,
        };
        match self.core.db.mcp_audit_insert(&row) {
            Ok(_) => self.core.events.push(Event::McpChanged),
            Err(err) => {
                tracing::error!("an MCP audit row could not be written");
                self.failure = Some(map_storage_error(err));
            }
        }
    }

    fn can_read(&self, space_id: SpaceId) -> bool {
        self.client
            .grants
            .iter()
            .any(|g| g.space_id == space_id && (g.read || g.write))
    }

    fn can_write(&self, space_id: SpaceId) -> bool {
        self.client
            .grants
            .iter()
            .any(|g| g.space_id == space_id && g.write)
    }

    fn require_read(&self, space_id: SpaceId) -> Result<(), Failure> {
        if self.can_read(space_id) {
            Ok(())
        } else {
            Err(Failure::Denied(
                "This client has no read access to that space".to_owned(),
            ))
        }
    }

    fn require_write(&self, space_id: SpaceId) -> Result<(), Failure> {
        if self.can_write(space_id) {
            Ok(())
        } else {
            Err(Failure::Denied(
                "This client has no write access to that space".to_owned(),
            ))
        }
    }

    /// The spaces the client can read, in the person's order.
    fn readable_spaces(&self) -> Result<Vec<SpaceInfo>, Failure> {
        let spaces = self.core.space_list()?;
        Ok(spaces
            .into_iter()
            .filter(|space| self.can_read(space.id))
            .collect())
    }

    fn run(&mut self, call: ToolCall, trail: &mut Trail) -> Result<String, Failure> {
        match call {
            ToolCall::ListSpaces => self.list_spaces(),
            ToolCall::Search {
                query,
                space_id,
                limit,
            } => self.search(&query, space_id.as_deref(), limit, trail),
            ToolCall::ListPages {
                space_id,
                parent_id,
            } => self.list_pages(&space_id, parent_id.as_deref(), trail),
            ToolCall::ReadPage { page_id } => self.read_page(&page_id, trail),
            ToolCall::CreatePage {
                space_id,
                parent_id,
                title,
                markdown,
            } => self.create_page(
                &space_id,
                parent_id.as_deref(),
                &title,
                markdown.as_deref(),
                trail,
            ),
            ToolCall::AppendToPage { page_id, markdown } => {
                self.append_to_page(&page_id, &markdown, trail)
            }
        }
    }

    // ----- list_spaces ----------------------------------------------------------------------------

    fn list_spaces(&self) -> Result<String, Failure> {
        let spaces = self.readable_spaces()?;
        if spaces.is_empty() {
            return Ok(NO_SPACES.to_owned());
        }
        let mut out = format!("Spaces you can access ({}):", spaces.len());
        for space in &spaces {
            let access = if self.can_write(space.id) {
                "read and write"
            } else {
                "read only"
            };
            out.push_str(&format!(
                "\n- {} (spaceId: {}, {}): {access}",
                space.name,
                space.id,
                count(u64::from(space.page_count), "page"),
            ));
        }
        Ok(out)
    }

    // ----- search ---------------------------------------------------------------------------------

    fn search(
        &mut self,
        query: &str,
        space: Option<&str>,
        limit: u32,
        trail: &mut Trail,
    ) -> Result<String, Failure> {
        trail.summary = query.to_owned();
        let spaces: Vec<SpaceInfo> = match space {
            Some(raw) => {
                let space_id = parse_space(raw)?;
                trail.space_id = Some(space_id);
                self.require_read(space_id)?;
                vec![
                    self.core
                        .space_list()?
                        .into_iter()
                        .find(|s| s.id == space_id)
                        .ok_or_else(|| Failure::error("Space not found"))?,
                ]
            }
            None => self.readable_spaces()?,
        };
        if spaces.is_empty() {
            return Ok(NO_SPACES.to_owned());
        }
        // Several blocks of a page may match: ask for more hits than pages wanted.
        let fetch = (limit.saturating_mul(5)).min(100);
        let mut lists: Vec<std::vec::IntoIter<SearchHit>> = Vec::with_capacity(spaces.len());
        for space in &spaces {
            // The page-level match (ADR-0040) but not the fuzzy titles: every page listed has the words.
            let hits = self.core.search_pages(
                query,
                Some(space.id),
                SearchScope::All,
                Some(fetch),
                None,
                false,
            )?;
            let mut seen: HashSet<PageId> = HashSet::new();
            let pages: Vec<SearchHit> = hits
                .into_iter()
                .filter(|hit| seen.insert(hit.page_id))
                .collect();
            lists.push(pages.into_iter());
        }
        // Best first inside each space; the spaces take turns.
        let mut hits: Vec<SearchHit> = Vec::new();
        'merge: loop {
            let before = hits.len();
            for list in &mut lists {
                if let Some(hit) = list.next() {
                    hits.push(hit);
                    if hits.len() >= limit as usize {
                        break 'merge;
                    }
                }
            }
            if hits.len() == before {
                break;
            }
        }
        if hits.is_empty() {
            return Ok(format!("No pages found for \"{query}\"."));
        }
        let mut out = format!(
            "Found {} for \"{query}\":",
            count(hits.len() as u64, "page")
        );
        for (n, hit) in hits.iter().enumerate() {
            let space_name = spaces
                .iter()
                .find(|s| s.id == hit.space_id)
                .map_or("", |s| s.name.as_str());
            out.push_str(&format!(
                "\n\n{}. {}\n   pageId: {} | space: {} (spaceId: {})",
                n + 1,
                title_or_untitled(&hit.title),
                hit.page_id,
                space_name,
                hit.space_id
            ));
            let snippet = clip(&one_line(&hit.snippet), SNIPPET_CHARS);
            if !snippet.is_empty() && snippet != one_line(&hit.title) {
                out.push_str("\n   ");
                out.push_str(&snippet);
            }
        }
        Ok(out)
    }

    // ----- list_pages -----------------------------------------------------------------------------

    fn list_pages(
        &mut self,
        space: &str,
        parent: Option<&str>,
        trail: &mut Trail,
    ) -> Result<String, Failure> {
        let space_id = parse_space(space)?;
        trail.space_id = Some(space_id);
        self.require_read(space_id)?;
        let open = self.core.structure(space_id)?;
        let space_name = open.model.space().name;
        let roots = open.model.live_tree();

        let (heading, children): (String, &[PageNode]) = match parent {
            None => {
                trail.summary.clone_from(&space_name);
                (format!("Top-level pages in \"{space_name}\""), &roots)
            }
            Some(raw) => {
                let page_id = parse_page(raw)?;
                trail.page_id = Some(page_id);
                let info = self
                    .core
                    .page_get(page_id)
                    .map_err(|_| Failure::error("Page not found"))?;
                if info.space_id != space_id {
                    return Err(Failure::invalid("parentId is not a page of that space"));
                }
                if info.trashed_at.is_some() {
                    return Err(Failure::error("That page is in the trash"));
                }
                // Templates and what is inside them are not listed (ADR-0056): their pages are not found
                // here (`read_page` by id still works).
                if open.model.in_template(page_id) {
                    return Err(Failure::error("Page not found"));
                }
                trail.summary.clone_from(&info.title);
                let node =
                    find_node(&roots, page_id).ok_or_else(|| Failure::error("Page not found"))?;
                (
                    format!("Subpages of \"{}\"", title_or_untitled(&info.title)),
                    &node.children,
                )
            }
        };
        // The templates among them (and their sub-pages) are left out, as are the templates among the
        // sub-pages that are counted.
        let children: Vec<&PageNode> = children.iter().filter(|node| !node.page.template).collect();
        if children.is_empty() {
            return Ok(format!("{heading}: none."));
        }
        let mut out = format!("{heading} ({}):", children.len());
        for node in children.iter().take(LIST_PAGES_MAX) {
            out.push_str(&format!(
                "\n- {} (pageId: {}",
                title_or_untitled(&node.page.title),
                node.page.id
            ));
            let subpages = node
                .children
                .iter()
                .filter(|child| !child.page.template)
                .count();
            if subpages > 0 {
                out.push_str(&format!(", {}", count(subpages as u64, "subpage")));
            }
            out.push(')');
        }
        if children.len() > LIST_PAGES_MAX {
            out.push_str(&format!(
                "\n... and {} more. Use search to find a particular page.",
                children.len() - LIST_PAGES_MAX
            ));
        }
        Ok(out)
    }

    // ----- read_page ------------------------------------------------------------------------------

    fn read_page(&mut self, page: &str, trail: &mut Trail) -> Result<String, Failure> {
        let page_id = parse_page(page)?;
        trail.page_id = Some(page_id);
        let info = self
            .core
            .page_get(page_id)
            .map_err(|_| Failure::error("Page not found"))?;
        trail.space_id = Some(info.space_id);
        self.require_read(info.space_id)?;
        if info.trashed_at.is_some() {
            return Err(Failure::error("That page is in the trash"));
        }
        trail.summary.clone_from(&info.title);
        let space_name = self.core.structure(info.space_id)?.model.space().name;
        // An inline reference names a page only when the client can read that page's space.
        let readable: HashSet<SpaceId> = self
            .client
            .grants
            .iter()
            .filter(|grant| grant.read || grant.write)
            .map(|grant| grant.space_id)
            .collect();
        let markdown = self
            .core
            .mcp_page_markdown(page_id, info.space_id, &|space| readable.contains(&space))?;
        // An empty first paragraph (every new page has one) renders as blank lines.
        let markdown = markdown.trim_start_matches('\n');

        let mut out = format!("# {}\n", one_line(&info.title));
        out.push_str(&format!(
            "pageId: {page_id} | space: {space_name} (spaceId: {})",
            info.space_id
        ));
        if let Some(parent) = info.parent_id {
            out.push_str(&format!(" | parentId: {parent}"));
        }
        out.push('\n');
        if let Some(line) = self.properties_line(info.space_id, page_id) {
            out.push_str("properties: ");
            out.push_str(&line);
            out.push('\n');
        }
        out.push('\n');
        if markdown.len() > READ_PAGE_MAX_BYTES {
            let mut end = READ_PAGE_MAX_BYTES;
            while !markdown.is_char_boundary(end) {
                end -= 1;
            }
            out.push_str(&markdown[..end]);
            out.push_str(&format!(
                "\n\n[Truncated: the page is {} KiB of Markdown; the first {} KiB are shown.]",
                markdown.len().div_ceil(1024),
                READ_PAGE_MAX_BYTES / 1024
            ));
        } else {
            out.push_str(markdown);
        }
        Ok(out)
    }

    /// `Name: value; Name: value` for the page's stored property values and its formulas (a formula
    /// is what the page shows, ADR-0055; the other computed values are left out), or `None` without
    /// any.
    fn properties_line(&self, space_id: SpaceId, page_id: PageId) -> Option<String> {
        let model = &self.core.structure(space_id).ok()?.model;
        let values = model.page_properties(page_id).ok()?;
        let schema = model.property_schema();
        let parts: Vec<String> = values
            .iter()
            .filter(|(_, value)| {
                !value.kind().is_computed() || matches!(value, PropertyValue::Formula(_))
            })
            .filter_map(|(id, value)| {
                let def = schema.iter().find(|def| def.id == *id)?;
                let people = |account: AccountId| {
                    let (name, former) = self.core.person_in_space(space_id, account);
                    if former {
                        "Former member".to_owned()
                    } else {
                        name
                    }
                };
                let text = property_text(model, def, value, &people)?;
                Some(format!("{}: {}", one_line(&def.name), text))
            })
            .collect();
        (!parts.is_empty()).then(|| clip(&parts.join("; "), PROPERTIES_LINE_CHARS))
    }

    // ----- create_page ----------------------------------------------------------------------------

    fn create_page(
        &mut self,
        space: &str,
        parent: Option<&str>,
        title: &str,
        markdown: Option<&str>,
        trail: &mut Trail,
    ) -> Result<String, Failure> {
        let space_id = parse_space(space)?;
        trail.space_id = Some(space_id);
        trail.summary = title.to_owned();
        self.require_write(space_id)?;
        let parent_id = match parent {
            Some(raw) => {
                let parent_id = parse_page(raw)?;
                let info = self
                    .core
                    .page_get(parent_id)
                    .map_err(|_| Failure::error("Parent page not found"))?;
                if info.space_id != space_id {
                    return Err(Failure::invalid("parentId is not a page of that space"));
                }
                if info.trashed_at.is_some() {
                    return Err(Failure::error("The parent page is in the trash"));
                }
                Some(parent_id)
            }
            None => None,
        };
        let content = match markdown {
            Some(markdown) => {
                let mut content = self.core.mcp_markdown_content(markdown)?;
                drop_leading_title(&mut content, title);
                (!is_blank(&content)).then_some(content)
            }
            None => None,
        };
        let page_id = self
            .core
            .mcp_create_page(space_id, parent_id, title, content.as_ref())?;
        trail.page_id = Some(page_id);
        let space_name = self.core.structure(space_id)?.model.space().name;
        Ok(format!(
            "Created page \"{}\" (pageId: {page_id}) in space \"{space_name}\" (spaceId: {space_id}).",
            one_line(title)
        ))
    }

    // ----- append_to_page ---------------------------------------------------------------------------

    fn append_to_page(
        &mut self,
        page: &str,
        markdown: &str,
        trail: &mut Trail,
    ) -> Result<String, Failure> {
        let page_id = parse_page(page)?;
        trail.page_id = Some(page_id);
        let info = self
            .core
            .page_get(page_id)
            .map_err(|_| Failure::error("Page not found"))?;
        trail.space_id = Some(info.space_id);
        self.require_write(info.space_id)?;
        if info.trashed_at.is_some() {
            return Err(Failure::error("That page is in the trash"));
        }
        trail.summary.clone_from(&info.title);
        let content = self.core.mcp_markdown_content(markdown)?;
        if is_blank(&content) {
            return Err(Failure::invalid("markdown has no content to append"));
        }
        let appended = self.core.mcp_append_blocks(page_id, &content)?;
        Ok(format!(
            "Appended {} to \"{}\" (pageId: {page_id}).",
            count(appended as u64, "block"),
            title_or_untitled(&info.title)
        ))
    }
}

// ----- helpers --------------------------------------------------------------------------------

fn parse_space(raw: &str) -> Result<SpaceId, Failure> {
    raw.parse()
        .map_err(|_| Failure::invalid("spaceId is not a valid id. Use an id from list_spaces."))
}

fn parse_page(raw: &str) -> Result<PageId, Failure> {
    raw.parse().map_err(|_| {
        Failure::invalid("pageId is not a valid id. Use an id from search or list_pages.")
    })
}

/// The node of a page in a tree of live pages.
fn find_node(roots: &[PageNode], page_id: PageId) -> Option<&PageNode> {
    let mut stack: Vec<&PageNode> = roots.iter().collect();
    while let Some(node) = stack.pop() {
        if node.page.id == page_id {
            return Some(node);
        }
        stack.extend(node.children.iter());
    }
    None
}

fn audit_entry(row: McpAuditRow) -> McpAuditEntry {
    McpAuditEntry {
        id: row.id,
        at: row.at,
        client_id: row.client_id,
        client_name: row.client_name,
        tool: row.tool,
        space_id: row.space_id,
        page_id: row.page_id,
        summary: row.summary,
        outcome: row.outcome,
    }
}

/// `kbm_` and 43 base64url characters.
fn token_shape_ok(token: &str) -> bool {
    token.strip_prefix(TOKEN_PREFIX).is_some_and(|body| {
        body.len() == TOKEN_BODY_CHARS
            && body
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

fn sha256(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// Control characters become spaces, runs of whitespace one space, the ends are trimmed.
fn one_line(text: &str) -> String {
    let spaced: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// At most `max` characters; a cut text ends with an ellipsis.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

fn title_or_untitled(title: &str) -> String {
    let title = one_line(title);
    if title.is_empty() {
        "(untitled)".to_owned()
    } else {
        title
    }
}

/// `1 page`, `2 pages`.
fn count(n: u64, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Whether portable content holds nothing: no block, or only paragraphs without any content (what
/// `kb-import` makes of blank Markdown).
fn is_blank(content: &Value) -> bool {
    content
        .get("content")
        .and_then(Value::as_array)
        .is_none_or(|blocks| {
            blocks.iter().all(|block| {
                block.get("type").and_then(Value::as_str) == Some("paragraph")
                    && block
                        .get("content")
                        .and_then(Value::as_array)
                        .is_none_or(Vec::is_empty)
            })
        })
}

/// Removes the first block of portable content when it is a level-1 heading with the page's title
/// (a model often starts with the title the page already has).
fn drop_leading_title(content: &mut Value, title: &str) {
    fn plain(node: &Value, out: &mut String) {
        if let Some(text) = node.get("text").and_then(Value::as_str) {
            out.push_str(text);
        }
        if let Some(children) = node.get("content").and_then(Value::as_array) {
            for child in children {
                plain(child, out);
            }
        }
    }
    let Some(blocks) = content.get_mut("content").and_then(Value::as_array_mut) else {
        return;
    };
    let Some(first) = blocks.first() else {
        return;
    };
    let is_title = first.get("type").and_then(Value::as_str) == Some("heading")
        && first
            .get("attrs")
            .and_then(|attrs| attrs.get("level"))
            .and_then(Value::as_i64)
            == Some(1);
    if !is_title {
        return;
    }
    let mut text = String::new();
    plain(first, &mut text);
    if one_line(&text).to_lowercase() == one_line(title).to_lowercase() {
        blocks.remove(0);
    }
}

/// A property value as a short text, or `None` for a computed value or one with nothing to show.
fn property_text(
    model: &StructureDoc,
    def: &PropertyDef,
    value: &PropertyValue,
    people: &dyn Fn(AccountId) -> String,
) -> Option<String> {
    let option_name = |id: &OptionId| {
        def.options
            .iter()
            .find(|option| option.id == *id)
            .map(|option| option.name.clone())
    };
    let text = match value {
        PropertyValue::Text(text)
        | PropertyValue::Url(text)
        | PropertyValue::Email(text)
        | PropertyValue::Phone(text) => text.clone(),
        PropertyValue::Number(number) => number_text(*number),
        PropertyValue::Select(id) | PropertyValue::Status(id) => option_name(id)?,
        PropertyValue::MultiSelect(ids) => ids
            .iter()
            .filter_map(option_name)
            .collect::<Vec<_>>()
            .join(", "),
        PropertyValue::Date(date) => match &date.time {
            Some(time) => format!("{} {time}", date.date),
            None => date.date.clone(),
        },
        PropertyValue::Checkbox(on) => (if *on { "yes" } else { "no" }).to_owned(),
        PropertyValue::Files(files) => count(files.len() as u64, "file"),
        PropertyValue::Checklist(items) => format!(
            "{} of {} done",
            items.iter().filter(|item| item.done).count(),
            items.len()
        ),
        PropertyValue::Place(place) => place.name.clone(),
        PropertyValue::Relation(pages) => {
            let mut titles: Vec<String> = pages
                .iter()
                .filter_map(|page| model.page(*page))
                .filter(|page| page.trashed_at.is_none())
                .map(|page| title_or_untitled(&page.title))
                .collect();
            let more = titles.len().saturating_sub(5);
            titles.truncate(5);
            let mut text = titles.join(", ");
            if more > 0 {
                text.push_str(&format!(" and {more} more"));
            }
            text
        }
        // The people's names, as the member panel shows them ("Former member" for an account that
        // left).
        PropertyValue::Person(accounts) => accounts
            .iter()
            .map(|account| people(*account))
            .collect::<Vec<_>>()
            .join(", "),
        PropertyValue::Formula(result) => formula_text(model, result, people),
        PropertyValue::Rollup(_)
        | PropertyValue::CreatedTime(_)
        | PropertyValue::LastEditedTime(_)
        | PropertyValue::CreatedBy(_)
        | PropertyValue::LastEditedBy(_) => return None,
    };
    let text = clip(&one_line(&text), PROPERTY_VALUE_CHARS);
    (!text.is_empty()).then_some(text)
}

/// A number as `property_text` shows one: whole numbers without a fraction.
fn number_text(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{number:.0}")
    } else {
        number.to_string()
    }
}

/// A formula's result (ADR-0055) as text, by the rules of the kind it looks like: numbers, texts,
/// yes or no, dates, the title of a live page, a person's name, a list joined with `, ` (items with
/// nothing to show left out).
fn formula_text(
    model: &StructureDoc,
    result: &FormulaResult,
    people: &dyn Fn(AccountId) -> String,
) -> String {
    match result {
        FormulaResult::Number(number) => number_text(*number),
        FormulaResult::Text(text) => text.clone(),
        FormulaResult::Boolean(on) => (if *on { "yes" } else { "no" }).to_owned(),
        FormulaResult::Date(date) => match &date.time {
            Some(time) => format!("{} {time}", date.date),
            None => date.date.clone(),
        },
        FormulaResult::Page(page) => model
            .page(*page)
            .filter(|page| page.trashed_at.is_none())
            .map(|page| title_or_untitled(&page.title))
            .unwrap_or_default(),
        FormulaResult::Person(account) => people(*account),
        FormulaResult::List(items) => items
            .iter()
            .map(|item| formula_text(model, item, people))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
    }
}

use crate::event_bridge::{embedding_event_callback, ingestion_event_callback};
use crate::mcp::types::*;
use crate::state::ServerEvent;
use atomic_core::manager::DatabaseManager;
use atomic_core::models::{
    AtomKind, KindFilter, ListAtomsParams, SortField, SortOrder, SourceFilter, TagWithCount,
};
use atomic_core::AtomicCore;
use atomic_core::{apply_atom_edits, AtomEditOperation};
use rmcp::{
    handler::server::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    service::RequestContext,
    tool, tool_handler, tool_router, ErrorData, RoleServer, ServerHandler,
};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Extension type inserted by the `on_request` hook to carry the `?db=` selection.
#[derive(Clone, Debug)]
pub struct DbSelection(pub Option<String>);

/// Per-request override for the [`DatabaseManager`] the MCP tools resolve
/// against, carried in the rmcp request extensions.
///
/// The transport mirrors the data plane's [`RequestDatabaseManager`]
/// extension (`crate::db_extractor`): a caller composing the MCP scope under
/// its own middleware can install a [`RequestDatabaseManager`] on the actix
/// request, and the transport copies the manager into this extension so the
/// tool call resolves against it instead of the manager baked in at
/// [`AtomicMcpServer::new`]. When absent — the standalone server installs no
/// such middleware — [`AtomicMcpServer::resolve_core`] falls back to the
/// baked-in manager, so self-hosted behavior is unchanged.
///
/// It carries the *manager* rather than a pre-resolved [`AtomicCore`] so the
/// per-request database selection ([`DbSelection`] / the `?db=` parameter)
/// stays applied in exactly one place ([`AtomicMcpServer::resolve_core`]),
/// regardless of where the manager came from — the same discipline as the
/// data plane's [`resolve_core`](crate::db_extractor::resolve_core).
///
/// [`RequestDatabaseManager`]: crate::db_extractor::RequestDatabaseManager
#[derive(Clone)]
pub struct RequestManager(pub Arc<DatabaseManager>);

/// Serialize a tool response as pretty-printed JSON text content.
///
/// Every tool returns its structured payload this way; keeping the
/// serialization in one place keeps the error mapping consistent.
fn json_result<T: serde::Serialize>(value: &T) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| ErrorData::internal_error(format!("Serialization error: {}", e), None))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

/// Map a hierarchical [`TagWithCount`] tree into the compact wire shape.
fn tag_tree(nodes: &[TagWithCount]) -> Vec<TagNode> {
    nodes
        .iter()
        .map(|node| TagNode {
            tag_id: node.tag.id.clone(),
            name: node.tag.name.clone(),
            atom_count: node.atom_count,
            children: tag_tree(&node.children),
        })
        .collect()
}

/// Find a tag by ID or case-insensitive name anywhere in the tree.
fn find_tag<'a>(
    nodes: &'a [TagWithCount],
    needle: &str,
    needle_lower: &str,
) -> Option<&'a TagWithCount> {
    for node in nodes {
        if node.tag.id == needle || node.tag.name.to_lowercase() == needle_lower {
            return Some(node);
        }
        if let Some(found) = find_tag(&node.children, needle, needle_lower) {
            return Some(found);
        }
    }
    None
}

/// MCP Server for Atomic knowledge base
#[derive(Clone)]
pub struct AtomicMcpServer {
    manager: Arc<DatabaseManager>,
    event_tx: broadcast::Sender<ServerEvent>,
    tool_router: ToolRouter<Self>,
}

impl AtomicMcpServer {
    pub fn new(manager: Arc<DatabaseManager>, event_tx: broadcast::Sender<ServerEvent>) -> Self {
        Self {
            manager,
            event_tx,
            tool_router: Self::tool_router(),
        }
    }

    /// The manager this request resolves against: the per-request
    /// [`RequestManager`] override when a composing layer installed one,
    /// otherwise the manager baked in at [`Self::new`].
    fn request_manager<'a>(
        &'a self,
        context: &'a RequestContext<RoleServer>,
    ) -> &'a Arc<DatabaseManager> {
        context
            .extensions
            .get::<RequestManager>()
            .map(|m| &m.0)
            .unwrap_or(&self.manager)
    }

    /// Resolve the correct AtomicCore for the request.
    ///
    /// The manager comes from [`Self::request_manager`] — the same fallback
    /// shape as the data plane's
    /// [`request_manager`](crate::db_extractor::request_manager). The database
    /// *within* that manager is then selected from the [`DbSelection`]
    /// extension (the `?db=` parameter), so selection lives in one place
    /// regardless of where the manager came from.
    async fn resolve_core(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<AtomicCore, ErrorData> {
        let manager = self.request_manager(context);
        let db_id = context
            .extensions
            .get::<DbSelection>()
            .and_then(|s| s.0.clone());
        match db_id {
            Some(id) => manager
                .get_core(&id)
                .await
                .map_err(|e| ErrorData::internal_error(format!("Database not found: {}", e), None)),
            None => manager
                .active_core()
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None)),
        }
    }
}

#[tool_router]
impl AtomicMcpServer {
    // ==================== Search & retrieval ====================

    /// Search for atoms using hybrid keyword + semantic search
    #[tool(
        title = "Search memory",
        description = "Search your memory for relevant knowledge. Use this before answering questions that may relate to previously stored information. Returns matching atoms ranked by relevance. Set since_days to constrain to recent atoms (e.g., 7 for last week, 30 for last month) when the question is time-sensitive. Set tag_id to scope the search to one topic, and include_generated=true to also search report findings.",
        annotations(
            title = "Search memory",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn semantic_search(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SemanticSearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let core = self.resolve_core(&context).await?;
        let limit = params.limit.unwrap_or(10).min(50);
        // External MCP clients (Claude Desktop, claude.ai, third-party
        // agents) get a captured-only view by default. The `kind`
        // discriminator landed in phase 1 so background-generated content
        // (report findings, etc.) wouldn't surface in external retrievers
        // unless the caller opts in — which `include_generated` now allows.
        // Threading the filter through `SearchOptions` rather than
        // post-filtering keeps result counts accurate at `limit` and pushes
        // the constraint into the SQL.
        let kinds = if params.include_generated.unwrap_or(false) {
            KindFilter::All
        } else {
            KindFilter::only(AtomKind::Captured)
        };
        let options =
            atomic_core::SearchOptions::new(params.query, atomic_core::SearchMode::Hybrid, limit)
                .with_threshold(0.3)
                .with_since_days(params.since_days)
                .with_scope(params.tag_id.into_iter().collect())
                .with_kinds(kinds);

        let results = core
            .search(options)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let search_results: Vec<SearchResult> = results
            .into_iter()
            .map(|r| SearchResult {
                atom_id: r.atom.atom.id.clone(),
                content_preview: r.atom.atom.content.chars().take(200).collect(),
                similarity_score: r.similarity_score,
                matching_chunk: r.matching_chunk_content,
            })
            .collect();

        json_result(&search_results)
    }

    /// Read a single atom with optional line-based pagination
    #[tool(
        title = "Read atom",
        description = "Read the full content of a specific atom. Use this after semantic_search returns a relevant result and you need the complete text. Supports pagination for large atoms.",
        annotations(title = "Read atom", read_only_hint = true, open_world_hint = false)
    )]
    async fn read_atom(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<ReadAtomParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let core = self.resolve_core(&context).await?;
        let limit = params.limit.unwrap_or(500).min(500) as usize;
        let offset = params.offset.unwrap_or(0).max(0) as usize;

        let atom_with_tags = match core.get_atom(&params.atom_id).await {
            Ok(Some(a)) => a,
            Ok(None) => {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Atom not found: {}",
                    params.atom_id
                ))]));
            }
            Err(e) => return Err(ErrorData::internal_error(e.to_string(), None)),
        };

        let content = &atom_with_tags.atom.content;
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len() as i32;
        let start = offset.min(lines.len());
        let end = (start + limit).min(lines.len());
        let paginated_lines = &lines[start..end];
        let returned_lines = paginated_lines.len() as i32;
        let has_more = end < lines.len();

        let mut paginated_content = paginated_lines.join("\n");

        if has_more {
            paginated_content.push_str(&format!(
                "\n\n(Atom content continues. Use offset {} to read more lines.)",
                end
            ));
        }

        let response = AtomContent {
            atom_id: atom_with_tags.atom.id,
            content: paginated_content,
            total_lines,
            returned_lines,
            offset: offset as i32,
            has_more,
            created_at: atom_with_tags.atom.created_at,
            updated_at: atom_with_tags.atom.updated_at,
        };

        json_result(&response)
    }

    /// Find atoms semantically similar to a given atom
    #[tool(
        title = "Find similar atoms",
        description = "Find atoms semantically related to a given atom. Use this to traverse the knowledge graph outward from a relevant atom — surfacing connected ideas that a keyword search would miss.",
        annotations(
            title = "Find similar atoms",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn find_similar(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<FindSimilarParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let core = self.resolve_core(&context).await?;
        let limit = params.limit.unwrap_or(10).clamp(1, 50);

        match core.get_atom(&params.atom_id).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Atom not found: {}",
                    params.atom_id
                ))]));
            }
            Err(e) => return Err(ErrorData::internal_error(e.to_string(), None)),
        }

        // 0.5 is the same threshold the app uses for related-atom semantic
        // edges — below that, neighbors stop being meaningfully "related".
        let results = core
            .find_similar(&params.atom_id, limit, 0.5)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let items: Vec<SimilarAtomItem> = results
            .into_iter()
            .map(|r| SimilarAtomItem {
                atom_id: r.atom.atom.id.clone(),
                title: r.atom.atom.title.clone(),
                content_preview: r.atom.atom.content.chars().take(200).collect(),
                similarity_score: r.similarity_score,
            })
            .collect();

        json_result(&items)
    }

    // ==================== Browsing & navigation ====================

    /// List the hierarchical tag tree
    #[tool(
        title = "List tags",
        description = "List the tag tree that organizes this knowledge base, with per-tag atom counts. Use it to discover topics, to find tag IDs for scoped searches (semantic_search tag_id), for tagging atoms (update_atom tag_ids), and for wiki lookups (get_wiki). Set min_count to prune rarely-used leaf tags.",
        annotations(title = "List tags", read_only_hint = true, open_world_hint = false)
    )]
    async fn list_tags(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<ListTagsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let core = self.resolve_core(&context).await?;
        let min_count = params.min_count.unwrap_or(0).max(0);

        let tags = core
            .get_all_tags_filtered(min_count)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        json_result(&tag_tree(&tags))
    }

    /// List atoms with pagination and optional tag filtering
    #[tool(
        title = "List atoms",
        description = "Browse atoms as a paginated list, most recently updated first. Set tag_id (from list_tags) to browse one topic, including atoms under descendant tags. Returns summaries only — use read_atom for full content.",
        annotations(title = "List atoms", read_only_hint = true, open_world_hint = false)
    )]
    async fn list_atoms(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<ListAtomsToolParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let core = self.resolve_core(&context).await?;
        let limit = params.limit.unwrap_or(20).clamp(1, 100);
        let offset = params.offset.unwrap_or(0).max(0);
        let kinds = if params.include_generated.unwrap_or(false) {
            KindFilter::All
        } else {
            KindFilter::only(AtomKind::Captured)
        };

        let list_params = ListAtomsParams {
            tag_id: params.tag_id,
            limit,
            offset,
            cursor: None,
            cursor_id: None,
            source_filter: SourceFilter::All,
            source_value: None,
            sort_by: SortField::Updated,
            sort_order: SortOrder::Desc,
        };

        let page = core
            .list_atoms(&list_params, &kinds)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let has_more = offset + (page.atoms.len() as i32) < page.total_count;
        let atoms: Vec<AtomListItem> = page
            .atoms
            .into_iter()
            .map(|a| AtomListItem {
                atom_id: a.id,
                title: a.title,
                snippet: a.snippet,
                tags: a.tags.into_iter().map(|t| t.name).collect(),
                source_url: a.source_url,
                created_at: a.created_at,
                updated_at: a.updated_at,
            })
            .collect();

        let response = AtomListResponse {
            atoms,
            total_count: page.total_count,
            offset,
            has_more,
        };

        json_result(&response)
    }

    /// List the knowledge databases available to this connection
    #[tool(
        title = "List databases",
        description = "List the knowledge databases available to this connection and which one tool calls currently operate on. The database is selected by the connection URL (?db=<id>) or the X-Atomic-Database header — tool calls cannot switch it, but this shows the user's options.",
        annotations(
            title = "List databases",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_databases(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let manager = self.request_manager(&context);
        let (databases, active) = manager
            .list_databases()
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        // The request-level ?db= selection wins over the manager's active
        // database — mirror resolve_core so "selected" reflects what tool
        // calls on this connection actually hit.
        let selected = context
            .extensions
            .get::<DbSelection>()
            .and_then(|s| s.0.clone())
            .unwrap_or(active);

        let items: Vec<DatabaseListItem> = databases
            .into_iter()
            .map(|d| DatabaseListItem {
                selected: d.id == selected,
                database_id: d.id,
                name: d.name,
                is_default: d.is_default,
            })
            .collect();

        json_result(&items)
    }

    // ==================== Synthesized knowledge ====================

    /// List wiki articles
    #[tool(
        title = "List wiki articles",
        description = "List the wiki articles in this knowledge base. Wikis are synthesized overviews of everything stored under a tag, with citations back to source atoms. Use get_wiki to read one.",
        annotations(
            title = "List wiki articles",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_wikis(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let core = self.resolve_core(&context).await?;
        let articles = core
            .get_all_wiki_articles()
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let items: Vec<WikiListItem> = articles
            .into_iter()
            .map(|a| WikiListItem {
                tag_id: a.tag_id,
                tag_name: a.tag_name,
                atom_count: a.atom_count,
                updated_at: a.updated_at,
            })
            .collect();

        json_result(&items)
    }

    /// Read the wiki article for a tag
    #[tool(
        title = "Read wiki article",
        description = "Read the synthesized wiki article for a tag — a curated overview of everything stored under that topic. Prefer this over piecing together many search results when the user asks for a summary of a topic. Inline [N] markers in the content resolve to the returned citations. Accepts a tag ID or exact tag name.",
        annotations(
            title = "Read wiki article",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_wiki(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<GetWikiParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let core = self.resolve_core(&context).await?;

        let tags = core
            .get_all_tags()
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let needle_lower = params.tag.to_lowercase();
        let tag = match find_tag(&tags, &params.tag, &needle_lower) {
            Some(t) => t,
            None => {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Tag not found: {} (use list_tags to see available tags)",
                    params.tag
                ))]));
            }
        };

        let wiki = core
            .get_wiki(&tag.tag.id)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let Some(wiki) = wiki else {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No wiki article exists for tag '{}' yet. Wiki articles are generated in the Atomic app; atoms under the tag can still be browsed with list_atoms or semantic_search.",
                tag.tag.name
            ))]));
        };

        let response = WikiResponse {
            tag_id: wiki.article.tag_id,
            tag_name: tag.tag.name.clone(),
            content: wiki.article.content,
            atom_count: wiki.article.atom_count,
            updated_at: wiki.article.updated_at,
            citations: wiki
                .citations
                .into_iter()
                .map(|c| WikiCitationItem {
                    citation_index: c.citation_index,
                    atom_id: c.atom_id,
                    excerpt: c.excerpt,
                    source_url: c.source_url,
                })
                .collect(),
        };

        json_result(&response)
    }

    /// List report definitions
    #[tool(
        title = "List reports",
        description = "List the automated research reports configured in this knowledge base. Reports run on a schedule and write their findings back as atoms. Use get_report_findings to read a report's output.",
        annotations(title = "List reports", read_only_hint = true, open_world_hint = false)
    )]
    async fn list_reports(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let core = self.resolve_core(&context).await?;
        let reports = core
            .list_reports()
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let items: Vec<ReportListItem> = reports
            .into_iter()
            .map(|r| ReportListItem {
                report_id: r.id,
                name: r.name,
                description: r.description,
                schedule: r.schedule,
                schedule_tz: r.schedule_tz,
                enabled: r.enabled,
                last_run_at: r.last_run_at,
                last_error: r.last_error,
            })
            .collect();

        json_result(&items)
    }

    /// Read the most recent findings of a report
    #[tool(
        title = "Read report findings",
        description = "Read the most recent findings a report produced. Each finding is a research write-up whose inline [N] markers resolve to the returned citations pointing at source atoms.",
        annotations(
            title = "Read report findings",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_report_findings(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<GetReportFindingsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let core = self.resolve_core(&context).await?;
        let limit = params.limit.unwrap_or(3).clamp(1, 10);

        match core.get_report(&params.report_id).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Report not found: {} (use list_reports to see available reports)",
                    params.report_id
                ))]));
            }
            Err(e) => return Err(ErrorData::internal_error(e.to_string(), None)),
        }

        let findings = core
            .list_findings_for_report(&params.report_id, limit)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let mut items = Vec::with_capacity(findings.len());
        for (finding, atom) in findings {
            let citations = core
                .list_citations_for_finding(&finding.finding_atom_id)
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
            items.push(ReportFindingItem {
                atom_id: atom.atom.id,
                title: atom.atom.title,
                report_name: finding.report_name_snapshot,
                created_at: finding.created_at,
                content: atom.atom.content,
                citations: citations
                    .into_iter()
                    .map(|c| FindingCitationItem {
                        position: c.position,
                        cited_atom_id: c.cited_atom_id,
                        excerpt: c.excerpt,
                    })
                    .collect(),
            });
        }

        json_result(&items)
    }

    // ==================== Writing ====================

    /// Create a new atom with markdown content
    #[tool(
        title = "Create atom",
        description = "Remember something new. Create an atom when you learn information worth retaining across conversations — user preferences, decisions, project context, or important facts. Write concise, self-contained markdown.",
        annotations(
            title = "Create atom",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn create_atom(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<CreateAtomParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let core = self.resolve_core(&context).await?;
        let request = atomic_core::CreateAtomRequest {
            content: params.content.clone(),
            source_url: params.source_url,
            published_at: None,
            tag_ids: vec![],
            skip_if_source_exists: false,
        };

        let on_event = embedding_event_callback(self.event_tx.clone());

        let result = core
            .create_atom(request, on_event)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
            .ok_or_else(|| {
                ErrorData::internal_error("Atom creation returned None".to_string(), None)
            })?;

        // Broadcast atom creation event
        let _ = self.event_tx.send(ServerEvent::AtomCreated {
            atom: result.clone(),
        });

        let response = AtomResponse {
            atom_id: result.atom.id.clone(),
            content_preview: result.atom.content.chars().take(200).collect(),
            tags: result.tags.iter().map(|t| t.name.clone()).collect(),
            embedding_status: result.atom.embedding_status.clone(),
        };

        json_result(&response)
    }

    /// Ingest a URL into Atomic
    #[tool(
        title = "Save web page",
        description = "Fetch a URL, extract its article content, and save it as an atom. Use this when the user asks to remember, save, or ingest a web page. If the URL already exists as an atom source_url, returns the existing atom_id with already_exists=true instead of creating a duplicate.",
        annotations(
            title = "Save web page",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn ingest_url(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<IngestUrlParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let core = self.resolve_core(&context).await?;
        let url = params.url;

        if let Some(existing) = core
            .get_atom_by_source_url(&url)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
        {
            let response = IngestUrlResponse {
                atom_id: existing.atom.id,
                url: existing.atom.source_url.unwrap_or(url),
                title: existing.atom.title,
                content_length: existing.atom.content.len(),
                already_exists: true,
            };

            return json_result(&response);
        }

        let request = atomic_core::IngestionRequest {
            url,
            tag_ids: vec![],
            title_hint: None,
            published_at: None,
        };

        let on_ingest = ingestion_event_callback(self.event_tx.clone());
        let on_embed = embedding_event_callback(self.event_tx.clone());

        let result = core
            .ingest_url(request, on_ingest, on_embed)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let response = IngestUrlResponse {
            atom_id: result.atom_id,
            url: result.url,
            title: result.title,
            content_length: result.content_length,
            already_exists: false,
        };

        json_result(&response)
    }

    /// Update an existing atom with optional full-content, metadata, or tag changes
    #[tool(
        title = "Update atom",
        description = "Compatibility full/partial atom update. Omitted fields are preserved. Content is optional so callers can update metadata or tag_ids (from list_tags) without rewriting markdown. Prefer edit_atom for content changes unless replacing the whole atom intentionally.",
        annotations(
            title = "Update atom",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn update_atom(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<UpdateAtomParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let core = self.resolve_core(&context).await?;

        let existing = match core.get_atom(&params.atom_id).await {
            Ok(Some(atom)) => atom,
            Ok(None) => {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Atom not found: {}",
                    params.atom_id
                ))]));
            }
            Err(e) => return Err(ErrorData::internal_error(e.to_string(), None)),
        };

        if params.content.is_none()
            && params.source_url.is_none()
            && params.published_at.is_none()
            && params.tag_ids.is_none()
        {
            return Ok(CallToolResult::success(vec![Content::text(
                "No update fields provided".to_string(),
            )]));
        }

        let content = match params.content {
            Some(content) => {
                if content.trim().is_empty() {
                    return Ok(CallToolResult::success(vec![Content::text(
                        "Cannot update an atom to empty content".to_string(),
                    )]));
                }
                content
            }
            None => existing.atom.content.clone(),
        };

        let source_url = params.source_url.or(existing.atom.source_url.clone());
        let published_at = params.published_at.or(existing.atom.published_at.clone());

        let request = atomic_core::UpdateAtomRequest {
            content,
            source_url,
            published_at,
            tag_ids: params.tag_ids,
        };

        let on_event = embedding_event_callback(self.event_tx.clone());

        let result = core
            .update_atom_if_unchanged(
                &params.atom_id,
                request,
                &existing.atom.updated_at,
                on_event,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let response = AtomResponse {
            atom_id: result.atom.id.clone(),
            content_preview: result.atom.content.chars().take(200).collect(),
            tags: result.tags.iter().map(|t| t.name.clone()).collect(),
            embedding_status: result.atom.embedding_status.clone(),
        };

        json_result(&response)
    }

    /// Apply targeted edits to an atom's markdown content
    #[tool(
        title = "Edit atom",
        description = "Apply safe edits to an existing atom. Supports replace, insert_after, append, and replace_all. replace and insert_after require exact text that appears exactly once. The whole operation fails without saving if any edit is invalid. Prefer this over update_atom for markdown changes.",
        annotations(
            title = "Edit atom",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn edit_atom(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<EditAtomParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let core = self.resolve_core(&context).await?;

        let existing = match core.get_atom(&params.atom_id).await {
            Ok(Some(atom)) => atom,
            Ok(None) => {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Atom not found: {}",
                    params.atom_id
                ))]));
            }
            Err(e) => return Err(ErrorData::internal_error(e.to_string(), None)),
        };

        let edits = params
            .edits
            .iter()
            .map(AtomEditOperation::from)
            .collect::<Vec<_>>();
        let content = match apply_atom_edits(&existing.atom.content, &edits) {
            Ok(content) => content,
            Err(error) => return Ok(CallToolResult::success(vec![Content::text(error)])),
        };

        if content == existing.atom.content {
            return Ok(CallToolResult::success(vec![Content::text(
                "Edits did not change the atom content".to_string(),
            )]));
        }
        if content.trim().is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "Cannot update an atom to empty content".to_string(),
            )]));
        }

        let request = atomic_core::UpdateAtomRequest {
            content,
            source_url: existing.atom.source_url.clone(),
            published_at: existing.atom.published_at.clone(),
            tag_ids: None,
        };

        let on_event = embedding_event_callback(self.event_tx.clone());

        let result = core
            .update_atom_if_unchanged(
                &params.atom_id,
                request,
                &existing.atom.updated_at,
                on_event,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let response = AtomResponse {
            atom_id: result.atom.id.clone(),
            content_preview: result.atom.content.chars().take(200).collect(),
            tags: result.tags.iter().map(|t| t.name.clone()).collect(),
            embedding_status: result.atom.embedding_status.clone(),
        };

        json_result(&response)
    }

    // ==================== ChatGPT-compatible aliases ====================
    //
    // ChatGPT's deep-research mode only invokes read-only tools named
    // exactly `search` and `fetch` with these schemas
    // (https://developers.openai.com/api/docs/mcp). They are thin aliases
    // of semantic_search and read_atom in the wire shape that surface
    // expects; other clients can use either pair.

    /// ChatGPT-compatible search alias
    #[tool(
        title = "Search",
        description = "Search the knowledge base and return matching documents. Alias of semantic_search in the search/fetch document shape.",
        annotations(title = "Search", read_only_hint = true, open_world_hint = false)
    )]
    async fn search(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<DeepSearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let core = self.resolve_core(&context).await?;
        let options =
            atomic_core::SearchOptions::new(params.query, atomic_core::SearchMode::Hybrid, 10)
                .with_threshold(0.3)
                .with_kinds(KindFilter::only(AtomKind::Captured));

        let results = core
            .search(options)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let response = DeepSearchResponse {
            results: results
                .into_iter()
                .map(|r| DeepSearchResult {
                    id: r.atom.atom.id.clone(),
                    title: r.atom.atom.title.clone(),
                    text: r.matching_chunk_content,
                    url: r.atom.atom.source_url.clone(),
                })
                .collect(),
        };

        json_result(&response)
    }

    /// ChatGPT-compatible fetch alias
    #[tool(
        title = "Fetch",
        description = "Fetch the full content of a document returned by search. Alias of read_atom in the search/fetch document shape.",
        annotations(title = "Fetch", read_only_hint = true, open_world_hint = false)
    )]
    async fn fetch(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<FetchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let core = self.resolve_core(&context).await?;

        let atom_with_tags = match core.get_atom(&params.id).await {
            Ok(Some(a)) => a,
            Ok(None) => {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Atom not found: {}",
                    params.id
                ))]));
            }
            Err(e) => return Err(ErrorData::internal_error(e.to_string(), None)),
        };

        let response = FetchResponse {
            id: atom_with_tags.atom.id,
            title: atom_with_tags.atom.title,
            text: atom_with_tags.atom.content,
            url: atom_with_tags.atom.source_url,
            metadata: FetchMetadata {
                created_at: atom_with_tags.atom.created_at,
                updated_at: atom_with_tags.atom.updated_at,
                tags: atom_with_tags.tags.into_iter().map(|t| t.name).collect(),
            },
        };

        json_result(&response)
    }
}

#[tool_handler]
impl ServerHandler for AtomicMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Atomic is your long-term memory. Search before answering from recall. \
                 Remember what's worth retaining. Update what's gone stale. Beyond raw \
                 notes: list_tags shows how knowledge is organized, get_wiki returns a \
                 synthesized overview of a topic with citations, and get_report_findings \
                 returns the output of scheduled research reports."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

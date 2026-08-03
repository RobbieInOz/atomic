//! Chat's tool registry.
//!
//! Every capability a chat turn has, expressed as an [`AgentTool`]. The four
//! core tools are always present; the page-context and canvas tools are
//! registered only when the frontend sent the matching context, which is how
//! "conditional tools" works now that there is no dispatch match to branch
//! inside.
//!
//! Tools that surface knowledge register it with the run's citation ledger
//! and put the number they get back into their own output — that number is
//! the `[N]` the model must write for the source to reach the user.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use chrono::Utc;
use regex::Regex;
use serde_json::json;
use uuid::Uuid;

use crate::agent::{CanvasContext, ChatEvent, ChatEventCallback, PageContext};
use crate::agent_runtime::{AgentTool, CitationSource, ToolContext, ToolRegistry, ToolResult};
use crate::atom_edit::{apply_atom_edits, AtomEditOperation};
use crate::embedding::EmbeddingEvent;
use crate::models::{AtomKind, AtomWithTags, SemanticSearchResult, TagWithCount};
use crate::providers::types::ToolDefinition;
use crate::providers::ProviderConfig;
use crate::search::{ScopeFilter, SearchMode, SearchOptions};
use crate::storage::StorageBackend;
use crate::CanvasCache;

/// How much of a source is stored with a citation. Long enough for the UI's
/// popover to be useful, short enough not to duplicate the atom.
const EXCERPT_LEN: usize = 200;

/// Default line limit for a single content-reading call. Chosen to keep
/// context usage bounded on long content (imports, pasted articles, wiki
/// articles) while being large enough that most of them fit in one call.
const READ_DEFAULT_LIMIT: usize = 500;
const READ_MAX_LIMIT: usize = 2000;

/// Tags rendered per level by `list_tags` before the rest are summarized as
/// a count. Deep trees are common; a turn's worth of context is not.
const MAX_TAGS_PER_LEVEL: usize = 15;

/// Levels of tag tree one `list_tags` call renders. Two is enough to see
/// what a branch is about; `parent_id` walks further.
const TAG_TREE_DEPTH: usize = 2;

/// Reports rendered by `list_reports`. A knowledge base with more than this
/// many scheduled reports is unusual, and the tail is the least recently
/// useful part of the list.
const MAX_REPORTS: usize = 25;

/// Everything a chat turn's tools need. Assembled by the chat entrypoints;
/// `page_context` and `canvas` decide which optional tools get registered.
pub(crate) struct ChatToolset {
    pub storage: StorageBackend,
    pub conversation_id: String,
    pub scope: ScopeFilter,
    pub settings: Option<HashMap<String, String>>,
    pub inline_pipeline: bool,
    pub canvas_cache: Option<CanvasCache>,
    pub page_context: Option<PageContext>,
    pub canvas_context: Option<CanvasContext>,
    pub on_event: ChatEventCallback,
}

impl ChatToolset {
    pub fn into_registry(self) -> ToolRegistry {
        let mutations = MutationDeps {
            storage: self.storage.clone(),
            settings: self.settings.clone(),
            inline_pipeline: self.inline_pipeline,
            canvas_cache: self.canvas_cache,
            conversation_id: self.conversation_id.clone(),
            on_embedding_event: embedding_event_bridge(
                Arc::clone(&self.on_event),
                self.conversation_id.clone(),
            ),
            on_event: Arc::clone(&self.on_event),
        };
        let guard = ScopeGuard {
            storage: self.storage.clone(),
            scope: self.scope.clone(),
        };

        let mut registry = ToolRegistry::new()
            .with(SearchAtoms {
                storage: self.storage.clone(),
                settings: self.settings,
                scope: self.scope,
            })
            .with(GetAtom {
                guard: guard.clone(),
            })
            .with(ListTags {
                guard: guard.clone(),
            })
            .with(GetWiki {
                guard: guard.clone(),
            })
            .with(ListReports {
                guard: guard.clone(),
            })
            .with(GetFinding {
                guard: guard.clone(),
            })
            .with(CreateAtom {
                deps: mutations.clone(),
            })
            .with(EditAtom { deps: mutations });

        if let Some(page_context) = self.page_context {
            registry.register(CurrentPageContext {
                guard,
                page_context,
            });
        }
        if self.canvas_context.is_some() {
            registry
                .register(CanvasNavigation {
                    action: CanvasAction::ZoomToCluster,
                    conversation_id: self.conversation_id.clone(),
                    on_event: Arc::clone(&self.on_event),
                })
                .register(CanvasNavigation {
                    action: CanvasAction::FocusAtom,
                    conversation_id: self.conversation_id,
                    on_event: self.on_event,
                });
        }

        registry
    }
}

/// Bridge the agent's atom mutations onto the conversation's event stream so
/// the UI sees embedding/tagging progress for atoms the agent touched.
fn embedding_event_bridge(
    on_event: ChatEventCallback,
    conversation_id: String,
) -> Arc<dyn Fn(EmbeddingEvent) + Send + Sync + 'static> {
    Arc::new(move |event| {
        on_event(ChatEvent::AtomPipelineEvent {
            conversation_id: conversation_id.clone(),
            event,
        });
    })
}

fn excerpt(text: &str) -> String {
    text.chars().take(EXCERPT_LEN).collect()
}

// ==================== Scope enforcement ====================

/// The conversation's scope, applied to the tools that read *by id*.
///
/// `search_atoms` carries its scope into the query, so nothing outside it can
/// come back. Every other reading tool is handed an id — an atom's, a tag's,
/// a finding's — and would otherwise fetch it unconditionally, which turns
/// the system prompt's promise that excluded tags "will never appear" into a
/// suggestion. Each of them asks this guard first and refuses in plain
/// language when the answer is no, so the model learns the boundary instead
/// of retrying against it.
#[derive(Clone)]
struct ScopeGuard {
    storage: StorageBackend,
    scope: ScopeFilter,
}

impl ScopeGuard {
    /// Which of `atom_ids` the scope admits. An empty scope admits
    /// everything without touching storage.
    async fn admitted_atoms(&self, atom_ids: &[String]) -> Result<HashSet<String>, String> {
        if self.scope.is_empty() || atom_ids.is_empty() {
            return Ok(atom_ids.iter().cloned().collect());
        }
        self.storage
            .atoms_passing_scope_sync(atom_ids, &self.scope)
            .await
            .map_err(|e| e.to_string())
    }

    async fn admits_atom(&self, atom_id: &str) -> Result<bool, String> {
        let ids = vec![atom_id.to_string()];
        Ok(self.admitted_atoms(&ids).await?.contains(atom_id))
    }

    /// Resolve the scope against a tag tree, once per call that needs it.
    fn tags(&self, tree: &[TagWithCount]) -> TagScope {
        TagScope::resolve(tree, &self.scope)
    }
}

/// The scope as it applies to tags, resolved against one tag tree.
///
/// Two questions, because they have different answers. *Readable* is whether
/// the tag's content — its wiki article — may be handed to the model:
/// evaluated with [`ScopeFilter::admitted`], the same predicate the atom
/// paths use, over the scope roots covering each tag. *Listable* is whether
/// `list_tags` may name it: excluded branches are hidden outright, and an
/// include scope narrows the listing to those branches plus the ancestors
/// they hang from, so the model still sees where they sit.
enum TagScope {
    /// No scope at all — every tag is both readable and listable.
    Open,
    Filtered {
        readable: HashSet<String>,
        listable: HashSet<String>,
    },
}

impl TagScope {
    fn resolve(tree: &[TagWithCount], scope: &ScopeFilter) -> Self {
        if scope.is_empty() {
            return TagScope::Open;
        }

        let TagCoverage {
            covering,
            include_ancestors,
            ..
        } = TagCoverage::of(tree, scope);

        let ids: Vec<String> = covering.keys().cloned().collect();
        let readable = scope.admitted(ids.iter().map(String::as_str), &covering);

        let covered_by = |id: &str, list: &[String]| {
            covering
                .get(id)
                .is_some_and(|roots| list.iter().any(|tag| roots.contains(tag)))
        };
        let listable = ids
            .iter()
            .filter(|id| !covered_by(id, &scope.exclude))
            .filter(|id| {
                scope.include.is_empty()
                    || covered_by(id, &scope.include)
                    || include_ancestors.contains(*id)
            })
            .cloned()
            .collect();

        TagScope::Filtered { readable, listable }
    }

    fn readable(&self, tag_id: &str) -> bool {
        match self {
            TagScope::Open => true,
            TagScope::Filtered { readable, .. } => readable.contains(tag_id),
        }
    }

    fn listable(&self, tag_id: &str) -> bool {
        match self {
            TagScope::Open => true,
            TagScope::Filtered { listable, .. } => listable.contains(tag_id),
        }
    }

    /// The tree with every non-listable tag — and the branch beneath it —
    /// removed. `children_total` is recounted so the "N sub-tags" pointer
    /// doesn't advertise branches this conversation can't open.
    fn prune(&self, nodes: &[TagWithCount]) -> Vec<TagWithCount> {
        if matches!(self, TagScope::Open) {
            return nodes.to_vec();
        }
        nodes
            .iter()
            .filter(|node| self.listable(&node.tag.id))
            .map(|node| {
                let children = self.prune(&node.children);
                TagWithCount {
                    tag: node.tag.clone(),
                    atom_count: node.atom_count,
                    children_total: children.len() as i32,
                    children,
                }
            })
            .collect()
    }
}

/// One walk of the tag tree, recording per tag the scope roots covering it —
/// itself or any ancestor — plus the ancestors of every include root, which a
/// listing keeps so admitted branches still have a tree to hang from.
struct TagCoverage {
    roots: HashSet<String>,
    include_roots: HashSet<String>,
    covering: HashMap<String, HashSet<String>>,
    include_ancestors: HashSet<String>,
}

impl TagCoverage {
    fn of(tree: &[TagWithCount], scope: &ScopeFilter) -> Self {
        let mut coverage = TagCoverage {
            roots: scope.roots().into_iter().collect(),
            include_roots: scope.include.iter().cloned().collect(),
            covering: HashMap::new(),
            include_ancestors: HashSet::new(),
        };
        coverage.walk(tree, &HashSet::new(), &[]);
        coverage
    }

    fn walk(&mut self, nodes: &[TagWithCount], inherited: &HashSet<String>, path: &[String]) {
        for node in nodes {
            let id = &node.tag.id;
            let mut mine = inherited.clone();
            if self.roots.contains(id) {
                mine.insert(id.clone());
            }
            if self.include_roots.contains(id) {
                self.include_ancestors.extend(path.iter().cloned());
            }
            self.covering.insert(id.clone(), mine.clone());

            let mut child_path = path.to_vec();
            child_path.push(id.clone());
            self.walk(&node.children, &mine, &child_path);
        }
    }
}

/// What a tool says when the scope won't let it read something. Names the
/// boundary without describing what is behind it, and tells the model the
/// refusal is final so it stops re-asking.
fn out_of_scope(subject: &str) -> String {
    format!(
        "{} is outside this conversation's scope, so it can't be read here. \
         The scope is fixed for this conversation — work from what is in scope, \
         or tell the user what you would need them to add.",
        subject
    )
}

/// Lead a tool result with the number the model must cite, or with nothing
/// when the run refuses to make this evidence citable.
fn citation_marker(number: Option<i32>) -> String {
    number.map(|n| format!("[{}] ", n)).unwrap_or_default()
}

/// Wiki articles and report findings carry `[N]` markers of their own,
/// numbered against *their* source lists. Left in place, the model copies
/// one into its answer and this turn's ledger resolves it to whatever
/// happens to hold that number — a citation pointing at the wrong source.
/// The prose reads fine without them.
fn strip_citation_markers(text: &str) -> String {
    static MARKER: OnceLock<Option<Regex>> = OnceLock::new();
    match MARKER
        .get_or_init(|| Regex::new(r" ?\[\d+\]").ok())
        .as_ref()
    {
        Some(marker) => marker.replace_all(text, "").into_owned(),
        None => text.to_string(),
    }
}

/// The shared `offset`/`limit` pair for tools that return a window of lines,
/// merged into the caller's own properties.
fn paged_schema(properties: serde_json::Value, required: serde_json::Value) -> serde_json::Value {
    let mut properties = properties;
    if let Some(map) = properties.as_object_mut() {
        map.insert(
            "offset".to_string(),
            json!({
                "type": "integer",
                "description": "Line number to start reading from (0-indexed). Use the value from a prior response's header to continue reading truncated content.",
                "minimum": 0,
                "default": 0
            }),
        );
        map.insert(
            "limit".to_string(),
            json!({
                "type": "integer",
                "description": format!("Maximum number of lines to return. Defaults to {}; prefer the default unless you know you need more.", READ_DEFAULT_LIMIT),
                "minimum": 1,
                "maximum": READ_MAX_LIMIT,
                "default": READ_DEFAULT_LIMIT
            }),
        );
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required
    })
}

fn paging_args(args: &serde_json::Value) -> (usize, usize) {
    let offset = args
        .get("offset")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(0);
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| (v as usize).clamp(1, READ_MAX_LIMIT))
        .unwrap_or(READ_DEFAULT_LIMIT);
    (offset, limit)
}

/// One window of content, split into what the model reads and what a
/// citation quotes.
struct LineWindow {
    /// What the model is shown: the lines, led by a header explaining how to
    /// read the rest whenever the content didn't fit in one call.
    rendered: String,
    /// The lines alone. Excerpts come from here, so a citation stored
    /// alongside a paginated read quotes the source rather than the
    /// bookkeeping line above it.
    body: String,
}

/// A window of `limit` lines starting at `offset`, headed by how to read the
/// rest through `tool`. Short content (the common case) comes back clean,
/// with no metadata noise.
fn line_window(content: &str, offset: usize, limit: usize, tool: &str) -> LineWindow {
    let plain = |text: String| LineWindow {
        body: text.clone(),
        rendered: text,
    };

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    // Empty content — return empty before the range math, otherwise a
    // nonzero offset would produce lines[offset..0] and panic.
    if total == 0 {
        return plain(String::new());
    }

    if offset >= total {
        // Nothing was read, so there is nothing to quote: the notice is
        // rendered but never becomes an excerpt.
        return LineWindow {
            rendered: format!(
                "[offset={} is past end of content ({} total lines)]",
                offset, total
            ),
            body: String::new(),
        };
    }

    let end = (offset + limit).min(total);
    let body = lines[offset..end].join("\n");

    if offset == 0 && end == total {
        return plain(body);
    }

    let header = if end < total {
        format!(
            "[lines {}-{} of {}. {} more lines. Call {} again with offset={} to continue.]\n",
            offset + 1,
            end,
            total,
            total - end,
            tool,
            end,
        )
    } else {
        format!(
            "[lines {}-{} of {} (end of content).]\n",
            offset + 1,
            end,
            total
        )
    };

    LineWindow {
        rendered: format!("{}{}", header, body),
        body,
    }
}

/// Find a tag by id, then by exact name, then case-insensitively — the same
/// resolution order the MCP surface uses, and no fuzzier than that.
fn resolve_tag<'a>(nodes: &'a [TagWithCount], needle: &str) -> Option<&'a TagWithCount> {
    fn walk<'a>(
        nodes: &'a [TagWithCount],
        matches: &impl Fn(&TagWithCount) -> bool,
    ) -> Option<&'a TagWithCount> {
        for node in nodes {
            if matches(node) {
                return Some(node);
            }
            if let Some(found) = walk(&node.children, matches) {
                return Some(found);
            }
        }
        None
    }

    let lowered = needle.to_lowercase();
    walk(nodes, &|node: &TagWithCount| {
        node.tag.id == needle || node.tag.name == needle
    })
    .or_else(|| {
        walk(nodes, &|node: &TagWithCount| {
            node.tag.name.to_lowercase() == lowered
        })
    })
}

// ==================== search_atoms ====================

struct SearchAtoms {
    storage: StorageBackend,
    settings: Option<HashMap<String, String>>,
    scope: ScopeFilter,
}

#[async_trait]
impl AgentTool for SearchAtoms {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "search_atoms",
            "Search for relevant atoms using hybrid keyword and semantic search. Use this to find information related to a specific topic or question. Set since_days when the user is asking about recent notes (e.g., 7 for last week, 30 for last month).",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query to find relevant atoms"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 5)",
                        "default": 5
                    },
                    "since_days": {
                        "type": "integer",
                        "description": "Optional recency filter: only return atoms created within the last N days. Use when the user's question is time-sensitive (e.g., 7 for last week, 30 for last month).",
                        "minimum": 1
                    }
                },
                "required": ["query"]
            }),
        )
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext<'_>) -> ToolResult {
        let query = args["query"].as_str().unwrap_or("");
        let limit = args["limit"].as_i64().unwrap_or(5) as i32;
        let since_days = args
            .get("since_days")
            .and_then(|v| v.as_f64())
            .map(|v| v as i32)
            .filter(|days| *days > 0);

        let results = match search_atoms(
            &self.storage,
            query,
            limit,
            since_days,
            &self.scope,
            self.settings.clone(),
        )
        .await
        {
            Ok(results) => results,
            Err(e) => return ToolResult::failed(e),
        };

        let count = results.len() as i32;
        let rendered: Vec<String> = results
            .iter()
            .map(|result| {
                let number = ctx.citations.register(
                    CitationSource::Atom,
                    &result.atom.atom.id,
                    Some(result.matching_chunk_index),
                    excerpt(&result.matching_chunk_content),
                );
                format!(
                    "{}(atom_id: {}, similarity: {:.2})\n{}",
                    citation_marker(number),
                    result.atom.atom.id,
                    result.similarity_score,
                    result.matching_chunk_content
                )
            })
            .collect();

        ToolResult::ok(rendered.join("\n\n"), count)
    }
}

async fn search_atoms(
    storage: &StorageBackend,
    query: &str,
    limit: i32,
    since_days: Option<i32>,
    scope: &ScopeFilter,
    external_settings: Option<HashMap<String, String>>,
) -> Result<Vec<SemanticSearchResult>, String> {
    // Try SQLite path first (uses full search module with settings resolution)
    if let Some(sqlite) = storage.as_sqlite() {
        let options = SearchOptions::new(query, SearchMode::Hybrid, limit)
            .with_threshold(0.3)
            .with_scope_filter(scope.clone())
            .with_since_days(since_days);
        return crate::search::search_atoms_with_settings(&sqlite.db, options, external_settings)
            .await;
    }

    // Postgres path: use storage dispatch methods. Provider config is
    // deployment-wide, so the fallback reads the global settings tier.
    let settings = match external_settings {
        Some(s) => s,
        None => storage
            .get_global_settings_sync()
            .await
            .map_err(|e| e.to_string())?,
    };
    let config = ProviderConfig::from_settings(&settings);

    // Generate query embedding
    let provider = crate::providers::get_embedding_provider(&config).map_err(|e| e.to_string())?;
    let embed_config = config.embedding_config();
    let embeddings = provider
        .embed_batch(&[query.to_string()], &embed_config)
        .await
        .map_err(|e| e.to_string())?;

    let cutoff = since_days.map(crate::search::since_days_cutoff);
    let cutoff_ref = cutoff.as_deref();

    // Chat is an in-app conversational surface over the user's own KB;
    // finding atoms participate as first-class context just like captured
    // ones (mirrors the SQLite path that goes through SearchOptions with
    // its default KindFilter::All).
    let kinds = crate::models::KindFilter::All;
    let keyword = storage
        .keyword_search_sync(query, limit * 2, scope, cutoff_ref, &kinds)
        .await
        .map_err(|e| e.to_string())?;
    let semantic = if !embeddings.is_empty() && !embeddings[0].is_empty() {
        storage
            .vector_search_sync(&embeddings[0], limit * 2, 0.3, scope, cutoff_ref, &kinds)
            .await
            .map_err(|e| e.to_string())?
    } else {
        vec![]
    };

    Ok(crate::search::merge_search_results_rrf(
        semantic, keyword, limit,
    ))
}

// ==================== get_atom ====================

struct GetAtom {
    guard: ScopeGuard,
}

#[async_trait]
impl AgentTool for GetAtom {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "get_atom",
            "Get the content of a specific atom by its ID. Returns up to `limit` lines starting at `offset` (defaults: offset=0, limit=500). If the atom has more content, the response includes a line-count header indicating how to continue reading via a follow-up call with a higher offset.",
            paged_schema(
                json!({
                    "atom_id": {
                        "type": "string",
                        "description": "The ID of the atom to retrieve"
                    }
                }),
                json!(["atom_id"]),
            ),
        )
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext<'_>) -> ToolResult {
        let atom_id = args["atom_id"].as_str().unwrap_or("");
        let (offset, limit) = paging_args(args);

        // Scope first: an id the model got from anywhere — a wiki article, a
        // report listing, its own memory of an earlier conversation — must
        // not become a way around the conversation's scope.
        match self.guard.admits_atom(atom_id).await {
            Ok(true) => {}
            Ok(false) => {
                return ToolResult::ok(out_of_scope(&format!("Atom {}", atom_id)), 0);
            }
            Err(e) => return ToolResult::failed(e),
        }

        match read_atom_lines(&self.guard.storage, atom_id, offset, limit).await {
            Ok(Some(window)) => {
                let number = ctx.citations.register(
                    CitationSource::Atom,
                    atom_id,
                    None,
                    excerpt(&window.body),
                );
                ToolResult::ok(
                    format!(
                        "{}(atom_id: {})\n{}",
                        citation_marker(number),
                        atom_id,
                        window.rendered
                    ),
                    1,
                )
            }
            Ok(None) => ToolResult::ok("Atom not found", 0),
            Err(e) => ToolResult::failed(e),
        }
    }
}

async fn read_atom_lines(
    storage: &StorageBackend,
    atom_id: &str,
    offset: usize,
    limit: usize,
) -> Result<Option<LineWindow>, String> {
    let Some(content) = storage
        .get_atom_content_impl(atom_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };

    Ok(Some(line_window(&content, offset, limit, "get_atom")))
}

// ==================== list_tags ====================

/// The shape of the knowledge base itself: which topics exist and how much
/// sits under each. Purely navigational — a tag is not a source, so nothing
/// here is citable.
struct ListTags {
    guard: ScopeGuard,
}

#[async_trait]
impl AgentTool for ListTags {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "list_tags",
            "List the tags that organize this knowledge base, with the number of atoms under each (counts include sub-tags). Use it to see how the user's knowledge is structured, to choose a tag for get_wiki, or to find a tag id to scope your reading. Returns the top of the tree and one level below it; pass parent_id to expand a branch further.",
            json!({
                "type": "object",
                "properties": {
                    "parent_id": {
                        "type": "string",
                        "description": "Tag id (or exact name) to expand. Omit for the top of the tree."
                    }
                }
            }),
        )
    }

    async fn execute(&self, args: &serde_json::Value, _ctx: &ToolContext<'_>) -> ToolResult {
        let tree = match self.guard.storage.get_all_tags_impl().await {
            Ok(tree) => tree,
            Err(e) => return ToolResult::failed(e),
        };
        // The listing is the map the model navigates by. Handing it branches
        // the scope has closed invites a get_wiki or get_atom call that can
        // only be refused, and names topics the user asked to keep out.
        let scope = self.guard.tags(&tree);
        let visible = scope.prune(&tree);

        let parent = args
            .get("parent_id")
            .and_then(|v| v.as_str())
            .filter(|value| !value.is_empty());
        let (heading, roots) = match parent {
            // Resolved against the full tree so an in-scope name and an
            // out-of-scope one get different answers.
            Some(needle) => match resolve_tag(&tree, needle) {
                Some(tag) if !scope.listable(&tag.tag.id) => {
                    return ToolResult::ok(out_of_scope(&format!("'{}'", tag.tag.name)), 0)
                }
                Some(tag) => match resolve_tag(&visible, &tag.tag.id) {
                    Some(pruned) => (
                        format!("Sub-tags of {} (counts include sub-tags):", pruned.tag.name),
                        std::slice::from_ref(pruned),
                    ),
                    None => return ToolResult::ok(out_of_scope(&format!("'{}'", tag.tag.name)), 0),
                },
                None => {
                    return ToolResult::ok(
                        format!("No tag matches '{}'. Call list_tags with no arguments to see the top of the tree.", needle),
                        0,
                    )
                }
            },
            None => (
                "Tags (counts include sub-tags):".to_string(),
                visible.as_slice(),
            ),
        };

        if roots.is_empty() {
            return ToolResult::ok(
                if tree.is_empty() {
                    "This knowledge base has no tags yet."
                } else {
                    "No tags are within this conversation's scope."
                },
                0,
            );
        }

        let mut out = heading;
        let mut rendered = 0;
        render_tags(roots, 0, &mut out, &mut rendered);
        ToolResult::ok(out, rendered)
    }
}

/// Render two levels of the tag tree: each node on its own line, its
/// children indented beneath it, and a pointer to `list_tags` for whatever
/// the caps left out.
fn render_tags(nodes: &[TagWithCount], depth: usize, out: &mut String, rendered: &mut i32) {
    let indent = "  ".repeat(depth + 1);
    for node in nodes.iter().take(MAX_TAGS_PER_LEVEL) {
        *rendered += 1;
        out.push_str(&format!(
            "\n{}{} ({} atom{}, tag_id: {})",
            indent,
            node.tag.name,
            node.atom_count,
            if node.atom_count == 1 { "" } else { "s" },
            node.tag.id
        ));
        if depth + 1 < TAG_TREE_DEPTH {
            render_tags(&node.children, depth + 1, out, rendered);
        } else if node.children_total > 0 {
            out.push_str(&format!(
                "\n{}  ({} sub-tags — call list_tags with parent_id \"{}\")",
                indent, node.children_total, node.tag.id
            ));
        }
    }
    if nodes.len() > MAX_TAGS_PER_LEVEL {
        out.push_str(&format!(
            "\n{}… {} more at this level",
            indent,
            nodes.len() - MAX_TAGS_PER_LEVEL
        ));
    }
}

// ==================== get_wiki ====================

/// The synthesized article for a tag — the one place the knowledge base
/// already answers "what do I know about X?" in prose.
struct GetWiki {
    guard: ScopeGuard,
}

#[async_trait]
impl AgentTool for GetWiki {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "get_wiki",
            "Read the wiki article for a tag: a synthesized overview of everything stored under that topic. Prefer this over stitching many search results together when the user asks about a topic as a whole. Accepts a tag id or tag name. Returns up to `limit` lines starting at `offset`, with a header telling you how to read the rest.",
            paged_schema(
                json!({
                    "tag": {
                        "type": "string",
                        "description": "Tag name or tag id whose article to read (see list_tags)"
                    }
                }),
                json!(["tag"]),
            ),
        )
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext<'_>) -> ToolResult {
        let needle = args["tag"].as_str().unwrap_or("").trim();
        if needle.is_empty() {
            return ToolResult::failed("tag is required");
        }

        let tree = match self.guard.storage.get_all_tags_impl().await {
            Ok(tree) => tree,
            Err(e) => return ToolResult::failed(e),
        };
        let Some(tag) = resolve_tag(&tree, needle) else {
            return ToolResult::ok(
                format!(
                    "No tag matches '{}'. Call list_tags to see what exists.",
                    needle
                ),
                0,
            );
        };

        // An article is the densest read in the knowledge base — a whole
        // tag's worth of atoms in prose. A scope that closes the tag has to
        // close the article with it, or excluding a topic would mean nothing
        // more than excluding its atoms from search.
        if !self.guard.tags(&tree).readable(&tag.tag.id) {
            return ToolResult::ok(
                out_of_scope(&format!("The wiki article for '{}'", tag.tag.name)),
                0,
            );
        }

        let wiki = match self.guard.storage.get_wiki_sync(&tag.tag.id).await {
            Ok(Some(wiki)) => wiki,
            Ok(None) => {
                return ToolResult::ok(
                    format!(
                        "No wiki article exists for '{}' yet. Use search_atoms to read the atoms under the tag instead.",
                        tag.tag.name
                    ),
                    0,
                )
            }
            Err(e) => return ToolResult::failed(e),
        };

        let (offset, limit) = paging_args(args);
        let window = line_window(
            &strip_citation_markers(&wiki.article.content),
            offset,
            limit,
            "get_wiki",
        );
        let number = ctx.citations.register(
            CitationSource::Wiki,
            &tag.tag.id,
            None,
            excerpt(&window.body),
        );
        ToolResult::ok(
            format!(
                "{}(wiki article for tag: {}, tag_id: {}, {} atoms, updated {})\n{}",
                citation_marker(number),
                tag.tag.name,
                tag.tag.id,
                wiki.article.atom_count,
                wiki.article.updated_at,
                window.rendered
            ),
            1,
        )
    }
}

// ==================== list_reports / get_finding ====================

/// What the user's scheduled research has been looking into, and when it
/// last reported back.
struct ListReports {
    guard: ScopeGuard,
}

#[async_trait]
impl AgentTool for ListReports {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "list_reports",
            "List the scheduled research reports configured in this knowledge base, each with its most recent finding. Reports run on a schedule and write their conclusions back as findings. Use get_finding to read one in full.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        )
    }

    async fn execute(&self, _args: &serde_json::Value, _ctx: &ToolContext<'_>) -> ToolResult {
        let reports = match self.guard.storage.list_reports_sync().await {
            Ok(reports) => reports,
            Err(e) => return ToolResult::failed(e),
        };
        if reports.is_empty() {
            return ToolResult::ok("No research reports are configured.", 0);
        }

        let total = reports.len();
        let listed: Vec<_> = reports.into_iter().take(MAX_REPORTS).collect();

        // One finding each: the list is a menu, and get_finding is how the
        // model reads the thing it picks. Loaded up front so the scope check
        // over all of them is a single query.
        let mut latest = Vec::with_capacity(listed.len());
        for report in &listed {
            latest.push(
                self.guard
                    .storage
                    .list_findings_for_report_sync(&report.id, 1)
                    .await
                    .map(|findings| findings.into_iter().next()),
            );
        }
        let finding_atom_ids: Vec<String> = latest
            .iter()
            .filter_map(|result| result.as_ref().ok()?.as_ref())
            .map(|(_, atom)| atom.atom.id.clone())
            .collect();
        // A report's scope and the conversation's are independent, so a
        // finding can sit outside this conversation entirely. The report
        // itself stays listed — its name and schedule are configuration, not
        // knowledge — but the finding's title and id do not, or the listing
        // becomes the index into content the scope closed.
        let admitted = match self.guard.admitted_atoms(&finding_atom_ids).await {
            Ok(admitted) => admitted,
            Err(e) => return ToolResult::failed(e),
        };

        let mut out = String::new();
        let mut count = 0;
        for (report, finding) in listed.into_iter().zip(latest) {
            count += 1;
            out.push_str(&format!(
                "{} (report_id: {}, schedule: {}{})",
                report.name,
                report.id,
                report.schedule,
                if report.enabled { "" } else { ", disabled" }
            ));
            if let Some(description) = report.description.as_deref().filter(|d| !d.is_empty()) {
                out.push_str(&format!("\n  {}", description));
            }
            match finding {
                Ok(Some((finding, atom))) if admitted.contains(&atom.atom.id) => {
                    out.push_str(&format!(
                        "\n  latest finding: {} ({}, atom_id: {})",
                        if atom.atom.title.is_empty() {
                            "(untitled)"
                        } else {
                            atom.atom.title.as_str()
                        },
                        finding.created_at,
                        atom.atom.id
                    ))
                }
                Ok(Some(_)) => {
                    out.push_str("\n  latest finding: outside this conversation's scope")
                }
                Ok(None) => out.push_str("\n  no findings yet"),
                Err(e) => out.push_str(&format!("\n  (could not load findings: {})", e)),
            }
            out.push_str("\n\n");
        }
        if total > MAX_REPORTS {
            out.push_str(&format!("… {} more reports\n", total - MAX_REPORTS));
        }

        ToolResult::ok(out.trim_end(), count)
    }
}

/// One finding in full. Findings are atoms of kind `report`, so this is the
/// atom read path with the provenance the finding reader shows.
struct GetFinding {
    guard: ScopeGuard,
}

#[async_trait]
impl AgentTool for GetFinding {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "get_finding",
            "Read a report finding in full — the write-up a scheduled report produced. Get finding ids from list_reports. Returns up to `limit` lines starting at `offset`, with a header telling you how to read the rest.",
            paged_schema(
                json!({
                    "atom_id": {
                        "type": "string",
                        "description": "The finding's atom id (from list_reports)"
                    }
                }),
                json!(["atom_id"]),
            ),
        )
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext<'_>) -> ToolResult {
        let atom_id = args["atom_id"].as_str().unwrap_or("");

        // Findings are atoms, so they are scoped like atoms — and this is the
        // tool the list_reports menu points at, which makes it the obvious
        // way around a scope if it doesn't check.
        match self.guard.admits_atom(atom_id).await {
            Ok(true) => {}
            Ok(false) => return ToolResult::ok(out_of_scope("That finding"), 0),
            Err(e) => return ToolResult::failed(e),
        }

        let finding = match self.guard.storage.get_atom_impl(atom_id).await {
            Ok(Some(finding)) => finding,
            Ok(None) => return ToolResult::ok("Finding not found", 0),
            Err(e) => return ToolResult::failed(e),
        };
        if finding.atom.kind != AtomKind::Report {
            return ToolResult::ok(
                format!(
                    "Atom {} is not a report finding. Use get_atom to read it.",
                    atom_id
                ),
                0,
            );
        }

        let provenance = self
            .guard
            .storage
            .get_finding_provenance_sync(atom_id)
            .await
            .unwrap_or_default();
        let report_name = provenance
            .as_ref()
            .map(|p| p.report_name_snapshot.as_str())
            .unwrap_or("(unknown report)");

        let (offset, limit) = paging_args(args);
        let window = line_window(
            &strip_citation_markers(&finding.atom.content),
            offset,
            limit,
            "get_finding",
        );
        let number = ctx.citations.register(
            CitationSource::Finding,
            atom_id,
            None,
            excerpt(&window.body),
        );
        ToolResult::ok(
            format!(
                "{}(finding from report: {}, atom_id: {}, created {})\n{}",
                citation_marker(number),
                report_name,
                atom_id,
                finding.atom.created_at,
                window.rendered
            ),
            1,
        )
    }
}

// ==================== create_atom / edit_atom ====================

/// Shared state for the tools that write to the knowledge base.
#[derive(Clone)]
struct MutationDeps {
    storage: StorageBackend,
    settings: Option<HashMap<String, String>>,
    inline_pipeline: bool,
    canvas_cache: Option<CanvasCache>,
    conversation_id: String,
    on_event: ChatEventCallback,
    on_embedding_event: Arc<dyn Fn(EmbeddingEvent) + Send + Sync + 'static>,
}

impl MutationDeps {
    /// Announce the mutation, make the atom citable, and describe it back to
    /// the model — identical for creates and edits.
    fn report(
        &self,
        atom: AtomWithTags,
        event: fn(String, AtomWithTags) -> ChatEvent,
        ctx: &ToolContext<'_>,
    ) -> ToolResult {
        (self.on_event)(event(self.conversation_id.clone(), atom.clone()));
        let number = ctx.citations.register(
            CitationSource::Atom,
            &atom.atom.id,
            None,
            excerpt(&atom.atom.snippet),
        );
        let mut payload = json!({
            "atom_id": atom.atom.id,
            "title": atom.atom.title,
            "snippet": atom.atom.snippet,
            "reference": format!("[[{}]]", atom.atom.id),
        });
        if let (Some(number), Some(payload)) = (number, payload.as_object_mut()) {
            payload.insert("citation_index".to_string(), json!(number));
        }
        ToolResult::ok(
            serde_json::to_string_pretty(&payload).unwrap_or(atom.atom.id),
            1,
        )
    }
}

struct CreateAtom {
    deps: MutationDeps,
}

#[async_trait]
impl AgentTool for CreateAtom {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "create_atom",
            "Create a new atom with markdown content. Only use this when the user explicitly asks you to create, save, draft, or add a new atom/note. Do not call this for ordinary answers. After creating an atom, mention it in your final response using [[atom_id]] so the UI can link to it.",
            json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "Full markdown content for the new atom"
                    },
                    "source_url": {
                        "type": "string",
                        "description": "Optional source URL for the atom"
                    },
                    "published_at": {
                        "type": "string",
                        "description": "Optional ISO 8601 publication date"
                    },
                    "tag_ids": {
                        "type": "array",
                        "description": "Optional existing tag IDs to assign",
                        "items": { "type": "string" },
                        "default": []
                    }
                },
                "required": ["content"]
            }),
        )
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext<'_>) -> ToolResult {
        let content = args["content"].as_str().unwrap_or("").to_string();
        if content.trim().is_empty() {
            return ToolResult::failed("Cannot create an empty atom");
        }

        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let request = crate::CreateAtomRequest {
            content,
            source_url: optional_string_arg(args, "source_url"),
            published_at: optional_string_arg(args, "published_at"),
            tag_ids: tag_ids_arg(args),
            skip_if_source_exists: false,
        };

        let atom = match self
            .deps
            .storage
            .insert_atom_impl(&id, &request, &now)
            .await
        {
            Ok(atom) => atom,
            Err(e) => return ToolResult::failed(e),
        };
        if let Some(cache) = &self.deps.canvas_cache {
            cache.invalidate();
        }
        enqueue_pipeline_in_background(&self.deps, id, "agent_create_atom");

        self.deps.report(
            atom,
            |conversation_id, atom| ChatEvent::AtomCreated {
                conversation_id,
                atom,
            },
            ctx,
        )
    }
}

struct EditAtom {
    deps: MutationDeps,
}

#[async_trait]
impl AgentTool for EditAtom {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "edit_atom",
            "Apply edits to an existing atom. Only use this when the user explicitly asks you to modify an atom. Supports replace, insert_after, append, and replace_all operations. Prefer targeted edits. Use replace_all only when the user explicitly asks for a full rewrite or provides complete replacement content. replace and insert_after require exact text that appears exactly once in the current atom; call get_atom first if you need context.",
            json!({
                "type": "object",
                "properties": {
                    "atom_id": {
                        "type": "string",
                        "description": "The ID of the atom to edit"
                    },
                    "edits": {
                        "type": "array",
                        "description": "One or more edits applied in order to the atom content. The whole operation fails without saving if any edit is invalid.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "operation": {
                                    "type": "string",
                                    "enum": ["replace", "insert_after", "append", "replace_all"],
                                    "description": "replace swaps exact old_text for new_text; insert_after inserts text after exact anchor_text; append adds text to the end of the atom; replace_all replaces the full atom content."
                                },
                                "old_text": {
                                    "type": "string",
                                    "description": "Exact text to replace. Required for replace and must occur exactly once."
                                },
                                "new_text": {
                                    "type": "string",
                                    "description": "Replacement text for replace."
                                },
                                "anchor_text": {
                                    "type": "string",
                                    "description": "Exact text to insert after. Required for insert_after and must occur exactly once."
                                },
                                "text": {
                                    "type": "string",
                                    "description": "Text to insert for insert_after or append. Include leading newlines/spaces exactly as desired."
                                },
                                "content": {
                                    "type": "string",
                                    "description": "Full replacement markdown content. Required for replace_all."
                                }
                            },
                            "required": ["operation"]
                        },
                        "minItems": 1
                    }
                },
                "required": ["atom_id", "edits"]
            }),
        )
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext<'_>) -> ToolResult {
        let atom_id = args["atom_id"].as_str().unwrap_or("");
        let existing = match self.deps.storage.get_atom_impl(atom_id).await {
            Ok(Some(existing)) => existing,
            Ok(None) => return ToolResult::ok("Atom not found", 0),
            Err(e) => return ToolResult::failed(e),
        };

        let edits: Vec<AtomEditOperation> = match args
            .get("edits")
            .cloned()
            .ok_or_else(|| "edits must be an array".to_string())
            .and_then(|value| {
                serde_json::from_value(value)
                    .map_err(|e| format!("edits must be valid edit operations: {}", e))
            }) {
            Ok(edits) => edits,
            Err(e) => return ToolResult::failed(e),
        };
        let content = match apply_atom_edits(&existing.atom.content, &edits) {
            Ok(content) => content,
            Err(e) => return ToolResult::failed(e),
        };
        if content == existing.atom.content {
            return ToolResult::failed("Edits did not change the atom content");
        }
        if content.trim().is_empty() {
            return ToolResult::failed("Cannot update an atom to empty content");
        }

        let request = crate::UpdateAtomRequest {
            content,
            source_url: existing.atom.source_url,
            published_at: existing.atom.published_at,
            tag_ids: None,
        };
        let now = Utc::now().to_rfc3339();
        let atom = match self
            .deps
            .storage
            .update_atom_if_unchanged_impl(atom_id, &request, &now, &existing.atom.updated_at)
            .await
        {
            Ok(atom) => atom,
            Err(e) => return ToolResult::failed(e),
        };
        if let Some(cache) = &self.deps.canvas_cache {
            cache.invalidate();
        }
        enqueue_pipeline_in_background(&self.deps, atom_id.to_string(), "agent_edit_atom");

        self.deps.report(
            atom,
            |conversation_id, atom| ChatEvent::AtomUpdated {
                conversation_id,
                atom,
            },
            ctx,
        )
    }
}

fn optional_string_arg(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn tag_ids_arg(args: &serde_json::Value) -> Vec<String> {
    args.get("tag_ids")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn enqueue_pipeline_in_background(deps: &MutationDeps, atom_id: String, reason: &'static str) {
    let deps = deps.clone();
    let failure_callback = Arc::clone(&deps.on_embedding_event);
    tokio::spawn(async move {
        let result = enqueue_and_process_pipeline(&deps, &atom_id, reason).await;
        if let Err(error) = result {
            tracing::warn!(
                atom_id = %atom_id,
                reason = %reason,
                error = %error,
                "Agent mutation pipeline failed"
            );
            failure_callback(EmbeddingEvent::EmbeddingFailed { atom_id, error });
        }
    });
}

async fn enqueue_and_process_pipeline(
    deps: &MutationDeps,
    atom_id: &str,
    reason: &str,
) -> Result<(), String> {
    let job = crate::models::AtomPipelineJobRequest {
        atom_id: atom_id.to_string(),
        embed_requested: true,
        tag_requested: true,
        not_before: None,
        reason: reason.to_string(),
        replace_existing: false,
    };
    deps.storage
        .enqueue_pipeline_jobs_sync(&[job])
        .await
        .map_err(|e| e.to_string())?;
    if !deps.inline_pipeline {
        // The job persists in the durable ledger; the host's dedicated
        // pipeline worker executes it (see AtomicCore::set_inline_pipeline).
        return Ok(());
    }
    let callback = {
        let on_embedding_event = Arc::clone(&deps.on_embedding_event);
        move |event| on_embedding_event(event)
    };
    match deps.settings.clone() {
        Some(settings) => {
            crate::embedding::process_queued_pipeline_jobs_with_settings(
                deps.storage.clone(),
                callback,
                settings,
                deps.canvas_cache.clone(),
            )
            .await?;
        }
        None => {
            crate::embedding::process_queued_pipeline_jobs(
                deps.storage.clone(),
                callback,
                deps.canvas_cache.clone(),
            )
            .await?;
        }
    }
    Ok(())
}

// ==================== get_current_page_context ====================

struct CurrentPageContext {
    guard: ScopeGuard,
    page_context: PageContext,
}

#[async_trait]
impl AgentTool for CurrentPageContext {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "get_current_page_context",
            "Get compact context about what the user is currently viewing in Atomic, such as the visible atom, wiki page, selected tag, and a short atom snippet. Use this first when the user says things like \"this atom\", \"the note I'm reading\", \"this page\", \"what I'm looking at\", or otherwise refers to visible UI context.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        )
    }

    async fn execute(&self, _args: &serde_json::Value, ctx: &ToolContext<'_>) -> ToolResult {
        let page = &self.page_context;
        let mut visible_atom = serde_json::Value::Null;
        if let Some(atom_id) = page.atom_id.as_deref().filter(|id| !id.is_empty()) {
            let stored = match self.guard.storage.get_atom_impl(atom_id).await {
                Ok(stored) => stored,
                Err(e) => return ToolResult::failed(e),
            };
            let found = stored.is_some();
            visible_atom = match stored {
                Some(atom_with_tags) => json!({
                    "id": atom_with_tags.atom.id,
                    "title": atom_with_tags.atom.title,
                    "snippet": atom_with_tags.atom.snippet,
                    "source_url": atom_with_tags.atom.source_url,
                    "tags": atom_with_tags
                        .tags
                        .into_iter()
                        .map(|tag| json!({ "id": tag.id, "name": tag.name }))
                        .collect::<Vec<_>>(),
                }),
                None => json!({
                    "id": atom_id,
                    "title": page.atom_title.as_deref(),
                    "snippet": page.atom_snippet.as_deref(),
                    "not_found": true,
                }),
            };

            // What the user is looking at reaches the model because the user
            // shared it — the context chip is that consent, per message. A
            // *citation* is a different claim: it says the answer stands on a
            // source this conversation is allowed to draw on. An atom the
            // scope excludes isn't, and neither is one the frontend named
            // that no longer exists — citing either would put a source in the
            // answer that nothing can open.
            let citable = found
                && match self.guard.admits_atom(atom_id).await {
                    Ok(admitted) => admitted,
                    Err(e) => return ToolResult::failed(e),
                };
            let snippet = visible_atom
                .get("snippet")
                .and_then(|snippet| snippet.as_str())
                .unwrap_or("");
            if citable && !snippet.is_empty() {
                let number =
                    ctx.citations
                        .register(CitationSource::Atom, atom_id, None, excerpt(snippet));
                if let (Some(number), Some(atom)) = (number, visible_atom.as_object_mut()) {
                    atom.insert("citation_index".to_string(), json!(number));
                }
            }
        }

        let context = json!({
            "view": page.view.as_deref(),
            "visible_atom": visible_atom,
            "wiki": {
                "tag_id": page.wiki_tag_id.as_deref(),
                "tag_name": page.wiki_tag_name.as_deref(),
            },
            "selected_tag_id": page.selected_tag_id.as_deref(),
        });
        ToolResult::ok(
            serde_json::to_string_pretty(&context).unwrap_or_else(|_| context.to_string()),
            1,
        )
    }
}

// ==================== canvas navigation ====================

#[derive(Clone, Copy)]
enum CanvasAction {
    ZoomToCluster,
    FocusAtom,
}

/// The canvas tools do no work in core: they ask the frontend to move its
/// camera and report back what they asked for.
struct CanvasNavigation {
    action: CanvasAction,
    conversation_id: String,
    on_event: ChatEventCallback,
}

#[async_trait]
impl AgentTool for CanvasNavigation {
    fn definition(&self) -> ToolDefinition {
        match self.action {
            CanvasAction::ZoomToCluster => ToolDefinition::new(
                "zoom_to_cluster",
                "Zoom the canvas camera to center on a single cluster of related atoms. The canvas can only show one view at a time — each call replaces the previous view. Use this when the user asks about a topic area visible on their knowledge graph.",
                json!({
                    "type": "object",
                    "properties": {
                        "cluster_label": {
                            "type": "string",
                            "description": "The label of the cluster to zoom to"
                        }
                    },
                    "required": ["cluster_label"]
                }),
            ),
            CanvasAction::FocusAtom => ToolDefinition::new(
                "focus_atom",
                "Zoom to a specific atom on the canvas and show its preview. The canvas can only focus on one atom at a time. Use this after searching to highlight a specific result on the graph.",
                json!({
                    "type": "object",
                    "properties": {
                        "atom_id": {
                            "type": "string",
                            "description": "The ID of the atom to focus on"
                        }
                    },
                    "required": ["atom_id"]
                }),
            ),
        }
    }

    async fn execute(&self, args: &serde_json::Value, _ctx: &ToolContext<'_>) -> ToolResult {
        let (action, params, acknowledgement) = match self.action {
            CanvasAction::ZoomToCluster => {
                let label = args["cluster_label"].as_str().unwrap_or("");
                (
                    "zoom_to_cluster",
                    json!({ "cluster_label": label }),
                    format!("Zoomed canvas to cluster '{}'", label),
                )
            }
            CanvasAction::FocusAtom => {
                let atom_id = args["atom_id"].as_str().unwrap_or("");
                (
                    "focus_atom",
                    json!({ "atom_id": atom_id }),
                    format!("Focused canvas on atom '{}'", atom_id),
                )
            }
        };
        (self.on_event)(ChatEvent::CanvasAction {
            conversation_id: self.conversation_id.clone(),
            action: action.to_string(),
            params,
        });
        ToolResult::ok(acknowledgement, 1)
    }
}

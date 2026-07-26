use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ==================== Tool Input Types ====================

/// Input parameters for semantic_search tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SemanticSearchParams {
    /// The search query to find relevant atoms using vector similarity
    pub query: String,

    /// Maximum number of results to return (default: 10, max: 50)
    #[serde(default)]
    pub limit: Option<i32>,

    /// Optional recency filter: only return atoms created within the last N days.
    /// Use this when the user asks about recent notes ("this week", "last month", etc.).
    #[serde(default)]
    pub since_days: Option<i32>,

    /// Optional tag ID to scope results to atoms under that tag (from list_tags).
    #[serde(default)]
    pub tag_id: Option<String>,

    /// Include report-generated finding atoms alongside captured notes (default: false).
    #[serde(default)]
    pub include_generated: Option<bool>,
}

/// Input parameters for read_atom tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadAtomParams {
    /// The UUID of the atom to retrieve
    pub atom_id: String,

    /// Maximum number of lines to return (default: 500, max: 500)
    #[serde(default)]
    pub limit: Option<i32>,

    /// Line offset for pagination, 0-indexed (default: 0)
    #[serde(default)]
    pub offset: Option<i32>,
}

/// Input parameters for create_atom tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateAtomParams {
    /// The markdown content of the atom
    pub content: String,

    /// Optional source URL where this content originated
    #[serde(default)]
    pub source_url: Option<String>,
}

/// Input parameters for update_atom tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateAtomParams {
    /// The UUID of the atom to update
    pub atom_id: String,

    /// Optional replacement markdown content for the atom. Omit to preserve current content.
    #[serde(default)]
    pub content: Option<String>,

    /// Optional replacement source URL. Omit to preserve current source URL.
    #[serde(default)]
    pub source_url: Option<String>,

    /// Optional replacement publication date. Omit to preserve current publication date.
    #[serde(default)]
    pub published_at: Option<String>,

    /// Optional replacement tag IDs (from list_tags). Omit to preserve current tags; pass [] to clear tags.
    #[serde(default)]
    pub tag_ids: Option<Vec<String>>,
}

/// A single edit operation for edit_atom.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct EditOperation {
    /// Operation type: replace, insert_after, append, or replace_all.
    pub operation: String,

    /// Exact text to replace. Required for replace and must occur exactly once.
    #[serde(default)]
    pub old_text: Option<String>,

    /// Replacement text for replace.
    #[serde(default)]
    pub new_text: Option<String>,

    /// Exact text to insert after. Required for insert_after and must occur exactly once.
    #[serde(default)]
    pub anchor_text: Option<String>,

    /// Text to insert for insert_after or append.
    #[serde(default)]
    pub text: Option<String>,

    /// Full replacement markdown content. Required for replace_all.
    #[serde(default)]
    pub content: Option<String>,
}

impl From<&EditOperation> for atomic_core::AtomEditOperation {
    fn from(value: &EditOperation) -> Self {
        Self {
            operation: value.operation.clone(),
            old_text: value.old_text.clone(),
            new_text: value.new_text.clone(),
            anchor_text: value.anchor_text.clone(),
            text: value.text.clone(),
            content: value.content.clone(),
        }
    }
}

/// Input parameters for edit_atom tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct EditAtomParams {
    /// The UUID of the atom to edit
    pub atom_id: String,

    /// Edits to apply in order. The whole operation fails if any edit is invalid.
    pub edits: Vec<EditOperation>,
}

/// Input parameters for ingest_url tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct IngestUrlParams {
    /// URL to fetch, extract, and save as an atom. Exact source_url matches return the existing atom.
    pub url: String,
}

/// Input parameters for list_tags tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListTagsParams {
    /// Prune leaf tags with fewer than this many atoms (default: 0, keep everything)
    #[serde(default)]
    pub min_count: Option<i32>,
}

/// Input parameters for list_atoms tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListAtomsToolParams {
    /// Optional tag ID to filter by (from list_tags; includes atoms under descendant tags)
    #[serde(default)]
    pub tag_id: Option<String>,

    /// Maximum number of atoms to return (default: 20, max: 100)
    #[serde(default)]
    pub limit: Option<i32>,

    /// Offset for pagination, 0-indexed (default: 0)
    #[serde(default)]
    pub offset: Option<i32>,

    /// Include report-generated finding atoms alongside captured notes (default: false)
    #[serde(default)]
    pub include_generated: Option<bool>,
}

/// Input parameters for find_similar tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindSimilarParams {
    /// The UUID of the atom to find semantic neighbors for
    pub atom_id: String,

    /// Maximum number of results to return (default: 10, max: 50)
    #[serde(default)]
    pub limit: Option<i32>,
}

/// Input parameters for the ChatGPT-compatible search tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeepSearchParams {
    /// The search query
    pub query: String,
}

/// Input parameters for the ChatGPT-compatible fetch tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FetchParams {
    /// The atom ID returned by a previous search call
    pub id: String,
}

/// Input parameters for get_wiki tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetWikiParams {
    /// Tag ID or exact tag name (case-insensitive) whose wiki article to read
    pub tag: String,
}

/// Input parameters for get_report_findings tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetReportFindingsParams {
    /// The report ID (from list_reports)
    pub report_id: String,

    /// Maximum number of findings to return, most recent first (default: 3, max: 10)
    #[serde(default)]
    pub limit: Option<i32>,
}

// ==================== Tool Output Types ====================

/// A search result with atom content and similarity score
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub atom_id: String,
    pub content_preview: String,
    pub similarity_score: f32,
    pub matching_chunk: String,
}

/// Paginated atom content response
#[derive(Debug, Serialize)]
pub struct AtomContent {
    pub atom_id: String,
    pub content: String,
    pub total_lines: i32,
    pub returned_lines: i32,
    pub offset: i32,
    pub has_more: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Created/updated atom response
#[derive(Debug, Serialize)]
pub struct AtomResponse {
    pub atom_id: String,
    pub content_preview: String,
    pub tags: Vec<String>,
    pub embedding_status: String,
}

/// Ingested URL response
#[derive(Debug, Serialize)]
pub struct IngestUrlResponse {
    pub atom_id: String,
    pub url: String,
    pub title: String,
    pub content_length: usize,
    pub already_exists: bool,
}

/// A node in the hierarchical tag tree
#[derive(Debug, Serialize)]
pub struct TagNode {
    pub tag_id: String,
    pub name: String,
    pub atom_count: i32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TagNode>,
}

/// One atom in a paginated listing (summary only, no full content)
#[derive(Debug, Serialize)]
pub struct AtomListItem {
    pub atom_id: String,
    pub title: String,
    pub snippet: String,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Paginated atom listing response
#[derive(Debug, Serialize)]
pub struct AtomListResponse {
    pub atoms: Vec<AtomListItem>,
    pub total_count: i32,
    pub offset: i32,
    pub has_more: bool,
}

/// One knowledge database visible to this connection
#[derive(Debug, Serialize)]
pub struct DatabaseListItem {
    pub database_id: String,
    pub name: String,
    pub is_default: bool,
    /// True for the database this connection's tool calls operate on.
    pub selected: bool,
}

/// A semantic neighbor of an atom
#[derive(Debug, Serialize)]
pub struct SimilarAtomItem {
    pub atom_id: String,
    pub title: String,
    pub content_preview: String,
    pub similarity_score: f32,
}

/// Envelope for the ChatGPT-compatible search tool
#[derive(Debug, Serialize)]
pub struct DeepSearchResponse {
    pub results: Vec<DeepSearchResult>,
}

/// One result row for the ChatGPT-compatible search tool
#[derive(Debug, Serialize)]
pub struct DeepSearchResult {
    pub id: String,
    pub title: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Full document for the ChatGPT-compatible fetch tool
#[derive(Debug, Serialize)]
pub struct FetchResponse {
    pub id: String,
    pub title: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub metadata: FetchMetadata,
}

/// Metadata block for the ChatGPT-compatible fetch tool
#[derive(Debug, Serialize)]
pub struct FetchMetadata {
    pub created_at: String,
    pub updated_at: String,
    pub tags: Vec<String>,
}

/// Wiki article summary for list view
#[derive(Debug, Serialize)]
pub struct WikiListItem {
    pub tag_id: String,
    pub tag_name: String,
    pub atom_count: i32,
    pub updated_at: String,
}

/// Wiki article with its citations
#[derive(Debug, Serialize)]
pub struct WikiResponse {
    pub tag_id: String,
    pub tag_name: String,
    pub content: String,
    pub atom_count: i32,
    pub updated_at: String,
    pub citations: Vec<WikiCitationItem>,
}

/// One inline citation in a wiki article: `[N]` markers in the content
/// resolve to `citation_index` entries here.
#[derive(Debug, Serialize)]
pub struct WikiCitationItem {
    pub citation_index: i32,
    pub atom_id: String,
    pub excerpt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

/// Report definition summary
#[derive(Debug, Serialize)]
pub struct ReportListItem {
    pub report_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub schedule: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_tz: Option<String>,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// A report finding (agent-authored research atom) with its citations
#[derive(Debug, Serialize)]
pub struct ReportFindingItem {
    pub atom_id: String,
    pub title: String,
    pub report_name: String,
    pub created_at: String,
    pub content: String,
    pub citations: Vec<FindingCitationItem>,
}

/// One `[N]` citation marker in a finding, resolved to its source atom
#[derive(Debug, Serialize)]
pub struct FindingCitationItem {
    pub position: i32,
    pub cited_atom_id: String,
    pub excerpt: String,
}

---
title: MCP Server
description: Give AI agents long-term memory by connecting them to Atomic through the Model Context Protocol.
---

Atomic exposes an MCP server so AI agents can search, browse, and write to your knowledge base — including the synthesized layers (wiki articles and report findings) that plain note-search tools don't have. You can use it locally through the desktop app bridge or remotely through the HTTP endpoint.

## Tools

Atomic exposes sixteen MCP tools. All read tools are annotated read-only, so clients that gate write actions (like claude.ai) won't prompt for them.

### Search and retrieval

| Tool | What It Does |
|------|--------------|
| `semantic_search` | Hybrid keyword + semantic search over atoms. Optional `since_days` recency filter, `tag_id` topic scoping, and `include_generated` to also search report findings |
| `read_atom` | Read full atom content by ID, with line-based pagination |
| `find_similar` | Semantic neighbors of an atom — traverse the knowledge graph outward from a relevant result |

### Browsing and navigation

| Tool | What It Does |
|------|--------------|
| `list_tags` | The hierarchical tag tree with atom counts — the map of how the knowledge base is organized, and the source of `tag_id` values for other tools |
| `list_atoms` | Paginated atom listing, most recently updated first, optionally filtered to a tag |
| `list_databases` | The knowledge databases available to this connection, and which one tool calls operate on |

### Synthesized knowledge

| Tool | What It Does |
|------|--------------|
| `list_wikis` | All wiki articles — synthesized overviews of everything stored under a tag |
| `get_wiki` | Read one wiki article by tag ID or name, with citations back to source atoms |
| `list_reports` | Automated research reports configured in the knowledge base |
| `get_report_findings` | The most recent findings a report produced, with citations |

### Writing

| Tool | What It Does |
|------|--------------|
| `create_atom` | Store new markdown memory as an atom |
| `ingest_url` | Fetch a URL, extract article content, and save it as an atom. Returns the existing atom with `already_exists: true` on duplicate URLs |
| `update_atom` | Full/partial atom update — content, metadata, or tags (`tag_ids` from `list_tags`) |
| `edit_atom` | Targeted, safe markdown edits: `replace`, `insert_after`, `append`, `replace_all` |

### ChatGPT compatibility

| Tool | What It Does |
|------|--------------|
| `search` | Alias of `semantic_search` in the document shape ChatGPT's deep research mode requires |
| `fetch` | Alias of `read_atom` in the same document shape |

The server instructions tell agents to search before answering from memory, remember durable context, update stale atoms instead of duplicating them, and reach for wikis when the user wants a topic overview.

## Desktop App: Local Bridge

The desktop app bundles `atomic-mcp-bridge`, a stdio-to-HTTP bridge. It reads the local sidecar connection automatically, so you do not need to create or paste a token. The bridge forwards MCP requests to Atomic's `/mcp` endpoint; tools are advertised by the server, so new server-side tools do not require bridge-specific configuration.

Open **Settings > Integrations > MCP Integration** in the desktop app for the exact bridge path.

Example for Claude Code, Claude Desktop, or any stdio MCP client:

```json
{
  "mcpServers": {
    "atomic": {
      "command": "/Applications/Atomic.app/Contents/MacOS/atomic-mcp-bridge"
    }
  }
}
```

On Windows the binary name is `atomic-mcp-bridge.exe`. On Linux the path depends on the installed package layout.

### Bridge configuration

The bridge is configured entirely through environment variables — useful when the server isn't on the default local port or you're pointing the bridge at a remote instance:

| Variable | Default | Purpose |
|----------|---------|---------|
| `ATOMIC_URL` | *(unset)* | Full base URL of a remote server (e.g. `https://you.atomicapp.ai`). Wins over host/port when set |
| `ATOMIC_HOST` | `127.0.0.1` | Host of the local Atomic server |
| `ATOMIC_PORT` | `44380` | Port of the local Atomic server |
| `ATOMIC_TOKEN` | *(auto)* | Bearer token. When unset, the bridge reads the desktop app's local token file automatically |

## Remote or Self-Hosted: Streamable HTTP

For self-hosted servers, connect to:

```text
https://your-server.example/mcp
```

Use a Bearer token:

```json
{
  "mcpServers": {
    "atomic": {
      "url": "https://your-server.example/mcp",
      "headers": {
        "Authorization": "Bearer YOUR_TOKEN"
      }
    }
  }
}
```

Some clients use `"type": "url"` for HTTP MCP servers. If your client requires it, add that field to the `atomic` object.

## Connect Specific Clients

### claude.ai and Claude Desktop (custom connector)

claude.ai custom connectors authenticate with OAuth rather than a pasted token, and Atomic implements the full flow:

1. In claude.ai (or Claude Desktop), open **Settings → Connectors → Add custom connector**.
2. Name it (e.g. "Atomic") and paste your MCP URL — `https://<your-subdomain>.atomicapp.ai/mcp` for cloud, or `https://your-server.example/mcp` for self-hosted.
3. Click **Connect**. Your browser opens Atomic's consent page: on cloud you approve with your existing login; on self-hosted you approve by pasting an API token once.
4. Done — Claude can now use every Atomic tool, and read-only tools run without confirmation prompts.

Self-hosted note: the OAuth flow requires `PUBLIC_URL` to be set (see [OAuth and Public URL](#oauth-and-public-url) below). Without it, use a client that supports Bearer headers instead.

### Claude Code

```bash
claude mcp add --transport http atomic https://your-server.example/mcp \
  --header "Authorization: Bearer YOUR_TOKEN"
```

Or with the desktop app installed, use the stdio bridge and skip tokens entirely:

```bash
claude mcp add atomic /Applications/Atomic.app/Contents/MacOS/atomic-mcp-bridge
```

### Cursor

Add to `.cursor/mcp.json` (project) or `~/.cursor/mcp.json` (global):

```json
{
  "mcpServers": {
    "atomic": {
      "url": "https://your-server.example/mcp",
      "headers": {
        "Authorization": "Bearer YOUR_TOKEN"
      }
    }
  }
}
```

### ChatGPT

ChatGPT connects to remote MCP servers on Plus/Pro and Business plans:

1. Enable developer mode: **Settings → Apps & Connectors → Advanced settings → Developer mode**.
2. Add a connector with your Atomic MCP URL.
3. In deep research, ChatGPT uses Atomic's `search` and `fetch` tools; in developer-mode chats it can use the full tool set.

The server must be reachable from the public internet — ChatGPT cannot connect to localhost, so use a cloud tenant or a self-hosted server with a public URL.

## Create a Token

From the UI, create a dedicated token in Settings or the onboarding integration step.

From the CLI:

```bash
atomic-server --data-dir ./data token create --name "claude"
```

Save the raw token immediately. It is shown only once.

## Multi-Database

To target a specific database, add the `db` query parameter:

```text
https://your-server.example/mcp?db=<database-id>
```

Without `db`, MCP tools use the active database. Agents can call `list_databases` to see what exists and which database their connection operates on — but switching requires changing the connection URL.

## OAuth and Public URL

Remote MCP OAuth discovery depends on `PUBLIC_URL` / `--public-url`. If this is not set, OAuth discovery endpoints return 404.

For self-hosted deployments:

```bash
PUBLIC_URL=https://atomic.example.com docker compose up -d
```

Your reverse proxy must pass:

- `/mcp`
- `/.well-known/oauth-authorization-server`
- `/.well-known/oauth-protected-resource`
- `/oauth/register`
- `/oauth/authorize`
- `/oauth/token`

## Suggested Agent Prompt

Add guidance like this to your project instructions:

```markdown
You have access to Atomic, your long-term memory. Search Atomic before answering
questions that may relate to past context. Store durable preferences, decisions,
project context, and important facts. Update stale atoms instead of creating
duplicates. When asked for an overview of a topic, check get_wiki before piecing
together search results.
```

## Troubleshooting

- **Desktop bridge cannot connect** - open Atomic first; the sidecar runs while the desktop app is running.
- **HTTP MCP returns 401** - create a new token and update your MCP client config.
- **Remote OAuth discovery fails** - set `PUBLIC_URL` and verify the `.well-known` routes through your proxy.
- **Agent cannot find expected memory** - check that it is using the intended database (`list_databases` shows which one the connection operates on).
- **Search can't see report findings** - external clients get captured notes only by default; pass `include_generated: true` to `semantic_search`, or read findings directly with `get_report_findings`.

## Related

- [Token Management](/self-hosting/token-management/)
- [Multi-Database](/guides/multi-database/)
- [Self-Hosting](/getting-started/self-hosting/)

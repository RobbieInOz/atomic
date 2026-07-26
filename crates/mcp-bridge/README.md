# atomic-mcp-bridge

A stdio-to-HTTP bridge for Atomic's MCP server. MCP clients that only speak
stdio (Claude Desktop's local server config, and any client using the
`command` form) run this binary; it forwards each JSON-RPC message to an
Atomic server's Streamable HTTP `/mcp` endpoint and relays the responses —
including SSE-framed ones — back over stdout.

The desktop app bundles the bridge as a sidecar-adjacent binary and shows the
exact path under **Settings > Integrations > MCP Integration**. Tools are
advertised by the server, so new server-side tools never require a bridge
update.

## Configuration

Everything is environment-variable driven:

| Variable | Default | Purpose |
|----------|---------|---------|
| `ATOMIC_URL` | *(unset)* | Full base URL of the Atomic server (e.g. `https://you.atomicapp.ai`). Wins over host/port when set — use this for remote HTTPS servers |
| `ATOMIC_HOST` | `127.0.0.1` | Host of the local Atomic server (plain HTTP) |
| `ATOMIC_PORT` | `44380` | Port of the local Atomic server |
| `ATOMIC_TOKEN` | *(auto-discovered)* | Bearer token for `/mcp`. When unset, the bridge reads the desktop app's local token file (`<data-dir>/com.atomic.app/local_server_token`), so the bundled desktop setup needs no configuration |

## Example client config

```json
{
  "mcpServers": {
    "atomic": {
      "command": "/Applications/Atomic.app/Contents/MacOS/atomic-mcp-bridge"
    }
  }
}
```

Pointing the bridge at a remote server:

```json
{
  "mcpServers": {
    "atomic": {
      "command": "atomic-mcp-bridge",
      "env": {
        "ATOMIC_URL": "https://atomic.example.com",
        "ATOMIC_TOKEN": "YOUR_TOKEN"
      }
    }
  }
}
```

Note: clients that support Streamable HTTP directly (claude.ai, Claude Code,
Cursor) don't need the bridge — point them at `https://server/mcp` with a
Bearer header instead. See `docs/manual/guides/mcp-server.md`.

## Limitations

The bridge forwards request/response traffic only. It does not open the
standalone GET (server-to-client) stream, so server-initiated notifications
don't flow through it — none of Atomic's current tools require them.

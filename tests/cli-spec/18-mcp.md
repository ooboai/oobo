# `oobo mcp`

MCP server for AI tool integration. Exposes code search and engineering memory over the Model Context Protocol (stdio JSON-RPC).

Subcommands:
- `oobo mcp` (no subcommand) - start the MCP server on stdin/stdout
- `oobo mcp install [tool]` - configure AI tools to use oobo MCP

---

## Start MCP server

### Invocation
`oobo mcp`

**Behavior:** Starts a stdio MCP server. Reads JSON-RPC requests from stdin, writes responses to stdout. Blocks until stdin closes (EOF). Intended to be launched by AI tools (Cursor, Claude, etc.) as a child process.

**Tools exposed (dynamic based on environment):**
- `search` - local code search (if inside a git repo or any directory)
- `find_related` - find semantically similar code (if inside a git repo)
- `recall` - search engineering memory (if API key configured)
- `get_context` - token-budgeted context for current files/topic (if API key configured)
- `ask` - conversational questions about team's work (if API key configured)

**Exit code:** `0` on clean shutdown.

---

## Install MCP config

### Invocation
`oobo mcp install`

**Behavior:** Auto-detects installed AI tools and writes MCP configuration. Merges into existing config without clobbering other MCP servers.

**Exit code:** `0` on success.

---

## Install for specific tool

### Invocation
`oobo mcp install cursor`

**Behavior:** Writes MCP configuration for the specified tool only.

**Exit code:** `0` on success, `2` if tool name is unrecognized.

---

## Install hosted mode

### Invocation
`oobo mcp install --hosted`

**Behavior:** Configures the AI tool to connect to the hosted MCP endpoint (`https://agentic.oobo.ai/mcp`) instead of launching a local `oobo mcp` process. Requires `OOBO_API_KEY` environment variable.

---

## Remove MCP config

### Invocation
`oobo mcp install --remove`

**Behavior:** Removes the "oobo" entry from the AI tool's MCP config file. Does not affect other MCP servers.

**Exit code:** `0` on success.

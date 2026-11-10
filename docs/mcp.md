# MCP server

`navigera mcp` is a Model Context Protocol server for Cline, Roo Code, Kilo
Code, Continue, VS Code, Claude Desktop and Cursor, with the same tree, refs and
waits as the CLI and a tool list half the size of Playwright MCP's. Add it as
`{"command": "navigera", "args": ["mcp"]}`, or without installing anything as
`{"command": "npx", "args": ["-y", "navigera", "mcp"]}`; per-client snippets
are in [docs/navigera.md](navigera.md#mcp-server-ide-agents). In the MCP
Registry it is `io.github.andrey-usa/navigera`. Agents that run
shell commands (Kilo, Cline CLI, Gemini CLI, Codex) can use the CLI and the
skill directly: `navigera install-skill` writes it to `.agents/skills`, where
they look for it.

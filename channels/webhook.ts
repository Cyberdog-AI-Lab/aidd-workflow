import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { parseArgs } from "util";

const { values } = parseArgs({
	args: Bun.argv.slice(2),
	options: {
		port: { type: "string", default: "8788" },
		hostname: { type: "string", default: "127.0.0.1" },
	},
});
const port = Number(values.port);
const hostname = values.hostname!;

// Create the MCP server and declare it as a channel
const mcp = new McpServer(
	{ name: "webhook", version: "0.0.1" },
	{
		// this key is what makes it a channel — Claude Code registers a listener for it
		capabilities: { experimental: { "claude/channel": {} } },
		// added to Claude's system prompt so it knows how to handle these events
		instructions:
			'Events from the webhook channel arrive as <channel source="webhook" ...>. They are one-way: read them and act, no reply expected.',
	},
);

// Connect to Claude Code over stdio (Claude Code spawns this process)
await mcp.connect(new StdioServerTransport());

// Start an HTTP server that forwards every POST to Claude
Bun.serve({
	port,
	hostname,
	async fetch(req) {
		const body = await req.text();
		await mcp.server.notification({
			method: "notifications/claude/channel",
			params: {
				content: body, // becomes the body of the <channel> tag
				// each key becomes a tag attribute, e.g. <channel path="/" method="POST">
				meta: { path: new URL(req.url).pathname, method: req.method },
			},
		});
		return new Response("ok");
	},
});

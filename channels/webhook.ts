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
		instructions: `\
Events from the webhook channel arrive as <channel source="webhook" ...>.
They are one-way: read them and act, no reply to the channel is needed.

## Task instruction format

When a task instruction arrives, the channel body is a JSON object:

\`\`\`json
{
  "task_id":     "implement",
  "task":        "Implement the feature",
  "prompt":      "Follow the design doc and implement …",
  "callback_url": "http://127.0.0.1:8789",
  "workflow_id": "4fd261ba-…",
  "outputs":     ["src/**", "tests/**"],
  "deny":        { "files": [".env"] }
}
\`\`\`

## How to handle a task instruction

1. Parse the JSON from the channel body.
2. Read \`prompt\` and carry out the requested work (edit files, run tests, etc.).
3. Respect \`outputs\` (only modify matching paths) and \`deny\` (never touch listed files/commands).
4. When the work is fully complete, report back:

\`\`\`bash
curl -sX POST {callback_url}/complete/{task_id}
\`\`\`

5. Optionally, report progress mid-task:

\`\`\`bash
curl -sX POST {callback_url}/report/{task_id} \\
  -H 'Content-Type: application/json' \\
  -d '{"session_id":"","task_id":"{task_id}","action_index":0,"action_type":"bash","exit_code":0,"stdout":"…"}'
\`\`\`

Do NOT call /complete until all work for that task is truly done.`,
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

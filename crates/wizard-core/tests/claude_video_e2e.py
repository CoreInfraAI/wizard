"""Run a native Claude Code turn with the user skill and MCP installed by Wizard."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
import threading

MODEL = "claude-sonnet-4-6"
TOOL = "mcp__coreinfra_video__video_models"


def content_blocks(request):
    return [b for m in request["messages"] if isinstance(m["content"], list) for b in m["content"]]


def catalog(result):
    assert not result.get("is_error"), result
    blocks = result["content"]
    texts = [blocks] if isinstance(blocks, str) else [b["text"] for b in blocks if b["type"] == "text"]
    for text in texts:
        try:
            # Claude can append a system reminder to the tool output.
            value, _ = json.JSONDecoder().raw_decode(text.lstrip())
        except ValueError:
            continue
        if isinstance(value, dict) and "models" in value:
            return value["models"]
    raise AssertionError("Claude did not deliver the catalog JSON to inference")


class Messages(BaseHTTPRequestHandler):
    """Ask for the skill and catalog. Only the native agent executes those tools."""
    def do_POST(self):
        path = self.path.split("?")[0]
        req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if path == "/v1/messages/count_tokens":
            self.reply("application/json", b'{"input_tokens":12}')
            return
        if path != "/v1/messages":
            self.send_error(404)
            return
        tools = {t["name"] for t in req.get("tools", [])}
        result = next((b for b in content_blocks(req) if b.get("tool_use_id") == "toolu_catalog"), None)
        block = {"type": "text", "text": "Requested skill or MCP tool unavailable"}
        try:
            if result:
                self.server.models = catalog(result)
                assert self.server.models and all(isinstance(m["id"], str) and m["id"].strip()
                                                  for m in self.server.models)
                block["text"] = self.server.models[0]["id"]
            elif self.server.skill_line in json.dumps(req["messages"], ensure_ascii=False) and TOOL in tools:
                block = {"type": "tool_use", "id": "toolu_catalog", "name": TOOL, "input": {}}
            elif "Skill" in tools and not any(b.get("id") == "toolu_skill" for b in content_blocks(req)):
                block = {"type": "tool_use", "id": "toolu_skill", "name": "Skill", "input": {"skill": "coreinfra-video"}}
        except (AssertionError, KeyError, TypeError, ValueError) as error:
            self.server.failure = str(error) or "Invalid catalog"
        self.stream(block)

    def stream(self, block):
        message = {"id": "msg_test", "type": "message", "role": "assistant", "model": MODEL,
                   "content": [], "stop_reason": None, "stop_sequence": None,
                   "usage": {"input_tokens": 12, "output_tokens": 0}}
        if block["type"] == "tool_use":
            start, delta = {**block, "input": {}}, {"type": "input_json_delta", "partial_json": json.dumps(block["input"])}
            reason = "tool_use"
        else:
            start, delta = {"type": "text", "text": ""}, {"type": "text_delta", "text": block["text"]}
            reason = "end_turn"
        events = [
            {"type": "message_start", "message": message},
            {"type": "content_block_start", "index": 0, "content_block": start},
            {"type": "content_block_delta", "index": 0, "delta": delta},
            {"type": "content_block_stop", "index": 0},
            {"type": "message_delta", "delta": {"stop_reason": reason, "stop_sequence": None}, "usage": {"output_tokens": 8}},
            {"type": "message_stop"},
        ]
        self.reply("text/event-stream", "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in events).encode())

    def reply(self, mime, data):
        self.send_response(200)
        self.send_header("Content-Type", mime)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_):
        pass


def run(profile, server):
    skill = profile / "skills/coreinfra-video/SKILL.md"
    source = Path(__file__).resolve().parents[3] / "plugins/coreinfra-video/skills/coreinfra-video/SKILL.md"
    assert skill.read_text(encoding="utf-8") == source.read_text(encoding="utf-8")
    server.skill_line = next(line for line in skill.read_text(encoding="utf-8").splitlines() if line.startswith("Use the "))
    server.models, server.failure = [], None
    # Replace only chat inference. Claude loads the installed skill and user MCP from its normal profile.
    settings_file = profile / "settings.json"
    settings = json.loads(settings_file.read_text(encoding="utf-8"))
    assert settings["env"]["ANTHROPIC_BASE_URL"] == "https://hub.coreinfra.ai/claude/api"
    settings["env"]["ANTHROPIC_BASE_URL"] = f"http://127.0.0.1:{server.server_port}"
    settings_file.write_text(json.dumps(settings), encoding="utf-8")
    env = {k: v for k, v in os.environ.items() if not k.startswith(("ANTHROPIC_", "CLAUDE", "COREINFRA_", "WIZARD_"))}
    home = profile.parent / "home"
    home.mkdir()
    env.update(HOME=str(home), USERPROFILE=str(home), CLAUDE_CONFIG_DIR=str(profile),
               CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC="1", ENABLE_TOOL_SEARCH="false")
    # Limit this catalog test to the two tools that the deterministic Messages provider requests.
    command = [os.environ["WIZARD_VIDEO_TEST_CLAUDE"], "-p", "--no-session-persistence", "--model", MODEL,
               "--tools", "Skill", "--allowedTools", TOOL, "--permission-mode", "dontAsk",
               "--verbose", "--output-format", "stream-json", "--", "Какие видеомодели доступны в CoreInfra?"]
    reply = subprocess.run(command, env=env, cwd=profile.parent, capture_output=True, text=True, encoding="utf-8", timeout=90)
    assert reply.returncode == 0, (reply.stderr + reply.stdout)[-2000:]
    events = [json.loads(line) for line in reply.stdout.splitlines()]
    init = next(event for event in events if event.get("subtype") == "init")
    assert "coreinfra-video" in init["skills"] and TOOL in init["tools"], "Installed skill or MCP missing in native Claude"
    calls = [b["name"] for e in events if e["type"] == "assistant" for b in e["message"]["content"] if b["type"] == "tool_use"]
    assert calls == ["Skill", TOOL], calls
    assert server.failure is None, server.failure
    assert server.models and server.models[0]["id"] in events[-1]["result"], "Empty catalog or missing model ID in reply"
    print(f"Claude Code loaded /coreinfra-video and received {len(server.models)} video models.")


if __name__ == "__main__":
    with ThreadingHTTPServer(("127.0.0.1", 0), Messages) as provider:
        worker = threading.Thread(target=provider.serve_forever, daemon=True)
        worker.start()
        try:
            run(Path(sys.argv[1]).resolve(), provider)
        finally:
            provider.shutdown()
            worker.join()

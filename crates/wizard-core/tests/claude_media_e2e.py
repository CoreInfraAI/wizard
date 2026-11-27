"""Run a native Claude Code turn with the user media skills and MCP installed by Wizard."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
import threading

MODEL = "claude-sonnet-4-6"
# Each skill is loaded before its read-only catalog tool, one per inference round-trip.
MEDIA = [("video", "coreinfra-video", "video_models"), ("speech", "coreinfra-speech", "speech_models"),
         ("transcription", "coreinfra-transcription", "transcription_models"),
         ("image", "coreinfra-image", "image_models")]
TOOLS = ["mcp__coreinfra_media__" + tool for _, _, tool in MEDIA]


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
    """Ask for each skill and its catalog. Only the native agent executes those tools."""
    def do_POST(self):
        path = self.path.split("?")[0]
        req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if path == "/v1/messages/count_tokens":
            self.reply("application/json", b'{"input_tokens":12}')
            return
        if path != "/v1/messages":
            self.send_error(404)
            return
        try:
            block = self.next_block(req)
        except (AssertionError, KeyError, TypeError, ValueError) as error:
            self.server.failure = str(error) or "Invalid catalog"
            block = {"type": "text", "text": "Invalid catalog"}
        self.stream(block)

    def next_block(self, req):
        tools = {t["name"] for t in req.get("tools", [])}
        blocks = content_blocks(req)
        used = {b["id"] for b in blocks if b.get("type") == "tool_use"}
        results = {b["tool_use_id"]: b for b in blocks if b.get("type") == "tool_result"}
        messages = json.dumps(req["messages"], ensure_ascii=False)
        unavailable = {"type": "text", "text": "Requested skill or MCP tool unavailable"}
        for (key, skill, tool), name in zip(MEDIA, TOOLS):
            if "toolu_skill_" + key not in used:
                if "Skill" not in tools:
                    return unavailable
                return {"type": "tool_use", "id": "toolu_skill_" + key, "name": "Skill", "input": {"skill": skill}}
            if "toolu_" + key not in used:
                # The skill body, not just its listed description, must have reached inference.
                if self.server.skill_lines[skill] not in messages or name not in tools:
                    return unavailable
                return {"type": "tool_use", "id": "toolu_" + key, "name": name, "input": {}}
            models = catalog(results["toolu_" + key])
            assert models and all(isinstance(m["id"], str) and m["id"].strip() for m in models), tool
            self.server.catalogs[tool] = models
        return {"type": "text", "text": "\n".join(f"{tool}: {self.server.catalogs[tool][0]['id']}"
                                                  for _, _, tool in MEDIA)}

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
    server.skill_lines = {}
    for _, skill, tool in MEDIA:
        installed = profile / "skills" / skill / "SKILL.md"
        source = Path(__file__).resolve().parents[3] / "plugins/coreinfra-media/skills" / skill / "SKILL.md"
        text = installed.read_text(encoding="utf-8")
        assert text == source.read_text(encoding="utf-8"), f"{skill} differs from the bundled plugin skill"
        server.skill_lines[skill] = next(line for line in text.splitlines() if line.startswith(f"- Call `{tool}`"))
    server.catalogs, server.failure = {}, None
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
    # Limit this catalog test to the tools that the deterministic Messages provider requests.
    command = [os.environ["WIZARD_VIDEO_TEST_CLAUDE"], "-p", "--no-session-persistence", "--model", MODEL,
               "--tools", "Skill", "--allowedTools", ",".join(TOOLS), "--permission-mode", "dontAsk",
               "--verbose", "--output-format", "stream-json", "--",
               "Какие модели для видео, озвучки, распознавания речи и генерации изображений доступны в CoreInfra?"]
    reply = subprocess.run(command, env=env, cwd=profile.parent, capture_output=True, text=True, encoding="utf-8", timeout=90)
    assert reply.returncode == 0, (reply.stderr + reply.stdout)[-2000:]
    events = [json.loads(line) for line in reply.stdout.splitlines()]
    init = next(event for event in events if event.get("subtype") == "init")
    missing = [s for _, s, _ in MEDIA if s not in init["skills"]] + [t for t in TOOLS if t not in init["tools"]]
    assert not missing, f"Installed skills or MCP tools missing in native Claude: {missing}"
    calls = [b["name"] for e in events if e["type"] == "assistant" for b in e["message"]["content"] if b["type"] == "tool_use"]
    assert calls == [name for tool in TOOLS for name in ("Skill", tool)], calls
    assert server.failure is None, server.failure
    assert set(server.catalogs) == {tool for _, _, tool in MEDIA}, server.catalogs.keys()
    for tool, models in server.catalogs.items():
        assert models[0]["id"] in events[-1]["result"], f"Missing {tool} ID in reply"
    counts = ", ".join(f"{len(models)} from {tool}" for tool, models in server.catalogs.items())
    print(f"Claude Code loaded {len(MEDIA)} media skills and received {counts}.")

if __name__ == "__main__":
    with ThreadingHTTPServer(("127.0.0.1", 0), Messages) as provider:
        worker = threading.Thread(target=provider.serve_forever, daemon=True)
        worker.start()
        try:
            run(Path(sys.argv[1]).resolve(), provider)
        finally:
            provider.shutdown()
            worker.join()

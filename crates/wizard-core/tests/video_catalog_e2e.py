"""Run one Codex turn against Wizard's installed skill and the public Hub catalog."""
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
from pathlib import Path
import sys
import threading
import time

from video_clients import codex_session, receive


class CatalogTurn(BaseHTTPRequestHandler):
    """A deterministic Responses provider. Codex executes the tool and sends its output back."""

    def do_POST(self):
        if self.path != "/v1/responses":
            self.send_error(404)
            return
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        output = next((item for item in request["input"] if item.get("type") == "function_call_output"
                       and item.get("call_id") == "call_catalog"), None)
        offered = any(tool.get("name") == "mcp__coreinfra_video" and
                      any(f["name"] == "video_models" for f in tool.get("tools", []))
                      for tool in request["tools"])
        if output is None and offered:
            item = {"type": "function_call", "id": "fc_catalog", "call_id": "call_catalog",
                    "namespace": "mcp__coreinfra_video", "name": "video_models", "arguments": "{}"}
        else:
            # Echo only what Codex delivered to the model on its next inference request.
            text = json.dumps(output["output"], ensure_ascii=False) if output else "video_models unavailable"
            item = {"type": "message", "id": "msg_catalog", "role": "assistant",
                    "content": [{"type": "output_text", "text": text}]}
        response = {"id": "resp_" + item["id"], "status": "completed", "output": [item]}
        events = [
            {"type": "response.created", "response": {"id": response["id"], "status": "in_progress"}},
            {"type": "response.output_item.done", "output_index": 0, "item": item},
            {"type": "response.completed", "response": response},
        ]
        data = "".join("data: " + json.dumps(event) + "\n\n" for event in events).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_):
        pass


def run(profile, model):
    with codex_session(profile, f"http://127.0.0.1:{model.server_port}/v1") as (call, events):
        cwd = str(profile.parent)
        thread = call("thread/start", {"cwd": cwd, "ephemeral": True, "sandbox": "read-only",
                                       "approvalPolicy": "never"})["thread"]["id"]
        installed = profile / "skills" / "coreinfra-video" / "SKILL.md"
        groups = call("skills/list", {"cwds": [cwd]})["data"]
        skills = [s for group in groups for s in group["skills"]
                  if s["name"] == "coreinfra-video" and s["enabled"] and Path(s["path"]).samefile(installed)]
        assert len(skills) == 1, "Codex must discover the enabled skill in the isolated profile"
        skill = skills[0]
        dependencies = (skill.get("dependencies") or {}).get("tools", [])
        assert any(d["type"] == "mcp" and d["value"] == "coreinfra_video" for d in dependencies), (
            "Codex did not read the bundled MCP dependency from agents/openai.yaml"
        )
        turn = call("turn/start", {"threadId": thread, "input": [
            {"type": "skill", "name": skill["name"], "path": skill["path"]},
            {"type": "text", "text": "List the video models available in CoreInfra Hub."},
        ]})["turn"]["id"]

        models = []
        answer = ""
        deadline = time.monotonic() + 85
        while time.monotonic() < deadline:
            message = receive(events, deadline - time.monotonic(), "Codex catalog turn")
            params = message.get("params", {})
            if params.get("threadId") != thread:
                continue
            if message.get("method") == "item/completed" and params.get("turnId") == turn:
                item = params["item"]
                if item["type"] == "mcpToolCall":
                    assert (item["server"], item["tool"]) == ("coreinfra_video", "video_models"), item
                    assert item["status"] == "completed" and not item.get("error"), item
                    models = item["result"]["structuredContent"]["models"]
                elif item["type"] == "agentMessage":
                    answer = item["text"]
            if message.get("method") == "turn/completed" and params["turn"]["id"] == turn:
                assert params["turn"]["status"] == "completed", params["turn"].get("error")
                break
        else:
            raise TimeoutError("Codex catalog turn")

        assert isinstance(models, list) and models, "Codex received an empty video catalog"
        assert all(isinstance(m.get("id"), str) and m["id"].strip() for m in models), "Missing video model ID"
        assert models[0]["id"] in answer, "The final agent reply did not contain a returned model ID"
        print(f"Codex discovered {skill['name']} and received {len(models)} video models.")


if __name__ == "__main__":
    with HTTPServer(("127.0.0.1", 0), CatalogTurn) as provider:
        worker = threading.Thread(target=provider.serve_forever, daemon=True)
        worker.start()
        try:
            run(Path(sys.argv[1]).resolve(), provider)
        finally:
            provider.shutdown()
            worker.join()

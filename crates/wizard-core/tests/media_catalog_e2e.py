"""Run one Codex turn against Wizard's installed media skills and the public Hub catalogs."""
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
from pathlib import Path
import sys
import threading
import time

from media_clients import MEDIA_SKILLS, codex_session, receive

SERVER = "coreinfra_media"
NAMESPACE = "mcp__" + SERVER
# Read-only catalog tools requested one per inference round-trip, keyed by distinct call IDs.
CATALOGS = [("call_video", "video_models"), ("call_speech", "speech_models"),
            ("call_transcription", "transcription_models")]


class CatalogTurn(BaseHTTPRequestHandler):
    """A deterministic Responses provider. Codex executes the tool and sends its output back."""

    def do_POST(self):
        if self.path != "/v1/responses":
            self.send_error(404)
            return
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        outputs = {item["call_id"]: item["output"] for item in request["input"]
                   if item.get("type") == "function_call_output"}
        offered = {f["name"] for tool in request["tools"] if tool.get("name") == NAMESPACE
                   for f in tool.get("tools", [])}
        pending = next(((call_id, name) for call_id, name in CATALOGS if call_id not in outputs), None)
        if pending and pending[1] in offered:
            call_id, name = pending
            item = {"type": "function_call", "id": "fc_" + call_id, "call_id": call_id,
                    "namespace": NAMESPACE, "name": name, "arguments": "{}"}
        else:
            # Echo only what Codex delivered to the model on its later inference requests.
            text = "\n".join(f"{name}: " + (json.dumps(outputs[call_id], ensure_ascii=False)
                                            if call_id in outputs else "unavailable")
                             for call_id, name in CATALOGS)
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
        groups = call("skills/list", {"cwds": [cwd]})["data"]
        skills = {}
        for name in sorted(MEDIA_SKILLS):
            installed = profile / "skills" / name / "SKILL.md"
            found = [s for group in groups for s in group["skills"]
                     if s["name"] == name and s["enabled"] and Path(s["path"]).samefile(installed)]
            assert len(found) == 1, f"Codex must discover the enabled {name} skill in the isolated profile"
            dependencies = (found[0].get("dependencies") or {}).get("tools", [])
            assert any(d["type"] == "mcp" and d["value"] == SERVER for d in dependencies), (
                f"Codex did not read the bundled MCP dependency from {name}/agents/openai.yaml"
            )
            skills[name] = found[0]
        turn = call("turn/start", {"threadId": thread, "input": [
            *({"type": "skill", "name": s["name"], "path": s["path"]} for s in skills.values()),
            {"type": "text", "text": "List the video, speech and transcription models available in CoreInfra Hub."},
        ]})["turn"]["id"]

        catalogs = {}
        calls = []
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
                    assert item["server"] == SERVER and item["tool"] in dict(CATALOGS).values(), item
                    assert item["status"] == "completed" and not item.get("error"), item
                    calls.append(item["tool"])
                    catalogs[item["tool"]] = item["result"]["structuredContent"]["models"]
                elif item["type"] == "agentMessage":
                    answer = item["text"]
            if message.get("method") == "turn/completed" and params["turn"]["id"] == turn:
                assert params["turn"]["status"] == "completed", params["turn"].get("error")
                break
        else:
            raise TimeoutError("Codex catalog turn")

        assert calls == [name for _, name in CATALOGS], calls
        for name, models in catalogs.items():
            assert isinstance(models, list) and models, f"Codex received an empty {name} catalog"
            assert all(isinstance(m.get("id"), str) and m["id"].strip() for m in models), f"Missing {name} ID"
            assert models[0]["id"] in answer, f"The final agent reply did not contain a {name} ID"
        counts = ", ".join(f"{len(models)} from {name}" for name, models in catalogs.items())
        print(f"Codex discovered {len(skills)} media skills and received {counts}.")

if __name__ == "__main__":
    with HTTPServer(("127.0.0.1", 0), CatalogTurn) as provider:
        worker = threading.Thread(target=provider.serve_forever, daemon=True)
        worker.start()
        try:
            run(Path(sys.argv[1]).resolve(), provider)
        finally:
            provider.shutdown()
            worker.join()

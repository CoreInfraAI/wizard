"""Opt-in compatibility test, launched by the Rust native_and_python_clients fixture."""
import asyncio
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading
import time
import tomllib

import httpx2
from mcp import Client
from mcp.client.streamable_http import streamable_http_client


MP4 = (Path(__file__).parent / "fixtures/blue.mp4").read_bytes()


def native_client(profile):
    config = profile / "config.toml"
    with config.open("a") as file:
        file.write('\n[model_providers.fixture]\nname="Offline fixture"\n'
                   'base_url="http://127.0.0.1:1/v1"\nwire_api="responses"\n')
    env = {k: os.environ[k] for k in ("PATH", "HOME", "TMPDIR", "LANG", "USER") if k in os.environ}
    env.update(CODEX_HOME=str(profile), HTTP_PROXY="http://127.0.0.1:1",
               HTTPS_PROXY="http://127.0.0.1:1", ALL_PROXY="http://127.0.0.1:1",
               NO_PROXY="127.0.0.1,localhost", WIZARD_VIDEO_TEST_FILTER="fixture")
    binary = os.environ["WIZARD_VIDEO_TEST_CODEX"]
    server = subprocess.Popen([binary, "app-server", "--listen", "stdio://",
                               "-c", 'model_provider="fixture"', "-c", 'model="offline"'],
                              cwd=profile.parent, env=env, stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)
    replies = queue.Queue()
    def read():
        for line in server.stdout:
            replies.put(json.loads(line))
        replies.put(None)
    threading.Thread(target=read, daemon=True).start()
    def send(message):
        server.stdin.write(json.dumps(message) + "\n")
        server.stdin.flush()
    seq = 0
    def call(method, params):
        nonlocal seq
        seq += 1
        send({"jsonrpc":"2.0", "id":seq, "method":method, "params":params})
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            msg = replies.get(timeout=max(.01, deadline - time.monotonic()))
            if msg is None:
                raise RuntimeError("test app-server exited")
            if msg.get("id") == seq and "method" not in msg:
                assert "error" not in msg, (method, msg.get("error"))
                return msg["result"]
        raise TimeoutError(method)
    try:
        call("initialize", {"clientInfo":{"name":"wizard_fixture","version":"1"},
                            "capabilities":{"experimentalApi":True}})
        send({"jsonrpc":"2.0","method":"initialized"})
        thread = call("thread/start", {"cwd":str(profile.parent),"ephemeral":True,"sandbox":"workspace-write"})["thread"]["id"]
        skills = call("skills/list", {"cwds":[str(profile.parent)],"forceReload":True})
        assert any(s["name"] == "coreinfra-video" for group in skills["data"] for s in group["skills"])
        inventory = call("mcpServerStatus/list", {"threadId":thread,"serverName":"coreinfra_video"})
        tools = next(s["tools"] for s in inventory["data"] if s["name"] == "coreinfra_video")
        assert set(tools) == {"video_models","video_create","video_result"}
        shell = call("command/exec", {"command":[sys.executable,"-I","-c",
                     "import os; print('WIZARD_VIDEO_TEST_FILTER' in os.environ)"],
                     "cwd":str(profile.parent), "sandboxPolicy":{"type":"workspaceWrite","networkAccess":False}})
        assert shell["exitCode"] == 0 and shell["stdout"].strip() == "False", shell
        result = call("mcpServer/tool/call", {"threadId":thread,"server":"coreinfra_video",
                      "tool":"video_create","arguments":{"model":"example/cinema","prompt":"native client",
                      "fields":{"provider":{"options":{"test":{"seed":0,"flag":False,"values":[None,2.75]}}}}}})
        assert not result.get("isError"), result
        return result["structuredContent"]["job"]
    finally:
        server.stdin.close()
        try:
            server.wait(timeout=4)
        except subprocess.TimeoutExpired:
            server.terminate()
            server.wait(timeout=4)


async def python_client(profile, existing_job):
    with (profile / "config.toml").open("rb") as file:
        local = tomllib.load(file)["mcp_servers"]["coreinfra_video"]
    async with httpx2.AsyncClient(headers=local["http_headers"], trust_env=False) as http:
        async with Client(streamable_http_client(local["url"], http_client=http)) as wizard:
            assert len((await wizard.list_tools()).tools) == 3
            models = await wizard.call_tool("video_models", {})
            assert len(models.structured_content["models"]) == 2
            for _ in range(2):
                result = await wizard.call_tool("video_result", {"job": existing_job, "index": 1})
            assert Path(result.structured_content["path"]).read_bytes() == MP4
            submitted = await wizard.call_tool("video_create", {
                "model": "example/flux/image-to-video", "prompt": "python client",
                "fields_json": json.dumps({"image_url":"data:image/png;base64,AAAA","extra":[False,None,3]})})
            job = submitted.structured_content["job"]
        async with Client(streamable_http_client(local["url"], http_client=http)) as resumed:
            final = await resumed.call_tool("video_result", {"job":job})
            assert Path(final.structured_content["path"]).read_bytes() == MP4


if __name__ == "__main__":
    test_profile = Path(sys.argv[1]).resolve()
    existing = native_client(test_profile)
    asyncio.run(python_client(test_profile, existing))
    print("Native ChatGPT skill, existing shell policy, MCP tools and Python SDK round trip passed.")

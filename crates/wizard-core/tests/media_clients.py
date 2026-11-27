"""Codex app-server test client and the opt-in Python MCP compatibility test."""
import asyncio
from contextlib import contextmanager
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading

MEDIA_SKILLS = {"coreinfra-video", "coreinfra-speech", "coreinfra-transcription", "coreinfra-image"}
MEDIA_TOOLS = {"video_models", "video_create", "video_result", "speech_models", "speech_create",
               "transcription_models", "transcription_create", "image_models", "image_create"}


def receive(channel, timeout, operation):
    try:
        message = channel.get(timeout=max(0, timeout))
    except queue.Empty:
        raise TimeoutError(f"Timed out waiting for {operation}") from None
    if message is None:
        raise RuntimeError(f"Codex exited during {operation}")
    return message


@contextmanager
def codex_session(profile, model_url="http://127.0.0.1:1/v1", extra_env=None):
    config = profile / "config.toml"
    with config.open("a", encoding="utf-8") as file:
        file.write('\n[model_providers.fixture]\nname="Offline fixture"\n'
                   f'base_url="{model_url}"\nwire_api="responses"\n')
    system_vars = ("PATH", "HOME", "TMPDIR", "LANG", "USER", "SYSTEMROOT", "WINDIR",
                   "SYSTEMDRIVE", "USERPROFILE", "LOCALAPPDATA", "APPDATA", "PROGRAMDATA",
                   "COMSPEC", "PATHEXT", "TEMP", "TMP")
    env = {k: os.environ[k] for k in system_vars if k in os.environ}
    env.update(CODEX_HOME=str(profile), HTTP_PROXY="http://127.0.0.1:1",
               HTTPS_PROXY="http://127.0.0.1:1", ALL_PROXY="http://127.0.0.1:1",
               NO_PROXY="127.0.0.1,localhost")
    env.update(extra_env or {})
    binary = os.environ["WIZARD_VIDEO_TEST_CODEX"]
    launcher = ["node", binary] if binary.endswith(".js") else [binary]
    server = subprocess.Popen([*launcher, "app-server", "--listen", "stdio://",
                               "-c", 'model_provider="fixture"', "-c", 'model="offline"'],
                              cwd=profile.parent, env=env, stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, stderr=sys.stderr, encoding="utf-8", bufsize=1)
    replies = queue.Queue()
    events = queue.Queue()
    def read():
        try:
            for line in server.stdout:
                message = json.loads(line)
                (events if "method" in message else replies).put(message)
        finally:
            replies.put(None)
            events.put(None)
    threading.Thread(target=read, daemon=True).start()
    def send(message):
        server.stdin.write(json.dumps(message) + "\n")
        server.stdin.flush()
    seq = 0
    def call(method, params):
        nonlocal seq
        seq += 1
        send({"jsonrpc":"2.0", "id":seq, "method":method, "params":params})
        msg = receive(replies, 30, method)
        assert msg["id"] == seq and "error" not in msg, (method, msg.get("error"))
        return msg["result"]
    try:
        call("initialize", {"clientInfo":{"name":"wizard_fixture","version":"1"},
                            "capabilities":{"experimentalApi":True}})
        send({"jsonrpc":"2.0","method":"initialized"})
        yield call, events
    finally:
        server.stdin.close()
        try:
            server.wait(timeout=4)
        except subprocess.TimeoutExpired:
            server.terminate()
            server.wait(timeout=4)


def native_client(profile):
    shell_env = {"WIZARD_VIDEO_TEST_FILTER": "fixture", "COREINFRA_API_KEY": "unrelated-process-fixture-key"}
    with codex_session(profile, extra_env=shell_env) as (call, _):
        options = {"cwd":str(profile.parent),"ephemeral":True,"sandbox":"workspace-write"}
        old_thread = call("thread/start", options)["thread"]["id"]
        before = call("mcpServerStatus/list", {"threadId":old_thread})
        assert all(s["name"] != "coreinfra_media" for s in before["data"])
        skill_query = {"cwds":[str(profile.parent)]}
        previous_skills = call("skills/list", skill_query)
        assert not any(s["name"] in MEDIA_SKILLS for group in previous_skills["data"] for s in group["skills"])
        print("codex-ready", flush=True)
        assert sys.stdin.readline() == "installed\n"
        thread = call("thread/start", options)["thread"]["id"]
        skills = call("skills/list", skill_query)
        enabled = {s["name"] for group in skills["data"] for s in group["skills"] if s["enabled"]}
        assert MEDIA_SKILLS <= enabled, enabled
        inventory = call("mcpServerStatus/list", {"threadId":thread,"serverName":"coreinfra_media"})
        tools = next(s["tools"] for s in inventory["data"] if s["name"] == "coreinfra_media")
        assert set(tools) == MEDIA_TOOLS, tools
        shell = call("command/exec", {"command":[sys.executable,"-I","-c",
                     "import os; print(any(k in os.environ for k in ('WIZARD_VIDEO_TEST_FILTER', 'COREINFRA_API_KEY')))"],
                     "cwd":str(profile.parent), "sandboxPolicy":{"type":"workspaceWrite","networkAccess":False}})
        assert shell["exitCode"] == 0 and shell["stdout"].strip() == "False", shell
        result = call("mcpServer/tool/call", {"threadId":thread,"server":"coreinfra_media",
                      "tool":"video_create","arguments":{"model":"example/cinema","prompt":"native client",
                      "fields":{"provider":{"options":{"test":{"seed":0,"flag":False,"values":[None,2.75]}}}}}})
        assert not result.get("isError"), result
        return result["structuredContent"]["job"]


async def python_client(profile, existing_job):
    import base64
    import tomllib
    import httpx2
    from mcp import Client
    from mcp.client.streamable_http import streamable_http_client

    mp4 = (Path(__file__).parent / "fixtures/blue.mp4").read_bytes()
    with (profile / "config.toml").open("rb") as file:
        local = tomllib.load(file)["mcp_servers"]["coreinfra_media"]
    async with httpx2.AsyncClient(headers=local["http_headers"], trust_env=False) as http:
        async with Client(streamable_http_client(local["url"], http_client=http)) as wizard:
            assert {t.name for t in (await wizard.list_tools()).tools} == MEDIA_TOOLS
            models = await wizard.call_tool("video_models", {})
            assert len(models.structured_content["models"]) == 2
            voices = await wizard.call_tool("speech_models", {})
            assert not voices.is_error, voices
            speech = voices.structured_content["models"]
            assert len(speech) == 2 and speech[0]["id"] == "example/voice" and speech[0]["voice_count"] == 3, speech
            spoken = await wizard.call_tool("speech_create", {"model": "example/voice", "input": "python speech"})
            assert not spoken.is_error and Path(spoken.structured_content["path"]).is_file(), spoken
            scribes = await wizard.call_tool("transcription_models", {})
            assert not scribes.is_error, scribes
            assert [m["id"] for m in scribes.structured_content["models"]] == ["example/scribe"], scribes
            transcript = await wizard.call_tool("transcription_create", {
                "model": "example/scribe",
                "audio_base64": base64.b64encode(b"RIFF fixture wav").decode(),
                "format": "wav",
            })
            assert not transcript.is_error and transcript.structured_content["text"] == "привет", transcript
            painters = await wizard.call_tool("image_models", {})
            assert not painters.is_error, painters
            assert [m["id"] for m in painters.structured_content["models"]] == ["example/paint"], painters
            painted = await wizard.call_tool("image_create", {"model": "example/paint", "prompt": "python image"})
            assert not painted.is_error and Path(painted.structured_content["images"][0]["path"]).is_file(), painted
            for _ in range(2):
                result = await wizard.call_tool("video_result", {"job": existing_job, "index": 1})
            assert Path(result.structured_content["path"]).read_bytes() == mp4
            submitted = await wizard.call_tool("video_create", {
                "model": "example/flux/image-to-video", "prompt": "python client",
                "fields_json": json.dumps({"image_url":"data:image/png;base64,AAAA","extra":[False,None,3]})})
            job = submitted.structured_content["job"]
        async with Client(streamable_http_client(local["url"], http_client=http)) as resumed:
            final = await resumed.call_tool("video_result", {"job":job})
            assert Path(final.structured_content["path"]).read_bytes() == mp4


if __name__ == "__main__":
    test_profile = Path(sys.argv[1]).resolve()
    existing = native_client(test_profile)
    asyncio.run(python_client(test_profile, existing))
    print("Native Codex discovered the installed MCP in a new chat, applied the shell policy and shared jobs with the Python SDK.", file=sys.stderr)

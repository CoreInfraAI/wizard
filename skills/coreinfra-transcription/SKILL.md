---
name: coreinfra-transcription
description: Transcribe speech from an audio file through CoreInfra Hub and list transcription models and prices. Use for requests such as «расшифруй запись», «сделай субтитры к аудио» or «переведи голосовое в текст» in a Wizard-connected workspace.
---

Use the tools on the `coreinfra_media` MCP server. Wizard uses the active CoreInfra key of the agent making the request. If the MCP server is unavailable, ask the user to open Wizard and check that `coreinfra-media` is enabled for this agent.

- Call `transcription_models` for current model IDs and public RUB-per-minute rates. A request for models or a quote only calls for a read.
- Wizard never reads audio files from disk for you. Read the audio file with your own tools and pass its bytes as raw base64 in `audio_base64`, without a `data:` prefix. Set `format` to the container: `wav`, `mp3`, `flac`, `m4a`, `ogg`, `webm` or `aac`. The whole request must stay under 32 MiB, so keep the audio under about 24 MiB. Split or compress longer recordings first.
- To transcribe, call `transcription_create` once with `model`, `audio_base64` and `format`. Add `language` (for example `ru`) and a short `prompt` with names or terms when known. The call is paid. If the outcome is uncertain, report it before attempting another paid request.
- The result is the Hub JSON, such as `text`, `usage.cost_rub` and (with `response_format: "verbose_json"`) timestamped segments when supported by the model. If the user asks for subtitles (`srt` or `vtt`), request `verbose_json` and format the returned timestamps yourself.
- The MCP object `fields` is merged into the **top level** of the upstream JSON body, for example `{"temperature":0}` when supported by the model. For a client with fixed object schemas, pass the same object encoded as a string in `fields_json` instead. Supply the named arguments only in their own parameters.

[OpenRouter transcription API](https://coreinfra.ru/docs/reference/openrouter/audio-transcriptions)

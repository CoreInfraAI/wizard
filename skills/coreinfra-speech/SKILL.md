---
name: coreinfra-speech
description: Turn text into speech through CoreInfra Hub, list speech models, voices and prices, and save the audio locally. Use for requests such as «озвучь этот текст», «сделай аудио из статьи» or «какие голоса есть» in a Wizard-connected workspace.
---

Use the tools on the `coreinfra_media` MCP server. Wizard uses the active CoreInfra key of the agent making the request. If the MCP server is unavailable, ask the user to open Wizard and check that `coreinfra-media` is enabled for this agent.

- Call `speech_models` without arguments for current model IDs, output formats, voice counts, speed ranges, supported parameters and public RUB per 1,000 characters. A request for models, voices or a quote only calls for a read.
- Before `speech_create`, call `speech_models` with `model` set to the exact `author/slug`. Pick `voice`, `response_format` and `speed` from its `supported_parameters` only. Prefer `mp3` when the model supports it. `pcm` is raw headerless audio that most players cannot open.
- To synthesize a requested text, call `speech_create` once with `model`, `input` (or `turns`: `[{"text","voice","instructions"?}]` when the model's `input_modalities` support multi-speaker dialogue) and the chosen `voice`, `response_format` or `speed`. The call is paid and not idempotent. If the outcome is uncertain, report it instead of retrying. A second submission can incur another charge.
- Wizard saves the audio to **Downloads/CoreInfra Media/speech-<random>.<ext>** and returns `{path, content_type, size_bytes}`. Present the returned absolute `path` as audio. The output location cannot be chosen. Copy the file afterwards if the user needs it elsewhere.
- The MCP object `fields` is merged into the **top level** of the upstream `/audio/speech` JSON body, for example `{"instructions":"Говори спокойно и медленно"}` or `{"provider":{"options":{"<provider-slug>":{"<option>":true}}}}` when supported by the model. For a client with fixed object schemas, pass the same object encoded as a string in `fields_json` instead. Supply `model`, `input`, `voice`, `response_format` and `speed` only in their named arguments. Streaming (`stream`, `stream_format`) is not available.

[OpenRouter speech API](https://coreinfra.ru/docs/reference/openrouter/audio-speech)

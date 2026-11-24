---
name: coreinfra-speech
description: List speech models, voices and prices, and turn text into an audio file through CoreInfra Wizard. Use for requests such as «озвучь этот текст», «какие есть голоса» or «сделай аудиоверсию».
---

Use the `coreinfra_media` connector configured by CoreInfra Wizard. Wizard supplies the generation key for the selected CoreInfra Hub.

- Call `speech_models` to list current model IDs, formats, voice counts and prices in RUB per 1,000 characters. A catalog or price request only needs this read.
- Before synthesis, call `speech_models` with the exact `model` and choose `voice`, `response_format` and `speed` from its `supported_parameters`. Prefer `mp3` when available. `pcm` is raw headerless audio.
- For a requested voiceover, call `speech_create` once with `model`, `input` (or `turns`: `[{"text","voice","instructions"?}]` when the model's `input_modalities` support multi-speaker dialogue) and the chosen options. Pass other upstream JSON, such as `instructions` or `provider`, as `fields`, or encode that object in `fields_json` if the client needs a fixed schema. Do not repeat named arguments inside it. Streaming is not available.
- The call is paid and not idempotent. If its outcome is uncertain, explain it to the user before attempting another paid request.
- Give the user the returned `path` in **Downloads/CoreInfra Media** on their computer, with its `content_type` and `size_bytes`. The location cannot be chosen.

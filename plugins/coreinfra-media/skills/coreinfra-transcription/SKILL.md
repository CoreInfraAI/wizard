---
name: coreinfra-transcription
description: List transcription models and prices, and transcribe an audio file into text or subtitles through CoreInfra Wizard. Use for requests such as «расшифруй запись», «сделай субтитры» or «что сказано в этом аудио».
---

Use the `coreinfra_media` connector configured by CoreInfra Wizard. Wizard supplies the generation key for the selected CoreInfra Hub.

- Call `transcription_models` to list current model IDs and prices in RUB per minute. A catalog or price request only needs this read.
- Wizard does not read files from disk. Read the audio yourself and pass it as raw base64 in `audio_base64`, with `format` set to its container: `wav`, `mp3`, `flac`, `m4a`, `ogg`, `webm` or `aac`. Keep the audio under about 24 MiB so the request stays below 32 MiB. Split longer recordings.
- For a requested transcription, call `transcription_create` once with `model`, `audio_base64` and `format`. Add `language` and a `prompt` with names or terms when known. Pass other upstream JSON, such as `{"temperature":0}`, as `fields` or `fields_json`.
- The call is paid. If its outcome is uncertain, explain it to the user before attempting another paid request.
- The result is the Hub JSON with `text`, `usage.cost_rub` and (with `response_format: "verbose_json"`) timestamped segments when supported by the model. If the user asks for subtitles (`srt` or `vtt`), request `verbose_json` and format the returned timestamps yourself.

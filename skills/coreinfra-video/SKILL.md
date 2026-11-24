---
name: coreinfra-video
description: Generate video from a prompt or image through CoreInfra Hub, list video models and prices, and retrieve existing video jobs. Use for requests such as «сгенерируй видео», «оживи фото» or «забери готовый ролик» in a Wizard-connected workspace.
---

Use the tools on the `coreinfra_media` MCP server. Wizard uses the active CoreInfra key of the agent making the request. If the MCP server is unavailable, ask the user to open Wizard and check that `coreinfra-media` is enabled for this agent.

- Use `video_models` for current model IDs, providers and public RUB-per-second rates. A request for models, a quote or a prompt only calls for a read.
- To generate a requested video, call `video_create` once with `model`, `prompt` and any model-specific JSON in `fields`. Keep the entire returned `job` object with the conversation or the user's script. If the submission outcome is uncertain, report it. A second submission can incur another charge.
- Call `video_result` with that job. Poll pending or in-progress jobs with a delay. On completion, present the returned absolute `path` (in **Downloads/CoreInfra Media**) as a video. To check status without saving a file, pass `download: false`. For another OpenRouter output, pass its zero-based `index`. After interruption, resume with the same job. If `video_result` reports that the job requires its original key, ask the user to select that key for this agent in Wizard and apply the change. Once they confirm, retry `video_result` with the same job. Do not submit a new job or ask the user to paste the key into chat.
- The MCP object `fields` is merged into the **top level** of the upstream JSON body. Preserve the requested types and provider nesting. For example, OpenRouter accepts `{"duration":5,"provider":{"options":{"<provider-slug>":{"<option>":true}}}}` when supported by the model. fal accepts fields such as `{"duration":5,"generate_audio":false}`. For a client with fixed object schemas, pass the same object encoded as a string in `fields_json` instead. Supply model and prompt only in their named arguments.
- For image input, use an inline `data:image/...;base64,...` URI. For fal image-to-video put it in `fields.image_url`. For OpenRouter use `fields.frame_images` with `{"type":"image_url","image_url":{"url":"<data URI>"},"frame_type":"first_frame"}`. Pass custom body fields only when the selected model supports them.

[OpenRouter video API](https://coreinfra.ru/docs/reference/openrouter/videos) · [fal video API](https://coreinfra.ru/docs/use/fal)

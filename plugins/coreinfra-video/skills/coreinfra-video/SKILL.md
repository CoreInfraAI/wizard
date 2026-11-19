---
name: coreinfra-video
description: List video models and prices, generate a video from text or an image, and retrieve an existing video through CoreInfra Wizard. Use for requests such as «какие есть видеомодели», «сгенерируй видео» or «оживи фото».
---

Use the `coreinfra_video` connector configured by CoreInfra Wizard. Wizard supplies the generation key for the selected CoreInfra Hub.

- Call `video_models` to list current model IDs, providers and prices in RUB per second. A catalog or price request only needs this read.
- For a requested generation, call `video_create` once with an exact model ID and the prompt. Pass provider-specific JSON as `fields`, preserving the types and nesting, or encode that object in `fields_json` if the client needs a fixed schema. Put `model` and `prompt` in their named arguments.
- For image-to-video, send the image as an inline data URI. fal uses `fields.image_url`. OpenRouter uses `fields.frame_images` with an item such as `{"type":"image_url","image_url":{"url":"<data URI>"},"frame_type":"first_frame"}`.
- Keep the entire returned `job`. Poll it with `video_result`, pausing between pending responses. Resume with the same job after interruptions. If the outcome of a submission is uncertain, explain it to the user before attempting another paid generation.
- On completion, give the user the returned path in their computer's Downloads folder. `download: false` checks status without saving a file and can return a signed fal URL. For another OpenRouter output, use its zero-based `index`. If the active Hub changed, select the job's original Hub in Wizard.

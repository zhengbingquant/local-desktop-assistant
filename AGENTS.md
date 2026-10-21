# Repository guidance

- Keep the assistant local-first and useful without an internet connection for local tasks.
- Treat model weights, weight sources, and inference runtimes as replaceable. Do not make Ollama a hard dependency.
- Do not send prompts, files, transcripts, or local context to a cloud service by default. Any future network-backed feature must be explicit and visible to the user.
- Give tools narrow permissions. Start with read-only tools; require a clear user confirmation before deleting, overwriting, sending, purchasing, or making other consequential changes.
- Keep downloaded weights, credentials, machine-specific settings, caches, and user data out of version control.
- Read the project notes before choosing a framework or model-serving interface. Record trade-offs in the docs when making those decisions.
- Do not add or run tests unless the user requests testing.

# Experimental Gemini streaming translation

When `model` begins with `gemini-`, Kilovolt maps OpenAI-shaped messages to
Google's native streaming endpoint and maps supported candidate text back to
OpenAI-shaped SSE.

This path is experimental and supports only `stream=true`. A non-streaming
Gemini request is rejected with structured `400` before a prompt reservation.

```bash
curl --no-buffer \
  -H "Authorization: Bearer ${GEMINI_API_KEY}" \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: trusted-user-42' \
  --data '{
    "model":"gemini-1.5-flash",
    "messages":[{"role":"user","content":"Reply with OK."}],
    "stream":true
  }' \
  http://127.0.0.1:8080/v1/chat/completions
```

Kilovolt extracts the bearer value into `x-goog-api-key`, sends translated
`contents`, converts supported `candidates[0].content.parts[].text`, and emits
`[DONE]` when the upstream ends.

The compiled Gemini price prefixes were not independently verified in this
offline Phase 3 run. Validate current provider pricing and response compatibility
before production. System/tool/multimodal semantics are not claimed.

`X-User-ID` remains a trusted backend assertion and must never be selected by an
untrusted browser or mobile client.

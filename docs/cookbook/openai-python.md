# OpenAI Python backend

## Goal

Send streaming and non-streaming chat completions through Kilovolt from trusted
Python backend code.

## Request flow

```text
Browser -> authenticated Python backend -> trusted X-User-ID -> Kilovolt -> OpenAI
```

## Prerequisites

Python 3.10+, `pip install openai`, a running browser-evaluation Kilovolt
container, and the generated Kilovolt gateway key shown after setup.

## Complete configuration

Start with the [one-command Docker flow](docker-deployment.md), complete browser
setup, then copy the generated key into the trusted backend environment:

```bash
export KILOVOLT_GATEWAY_KEY='generated-kilovolt-gateway-key'
```

## Complete runnable code

```python
import os
from openai import OpenAI

client = OpenAI(
    api_key=os.environ["KILOVOLT_GATEWAY_KEY"],
    base_url="http://127.0.0.1:8080/v1",
)

# Derive this after authenticating the application user.
trusted_user_id = "user_123"
stream = client.chat.completions.create(
    model="gpt-4o-mini",
    messages=[{"role": "user", "content": "Explain Rust ownership briefly."}],
    stream=True,
    extra_headers={"X-User-ID": trusted_user_id},
)
for chunk in stream:
    text = chunk.choices[0].delta.content
    if text:
        print(text, end="", flush=True)
print()

response = client.chat.completions.create(
    model="gpt-4o-mini",
    messages=[{"role": "user", "content": "Reply with OK."}],
    stream=False,
    max_completion_tokens=100,
    extra_headers={"X-User-ID": trusted_user_id},
)
print(response.choices[0].message.content)
```

## Verify it works

Run the script, then open the local dashboard or `/api/stats` and confirm
`user_123` appears under `current_spend_by_user`.

## Expected success behavior

Streaming frames arrive in order. The JSON response remains intact. Prompt and
supported output charges appear in the same project/user ledger.

## Expected budget-block behavior

The SDK raises an API status error for preflight or non-stream output `429`.
Mid-stream cutoff appears as an early stream end.

## Security notes

Never accept a raw `X-User-ID` from the browser. Derive it from the authenticated
session. Keep both the generated gateway key and provider key out of browser
code; Kilovolt holds the provider key only in its evaluation process memory.

## Common failure modes

Use `base_url` ending in `/v1`; browser evaluation accepts OpenAI models only;
verify model-price assumptions. A public HTTP URL leaks credentials without TLS.
Configured manual mode instead uses the provider key as the SDK key and may also
require `X-Kilovolt-Key`; see [proxy authentication](proxy-authentication.md).

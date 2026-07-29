# OpenAI Node.js backend

## Goal

Use the official Node.js client from a trusted server process.

## Request flow

```text
Web client -> authenticated Node backend -> X-User-ID -> Kilovolt -> OpenAI
```

## Prerequisites

Node.js 20+, `npm install openai`, Kilovolt, and `OPENAI_API_KEY`.

## Complete configuration

```bash
export KILOVOLT_PROJECT_BUDGET=100
export KILOVOLT_DEFAULT_BUDGET=5
export KILOVOLT_TELEMETRY_ENABLED=false
export OPENAI_API_KEY='provider-secret'
```

## Complete runnable code

```javascript
import OpenAI from 'openai';

const client = new OpenAI({
  apiKey: process.env.OPENAI_API_KEY,
  baseURL: 'http://127.0.0.1:8080/v1',
});

const trustedUserId = 'user_123'; // derive from the authenticated session
const stream = await client.chat.completions.create(
  {
    model: 'gpt-4o-mini',
    messages: [{ role: 'user', content: 'Give one Rust safety benefit.' }],
    stream: true,
  },
  { headers: { 'X-User-ID': trustedUserId } },
);

for await (const chunk of stream) {
  process.stdout.write(chunk.choices[0]?.delta?.content ?? '');
}
process.stdout.write('\n');
```

## Verify it works

Run `node example.mjs` and check the authenticated customer dashboard for
`user_123`.

## Expected success behavior

The SDK parses the forwarded SSE frames and the local project/user spend
increases only for accepted increments.

## Expected budget-block behavior

A preflight rejection throws a `429` status error. A cutoff after headers ends
the iterator early.

## Security notes

Run this code server-side only. Do not use `dangerouslyAllowBrowser`, and do not
copy an untrusted request header into `X-User-ID`.

## Common failure modes

ESM projects need `"type": "module"` or an `.mjs` file. Missing `Content-Type`
is handled by the SDK; incorrect base URL or private-network routing causes
connection errors.

# Next.js backend-for-frontend

## Goal

Keep provider credentials and the trusted identity assertion in a server-only
Next.js route.

## Request flow

```text
Browser -> authenticated Next.js route -> X-User-ID -> Kilovolt -> provider
```

## Prerequisites

A Next.js server deployment, an existing authenticated session function, and
server-only `KILOVOLT_URL`/`OPENAI_API_KEY` environment variables.

## Complete configuration

```bash
KILOVOLT_URL=http://127.0.0.1:8080
OPENAI_API_KEY=provider-secret
KILOVOLT_PROJECT_BUDGET=100
KILOVOLT_DEFAULT_BUDGET=5
KILOVOLT_TELEMETRY_ENABLED=false
```

## Complete runnable code

The authentication function is application-specific; this example makes that
boundary explicit:

```typescript
// app/api/chat/route.ts
import { NextResponse } from 'next/server';
import { auth } from '@/lib/auth'; // must return a verified server-side user

export async function POST(request: Request) {
  const session = await auth();
  if (!session?.user?.id) {
    return NextResponse.json({ error: 'Unauthorized' }, { status: 401 });
  }

  const prompt = await request.json();
  const upstream = await fetch(
    `${process.env.KILOVOLT_URL}/v1/chat/completions`,
    {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${process.env.OPENAI_API_KEY}`,
        'Content-Type': 'application/json',
        'X-User-ID': session.user.id,
      },
      body: JSON.stringify({ ...prompt, stream: false }),
    },
  );

  return new Response(upstream.body, {
    status: upstream.status,
    headers: {
      'Content-Type':
        upstream.headers.get('Content-Type') ?? 'application/json',
    },
  });
}
```

## Verify it works

An unauthenticated browser request must receive `401`. An authenticated request
must create spend under the server-derived session user ID.

## Expected success behavior

Kilovolt forwards the bounded JSON result only after accounting its supported
output.

## Expected budget-block behavior

The route preserves Kilovolt's `429` and structured body for the browser.

## Security notes

Ignore/remove any browser-provided `X-User-ID`. Keep the provider key and
Kilovolt URL out of `NEXT_PUBLIC_*` variables.

## Common failure modes

Edge runtimes may not reach a private localhost service; deploy both services on
a reachable private network. Streaming requires forwarding headers/body without
buffering and is deliberately omitted from this minimal JSON example.

# Using Google Gemini Models with Kilovolt

Kilovolt includes transparent translation and routing support for Google Gemini models. You can interact with Google's production Gemini endpoints using standard OpenAI client libraries, while retaining full budget controls and token tracking capabilities.

---

## 🔌 How It Works

1. **Routing**: If the `model` name starts with **`gemini-`** (e.g. `gemini-1.5-flash`, `gemini-1.5-pro`), Kilovolt automatically routes the transaction to Google's production endpoint:
   `https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent?alt=sse`
2. **Credential Translation**: You supply your Google Gemini API key in the standard OpenAI `Authorization: Bearer <key>` header. Kilovolt extracts the bearer key and maps it to Google's expected HTTP header: `x-goog-api-key: <key>`.
3. **Payload Mapping**: The proxy parses the standard OpenAI request JSON and structures it into Google's native `contents` layout.
4. **Stream Conversion**: The upstream camelCase Gemini Server-Sent Events (SSE) stream is intercepted and converted on the fly into OpenAI-compatible `choices[0].delta` JSON chunks before being returned to your client.
5. **Draining & Termination**: The proxy detects when the Google stream ends and appends `data: [DONE]\n\n` to ensure standard client libraries disconnect cleanly.

---

## 🧮 Model Pricing Matrix

Gemini requests are billed at the following rates (tracked in real-time by the Bankruptcy Shield):

| Model | Input Cost (per 1M tokens) | Output Cost (per 1M tokens) |
| :--- | :--- | :--- |
| **`gemini-1.5-flash`** | $0.075 | $0.30 |
| **`gemini-1.5-pro`** | $1.25 | $5.00 |
| **Other `gemini-`** | $0.075 | $0.30 (Default Fallback) |

---

## 💻 Code Examples

To target Gemini models, configure your OpenAI SDK client to point to the Kilovolt server address and pass your **Gemini API Key**.

### Python (OpenAI SDK)

```python
import os
from openai import OpenAI

# Initialize client pointing to your local Kilovolt server
client = OpenAI(
    api_key="YOUR_GEMINI_API_KEY", # Standard Gemini key (AIzaSy...)
    base_url="http://localhost:8080/v1"
)

response_stream = client.chat.completions.create(
    model="gemini-1.5-flash",
    messages=[
        {"role": "user", "content": "Write a 3-word slogan for an AI company."}
    ],
    stream=True
)

for chunk in response_stream:
    content = chunk.choices[0].delta.content
    if content:
        print(content, end="", flush=True)
print()
```

### Node.js (OpenAI SDK)

```javascript
import OpenAI from 'openai';

// Initialize client pointing to your local Kilovolt server
const openai = new OpenAI({
  apiKey: 'YOUR_GEMINI_API_KEY', // Standard Gemini key (AIzaSy...)
  baseURL: 'http://localhost:8080/v1'
});

async function main() {
  const stream = await openai.chat.completions.create({
    model: 'gemini-1.5-flash',
    messages: [{ role: 'user', content: 'Write a 3-word slogan for an AI company.' }],
    stream: true,
  });

  for await (const chunk of stream) {
    process.stdout.write(chunk.choices[0]?.delta?.content || '');
  }
  console.log();
}

main();
```

### Raw Curl Request

```bash
curl -i -X POST \
  -H "Authorization: Bearer YOUR_GEMINI_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gemini-1.5-flash",
    "messages": [{"role": "user", "content": "Hello!"}],
    "stream": true
  }' \
  http://localhost:8080/v1/chat/completions
```

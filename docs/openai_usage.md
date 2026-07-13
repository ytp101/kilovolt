# Using OpenAI Models with Kilovolt

Kilovolt acts as a drop-in replacement for standard OpenAI API endpoints. By pointing your OpenAI client libraries to the Kilovolt proxy server, you inherit real-time token tracking and the Bankruptcy Shield circuit breaker.

---

## 🔌 How It Works

1. **Routing**: Any chat completion request with a model name that does not start with `gemini-` (e.g. `gpt-4o`, `gpt-4o-mini`, `gpt-4`) is routed directly to the official OpenAI completions endpoint:
   `https://api.openai.com/v1/chat/completions`
2. **Authentication Pass-Through**: Your OpenAI API key is passed in the standard `Authorization: Bearer <key>` header. Kilovolt forwards it securely to OpenAI's servers.
3. **Identity Tracking**: Pass a custom `X-User-ID` header (e.g. `X-User-ID: developer_bob`) to identify who is making the request. Kilovolt uses this identity to check and update the user's spend ledger.
4. **Mid-stream Token Audit**: The proxy interceptor decodes the streaming chunks (`choices[0].delta.content`) in real-time, counts tokens using local Byte-Pair Encoding (`tiktoken`), and cuts the stream off instantly if the user's spending limit is breached.

---

## 🧮 Model Pricing Matrix

OpenAI requests are billed based on the model's actual rates:

| Model Prefix | Input Cost (per 1M tokens) | Output Cost (per 1M tokens) |
| :--- | :--- | :--- |
| **`gpt-4o`** | $5.00 | $15.00 |
| **`gpt-4o-mini`** | $0.15 | $0.60 |
| **`gpt-4`** | $30.00 | $60.00 |
| **`gpt-3.5-turbo`** | $0.50 | $1.50 |
| **Other models** | $5.00 | $15.00 (Default Fallback) |

---

## 💻 Code Examples

### Python (OpenAI SDK)

```python
import os
from openai import OpenAI

# Initialize the client pointing to the Kilovolt proxy gateway
client = OpenAI(
    api_key=os.environ.get("OPENAI_API_KEY", "your-openai-api-key"),
    base_url="http://localhost:8080/v1"
)

try:
    response = client.chat.completions.create(
        model="gpt-4o",
        messages=[
            {"role": "user", "content": "Explain async streams in 10 words."}
        ],
        stream=True,
        # Custom identity tracking for the Bankruptcy Shield
        extra_headers={"X-User-ID": "developer_bob"} 
    )

    for chunk in response:
        content = chunk.choices[0].delta.content
        if content:
            print(content, end="", flush=True)
    print()
    
except Exception as e:
    # Catches 429 Budget Exceeded and network errors
    print(f"\nAPI Error: {e}")
```

### Node.js (OpenAI SDK)

```javascript
import OpenAI from 'openai';

// Initialize the client pointing to the Kilovolt proxy gateway
const openai = new OpenAI({
  apiKey: process.env.OPENAI_API_KEY || 'your-openai-api-key',
  baseURL: 'http://localhost:8080/v1'
});

async function main() {
  try {
    const stream = await openai.chat.completions.create({
      model: 'gpt-4o-mini',
      messages: [{ role: 'user', content: 'Explain async streams in 10 words.' }],
      stream: true,
    }, {
      // Custom identity tracking for the Bankruptcy Shield
      headers: { 'X-User-ID': 'developer_bob' }
    });

    for await (const chunk of stream) {
      process.stdout.write(chunk.choices[0]?.delta?.content || '');
    }
    console.log();
  } catch (err) {
    console.error('API Error:', err.message);
  }
}

main();
```

### Raw Curl Request

Use the `-N` flag to disable curl's output buffering:

```bash
curl -i -N -X POST http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer YOUR_OPENAI_API_KEY" \
  -H "Content-Type: application/json" \
  -H "X-User-ID: developer_bob" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "Explain async streams in 10 words."}],
    "stream": true
  }'
```

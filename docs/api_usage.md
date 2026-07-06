# Kilovolt (kvlt) - API Integration & Usage Guide

Kilovolt behaves as a drop-in replacement for standard OpenAI-compatible API gateways. By redirecting your client library's base URL to your Kilovolt proxy server and sending a custom identity header (`X-User-ID`), you instantly equip your application with zero-copy stream piping and the Bankruptcy Shield circuit breaker.

---

## 📌 API Reference

### 1. Health Check
* **Route**: `GET /health`
* **Description**: Verifies that the gateway is running and ready to handle traffic.
* **Response**: `200 OK` (Body: `"OK"`)

### 2. Chat Completions
* **Route**: `POST /v1/chat/completions`
* **Description**: Proxies standard chat completion payloads upstream.
* **Headers**:
  * `Authorization`: `Bearer <API_KEY>` (Passed upstream to the target provider).
  * `Content-Type`: `application/json` (Required).
  * `X-User-ID`: `<USER_ID>` (Identity tracking key for the circuit breaker. Defaults to `"anonymous"` if missing).
  * `X-Mock-Upstream`: `true` (Optional: routes the request internally to Kilovolt's mock SSE stream for offline testing).
* **Payload**: Any valid OpenAI-compatible chat completions payload (supports `"stream": true` and `"stream": false`).

---

## 💻 Integration Examples

### 1. Curl

#### Standard Request (No Streaming)
```bash
curl -X POST http://127.0.0.1:8080/v1/chat/completions \
  -H "Authorization: Bearer sk-proj-your-openai-key" \
  -H "Content-Type: application/json" \
  -H "X-User-ID: developer_alice" \
  -d '{
    "model": "gpt-4o",
    "messages": [
      {"role": "user", "content": "Explain async streams in 10 words."}
    ],
    "stream": false
  }'
```

#### Streaming Request (Server-Sent Events)
Use the `-N` flag to disable curl's output buffering, enabling you to inspect the token stream in real-time:
```bash
curl -i -N -X POST http://127.0.0.1:8080/v1/chat/completions \
  -H "Authorization: Bearer sk-proj-your-openai-key" \
  -H "Content-Type: application/json" \
  -H "X-User-ID: developer_alice" \
  -d '{
    "model": "gpt-4o",
    "messages": [
      {"role": "user", "content": "Write a short poem about lightning."}
    ],
    "stream": true
  }'
```

---

### 2. Python (using `openai` SDK)

To integrate Kilovolt into a Python AI application, initialize the `OpenAI` client with a custom `base_url` and pass the `X-User-ID` header in `extra_headers`:

```python
import os
from openai import OpenAI

# Initialize the OpenAI client pointing to the Kilovolt proxy gateway
client = OpenAI(
    api_key=os.environ.get("OPENAI_API_KEY", "your-openai-api-key"),
    base_url="http://127.0.0.1:8080/v1"
)

# Call the API with the Bankruptcy Shield header
try:
    response = client.chat.completions.create(
        model="gpt-4o",
        messages=[
            {"role": "user", "content": "Why is Rust so memory efficient?"}
        ],
        stream=True,
        extra_headers={"X-User-ID": "developer_bob"} # Identifies the user for budget tracking
    )

    for chunk in response:
        content = chunk.choices[0].delta.content
        if content:
            print(content, end="", flush=True)
            
except Exception as e:
    # Captures 429 Budget Exceeded and network errors
    print(f"\nAPI Error: {e}")
```

---

### 3. Node.js / JavaScript (using `@openai/api` SDK)

For Node.js backends or frontend clients:

```javascript
const { OpenAI } = require('openai');

const openai = new OpenAI({
  apiKey: process.env.OPENAI_API_KEY || 'your-openai-api-key',
  baseURL: 'http://127.0.0.1:8080/v1',
});

async function main() {
  try {
    const stream = await openai.chat.completions.create({
      model: 'gpt-4o',
      messages: [{ role: 'user', content: 'What is O(1) memory complexity?' }],
      stream: true,
    }, {
      // Pass the user tracking ID
      headers: {
        'X-User-ID': 'developer_charlie',
      }
    });

    for await (const chunk of stream) {
      process.stdout.write(chunk.choices[0]?.delta?.content || '');
    }
  } catch (err) {
    console.error('\nAPI Failed:', err.message);
  }
}

main();
```

---

### 4. Go (using standard HTTP client)

```go
package main

import (
	"bufio"
	"bytes"
	"fmt"
	"net/http"
)

func main() {
	jsonData := []byte(`{
		"model": "gpt-4o",
		"messages": [{"role": "user", "content": "Hello"}],
		"stream": true
	}`)

	req, err := http.NewRequest("POST", "http://127.0.0.1:8080/v1/chat/completions", bytes.NewBuffer(jsonData))
	if err != nil {
		panic(err)
	}

	req.Header.Set("Authorization", "Bearer your-api-key")
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("X-User-ID", "developer_david")

	client := &http.Client{}
	resp, err := client.Do(req)
	if err != nil {
		panic(err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		fmt.Printf("Error: status %d\n", resp.StatusCode)
		return
	}

	reader := bufio.NewReader(resp.Body)
	for {
		line, err := reader.ReadString('\n')
		if err != nil {
			break
		}
		fmt.Print(line)
	}
}
```

---

### 5. Rust (using `reqwest` and `futures`)

```rust
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new();
    let payload = json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "Hello!"}],
        "stream": true
    });

    let mut response = client
        .post("http://127.0.0.1:8080/v1/chat/completions")
        .header("Authorization", "Bearer your-api-key")
        .header("Content-Type", "application/json")
        .header("X-User-ID", "developer_elena")
        .json(&payload)
        .send()
        .await?;

    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let err_json: serde_json::Value = response.json().await?;
        println!("Circuit Breaker Tripped: {:?}", err_json);
        return Ok(());
    }

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk?;
        print!("{}", String::from_utf8_lossy(&bytes));
    }

    Ok(())
}
```

---

## 🛑 Error & Circuit Breaker Schemas

When Kilovolt blocks a request due to authorization errors, bad headers, or exceeded budgets, it returns standard HTTP error codes formatted to match OpenAI's structured error format.

### 1. Budget Exceeded (`429 Too Many Requests`)
Tripped when the user's total aggregate spend reaches or exceeds the dynamically loaded `KILOVOLT_DEFAULT_BUDGET` variable:
```json
{
  "error": {
    "message": "Budget Exceeded",
    "type": "requests",
    "param": null,
    "code": "budget_exceeded"
  }
}
```

### 2. Unauthorized (`401 Unauthorized`)
Returned if the `Authorization` header is missing, is invalid UTF-8, or does not begin with `"Bearer "`:
```json
{
  "error": {
    "message": "Authorization header must start with 'Bearer '",
    "type": "invalid_request_error",
    "param": null,
    "code": "invalid_api_key"
  }
}
```

### 3. Missing Content-Type (`400 Bad Request`)
Returned if `Content-Type` is missing or is not `application/json`:
```json
{
  "error": {
    "message": "Content-Type must be application/json",
    "type": "invalid_request_error",
    "param": null,
    "code": null
  }
}
```

# Kilovolt (kvlt) ⚡

[![Rust](https://img.shields.io/badge/rust-stable-brightgreen.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**Kilovolt (kvlt)** is a hyper-optimized, high-throughput asynchronous reverse proxy designed to act as a **Bankruptcy Shield** for independent developers, startups, and coding agents running AI applications on low-resource hardware (such as a $5/month VPS). 

It intercepts OpenAI-compatible API traffic on localhost, proxies it upstream to official providers, and pipes the streaming tokens back to client applications with a strictly flat, non-expanding memory footprint ($O(1)$ space complexity) to completely eliminate memory-inflation crashes.

---

## 💡 The Value Proposition

When running high-volume LLM pipelines on small virtual servers, standard gateway integrations can cause major memory leaks and unexpected cloud charges:
1. **OOM Crashes**: Buffering large token streams into memory before sending them downstream quickly consumes VPS heap space, resulting in Out-Of-Memory (OOM) process terminations.
2. **Ghost Billing**: If a downstream client abruptly disconnects mid-stream, many API integrations continue downloading and billing for upstream tokens.
3. **Bankruptcy Shield**: Kilovolt tracks user budgets in memory and trips a low-latency circuit breaker both pre-flight and mid-stream the exact millisecond a user exceeds their limit—preventing runaway costs and fractional overdrafts.

---

## 🚀 Core Features

- **Zero-Copy Stream Piping**: Pipes byte-chunks from OpenAI's SSE (`text/event-stream`) directly to client sockets. The gateway's memory footprint remains flat regardless of the size or duration of the chat stream.
- **Pre-Flight Prompt Validation**: Inspects incoming payloads, calculates prompt tokens using tiktoken BPE, projects the USD cost based on model-specific rates, and rejects requests with `429 Too Many Requests` if the user is over budget.
- **Mid-Stream Circuit Breaker**: Actively parses streaming SSE chunks to count output tokens on the fly. The exact millisecond the cumulative spend exceeds the budget limit, it severs the TCP connection, preventing the fractional overdraft edge case.
- **Upstream Refund Guard**: If an upstream connection fails or returns an error response, the pre-flight prompt cost is automatically refunded to the user's spend ledger.
- **Connection Abortion**: Monitors downstream client sockets. If a client terminates a request early, Kilovolt instantly drops the upstream Reqwest socket, canceling downstream transmission and preventing ghost token costs.
- **Dynamic Configuration**: Supports `.env` loading and system environment variable overrides for quick configuration of listening port and default budgets.

---

## 🔌 Supported Providers & LLM Engines

Kilovolt acts as a zero-copy byte pipeline, meaning it streams payloads and tokens without parsing the main chat JSON structure. This makes it natively compatible with **any hosted provider or local engine using the standard OpenAI-compatible wire format (`/v1/chat/completions`)**:

### Hosted Cloud Providers
- **OpenAI** (Official APIs — Default upstream destination)
- **DeepSeek** (100% OpenAI-compatible endpoints)
- **Groq** / **Together AI** / **OpenRouter** / **Fireworks AI**

### Self-Hosted / Local LLM Engines
- **Ollama** (exposes OpenAI compatible endpoint on port `11434`)
- **vLLM** / **llama.cpp** (built-in server)
- **LM Studio** / **LocalAI**

---

## 🛠️ Configuration Parameters

Kilovolt is configured using environment variables or a `.env` file in the working directory:

| Variable | Default Value | Description |
| :--- | :--- | :--- |
| `KILOVOLT_PORT` | `8080` | The local port the proxy server binds to (e.g. `127.0.0.1:8080`). |
| `KILOVOLT_DEFAULT_BUDGET` | `1.00` | The maximum aggregate dollar spend allowed per user (e.g., `1.00` is $1.00 USD). |
| `RUST_LOG` | `kilovolt=info` | Observability and debug logging levels. |

---

## 📦 Quickstart

To run the Bankruptcy Shield proxy gateway in production with zero dependencies, pull and execute the pre-compiled Docker image from Docker Hub:

### 1. Create a `.env` file
Configure your environment variables in a local `.env` file:
```env
KILOVOLT_PORT=8080
KILOVOLT_DEFAULT_BUDGET=5.00
RUST_LOG=kilovolt=info
```

### 2. Run the Docker Container
Launch the gateway using your environment settings and expose the configured port:
```bash
docker run -d --env-file .env -p 8080:8080 yodsarun/kilovolt-proxy:latest
```

---

## 🔌 API Integration

To route traffic through the Bankruptcy Shield, simply redirect your client's API base URL to Kilovolt and append the custom identity header `X-User-ID`:

```bash
curl -X POST http://127.0.0.1:8080/v1/chat/completions \
  -H "Authorization: Bearer YOUR_OPENAI_API_KEY" \
  -H "Content-Type: application/json" \
  -H "X-User-ID: developer_1" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}],
    "stream": true
  }'
```

- If `developer_1` exceeds the budget limit set by `KILOVOLT_DEFAULT_BUDGET`, Kilovolt immediately short-circuits the connection:
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

---

## 🗺️ Future Roadmap

- **Automated CI/CD Pipeline**: Transitioning from manual Docker Hub releases to a zero-touch GitHub Actions architecture.
- **Dynamic Pricing Expansion**: Scaling the internal ledger matrix to support additional upstream LLM providers (Anthropic, Gemini).

---

## 📄 License
This project is licensed under the MIT License - see the LICENSE file for details.

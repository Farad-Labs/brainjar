# OpenAI Embedding API Research — Brainjar Feature Parity

> Researched: 2026-04-02  
> Goal: Achieve feature parity with our Gemini embedding implementation for OpenAI users.

---

## TL;DR Recommendations

**Embedding model:** `text-embedding-3-large` at **1024 dimensions** for best quality/cost balance. Use `text-embedding-3-small` at 512 dims for cost-sensitive deployments.

**Smart search extraction:** `gpt-4.1-nano` — $0.10/M input, $0.40/M output, supports Structured Outputs. Cheapest capable model.

**Task differentiation:** OpenAI has NO native task type support. The `embed_openai()` path currently ignores `task_type` — this is fine for now, but document it. No prefix tricks are established/documented for text-embedding-3 models.

**Batching:** Send arrays of up to ~100 texts per sync request (practical limit before hitting per-request token limits). Use the Batch API for large ingests — **50% cheaper** with 24h completion window.

**Estimated costs at 1M tokens/month:**
- `text-embedding-3-small`: $0.02 vs Gemini embedding-2 at $0.20 → **10x cheaper**
- `text-embedding-3-large`: $0.13 vs Gemini embedding-2 at $0.20 → **1.5x cheaper**
- Smart search with `gpt-4.1-nano`: ~$0.50/10K queries (tiny prompts)

---

## 1. Model Comparison

### text-embedding-3-small vs text-embedding-3-large

| Property | text-embedding-3-small | text-embedding-3-large | Gemini embedding-2 |
|----------|----------------------|----------------------|-------------------|
| MTEB score | 62.3% | 64.6% | ~84.0% |
| MIRACL (multilingual) | 44.0% | 54.9% | — |
| Default dimensions | 1536 | 3072 | 3072 |
| Max input tokens | 8192 | 8192 | 8192 |
| Price (per 1M tokens) | **$0.02** | **$0.13** | $0.20 |
| Pages per dollar (~800 tok/page) | 62,500 | 9,615 | ~6,250 |
| Task type support | ❌ No | ❌ No | ✅ Yes (v1) / prefix (v2) |

**Important MTEB caveat:** The OpenAI MTEB scores (62-64%) appear to be from 2024 documentation. Gemini embedding-2's 84.0% score likely reflects a newer benchmark run or different evaluation methodology. Direct apples-to-apples comparison is tricky — OpenAI's models are trained differently and may perform better on specific English retrieval tasks than the single MTEB score implies.

### Are there newer models (2025-2026)?

As of April 2026, **no new OpenAI embedding models have been released beyond text-embedding-3-small and text-embedding-3-large** (both released January 25, 2024). OpenAI's focus has been on GPT-5.x and realtime models. `text-embedding-ada-002` is legacy but still available.

No `text-embedding-4` exists yet.

### Dimension Reduction (Matryoshka)

Both v3 models use **Matryoshka Representation Learning**, enabling dimension reduction with graceful quality degradation.

**Key finding from OpenAI's blog:**
> "A `text-embedding-3-large` embedding shortened to 256 dimensions still outperforms an unshortened `text-embedding-ada-002` at 1536 dimensions on MTEB."

**Practical dimension tradeoffs for text-embedding-3-large:**

| Dimensions | Quality | Storage reduction | Recommended use |
|-----------|---------|-------------------|-----------------|
| 3072 (default) | Best | — | Maximum quality, unlimited storage |
| 1024 | Very good | 67% | **Best balance — our recommendation** |
| 512 | Good | 83% | Cost-sensitive deployments |
| 256 | Acceptable | 92% | Still beats ada-002 at 1536! |

**For text-embedding-3-small:**

| Dimensions | Notes |
|-----------|-------|
| 1536 (default) | Full quality |
| 512 | Recommended reduction |
| 256 | Aggressive but usable |

**API usage:**
```json
{
  "model": "text-embedding-3-large",
  "input": "text here",
  "dimensions": 1024
}
```

**brainjar implication:** The `dimensions` field in `brainjar.toml` maps directly to this API parameter. The current `embed_openai()` implementation does NOT pass this parameter yet — it always returns the model's default dimensions. **This is a gap that needs fixing.**

---

## 2. Task Type / Query-Document Differentiation

### Does OpenAI have task types?

**No.** OpenAI has no equivalent to Gemini's `taskType` parameter (`RETRIEVAL_DOCUMENT` / `RETRIEVAL_QUERY`). The embeddings API takes a model name and input text — that's it. No task type, no role distinction.

### Do prefix tricks work like Nomic/E5?

**No established prefix format.** Unlike:
- **Nomic Embed:** requires `search_document:` / `search_query:` prefixes (mandatory)
- **E5 models:** use `passage:` / `query:` prefixes
- **BGE models:** use `Represent this sentence for searching relevant passages:` prefix for queries

OpenAI's text-embedding-3 models were **not trained with task-prefix conventions** and there is no official guidance or community evidence that arbitrary prefixes improve retrieval quality. The models are "universal" — they don't distinguish between query and document encoding.

### What this means for brainjar

The current code in `embed.rs` already handles this correctly:

```rust
"openai" => self.embed_openai(texts).await,
```

The `task_type` parameter is silently ignored for OpenAI. This is the right behavior — there's nothing useful to do with it. The comment in the code acknowledges this:

```rust
/// - gemini-embedding-001: uses `taskType` API parameter
/// - gemini-embedding-2-preview: uses text prefix format
```

OpenAI should be added to this comment as:
```
/// - openai: no task type support — task_type parameter is ignored
```

### Research: Could custom prefixes help?

Community experiments (2024) have tested prepending "query: " or "passage: " to text-embedding-3 models. Results are **inconsistent and model-dependent** — the text-embedding-3 family was not trained for this and shows no reliable improvement. Don't add prefix handling for OpenAI.

---

## 3. Batch Embedding

### Synchronous API batching

The `/v1/embeddings` endpoint accepts an **array of strings or token arrays** in a single request:

```json
{
  "model": "text-embedding-3-small",
  "input": ["text 1", "text 2", "text 3", ...]
}
```

**Limits per single synchronous request:**
- **Per-input token limit:** 8,192 tokens per individual text
- **Practical batch size:** ~100 texts (undocumented soft limit; community observes ~300K total tokens per request before errors)
- **No official documented per-request array count limit** — it's bounded by the TPM rate limit, not a hard item count

The current `embed_openai()` implementation already passes the full array — this is correct. ✅

### Async Batch API

OpenAI has a **Batch API** at `/v1/batch` that supports embeddings:

- Upload a `.jsonl` file of embedding requests
- Submit as a batch job
- Results available within 24 hours (usually much faster)
- **50% cost discount** on all tokens
- Separate, higher rate limit pool (doesn't count against sync limits)
- Up to **50,000 requests per batch**, max 200MB file
- For `/v1/embeddings`: max **50,000 embedding inputs** across all requests in a batch

**This is equivalent to Gemini's `batchEmbedContents`.** Unlike Gemini's true async batch endpoint, OpenAI's Batch API is a file-upload workflow, not a single API call.

**For brainjar's use case:** Large initial syncs (thousands of files) should use the Batch API for 50% savings. Currently not implemented.

### Rate Limits by Tier

**text-embedding-3-large:**

| Tier | Qualification | RPM | RPD | TPM | Batch queue |
|------|--------------|-----|-----|-----|-------------|
| Free | Allowed geography | 100 | 2,000 | 40,000 | — |
| Tier 1 | $5 paid | 3,000 | — | 1,000,000 | 3,000,000 |
| Tier 2 | $50 paid + 7d | 5,000 | — | 1,000,000 | 20,000,000 |
| Tier 3 | $100 paid + 7d | 5,000 | — | 5,000,000 | 100,000,000 |
| Tier 4 | $250 paid + 14d | 10,000 | — | 5,000,000 | 500,000,000 |
| Tier 5 | $1K paid + 30d | 10,000 | — | 10,000,000 | 4,000,000,000 |

**text-embedding-3-small:** Same rate limit structure, typically higher TPM ceilings than large.

**Key insight:** Embedding models have **much higher TPM than chat models** at every tier. A Tier 1 user gets 1M tokens/minute for embeddings vs. much lower limits for GPT models.

---

## 4. Smart Search Extraction

This is used for generating 2-5 search queries from a user's natural language input.

### Model Comparison for JSON Extraction

| Model | Input $/1M | Output $/1M | Context | Structured Outputs | Notes |
|-------|-----------|-----------|---------|-------------------|-------|
| **gpt-4.1-nano** | **$0.10** | **$0.40** | 1M tokens | ✅ Yes | **Recommended** |
| gpt-4.1-mini | $0.40 | $1.60 | 1M tokens | ✅ Yes | 4x more expensive |
| gpt-4.1 | $2.00 | $8.00 | 1M tokens | ✅ Yes | Overkill |
| gpt-3.5-turbo | $3.00 | $6.00 | 16K tokens | ⚠️ JSON mode only | Legacy, expensive |
| gpt-5.4-nano | $0.05 | $0.20 | — | ✅ Yes | Newer, even cheaper if available |

> Note: `gpt-4.1-nano` pricing from official docs: $0.10/1M input, $0.40/1M output (as of April 2025 release). `gpt-5.4-nano` at $0.20 input/$1.25 output is actually MORE expensive than gpt-4.1-nano.

**Winner: `gpt-4.1-nano`** — cheapest capable model with full Structured Outputs support.

### Cost Estimate for Smart Search

A typical smart search prompt (extract 3 queries from a paragraph of text):
- ~200 input tokens + ~80 output tokens = ~280 tokens per call
- At $0.10/1M input + $0.40/1M output: **~$0.000052 per query** = $0.052 per 1,000 queries

This is essentially free for personal use.

### JSON Mode vs Structured Outputs

OpenAI has two mechanisms for JSON:

1. **JSON mode** (`response_format: {"type": "json_object"}`): Guarantees valid JSON but not a specific schema. Works with older models.

2. **Structured Outputs** (`response_format: {"type": "json_schema", "json_schema": {...}}`): Guarantees the output matches your exact schema. Requires gpt-4o-2024-08-06 or newer, or any gpt-4.1 model.

For brainjar's use case (extracting a `{"queries": ["..."]}` object), **Structured Outputs** is the correct choice with gpt-4.1-nano.

### Current Implementation

The `extract.rs` OpenAI path currently uses the chat completions API with a prompt that asks for JSON. It works but doesn't use Structured Outputs. For the simple `ExtractionResult` schema (entities + relationships), this is fine.

For the smart search extraction specifically (if/when we add it), use Structured Outputs:

```json
{
  "model": "gpt-4.1-nano",
  "messages": [...],
  "response_format": {
    "type": "json_schema",
    "json_schema": {
      "name": "search_queries",
      "schema": {
        "type": "object",
        "properties": {
          "queries": {
            "type": "array",
            "items": {"type": "string"},
            "minItems": 1,
            "maxItems": 5
          }
        },
        "required": ["queries"],
        "additionalProperties": false
      },
      "strict": true
    }
  }
}
```

---

## 5. API Differences vs Gemini

### Error Handling

**OpenAI error format:**
```json
{
  "error": {
    "message": "Rate limit exceeded...",
    "type": "requests",
    "param": null,
    "code": "rate_limit_exceeded"
  }
}
```

**Gemini error format:**
```json
{
  "error": {
    "code": 429,
    "message": "Resource exhausted...",
    "status": "RESOURCE_EXHAUSTED"
  }
}
```

**Current brainjar gap:** `embed_openai()` does NOT check the HTTP status code before parsing the response body. If the response is a 429 or 500, it will try to parse the error JSON as an embedding response and fail with a confusing "missing data array" error. The Gemini implementation correctly checks `status.is_success()`.

**Fix needed:**
```rust
let status = resp.status();
let json: serde_json::Value = resp.json().await...;
if !status.is_success() {
    let err_msg = json["error"]["message"].as_str().unwrap_or("unknown error");
    anyhow::bail!("OpenAI API error ({}): {}", status, err_msg);
}
```

### Rate Limit Headers

OpenAI returns these headers on EVERY response:

| Header | Example | Description |
|--------|---------|-------------|
| `x-ratelimit-limit-requests` | `3000` | Max requests per minute |
| `x-ratelimit-limit-tokens` | `1000000` | Max tokens per minute |
| `x-ratelimit-remaining-requests` | `2999` | Remaining requests this window |
| `x-ratelimit-remaining-tokens` | `999985` | Remaining tokens this window |
| `x-ratelimit-reset-requests` | `20ms` | Time until request limit resets |
| `x-ratelimit-reset-tokens` | `0s` | Time until token limit resets |

Gemini does NOT provide these headers. OpenAI's approach is much more debuggable.

### Retry Strategy

OpenAI recommends exponential backoff with jitter:
1. On 429: read `Retry-After` header if present, else start at 1s
2. Double the wait each retry: 1s → 2s → 4s → 8s → 16s
3. Add random jitter (±20%) to prevent thundering herd
4. Max retries: 5-6

```rust
// Pseudocode for retry logic
let mut delay = Duration::from_secs(1);
for attempt in 0..6 {
    match make_request().await {
        Ok(resp) if resp.status() == 429 => {
            let jitter = rand::random::<f32>() * 0.4 + 0.8; // 0.8-1.2x
            tokio::time::sleep(delay.mul_f32(jitter)).await;
            delay *= 2;
        }
        Ok(resp) => return Ok(resp),
        Err(e) => return Err(e),
    }
}
```

**Current brainjar:** No retry logic anywhere. Both Gemini and OpenAI paths will hard-fail on 429s. This should be added to the HTTP client layer, not per-provider.

### Other Gotchas

1. **Token counting:** OpenAI uses `cl100k_base` tokenizer (tiktoken) for text-embedding-3 models. Characters ≠ tokens. A safe heuristic is ~4 chars/token for English text.

2. **Response ordering:** OpenAI returns embeddings in order matching the input array, indexed by `data[i].index`. The current implementation assumes ordered response — this is correct per API contract.

3. **Base URL for the dimensions param:** Currently `embed_openai()` sends `{"model": ..., "input": [...]}` without the `dimensions` parameter. Need to add:
   ```rust
   if self.config.dimensions > 0 {
       body["dimensions"] = self.config.dimensions.into();
   }
   ```
   Without this, `text-embedding-3-large` returns 3072-dim vectors even if `brainjar.toml` says `dimensions = 1024`.

4. **No streaming for embeddings:** Neither OpenAI nor Gemini support streaming embeddings. All requests are synchronous response.

5. **Organization vs project rate limits:** OpenAI rate limits apply at the organization level AND project level. If sharing an API key across projects, each project has its own limit pool.

6. **Token counting quirk:** If you pass 100 texts with 50 tokens each, OpenAI counts 5,000 tokens against your TPM limit. But the RPM limit counts the single request (not 100 separate requests). This means large batches are more rate-limit-efficient.

---

## Gemini vs OpenAI Comparison Table

| Feature | Gemini embedding-2 | OpenAI text-embedding-3-large | Winner |
|---------|-------------------|------------------------------|--------|
| MTEB score | ~84.0% | 64.6% | Gemini |
| Dimensions | 3072 | 3072 (reducible to 256+) | Tie |
| Pricing | $0.20/1M | $0.13/1M | **OpenAI** |
| Task type support | ✅ (prefix) | ❌ | Gemini |
| Dimension reduction | ❌ | ✅ Matryoshka | **OpenAI** |
| Batch API | ✅ batchEmbedContents | ✅ Batch API (50% off) | Tie |
| Max batch per sync request | 100+ texts | 100+ texts | Tie |
| Rate limit headers | ❌ | ✅ | **OpenAI** |
| Error messages | Verbose | Verbose | Tie |
| Max input tokens | 8192 | 8192 | Tie |
| Multilingual (MIRACL) | — | 54.9% | Unknown |
| Knowledge cutoff | 2024+ | September 2021 | Gemini |
| API docs quality | Good | Excellent | **OpenAI** |

---

## Recommended `brainjar.toml` for OpenAI Users

```toml
# brainjar.toml — OpenAI configuration

[providers.openai]
api_key = "${OPENAI_API_KEY}"  # or set directly

[embeddings]
provider = "openai"
model = "text-embedding-3-large"
dimensions = 1024  # Best quality/cost tradeoff (was: 3072 at 67% less storage)

[extraction]
provider = "openai"
model = "gpt-4.1-nano"  # Cheapest model with Structured Outputs support
enabled = true

[knowledge_bases.notes]
watch_paths = ["~/notes"]
auto_sync = true
description = "Personal notes and documents"
```

**For cost-sensitive / free tier:**
```toml
[embeddings]
provider = "openai"
model = "text-embedding-3-small"
dimensions = 512  # Solid quality at tiny cost ($0.02/1M tokens)
```

---

## Code Changes Needed

### What's Already Working ✅
- `embed_openai()` sends array of texts to `/v1/embeddings` — correct
- Provider config (`[providers.openai]`) with `api_key` — correct
- `ExtractionConfig` supports `provider = "openai"` — correct
- Basic JSON parsing of OpenAI embedding response — correct
- Task type silently ignored for OpenAI (correct behavior, no fix needed)

### What Needs Fixing 🔧

#### 1. Pass `dimensions` parameter to OpenAI embeddings (HIGH PRIORITY)

**File:** `src/embed.rs` — `embed_openai()` method

```rust
// Current:
let body = serde_json::json!({
    "model": self.config.model,
    "input": texts,
});

// Fixed:
let mut body = serde_json::json!({
    "model": self.config.model,
    "input": texts,
});
// Pass dimensions if configured and > 0
// (text-embedding-3 models support Matryoshka dimension reduction)
if self.config.dimensions > 0 {
    body["dimensions"] = serde_json::json!(self.config.dimensions);
}
```

#### 2. Add HTTP status check to `embed_openai()` (HIGH PRIORITY)

**File:** `src/embed.rs` — `embed_openai()` method

```rust
// After sending request:
let status = resp.status();
let json: serde_json::Value =
    resp.json().await.context("Failed to parse OpenAI embed response")?;

if !status.is_success() {
    let err_msg = json["error"]["message"]
        .as_str()
        .unwrap_or("unknown error");
    anyhow::bail!("OpenAI API error ({}): {}", status, err_msg);
}
```

#### 3. Document task type behavior for OpenAI (LOW PRIORITY)

**File:** `src/embed.rs` — top-level `TaskType` doc comment

Add to the existing comment:
```
/// - openai (text-embedding-3-*): no task type support — task_type ignored
/// - ollama: no task type support — task_type ignored
```

#### 4. Consider: Retry logic (MEDIUM PRIORITY — separate issue)

Add exponential backoff retry to the HTTP client layer. Both Gemini and OpenAI paths benefit. Not OpenAI-specific but more important now that OpenAI surfaces rate limit headers we can act on.

#### 5. Consider: Batch API for large syncs (FUTURE)

For initial full-corpus syncs (>1000 files), implement the Batch API path:
- 50% cost savings
- Higher rate limits
- 24h completion window (fine for background sync)

This would be a new code path, not a fix to existing code. Worth adding as a future issue.

---

## Quick Reference: OpenAI Embedding API

```http
POST https://api.openai.com/v1/embeddings
Authorization: Bearer {OPENAI_API_KEY}
Content-Type: application/json

{
  "model": "text-embedding-3-large",
  "input": ["text 1", "text 2"],
  "dimensions": 1024,
  "encoding_format": "float"
}
```

Response:
```json
{
  "object": "list",
  "data": [
    {"object": "embedding", "index": 0, "embedding": [...]},
    {"object": "embedding", "index": 1, "embedding": [...]}
  ],
  "model": "text-embedding-3-large",
  "usage": {"prompt_tokens": 12, "total_tokens": 12}
}
```

**Tokenizer:** `cl100k_base` (same as GPT-4)  
**Distance:** Cosine similarity (normalized vectors, dot product works too)  
**Base URL:** `https://api.openai.com/v1/embeddings` (no versioned path needed)

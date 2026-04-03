# Embedding Models Reference — Brainjar

> Last updated: April 2026. Covers Google Gemini, OpenAI, Ollama (local), and notable alternatives.

---

## TL;DR Recommendations

| Use Case | Recommended Model | Why |
|---|---|---|
| **Best quality (cloud)** | `gemini-embedding-2-preview` | MTEB 84.0, massive improvement over v1, text prefix format, $0.20/MTok |
| **Previous best (cloud)** | `gemini-embedding-001` | MTEB 68.3, $0.15/MTok, still good but v2 is significantly better |
| **Best value (cloud)** | `text-embedding-3-small` | MTEB 62.3, $0.02/MTok, great for general retrieval |
| **Best local/offline** | `mxbai-embed-large` | MTEB 64.7 (beats OpenAI large!), 1024 dims, 670MB |
| **Lightweight local** | `nomic-embed-text` | 768 dims, 274MB, very fast, good quality for size |
| **Multilingual local** | `bge-m3` | 1024 dims, 100+ languages, MTEB 63.0 |
| **Best open-source (GPU)** | `NV-Embed-v2` or `Qwen3-Embedding-8B` | MTEB 72.3 / 70.6, beats everything, needs GPU |

---

## 🔑 Query vs Document Differentiation — Critical for Brainjar

**Most embedding models perform better when you tell them whether text is a query or a document.** This is asymmetric retrieval: queries are short questions, documents are longer passages. The model can optimize the embedding space accordingly.

| Provider | Method | Query Syntax | Document Syntax |
|---|---|---|---|
| **Google Gemini v1** | API parameter | `task_type="RETRIEVAL_QUERY"` | `task_type="RETRIEVAL_DOCUMENT"` |
| **Google Gemini v2** | Text prefix | `"task: search result \| query: {text}"` | `"title: {title} \| text: {content}"` |
| **Cohere** | API parameter | `input_type="search_query"` | `input_type="search_document"` |
| **Voyage AI** | API parameter | `input_type="query"` | `input_type="document"` |
| **Nomic (via Ollama)** | Text prefix | `"search_query: {text}"` | `"search_document: {text}"` |
| **mxbai (via Ollama)** | Text prefix (query only) | `"Represent this sentence for searching relevant passages: {text}"` | No prefix (raw text) |
| **OpenAI** | ❌ None | Same embedding for both | Same embedding for both |
| **BGE-M3, snowflake-arctic** | ❌ None | Same embedding for both | Same embedding for both |

### Implementation Notes for Brainjar

**Gemini:**
```python
# Index time (documents)
genai.embed_content(
    model="models/gemini-embedding-001",
    content=text,
    task_type="RETRIEVAL_DOCUMENT"
)

# Query time
genai.embed_content(
    model="models/gemini-embedding-001",
    content=query,
    task_type="RETRIEVAL_QUERY"
)
```

**Nomic (Ollama):**
```python
# Index time
ollama.embeddings(model='nomic-embed-text', prompt=f'search_document: {text}')

# Query time
ollama.embeddings(model='nomic-embed-text', prompt=f'search_query: {query}')
```

**mxbai (Ollama):**
```python
# Index time - NO PREFIX
ollama.embeddings(model='mxbai-embed-large', prompt=text)

# Query time - ADD PREFIX
query_prompt = 'Represent this sentence for searching relevant passages: '
ollama.embeddings(model='mxbai-embed-large', prompt=query_prompt + query)
```

**Cohere:**
```python
# Index time
co.embed(texts=[text], model="embed-v3-english", input_type="search_document")

# Query time
co.embed(texts=[query], model="embed-v3-english", input_type="search_query")
```

**Voyage AI:**
```python
# Index time
vo.embed(texts, model="voyage-4", input_type="document")

# Query time
vo.embed([query], model="voyage-4", input_type="query")
```

**OpenAI (no differentiation):**
```python
# Same call for both index and query
client.embeddings.create(model="text-embedding-3-small", input=text)
```

### Additional Task Types

Some models support task types beyond retrieval:

**Gemini (gemini-embedding-001):**
- `RETRIEVAL_QUERY`, `RETRIEVAL_DOCUMENT` (primary for RAG)
- `SEMANTIC_SIMILARITY`, `CLASSIFICATION`, `CLUSTERING`
- `QUESTION_ANSWERING`, `FACT_VERIFICATION`

**Cohere (embed-v3):**
- `search_query`, `search_document` (primary for RAG)
- `classification`, `clustering`

**Nomic (nomic-embed-text-v1.5):**
- `search_query:`, `search_document:` (primary for RAG)
- `clustering:`, `classification:`

---

## 1. Google Gemini

> **⚠️ API Name Change:** `text-embedding-004` is **GONE** (404 on both v1 and v1beta as of March 2026). Use `gemini-embedding-001` or `gemini-embedding-2-preview` instead.

| Model | Dims | Price (per 1M tokens) | Max Tokens | MTEB | Status |
|---|---|---|---|---|---|
| `gemini-embedding-2-preview` | 3,072 | **$0.20/MTok** | 8,192 | **84.0** | ✅ **Recommended** — text prefix format, multimodal |
| `gemini-embedding-001` | 3,072 | Free tier (1,500 RPD) / ~$0.15/MTok via Vertex | 8,192 | 68.3 | ✅ Active — taskType parameter |
| ~~`text-embedding-004`~~ | ~~768~~ | — | — | — | ❌ **Deprecated / 404** |

### gemini-embedding-2-preview Details

- **MTEB score:** 84.0 (vs 68.3 for v1 — a massive leap)
- **Price:** $0.20/M tokens (vs ~$0.15 for v1)
- **Dimensions:** 3,072 (same as v1)
- **Task type mechanism:** Text prefix format — does **NOT** use the `taskType` API parameter
  - Documents: `title: {title} | text: {content}`
  - Queries: `task: search result | query: {content}`
  - Code queries: `task: code retrieval | query: {content}`
- **Multimodal:** Supports images, video, audio, and PDF natively — but Brainjar only uses the text path
- **When to use:** Default cloud recommendation. Use v1 only if you need the free tier.

---

## 2. OpenAI

| Model | Dims | Price (per 1M tokens) | Max Tokens | MTEB | Notes |
|---|---|---|---|---|---|
| `text-embedding-3-small` | 1,536 | **$0.02** | 8,192 | 62.3 | ✅ Best value |
| `text-embedding-3-large` | 3,072 | $0.13 | 8,192 | 64.6 | Higher quality, 6.5× cost |
| `text-embedding-ada-002` | 1,536 | $0.10 | 8,192 | 61.0 | ⚠️ Legacy, avoid for new projects |

### Special Features
- **Dimension truncation (Matryoshka):** Both `-3-small` and `-3-large` support truncating to fewer dimensions via the `dimensions` parameter without catastrophic quality loss. Example: `text-embedding-3-large` at 256 dims still outperforms ada-002 at full 1,536.
- **Batch API discount:** 50% off → $0.01/MTok (small), $0.065/MTok (large). Good for bulk indexing.
- **⚠️ No task type differentiation** — same embedding for queries and documents (symmetric retrieval). This is a known limitation compared to Gemini/Cohere/Voyage.

```python
# Dimension truncation example
response = client.embeddings.create(
    model="text-embedding-3-large",
    input=text,
    dimensions=512  # Reduce from 3072, quality degrades gracefully
)
```

---

## 3. Ollama (Local / Offline)

All models are free to run. Cost = your compute only.

| Model | Ollama Name | Dims | Size | MTEB | Notes |
|---|---|---|---|---|---|
| mxbai-embed-large | `mxbai-embed-large` | 1,024 | ~670MB | **64.7** | 🏆 Best quality/size for local; SOTA BERT-large class |
| nomic-embed-text v1.5 | `nomic-embed-text` | 768 | ~274MB | 59.4 | Fast, good context (8K), Matryoshka support |
| BGE-M3 | `bge-m3` | 1,024 | ~1.2GB | 63.0 | Multilingual (100+ langs), multi-functionality |
| snowflake-arctic-embed | `snowflake-arctic-embed` | 1,024 (large) | 22M–335M | ~63 | Multiple sizes; good English retrieval |
| snowflake-arctic-embed2 | `snowflake-arctic-embed2` | 1,024 | ~568M | ~64 | Adds multilingual, improved quality |
| all-MiniLM-L6-v2 | `all-minilm` | 384 | ~90MB | 56.3 | Ultra-lightweight, fast prototyping only |
| BGE-large | `bge-large` | 1,024 | ~335M | ~64 | English-focused, good retrieval |
| nomic-embed-text-v2-moe | `nomic-embed-text-v2-moe` | ? | ~143M | — | Multilingual MoE, newer model |
| Qwen3-Embedding | `qwen3-embedding` | 2,048 (8B) | 0.6B/4B/8B | **70.6** (multilingual) | 🏆 Best open-source if you have GPU |
| EmbeddingGemma | `embeddinggemma` | 768 | 300M | ~60 | Google, ultra-lightweight on-device |
| IBM Granite Embedding | `granite-embedding` | ? | 30M/278M | — | Enterprise-grade, multilingual 278M |

### Notes for Brainjar
- **nomic-embed-text** uses text prefixes: `search_query:` and `search_document:` (see example above)
- **mxbai-embed-large** uses a query prefix only: `"Represent this sentence for searching relevant passages: "` prepended to queries, nothing for documents
- **bge-m3, snowflake-arctic, all-minilm** have NO task type support — symmetric retrieval only
- `mxbai-embed-large` is the go-to default for local setups on CPU (M-series Mac, etc.) — but remember to use the query prefix!
- `nomic-embed-text` if you want proper query/document differentiation locally
- `bge-m3` if multilingual support matters
- `qwen3-embedding` if you have a GPU and want best-in-class open-source

---

## 4. Notable Alternatives

| Provider | Model | Dims | Price (per 1M tokens) | MTEB | Notes |
|---|---|---|---|---|---|
| **Cohere** | `embed-v3-english` | 1,024 | $0.10 | 64.5 | Task types (search_document/query/classification/clustering) |
| **Cohere** | `embed-v4` | 1,024–1,536 | $0.12 | 65.2 | Multimodal (text + images), 8K context |
| **Voyage AI** | `voyage-3.5-lite` | 1,024 | $0.02 | ~64 | Long context (32K), budget pick |
| **Voyage AI** | `voyage-3.5` | 1,024 | $0.06 | ~67 | Strong retrieval, domain-specific |
| **Voyage AI** | `voyage-code-3` | 1,024 | $0.18 | N/A | Code retrieval specialist |
| **Mistral** | `mistral-embed` | 1,024 | **$0.01** | ~63 | Cheapest commercial option |
| **Mistral** | `codestral-embed` | 1,536 | $0.15 | N/A | Code-specialized, 32K context |
| **Jina AI** | `jina-embeddings-v4` | 4,096 | Contact sales | ~67 | Multimodal, 3.8B params, 32K context |

### Cohere Input Types
Cohere `embed-v3` and `embed-v4` support `input_type` parameter:
- `search_document` — indexing documents (embeddings optimized for being retrieved)
- `search_query` — searching/querying (embeddings optimized for retrieval)
- `classification` — text classification
- `clustering` — grouping

**Critical:** Always specify `input_type` for retrieval tasks. Embeddings created with `search_query` and `search_document` are compatible (same vector space) but optimized differently.

---

## 5. GPU-Required Open Source (Self-Hosted, Not Ollama)

These require serious GPU infrastructure but beat every commercial API:

| Model | Dims | MTEB | License | Notes |
|---|---|---|---|---|
| `NV-Embed-v2` (NVIDIA) | 4,096 | **72.3** | Open | Needs A100/H100; ~$0.001/MTok on cloud GPU |
| `Qwen3-Embedding-8B` | 2,048 | **70.6** (multilingual) | Apache 2.0 | Best multilingual open-source |
| `Llama-Embed-Nemotron-8B` | 4,096 | N/A | Open | Top multilingual MTEB |
| `BGE-large-en-v1.5` (BAAI) | 1,024 | 63.98 | MIT | Solid, well-supported |

---

## 6. Full Comparison Table

| Provider | Model | Dims | Price /1M | MTEB | Context | Task Types | Matryoshka |
|---|---|---|---|---|---|---|---|
| Google | `gemini-embedding-2-preview` | 3,072 | $0.20 | **84.0** | 8K | ✅ Text prefix | ❌ |
| Google | `gemini-embedding-001` | 3,072 | Free / ~$0.15 via Vertex | 68.3 | 8K | ✅ 7 types | ❌ |
| OpenAI | `text-embedding-3-small` | 1,536 | $0.02 | 62.3 | 8K | ❌ | ✅ |
| OpenAI | `text-embedding-3-large` | 3,072 | $0.13 | 64.6 | 8K | ❌ | ✅ |
| OpenAI | `text-embedding-ada-002` | 1,536 | $0.10 | 61.0 | 8K | ❌ | ❌ |
| Ollama | `mxbai-embed-large` | 1,024 | Free (local) | 64.7 | 512 | ❌ | ❌ |
| Ollama | `nomic-embed-text` | 768 | Free (local) | 59.4 | 8K | ❌ | ✅ |
| Ollama | `bge-m3` | 1,024 | Free (local) | 63.0 | 8K | ❌ | ❌ |
| Ollama | `qwen3-embedding` (8B) | 2,048 | Free (local) | 70.6† | — | ❌ | ✅ |
| Cohere | `embed-v3-english` | 1,024 | $0.10 | 64.5 | 512 | ✅ 4 types | ❌ |
| Cohere | `embed-v4` | 1,024–1,536 | $0.12 | 65.2 | 8K | ✅ 4 types | ❌ |
| Voyage AI | `voyage-3.5` | 1,024 | $0.06 | ~67 | 32K | ❌ | ❌ |
| Mistral | `mistral-embed` | 1,024 | $0.01 | ~63 | 8K | ❌ | ❌ |
| Jina AI | `jina-embeddings-v4` | 4,096 | Contact | ~67 | 32K | ❌ | ❌ |

> †Qwen3-Embedding-8B MTEB score is on the multilingual leaderboard. English-only score may differ.

---

## 7. Brainjar-Specific Guidance

### Provider Defaults

```toml
# brainjar.toml recommended defaults

[embeddings.gemini]
model = "gemini-embedding-2-preview"   # recommended: MTEB 84.0, text prefix format
# model = "gemini-embedding-001"       # fallback: free tier, taskType parameter
# NOT text-embedding-004 (deprecated/404)

[embeddings.openai]
model = "text-embedding-3-small"
# Use text-embedding-3-large for quality-critical use cases
# Both support dimension truncation via `dimensions` param

[embeddings.ollama]
model = "mxbai-embed-large"
query_prefix = "Represent this sentence for searching relevant passages: "
# NO prefix for documents

# Alternative: nomic-embed-text with proper task prefixes
# model = "nomic-embed-text"
# query_prefix = "search_query: "
# document_prefix = "search_document: "

# Multilingual (no task types): bge-m3
```

### Dimension Compatibility Warning
If you switch models, you **must re-embed your entire corpus**. Embeddings from different models (or different dimension settings) are not compatible. Brainjar should track which model was used to create the index and warn on mismatch.

### Context Length Notes
- `mxbai-embed-large` has a **512 token** context limit — short! Chunk aggressively.
- `nomic-embed-text` supports **8K tokens** — much better for longer documents.
- Cloud models (Gemini, OpenAI) all support 8K.
- For very long documents, **Voyage 4 and Code-3 support 32K** — best cloud option for large contexts.

### Storage Impact
Dimension count directly affects vector DB storage:
- 384 dims (all-MiniLM): ~1.5KB per vector
- 768 dims (nomic): ~3KB per vector
- 1,024 dims (mxbai, bge-m3): ~4KB per vector
- 3,072 dims (Gemini, OpenAI large): ~12KB per vector

At 1M documents: 384 dims → ~1.5GB | 3,072 dims → ~12GB

---

## 8. Summary: Which Model for Brainjar?

### Cloud (API-based)

| Priority | Model | Why |
|---|---|---|
| **Quality** | `gemini-embedding-2-preview` | MTEB 84.0, text prefix format, 3072 dims, $0.20/MTok |
| **Free tier** | `gemini-embedding-001` | MTEB 68.3, free tier (1,500 RPD), taskType parameter |
| **Value** | `text-embedding-3-small` | $0.02/MTok, MTEB 62.3, good enough for most use cases |
| **Retrieval-optimized** | `voyage-4` | MTEB ~67, 32K context, task types, $0.06/MTok |
| **Budget** | `mistral-embed` | $0.01/MTok, MTEB ~63, no task types |

### Local (Ollama)

| Priority | Model | Why |
|---|---|---|
| **Quality** | `mxbai-embed-large` | MTEB 64.7, beats OpenAI large, 1024 dims, **query prefix required** |
| **Proper task types** | `nomic-embed-text` | MTEB 59.4, 768 dims, 8K context, `search_query:` / `search_document:` prefixes |
| **Multilingual** | `bge-m3` | MTEB 63.0, 100+ languages, no task types |
| **Lightweight** | `all-minilm` | MTEB 56.3, 384 dims, 90MB, prototyping only |

### Recommendation

**For Brainjar:**
1. **Default cloud:** `gemini-embedding-2-preview` (MTEB 84.0 + text prefix format + 3072 dims)
2. **Free tier cloud:** `gemini-embedding-001` (free tier + MTEB 68.3 + taskType parameter)
3. **Default local:** `nomic-embed-text` (proper query/document prefixes + 8K context)
4. **High-quality local:** `mxbai-embed-large` (but remember the query prefix!)

**Key implementation requirement:** Brainjar MUST handle task types/prefixes differently at index time vs query time for each provider.

**v2 vs v1 implementation difference:** gemini-embedding-2-preview uses text prefix format (prepended strings) NOT the `taskType` API parameter. The `embed_documents()` method handles this automatically.

---

## 9. Sources & Further Reading

- [Awesome Agents: Embedding Pricing March 2026](https://awesomeagents.ai/pricing/embedding-models-pricing/)
- [MTEB Leaderboard (HuggingFace)](https://huggingface.co/spaces/mteb/leaderboard)
- [OpenAI Embeddings Guide](https://platform.openai.com/docs/guides/embeddings)
- [Google Gemini Embeddings Docs](https://ai.google.dev/gemini-api/docs/embeddings)
- [Ollama Embedding Models](https://ollama.com/search?c=embedding)
- [Google Vertex AI Task Types](https://docs.cloud.google.com/vertex-ai/generative-ai/docs/embeddings/task-types)

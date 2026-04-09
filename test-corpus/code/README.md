# Atlas Code Corpus — Tree-Sitter Testing

Small but realistic source files across 17 languages, used to test tree-sitter
AST parsing, node chunking, and entity extraction in BrainJar.

All files model parts of the **Atlas data pipeline** system:
ingestion → transformation → storage, with monitoring and event routing support.

---

## Files

| File | Language | Key features tested |
|------|----------|---------------------|
| `atlas_ingestion.rs` | Rust | `struct`, `impl`, trait, `async fn`, `use`, doc comments, module constants, `type` alias |
| `transform_engine.py` | Python | `dataclass`, decorator, `TypeAlias`, type hints, docstrings, class methods |
| `pipeline.ts` | TypeScript | `interface`, generic class, `async`/`await`, `export`, JSDoc, `const` assertion |
| `handlers.js` | JavaScript | ES module `import`/`export`, arrow functions, `class`, CommonJS interop comment, JSDoc |
| `storage.go` | Go | `struct`, `interface`, method receivers, goroutine, `const`, `type` alias, error wrapping |
| `DataProcessor.java` | Java | `class`, `interface`, `record`, `@Override`, inner types, Javadoc, `static final` |
| `ingestion.c` | C | `struct`, `typedef`, `#define`, flexible array member, function prototypes, `#include` |
| `transform.cpp` | C++ | `template`, `class`, `namespace`, virtual/override, `using`, `#include`, constexpr |
| `event_router.rb` | Ruby | `module`, `class`, mixin (`include`), `method_missing`, `require`, frozen string literal |
| `Protocol.swift` | Swift | `protocol`, `struct`, `extension`, `enum`, `typealias`, `guard let`, `async` |
| `DataModels.kt` | Kotlin | `data class`, `interface`, `companion object`, `suspend fun`, extension functions |
| `EventBus.cs` | C# | `interface`, `sealed record`, `async`/`await`, LINQ, `using`, primary constructor |
| `config.lua` | Lua | module table, `local` functions, metatables, variadic args, `os.getenv` |
| `pipeline.zig` | Zig | `comptime`, error set, `pub fn`, generic `fn` returning type, `assert` |
| `metrics.scala` | Scala | `sealed trait`, `case class`, companion `object`, pattern matching, `type` alias |
| `supervisor.ex` | Elixir | `use Supervisor`, `GenServer`, pattern matching, pipe operator (`\|>`), `@moduledoc` |
| `middleware.php` | PHP | `namespace`, `interface`, `final class`, constructor promotion, `declare(strict_types)` |

---

## Content Theme

Every file uses Atlas-domain naming to keep the corpus coherent:

- **Ingestion**: `IngestEvent`, `IngestionBackend`, `IngestionCoordinator`
- **Transformation**: `TransformResult`, `TransformRule`, `TransformEngine`
- **Storage**: `StorageBackend`, `BufferedWriter`, `ClickHouseBackend`
- **Monitoring**: `MetricsCollector`, `Counter`, `Gauge`, `Histogram`
- **Events**: `EventBus`, `EventRouter`, `DeadLetterHandler`

---

## Rationale Markers

Each file contains at least one `// WHY:`, `// HACK:`, or `// NOTE:` comment
explaining a non-obvious design choice — these are used to test comment-aware
chunking and annotation extraction.

---

## Usage in Tests

```toml
# brainjar.toml snippet
[[corpus]]
path = "test-corpus/code"
glob = "**/*"
label = "code"
```

# Test Corpus

Golden corpus for testing brainjar's search, indexing, and scoring algorithms.

## Structure

```
test-corpus/
├── docs/                        # Docs KB -- markdown, meetings, memories
│   ├── *.md                     # Golden corpus (project docs, semantic traps, etc.)
│   └── memories/                # Date-rich multi-format corpus for temporal scoring
│       ├── semantic/            # Extracted signals from conversations
│       ├── episodic/            # Daily memory captures (standups, sessions, debug notes)
│       ├── consolidated/        # Deduplicated, sorted, refined entries from episodic
│       └── canonical_dates.json # Ground-truth dates for grading date extraction
├── code/                        # Code KB -- multi-language source files for tree-sitter
├── generate_memories_corpus.py  # Script to regenerate docs/memories/
└── README.md
```

## Original Golden Corpus

15 markdown files simulating a software team's knowledge base (Nexus Labs / Atlas project). Covers architecture docs, meeting notes, incident reports, client notes, onboarding guides, and specialized test cases for fuzzy matching, semantic traps, and synonym resolution.

Date range: Nov 2025 -- Mar 2026.

## Memories Corpus

77 files across 4 formats designed to test temporal signal extraction and weighted scoring.

| Format | Count | Date Signals |
|--------|-------|-------------|
| Markdown (.md) | 37 | filename, content headers, body text |
| Word (.docx) | 12 | filename, content, metadata (created/modified/author/subject) |
| PDF (.pdf) | 15 | filename, content text, metadata (created/author/subject) |
| Scanned PDF | 1 | filename, rasterized image text (no text layer) |
| JPEG (.jpg) | 12 | filename, EXIF (DateTimeOriginal/CreateDate/ModifyDate/Artist), watermark |

Date range: 2025-11-03 to 2026-03-28 (15 distinct dates).

### Memory Directories

- **semantic/** -- Extracted preferences, terminology snapshots, relevance feedback, codebase conventions. These represent high-confidence signals distilled from conversations.

- **episodic/** -- Daily standups, session logs, debug sessions, incident debriefs, search quality reports. Raw daily captures before deduplication.

- **consolidated/** -- Weekly summaries, confidence-scored insight tables, project health reports, aggregated metrics. Deduplicated and refined from episodic entries.

### Grading

`canonical_dates.json` contains the ground-truth date for every file, plus which signals carry the date. Use it to measure date extraction accuracy:

```rust
// Example: load and compare
let truth: HashMap<String, String> = canonical_dates.entries
    .iter()
    .map(|e| (e.path.clone(), e.canonical_date.clone()))
    .collect();

let extracted = extract_date(&file_path);
assert_eq!(extracted, truth[&relative_path]);
```

### Regenerating

```bash
cd test-corpus
python3 generate_memories_corpus.py
```

Requirements: `python-docx`, `fpdf2`, `Pillow`, ImageMagick (`magick`), `exiftool`.

```bash
pip3 install python-docx fpdf2 Pillow
brew install imagemagick exiftool
```

## Content Coherence

Both corpora share the same fictional universe:
- Team members: Sarah Chen, Marcus Webb, Priya Patel, James Liu, Diana Ross, Elena Vasquez, Alex Novak
- Project: Atlas data pipeline (Rust, ClickHouse, Redis, Kafka/NATS)
- Key events: Redis OOM incident (2026-03-10), NATS migration, ClickHouse schema review, security audit
- Cross-references between memories/ and top-level docs (incident-report.md, architecture.md, sprint-retro.md, etc.)

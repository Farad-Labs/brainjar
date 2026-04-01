use anyhow::{Context, Result};
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use rustyline::completion::{Completer, FilenameCompleter, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config as RlConfig, Editor, Helper};
use std::path::PathBuf;

// ─────────────────────────────────────────────────────────────────────────────
// Rustyline helper: filename completion only
// ─────────────────────────────────────────────────────────────────────────────

struct PathHelper {
    completer: FilenameCompleter,
}

impl Helper for PathHelper {}

impl Completer for PathHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        self.completer.complete(line, pos, ctx)
    }
}

impl Hinter for PathHelper {
    type Hint = String;
    fn hint(&self, _line: &str, _pos: usize, _ctx: &rustyline::Context<'_>) -> Option<String> {
        None
    }
}

impl Highlighter for PathHelper {}
impl Validator for PathHelper {}

// ─────────────────────────────────────────────────────────────────────────────
// Knowledge base config gathered from the wizard
// ─────────────────────────────────────────────────────────────────────────────

struct KbConfig {
    name: String,
    watch_paths: Vec<String>,
    auto_sync: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

pub async fn run_init() -> Result<()> {
    println!("\n{}", "🧠 brainjar init".cyan().bold());
    println!(
        "{}\n",
        "Interactive wizard — generates brainjar.toml and creates .brainjar/ directory".dimmed()
    );

    let theme = ColorfulTheme::default();

    // Guard against overwriting existing config
    let config_path = PathBuf::from("brainjar.toml");
    if config_path.exists() {
        let overwrite = Confirm::with_theme(&theme)
            .with_prompt("brainjar.toml already exists. Overwrite?")
            .default(false)
            .interact()?;
        if !overwrite {
            println!("{}", "Aborted.".yellow());
            return Ok(());
        }
    }

    // ── Knowledge bases ───────────────────────────────────────────────────────
    println!("{}", "── Knowledge bases ────────────────────────────────".dimmed());

    let kb_count: usize = Input::with_theme(&theme)
        .with_prompt("How many knowledge bases?")
        .default(1usize)
        .interact_text()?;

    let mut knowledge_bases: Vec<KbConfig> = Vec::with_capacity(kb_count);

    for i in 0..kb_count {
        println!("\n  {}", format!("Knowledge base {} of {}", i + 1, kb_count).bold());

        let name: String = Input::with_theme(&theme)
            .with_prompt("  Name (e.g. memory, project-docs)")
            .interact_text()?;

        println!("  {}", "Watch paths — tab-complete enabled, empty line to finish:".dimmed());
        let watch_paths = prompt_watch_paths()?;

        if watch_paths.is_empty() {
            println!(
                "  {}",
                "Warning: no watch paths set. Add them manually to brainjar.toml.".yellow()
            );
        }

        let auto_sync = Confirm::with_theme(&theme)
            .with_prompt(format!("  Enable auto_sync for '{}'?", name))
            .default(true)
            .interact()?;

        knowledge_bases.push(KbConfig {
            name,
            watch_paths,
            auto_sync,
        });
    }

    // ── Embedding provider (optional) ─────────────────────────────────────────
    println!("\n{}", "── Embedding provider (optional) ──────────────────".dimmed());
    println!(
        "  {}",
        "Used for vector search (coming in phase 2). Skip for FTS + fuzzy only.".dimmed()
    );

    let embed_providers = &["none", "gemini", "openai", "ollama"];
    let embed_idx = Select::with_theme(&theme)
        .with_prompt("  Embedding provider")
        .items(embed_providers)
        .default(0)
        .interact()?;
    let embed_provider = embed_providers[embed_idx];

    let embedding_config = if embed_provider != "none" {
        let model: String = Input::with_theme(&theme)
            .with_prompt("  Embedding model")
            .default(default_embed_model(embed_provider).to_string())
            .interact_text()?;
        let api_key_env: String = if embed_provider != "ollama" {
            Input::with_theme(&theme)
                .with_prompt("  API key env var (e.g. GEMINI_API_KEY)")
                .default(default_embed_env(embed_provider).to_string())
                .interact_text()?
        } else {
            String::new()
        };
        let base_url: String = if embed_provider == "ollama" {
            Input::with_theme(&theme)
                .with_prompt("  Ollama base URL")
                .default("http://localhost:11434".to_string())
                .interact_text()?
        } else {
            String::new()
        };
        Some((embed_provider.to_string(), model, api_key_env, base_url))
    } else {
        None
    };

    // ── Extraction provider (optional) ────────────────────────────────────────
    println!("\n{}", "── Extraction provider (optional) ─────────────────".dimmed());
    println!(
        "  {}",
        "Used for entity extraction (coming in phase 2). Skip for FTS + fuzzy only.".dimmed()
    );

    let extract_providers = &["none", "gemini", "openai", "ollama"];
    let extract_idx = Select::with_theme(&theme)
        .with_prompt("  Extraction provider")
        .items(extract_providers)
        .default(0)
        .interact()?;
    let extract_provider = extract_providers[extract_idx];

    let extraction_config = if extract_provider != "none" {
        let model: String = Input::with_theme(&theme)
            .with_prompt("  Extraction model")
            .default(default_extract_model(extract_provider).to_string())
            .interact_text()?;
        let api_key_env: String = if extract_provider != "ollama" {
            Input::with_theme(&theme)
                .with_prompt("  API key env var (e.g. GEMINI_API_KEY)")
                .default(default_embed_env(extract_provider).to_string())
                .interact_text()?
        } else {
            String::new()
        };
        Some((extract_provider.to_string(), model, api_key_env))
    } else {
        None
    };

    // ── Generate brainjar.toml ────────────────────────────────────────────────
    println!();
    generate_brainjar_toml(&knowledge_bases, embedding_config.as_ref(), extraction_config.as_ref())?;

    // ── Create .brainjar directory ────────────────────────────────────────────
    std::fs::create_dir_all(".brainjar").context("Failed to create .brainjar directory")?;

    // Add .brainjar to .gitignore if it doesn't already contain it
    maybe_update_gitignore()?;

    print_next_steps();

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Collect watch paths with rustyline tab completion
// ─────────────────────────────────────────────────────────────────────────────

fn prompt_watch_paths() -> Result<Vec<String>> {
    let rl_config = RlConfig::builder()
        .completion_type(CompletionType::List)
        .build();

    let helper = PathHelper {
        completer: FilenameCompleter::new(),
    };

    let mut rl = Editor::with_config(rl_config)?;
    rl.set_helper(Some(helper));

    let mut paths: Vec<String> = Vec::new();

    loop {
        let prompt = format!(
            "  {}",
            if paths.is_empty() {
                "Watch path (empty to finish): "
            } else {
                "Next path  (empty to finish): "
            }
        );

        match rl.readline(&prompt) {
            Ok(line) => {
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    break;
                }
                rl.add_history_entry(&trimmed).ok();
                paths.push(trimmed);
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(e) => return Err(e.into()),
        }
    }

    Ok(paths)
}

// ─────────────────────────────────────────────────────────────────────────────
// brainjar.toml generation
// ─────────────────────────────────────────────────────────────────────────────

fn generate_brainjar_toml(
    kbs: &[KbConfig],
    embedding: Option<&(String, String, String, String)>,
    extraction: Option<&(String, String, String)>,
) -> Result<()> {
    let mut toml = String::from(
        "# brainjar.toml — Knowledge base configuration\n\
         # Generated by `brainjar init`\n\n",
    );

    // Embedding section
    if let Some((provider, model, api_key_env, base_url)) = embedding {
        toml.push_str("[embeddings]\n");
        toml.push_str(&format!("provider   = \"{}\"\n", provider));
        toml.push_str(&format!("model      = \"{}\"\n", model));
        if !api_key_env.is_empty() {
            toml.push_str(&format!("api_key    = \"${{{}}}\"\n", api_key_env));
        }
        if !base_url.is_empty() {
            toml.push_str(&format!("base_url   = \"{}\"\n", base_url));
        }
        toml.push_str("dimensions = 768\n\n");
    }

    // Extraction section
    if let Some((provider, model, api_key_env)) = extraction {
        toml.push_str("[extraction]\n");
        toml.push_str(&format!("provider = \"{}\"\n", provider));
        toml.push_str(&format!("model    = \"{}\"\n", model));
        if !api_key_env.is_empty() {
            toml.push_str(&format!("api_key  = \"${{{}}}\"\n", api_key_env));
        }
        toml.push_str("enabled  = true\n\n");
    }

    // Knowledge base sections
    for kb in kbs {
        let watch_paths_toml = if kb.watch_paths.is_empty() {
            "[]  # TODO: add paths to watch".to_string()
        } else {
            let entries: Vec<String> = kb
                .watch_paths
                .iter()
                .map(|p| format!("\"{}\"", p.replace('"', "\\\"")))
                .collect();
            format!("[{}]", entries.join(", "))
        };

        toml.push_str(&format!(
            "[knowledge_bases.{}]\n\
             watch_paths = {}\n\
             auto_sync   = {}\n\n",
            kb.name, watch_paths_toml, kb.auto_sync,
        ));
    }

    std::fs::write("brainjar.toml", &toml).context("Failed to write brainjar.toml")?;
    println!("{} Generated {}", "✓".green(), "brainjar.toml".cyan());

    Ok(())
}

fn maybe_update_gitignore() -> Result<()> {
    let gitignore_path = PathBuf::from(".gitignore");
    let entry = ".brainjar/";

    if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)?;
        if !content.contains(".brainjar") {
            let updated = format!("{}\n# brainjar local DB\n{}\n", content.trim_end(), entry);
            std::fs::write(&gitignore_path, updated)?;
            println!("{} Added .brainjar/ to {}", "✓".green(), ".gitignore".cyan());
        }
    } else {
        std::fs::write(&gitignore_path, format!("# brainjar local DB\n{}\n", entry))?;
        println!("{} Created {}", "✓".green(), ".gitignore".cyan());
    }

    Ok(())
}

fn print_next_steps() {
    println!("\n{}", "──────────────────────────────────────────────────".dimmed());
    println!("{}", "  Next steps".bold().white());
    println!("{}", "──────────────────────────────────────────────────".dimmed());
    println!("\n  {}  Sync your files to the local index\n", "1.".bold());
    println!("     {}", "brainjar sync".cyan());
    println!("\n  {}  Search your knowledge base\n", "2.".bold());
    println!("     {}", "brainjar search \"your query\"".cyan());
    println!("\n  {}  Use as an MCP server\n", "3.".bold());
    println!("     {}", "brainjar mcp".cyan());
    println!();
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider defaults
// ─────────────────────────────────────────────────────────────────────────────

fn default_embed_model(provider: &str) -> &'static str {
    match provider {
        "gemini" => "text-embedding-004",
        "openai" => "text-embedding-3-small",
        "ollama" => "nomic-embed-text",
        _ => "text-embedding-004",
    }
}

fn default_extract_model(provider: &str) -> &'static str {
    match provider {
        "gemini" => "gemini-3.1-flash-lite-preview",
        "openai" => "gpt-4o-mini",
        "ollama" => "llama3",
        _ => "gemini-3.1-flash-lite-preview",
    }
}

fn default_embed_env(provider: &str) -> &'static str {
    match provider {
        "gemini" => "GEMINI_API_KEY",
        "openai" => "OPENAI_API_KEY",
        _ => "API_KEY",
    }
}

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
// Internal config structures gathered from the wizard
// ─────────────────────────────────────────────────────────────────────────────

struct KbConfig {
    name: String,
    watch_paths: Vec<String>,
    description: Option<String>,
    auto_sync: bool,
}

struct ProviderEntry {
    name: String,   // "gemini" | "openai" | "ollama"
    api_key: String,
    base_url: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// ASCII art mascot
// ─────────────────────────────────────────────────────────────────────────────

fn print_mascot() {
    use colored::Colorize;
 
    // ── Glass dome (cyan) ──────────────────────────────────────────
    //   Rounded capsule shape with brain & sparkle texture inside
    let d1 = "         ,------------,";
    let d2 = "        /  * ~~~ * ~~  \\";
    let d3 = "       |  /~~~~~~~~~\\   |";
    let d4 = "       |  (~~~~~~~~~~)  |";
    let d5 = "       |  \\__________/  |";
    let d6 = "        \\~~~~~~~~~~~~~~/";
 
    // ── Face collar / head band (yellow) ──────────────────────────
    //   Riveted band housing the eyes and mouth
    let f1 = "      .-[● ● ● ● ● ● ● ●]-.";
    let f2 = "      |   (◉)       (◉)   |";
    let f3 = "      |        \\_/        |";
    let f4 = "      '-[● ● ● ● ● ● ● ●]-'";
 
    // ── Body (white/bold) ──────────────────────────────────────────
    //   Claw arm on left, pointing arm on right, chest plate center
    let b1 = "     ----------------------";
    let b2 = "     |                    |";
    let b3 = "<{)==|    [__________]    |==(}>";    // arms at chest level
    let b4 = "     |                    |";
    let b5 = "     ----------------------";
 
    // ── Legs & tank treads (white) ────────────────────────────────
    let l1 = "           |        |";
    let l2 = "          [=]      [=]";
    let l3 = "         /   \\    /   \\";
    let l4 = "        [=====]  [=====]";
    let l5 = "         '----'  '----'";
 
    println!();
    println!("{}", d1.cyan().bold());
    println!("{}", d2.cyan());
    println!("{}", d3.cyan());
    println!("{}", d4.cyan());
    println!("{}", d5.cyan());
    println!("{}", d6.cyan().bold());
    println!("{}", f1.yellow().bold());
    println!("{}", f2.yellow().bold());
    println!("{}", f3.yellow());
    println!("{}", f4.yellow().bold());
    println!("{}", b1.white().bold());
    println!("{}", b2.white().bold());
    println!("{}", b3.white().bold());
    println!("{}", b4.white().bold());
    println!("{}", b5.white().bold());
    println!("{}", l1.white());
    println!("{}", l2.white().bold());
    println!("{}", l3.white());
    println!("{}", l4.white().bold());
    println!("{}", l5.white());
    println!();
    println!("     {}",
        "B  R  A  I  N  J  A  R"
            .bold()
            .cyan()
    );
    println!(
        "   {}",
        "Your local AI memory system".dimmed()
    );
    println!();
}

// ─────────────────────────────────────────────────────────────────────────────
// Colored info box helper
// ─────────────────────────────────────────────────────────────────────────────

fn info_box(lines: &[&str]) {
    let width = lines.iter().map(|l| l.len()).max().unwrap_or(0) + 4;
    let border = "\u{2550}".repeat(width - 2);
    println!("  {}{}{}", "\u{2554}".cyan(), border.cyan(), "\u{2557}".cyan());
    for line in lines {
        let pad = width - 4 - line.len();
        println!(
            "  {}   {}{} {}",
            "\u{2551}".cyan(),
            line,
            " ".repeat(pad),
            "\u{2551}".cyan()
        );
    }
    println!("  {}{}{}", "\u{255a}".cyan(), border.cyan(), "\u{255d}".cyan());
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

pub async fn run_init() -> Result<()> {
    print_mascot();

    println!(
        "  {}",
        "Welcome to Brainjar! Let's get your memory system set up.".bold().white()
    );
    println!();

    let theme = ColorfulTheme::default();

    // Guard against overwriting existing config
    let brainjar_home = dirs::home_dir()
        .map(|h| h.join(".brainjar"))
        .unwrap_or_else(|| PathBuf::from(".brainjar"));
    std::fs::create_dir_all(&brainjar_home).ok();
    let config_path = brainjar_home.join("brainjar.toml");
    if config_path.exists() {
        let overwrite = Confirm::with_theme(&theme)
            .with_prompt("brainjar.toml already exists. Overwrite?")
            .default(false)
            .interact()?;
        if !overwrite {
            println!("{}", "  Aborted.".yellow());
            return Ok(());
        }
    }

    // ── Step 1 — Storage location ─────────────────────────────────────────────
    println!("\n  {}", "Step 1 of 4 — Storage".bold().white());
    println!("  {}", "─".repeat(40).dimmed());
    println!("  {}", "Where should Brainjar store its databases?".dimmed());
    println!();

    let storage_choices = &[
        "~/.brainjar  (recommended — survives project moves)",
        ".brainjar    (current directory — project-local)",
        "Custom path",
    ];
    let storage_idx = Select::with_theme(&theme)
        .with_prompt("  Storage location")
        .items(storage_choices)
        .default(0)
        .interact()?;

    let data_dir: String = match storage_idx {
        0 => "~/.brainjar".to_string(),
        1 => ".brainjar".to_string(),
        _ => {
            let custom: String = Input::with_theme(&theme)
                .with_prompt("  Custom path (~ is expanded)")
                .interact_text()?;
            custom.trim().to_string()
        }
    };

    println!(
        "  {} Data dir: {}",
        "\u{2713}".green(),
        data_dir.cyan()
    );

    // ── Step 2 — AI Providers ─────────────────────────────────────────────────
    println!("\n  {}", "Step 2 of 4 — AI Providers".bold().white());
    println!("  {}", "─".repeat(40).dimmed());

    info_box(&[
        "Brainjar uses AI models for two things:",
        "",
        "  1. Embeddings   — converting notes into searchable vectors",
        "  2. Extraction   — finding people, projects, concepts in docs",
        "",
        "You can use any OpenAI-compatible API, or choose from the",
        "defaults below that balance cost and quality.",
        "",
        "Skip any provider you don't plan to use.",
    ]);
    println!();

    let provider_choices = &["gemini", "openai", "ollama", "other (OpenAI-compatible)"];

    let mut providers: Vec<ProviderEntry> = Vec::new();
    loop {
        let idx = Select::with_theme(&theme)
            .with_prompt("  Provider to configure")
            .items(provider_choices)
            .default(0)
            .interact()?;

        let provider_name = match idx {
            0 => "gemini".to_string(),
            1 => "openai".to_string(),
            2 => "ollama".to_string(),
            _ => {
                let name: String = Input::with_theme(&theme)
                    .with_prompt("  Provider name (used as key in config)")
                    .interact_text()?;
                name.trim().to_string()
            }
        };

        let api_key: String = if provider_name == "ollama" {
            // Ollama uses base_url, not an API key
            String::new()
        } else {
            Input::with_theme(&theme)
                .with_prompt(format!(
                    "  {} API key or env var (e.g. GEMINI_API_KEY, blank to skip)",
                    provider_name.to_uppercase()
                ))
                .default(String::new())
                .allow_empty(true)
                .interact_text()?
        };

        let base_url: String = if provider_name == "ollama" {
            Input::with_theme(&theme)
                .with_prompt("  Ollama base URL")
                .default("http://localhost:11434".to_string())
                .interact_text()?
        } else if idx == 3 {
            // "other" provider always needs a URL
            Input::with_theme(&theme)
                .with_prompt("  Base URL for this provider")
                .interact_text()?
        } else {
            String::new()
        };

        providers.push(ProviderEntry {
            name: provider_name.clone(),
            api_key,
            base_url,
        });

        println!(
            "  {} Added provider: {}",
            "\u{2713}".green(),
            provider_name.cyan()
        );

        let add_more = Confirm::with_theme(&theme)
            .with_prompt("  Add another provider?")
            .default(false)
            .interact()?;
        if !add_more {
            break;
        }
        println!();
    }

    // ── Step 3 — Model defaults ───────────────────────────────────────────────
    println!("\n  {}", "Step 3 of 4 — Model Defaults".bold().white());
    println!("  {}", "─".repeat(40).dimmed());
    println!("  {}", "Choose which provider handles embeddings and entity extraction.".dimmed());
    println!();

    // Embedding provider
    let embed_provider_name: Option<String>;
    let embed_model: Option<String>;

    if providers.is_empty() {
        println!("  {} Skipping embeddings (no providers configured)", "\u{2013}".dimmed());
        embed_provider_name = None;
        embed_model = None;
    } else {
        let mut embed_opts: Vec<String> = vec!["none (FTS + fuzzy only)".to_string()];
        embed_opts.extend(providers.iter().map(|p| p.name.clone()));
        let eidx = Select::with_theme(&theme)
            .with_prompt("  Embedding provider")
            .items(&embed_opts)
            .default(if providers.is_empty() { 0 } else { 1 })
            .interact()?;

        if eidx == 0 {
            embed_provider_name = None;
            embed_model = None;
            println!("  {} Embeddings: none", "\u{2013}".dimmed());
        } else {
            let pname = providers[eidx - 1].name.clone();
            let default_model = default_embed_model(&pname);
            let model: String = Input::with_theme(&theme)
                .with_prompt(format!("  Embedding model ({})", &pname))
                .default(default_model.to_string())
                .interact_text()?;
            println!(
                "  {} Embeddings: {} / {}",
                "\u{2713}".green(),
                pname.cyan(),
                model.dimmed()
            );
            embed_provider_name = Some(pname);
            embed_model = Some(model);
        }
    }

    // Extraction provider
    let extract_provider_name: Option<String>;
    let extract_model: Option<String>;

    if providers.is_empty() {
        println!("  {} Skipping extraction (no providers configured)", "\u{2013}".dimmed());
        extract_provider_name = None;
        extract_model = None;
    } else {
        let mut ext_opts: Vec<String> = vec!["none (graph search disabled)".to_string()];
        ext_opts.extend(providers.iter().map(|p| p.name.clone()));
        let xidx = Select::with_theme(&theme)
            .with_prompt("  Extraction provider")
            .items(&ext_opts)
            .default(if providers.is_empty() { 0 } else { 1 })
            .interact()?;

        if xidx == 0 {
            extract_provider_name = None;
            extract_model = None;
            println!("  {} Extraction: none", "\u{2013}".dimmed());
        } else {
            let pname = providers[xidx - 1].name.clone();
            let default_model = default_extract_model(&pname);
            let model: String = Input::with_theme(&theme)
                .with_prompt(format!("  Extraction model ({})", &pname))
                .default(default_model.to_string())
                .interact_text()?;
            println!(
                "  {} Extraction: {} / {}",
                "\u{2713}".green(),
                pname.cyan(),
                model.dimmed()
            );
            extract_provider_name = Some(pname);
            extract_model = Some(model);
        }
    }

    // ── Step 4 — Knowledge bases ──────────────────────────────────────────────
    println!("\n  {}", "Step 4 of 4 — Knowledge Bases".bold().white());
    println!("  {}", "─".repeat(40).dimmed());
    println!(
        "  {}",
        "A knowledge base is a collection of directories/files you want indexed.".dimmed()
    );
    println!();

    let mut knowledge_bases: Vec<KbConfig> = Vec::new();

    loop {
        let kb_num = knowledge_bases.len() + 1;
        println!(
            "  {}",
            format!("Knowledge base #{}", kb_num).bold()
        );

        let name: String = Input::with_theme(&theme)
            .with_prompt("  Name (e.g. memory, project-docs)")
            .interact_text()?;

        let desc_str: String = Input::with_theme(&theme)
            .with_prompt("  Description (optional, press Enter to skip)")
            .default(String::new())
            .allow_empty(true)
            .interact_text()?;
        let description = if desc_str.trim().is_empty() {
            None
        } else {
            Some(desc_str.trim().to_string())
        };

        println!("  {}", "Watch paths — tab-complete enabled, empty line to finish:".dimmed());
        let watch_paths = prompt_watch_paths()?;

        if watch_paths.is_empty() {
            println!(
                "  {}",
                "Warning: no watch paths set. Add them manually to brainjar.toml.".yellow()
            );
        }

        let auto_sync = Confirm::with_theme(&theme)
            .with_prompt(format!(
                "  Enable auto_sync for '{}'?",
                name
            ))
            .default(true)
            .interact()?;

        knowledge_bases.push(KbConfig {
            name: name.clone(),
            watch_paths,
            description,
            auto_sync,
        });

        println!(
            "  {} KB '{}' added",
            "\u{2713}".green(),
            name.cyan()
        );

        let add_more = Confirm::with_theme(&theme)
            .with_prompt("  Add another knowledge base?")
            .default(false)
            .interact()?;
        if !add_more {
            break;
        }
        println!();
    }

    // ── Step 5 — Summary + confirm + write ───────────────────────────────────
    println!("\n  {}", "Summary".bold().white());
    println!("  {}", "─".repeat(40).dimmed());
    println!("  {} Data dir:   {}", "\u{2022}".cyan(), data_dir.cyan());
    println!(
        "  {} Providers:  {}",
        "\u{2022}".cyan(),
        if providers.is_empty() {
            "none".dimmed().to_string()
        } else {
            providers.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ").cyan().to_string()
        }
    );
    let embed_summary = embed_provider_name
        .as_deref()
        .map(|p| format!("{} / {}", p, embed_model.as_deref().unwrap_or("")))
        .unwrap_or_else(|| "none".to_string());
    println!(
        "  {} Embeddings: {}",
        "\u{2022}".cyan(),
        embed_summary.dimmed()
    );
    let extract_summary = extract_provider_name
        .as_deref()
        .map(|p| format!("{} / {}", p, extract_model.as_deref().unwrap_or("")))
        .unwrap_or_else(|| "none".to_string());
    println!(
        "  {} Extraction: {}",
        "\u{2022}".cyan(),
        extract_summary.dimmed()
    );
    println!(
        "  {} KBs:        {}",
        "\u{2022}".cyan(),
        knowledge_bases
            .iter()
            .map(|kb| kb.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
            .cyan()
    );
    println!();

    let confirm = Confirm::with_theme(&theme)
        .with_prompt("  Write brainjar.toml?")
        .default(true)
        .interact()?;

    if !confirm {
        println!("{}", "  Aborted — no files written.".yellow());
        return Ok(());
    }

    // ── Generate brainjar.toml ────────────────────────────────────────────────
    generate_brainjar_toml(
        &data_dir,
        &providers,
        embed_provider_name.as_deref(),
        embed_model.as_deref(),
        extract_provider_name.as_deref(),
        extract_model.as_deref(),
        &knowledge_bases,
    )?;

    // ── Create data_dir ───────────────────────────────────────────────────────
    let expanded_data_dir = crate::config::expand_tilde(&data_dir);
    if data_dir == ".brainjar" || data_dir.starts_with("./") || data_dir.starts_with('/') {
        // project-local — create relative to cwd
        std::fs::create_dir_all(&expanded_data_dir)
            .with_context(|| format!("Failed to create data directory: {}", expanded_data_dir.display()))?;
        // Add to .gitignore if project-local
        maybe_update_gitignore(&data_dir)?;
    } else {
        // global — create but don't touch .gitignore
        std::fs::create_dir_all(&expanded_data_dir)
            .with_context(|| format!("Failed to create data directory: {}", expanded_data_dir.display()))?;
    }

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
/// Heuristic: env var names are ALL_CAPS_WITH_UNDERSCORES, raw keys are not.
fn is_env_var_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn format_api_key_value(key: &str) -> String {
    if key.is_empty() {
        String::from("\"\"")
    } else if is_env_var_name(key) {
        format!("\"${{{}}}\"", key)
    } else {
        format!("\"{}\"", key)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// brainjar.toml generation
// ─────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn generate_brainjar_toml(
    data_dir: &str,
    providers: &[ProviderEntry],
    embed_provider: Option<&str>,
    embed_model: Option<&str>,
    extract_provider: Option<&str>,
    extract_model: Option<&str>,
    kbs: &[KbConfig],
) -> Result<()> {
    let mut toml = String::from(
        "# brainjar.toml — Knowledge base configuration\n\
         # Generated by `brainjar init`\n\n",
    );

    // data_dir
    toml.push_str(&format!("data_dir = \"{}\"\n\n", data_dir));

    // [providers] section
    if !providers.is_empty() {
        toml.push_str("[providers]\n");
        for p in providers {
            if !p.api_key.is_empty() {
                toml.push_str(&format!(
                    "{}.api_key = {}\n",
                    p.name,
                    format_api_key_value(&p.api_key)
                ));
            }
            if !p.base_url.is_empty() {
                toml.push_str(&format!(
                    "{}.base_url = \"{}\"\n",
                    p.name, p.base_url
                ));
            }
        }
        toml.push('\n');
    } else {
        toml.push_str(
            "# [providers]\n\
             # gemini.api_key = \"${GEMINI_API_KEY}\"\n\
             # openai.api_key = \"${OPENAI_API_KEY}\"\n\
             # ollama.base_url = \"http://localhost:11434\"\n\n",
        );
    }

    // [embeddings] section
    if let (Some(provider), Some(model)) = (embed_provider, embed_model) {
        toml.push_str("[embeddings]\n");
        toml.push_str(&format!("provider   = \"{}\"\n", provider));
        toml.push_str(&format!("model      = \"{}\"\n", model));
        toml.push_str("dimensions = 768\n\n");
    }

    // [extraction] section
    if let (Some(provider), Some(model)) = (extract_provider, extract_model) {
        toml.push_str("[extraction]\n");
        toml.push_str(&format!("provider = \"{}\"\n", provider));
        toml.push_str(&format!("model    = \"{}\"\n", model));
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
            "[knowledge_bases.{}]\n",
            kb.name
        ));
        if let Some(desc) = &kb.description {
            toml.push_str(&format!("description = \"{}\"\n", desc.replace('"', "\\\"")));
        }
        toml.push_str(&format!(
            "watch_paths = {}\n\
             auto_sync   = {}\n\n",
            watch_paths_toml, kb.auto_sync,
        ));
    }

    let brainjar_home = dirs::home_dir()
        .map(|h| h.join(".brainjar"))
        .unwrap_or_else(|| PathBuf::from(".brainjar"));
    std::fs::create_dir_all(&brainjar_home).ok();
    let config_path = brainjar_home.join("brainjar.toml");
    std::fs::write(&config_path, &toml)
        .with_context(|| format!("Failed to write {}", config_path.display()))?;
    println!("  {} Generated {}", "\u{2713}".green(), config_path.display().to_string().cyan());

    Ok(())
}

fn maybe_update_gitignore(data_dir: &str) -> Result<()> {
    let gitignore_path = PathBuf::from(".gitignore");
    // Normalize: strip leading ./
    let entry = data_dir.trim_start_matches("./");
    let entry_slash = format!("{}/", entry.trim_end_matches('/'));

    if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)?;
        if !content.contains(entry) {
            let updated = format!(
                "{}\n# brainjar local DBs\n{}\n",
                content.trim_end(),
                entry_slash
            );
            std::fs::write(&gitignore_path, updated)?;
            println!("  {} Added {} to {}", "\u{2713}".green(), entry_slash.cyan(), ".gitignore".dimmed());
        }
    } else {
        std::fs::write(
            &gitignore_path,
            format!("# brainjar local DBs\n{}\n", entry_slash),
        )?;
        println!("  {} Created {}", "\u{2713}".green(), ".gitignore".cyan());
    }

    Ok(())
}

fn print_next_steps() {
    println!();
    println!("  {}", "\u{2500}".repeat(46).dimmed());
    println!("  {}", "You're all set!".bold().green());
    println!("  {}", "\u{2500}".repeat(46).dimmed());
    println!();
    println!("  {}  Sync your files to the local index", "1.".bold());
    println!("     {}", "brainjar sync".cyan());
    println!();
    println!("  {}  Search your knowledge base", "2.".bold());
    println!("     {}", "brainjar search \"your query\"".cyan());
    println!();
    println!("  {}  Use as an MCP server (Claude, Cursor, etc.)", "3.".bold());
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

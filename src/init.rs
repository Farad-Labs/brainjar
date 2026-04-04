use anyhow::{Context, Result};
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use rustyline::completion::{Completer, FilenameCompleter, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config as RlConfig, Editor, Helper};
use std::path::{Path, PathBuf};

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

pub struct KbConfig {
    name: String,
    watch_paths: Vec<String>,
    description: Option<String>,
    auto_sync: bool,
}

pub struct ProviderEntry {
    name: String,   // "gemini" | "openai" | "ollama"
    api_key: String,
    base_url: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Defaults carried into the wizard for edit mode
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct WizardDefaults {
    data_dir: Option<String>,
    providers: Vec<ProviderEntry>,
    embed_provider: Option<String>,
    embed_model: Option<String>,
    embed_dimensions: Option<usize>,
    extract_provider: Option<String>,
    extract_model: Option<String>,
    knowledge_bases: Vec<KbConfig>,
}

impl WizardDefaults {
    fn from_config(config: &crate::config::Config) -> Self {
        // Convert providers map to ProviderEntry vec (sorted for stable ordering)
        let mut providers: Vec<ProviderEntry> = config
            .providers
            .iter()
            .map(|(name, p)| ProviderEntry {
                name: name.clone(),
                api_key: p.api_key.clone().unwrap_or_default(),
                base_url: p.base_url.clone().unwrap_or_default(),
            })
            .collect();
        providers.sort_by(|a, b| a.name.cmp(&b.name));

        // Convert knowledge_bases map to KbConfig vec (sorted)
        let mut knowledge_bases: Vec<KbConfig> = config
            .knowledge_bases
            .iter()
            .map(|(name, kb)| KbConfig {
                name: name.clone(),
                watch_paths: kb.watch_paths.clone(),
                description: kb.description.clone(),
                auto_sync: kb.auto_sync,
            })
            .collect();
        knowledge_bases.sort_by(|a, b| a.name.cmp(&b.name));

        WizardDefaults {
            data_dir: config.data_dir.clone(),
            providers,
            embed_provider: config.embeddings.as_ref().map(|e| e.provider.clone()),
            embed_model: config.embeddings.as_ref().map(|e| e.model.clone()),
            embed_dimensions: config.embeddings.as_ref().map(|e| e.dimensions),
            extract_provider: config.extraction.as_ref().map(|e| e.provider.clone()),
            extract_model: config.extraction.as_ref().map(|e| e.model.clone()),
            knowledge_bases,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ASCII art mascot
// ─────────────────────────────────────────────────────────────────────────────

fn print_mascot() {
    use colored::Colorize;

    // ── Glass dome (cyan walls, pink brain inside) ─────────────────
    let d1 = "         ,------------,";
    // d2–d5 are printed inline with split coloring (see below)
    let d6 = "        \\~~~~~~~~~~~~~~/";

    // ── Face collar / head band (yellow) ──────────────────────────
    let f1 = "      .-[● ● ● ● ● ● ● ●]-.";
    let f2 = "      |   (◉)       (◉)   |";
    let f3 = "      |        \\_/        |";
    let f4 = "      '-[● ● ● ● ● ● ● ●]-'";

    // ── Body (white/bold) ──────────────────────────────────────────
    let b1 = "     ----------------------";
    let b2 = "     |                    |";
    let b3 = "<{)==|    [__________]    |==(}>"; // arms at chest level
    let b4 = "     |                    |";
    let b5 = "     ----------------------";

    // ── Legs & tank treads (white) ────────────────────────────────
    let l1 = "           |        |";
    let l2 = "          [=]      [=]";
    let l3 = "         /   \\    /   \\";
    let l4 = "        [=====]  [=====]";
    let l5 = "         '----'  '----'";

    println!();

    // Dome — walls in cyan, brain interior in bright magenta (pink)
    println!("{}", d1.cyan().bold());
    print!("{}", "        /  ".cyan());
    print!("{}", "* ~~~ * ~~".bright_magenta());
    println!("{}", "  \\".cyan());
    print!("{}", "       |  ".cyan());
    print!("{}", "/~~~~~~~~~~\\".bright_magenta());
    println!("{}", "  |".cyan());
    print!("{}", "       |  ".cyan());
    print!("{}", "(~~~~~~~~~~)".bright_magenta());
    println!("{}", "  |".cyan());
    print!("{}", "       |  ".cyan());
    print!("{}", "\\__________/".bright_magenta());
    println!("{}", "  |".cyan());
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
    let max_len = lines.iter().map(|l| l.len()).max().unwrap_or(0);
    let inner_width = max_len + 4; // 2 padding each side
    let border = "\u{2550}".repeat(inner_width);
    println!("  {}{}{}", "\u{2554}".cyan(), border.cyan(), "\u{2557}".cyan());
    for line in lines {
        let pad = max_len - line.len();
        println!(
            "  {}  {}{}  {}",
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

// ─────────────────────────────────────────────────────────────────────────────
// Smart data_dir resolution
// ─────────────────────────────────────────────────────────────────────────────

/// Compute the appropriate data_dir for a given config file path.
///
/// Rules:
/// - If the config is already inside a `.brainjar` directory, use that directory
///   directly (no double-nesting).
/// - Otherwise, create a `.brainjar` subdirectory next to the config file.
pub fn resolve_data_dir(config_path: &Path) -> PathBuf {
    let config_parent = config_path.parent().unwrap_or(Path::new("."));
    if config_parent
        .file_name()
        .map(|n| n == ".brainjar")
        .unwrap_or(false)
    {
        // Config is already inside a .brainjar dir — use it directly
        config_parent.to_path_buf()
    } else {
        // Create a .brainjar subdirectory next to the config
        config_parent.join(".brainjar")
    }
}

/// Like [`resolve_data_dir`] but returns a human-readable string using `~` as
/// a shorthand for the user's home directory.
pub fn resolve_data_dir_string(config_path: &Path) -> String {
    let data_dir = resolve_data_dir(config_path);
    if let Some(home) = dirs::home_dir()
        && let Ok(rel) = data_dir.strip_prefix(&home)
    {
        let rel_str = rel.to_string_lossy();
        return if rel_str.is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", rel_str)
        };
    }
    data_dir.to_string_lossy().to_string()
}

pub async fn run_init(config_path: Option<&str>) -> Result<()> {
    print_mascot();

    println!(
        "  {}",
        "Welcome to Brainjar! Let's get your memory system set up.".bold().white()
    );
    println!();

    let theme = ColorfulTheme::default();

    // Determine the output config file path
    let resolved_config_path: PathBuf = if let Some(p) = config_path {
        PathBuf::from(p)
    } else {
        let brainjar_home = dirs::home_dir()
            .map(|h| h.join(".brainjar"))
            .unwrap_or_else(|| PathBuf::from(".brainjar"));
        brainjar_home.join("brainjar.toml")
    };

    // Guard against overwriting existing config
    let defaults: Option<WizardDefaults> = if resolved_config_path.exists() {
        println!(
            "\n  {}",
            "Config file already exists. What would you like to do?"
                .bold()
                .white()
        );
        let choices = &[
            "1. Edit    (modify existing settings)",
            "2. Overwrite  (start from scratch)",
            "3. Abort      (exit without changes)",
        ];
        let idx = Select::with_theme(&theme)
            .items(choices)
            .default(0)
            .interact()?;
        match idx {
            0 => {
                // Edit mode — load existing config and extract defaults
                match crate::config::load_config(Some(
                    resolved_config_path.to_str().unwrap_or(""),
                )) {
                    Ok(cfg) => {
                        println!(
                            "  {} Loaded existing config from {}",
                            "\u{2713}".green(),
                            resolved_config_path.display().to_string().cyan()
                        );
                        Some(WizardDefaults::from_config(&cfg))
                    }
                    Err(e) => {
                        println!(
                            "  {} Could not parse existing config: {}",
                            "!".yellow(),
                            e
                        );
                        println!(
                            "  {}",
                            "Continuing with fresh wizard (no pre-fills).".dimmed()
                        );
                        Some(WizardDefaults::default())
                    }
                }
            }
            1 => None, // Overwrite — no defaults, fresh wizard
            _ => {
                println!("{}", "  Aborted.".yellow());
                return Ok(());
            }
        }
    } else {
        None
    };

    // ── Step 1 — Storage location ─────────────────────────────────────────────
    println!("\n  {}", "Step 1 of 4 — Storage".bold().white());
    println!("  {}", "─".repeat(40).dimmed());
    println!("  {}", "Where should Brainjar store its databases?".dimmed());
    println!();

    // Compute the smart default based on where the config file will live
    let smart_data_dir = resolve_data_dir_string(&resolved_config_path);
    let opt0_label = format!("{}  (recommended based on config location)", smart_data_dir);
    let storage_choices = vec![
        opt0_label.as_str(),
        ".brainjar    (current directory — project-local)",
        "Custom path",
    ];

    // In edit mode, try to pre-select the option matching the existing data_dir
    let default_storage_idx: usize = defaults
        .as_ref()
        .and_then(|d| d.data_dir.as_ref())
        .map(|existing| {
            if existing == &smart_data_dir {
                0
            } else if existing == ".brainjar" {
                1
            } else {
                2 // Custom
            }
        })
        .unwrap_or(0);

    let storage_idx = Select::with_theme(&theme)
        .with_prompt("  Storage location")
        .items(&storage_choices)
        .default(default_storage_idx)
        .interact()?;

    let data_dir: String = match storage_idx {
        0 => smart_data_dir.clone(),
        1 => ".brainjar".to_string(),
        _ => {
            // In edit mode, pre-fill custom path with existing value
            let custom_default = defaults
                .as_ref()
                .and_then(|d| d.data_dir.clone())
                .unwrap_or_default();
            let mut builder = Input::with_theme(&theme)
                .with_prompt("  Custom path (~ is expanded)");
            if !custom_default.is_empty() {
                builder = builder.default(custom_default);
            }
            let custom: String = builder.interact_text()?;
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
        "  1. Embeddings   - converting notes into searchable vectors",
        "  2. Extraction   - finding people, projects, concepts in docs",
        "",
        "You can use any OpenAI-compatible API, or choose from the",
        "defaults below that balance cost and quality.",
        "",
        "Skip any provider you don't plan to use.",
    ]);
    println!();

    let provider_choices = &["gemini", "openai", "ollama", "other (OpenAI-compatible)"];

    // In edit mode, pre-load existing providers
    let mut providers: Vec<ProviderEntry> = if let Some(ref defs) = defaults {
        if !defs.providers.is_empty() {
            println!(
                "  {} Loaded {} existing provider(s): {}",
                "\u{2713}".green(),
                defs.providers.len(),
                defs.providers
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
                    .cyan()
            );
            defs.providers
                .iter()
                .map(|p| ProviderEntry {
                    name: p.name.clone(),
                    api_key: p.api_key.clone(),
                    base_url: p.base_url.clone(),
                })
                .collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    // In edit mode with existing providers, ask before entering the add loop
    let enter_provider_loop = if providers.is_empty() {
        true
    } else {
        Confirm::with_theme(&theme)
            .with_prompt("  Add or modify providers?")
            .default(false)
            .interact()?
    };

    if enter_provider_loop { loop {
        let idx = Select::with_theme(&theme)
            .with_prompt("  Provider to configure")
            .items(provider_choices)
            .default(0)
            .interact()?;

        let provider_name = match idx {
            0 => "gemini".to_string(),
            1 => "openai".to_string(),
            2 => "ollama (experimental)".to_string(),
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
    } } // end if enter_provider_loop / end loop

    // ── Step 3 — Model defaults ───────────────────────────────────────────────
    println!("\n  {}", "Step 3 of 4 — Model Defaults".bold().white());
    println!("  {}", "─".repeat(40).dimmed());
    println!();

    // Embedding provider
    let embed_provider_name: Option<String>;
    let embed_model: Option<String>;
    let embed_dimensions: Option<usize>;

    // With local-embed: always show embedding section (local option needs no provider)
    // Without: only show if providers are configured
    #[cfg(feature = "local-embed")]
    let has_embed_options = true;
    #[cfg(not(feature = "local-embed"))]
    let has_embed_options = !providers.is_empty();

    if !has_embed_options {
        println!("  {} Skipping embeddings (no providers configured)", "\u{2013}".dimmed());
        embed_provider_name = None;
        embed_model = None;
        embed_dimensions = None;
    } else {
        println!("  {}", "Embeddings convert your text into vectors for semantic search.".dimmed());
        println!("  {}", "This lets brainjar find results by meaning, not just keywords.".dimmed());
        #[cfg(feature = "local-embed")]
        println!("  {}", "Local = free, private, no API key needed. API providers offer higher quality for large collections.".dimmed());
        #[cfg(not(feature = "local-embed"))]
        {
            println!("  {}", "Gemini = highest quality, OpenAI = lowest cost.".dimmed());
            println!("  {}", "For ~200 docs: ~$0.25 (Gemini) or ~$0.15 (OpenAI).".dimmed());
        }
        println!();

        // Build option list; with local-embed, prepend "Local" as first/default
        let mut embed_opts: Vec<String> = Vec::new();
        #[cfg(feature = "local-embed")]
        embed_opts.push("Local (BGE-small, no API key needed)".to_string());
        embed_opts.push("none (FTS + fuzzy only)".to_string());
        embed_opts.extend(providers.iter().map(|p| p.name.clone()));
        // In edit mode, try to pre-select the existing embed provider
        let default_eidx = defaults
            .as_ref()
            .and_then(|d| d.embed_provider.as_ref())
            .and_then(|ep| embed_opts.iter().position(|o| o == ep))
            .unwrap_or(if providers.is_empty() { 0 } else { 1 });

        let eidx = Select::with_theme(&theme)
            .with_prompt("  Embedding provider")
            .items(&embed_opts)
            .default(default_eidx)
            .interact()?;

        // Index of the "none" option; shifts by 1 when local-embed prepends an entry
        #[cfg(feature = "local-embed")]
        let embed_offset: usize = 1;
        #[cfg(not(feature = "local-embed"))]
        let embed_offset: usize = 0;

        // Whether the user picked the local embedding option (only exists with local-embed)
        #[cfg(feature = "local-embed")]
        let local_picked = eidx == 0;
        #[cfg(not(feature = "local-embed"))]
        let local_picked = false;

        if local_picked {
            // Local embedding: show model picker
            let local_model_choices = &[
                "BGE-small-en-v1.5    (33M params, fastest \u{2014} recommended)",
                "BGE-base-en-v1.5     (110M params, better quality)",
                "BGE-large-en-v1.5    (335M params, highest quality BGE)",
                "Qwen3-Embedding-0.6B (600M params, best retrieval \u{2014} needs more RAM)",
                "Custom model name",
            ];
            let midx = Select::with_theme(&theme)
                .with_prompt("  Local embedding model")
                .items(local_model_choices)
                .default(0)
                .interact()?;

            let (local_model, local_dims): (String, usize) = match midx {
                0 => ("bge-small-en-v1.5".to_string(), 384),
                1 => ("bge-base-en-v1.5".to_string(), 768),
                2 => ("bge-large-en-v1.5".to_string(), 1024),
                3 => ("Qwen3-Embedding-0.6B".to_string(), 1536),
                _ => {
                    let model_name: String = Input::with_theme(&theme)
                        .with_prompt("  Model name")
                        .interact_text()?;
                    let dims_str: String = Input::with_theme(&theme)
                        .with_prompt("  Dimensions")
                        .default("768".to_string())
                        .interact_text()?;
                    let dims = dims_str.trim().parse::<usize>().unwrap_or(768);
                    (model_name.trim().to_string(), dims)
                }
            };

            embed_provider_name = Some("local".to_string());
            embed_model = Some(local_model.clone());
            embed_dimensions = Some(local_dims);
            println!(
                "  {} Embeddings: {} / {} / {} dims",
                "\u{2713}".green(),
                "local".cyan(),
                local_model.dimmed(),
                local_dims.to_string().dimmed()
            );
        } else if eidx == embed_offset {
            embed_provider_name = None;
            embed_model = None;
            embed_dimensions = None;
            println!("  {} Embeddings: none", "\u{2013}".dimmed());
            println!();
        } else {
            let pname = providers[eidx - embed_offset - 1].name.clone();

            // Model selection — multiple choice for gemini/openai, free text for ollama/other
            let model: String = match pname.as_str() {
                "gemini" => {
                    let model_choices = &[
                        "gemini-embedding-2-preview  (recommended)",
                        "gemini-embedding-001",
                        "Custom",
                    ];
                    let existing_embed_model = defaults
                        .as_ref()
                        .and_then(|d| d.embed_model.as_deref());
                    let default_midx = match existing_embed_model {
                        Some("gemini-embedding-2-preview") => 0,
                        Some("gemini-embedding-001") => 1,
                        Some(_) => 2,
                        None => 0,
                    };
                    let midx = Select::with_theme(&theme)
                        .with_prompt("  Embedding model")
                        .items(model_choices)
                        .default(default_midx)
                        .interact()?;
                    match midx {
                        0 => "gemini-embedding-2-preview".to_string(),
                        1 => "gemini-embedding-001".to_string(),
                        _ => {
                            let custom_default = existing_embed_model.unwrap_or("").to_string();
                            Input::with_theme(&theme)
                                .with_prompt("  Custom model name")
                                .default(custom_default)
                                .interact_text()?
                        }
                    }
                }
                "openai" => {
                    let model_choices = &[
                        "text-embedding-3-small  (recommended)",
                        "text-embedding-3-large",
                        "Custom",
                    ];
                    let existing_embed_model = defaults
                        .as_ref()
                        .and_then(|d| d.embed_model.as_deref());
                    let default_midx = match existing_embed_model {
                        Some("text-embedding-3-small") => 0,
                        Some("text-embedding-3-large") => 1,
                        Some(_) => 2,
                        None => 0,
                    };
                    let midx = Select::with_theme(&theme)
                        .with_prompt("  Embedding model")
                        .items(model_choices)
                        .default(default_midx)
                        .interact()?;
                    match midx {
                        0 => "text-embedding-3-small".to_string(),
                        1 => "text-embedding-3-large".to_string(),
                        _ => {
                            let custom_default = existing_embed_model.unwrap_or("").to_string();
                            Input::with_theme(&theme)
                                .with_prompt("  Custom model name")
                                .default(custom_default)
                                .interact_text()?
                        }
                    }
                }
                _ => {
                    // Ollama or other — free text
                    let existing_embed_model = defaults
                        .as_ref()
                        .and_then(|d| d.embed_model.as_deref());
                    let default_model = existing_embed_model
                        .unwrap_or_else(|| default_embed_model(&pname))
                        .to_string();
                    Input::with_theme(&theme)
                        .with_prompt(format!("  Embedding model ({})", &pname))
                        .default(default_model)
                        .interact_text()?
                }
            };

            println!();

            // Dimension selection
            let dims: usize = {
                let choices = dimension_choices(&model);
                if choices.is_empty() {
                    // Custom model — ask for dimensions via free text
                    let d: String = Input::with_theme(&theme)
                        .with_prompt("  Embedding dimensions")
                        .default(default_dimensions(&model).to_string())
                        .interact_text()?;
                    d.trim().parse::<usize>().unwrap_or_else(|_| default_dimensions(&model))
                } else {
                    // Build a labeled list; last item is always "Custom"
                    let mut dim_opts: Vec<String> = choices
                        .iter()
                        .map(|(v, recommended)| {
                            if *recommended {
                                format!("{}  (recommended)", v)
                            } else {
                                v.to_string()
                            }
                        })
                        .collect();
                    dim_opts.push("Custom".to_string());

                    let didx = Select::with_theme(&theme)
                        .with_prompt("  Embedding dimensions")
                        .items(&dim_opts)
                        .default(0)
                        .interact()?;

                    if didx == dim_opts.len() - 1 {
                        // Custom — pre-fill with existing dims if in edit mode
                        let custom_dims_default = defaults
                            .as_ref()
                            .and_then(|d| d.embed_dimensions)
                            .unwrap_or_else(|| default_dimensions(&model));
                        let d: String = Input::with_theme(&theme)
                            .with_prompt("  Custom dimensions")
                            .default(custom_dims_default.to_string())
                            .interact_text()?;
                        d.trim().parse::<usize>().unwrap_or_else(|_| default_dimensions(&model))
                    } else {
                        choices[didx].0
                    }
                }
            };

            println!(
                "  {} Embeddings: {} / {} / {} dims",
                "\u{2713}".green(),
                pname.cyan(),
                model.dimmed(),
                dims.to_string().dimmed()
            );
            embed_provider_name = Some(pname);
            embed_model = Some(model);
            embed_dimensions = Some(dims);
        }
    }

    println!(); // breathing room before next prompt

    // Extraction provider
    let extract_provider_name: Option<String>;
    let extract_model: Option<String>;

    if providers.is_empty() {
        println!("  {} Skipping extraction (no providers configured)", "\u{2013}".dimmed());
        extract_provider_name = None;
        extract_model = None;
    } else {
        println!("  {}", "Extraction uses an LLM to pull out people, projects, and".dimmed());
        println!("  {}", "relationships from your docs — powering graph search.".dimmed());
        println!("  {}", "Uses a small/cheap model. Cost is negligible (~$0.01 for 200 docs).".dimmed());
        println!();

        let mut ext_opts: Vec<String> = vec!["none (graph search disabled)".to_string()];
        ext_opts.extend(providers.iter().map(|p| p.name.clone()));
        // In edit mode, try to pre-select the existing extract provider
        let default_xidx = defaults
            .as_ref()
            .and_then(|d| d.extract_provider.as_ref())
            .and_then(|ep| ext_opts.iter().position(|o| o == ep))
            .unwrap_or(if providers.is_empty() { 0 } else { 1 });

        let xidx = Select::with_theme(&theme)
            .with_prompt("  Extraction provider")
            .items(&ext_opts)
            .default(default_xidx)
            .interact()?;

        if xidx == 0 {
            extract_provider_name = None;
            extract_model = None;
            println!("  {} Extraction: none", "\u{2013}".dimmed());
            println!();
        } else {
            let pname = providers[xidx - 1].name.clone();

            // Model selection — multiple choice for gemini/openai, free text for ollama/other
            let model: String = match pname.as_str() {
                "gemini" => {
                    let model_choices = &[
                        "gemini-3.1-flash-lite-preview  (recommended — cheapest)",
                        "gemini-3-flash-preview",
                        "gemini-3.1-pro-preview",
                        "Custom",
                    ];
                    let existing_extract_model = defaults
                        .as_ref()
                        .and_then(|d| d.extract_model.as_deref());
                    let default_midx = match existing_extract_model {
                        Some("gemini-3.1-flash-lite-preview") => 0,
                        Some("gemini-3-flash-preview") => 1,
                        Some("gemini-3.1-pro-preview") => 2,
                        Some(_) => 3,
                        None => 0,
                    };
                    let midx = Select::with_theme(&theme)
                        .with_prompt("  Extraction model")
                        .items(model_choices)
                        .default(default_midx)
                        .interact()?;
                    match midx {
                        0 => "gemini-3.1-flash-lite-preview".to_string(),
                        1 => "gemini-3-flash-preview".to_string(),
                        2 => "gemini-3.1-pro-preview".to_string(),
                        _ => {
                            let custom_default = existing_extract_model.unwrap_or("").to_string();
                            Input::with_theme(&theme)
                                .with_prompt("  Custom model name")
                                .default(custom_default)
                                .interact_text()?
                        }
                    }
                }
                "openai" => {
                    let model_choices = &[
                        "gpt-4.1-mini  (recommended)",
                        "gpt-4.1-nano",
                        "gpt-4.1",
                        "Custom",
                    ];
                    let existing_extract_model = defaults
                        .as_ref()
                        .and_then(|d| d.extract_model.as_deref());
                    let default_midx = match existing_extract_model {
                        Some("gpt-4.1-mini") => 0,
                        Some("gpt-4.1-nano") => 1,
                        Some("gpt-4.1") => 2,
                        Some(_) => 3,
                        None => 0,
                    };
                    let midx = Select::with_theme(&theme)
                        .with_prompt("  Extraction model")
                        .items(model_choices)
                        .default(default_midx)
                        .interact()?;
                    match midx {
                        0 => "gpt-4.1-mini".to_string(),
                        1 => "gpt-4.1-nano".to_string(),
                        2 => "gpt-4.1".to_string(),
                        _ => {
                            let custom_default = existing_extract_model.unwrap_or("").to_string();
                            Input::with_theme(&theme)
                                .with_prompt("  Custom model name")
                                .default(custom_default)
                                .interact_text()?
                        }
                    }
                }
                _ => {
                    // Ollama or other — free text
                    let existing_extract_model = defaults
                        .as_ref()
                        .and_then(|d| d.extract_model.as_deref());
                    let default_model = existing_extract_model
                        .unwrap_or_else(|| default_extract_model(&pname))
                        .to_string();
                    Input::with_theme(&theme)
                        .with_prompt(format!("  Extraction model ({})", &pname))
                        .default(default_model)
                        .interact_text()?
                }
            };

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

    println!(); // breathing room before next step

    // ── Step 4 — Knowledge bases ──────────────────────────────────────────────
    println!("\n  {}", "Step 4 of 4 — Knowledge Bases".bold().white());
    println!("  {}", "─".repeat(40).dimmed());
    println!(
        "  {}",
        "A knowledge base is a collection of directories/files you want indexed.".dimmed()
    );
    println!();

    // In edit mode, pre-load existing knowledge bases
    let mut knowledge_bases: Vec<KbConfig> = if let Some(ref defs) = defaults {
        if !defs.knowledge_bases.is_empty() {
            println!(
                "  {} Loaded {} existing knowledge base(s): {}",
                "\u{2713}".green(),
                defs.knowledge_bases.len(),
                defs.knowledge_bases
                    .iter()
                    .map(|kb| kb.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
                    .cyan()
            );
            defs.knowledge_bases
                .iter()
                .map(|kb| KbConfig {
                    name: kb.name.clone(),
                    watch_paths: kb.watch_paths.clone(),
                    description: kb.description.clone(),
                    auto_sync: kb.auto_sync,
                })
                .collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    // In edit mode with existing KBs, confirm before adding more
    let enter_kb_loop = if knowledge_bases.is_empty() {
        true
    } else {
        Confirm::with_theme(&theme)
            .with_prompt("  Add more knowledge bases?")
            .default(false)
            .interact()?
    };

    if enter_kb_loop { loop {
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
    } } // end if enter_kb_loop / end loop

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
        .map(|p| {
            let dims_str = embed_dimensions.map(|d| format!(" / {} dims", d)).unwrap_or_default();
            format!("{} / {}{}", p, embed_model.as_deref().unwrap_or(""), dims_str)
        })
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
        &resolved_config_path,
        &data_dir,
        &providers,
        embed_provider_name.as_deref(),
        embed_model.as_deref(),
        embed_dimensions,
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
    println!(
        "  {} Config path: {}",
        "\u{2139}".cyan(),
        resolved_config_path.display().to_string().dimmed()
    );

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
pub fn generate_brainjar_toml(
    config_path: &PathBuf,
    data_dir: &str,
    providers: &[ProviderEntry],
    embed_provider: Option<&str>,
    embed_model: Option<&str>,
    embed_dimensions: Option<usize>,
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
        let dims = embed_dimensions.unwrap_or_else(|| default_dimensions(model));
        toml.push_str(&format!("dimensions = {}\n\n", dims));
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

    if let Some(parent) = config_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }
    std::fs::write(config_path, &toml)
        .with_context(|| format!("Failed to write {}", config_path.display()))?;
    println!("  {} Config written to {}", "\u{2713}".green(), config_path.display().to_string().cyan());

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

fn default_dimensions(model: &str) -> usize {
    match model {
        "gemini-embedding-2-preview" => 3072,
        "gemini-embedding-001" => 768,
        "text-embedding-3-small" => 1536,
        "text-embedding-3-large" => 3072,
        _ => 768, // safe fallback
    }
}

fn dimension_choices(model: &str) -> Vec<(usize, bool)> {
    // Returns (dimension_value, is_recommended)
    match model {
        "gemini-embedding-2-preview" => vec![(3072, true), (768, false)],
        "gemini-embedding-001" => vec![(768, true)],
        "text-embedding-3-small" => vec![(1536, true)],
        "text-embedding-3-large" => vec![(3072, true), (1024, false)],
        _ => vec![], // custom model = ask for dimensions directly
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_data_dir_default_location() {
        // ~/.brainjar/brainjar.toml → ~/.brainjar
        let home = dirs::home_dir().unwrap();
        let config = home.join(".brainjar").join("brainjar.toml");
        let result = resolve_data_dir(&config);
        assert_eq!(result, home.join(".brainjar"));
    }

    #[test]
    fn test_resolve_data_dir_already_in_brainjar_dir() {
        // /tmp/myproject/.brainjar/brainjar.toml → /tmp/myproject/.brainjar
        let config = Path::new("/tmp/myproject/.brainjar/brainjar.toml");
        let result = resolve_data_dir(config);
        assert_eq!(result, Path::new("/tmp/myproject/.brainjar"));
    }

    #[test]
    fn test_resolve_data_dir_no_double_nesting() {
        // Ensure we never get .brainjar/.brainjar
        let config = Path::new("/tmp/test/.brainjar/config.toml");
        let result = resolve_data_dir(config);
        assert!(!result.to_string_lossy().contains(".brainjar/.brainjar"));
    }

    #[test]
    fn test_resolve_data_dir_root_path() {
        // /brainjar.toml → /.brainjar
        let config = Path::new("/brainjar.toml");
        let result = resolve_data_dir(config);
        assert_eq!(result, Path::new("/.brainjar"));
    }

    #[test]
    fn test_resolve_data_dir_default_home() {
        // Config inside .brainjar dir → use that dir directly (no nesting)
        let config = Path::new("/home/user/.brainjar/brainjar.toml");
        let result = resolve_data_dir(config);
        assert_eq!(result, Path::new("/home/user/.brainjar"));
    }

    #[test]
    fn test_resolve_data_dir_custom_location() {
        // Config in a regular dir → create .brainjar subdir
        let config = Path::new("/home/user/experiments/test.toml");
        let result = resolve_data_dir(config);
        assert_eq!(result, Path::new("/home/user/experiments/.brainjar"));
    }

    #[test]
    fn test_resolve_data_dir_nested_brainjar() {
        // Config already inside .brainjar → no double nesting
        let config = Path::new("/home/user/myproject/.brainjar/brainjar.toml");
        let result = resolve_data_dir(config);
        assert_eq!(result, Path::new("/home/user/myproject/.brainjar"));
    }

    #[test]
    fn test_resolve_data_dir_tmp_custom() {
        let config = Path::new("/tmp/brainjar-test-custom/test.toml");
        let result = resolve_data_dir(config);
        assert_eq!(result, Path::new("/tmp/brainjar-test-custom/.brainjar"));
    }

    #[test]
    fn test_resolve_data_dir_tmp_nested() {
        let config = Path::new("/tmp/brainjar-test-nested/.brainjar/brainjar.toml");
        let result = resolve_data_dir(config);
        assert_eq!(result, Path::new("/tmp/brainjar-test-nested/.brainjar"));
    }

    #[test]
    fn test_resolve_data_dir_string_home() {
        if let Some(home) = dirs::home_dir() {
            // Default case: config in ~/.brainjar/
            let config = home.join(".brainjar").join("brainjar.toml");
            let result = resolve_data_dir_string(&config);
            assert_eq!(result, "~/.brainjar");
        }
    }

    #[test]
    fn test_resolve_data_dir_string_custom_home_subdir() {
        if let Some(home) = dirs::home_dir() {
            // Custom config in ~/experiments/
            let config = home.join("experiments").join("test.toml");
            let result = resolve_data_dir_string(&config);
            assert_eq!(result, "~/experiments/.brainjar");
        }
    }

    #[test]
    fn test_resolve_data_dir_string_absolute_non_home() {
        // /tmp path → no ~ prefix, just absolute
        let config = Path::new("/tmp/brainjar-test/test.toml");
        let result = resolve_data_dir_string(config);
        assert_eq!(result, "/tmp/brainjar-test/.brainjar");
    }

    #[test]
    fn test_resolve_data_dir_string_nested_non_home() {
        let config = Path::new("/tmp/proj/.brainjar/brainjar.toml");
        let result = resolve_data_dir_string(config);
        assert_eq!(result, "/tmp/proj/.brainjar");
    }

    #[test]
    fn test_generate_toml_data_dir_custom() {
        // Verify generate_brainjar_toml writes the data_dir we pass in
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("test.toml");
        generate_brainjar_toml(
            &config_path,
            "/tmp/brainjar-test-custom/.brainjar",
            &[],
            None,
            None,
            None,
            None,
            None,
            &[],
        )
        .unwrap();
        let content = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            content.contains("data_dir = \"/tmp/brainjar-test-custom/.brainjar\""),
            "Expected data_dir in toml, got: {}",
            content
        );
    }

    #[test]
    fn test_generate_toml_data_dir_tilde() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("test.toml");
        generate_brainjar_toml(
            &config_path,
            "~/.brainjar",
            &[],
            None,
            None,
            None,
            None,
            None,
            &[],
        )
        .unwrap();
        let content = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            content.contains("data_dir = \"~/.brainjar\""),
            "Expected data_dir in toml, got: {}",
            content
        );
    }

    #[test]
    fn test_no_collision_between_separate_configs() {
        // Two different config paths should resolve to different data_dirs
        let config1 = Path::new("/tmp/proj1/brainjar.toml");
        let config2 = Path::new("/tmp/proj2/brainjar.toml");
        let dir1 = resolve_data_dir(config1);
        let dir2 = resolve_data_dir(config2);
        assert_ne!(dir1, dir2);
        assert_eq!(dir1, Path::new("/tmp/proj1/.brainjar"));
        assert_eq!(dir2, Path::new("/tmp/proj2/.brainjar"));
    }
}

fn default_embed_model(provider: &str) -> &'static str {
    match provider {
        "gemini" => "gemini-embedding-2-preview",
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

use anyhow::{Context, Result};
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm, Input};
use rustyline::{hint::Hinter, validate::Validator, Editor};
use rustyline::completion::{Completer, FilenameCompleter, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::{CompletionType, Config as RlConfig, Helper};
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
    description: String,
    watch_paths: Vec<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

pub async fn run_init() -> Result<()> {
    println!("\n{}", "🧠 brainjar init".cyan().bold());
    println!(
        "{}\n",
        "Interactive wizard — generates Terraform infrastructure + brainjar.toml".dimmed()
    );

    let theme = ColorfulTheme::default();

    // ── Guard against overwriting existing config ─────────────────────────────
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

    // ── AWS config ────────────────────────────────────────────────────────────
    println!("{}", "── AWS configuration ──────────────────────────────".dimmed());

    let aws_profile: String = Input::with_theme(&theme)
        .with_prompt("AWS profile name")
        .default("default".to_string())
        .interact_text()?;

    let aws_region: String = Input::with_theme(&theme)
        .with_prompt("AWS region")
        .default("us-east-1".to_string())
        .interact_text()?;

    // ── Knowledge bases ───────────────────────────────────────────────────────
    println!("\n{}", "── Knowledge bases ────────────────────────────────".dimmed());

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

        let description: String = Input::with_theme(&theme)
            .with_prompt("  Description")
            .interact_text()?;

        // Tab-completing path input via rustyline
        println!("  {}", "Watch paths — tab-complete enabled, empty line to finish:".dimmed());
        let watch_paths = prompt_watch_paths()?;

        if watch_paths.is_empty() {
            println!(
                "  {}",
                "Warning: no watch paths set. Add them manually to brainjar.toml.".yellow()
            );
        }

        knowledge_bases.push(KbConfig {
            name,
            description,
            watch_paths,
        });
    }

    // ── Terraform output directory ─────────────────────────────────────────────
    println!("\n{}", "── Terraform output ───────────────────────────────".dimmed());

    let tf_dir: String = Input::with_theme(&theme)
        .with_prompt("Terraform output directory")
        .default("./infrastructure".to_string())
        .interact_text()?;

    // ── Generate everything ────────────────────────────────────────────────────
    println!();
    generate_terraform(&tf_dir, &aws_profile, &aws_region, &knowledge_bases)?;
    generate_brainjar_toml(&aws_profile, &aws_region, &tf_dir, &knowledge_bases)?;
    print_next_steps(&tf_dir);

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
// Terraform generation
// ─────────────────────────────────────────────────────────────────────────────

fn generate_terraform(
    tf_dir: &str,
    aws_profile: &str,
    aws_region: &str,
    kbs: &[KbConfig],
) -> Result<()> {
    std::fs::create_dir_all(tf_dir)
        .with_context(|| format!("Failed to create Terraform directory: {tf_dir}"))?;

    std::fs::write(format!("{tf_dir}/main.tf"), render_main_tf(aws_profile, aws_region))
        .context("Failed to write main.tf")?;

    std::fs::write(format!("{tf_dir}/variables.tf"), render_variables_tf(aws_profile, aws_region, kbs))
        .context("Failed to write variables.tf")?;

    std::fs::write(format!("{tf_dir}/s3.tf"), render_s3_tf())
        .context("Failed to write s3.tf")?;

    std::fs::write(format!("{tf_dir}/bedrock.tf"), render_bedrock_tf())
        .context("Failed to write bedrock.tf")?;

    std::fs::write(format!("{tf_dir}/iam.tf"), render_iam_tf())
        .context("Failed to write iam.tf")?;

    std::fs::write(format!("{tf_dir}/outputs.tf"), render_outputs_tf())
        .context("Failed to write outputs.tf")?;

    println!("{} Generated Terraform in {}", "✓".green(), tf_dir.cyan());
    for file in &["main.tf", "variables.tf", "s3.tf", "bedrock.tf", "iam.tf", "outputs.tf"] {
        println!("    {}", format!("{tf_dir}/{file}").dimmed());
    }

    Ok(())
}

fn render_main_tf(aws_profile: &str, aws_region: &str) -> String {
    format!(
        r#"# ──────────────────────────────────────────────────────────────────────────
# brainjar — generated by `brainjar init`
# ──────────────────────────────────────────────────────────────────────────

terraform {{
  required_version = ">= 1.5"

  required_providers {{
    aws = {{
      source  = "hashicorp/aws"
      version = ">= 5.82.0"
    }}
    awscc = {{
      source  = "hashicorp/awscc"
      version = ">= 1.0.0"
    }}
  }}

  # Uncomment and configure to use remote state:
  # backend "s3" {{
  #   bucket  = "your-tfstate-bucket"
  #   key     = "brainjar/terraform.tfstate"
  #   region  = "{aws_region}"
  #   encrypt = true
  #   profile = "{aws_profile}"
  # }}
}}

provider "aws" {{
  region  = var.region
  profile = var.aws_profile

  default_tags {{
    tags = {{
      Project   = "brainjar"
      ManagedBy = "terraform"
    }}
  }}
}}

provider "awscc" {{
  region  = var.region
  profile = var.aws_profile
}}

data "aws_caller_identity" "current" {{}}
"#
    )
}

fn render_variables_tf(aws_profile: &str, aws_region: &str, kbs: &[KbConfig]) -> String {
    // Build the knowledge_bases default block
    let mut kb_entries = String::new();
    for (i, kb) in kbs.iter().enumerate() {
        // We can't reference account_id in Terraform variable defaults,
        // so use a stable short naming pattern the user can rename after apply.
        let comma = if i + 1 < kbs.len() { "," } else { "" };
        kb_entries.push_str(&format!(
            r#"    {name} = {{
      description   = "{desc}"
      source_bucket = "brainjar-{name}-source"
      vector_bucket = "brainjar-{name}-vectors"
    }}{comma}
"#,
            name = kb.name,
            desc = escape_hcl(&kb.description),
            comma = comma,
        ));
    }

    format!(
        r#"variable "region" {{
  description = "AWS region"
  type        = string
  default     = "{aws_region}"
}}

variable "aws_profile" {{
  description = "AWS CLI profile to use"
  type        = string
  default     = "{aws_profile}"
}}

variable "embedding_model_id" {{
  description = "Bedrock embedding model ID"
  type        = string
  default     = "amazon.titan-embed-text-v2:0"
}}

variable "embedding_dimensions" {{
  description = "Embedding vector dimensions"
  type        = number
  default     = 1024
}}

variable "knowledge_bases" {{
  description = "Map of knowledge base configs"
  type = map(object({{
    description   = string
    source_bucket = string
    vector_bucket = string
  }}))
  default = {{
{kb_entries}  }}
}}
"#
    )
}

fn render_s3_tf() -> String {
    r#"# ──────────────────────────────────────────────────────────────────────────
# S3 — Source document buckets (markdown files synced from local)
# ──────────────────────────────────────────────────────────────────────────

resource "aws_s3_bucket" "kb_source" {
  for_each = var.knowledge_bases
  bucket   = each.value.source_bucket
}

resource "aws_s3_bucket_versioning" "kb_source" {
  for_each = var.knowledge_bases
  bucket   = aws_s3_bucket.kb_source[each.key].id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "kb_source" {
  for_each = var.knowledge_bases
  bucket   = aws_s3_bucket.kb_source[each.key].id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_public_access_block" "kb_source" {
  for_each = var.knowledge_bases
  bucket   = aws_s3_bucket.kb_source[each.key].id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# ──────────────────────────────────────────────────────────────────────────
# S3 Vectors — Vector buckets + indexes (via awscc provider)
# ──────────────────────────────────────────────────────────────────────────

resource "awscc_s3vectors_vector_bucket" "kb_vectors" {
  for_each           = var.knowledge_bases
  vector_bucket_name = each.value.vector_bucket
}

resource "awscc_s3vectors_index" "kb_index" {
  for_each           = var.knowledge_bases
  vector_bucket_name = awscc_s3vectors_vector_bucket.kb_vectors[each.key].vector_bucket_name
  index_name         = "${each.key}-index"

  dimension       = var.embedding_dimensions
  distance_metric = "cosine"
  data_type       = "float32"

  # AMAZON_BEDROCK_TEXT must be non-filterable — filterable metadata has a 2KB
  # limit in S3 Vectors, and chunk text easily exceeds that.
  metadata_configuration = {
    non_filterable_metadata_keys = ["AMAZON_BEDROCK_TEXT", "AMAZON_BEDROCK_METADATA"]
  }
}
"#
    .to_string()
}

fn render_bedrock_tf() -> String {
    r#"# ──────────────────────────────────────────────────────────────────────────
# Bedrock Knowledge Bases (via awscc provider — supports S3 Vectors backend)
# ──────────────────────────────────────────────────────────────────────────

resource "awscc_bedrock_knowledge_base" "kb" {
  for_each = var.knowledge_bases

  name        = each.key
  description = each.value.description
  role_arn    = aws_iam_role.bedrock_kb.arn
  depends_on  = [aws_iam_role_policy.bedrock_kb_access]

  knowledge_base_configuration = {
    type = "VECTOR"

    vector_knowledge_base_configuration = {
      embedding_model_arn = "arn:aws:bedrock:${var.region}::foundation-model/${var.embedding_model_id}"

      embedding_model_configuration = {
        bedrock_embedding_model_configuration = {
          dimensions          = var.embedding_dimensions
          embedding_data_type = "FLOAT32"
        }
      }
    }
  }

  storage_configuration = {
    type = "S3_VECTORS"

    s3_vectors_configuration = {
      vector_bucket_arn = awscc_s3vectors_vector_bucket.kb_vectors[each.key].vector_bucket_arn
      index_arn         = awscc_s3vectors_index.kb_index[each.key].index_arn
    }
  }
}

# ──────────────────────────────────────────────────────────────────────────
# Bedrock Data Sources — S3 source buckets linked to each KB
# ──────────────────────────────────────────────────────────────────────────

resource "awscc_bedrock_data_source" "s3_source" {
  for_each = var.knowledge_bases

  name              = "${each.key}-s3-source"
  knowledge_base_id = awscc_bedrock_knowledge_base.kb[each.key].knowledge_base_id

  data_source_configuration = {
    type = "S3"

    s3_configuration = {
      bucket_arn = aws_s3_bucket.kb_source[each.key].arn
    }
  }

  vector_ingestion_configuration = {
    chunking_configuration = {
      chunking_strategy = "NONE"
    }
  }
}
"#
    .to_string()
}

fn render_iam_tf() -> String {
    r#"# ──────────────────────────────────────────────────────────────────────────
# IAM — Bedrock service role (for KB to access S3 + embedding model)
# ──────────────────────────────────────────────────────────────────────────

resource "aws_iam_role" "bedrock_kb" {
  name = "bedrock-knowledge-base-role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect    = "Allow"
        Principal = { Service = "bedrock.amazonaws.com" }
        Action    = "sts:AssumeRole"
        Condition = {
          StringEquals = {
            "aws:SourceAccount" = data.aws_caller_identity.current.account_id
          }
        }
      }
    ]
  })
}

resource "aws_iam_role_policy" "bedrock_kb_access" {
  name = "bedrock-kb-access"
  role = aws_iam_role.bedrock_kb.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      # Read source documents from S3
      {
        Effect   = "Allow"
        Action   = ["s3:GetObject", "s3:ListBucket"]
        Resource = flatten([
          for key, kb in var.knowledge_bases : [
            aws_s3_bucket.kb_source[key].arn,
            "${aws_s3_bucket.kb_source[key].arn}/*"
          ]
        ])
      },
      # Read/write S3 Vectors
      {
        Effect   = "Allow"
        Action   = ["s3vectors:*"]
        Resource = flatten([
          for key, kb in var.knowledge_bases : [
            awscc_s3vectors_vector_bucket.kb_vectors[key].vector_bucket_arn,
            "${awscc_s3vectors_vector_bucket.kb_vectors[key].vector_bucket_arn}/*"
          ]
        ])
      },
      # Invoke embedding model
      {
        Effect   = "Allow"
        Action   = ["bedrock:InvokeModel"]
        Resource = [
          "arn:aws:bedrock:${var.region}::foundation-model/${var.embedding_model_id}"
        ]
      }
    ]
  })
}

# ──────────────────────────────────────────────────────────────────────────
# IAM — Agent user (brainjar-agent, for CLI access to query and sync)
# ──────────────────────────────────────────────────────────────────────────

resource "aws_iam_user" "brainjar_agent" {
  name = "brainjar-agent"

  tags = {
    Description = "Agent access for Bedrock KB queries and S3 document sync"
  }
}

resource "aws_iam_access_key" "brainjar_agent" {
  user = aws_iam_user.brainjar_agent.name
}

resource "aws_iam_user_policy" "brainjar_agent" {
  name = "brainjar-agent-policy"
  user = aws_iam_user.brainjar_agent.name

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      # S3: sync source documents
      {
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:PutObject",
          "s3:DeleteObject",
          "s3:ListBucket"
        ]
        Resource = flatten([
          for key, kb in var.knowledge_bases : [
            aws_s3_bucket.kb_source[key].arn,
            "${aws_s3_bucket.kb_source[key].arn}/*"
          ]
        ])
      },
      # Bedrock: retrieve from knowledge bases
      {
        Effect   = "Allow"
        Action   = ["bedrock:Retrieve"]
        Resource = [
          "arn:aws:bedrock:${var.region}:${data.aws_caller_identity.current.account_id}:knowledge-base/*"
        ]
      },
      # Bedrock: manage ingestion jobs and inspect KB/data source metadata
      {
        Effect = "Allow"
        Action = [
          "bedrock:StartIngestionJob",
          "bedrock:GetIngestionJob",
          "bedrock:ListIngestionJobs",
          "bedrock:GetKnowledgeBase",
          "bedrock:GetDataSource",
          "bedrock-agent:StartIngestionJob",
          "bedrock-agent:GetIngestionJob",
          "bedrock-agent:ListIngestionJobs",
          "bedrock-agent:GetKnowledgeBase",
          "bedrock-agent:GetDataSource",
          "bedrock-agent:UpdateDataSource",
          "bedrock:UpdateDataSource"
        ]
        Resource = [
          "arn:aws:bedrock:${var.region}:${data.aws_caller_identity.current.account_id}:knowledge-base/*"
        ]
      }
    ]
  })
}
"#
    .to_string()
}

fn render_outputs_tf() -> String {
    r#"output "source_buckets" {
  description = "Source document bucket names — copy into brainjar.toml s3_bucket fields"
  value = {
    for key, bucket in aws_s3_bucket.kb_source : key => bucket.bucket
  }
}

output "knowledge_bases" {
  description = "Bedrock Knowledge Base IDs — copy into brainjar.toml kb_id fields"
  value = {
    for key, kb in awscc_bedrock_knowledge_base.kb : key => {
      id  = kb.knowledge_base_id
      arn = kb.knowledge_base_arn
    }
  }
}

output "data_sources" {
  description = "Bedrock Data Source IDs — copy into brainjar.toml data_source_id fields"
  value = {
    for key, ds in awscc_bedrock_data_source.s3_source : key => {
      id                = ds.data_source_id
      knowledge_base_id = ds.knowledge_base_id
    }
  }
}

output "vector_buckets" {
  description = "S3 Vector bucket and index details"
  value = {
    for key, vb in awscc_s3vectors_vector_bucket.kb_vectors : key => {
      bucket_name = vb.vector_bucket_name
      bucket_arn  = vb.vector_bucket_arn
      index_name  = awscc_s3vectors_index.kb_index[key].index_name
      index_arn   = awscc_s3vectors_index.kb_index[key].index_arn
    }
  }
}

output "bedrock_role_arn" {
  description = "IAM role ARN used by Bedrock KB service"
  value       = aws_iam_role.bedrock_kb.arn
}

output "agent_access_key_id" {
  description = "Access key ID for brainjar-agent — add to brainjar.toml or AWS credentials"
  value       = aws_iam_access_key.brainjar_agent.id
}

output "agent_secret_access_key" {
  description = "Secret access key for brainjar-agent — store in 1Password!"
  value       = aws_iam_access_key.brainjar_agent.secret
  sensitive   = true
}
"#
    .to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// brainjar.toml generation
// ─────────────────────────────────────────────────────────────────────────────

fn generate_brainjar_toml(
    aws_profile: &str,
    aws_region: &str,
    tf_dir: &str,
    kbs: &[KbConfig],
) -> Result<()> {
    let mut toml = format!(
        r#"# brainjar.toml — Knowledge base configuration
# Generated by `brainjar init`
#
# NEXT STEPS:
#   1. cd {tf_dir} && terraform init && terraform apply
#   2. Run: terraform output -json
#   3. Fill in the TODO values below from the terraform output
#   4. Run: brainjar sync

[aws]
profile = "{aws_profile}"
region  = "{aws_region}"

"#
    );

    for kb in kbs {
        let watch_paths_toml = if kb.watch_paths.is_empty() {
            r#"[]  # TODO: add paths to watch"#.to_string()
        } else {
            let entries: Vec<String> = kb
                .watch_paths
                .iter()
                .map(|p| format!("\"{}\"", p.replace('"', "\\\"")))
                .collect();
            format!("[{}]", entries.join(", "))
        };

        toml.push_str(&format!(
            r#"[knowledge_bases.{name}]
# TODO: run `terraform output -json` and fill these in
kb_id          = "TODO"  # knowledge_bases.{name}.id
data_source_id = "TODO"  # data_sources.{name}.id
s3_bucket      = "TODO"  # source_buckets.{name}
watch_paths    = {watch_paths}
auto_sync      = true

"#,
            name = kb.name,
            watch_paths = watch_paths_toml,
        ));
    }

    std::fs::write("brainjar.toml", &toml).context("Failed to write brainjar.toml")?;
    println!("{} Generated {}", "✓".green(), "brainjar.toml".cyan());

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Next steps banner
// ─────────────────────────────────────────────────────────────────────────────

fn print_next_steps(tf_dir: &str) {
    println!("\n{}", "──────────────────────────────────────────────────".dimmed());
    println!("{}", "  Next steps".bold().white());
    println!("{}", "──────────────────────────────────────────────────".dimmed());
    println!(
        "\n  {}  Deploy infrastructure\n",
        "1.".bold()
    );
    println!("     {}", format!("cd {tf_dir}").cyan());
    println!("     {}", "terraform init".cyan());
    println!("     {}", "terraform apply".cyan());
    println!(
        "\n  {}  Copy outputs into brainjar.toml\n",
        "2.".bold()
    );
    println!("     {}", "terraform output -json".cyan());
    println!(
        "     {}",
        "Fill in the kb_id, data_source_id, and s3_bucket fields marked TODO".dimmed()
    );
    println!(
        "\n  {}  Store the agent credentials\n",
        "3.".bold()
    );
    println!(
        "     {}",
        "terraform output -raw agent_secret_access_key | pbcopy".cyan()
    );
    println!(
        "     {}",
        "Save the access key ID and secret to 1Password as \"brainjar-agent\"".dimmed()
    );
    println!(
        "\n  {}  Start syncing\n",
        "4.".bold()
    );
    println!("     {}", "brainjar sync".cyan());
    println!();
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Escape double-quotes for HCL string values.
fn escape_hcl(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

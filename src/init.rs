use anyhow::{Context, Result};
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm, Input};
use std::path::PathBuf;

pub async fn run_init() -> Result<()> {
    println!("\n{}", "🧠 brainjar init".cyan().bold());
    println!("{}\n", "Set up a new knowledge base backed by AWS Bedrock + S3".dimmed());

    let theme = ColorfulTheme::default();

    // Check if config already exists
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

    // Gather config
    let aws_profile: String = Input::with_theme(&theme)
        .with_prompt("AWS profile name (leave empty to use env vars)")
        .allow_empty(true)
        .interact_text()?;

    let aws_region: String = Input::with_theme(&theme)
        .with_prompt("AWS region")
        .default("us-east-1".to_string())
        .interact_text()?;

    let kb_name: String = Input::with_theme(&theme)
        .with_prompt("Knowledge base name (e.g. 'memory')")
        .default("memory".to_string())
        .interact_text()?;

    let kb_id: String = Input::with_theme(&theme)
        .with_prompt("Bedrock KB ID (from AWS console, or press Enter to fill in later)")
        .allow_empty(true)
        .interact_text()?;

    let ds_id: String = Input::with_theme(&theme)
        .with_prompt("Data source ID (or press Enter to fill in later)")
        .allow_empty(true)
        .interact_text()?;

    let s3_bucket: String = Input::with_theme(&theme)
        .with_prompt("S3 bucket name (or press Enter to fill in later)")
        .allow_empty(true)
        .interact_text()?;

    let scaffold_terraform = Confirm::with_theme(&theme)
        .with_prompt("Scaffold Terraform templates for AWS infrastructure?")
        .default(true)
        .interact()?;

    // Write brainjar.toml
    let profile_line = if aws_profile.is_empty() {
        "# profile = \"your-aws-profile\"  # uncomment to use a named profile".to_string()
    } else {
        format!("profile = \"{}\"", aws_profile)
    };

    let kb_id_val = if kb_id.is_empty() { "YOUR_KB_ID".to_string() } else { kb_id };
    let ds_id_val = if ds_id.is_empty() { "YOUR_DATA_SOURCE_ID".to_string() } else { ds_id };
    let bucket_val = if s3_bucket.is_empty() { "your-brainjar-bucket".to_string() } else { s3_bucket };

    let config_content = format!(
        r#"# brainjar.toml — Knowledge base configuration
# See: https://github.com/farad-labs/brainjar

[aws]
{profile_line}
region = "{region}"

# Named knowledge bases — add as many as you need
[knowledge_bases.{kb_name}]
kb_id = "{kb_id}"
data_source_id = "{ds_id}"
s3_bucket = "{bucket}"
watch_paths = ["memory/", "MEMORY.md"]
auto_sync = true   # sync this KB when brainjar sync is run without a name
"#,
        profile_line = profile_line,
        region = aws_region,
        kb_name = kb_name,
        kb_id = kb_id_val,
        ds_id = ds_id_val,
        bucket = bucket_val,
    );

    std::fs::write("brainjar.toml", &config_content)
        .context("Failed to write brainjar.toml")?;
    println!("\n{} Created {}", "✓".green(), "brainjar.toml".cyan());

    // Create .brainjar directory
    std::fs::create_dir_all(".brainjar")?;
    std::fs::write(
        ".brainjar/.gitignore",
        "# brainjar state — commit this if you want sync state in git\n# state.json\n",
    )?;

    if scaffold_terraform {
        scaffold_terraform_templates(&kb_name, &aws_region)?;
    }

    println!("\n{}", "Next steps:".bold().white());
    println!(
        "  1. {}",
        "Edit brainjar.toml with your KB ID and data source ID".dimmed()
    );
    if scaffold_terraform {
        println!(
            "  2. {} {}",
            "Run Terraform to create AWS infrastructure:".dimmed(),
            "cd terraform && terraform init && terraform apply".cyan()
        );
        println!(
            "  3. {} {}",
            "Copy the output values into brainjar.toml:".dimmed(),
            "kb_id, data_source_id, s3_bucket".cyan()
        );
        println!(
            "  4. {}",
            "Run brainjar sync to upload files and start ingestion".dimmed()
        );
    } else {
        println!(
            "  2. {}",
            "Run brainjar sync to upload files and start ingestion".dimmed()
        );
    }
    println!();

    Ok(())
}

fn scaffold_terraform_templates(kb_name: &str, region: &str) -> Result<()> {
    std::fs::create_dir_all("terraform")?;

    // main.tf
    let main_tf = include_str!("terraform/main.tf.tmpl")
        .replace("{{KB_NAME}}", kb_name)
        .replace("{{REGION}}", region);
    std::fs::write("terraform/main.tf", main_tf)?;

    // variables.tf
    let vars_tf = include_str!("terraform/variables.tf.tmpl")
        .replace("{{KB_NAME}}", kb_name)
        .replace("{{REGION}}", region);
    std::fs::write("terraform/variables.tf", vars_tf)?;

    // outputs.tf
    std::fs::write("terraform/outputs.tf", include_str!("terraform/outputs.tf.tmpl"))?;

    // README
    std::fs::write("terraform/README.md", terraform_readme())?;

    println!("{} Scaffolded {}", "✓".green(), "terraform/".cyan());
    Ok(())
}

fn terraform_readme() -> &'static str {
    r#"# brainjar Terraform

This directory contains Terraform configuration to create the AWS infrastructure
needed by brainjar.

## Prerequisites

- [Terraform](https://terraform.io) >= 1.5
- [AWS CLI](https://aws.amazon.com/cli/) configured with appropriate credentials
- AWS account with Bedrock model access enabled for `amazon.titan-embed-text-v2:0`

## Usage

```bash
# Initialize providers
terraform init

# Review plan
terraform plan

# Apply (creates all resources)
terraform apply

# Copy outputs to brainjar.toml
terraform output
```

## Resources Created

- **S3 bucket** — stores source documents for Bedrock ingestion
- **S3 Vectors index** — vector storage backend (via awscc provider)
- **Bedrock Knowledge Base** — the KB with Titan Embed V2
- **Bedrock Data Source** — connects S3 bucket to the KB
- **IAM roles** — Bedrock service role with S3 and S3 Vectors access

## Notes

- Uses `awscc` provider for S3 Vectors (not yet in the `aws` provider)
- Chunking strategy is NONE — each file = one vector
- Non-filterable metadata fields are configured in the vector index
- Embedding dimension: 1024 (Titan Embed V2)
"#
}

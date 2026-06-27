mod compose;
mod model;
mod schema;
mod seg;
mod translate;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "transpaper", about = "Translate English PDF papers to Japanese")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(short, long, help = "Input PDF file path")]
    input: Option<PathBuf>,

    #[arg(short, long, help = "Output PDF file path")]
    output: Option<PathBuf>,

    #[arg(short, long, default_value = "deepl", help = "Translation model (deepl, idx)")]
    model: String,

    #[arg(short, long, default_value_t = false, help = "Create comparison PDF")]
    compare: bool,

    #[arg(long, default_value_t = false, help = "Disable duplicate segment deduplication")]
    no_dedup: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// PoC: Test PDF rendering
    PocPdf {
        #[arg(short, long)]
        input: PathBuf,
        #[arg(short, long, default_value = "/tmp/poc_pdf_output")]
        output: PathBuf,
    },
    /// PoC: Run ONNX inference on a PNG image
    PocInfer {
        #[arg(short, long)]
        model: PathBuf,
        #[arg(short, long)]
        image: PathBuf,
        #[arg(short, long, default_value = "poc_output.json")]
        output: PathBuf,
        #[arg(long, default_value_t = 0.25)]
        conf: f64,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    if let Some(command) = cli.command {
        return run_subcommand(command);
    }

    let input = cli
        .input
        .ok_or_else(|| anyhow::anyhow!("--input is required"))?;

    let dedup_enabled = !cli.no_dedup;
    run_pipeline(&input, cli.output, &cli.model, cli.compare, dedup_enabled)
}

fn run_pipeline(
    input: &std::path::Path,
    output: Option<PathBuf>,
    model_name: &str,
    compare: bool,
    dedup_enabled: bool,
) -> anyhow::Result<()> {
    let input_stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "output".to_string());

    let output_path = output.unwrap_or_else(|| {
        if compare {
            PathBuf::from(format!("translated_{input_stem}_compare.pdf"))
        } else {
            PathBuf::from(format!("translated_{input_stem}.pdf"))
        }
    });

    println!("Input:   {}", input.display());
    println!("Output:  {}", output_path.display());
    println!("Model:   {model_name}");
    if compare {
        println!("Mode:    compare");
    }
    println!();

    let working_dir = PathBuf::from(format!("/tmp/_{input_stem}"));
    std::fs::create_dir_all(&working_dir)?;

    let n = if compare { 5 } else { 4 };

    // 1. Segmentation
    println!("({}/{n}) Segmentation...", 1);
    let seg_dir = working_dir.join("segments");
    let mut seg_results = seg::segment_pdf(input, &seg_dir, 150, 0.25, None)?;
    println!("  → {} pages, {} blocks detected",
        seg_results.len(),
        seg_results.iter().map(|p| p.blocks.len()).sum::<usize>());

    // Extract text metadata
    for i in 0..seg_results.len() {
        let page_idx = seg_results[i].page - 1;
        seg::extract_text_metadata(input, page_idx, &mut seg_results[i..=i])?;
    }

    // 2. Translation
    println!("({}/{n}) Translation ({model_name})...", 2);
    let translated_dir = working_dir.join("translated");
    let auth_key = std::env::var("DEEPL_API").ok();
    let translation_ok = translate::translate(
        &mut seg_results,
        model_name,
        &translated_dir,
        auth_key.as_deref(),
    )?;
    if !translation_ok {
        anyhow::bail!("翻訳に失敗しました");
    }

    // 3. Collect translated pages
    println!("({}/{n}) Collecting translated pages...", 3);
    let translated_pages = translate::collect_translated_pages(&translated_dir)?;
    if translated_pages.is_empty() {
        anyhow::bail!("再構成できる翻訳結果が見つかりませんでした");
    }
    println!("  → {} pages collected", translated_pages.len());

    let doc_translation_path = translated_dir.join("document_translation.json");
    let doc_json = serde_json::to_string_pretty(&translated_pages)?;
    std::fs::write(&doc_translation_path, &doc_json)?;

    // 4. Compose translated PDF
    println!("({}/{n}) Composing translated PDF...", 4);
    let compose_dir = working_dir.join("composed");
    std::fs::create_dir_all(&compose_dir)?;

    let translated_pdf_path = if compare {
        compose_dir.join(format!("translated_{input_stem}_temp.pdf"))
    } else {
        output_path.clone()
    };

    let compose_result =
        compose::compose_pdf(input, &translated_pages, &translated_pdf_path, dedup_enabled)?;
    println!("  → {} segments placed", compose_result.segment_count);
    if !compose_result.warnings.is_empty() {
        println!("  ⚠ {} warnings", compose_result.warnings.len());
        for w in compose_result.warnings.iter().take(10) {
            println!("    - {w}");
        }
    }

    // 5. Compare PDF (if requested)
    if compare {
        println!("({}/{n}) Creating comparison PDF...", 5);
        let comparison_path =
            compose::create_comparison_pdf(input, &translated_pdf_path, &output_path)?;
        println!("  → {}", comparison_path.display());
    }

    println!();
    println!("Done! → {}", output_path.display());
    Ok(())
}

fn run_subcommand(command: Commands) -> anyhow::Result<()> {
    match command {
        Commands::PocPdf { input, output } => {
            tracing::info!("PoC: PDF operations test");
            std::fs::create_dir_all(&output)?;
            let png_path = output.join("page1.png");
            compose::poc_pdf_to_png(&input, 0, &png_path)?;
            tracing::info!("PDF→PNG test passed! Output in {:?}", output);
        }
        Commands::PocInfer {
            model,
            image,
            output,
            conf,
        } => {
            tracing::info!("PoC: ONNX inference with ort");
            let img = image::open(&image)?;
            let page_size = schema::PageSize {
                width: img.width() as f64,
                height: img.height() as f64,
            };
            let result = seg::run_onnx_inference(&model, &image, 1, page_size, conf)?;
            let json = serde_json::to_string_pretty(&result)?;
            std::fs::write(&output, &json)?;
            tracing::info!("Output written to {:?}", output);
            println!("{json}");
        }
    }
    Ok(())
}

mod compose;
mod schema;
mod seg;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "transpaper", about = "Translate English PDF papers to Japanese")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the full translation pipeline
    Translate {
        #[arg(short, long, help = "Input PDF file path")]
        input: PathBuf,
        #[arg(short, long, help = "Output PDF file path")]
        output: Option<PathBuf>,
        #[arg(short, long, default_value = "deepl", help = "Translation model")]
        model: String,
        #[arg(short, long, default_value_t = false, help = "Create comparison PDF")]
        compare: bool,
    },
    /// PoC: Test PDF operations (render, redact, text, image)
    PocPdf {
        #[arg(short, long, help = "Input PDF file path")]
        input: PathBuf,
        #[arg(short, long, default_value = "/tmp/poc_pdf_output", help = "Output directory")]
        output: PathBuf,
    },
    /// PoC: Run ONNX inference on a PNG image
    PocInfer {
        #[arg(short, long, help = "Path to ONNX model file")]
        model: PathBuf,
        #[arg(short, long, help = "Path to input PNG image")]
        image: PathBuf,
        #[arg(short, long, default_value = "poc_output.json", help = "Output JSON path")]
        output: PathBuf,
        #[arg(long, default_value_t = 0.25, help = "Confidence threshold")]
        conf: f64,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Translate {
            input,
            output: _,
            model,
            compare,
        } => {
            tracing::info!("Input PDF: {:?}", input);
            tracing::info!("Model: {}", model);
            tracing::info!("Compare mode: {}", compare);
            tracing::info!("Full pipeline not yet implemented");
        }
        Commands::PocPdf { input, output } => {
            tracing::info!("PoC: PDF operations test");
            std::fs::create_dir_all(&output)?;

            // Test 1: PDF → PNG rendering
            let png_path = output.join("page1.png");
            compose::poc_pdf_to_png(&input, 0, &png_path)?;
            tracing::info!("Test 1 (PDF→PNG): PASS");

            // Test 2: Redaction + text placement
            let redacted_path = output.join("redacted.pdf");
            compose::poc_redact_and_write(
                &input,
                &redacted_path,
                0,
                "Hello from Rust!",
                (100.0, 100.0, 400.0, 130.0),
            )?;
            tracing::info!("Test 2 (Redaction + Text): PASS");

            // Test 3: Image insertion
            let img_path = output.join("image_inserted.pdf");
            compose::poc_insert_image(
                &input,
                &img_path,
                0,
                &png_path,
                (50.0, 600.0, 250.0, 750.0),
            )?;
            tracing::info!("Test 3 (Image insertion): PASS");

            tracing::info!("All PDF PoC tests passed! Output in {:?}", output);
        }
        Commands::PocInfer {
            model,
            image,
            output,
            conf,
        } => {
            tracing::info!("PoC: ONNX inference with ort");
            tracing::info!("Model: {:?}", model);
            tracing::info!("Image: {:?}", image);

            let img = image::open(&image)?;
            let page_size = schema::PageSize {
                width: img.width() as f64,
                height: img.height() as f64,
            };

            let result = seg::run_onnx_inference(&model, &image, 1, page_size, conf)?;

            let json = serde_json::to_string_pretty(&result)?;
            std::fs::write(&output, &json)?;
            tracing::info!("Output written to {:?}", output);
            println!("{}", json);
        }
    }

    Ok(())
}

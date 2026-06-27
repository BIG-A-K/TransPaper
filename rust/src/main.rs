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

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const HF_REPO_ID: &str = "wybxc/DocLayout-YOLO-DocStructBench-onnx";
const MODEL_FILENAME: &str = "doclayout_yolo_docstructbench_imgsz1024.onnx";

pub enum ModelSource {
    Local(PathBuf),
    HuggingFace(PathBuf),
    Fallback,
}

impl ModelSource {
    pub fn path(&self) -> Option<&Path> {
        match self {
            ModelSource::Local(p) | ModelSource::HuggingFace(p) => Some(p),
            ModelSource::Fallback => None,
        }
    }

    pub fn description(&self) -> String {
        match self {
            ModelSource::Local(p) => format!("local:{}", p.display()),
            ModelSource::HuggingFace(p) => format!("hf-hub:{}", p.display()),
            ModelSource::Fallback => "fallback:text-full-page".to_string(),
        }
    }
}

pub fn resolve_model(local_model_path: Option<&Path>) -> ModelSource {
    if let Some(path) = local_model_path {
        if path.is_file() {
            tracing::info!("Using local model: {:?}", path);
            return ModelSource::Local(path.to_path_buf());
        }
        tracing::warn!("Specified model path not found: {:?}", path);
    }

    // Check project-local models directory
    let project_model = Path::new("models").join(MODEL_FILENAME);
    if project_model.is_file() {
        tracing::info!("Using project model: {:?}", project_model);
        return ModelSource::Local(project_model);
    }

    // Try HuggingFace Hub download
    match download_from_hf() {
        Ok(path) => {
            tracing::info!("Downloaded model from HuggingFace: {:?}", path);
            ModelSource::HuggingFace(path)
        }
        Err(e) => {
            tracing::warn!("Failed to download model from HuggingFace: {e}. Using fallback.");
            ModelSource::Fallback
        }
    }
}

fn download_from_hf() -> Result<PathBuf> {
    let api = hf_hub::api::sync::Api::new().context("Failed to create HuggingFace API")?;
    let repo = api.model(HF_REPO_ID.to_string());
    let path = repo
        .get(MODEL_FILENAME)
        .with_context(|| format!("Failed to download {MODEL_FILENAME} from {HF_REPO_ID}"))?;
    Ok(path)
}

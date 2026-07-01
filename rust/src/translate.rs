use anyhow::{Context, Result};
use rayon::prelude::*;
use std::path::Path;

use crate::schema::{SegmentPage, TranslatedPage, TranslationSegment};

const DEEPL_API_URL: &str = "https://api-free.deepl.com/v2/translate";
const BATCH_THRESHOLD: usize = 50;

#[derive(serde::Deserialize)]
struct DeepLResponse {
    translations: Vec<DeepLTranslation>,
}

#[derive(serde::Deserialize)]
struct DeepLTranslation {
    text: String,
}

fn translate_deepl(
    texts: &[String],
    target_lang: &str,
    auth_key: &str,
) -> Result<Vec<String>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let client = reqwest::blocking::Client::new();
    let response = client
        .post(DEEPL_API_URL)
        .header("Authorization", format!("DeepL-Auth-Key {auth_key}"))
        .json(&serde_json::json!({
            "text": texts,
            "target_lang": target_lang,
        }))
        .send()
        .context("DeepL API request failed")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        anyhow::bail!("DeepL API error: {status} — {body}");
    }

    let resp: DeepLResponse = response.json().context("Failed to parse DeepL response")?;
    Ok(resp.translations.into_iter().map(|t| t.text).collect())
}

fn translate_idx(texts: &[String]) -> Vec<String> {
    texts.to_vec()
}

const OLLAMA_SYSTEM_PROMPT: &str = "You are a professional academic translator. Translate the given English academic paper text into Japanese. Preserve equations, symbols, citations, references, section numbers, and inline code exactly as they are. Return only the translated Japanese text. Do not add any explanation, preface, markdown, or formatting.";
const OLLAMA_TIMEOUT_SECS: u64 = 300;
const OLLAMA_DEFAULT_WORKERS: usize = 8;

fn ollama_base_url() -> String {
    std::env::var("OLLAMA_HOST")
        .unwrap_or_else(|_| "http://localhost:11434".to_string())
        .trim_end_matches('/')
        .to_string()
}

fn ollama_num_workers() -> usize {
    std::env::var("OLLAMA_NUM_WORKERS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(OLLAMA_DEFAULT_WORKERS)
}

fn translate_ollama_one(
    client: &reqwest::blocking::Client,
    text: &str,
    ollama_model: &str,
    url: &str,
) -> Result<String> {
    let payload = serde_json::json!({
        "model": ollama_model,
        "messages": [
            {"role": "system", "content": OLLAMA_SYSTEM_PROMPT},
            {"role": "user", "content": text},
        ],
        "stream": false,
    });
    let response = match client
        .post(url)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            if e.is_connect() {
                anyhow::bail!(
                    "Ollamaサーバーに接続できませんでした ({url})。 `ollama serve` で起動しているか確認してください。"
                );
            } else if e.is_timeout() {
                anyhow::bail!(
                    "Ollamaサーバーがタイムアウトしました ({url})。 モデルが大きすぎるか、GPUメモリ不足の可能性があります。"
                );
            }
            return Err(e).context("Ollama API request failed");
        }
    };

    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        if status.as_u16() == 404 {
            anyhow::bail!(
                "Ollamaモデル '{ollama_model}' が見つかりません。 `ollama pull {ollama_model}` で取得してください。"
            );
        }
        anyhow::bail!("Ollama API error: {status} — {body}");
    }

    let v: serde_json::Value = response.json().context("Failed to parse Ollama response")?;
    let content = v["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Ollamaのレスポンス形式が想定と異なります: {v}"))?
        .trim()
        .to_string();
    Ok(content)
}

/// Ollama の /api/chat を使い、複数テキストを並列リクエストで翻訳する。
/// 順序は入力と同じ順で保持される。
fn translate_ollama(texts: &[String], model_name: &str) -> Result<Vec<String>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    // `ollama:<model>` のコロン以降をOllamaモデル名として扱う
    let ollama_model = model_name.strip_prefix("ollama:").unwrap_or(model_name);
    let url = format!("{}/api/chat", ollama_base_url());

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(OLLAMA_TIMEOUT_SECS))
        .build()
        .context("Failed to build HTTP client for Ollama")?;

    // 1テキストならスレッドプールのオーバーヘッドを避ける
    if texts.len() == 1 {
        return Ok(vec![translate_ollama_one(
            &client,
            &texts[0],
            ollama_model,
            &url,
        )?]);
    }

    let workers = ollama_num_workers().min(texts.len());
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .context("Failed to build Ollama worker pool")?;

    let translated = pool.install(|| {
        texts
            .par_iter()
            .map(|t| translate_ollama_one(&client, t, ollama_model, &url))
            .collect::<Result<Vec<_>>>()
    })?;
    Ok(translated)
}

/// 翻訳モデル名が有効か検証する。無効な場合は Err を返す。
/// 有効: "deepl", "idx", "ollama:<model>" (例: "ollama:gemma3:4b")
pub fn validate_model_name(model_name: &str) -> Result<()> {
    if model_name == "idx" || model_name == "deepl" {
        return Ok(());
    }
    if let Some(rest) = model_name.strip_prefix("ollama:") {
        if rest.is_empty() {
            anyhow::bail!(
                "Ollamaモデル名が空です: '{model_name}'。 `ollama:<model>` 形式で指定してください (例: ollama:gemma3:4b)"
            );
        }
        return Ok(());
    }
    anyhow::bail!(
        "未知の翻訳モデルです: '{model_name}'。 指定可能: 'deepl', 'idx', 'ollama:<model>' (例: ollama:gemma3:4b)"
    );
}

pub fn translate(
    seg_results: &mut [SegmentPage],
    model_name: &str,
    out_dir: &Path,
    auth_key: Option<&str>,
) -> Result<bool> {
    validate_model_name(model_name)?;
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("Failed to create output dir: {out_dir:?}"))?;

    let mut total_words = 0usize;

    let pb = indicatif::ProgressBar::new(seg_results.len() as u64);
    pb.set_style(
        indicatif::ProgressStyle::with_template("  Translating [{bar:30}] {pos}/{len} pages")
            .unwrap()
            .progress_chars("█▓░"),
    );

    for seg_page in seg_results.iter_mut() {
        // Collect texts to translate (separate pass to avoid borrow conflicts)
        let mut tasks: Vec<(usize, String)> = Vec::new();
        for (idx, block) in seg_page.blocks.iter().enumerate() {
            if block.block_type != "text" && block.block_type != "caption" {
                continue;
            }
            let text = block
                .meta
                .as_ref()
                .and_then(|m| m.text.as_ref())
                .map(|t| t.trim().to_string())
                .unwrap_or_default();
            if text.is_empty() {
                continue;
            }
            total_words += text.split_whitespace().count();
            tasks.push((idx, text));
        }

        // Batch short texts, translate long texts individually
        let mut batch_indices = Vec::new();
        let mut batch_texts = Vec::new();
        let mut translations: Vec<(usize, String)> = Vec::new();

        for (idx, text) in &tasks {
            let word_count = text.split_whitespace().count();
            if word_count < BATCH_THRESHOLD {
                batch_indices.push(*idx);
                batch_texts.push(text.clone());
            } else {
                if !batch_texts.is_empty() {
                    let translated = do_translate(&batch_texts, model_name, auth_key)?;
                    for (bi, tr) in batch_indices.drain(..).zip(translated) {
                        translations.push((bi, tr));
                    }
                    batch_texts.clear();
                }
                let translated = do_translate(&[text.clone()], model_name, auth_key)?;
                if let Some(tr) = translated.into_iter().next() {
                    translations.push((*idx, tr));
                }
            }
        }
        if !batch_texts.is_empty() {
            let translated = do_translate(&batch_texts, model_name, auth_key)?;
            for (bi, tr) in batch_indices.drain(..).zip(translated) {
                translations.push((bi, tr));
            }
        }

        // Apply translations
        for (idx, tr) in translations {
            if let Some(meta) = seg_page.blocks[idx].meta.as_mut() {
                meta.translated_text = Some(tr);
            }
        }

        // Save translated JSON
        let json_name = seg_page
            .json
            .as_ref()
            .and_then(|p| Path::new(p).file_name())
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("page_{:03}.json", seg_page.page));

        let out_path = out_dir.join(&json_name);
        let json = serde_json::to_string_pretty(seg_page)?;
        std::fs::write(&out_path, &json)
            .with_context(|| format!("Failed to write: {out_path:?}"))?;
        pb.inc(1);
    }

    pb.finish_and_clear();
    println!("  → {total_words} words translated");
    Ok(true)
}

fn do_translate(
    texts: &[String],
    model_name: &str,
    auth_key: Option<&str>,
) -> Result<Vec<String>> {
    match model_name {
        "idx" => Ok(translate_idx(texts)),
        "deepl" => {
            let key = auth_key.context("DeepL API key is required (set DEEPL_API env var)")?;
            translate_deepl(texts, "JA", key)
        }
        m if m.starts_with("ollama:") => translate_ollama(texts, model_name),
        _ => anyhow::bail!("Unknown translation model: {model_name}"),
    }
}

pub fn collect_translated_pages(translated_dir: &Path) -> Result<Vec<TranslatedPage>> {
    let mut pages = Vec::new();

    let mut entries: Vec<_> = std::fs::read_dir(translated_dir)
        .with_context(|| format!("Failed to read dir: {translated_dir:?}"))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("page_") && name.ends_with(".json")
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let content = std::fs::read_to_string(entry.path())
            .with_context(|| format!("Failed to read: {:?}", entry.path()))?;
        let page_data: SegmentPage =
            serde_json::from_str(&content).context("Failed to parse translated JSON")?;

        let mut segments = Vec::new();
        for block in &page_data.blocks {
            let meta = block.meta.as_ref();

            if block.block_type == "image" || block.block_type == "table" {
                segments.push(TranslationSegment {
                    id: block.id.clone(),
                    seg_type: block.block_type.clone(),
                    bbox: block.bbox,
                    source_text: None,
                    char_count: None,
                    avg_font_size: None,
                    translated_text: None,
                });
                continue;
            }

            if block.block_type == "math" {
                let translated = meta.and_then(|m| m.translated_text.clone());
                segments.push(TranslationSegment {
                    id: block.id.clone(),
                    seg_type: block.block_type.clone(),
                    bbox: block.bbox,
                    source_text: None,
                    char_count: None,
                    avg_font_size: None,
                    translated_text: translated,
                });
                continue;
            }

            let translated = meta
                .and_then(|m| m.translated_text.as_ref())
                .map(|t| t.trim().to_string())
                .unwrap_or_default();
            if translated.is_empty() {
                continue;
            }

            segments.push(TranslationSegment {
                id: block.id.clone(),
                seg_type: block.block_type.clone(),
                bbox: block.bbox,
                source_text: meta.and_then(|m| m.text.clone()),
                char_count: meta.and_then(|m| m.char_count),
                avg_font_size: meta.and_then(|m| m.avg_font_size),
                translated_text: Some(translated),
            });
        }

        pages.push(TranslatedPage {
            page: page_data.page,
            segments,
        });
    }

    Ok(pages)
}

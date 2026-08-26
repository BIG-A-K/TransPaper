use anyhow::{Context, Result};
use rayon::prelude::*;
use std::path::Path;

use crate::schema::{SegmentPage, TextBlockMeta, TranslatedPage, TranslationSegment};

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

fn translate_deepl(texts: &[String], target_lang: &str, auth_key: &str) -> Result<Vec<String>> {
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

// --- コーディングエージェントCLI経由の翻訳 (cc:/oc:/cx:) ---
// claude / opencode / codex を非対話モードでsubprocess起動し、
// その裏で動くLLMに翻訳させる。

const AGENT_BASE_PROMPT: &str = "You are a professional academic translator. Translate the given English academic paper text into Japanese. Preserve equations, symbols, citations, references, section numbers, and inline code exactly as they are. Tokens such as [[TRANSPAPER_INLINE_MATH_0001]] and [[TRANSPAPER_INLINE_MATH_0002]] are immutable placeholders: copy every such token exactly once without changing any character. Keep technical terms, proper nouns, product names, URLs, and inline code snippets in the original language. Aim for natural, fluent Japanese rather than literal word-by-word translation.";

const AGENT_TRANSLATE_INSTRUCTIONS: &str = "Return only the translated Japanese text. Do not add any explanation, preface, markdown, or formatting. This is a pure translation task: do not use any tools, do not read or write files, and do not run shell commands.";

const AGENT_BATCH_INSTRUCTIONS: &str = "You receive a JSON object like {\"texts\": [\"...\", \"...\"]} containing short English academic paper texts. Translate each text into Japanese. Return a JSON object {\"translations\": [\"...\", \"...\"]} with the SAME number of items in the SAME order. Do not add any explanation. This is a pure translation task: do not use any tools, do not read or write files, and do not run shell commands.";

const AGENT_BATCH_CHAR_THRESHOLD: usize = 300;
const AGENT_BATCH_MAX_ITEMS: usize = 16;
const AGENT_BATCH_MAX_CHARS: usize = 3000;
const AGENT_DEFAULT_TIMEOUT_SEC: u64 = 600;

fn agent_cli(agent_key: &str) -> Result<&'static str> {
    match agent_key {
        "cc" => Ok("claude"),
        "oc" => Ok("opencode"),
        "cx" => Ok("codex"),
        other => anyhow::bail!("未知のエージェントプレフィックスです: '{other}'"),
    }
}

fn agent_timeout_sec() -> u64 {
    std::env::var("TRANSPAPER_AGENT_TIMEOUT")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(AGENT_DEFAULT_TIMEOUT_SEC)
}

fn agent_command(
    agent_key: &str,
    agent_model: &str,
    prompt: &str,
    last_message_file: Option<&str>,
) -> Vec<String> {
    match agent_key {
        "cc" => vec![
            "claude".into(),
            "-p".into(),
            prompt.into(),
            "--model".into(),
            agent_model.into(),
        ],
        "oc" => vec![
            "opencode".into(),
            "run".into(),
            prompt.into(),
            "--model".into(),
            agent_model.into(),
        ],
        "cx" => {
            let mut cmd = vec![
                "codex".into(),
                "exec".into(),
                prompt.into(),
                "-m".into(),
                agent_model.into(),
                "--skip-git-repo-check".into(),
                "-s".into(),
                "read-only".into(),
            ];
            if let Some(file) = last_message_file {
                cmd.push("-o".into());
                cmd.push(file.into());
            }
            cmd
        }
        _ => unreachable!(),
    }
}

/// エージェントCLIを1回実行して応答テキストを返す。
/// タイムアウト時は子プロセスをkillしてエラーを返す。
fn run_agent_command(cmd: &[String]) -> Result<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let program = &cmd[0];
    let mut child = Command::new(program)
        .args(&cmd[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("'{program}' コマンドの起動に失敗しました。インストールするかPATHを通してください"))?;

    // stdout/stderr は別スレッドで読み込み、ブロッキングを回避する
    let mut stdout_pipe = child.stdout.take().context("failed to capture stdout")?;
    let mut stderr_pipe = child.stderr.take().context("failed to capture stderr")?;
    let stdout_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout_pipe.read_to_string(&mut buf);
        buf
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr_pipe.read_to_string(&mut buf);
        buf
    });

    // タイムアウト付きの待機 (polling)
    let timeout = std::time::Duration::from_secs(agent_timeout_sec());
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait().context("failed to wait for child process")? {
            Some(status) => break status,
            None => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!(
                        "エージェントCLIがタイムアウトしました ({}秒)。 TRANSPAPER_AGENT_TIMEOUT で延長できます。",
                        agent_timeout_sec()
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    };

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();
    if !status.success() {
        let trimmed_stderr = stderr.trim();
        let head: String = trimmed_stderr.chars().take(500).collect();
        anyhow::bail!("エージェントCLIがエラー終了しました (exit={}): {head}", status.code().unwrap_or(-1));
    }
    Ok(stdout.trim().to_string())
}

fn run_agent(agent_key: &str, agent_model: &str, prompt: &str) -> Result<String> {
    let cli = agent_cli(agent_key)?;
    if which_missing(cli) {
        anyhow::bail!("'{cli}' コマンドが見つかりません。 インストールするかPATHを通してください: -m {agent_key}:<model>");
    }

    // codex はログがstdoutに混ざるため、最終メッセージをファイル経由で受け取る
    let tmp_file;
    let last_message_file = if agent_key == "cx" {
        tmp_file = std::env::temp_dir().join(format!(
            "transpaper_codex_{}_{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        Some(tmp_file.to_string_lossy().to_string())
    } else {
        None
    };

    let cmd = agent_command(agent_key, agent_model, prompt, last_message_file.as_deref());
    let mut result = run_agent_command(&cmd);

    if let Some(file) = &last_message_file {
        if result.is_ok() {
            if let Ok(content) = std::fs::read_to_string(file) {
                result = Ok(content.trim().to_string());
            }
        }
        let _ = std::fs::remove_file(file);
    }
    result
}

/// PATH上にCLIがあるか簡易チェック (which 相当)。trueなら「無い」
fn which_missing(program: &str) -> bool {
    let path = std::env::var("PATH").unwrap_or_default();
    std::env::split_paths(&path).all(|dir| !dir.join(program).is_file())
}

/// LLM応答からJSONオブジェクトを寛容に取り出す。
/// ```json フェンスや前置きの文章が混ざっていてもパースできるようにする。
fn extract_json_object(text: &str) -> Option<serde_json::Value> {
    let mut candidates: Vec<&str> = vec![text];
    // ```json ... ``` フェンス
    if let Some(start) = text.find("```") {
        let rest = &text[start + 3..];
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        if let Some(end) = rest.find("```") {
            candidates.push(rest[..end].trim());
        }
    }
    // 最初の { から最後の } まで
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) {
        if end > start {
            candidates.push(&text[start..=end]);
        }
    }
    candidates
        .iter()
        .filter_map(|c| serde_json::from_str::<serde_json::Value>(c).ok())
        .find(|v: &serde_json::Value| v.is_object())
}

fn translate_agent_one(text: &str, agent_key: &str, agent_model: &str) -> Result<String> {
    let prompt = format!(
        "[Instructions]\n{AGENT_BASE_PROMPT} {AGENT_TRANSLATE_INSTRUCTIONS}\n\n[Text to translate]\n{text}"
    );
    run_agent(agent_key, agent_model, &prompt)
}

fn translate_agent_batch(
    texts: &[String],
    agent_key: &str,
    agent_model: &str,
) -> Result<Vec<String>> {
    let payload = serde_json::to_string(&serde_json::json!({ "texts": texts }))?;
    let prompt = format!(
        "[Instructions]\n{AGENT_BASE_PROMPT} {AGENT_BATCH_INSTRUCTIONS}\n\n[Input]\n{payload}"
    );
    let content = run_agent(agent_key, agent_model, &prompt)?;
    let translations = extract_json_object(&content)
        .and_then(|v| v.get("translations").cloned())
        .and_then(|v| v.as_array().cloned());
    match translations {
        Some(arr) if arr.len() == texts.len() => Ok(arr
            .into_iter()
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .collect()),
        // 件数・順序が保証されなかったら1テキストずつ翻訳
        _ => texts
            .iter()
            .map(|t| translate_agent_one(t, agent_key, agent_model))
            .collect(),
    }
}

struct BatchJob {
    texts: Vec<String>,
    indices: Vec<usize>,
}

/// テキストを翻訳ジョブ（バッチ or 個別）に分割。
/// 長いテキストは1テキスト1ジョブ、短いテキストは上限内で1バッチに束ねる。
fn plan_batch_jobs(texts: &[String]) -> Vec<BatchJob> {
    let mut jobs: Vec<BatchJob> = Vec::new();
    let mut batch_texts: Vec<String> = Vec::new();
    let mut batch_indices: Vec<usize> = Vec::new();
    let mut batch_chars = 0usize;

    for (i, t) in texts.iter().enumerate() {
        let n = t.chars().count();
        if n > AGENT_BATCH_CHAR_THRESHOLD {
            if !batch_texts.is_empty() {
                jobs.push(BatchJob {
                    texts: std::mem::take(&mut batch_texts),
                    indices: std::mem::take(&mut batch_indices),
                });
                batch_chars = 0;
            }
            jobs.push(BatchJob {
                texts: vec![t.clone()],
                indices: vec![i],
            });
            continue;
        }
        if batch_texts.len() + 1 > AGENT_BATCH_MAX_ITEMS
            || batch_chars + n > AGENT_BATCH_MAX_CHARS
        {
            jobs.push(BatchJob {
                texts: std::mem::take(&mut batch_texts),
                indices: std::mem::take(&mut batch_indices),
            });
            batch_chars = 0;
        }
        batch_texts.push(t.clone());
        batch_indices.push(i);
        batch_chars += n;
    }
    if !batch_texts.is_empty() {
        jobs.push(BatchJob {
            texts: batch_texts,
            indices: batch_indices,
        });
    }
    jobs
}

/// コーディングエージェントCLI (cc:/oc:/cx:) 経由でLLM翻訳する。
/// 短いテキストは1プロンプトに束ねてリクエスト数を削減し、ジョブ単位で並列実行する。
/// 順序は入力と同じ順で保持される。
fn translate_agent(texts: &[String], model_name: &str) -> Result<Vec<String>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let (agent_key, agent_model) = model_name
        .split_once(':')
        .context("エージェントモデル指定が不正です (例: oc:zai-coding-plan/glm-5.2)")?;
    agent_cli(agent_key)?; // バリデーション
    if agent_model.is_empty() {
        anyhow::bail!("エージェントのモデル名が空です: '{model_name}'");
    }

    let jobs = plan_batch_jobs(texts);

    let run_job = |job: &BatchJob| -> Result<(Vec<usize>, Vec<String>)> {
        if job.texts.len() == 1 {
            let out = translate_agent_one(&job.texts[0], agent_key, agent_model)?;
            Ok((job.indices.clone(), vec![out]))
        } else {
            let outs = translate_agent_batch(&job.texts, agent_key, agent_model)?;
            Ok((job.indices.clone(), outs))
        }
    };

    let job_results: Vec<(Vec<usize>, Vec<String>)> = if jobs.len() == 1 {
        vec![run_job(&jobs[0])?]
    } else {
        let workers = ollama_num_workers().min(jobs.len());
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .context("Failed to build agent worker pool")?;
        pool.install(|| jobs.par_iter().map(run_job).collect::<Result<Vec<_>>>() )?
    };

    let mut results = vec![String::new(); texts.len()];
    for (indices, outs) in job_results {
        for (i, t) in indices.into_iter().zip(outs) {
            results[i] = t;
        }
    }
    Ok(results)
}


const OLLAMA_SYSTEM_PROMPT: &str = "You are a professional academic translator. Translate the given English academic paper text into Japanese. Preserve equations, symbols, citations, references, section numbers, and inline code exactly as they are. Tokens such as [[TRANSPAPER_INLINE_MATH_0001]] and [[TRANSPAPER_INLINE_MATH_0002]] are immutable placeholders: copy every such token exactly once without changing any character. Return only the translated Japanese text. Do not add any explanation, preface, markdown, or formatting.";
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
        "options": {
            "temperature": 0,
        },
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
        .timeout(None::<std::time::Duration>)
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

fn store_translation_result(meta: &mut TextBlockMeta, translated: String) {
    let expected: Vec<String> = meta
        .inline_math
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|math| !math.placeholder.is_empty())
        .map(|math| math.placeholder.clone())
        .collect();
    if expected.is_empty() {
        meta.translated_text = Some(translated);
        return;
    }

    let all_preserved = expected
        .iter()
        .all(|placeholder| translated.matches(placeholder).count() == 1);
    if all_preserved {
        meta.translated_text = Some(translated);
        meta.inline_math_status = Some("preserved".to_string());
        return;
    }

    meta.translated_text = Some(meta.text.clone().unwrap_or(translated));
    meta.inline_math_status = Some("fallback_source".to_string());
    meta.translation_warnings
        .get_or_insert_with(Vec::new)
        .push("文中数式プレースホルダーが翻訳で壊れたため原文へフォールバックしました".to_string());
}

/// 翻訳モデル名が有効か検証する。無効な場合は Err を返す。
/// 有効: "deepl", "idx", "ollama:<model>" (例: "ollama:gemma3:4b"),
/// "cc:<model>" (Claude Code), "oc:<model>" (opencode), "cx:<model>" (codex)
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
    if let Some((prefix, rest)) = model_name.split_once(':') {
        if matches!(prefix, "cc" | "oc" | "cx") {
            if rest.is_empty() {
                anyhow::bail!(
                    "エージェントのモデル名が空です: '{model_name}'。 `{prefix}:<model>` 形式で指定してください (例: oc:zai-coding-plan/glm-5.2, cc:sonnet, cx:gpt-5.1)"
                );
            }
            return Ok(());
        }
    }
    anyhow::bail!(
        "未知の翻訳モデルです: '{model_name}'。 指定可能: 'deepl', 'idx', 'ollama:<model>', 'cc:<model>' (Claude Code), 'oc:<model>' (opencode), 'cx:<model>' (codex)"
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
                let translated = do_translate(std::slice::from_ref(text), model_name, auth_key)?;
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
                store_translation_result(meta, tr);
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

fn do_translate(texts: &[String], model_name: &str, auth_key: Option<&str>) -> Result<Vec<String>> {
    match model_name {
        "idx" => Ok(translate_idx(texts)),
        "deepl" => {
            let key = auth_key.context("DeepL API key is required (set DEEPL_API env var)")?;
            translate_deepl(texts, "JA", key)
        }
        m if m.starts_with("ollama:") => translate_ollama(texts, model_name),
        m if m.starts_with("cc:") || m.starts_with("oc:") || m.starts_with("cx:") => {
            translate_agent(texts, model_name)
        }
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
                    inline_math: None,
                    inline_math_status: None,
                    translation_warnings: None,
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
                    inline_math: None,
                    inline_math_status: None,
                    translation_warnings: None,
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
                inline_math: meta.and_then(|m| m.inline_math.clone()),
                inline_math_status: meta.and_then(|m| m.inline_math_status.clone()),
                translation_warnings: meta.and_then(|m| m.translation_warnings.clone()),
            });
        }

        pages.push(TranslatedPage {
            page: page_data.page,
            segments,
        });
    }

    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::InlineMath;

    fn protected_meta() -> TextBlockMeta {
        let placeholder = "[[TRANSPAPER_INLINE_MATH_0001]]".to_string();
        TextBlockMeta {
            text: Some(format!("shape {placeholder}")),
            inline_math: Some(vec![InlineMath {
                id: "m0001".to_string(),
                placeholder,
                text: "H×W".to_string(),
                bbox: (50.0, 10.0, 70.0, 22.0),
                ..Default::default()
            }]),
            ..Default::default()
        }
    }

    #[test]
    fn translation_preserves_valid_inline_math_placeholder() {
        let mut meta = protected_meta();
        store_translation_result(
            &mut meta,
            "形状は [[TRANSPAPER_INLINE_MATH_0001]] です".to_string(),
        );

        assert_eq!(meta.inline_math_status.as_deref(), Some("preserved"));
        assert!(meta
            .translated_text
            .as_deref()
            .unwrap()
            .contains("[[TRANSPAPER_INLINE_MATH_0001]]"));
    }

    #[test]
    fn broken_inline_math_placeholder_falls_back_to_source() {
        let mut meta = protected_meta();
        store_translation_result(&mut meta, "形状は H x W です".to_string());

        assert_eq!(meta.inline_math_status.as_deref(), Some("fallback_source"));
        assert_eq!(
            meta.translated_text.as_deref(),
            Some("shape [[TRANSPAPER_INLINE_MATH_0001]]")
        );
        assert_eq!(meta.translation_warnings.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn validate_model_name_accepts_agent_prefixes() {
        for ok in [
            "deepl",
            "idx",
            "ollama:gemma3:4b",
            "cc:sonnet",
            "oc:zai-coding-plan/glm-5.2",
            "cx:gpt-5.1",
        ] {
            assert!(validate_model_name(ok).is_ok(), "should accept {ok}");
        }
    }

    #[test]
    fn validate_model_name_rejects_invalid_agents() {
        for ng in ["cc:", "oc:", "cx:", "foo:bar", "agent", ""] {
            assert!(validate_model_name(ng).is_err(), "should reject {ng}");
        }
    }

    #[test]
    fn agent_command_uses_expected_flags() {
        let cc = agent_command("cc", "sonnet", "P", None);
        assert_eq!(cc, vec!["claude", "-p", "P", "--model", "sonnet"]);

        let oc = agent_command("oc", "glm-5-turbo", "P", None);
        assert_eq!(oc, vec!["opencode", "run", "P", "--model", "glm-5-turbo"]);

        let cx = agent_command("cx", "gpt-5.1", "P", Some("/tmp/o.txt"));
        assert_eq!(
            cx,
            vec![
                "codex",
                "exec",
                "P",
                "-m",
                "gpt-5.1",
                "--skip-git-repo-check",
                "-s",
                "read-only",
                "-o",
                "/tmp/o.txt"
            ]
        );
    }

    #[test]
    fn extract_json_object_handles_fences_and_prose() {
        assert_eq!(
            extract_json_object(r#"{"translations": ["a"]}"#)
                .and_then(|v| v["translations"][0].as_str().map(str::to_string)),
            Some("a".to_string())
        );
        let fenced = "result:\n```json\n{\"translations\": [\"a\", \"b\"]}\n```\ndone";
        assert_eq!(
            extract_json_object(fenced).and_then(|v| v["translations"][1].as_str().map(str::to_string)),
            Some("b".to_string())
        );
        let prose = "結果は {\"translations\": [\"x\"]} です";
        assert_eq!(
            extract_json_object(prose).and_then(|v| v["translations"][0].as_str().map(str::to_string)),
            Some("x".to_string())
        );
        assert!(extract_json_object("no json here").is_none());
        assert!(extract_json_object("[\"a\"]").is_none());
    }

    #[test]
    fn plan_batch_jobs_splits_long_and_short_texts() {
        let long = "word ".repeat(100); // 500 chars > 300 threshold
        let texts: Vec<String> = vec![
            "short a".to_string(),
            "short b".to_string(),
            long.clone(),
            "short c".to_string(),
        ];
        let jobs = plan_batch_jobs(&texts);

        // 長いテキストで分割され、 [short a, short b], [long], [short c] の3ジョブ
        assert_eq!(jobs.len(), 3);
        assert_eq!(jobs[0].indices, vec![0, 1]);
        assert_eq!(jobs[1].indices, vec![2]);
        assert_eq!(jobs[1].texts, vec![long]);
        assert_eq!(jobs[2].indices, vec![3]);

        // 全ジョブのインデックスで元の全テキストが網羅される
        let mut all: Vec<usize> = jobs.iter().flat_map(|j| j.indices.clone()).collect();
        all.sort_unstable();
        assert_eq!(all, vec![0, 1, 2, 3]);
    }

    #[test]
    fn plan_batch_jobs_respects_max_items() {
        let texts: Vec<String> = (0..40).map(|i| format!("text {i}")).collect();
        let jobs = plan_batch_jobs(&texts);
        for job in &jobs {
            assert!(job.texts.len() <= AGENT_BATCH_MAX_ITEMS);
        }
        let mut all: Vec<usize> = jobs.iter().flat_map(|j| j.indices.clone()).collect();
        all.sort_unstable();
        assert_eq!(all, (0..40).collect::<Vec<_>>());
    }

    #[test]
    fn run_agent_command_captures_stdout_and_reports_failure() {
        // 標準コマンドでstdout取得を確認
        let out = run_agent_command(&["echo".to_string(), "hello".to_string()]).unwrap();
        assert_eq!(out, "hello");

        // 異常終了時はエラー
        let err = run_agent_command(&["false".to_string()]);
        assert!(err.is_err());
    }

    // 実CLI（opencode等）が必要なため通常はスキップ。
    // `cargo test -- --ignored` で手動実行する。
    #[test]
    #[ignore]
    fn agent_translate_batch_via_real_cli() {
        let texts = vec![
            "Figure 1: Model architecture.".to_string(),
            "Table 2 shows the results.".to_string(),
        ];
        let out = translate_agent(&texts, "oc:zai-coding-plan/glm-5-turbo").unwrap();
        assert_eq!(out.len(), 2);
        assert!(!out[0].is_empty() && !out[1].is_empty());
        eprintln!("translated: {out:?}");
    }
}

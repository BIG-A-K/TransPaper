# 翻訳方法
# 1. DeepL API
# 2. HuggingFaceの翻訳モデル
# 3. Ollama経由のローカルLLM
# 4. コーディングエージェントCLI (cc:/oc:/cx:) 経由のLLM
import json
import os
import re
import shutil
import subprocess
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import requests
from tqdm import tqdm

from common.schema import SegmentPage, TranslatedPage, TranslationSegment


def translate_deepl(texts: list[str], target_lang="JA", auth_key=None) -> list[str]:
    if auth_key is None:
        raise ValueError("DeepL API key is required for translation.")
    if not texts:
        return []

    # 単一のテキストが渡された場合でもリストとして処理
    if isinstance(texts, str):
        texts = [texts]

    URL = "https://api-free.deepl.com/v2/translate"
    headers = {
        "Authorization": f"DeepL-Auth-Key {auth_key}",
        "Content-Type": "application/json",
    }
    data = {
        "text": texts,
        "target_lang": target_lang,
    }
    try:
        response = requests.post(URL, headers=headers, json=data)
        translations = response.json()["translations"]
        translated_texts = [t["text"] for t in translations]
    except Exception as e:
        print(f"Batch translation error: {e}")
        # エラー時はフォールバック：個別に翻訳
        translated_texts = [translate_deepl(text, target_lang, auth_key) for text in texts]
    return translated_texts


def idx(texts: list[str]) -> list[str]:
    """
    そのまま返すだけのダミー関数
    """
    return texts


def translate_huggingface(texts: list[str], model_name="staka/fugumt-en-ja") -> list[str]:
    # TODO: 実装 (MarianMT等)
    raise NotImplementedError(
        f"HuggingFace翻訳は未実装です (model_name={model_name})。"
        " 現在は 'deepl', 'idx', 'ollama:<model>', 'cc:<model>', 'oc:<model>', "
        "'cx:<model>' が利用可能です。"
    )


OLLAMA_TRANSLATE_SYSTEM_PROMPT = (
    "You are a professional academic translator. "
    "Translate the given English academic paper text into Japanese. "
    "Preserve equations, symbols, citations, references, section numbers, "
    "and inline code exactly as they are. "
    "Tokens such as [[TRANSPAPER_INLINE_MATH_0001]] and "
    "[[TRANSPAPER_INLINE_MATH_0002]] are immutable placeholders: "
    "copy every such token exactly once without changing any character. "
    "Return only the translated Japanese text. "
    "Do not add any explanation, preface, markdown, or formatting."
)

# --- Ollama バッチプロンプト化のパラメータ ---
# この文字数以下のテキストは1プロンプトに束ねる候補になる
OLLAMA_BATCH_CHAR_THRESHOLD = 300
# 1つのバッチプロンプトに含めるテキスト数の上限
OLLAMA_BATCH_MAX_ITEMS = 16
# 1つのバッチプロンプトの入力合計文字数の上限
OLLAMA_BATCH_MAX_CHARS = 3000

OLLAMA_BATCH_SYSTEM_PROMPT = (
    "You are a professional academic translator. "
    'You receive a JSON object like {"texts": ["...", "..."]} containing '
    "short English academic paper texts. "
    "Translate each text into Japanese, preserving equations, symbols, citations, "
    "references, section numbers, and inline code exactly as they are. "
    "Tokens such as [[TRANSPAPER_INLINE_MATH_0001]] and "
    "[[TRANSPAPER_INLINE_MATH_0002]] are immutable placeholders; "
    "copy every such token exactly once without changing any character. "
    'Return a JSON object {"translations": ["...", "..."]} with the SAME number '
    "of items in the SAME order. Do not add any explanation."
)


def translate_ollama(
    texts: list[str],
    model_name: str,
    base_url: str | None = None,
    num_workers: int | None = None,
) -> list[str]:
    """Ollama のローカルLLMで翻訳。入力と同じ順序で返す。

    短いテキストは1プロンプトに束ねて1リクエストで翻訳し（リクエスト数削減）、
    長いテキストは1テキスト1リクエスト。すべてのジョブを ThreadPoolExecutor で
    並列実行する。真の並列推論にするにはサーバー側で OLLAMA_NUM_PARALLEL を
    上げておく必要がある。

    Args:
        texts: 翻訳対象テキストのリスト
        model_name: `ollama:<model>` 形式のモデル指定
        base_url: OllamaサーバーのベースURL。未指定時は OLLAMA_HOST 環境変数、
            さらに未設定なら http://localhost:11434
        num_workers: 並列リクエスト数。未指定時は _default_num_workers() で自動決定

    Returns:
        翻訳結果テキストのリスト（入力と同じ順序）
    """
    if isinstance(texts, str):
        texts = [texts]
    if not texts:
        return []

    # `ollama:<model>` のコロン以降をOllamaモデル名として扱う
    ollama_model = model_name.split(":", 1)[1] if ":" in model_name else model_name

    base = base_url or os.getenv("OLLAMA_HOST", "http://localhost:11434").rstrip("/")
    url = f"{base}/api/chat"
    if num_workers is None:
        num_workers = _default_num_workers()

    def one(text: str) -> str:
        return _ollama_chat_one(text, ollama_model, url)

    def batch(slice_texts: list[str]) -> list[str]:
        return _ollama_chat_batch(slice_texts, ollama_model, url)

    return _run_batched_jobs(texts, one, batch, desc="Ollama jobs", num_workers=num_workers)


def _plan_ollama_jobs(
    texts: list[str],
) -> list[tuple[list[str], list[int]]]:
    """テキストを翻訳ジョブ（バッチ or 個別）に分割。

    長いテキスト（OLLAMA_BATCH_CHAR_THRESHOLD 超）は1テキスト1ジョブ。
    短いテキストは OLLAMA_BATCH_MAX_ITEMS / OLLAMA_BATCH_MAX_CHARS の上限内で
    1つのバッチジョブに貪欲に束ねる。元の順序のインデックスを各ジョブに保持する。

    Ollama でもコーディングエージェント (cc:/oc:/cx:) でも同じ分割戦略を使う。
    """
    jobs: list[tuple[list[str], list[int]]] = []
    batch_texts: list[str] = []
    batch_idx: list[int] = []
    batch_chars = 0

    def flush():
        nonlocal batch_texts, batch_idx, batch_chars
        if batch_texts:
            jobs.append((batch_texts, batch_idx))
            batch_texts, batch_idx, batch_chars = [], [], 0

    for i, t in enumerate(texts):
        n = len(t)
        if n > OLLAMA_BATCH_CHAR_THRESHOLD:
            # 長いテキストは保留中のバッチを吐いてから個別ジョブに
            flush()
            jobs.append(([t], [i]))
            continue
        if (
            len(batch_texts) + 1 > OLLAMA_BATCH_MAX_ITEMS
            or batch_chars + n > OLLAMA_BATCH_MAX_CHARS
        ):
            flush()
        batch_texts.append(t)
        batch_idx.append(i)
        batch_chars += n
    flush()
    return jobs


def _run_batched_jobs(
    texts: list[str],
    one,
    batch,
    desc: str,
    num_workers: int,
) -> list[str]:
    """ジョブ分割＋並列実行の共通基盤（Ollama / コーディングエージェント共用）。

    `one(text) -> str` で1テキスト翻訳、`batch(texts) -> list[str]` で
    複数テキストを1リクエストで翻訳する関数を受け取る。入力と同じ順序で
    結果を返す。欠損があった要素は空文字で埋める。
    """
    jobs = _plan_ollama_jobs(texts)
    results: list[str | None] = [None] * len(texts)

    def run_job(job):
        slice_texts, indices = job
        if len(slice_texts) == 1:
            results[indices[0]] = one(slice_texts[0])
        else:
            outs = batch(slice_texts)
            for i, t in zip(indices, outs):
                results[i] = t

    if len(jobs) == 1:
        run_job(jobs[0])
    else:
        workers = min(num_workers, len(jobs))
        with ThreadPoolExecutor(max_workers=workers) as ex:
            list(tqdm(ex.map(run_job, jobs), total=len(jobs), desc=desc))

    # フォールバック: 欠損があれば空文字で埋める
    return [r if r is not None else "" for r in results]


def _estimate_num_predict(total_chars: int) -> int:
    """入力合計文字数から生成トークン数の上限を見積もる。

    英→日で文字数は増える方向だが、LLM の暴走（同一トークン反復など）時の
    無駄な生成時間を抑えるための安全上限。
    """
    return max(64, min(4096, int(total_chars * 2.5)))


def _ollama_chat_messages(
    messages: list[dict],
    ollama_model: str,
    url: str,
    num_predict: int,
    fmt: dict | None = None,
) -> str:
    """Ollama の /api/chat で1リクエスト分の推論を行い、応答テキストを返す。

    temperature=0 で決定的な翻訳を行い、num_predict で生成長の安全上限を設ける。
    `fmt`（JSONスキーマ）が渡された場合は構造化出力を要求する。
    """
    payload: dict = {
        "model": ollama_model,
        "messages": messages,
        "stream": False,
        "options": {"temperature": 0, "num_predict": num_predict},
    }
    if fmt is not None:
        payload["format"] = fmt
    try:
        response = requests.post(url, headers={"Content-Type": "application/json"}, json=payload)
        response.raise_for_status()
        result = response.json()
        return str(result["message"]["content"]).strip()
    except requests.exceptions.ConnectionError as e:
        raise RuntimeError(
            f"Ollamaサーバーに接続できませんでした ({url})。"
            " `ollama serve` で起動しているか確認してください。"
        ) from e
    except requests.exceptions.HTTPError as e:
        if e.response is not None and e.response.status_code == 404:
            raise RuntimeError(
                f"Ollamaモデル '{ollama_model}' が見つかりません。"
                f" `ollama pull {ollama_model}` で取得してください。"
            ) from e
        body = e.response.text if e.response is not None else ""
        raise RuntimeError(f"Ollama APIエラー: {e} body={body}") from e
    except KeyError as e:
        raise RuntimeError(f"Ollamaのレスポンス形式が想定と異なります: {result}") from e


def _ollama_chat_one(text: str, ollama_model: str, url: str) -> str:
    """1テキストを Ollama の /api/chat で翻訳する。"""
    return _ollama_chat_messages(
        messages=[
            {"role": "system", "content": OLLAMA_TRANSLATE_SYSTEM_PROMPT},
            {"role": "user", "content": text},
        ],
        ollama_model=ollama_model,
        url=url,
        num_predict=_estimate_num_predict(len(text)),
    )


def _ollama_chat_batch(texts: list[str], ollama_model: str, url: str) -> list[str]:
    """複数テキストを1プロンプトに束ねて1リクエストで翻訳する。

    JSON構造化出力で件数と順序を保証する。件数が不一致になった場合は
    フォールバックとして各テキストを個別に翻訳し直す。
    """
    total_chars = sum(len(t) for t in texts)
    fmt = {
        "type": "object",
        "properties": {
            "translations": {"type": "array", "items": {"type": "string"}},
        },
        "required": ["translations"],
    }
    content = _ollama_chat_messages(
        messages=[
            {"role": "system", "content": OLLAMA_BATCH_SYSTEM_PROMPT},
            {"role": "user", "content": json.dumps({"texts": list(texts)}, ensure_ascii=False)},
        ],
        ollama_model=ollama_model,
        url=url,
        num_predict=_estimate_num_predict(total_chars),
        fmt=fmt,
    )
    try:
        obj = json.loads(content)
        trans = obj.get("translations", [])
    except (json.JSONDecodeError, TypeError, AttributeError):
        trans = []
    if len(trans) != len(texts):
        # 順序・件数が保証されなかったら確実に1テキストずつ翻訳
        return [_ollama_chat_one(t, ollama_model, url) for t in texts]
    return [str(x) for x in trans]


# --- コーディングエージェントCLI経由の翻訳 (cc:/oc:/cx:) ---
# claude code / opencode / codex を非対話モードでsubprocess起動し、
# その裏で動くLLMに翻訳させる。プレフィックスとCLIの対応:
#   cc: -> claude  (`claude -p <prompt> --model <model>`)
#   oc: -> opencode (`opencode run <prompt> --model <model>`)
#   cx: -> codex   (`codex exec <prompt> -m <model> -s read-only`)
AGENT_BACKENDS: dict[str, dict] = {
    "cc": {"cli": "claude", "display": "Claude Code"},
    "oc": {"cli": "opencode", "display": "opencode"},
    "cx": {"cli": "codex", "display": "codex"},
}

AGENT_TRANSLATE_SYSTEM_PROMPT = (
    OLLAMA_TRANSLATE_SYSTEM_PROMPT
    + " Keep technical terms, proper nouns, product names, URLs, "
    "and inline code snippets in the original language. "
    "Aim for natural, fluent Japanese rather than literal word-by-word translation. "
    "This is a pure translation task: do not use any tools, "
    "do not read or write files, and do not run shell commands. "
    "Output only the translated text."
)

AGENT_BATCH_SYSTEM_PROMPT = (
    OLLAMA_BATCH_SYSTEM_PROMPT
    + " Keep technical terms, proper nouns, product names, URLs, "
    "and inline code snippets in the original language. "
    "Aim for natural, fluent Japanese rather than literal word-by-word translation. "
    "This is a pure translation task: do not use any tools, "
    "do not read or write files, and do not run shell commands. "
    "Output only the JSON object."
)

# 1リクエストあたりのタイムアウト秒数（コーディングエージェントは起動・応答が遅い）
AGENT_TIMEOUT_SEC = 600


def _agent_timeout() -> int:
    """エージェントCLIのタイムアウト秒数。環境変数 TRANSPAPER_AGENT_TIMEOUT で上書き可。"""
    v = os.getenv("TRANSPAPER_AGENT_TIMEOUT")
    if v and v.strip().isdigit():
        return max(1, int(v))
    return AGENT_TIMEOUT_SEC


def _agent_command(
    agent_key: str,
    agent_model: str,
    prompt: str,
    last_message_file: str | None = None,
) -> list[str]:
    """エージェントキー (cc/oc/cx) に対応するCLIのコマンドラインを組み立てる。

    codex はstdoutにログが混ざるため `-o <file>` で最終メッセージをファイルに
    書き出させる。翻訳でシェルは使わないので read-only サンドボックスで起動する。
    """
    if agent_key == "cc":
        return ["claude", "-p", prompt, "--model", agent_model]
    if agent_key == "oc":
        return ["opencode", "run", prompt, "--model", agent_model]
    if agent_key == "cx":
        cmd = [
            "codex",
            "exec",
            prompt,
            "-m",
            agent_model,
            "--skip-git-repo-check",
            "-s",
            "read-only",
        ]
        if last_message_file is not None:
            cmd += ["-o", last_message_file]
        return cmd
    raise ValueError(f"未知のエージェントプレフィックスです: '{agent_key}'")


def _run_agent(agent_key: str, agent_model: str, prompt: str) -> str:
    """エージェントCLIを1回実行して応答テキストを返す。"""
    backend = AGENT_BACKENDS[agent_key]
    cli = backend["cli"]
    if shutil.which(cli) is None:
        raise RuntimeError(
            f"'{cli}' コマンドが見つかりません ({backend['display']})。"
            f" インストールするかPATHを通してください: -m {agent_key}:<model>"
        )

    # codex は最終メッセージをファイル経由で受け取る
    out_file: str | None = None
    if agent_key == "cx":
        fd, out_file = tempfile.mkstemp(prefix="transpaper_codex_", suffix=".txt")
        os.close(fd)

    cmd = _agent_command(agent_key, agent_model, prompt, last_message_file=out_file)
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=_agent_timeout(),
        )
        if proc.returncode != 0:
            stderr = (proc.stderr or "").strip()
            raise RuntimeError(
                f"{backend['display']} がエラー終了しました (exit={proc.returncode}): {stderr[:500]}"
            )
        if out_file is not None:
            content = Path(out_file).read_text(encoding="utf-8")
        else:
            content = proc.stdout
        return content.strip()
    except subprocess.TimeoutExpired as e:
        raise RuntimeError(
            f"{backend['display']} がタイムアウトしました ({_agent_timeout()}秒)。"
            " TRANSPAPER_AGENT_TIMEOUT で延長できます。"
        ) from e
    finally:
        if out_file is not None and Path(out_file).exists():
            Path(out_file).unlink()


def _extract_json_object(text: str) -> dict | None:
    """LLM応答からJSONオブジェクトを寛容に取り出す。

    コーディングエージェントは構造化出力を強制できないため、
    ```json フェンスや前置きの文章が混ざっていてもパースできるようにする。
    """
    candidates: list[str] = [text]
    fence = re.search(r"```(?:json)?\s*(\{.*?\})\s*```", text, re.DOTALL)
    if fence:
        candidates.append(fence.group(1))
    start, end = text.find("{"), text.rfind("}")
    if start != -1 and end > start:
        candidates.append(text[start : end + 1])
    for candidate in candidates:
        try:
            obj = json.loads(candidate)
            if isinstance(obj, dict):
                return obj
        except json.JSONDecodeError:
            continue
    return None


def _agent_translate_one(text: str, agent_key: str, agent_model: str) -> str:
    """1テキストをコーディングエージェント経由で翻訳する。"""
    prompt = (
        "[Instructions]\n"
        f"{AGENT_TRANSLATE_SYSTEM_PROMPT}\n\n"
        "[Text to translate]\n"
        f"{text}"
    )
    return _run_agent(agent_key, agent_model, prompt)


def _agent_translate_batch(
    texts: list[str], agent_key: str, agent_model: str
) -> list[str]:
    """複数テキストを1プロンプトに束ねてコーディングエージェント経由で翻訳する。

    構造化出力が使えないため、応答からJSONを抽出して件数・順序を検証する。
    件数が不一致の場合はフォールバックとして各テキストを個別に翻訳し直す。
    """
    payload = json.dumps({"texts": list(texts)}, ensure_ascii=False)
    prompt = (
        "[Instructions]\n"
        f"{AGENT_BATCH_SYSTEM_PROMPT}\n\n"
        "[Input]\n"
        f"{payload}"
    )
    content = _run_agent(agent_key, agent_model, prompt)
    obj = _extract_json_object(content)
    trans = obj.get("translations") if obj else None
    if not isinstance(trans, list) or len(trans) != len(texts):
        # 順序・件数が保証されなかったら確実に1テキストずつ翻訳
        return [_agent_translate_one(t, agent_key, agent_model) for t in texts]
    return [str(x) for x in trans]


def translate_agent(
    texts: list[str],
    model_name: str,
    num_workers: int | None = None,
) -> list[str]:
    """コーディングエージェントCLI (cc:/oc:/cx:) 経由でLLM翻訳する。

    `model_name` は `<prefix>:<agentモデル名>` 形式（例: `oc:zai-coding-plan/glm-5.2`）。
    バッチプロンプト化と並列実行はOllamaと同じ戦略（_run_batched_jobs）を使う。

    Args:
        texts: 翻訳対象テキストのリスト
        model_name: `cc:<model>` / `oc:<model>` / `cx:<model>` 形式のモデル指定
        num_workers: 並列プロセス数。未指定時は _default_num_workers() で自動決定

    Returns:
        翻訳結果テキストのリスト（入力と同じ順序）
    """
    if isinstance(texts, str):
        texts = [texts]
    if not texts:
        return []

    agent_key, _, agent_model = model_name.partition(":")
    if agent_key not in AGENT_BACKENDS:
        raise ValueError(f"未知のエージェントプレフィックスです: '{model_name}'")
    if not agent_model:
        raise ValueError(
            f"エージェントのモデル名が空です: '{model_name}'。"
            f" `{agent_key}:<model>` 形式で指定してください (例: oc:zai-coding-plan/glm-5.2)"
        )

    if num_workers is None:
        num_workers = _default_num_workers()

    def one(text: str) -> str:
        return _agent_translate_one(text, agent_key, agent_model)

    def batch(slice_texts: list[str]) -> list[str]:
        return _agent_translate_batch(slice_texts, agent_key, agent_model)

    desc = f"{AGENT_BACKENDS[agent_key]['display']} jobs"
    return _run_batched_jobs(texts, one, batch, desc=desc, num_workers=num_workers)


def _do_translate(texts: list[str], model_name: str, auth_key: str | None = None) -> list[str]:
    """model_name に応じて翻訳バックエンドをディスパッチする。"""
    validate_model_name(model_name)
    if model_name == "idx":
        return idx(texts)
    if model_name == "deepl":
        return translate_deepl(texts, target_lang="JA", auth_key=auth_key)
    if model_name.startswith("ollama:"):
        return translate_ollama(texts, model_name=model_name)
    if model_name.startswith(tuple(f"{k}:" for k in AGENT_BACKENDS)):
        return translate_agent(texts, model_name=model_name)
    # validate_model_name で弾くのでここには到達しない
    raise ValueError(f"未知の翻訳モデルです: '{model_name}'")


def _store_translation_result(meta: dict, translated: str) -> None:
    """Store a translation only when all protected inline-math tokens survived."""
    inline_math = meta.get("inline_math") or []
    expected = [item.get("placeholder") for item in inline_math if item.get("placeholder")]
    if not expected:
        meta["translated_text"] = translated
        return

    missing = [placeholder for placeholder in expected if translated.count(placeholder) != 1]
    if not missing:
        meta["translated_text"] = translated
        meta["inline_math_status"] = "preserved"
        return

    # A damaged token cannot be mapped back to a reliable insertion position. Keep
    # the protected source text instead of emitting raw TeX or silently dropping math.
    meta["translated_text"] = str(meta.get("text") or translated)
    meta["inline_math_status"] = "fallback_source"
    warnings = list(meta.get("translation_warnings") or [])
    warnings.append("文中数式プレースホルダーが翻訳で壊れたため原文へフォールバックしました")
    meta["translation_warnings"] = warnings


def validate_model_name(model_name: str) -> None:
    """翻訳モデル名が有効か検証する。無効な場合は ValueError を発生させる。

    有効な指定:
        - 'deepl'
        - 'idx'
        - 'ollama:<model>' (例: 'ollama:gemma3:4b')
        - 'cc:<model>' / 'oc:<model>' / 'cx:<model>'
          (例: 'oc:zai-coding-plan/glm-5.2', 'cc:sonnet', 'cx:gpt-5.1')
    """
    if model_name in ("idx", "deepl"):
        return
    if model_name.startswith("ollama:"):
        ollama_model = model_name.split(":", 1)[1]
        if not ollama_model:
            raise ValueError(
                f"Ollamaモデル名が空です: '{model_name}'。"
                " `ollama:<model>` 形式で指定してください (例: ollama:gemma3:4b)"
            )
        return
    agent_key, _, agent_model = model_name.partition(":")
    if agent_key in AGENT_BACKENDS:
        if not agent_model:
            raise ValueError(
                f"エージェントのモデル名が空です: '{model_name}'。"
                f" `{agent_key}:<model>` 形式で指定してください"
                " (例: oc:zai-coding-plan/glm-5.2, cc:sonnet, cx:gpt-5.1)"
            )
        return
    raise ValueError(
        f"未知の翻訳モデルです: '{model_name}'。"
        " 指定可能: 'deepl', 'idx', 'ollama:<model>', 'cc:<model>' (Claude Code), "
        "'oc:<model>' (opencode), 'cx:<model>' (codex)"
    )


def _default_num_workers() -> int:
    """Ollama翻訳の並列ワーカー数を自動決定。

    優先順位:
      1. 環境変数 TRANSPAPER_NUM_WORKERS
      2. 環境変数 OLLAMA_NUM_WORKERS
      3. CPU 論理コア数（min 2, max 8 でクランプ）

    ※ Ollama の真の並列度はサーバー側の OLLAMA_NUM_PARALLEL に依存するため、
       この値はあくまでクライアント側の同時リクエスト数の目安。サーバー側の
       スロット数に合わせて環境変数で上書きすることを推奨。
    """
    for key in ("TRANSPAPER_NUM_WORKERS", "OLLAMA_NUM_WORKERS"):
        v = os.getenv(key)
        if v and v.strip().isdigit():
            return max(1, int(v))
    cpu = os.cpu_count() or 4
    return max(2, min(cpu, 8))


def translate(
    seg_results: list[SegmentPage],
    model_name="staka/fugumt-en-ja",
    out_dir: str = "out/translation",
    auth_key=None,
    batch_threshold=50,
) -> bool:
    """
    翻訳を実行する。短いテキスト(単語数がbatch_threshold未満)はバッチ処理する。
    Args:
        seg_results: セグメント分割結果のリスト
        model_name: 翻訳モデル名
            ('deepl', 'idx', 'ollama:<model>', 'cc:<model>', 'oc:<model>',
            'cx:<model>' または HuggingFaceモデル名)
        out_dir: 出力ディレクトリ
        auth_key: DeepL APIキー (deepl利用時)
        batch_threshold: この単語数未満のテキストをバッチ処理する (デフォルト: 50)
    """
    try:
        validate_model_name(model_name)
    except ValueError as e:
        print(f"ERROR: {e}")
        return False

    agent_key = model_name.partition(":")[0]
    if model_name == "deepl":
        print("Using DeepL for translation.")
    elif model_name == "idx":
        print("Using idx (no translation) for translation.")
    elif model_name.startswith("ollama:"):
        print(f"Using Ollama model '{model_name.split(':', 1)[1]}' for translation.")
        return _translate_llm_all(seg_results, model_name, out_dir, translate_ollama)
    elif agent_key in AGENT_BACKENDS:
        print(
            f"Using {AGENT_BACKENDS[agent_key]['display']} "
            f"model '{model_name.split(':', 1)[1]}' for translation."
        )
        return _translate_llm_all(seg_results, model_name, out_dir, translate_agent)
    try:
        word_count = 0
        if not Path(out_dir).exists():
            Path(out_dir).mkdir(parents=True, exist_ok=True)

        # バッチ処理用のバッファ
        batch_buffer = []  # [(block, meta, original_text), ...]
        batch_text = []  # 翻訳するテキストのリスト

        def flush_batch():
            """バッチバッファを処理して翻訳する"""
            if not batch_text:
                return

            translated_texts = _do_translate(batch_text, model_name, auth_key)

            # 翻訳結果を各ブロックに割り当て
            for (block, meta, _), translated in zip(batch_buffer, translated_texts):
                _store_translation_result(meta, translated)

            batch_buffer.clear()
            batch_text.clear()

        for res in tqdm(seg_results, desc="Translating segments"):
            for block in tqdm(res["blocks"], desc="Translating blocks", leave=False):
                block_type = block.get("type")
                meta = block.setdefault("meta", {})
                original_text = meta.get("text") or ""

                if block_type in ("text", "caption"):
                    if not original_text.strip():
                        continue

                    text_word_count = len(original_text.split())
                    word_count += text_word_count

                    # 短いテキストはバッチに追加
                    if text_word_count < batch_threshold:
                        batch_buffer.append((block, meta, original_text))
                        batch_text.append(original_text)
                    else:
                        # 長いテキストの前にバッチを処理
                        flush_batch()

                        # 長いテキストは個別に翻訳
                        translated_texts = _do_translate([original_text], model_name, auth_key)
                        _store_translation_result(meta, translated_texts[0])

            # ページ終了時にバッチを処理
            flush_batch()

            json_path = Path(res["json"])
            out_json_path = Path(out_dir) / json_path.name
            with open(out_json_path, "w", encoding="utf-8") as out_f:
                json.dump(res, out_f, ensure_ascii=False, indent=2)

        print(f"Total translated words: {word_count}")
        return True
    except Exception as e:
        print(f"Translation error: {e}")
        return False


def _translate_llm_all(
    seg_results: list[SegmentPage],
    model_name: str,
    out_dir: str,
    translate_fn,
) -> bool:
    """LLM系バックエンド（Ollama / コーディングエージェント）用の全テキスト一括収集モード。

    バッチプロンプト化と並列実行を最大限に活かすため、ページをまたいで
    全テキストを一度収集してから translate_fn() に渡し、結果を各ブロックの
    meta["translated_text"] へ順序通りに書き戻す。
    """
    try:
        if not Path(out_dir).exists():
            Path(out_dir).mkdir(parents=True, exist_ok=True)

        # 第1パス: 翻訳対象テキストを収集（block/meta と対応付けて順序を保持）
        items: list[tuple[dict, dict, str]] = []  # (block, meta, original_text)
        word_count = 0
        for res in tqdm(seg_results, desc="Collecting segments"):
            for block in res["blocks"]:
                if block.get("type") not in ("text", "caption"):
                    continue
                meta = block.setdefault("meta", {})
                original_text = (meta.get("text") or "").strip()
                if not original_text:
                    continue
                items.append((block, meta, original_text))
                word_count += len(original_text.split())

        if items:
            texts = [it[2] for it in items]
            workers = _default_num_workers()
            print(
                f"Translating {len(texts)} segments ({word_count} words) with {workers} workers..."
            )
            translated = translate_fn(texts, model_name)
            for (block, meta, _), t in zip(items, translated):
                _store_translation_result(meta, t)

        # ページごとにJSONを保存
        for res in seg_results:
            json_path = Path(res["json"])
            out_json_path = Path(out_dir) / json_path.name
            with open(out_json_path, "w", encoding="utf-8") as out_f:
                json.dump(res, out_f, ensure_ascii=False, indent=2)

        print(f"Total translated words: {word_count}")
        return True
    except Exception as e:
        print(f"Translation error: {e}")
        return False


def collect_translated_pages(translated_dir: Path) -> list[TranslatedPage]:
    pages: list[TranslatedPage] = []
    for json_path in sorted(translated_dir.glob("page_*.json")):
        with json_path.open("r", encoding="utf-8") as fh:
            page_data = json.load(fh)
        segments: list[TranslationSegment] = []
        for block in page_data.get("blocks") or []:
            bbox = block.get("bbox")
            if not bbox or len(bbox) != 4:
                continue
            meta = block.get("meta") or {}
            block_type = block.get("type", "text")
            translated_text_raw = meta.get("translated_text")

            # リストの場合は最初の要素を取得、文字列の場合はそのまま使用
            if isinstance(translated_text_raw, list):
                translated_text = (translated_text_raw[0] if translated_text_raw else "").strip()
            else:
                translated_text = (translated_text_raw or "").strip()

            if block_type in {"image", "table"}:
                segment: TranslationSegment = {
                    "type": block_type,
                    "bbox": tuple(float(v) for v in bbox),
                }
                if block.get("id"):
                    segment["id"] = block["id"]
                segments.append(segment)
                continue

            if block_type == "math":
                segment: TranslationSegment = {
                    "type": block_type,
                    "bbox": tuple(float(v) for v in bbox),
                    "translated_text": translated_text,
                }
                if block.get("id"):
                    segment["id"] = block["id"]
                segments.append(segment)
                continue

            if not translated_text:
                continue
            segment: TranslationSegment = {
                "type": block_type,
                "bbox": tuple(float(v) for v in bbox),
                "translated_text": translated_text,
            }
            if block.get("id"):
                segment["id"] = block["id"]
            source_text = meta.get("text")
            if source_text:
                segment["source_text"] = source_text
            char_count = meta.get("char_count")
            if isinstance(char_count, (int, float)):
                segment["char_count"] = int(char_count)
            avg_font_size = meta.get("avg_font_size")
            if isinstance(avg_font_size, (int, float)):
                segment["avg_font_size"] = float(avg_font_size)
            inline_math = meta.get("inline_math")
            if isinstance(inline_math, list) and inline_math:
                segment["inline_math"] = inline_math
                segment["inline_math_status"] = str(
                    meta.get("inline_math_status") or "unknown"
                )
            translation_warnings = meta.get("translation_warnings")
            if isinstance(translation_warnings, list) and translation_warnings:
                segment["translation_warnings"] = [str(item) for item in translation_warnings]
            segments.append(segment)
        pages.append(
            {
                "page": int(page_data.get("page", 0)),
                "segments": segments,
            }
        )
    return pages

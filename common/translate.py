# 2つの翻訳方法
# 1. DeepL API
# 2. HuggingFaceの翻訳モデル
import json
import os
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
        " 現在は 'deepl', 'idx', 'ollama:<model>' が利用可能です。"
    )


OLLAMA_TRANSLATE_SYSTEM_PROMPT = (
    "You are a professional academic translator. "
    "Translate the given English academic paper text into Japanese. "
    "Preserve equations, symbols, citations, references, section numbers, "
    "and inline code exactly as they are. "
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

    # 短いテキストを束ねた「ジョブ」群に分割（順序のインデックスを保持）
    jobs = _plan_ollama_jobs(texts)
    results: list[str | None] = [None] * len(texts)

    def run_job(job):
        slice_texts, indices = job
        if len(slice_texts) == 1:
            results[indices[0]] = _ollama_chat_one(slice_texts[0], ollama_model, url)
        else:
            outs = _ollama_chat_batch(slice_texts, ollama_model, url)
            for i, t in zip(indices, outs):
                results[i] = t

    if len(jobs) == 1:
        run_job(jobs[0])
    else:
        workers = min(num_workers, len(jobs))
        with ThreadPoolExecutor(max_workers=workers) as ex:
            list(tqdm(ex.map(run_job, jobs), total=len(jobs), desc="Ollama jobs"))

    # フォールバック: 欠損があれば空文字で埋める
    return [r if r is not None else "" for r in results]


def _plan_ollama_jobs(
    texts: list[str],
) -> list[tuple[list[str], list[int]]]:
    """テキストを翻訳ジョブ（バッチ or 個別）に分割。

    長いテキスト（OLLAMA_BATCH_CHAR_THRESHOLD 超）は1テキスト1ジョブ。
    短いテキストは OLLAMA_BATCH_MAX_ITEMS / OLLAMA_BATCH_MAX_CHARS の上限内で
    1つのバッチジョブに貪欲に束ねる。元の順序のインデックスを各ジョブに保持する。
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


def _do_translate(texts: list[str], model_name: str, auth_key: str | None = None) -> list[str]:
    """model_name に応じて翻訳バックエンドをディスパッチする。"""
    validate_model_name(model_name)
    if model_name == "idx":
        return idx(texts)
    if model_name == "deepl":
        return translate_deepl(texts, target_lang="JA", auth_key=auth_key)
    if model_name.startswith("ollama:"):
        return translate_ollama(texts, model_name=model_name)
    # validate_model_name で弾くのでここには到達しない
    raise ValueError(f"未知の翻訳モデルです: '{model_name}'")


def validate_model_name(model_name: str) -> None:
    """翻訳モデル名が有効か検証する。無効な場合は ValueError を発生させる。

    有効な指定:
        - 'deepl'
        - 'idx'
        - 'ollama:<model>' (例: 'ollama:gemma3:4b')
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
    raise ValueError(
        f"未知の翻訳モデルです: '{model_name}'。"
        " 指定可能: 'deepl', 'idx', 'ollama:<model>' (例: ollama:gemma3:4b)"
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
            ('deepl', 'idx', 'ollama:<model>' または HuggingFaceモデル名)
        out_dir: 出力ディレクトリ
        auth_key: DeepL APIキー (deepl利用時)
        batch_threshold: この単語数未満のテキストをバッチ処理する (デフォルト: 50)
    """
    try:
        validate_model_name(model_name)
    except ValueError as e:
        print(f"ERROR: {e}")
        return False

    if model_name == "deepl":
        print("Using DeepL for translation.")
    elif model_name == "idx":
        print("Using idx (no translation) for translation.")
    elif model_name.startswith("ollama:"):
        print(f"Using Ollama model '{model_name.split(':', 1)[1]}' for translation.")
        return _translate_ollama_all(seg_results, model_name, out_dir)
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
                meta["translated_text"] = translated

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
                        meta["translated_text"] = translated_texts[0]

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


def _translate_ollama_all(
    seg_results: list[SegmentPage],
    model_name: str,
    out_dir: str,
) -> bool:
    """Ollama 用の全テキスト一括収集モード。

    バッチプロンプト化と並列実行を最大限に活かすため、ページをまたいで
    全テキストを一度収集してから translate_ollama() に渡し、結果を各ブロックの
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
            translated = translate_ollama(texts, model_name)
            for (block, meta, _), t in zip(items, translated):
                meta["translated_text"] = t

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
            segments.append(segment)
        pages.append(
            {
                "page": int(page_data.get("page", 0)),
                "segments": segments,
            }
        )
    return pages

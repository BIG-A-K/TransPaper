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


def translate_ollama(
    texts: list[str],
    model_name: str,
    base_url: str | None = None,
    timeout: float = 300.0,
    num_workers: int | None = None,
) -> list[str]:
    """OllamaのHTTP API経由でローカルLLM翻訳を行う。

    複数テキストは並列リクエストで処理する（入力と同じ順序で返る）。
    サーバー側で `OLLAMA_NUM_PARALLEL` を上げておくことで真の並列推論になる。

    Args:
        texts: 翻訳対象テキストのリスト
        model_name: `ollama:<model>` 形式のモデル指定
        base_url: OllamaサーバーのベースURL。未指定時は `OLLAMA_HOST` 環境変数、
            さらに未設定なら `http://localhost:11434`
        timeout: 1リクエストのタイムアウト秒
        num_workers: 並列リクエスト数。未指定時は `OLLAMA_NUM_WORKERS` 環境変数、
            さらに未設定なら 8

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
        num_workers = max(1, int(os.getenv("OLLAMA_NUM_WORKERS", "8")))

    # 1テキストならスレッドプールのオーバーヘッドを避ける
    if len(texts) == 1:
        return [_ollama_chat_one(texts[0], ollama_model, url, timeout)]

    workers = min(num_workers, len(texts))
    with ThreadPoolExecutor(max_workers=workers) as ex:
        return list(
            ex.map(lambda t: _ollama_chat_one(t, ollama_model, url, timeout), texts)
        )


def _ollama_chat_one(text: str, ollama_model: str, url: str, timeout: float) -> str:
    """1テキストを Ollama の `/api/chat` で翻訳する。"""
    payload = {
        "model": ollama_model,
        "messages": [
            {"role": "system", "content": OLLAMA_TRANSLATE_SYSTEM_PROMPT},
            {"role": "user", "content": text},
        ],
        "stream": False,
    }
    try:
        response = requests.post(
            url, headers={"Content-Type": "application/json"}, json=payload, timeout=timeout
        )
        response.raise_for_status()
        result = response.json()
        return str(result["message"]["content"]).strip()
    except requests.exceptions.ConnectionError as e:
        raise RuntimeError(
            f"Ollamaサーバーに接続できませんでした ({url})。"
            " `ollama serve` で起動しているか確認してください。"
        ) from e
    except requests.exceptions.Timeout as e:
        raise RuntimeError(
            f"Ollamaサーバーがタイムアウトしました ({url})。"
            " モデルが大きすぎるか、GPUメモリ不足の可能性があります。"
        ) from e
    except requests.exceptions.HTTPError as e:
        body = ""
        if e.response is not None:
            body = e.response.text
        if e.response is not None and e.response.status_code == 404:
            raise RuntimeError(
                f"Ollamaモデル '{ollama_model}' が見つかりません。"
                f" `ollama pull {ollama_model}` で取得してください。"
            ) from e
        raise RuntimeError(f"Ollama APIエラー: {e} body={body}") from e
    except KeyError as e:
        raise RuntimeError(f"Ollamaのレスポンス形式が想定と異なります: {result}") from e


def _do_translate(
    texts: list[str], model_name: str, auth_key: str | None = None
) -> list[str]:
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

# 2つの翻訳方法
# 1. DeepL API
# 2. HuggingFaceの翻訳モデル
import json
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
    # TODO: 実装
    return idx(texts)
    # from transformers import MarianMTModel, MarianTokenizer

    # if not texts:
    #     return []

    # # 単一のテキストが渡された場合でもリストとして処理
    # if isinstance(texts, str):
    #     texts = [texts]

    # tokenizer = MarianTokenizer.from_pretrained(model_name)
    # model = MarianMTModel.from_pretrained(model_name)

    # # 各テキストを個別に翻訳
    # translated_texts = []
    # for text in texts:
    #     translated = model.generate(**tokenizer(text, return_tensors="pt", padding=True))
    #     translated_text = tokenizer.decode(translated[0], skip_special_tokens=True)
    #     translated_texts.append(translated_text)

    # return translated_texts


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
        model_name: 翻訳モデル名 ('deepl' または HuggingFaceモデル名)
        out_dir: 出力ディレクトリ
        auth_key: DeepL APIキー
        batch_threshold: この単語数未満のテキストをバッチ処理する (デフォルト: 50)
    """
    if model_name == "deepl":
        print("Using DeepL for translation.")
    elif model_name == "idx":
        print("Using idx (no translation) for translation.")
    else:
        print(f"Using HuggingFace model '{model_name}' for translation.")
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

            if model_name == "idx":
                translated_texts = idx(batch_text)

            elif model_name == "deepl":
                # DeepLで翻訳
                translated_texts = translate_deepl(batch_text, target_lang="JA", auth_key=auth_key)
            else:
                # HuggingFaceモデルで翻訳
                translated_texts = translate_huggingface(batch_text, model_name=model_name)

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
                        if model_name == "deepl":
                            translated_texts = translate_deepl(
                                [original_text], target_lang="JA", auth_key=auth_key
                            )
                            translated_text = translated_texts[0]
                        elif model_name == "idx":
                            translated_texts = idx([original_text])
                            translated_text = translated_texts[0]
                        else:
                            translated_texts = translate_huggingface(
                                [original_text], model_name=model_name
                            )
                            translated_text = translated_texts[0]
                        meta["translated_text"] = translated_text

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

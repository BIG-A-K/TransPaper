import json
import os
from pathlib import Path

import click
from loguru import logger

from common import compose as composemod
from common import seg as segmod
from common import translate
from common.schema import SegmentPage


@click.command()
@click.option("--input", "-i", type=click.Path(exists=True), required=True, help="Input file path")
@click.option("--output", "-o", type=click.Path(), help="Output file path", default=None)
@click.option(
    "--model",
    "-m",
    type=str,
    # default='staka/fugumt-en-ja',
    default="deepl",
    # default='idx',
    help="Translation model to use (for test: idx)",
)
def main(input, output, model):
    input_path = Path(input)
    if input_path.suffix.lower() != ".pdf":
        raise click.ClickException("Input file must be a PDF")
    if output is None:
        output = f"translated_{input_path.stem}.pdf"
    logger.info(f"Input PDF: {input_path}")
    logger.info(f"Output PDF: {output}")
    # seg画像などは/tmpに保存する
    working_dir = Path("/tmp/" + f"_{input_path.stem}")
    working_dir.mkdir(parents=True, exist_ok=True)

    logger.info(f"Model: {model}")
    n = 4
    #  1.segment分割を実行する。
    logger.info(f"(1/{n}) segmentation...")
    seg_results: list[SegmentPage] = segmod.segment_pdf(
        input_path, outdir=f"{working_dir}/segments"
    )
    #  2.翻訳を実行する。
    logger.info(f"(2/{n}) translation...")
    translated_dir = Path(working_dir / "translated")
    translation_ok = translate.translate(
        seg_results,
        model_name=model,
        out_dir=str(translated_dir),
        auth_key=os.getenv("DEEPL_API"),
    )
    if not translation_ok:
        raise click.ClickException("翻訳に失敗しました")
    #  3.翻訳結果を収集する。
    logger.info(f"(3/{n}) collecting translated pages...")
    translated_pages = translate.collect_translated_pages(translated_dir)
    if not translated_pages:
        raise click.ClickException("再構成できる翻訳結果が見つかりませんでした")

    document_translation_path = translated_dir / "document_translation.json"
    with document_translation_path.open("w", encoding="utf-8") as fh:
        json.dump(translated_pages, fh, ensure_ascii=False, indent=2)
    # 4.翻訳結果をPDFに再構成する。
    logger.info(f"(4/{n}) composing translated PDF...")
    compose_dir = working_dir / "composed"
    compose_dir.mkdir(parents=True, exist_ok=True)

    compose_result = composemod.compose_pdf(
        original_pdf=input_path,
        translated_pages=document_translation_path,
        output_pdf=output,
    )
    logger.info(f"Composed PDF: {compose_result.output_path}")
    if compose_result.warning_count:
        logger.warning(f"Compose warnings: {compose_result.warning_count}")
        for warning in compose_result.warnings:
            logger.warning(warning)


if __name__ == "__main__":
    main()

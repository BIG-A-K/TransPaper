from __future__ import annotations

import json
import unittest
from unittest import mock

from common import translate
from common.translate import (
    _agent_command,
    _extract_json_object,
    _run_agent,
    translate_agent,
    validate_model_name,
)


class ValidateModelNameAgentTests(unittest.TestCase):
    def test_accepts_agent_prefixes(self) -> None:
        for name in (
            "cc:sonnet",
            "cc:claude-sonnet-4-5",
            "oc:zai-coding-plan/glm-5.2",
            "cx:gpt-5.1",
        ):
            with self.subTest(model=name):
                validate_model_name(name)

    def test_rejects_empty_agent_model(self) -> None:
        for name in ("cc:", "oc:", "cx:"):
            with self.subTest(model=name):
                with self.assertRaises(ValueError):
                    validate_model_name(name)

    def test_rejects_unknown_prefix(self) -> None:
        for name in ("foo:bar", "agent", "oco:gpt"):
            with self.subTest(model=name):
                with self.assertRaises(ValueError):
                    validate_model_name(name)


class AgentCommandTests(unittest.TestCase):
    def test_claude(self) -> None:
        self.assertEqual(
            _agent_command("cc", "sonnet", "hello"),
            ["claude", "-p", "hello", "--model", "sonnet"],
        )

    def test_opencode(self) -> None:
        self.assertEqual(
            _agent_command("oc", "zai-coding-plan/glm-5.2", "hello"),
            ["opencode", "run", "hello", "--model", "zai-coding-plan/glm-5.2"],
        )

    def test_codex_uses_readonly_sandbox(self) -> None:
        cmd = _agent_command("cx", "gpt-5.1", "hello", "/tmp/out.txt")
        self.assertEqual(cmd[0], "codex")
        self.assertIn("exec", cmd)
        self.assertIn("-m", cmd)
        self.assertIn("gpt-5.1", cmd)
        self.assertIn("read-only", cmd)
        self.assertIn("--skip-git-repo-check", cmd)
        self.assertIn("-o", cmd)
        self.assertIn("/tmp/out.txt", cmd)

    def test_unknown_agent_raises(self) -> None:
        with self.assertRaises(ValueError):
            _agent_command("zz", "model", "hello")


class ExtractJsonObjectTests(unittest.TestCase):
    def test_plain_json(self) -> None:
        self.assertEqual(
            _extract_json_object('{"translations": ["a"]}'),
            {"translations": ["a"]},
        )

    def test_json_fence(self) -> None:
        text = "Here you go:\n```json\n{\"translations\": [\"a\", \"b\"]}\n```\nDone."
        self.assertEqual(
            _extract_json_object(text),
            {"translations": ["a", "b"]},
        )

    def test_json_with_prose(self) -> None:
        text = '結果は {"translations": ["x"]} です。'
        self.assertEqual(
            _extract_json_object(text),
            {"translations": ["x"]},
        )

    def test_broken_returns_none(self) -> None:
        self.assertIsNone(_extract_json_object("no json here"))
        self.assertIsNone(_extract_json_object('{"translations": [broken'))

    def test_non_dict_json_returns_none(self) -> None:
        self.assertIsNone(_extract_json_object('["a", "b"]'))


class TranslateAgentTests(unittest.TestCase):
    def test_batch_success_preserves_order(self) -> None:
        # 短いテキスト3件 -> 1つのバッチジョブに束ねられる
        texts = ["one", "two", "three"]

        def fake_run(agent_key, agent_model, prompt):
            self.assertEqual(agent_key, "oc")
            self.assertEqual(agent_model, "zai-coding-plan/glm-5.2")
            # プロンプトに入力JSONが含まれていること
            self.assertIn(json.dumps({"texts": texts}, ensure_ascii=False), prompt)
            return '{"translations": ["一", "二", "三"]}'

        with mock.patch.object(translate, "_run_agent", side_effect=fake_run):
            result = translate_agent(texts, "oc:zai-coding-plan/glm-5.2")

        self.assertEqual(result, ["一", "二", "三"])

    def test_long_text_is_translated_individually(self) -> None:
        long_text = "word " * 200  # OLLAMA_BATCH_CHAR_THRESHOLD 超
        with mock.patch.object(
            translate, "_run_agent", return_value="長い訳文"
        ) as run_mock:
            result = translate_agent([long_text], "cc:sonnet")
        self.assertEqual(result, ["長い訳文"])
        # 1テキスト1リクエストで、指示がプロンプトに含まれること
        self.assertEqual(run_mock.call_count, 1)
        prompt = run_mock.call_args[0][2]
        self.assertIn("[Text to translate]", prompt)
        self.assertIn(long_text, prompt)

    def test_batch_fallback_on_count_mismatch(self) -> None:
        texts = ["a", "b", "c"]
        calls: list[str] = []

        def fake_run(agent_key, agent_model, prompt):
            calls.append(prompt)
            if len(calls) == 1:
                # バッチ応答の件数が不正 -> 個別翻訳へフォールバック
                return '{"translations": ["x"]}'
            # 個別翻訳: プロンプト末尾のテキスト部分と厳密に一致する
            marker = "[Text to translate]\n"
            self.assertTrue(prompt.startswith(marker) or marker in prompt)
            body = prompt.split(marker, 1)[1]
            self.assertIn(body, texts)
            return f"訳:{body}"

        with mock.patch.object(translate, "_run_agent", side_effect=fake_run):
            result = translate_agent(texts, "cx:gpt-5.1")

        self.assertEqual(result, ["訳:a", "訳:b", "訳:c"])
        self.assertEqual(len(calls), 4)  # バッチ1 + 個別3

    def test_batch_fallback_on_non_json(self) -> None:
        texts = ["a", "b"]
        with mock.patch.object(
            translate, "_run_agent", return_value="訳文です"
        ) as run_mock:
            result = translate_agent(texts, "cc:sonnet")
        # バッチ1(非JSON) + 個別2 でフォールバック
        self.assertEqual(run_mock.call_count, 3)
        self.assertEqual(result, ["訳文です", "訳文です"])

    def test_empty_and_invalid(self) -> None:
        self.assertEqual(translate_agent([], "oc:m"), [])
        with mock.patch.object(translate, "_run_agent", return_value="訳") as run_mock:
            # strは1要素リストとして扱われる
            self.assertEqual(translate_agent("hello", "oc:m"), ["訳"])
            self.assertEqual(run_mock.call_count, 1)
        with self.assertRaises(ValueError):
            translate_agent(["x"], "zz:model")
        with self.assertRaises(ValueError):
            translate_agent(["x"], "oc:")

    def test_missing_cli_raises_runtime_error(self) -> None:
        # PATH上にCLIが無い状況をシミュレート
        with mock.patch.object(translate.shutil, "which", return_value=None):
            with self.assertRaises(RuntimeError) as ctx:
                _run_agent("cc", "sonnet", "hello")
        self.assertIn("claude", str(ctx.exception))


class DispatchTests(unittest.TestCase):
    def test_do_translate_dispatches_agent(self) -> None:
        with mock.patch.object(
            translate, "translate_agent", return_value=["訳"]
        ) as agent_mock:
            result = translate._do_translate(["hello"], "oc:some/model")
        self.assertEqual(result, ["訳"])
        agent_mock.assert_called_once_with(["hello"], model_name="oc:some/model")

    def test_translate_entry_dispatches_agent_llm_all(self) -> None:
        seg_results = [
            {
                "page": 1,
                "json": "/tmp/page_001.json",
                "blocks": [
                    {
                        "type": "text",
                        "bbox": (0, 0, 10, 10),
                        "meta": {"text": "hello world"},
                    }
                ],
            }
        ]
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as td:
            with mock.patch.object(
                translate, "translate_agent", return_value=["こんにちは世界"]
            ) as agent_mock:
                ok = translate.translate(
                    seg_results, model_name="cc:sonnet", out_dir=td
                )
        self.assertTrue(ok)
        agent_mock.assert_called_once()
        self.assertEqual(
            seg_results[0]["blocks"][0]["meta"]["translated_text"], "こんにちは世界"
        )


if __name__ == "__main__":
    unittest.main()

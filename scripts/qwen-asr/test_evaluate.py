import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from evaluate import word_errors
from model_files import verify


class EvaluationTests(unittest.TestCase):
    def test_scoring_counts_changes_but_ignores_case_and_punctuation(self):
        self.assertEqual(word_errors("Okay, thank you!", "okay thank you"), 0)
        self.assertEqual(word_errors("do not delete this", "do delete this"), 1)
        self.assertEqual(word_errors("", "invented speech"), 2)

    def test_corrupt_file_cannot_be_used_for_inference(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "model.bin").write_bytes(b"wrong bytes")
            with patch("model_files.pins", return_value={"files": {"model.bin": "0" * 64}}):
                with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                    verify(root)


if __name__ == "__main__":
    unittest.main()

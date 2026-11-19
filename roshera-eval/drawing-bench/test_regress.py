"""Tests for regress.py -- the before/after judge for the drawing rewrite.

Run: python -I -m pytest roshera-eval/drawing-bench/test_regress.py -q
(or: python -I roshera-eval/drawing-bench/test_regress.py)
"""
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import regress  # noqa: E402


def summary(errors=0, not_dim=0, complete=True, sound=True, stage="done", ms=1000, stale=0):
    return {
        "stage": stage,
        "quality": {"errors": errors, "warnings": 0, "passed": errors == 0},
        "certificate": {
            "complete": complete,
            "sound": sound,
            "counts": {"not_dimensioned": not_dim, "stale": stale, "dangling": 0},
        },
        "sheet": {"ms": ms, "ok": stage == "done", "size": "A3", "dimensions_inked": 10},
    }


def write(root, pid, s):
    os.makedirs(os.path.join(root, pid), exist_ok=True)
    with open(os.path.join(root, pid, "summary.json"), "w", encoding="utf-8") as f:
        json.dump(s, f)


class Judge(unittest.TestCase):
    def test_fewer_errors_and_missing_dims_is_better(self):
        v = regress.judge(summary(errors=5, not_dim=9, complete=False), summary(errors=1, not_dim=1, complete=False))
        self.assertEqual(v["verdict"], "better")

    def test_more_layout_errors_is_worse(self):
        v = regress.judge(summary(errors=0), summary(errors=2))
        self.assertEqual(v["verdict"], "worse")
        self.assertIn("layout errors 0 -> 2", v["why"])

    def test_losing_soundness_is_worse_even_if_errors_drop(self):
        v = regress.judge(summary(errors=4, sound=True), summary(errors=0, sound=False))
        self.assertEqual(v["verdict"], "worse")

    def test_a_sheet_that_no_longer_draws_is_worse(self):
        v = regress.judge(summary(), summary(stage="sheet_failed"))
        self.assertEqual(v["verdict"], "worse")

    def test_much_slower_is_worse_small_jitter_is_not(self):
        self.assertEqual(regress.judge(summary(ms=4000), summary(ms=12000))["verdict"], "worse")
        self.assertEqual(regress.judge(summary(ms=4000), summary(ms=4500))["verdict"], "same")

    def test_a_trade_is_mixed_not_better(self):
        v = regress.judge(summary(errors=3, not_dim=1), summary(errors=1, not_dim=4))
        self.assertEqual(v["verdict"], "worse")

    def test_unchanged_is_same(self):
        self.assertEqual(regress.judge(summary(errors=1), summary(errors=1))["verdict"], "same")


class Run(unittest.TestCase):
    def test_compare_dirs_flags_any_worse_part(self):
        with tempfile.TemporaryDirectory() as d:
            before, after = os.path.join(d, "b"), os.path.join(d, "a")
            write(before, "101", summary(errors=3))
            write(after, "101", summary(errors=0))
            write(before, "102", summary(errors=0))
            write(after, "102", summary(errors=1))
            rows, worse = regress.compare(before, after)
            self.assertEqual([r["id"] for r in rows], ["101", "102"])
            self.assertEqual(worse, ["102"])

    def test_a_part_missing_after_is_worse(self):
        with tempfile.TemporaryDirectory() as d:
            before, after = os.path.join(d, "b"), os.path.join(d, "a")
            write(before, "101", summary())
            os.makedirs(after)
            _, worse = regress.compare(before, after)
            self.assertEqual(worse, ["101"])


if __name__ == "__main__":
    unittest.main()

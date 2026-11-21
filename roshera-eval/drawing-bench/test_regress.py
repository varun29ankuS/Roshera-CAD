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


def summary(errors=0, not_dim=0, complete=True, sound=True, stage="done", ms=1000, stale=0,
            util=0.3, size="A3", scale=1.0, grade=None, below_bar=None):
    s = {
        "stage": stage,
        "quality": {"errors": errors, "warnings": 0, "passed": errors == 0, "sheet_utilization": util},
        "certificate": {
            "complete": complete,
            "sound": sound,
            "counts": {"not_dimensioned": not_dim, "stale": stale, "dangling": 0},
        },
        "sheet": {"ms": ms, "ok": stage == "done", "size": size, "dimensions_inked": 10,
                  "views": [{"name": "FRONT", "scale": scale}]},
    }
    if grade is not None:
        # the space map's record, as drawing_bench writes it
        s["spaces"] = {"grade": grade, "below_bar": below_bar, "bar": 0.8, "spaces": 20,
                       "worst": "title_block", "by_rule": {}}
    return s


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

    def test_a_fuller_sheet_is_better(self):
        v = regress.judge(summary(util=0.08, size="A2", scale=0.5), summary(util=0.32, size="A2"))
        self.assertEqual(v["verdict"], "better")

    def test_a_fuller_sheet_with_a_new_error_is_worse(self):
        v = regress.judge(summary(util=0.08), summary(util=0.32, errors=1))
        self.assertEqual(v["verdict"], "worse")

    def test_a_sheet_that_becomes_underfilled_is_worse(self):
        v = regress.judge(summary(util=0.25), summary(util=0.06))
        self.assertEqual(v["verdict"], "worse")

    def test_the_report_states_size_scale_and_utilization(self):
        rows = [{"id": "124", "verdict": "better", "why": [],
                 **{k: regress._facts(x) for k, x in (("before", summary(util=0.09, size="A1")),
                                                     ("after", summary(util=0.3, size="A2", scale=0.5)))}}]
        text = regress.markdown(rows, [])
        self.assertIn("| A1 -> A2 | 1:1 -> 1:2 | 9.0% -> 30.0% |", text)

    def test_unchanged_is_same(self):
        self.assertEqual(regress.judge(summary(errors=1), summary(errors=1))["verdict"], "same")

    # The space map (drafting craft): the grade is the sheet's WORST space.
    def test_a_lower_grade_is_worse(self):
        v = regress.judge(summary(grade=0.6, below_bar=3), summary(grade=0.5, below_bar=3))
        self.assertEqual(v["verdict"], "worse")
        self.assertIn("grade 0.600 -> 0.500", v["why"])

    def test_more_spaces_below_the_bar_is_worse_even_at_the_same_grade(self):
        v = regress.judge(summary(grade=0.6, below_bar=3), summary(grade=0.6, below_bar=4))
        self.assertEqual(v["verdict"], "worse")
        self.assertIn("spaces below bar 3 -> 4", v["why"])

    def test_a_higher_grade_or_fewer_spaces_below_is_better(self):
        self.assertEqual(regress.judge(summary(grade=0.5, below_bar=3),
                                       summary(grade=0.7, below_bar=3))["verdict"], "better")
        self.assertEqual(regress.judge(summary(grade=0.5, below_bar=3),
                                       summary(grade=0.5, below_bar=1))["verdict"], "better")

    def test_a_better_grade_bought_with_a_new_layout_error_is_worse(self):
        v = regress.judge(summary(grade=0.5, below_bar=3), summary(grade=0.9, below_bar=0, errors=1))
        self.assertEqual(v["verdict"], "worse")

    def test_float_noise_in_the_grade_is_not_a_change(self):
        v = regress.judge(summary(grade=0.6, below_bar=3), summary(grade=0.6 - 1e-9, below_bar=3))
        self.assertEqual(v["verdict"], "same")

    def test_a_sheet_from_before_the_space_map_is_not_judged_on_it(self):
        self.assertEqual(regress.judge(summary(), summary(grade=0.4, below_bar=9))["verdict"], "same")

    def test_losing_the_space_map_is_worse(self):
        v = regress.judge(summary(grade=0.6, below_bar=3), summary())
        self.assertEqual(v["verdict"], "worse")
        self.assertIn("space map missing", v["why"])


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


class Floors(unittest.TestCase):
    """The ratchet: a checked-in snapshot of each part's facts from an observed
    run (runs/ is gitignored), judged with the same rules as before/after."""

    def test_floors_written_from_a_run_hold_each_parts_facts(self):
        with tempfile.TemporaryDirectory() as d:
            run = os.path.join(d, "run")
            write(run, "101", summary(errors=2, grade=0.55, below_bar=4))
            path = os.path.join(d, "floors.json")
            regress.write_floors(run, path)
            with open(path, encoding="utf-8") as f:
                floors = json.load(f)
            self.assertEqual(floors["parts"]["101"]["errors"], 2)
            self.assertEqual(floors["parts"]["101"]["grade"], 0.55)
            self.assertEqual(floors["parts"]["101"]["below_bar"], 4)

    def test_a_run_below_its_floors_fails_and_one_at_them_passes(self):
        with tempfile.TemporaryDirectory() as d:
            base, path = os.path.join(d, "base"), os.path.join(d, "floors.json")
            write(base, "101", summary(grade=0.55, below_bar=4))
            write(base, "102", summary(grade=0.70, below_bar=1))
            regress.write_floors(base, path)
            same = os.path.join(d, "same")
            write(same, "101", summary(grade=0.55, below_bar=4))
            write(same, "102", summary(grade=0.70, below_bar=1))
            _, worse = regress.check_floors(path, same)
            self.assertEqual(worse, [])
            slipped = os.path.join(d, "slipped")
            write(slipped, "101", summary(grade=0.55, below_bar=4))
            write(slipped, "102", summary(grade=0.60, below_bar=1))
            _, worse = regress.check_floors(path, slipped)
            self.assertEqual(worse, ["102"])

    def test_a_part_with_a_floor_but_no_result_fails(self):
        with tempfile.TemporaryDirectory() as d:
            base, path = os.path.join(d, "base"), os.path.join(d, "floors.json")
            write(base, "101", summary(grade=0.55, below_bar=4))
            regress.write_floors(base, path)
            empty = os.path.join(d, "empty")
            os.makedirs(empty)
            _, worse = regress.check_floors(path, empty)
            self.assertEqual(worse, ["101"])


if __name__ == "__main__":
    unittest.main()

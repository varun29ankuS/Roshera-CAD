#!/usr/bin/env python3
"""Tests of score.py's matching rules. Run: python -I test_score.py

Each case is a defect of the first scorer (value-only matching, substring
hole search, recall with no precision) written as a sheet that used to score
well and must not.
"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from score import score  # noqa: E402


def sheet(inked, holes=(), views=("FRONT", "TOP", "RIGHT", "ISOMETRIC"), section=False, errors=0):
    return {
        "stage": "done",
        "sheet": {
            "inked": [{"kind": k, "value": v, "label": label} for k, v, label in inked],
            "hole_table": [{"diameter_mm": d, "size": size, "depth": depth} for d, size, depth in holes],
            "views": [{"name": n} for n in views],
            "has_section": section,
        },
        "quality": {"errors": errors},
    }


def run(summary, ref):
    return score(summary, ref, 0.005, 0.05)


class Matching(unittest.TestCase):
    def test_a_value_under_the_wrong_kind_is_not_found(self):
        # A 12 mm length on the sheet does not satisfy a reference R12.
        r = run(sheet([("length", 12.0, "12.00")]), {"dims": [{"kind": "radius", "value": 12.0}]})
        self.assertEqual(r["dims"]["found"], 0)
        self.assertEqual(len(r["dims"]["wrong_kind"]), 1)
        self.assertEqual(r["dims"]["recall"], 0.0)

    def test_one_inked_dimension_satisfies_one_reference_dimension(self):
        ref = {"dims": [{"kind": "linear", "value": 12.0, "feature": "step A"},
                        {"kind": "linear", "value": 12.0, "feature": "step B"}]}
        r = run(sheet([("length", 12.0, "12.00")]), ref)
        self.assertEqual(r["dims"]["found"], 1, "two different 12 mm steps need two inked 12s")
        r = run(sheet([("length", 12.0, "12.00"), ("extent", 12.0, "12.00")]), ref)
        self.assertEqual(r["dims"]["found"], 2)

    def test_a_count_is_met_by_a_counted_callout_or_by_that_many_dimensions(self):
        ref = {"dims": [{"kind": "diameter", "value": 6.6, "count": 4}]}
        self.assertEqual(run(sheet([("diameter", 6.6, "Ø6.60")]), ref)["dims"]["found"], 0)
        self.assertEqual(run(sheet([("hole_callout", 6.6, "4× Ø6.60 THRU")]), ref)["dims"]["found"], 1)
        self.assertEqual(run(sheet([("diameter", 6.6, "Ø6.60")] * 4), ref)["dims"]["found"], 1)

    def test_over_dimensioning_costs_precision(self):
        ref = {"dims": [{"kind": "linear", "value": 40.0}]}
        r = run(sheet([("extent", 40.0, "40.00")] + [("length", float(v), str(v)) for v in range(1, 10)]), ref)
        self.assertEqual(r["dims"]["recall"], 1.0)
        self.assertEqual(r["dims"]["precision"], 0.1)

    def test_a_hole_diameter_is_matched_as_a_number_not_a_substring(self):
        # The first scorer searched "12" in the table text and found it in 112.
        ref = {"holes": [{"dia": 12.0, "count": 1}]}
        r = run(sheet([], holes=[(112.0, "Ø112.00", "THRU")]), ref)
        self.assertEqual(r["holes"]["recall"], 0.0)
        r = run(sheet([], holes=[(12.0, "Ø12.00", "THRU")]), ref)
        self.assertEqual(r["holes"]["recall"], 1.0)

    def test_each_claim_of_a_hole_is_checked_on_its_own_row(self):
        ref = {"holes": [{"dia": 6.6, "count": 1, "thru": True, "cbore_dia": 11.0, "thread": "M8x1.25"}]}
        r = run(sheet([], holes=[(6.6, "Ø6.60 ⌴ Ø11.00 ↧ 6.80", "THRU")]), ref)
        self.assertEqual(r["holes"]["recall"], 0.0)
        self.assertIn("thread not stated", r["holes"]["incomplete_or_missing"][0]["why"])
        self.assertNotIn("counterbore", r["holes"]["incomplete_or_missing"][0]["why"])

    def test_views_come_from_the_view_list_not_from_any_text(self):
        # "SECTION" in a note does not make a section view.
        ref = {"views": ["front", "section", "detail"]}
        r = run(sheet([("length", 1.0, "SEE SECTION DETAIL")]), ref)
        self.assertEqual(r["views"]["missing"], ["section", "detail"])
        r = run(sheet([], section=True), ref)
        self.assertEqual(r["views"]["missing"], ["detail"])

    def test_an_unknown_kind_is_reported_never_taken_for_linear(self):
        # The first scorer mapped any kind it did not know to "linear", so a
        # new sheet kind silently scored as a length.
        r = run(sheet([("thread_depth", 12.0, "12")]), {"dims": [{"kind": "linear", "value": 12.0}]})
        self.assertEqual(r["dims"]["found"], 0)
        self.assertEqual(r["dims"]["unknown_kinds"], {"thread_depth": 1})

    def test_real_sheet_kinds_are_all_classed(self):
        # Every kind the sheet pipeline inks on the 43 baseline sheets
        # (collected from runs/baseline/*/summary.json on 2026-11-18).
        from score import KIND_CLASS
        for k in ("fillet", "length", "extent", "chamfer", "diameter", "location",
                  "hole_callout", "pitch", "pcd", "angle", "radius", "position"):
            self.assertIn(k, KIND_CLASS, k)

    def test_a_real_counted_label_reads_its_count(self):
        from score import count_of
        self.assertEqual(count_of("4× Ø9.20 THRU"), 4)
        self.assertEqual(count_of("4x R2.5"), 4)
        self.assertEqual(count_of("115.00"), 1)

    def test_a_sheet_that_was_not_built_scores_nothing(self):
        r = run({"stage": "sheet"}, {"dims": [{"kind": "linear", "value": 1.0}], "views": ["front"]})
        self.assertFalse(r["built"])
        self.assertEqual(r["dims"]["recall"], 0.0)
        self.assertEqual(r["views"]["coverage"], 0.0)


if __name__ == "__main__":
    unittest.main()

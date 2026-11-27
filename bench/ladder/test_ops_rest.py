"""The ops stage's rest offsets: in [0, 1/60 s), the same for every tool,
reproducible from the seed; and the cells' mean and one-frame share. No
browser or tool needed."""
import importlib.util
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE / 'contenders'))
import ops_agent
spec = importlib.util.spec_from_file_location('ladder', HERE / 'ladder.py')
ladder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ladder)


def tool(code):
    args = SimpleNamespace(tool=code, engine='chrome', tag='', session='test',
                           navigera='/bin/navigera', chrome='/bin/chrome', own_browser=False)
    return ops_agent.Tool(args, 'http://fixture', {}, Path('/tmp'))


def run(t, op, seed, rep_offset, reps=5):
    """run_op with a fake op whose timed call follows the op's own rest (or
    none): returns the cell and every sleep before each timed call."""
    sleeps, seen = [], []

    def fake(self, rep):
        if op in ops_agent.OWN_REST_OPS:
            self.rest(ops_agent.SETUP_REST_S)
        seen.append(list(sleeps))
        sleeps.clear()
        return 20.0 + rep, 1, ''
    with patch.dict(ops_agent.Tool.OP_METHODS, {op: fake}), \
            patch.object(ops_agent.time, 'sleep', sleeps.append):
        cell = t.run_op(op, reps, seed, rep_offset)
    return cell, seen


class RestOffsets(unittest.TestCase):
    def test_in_one_frame_and_reproducible_from_the_seed(self):
        for op in ops_agent.OPS:
            offs = ops_agent.rest_offsets(45950542045, op, 10)
            self.assertEqual(len(offs), 10)
            self.assertTrue(all(0 <= x < 1 / 60 for x in offs), offs)
            self.assertEqual(offs, ops_agent.rest_offsets(45950542045, op, 10))
            self.assertNotEqual(offs, ops_agent.rest_offsets(7, op, 10))
        # A prefix: round 1 draws the cell's first 5, round 2 the next 5.
        self.assertEqual(ops_agent.rest_offsets(3, 'click', 5), ops_agent.rest_offsets(3, 'click', 10)[:5])
        # Spread over the frame, not a fixed phase.
        offs = ops_agent.rest_offsets(1, 'screenshot-720', 1000)
        self.assertLess(min(offs), 0.001)
        self.assertGreater(max(offs), 1 / 60 - 0.001)

    def test_every_tool_meets_the_same_offsets(self):
        for op in ('screenshot-720', 'screenshot', 'click', 'goto'):
            cells = {}
            for code in ('nv', 'ab', 'cdmcp', 'pwmcp'):
                r1, s1 = run(tool(code), op, 11, 0)
                r2, s2 = run(tool(code), op, 11, 5)
                cells[code] = (r1['rest_offsets_ms'] + r2['rest_offsets_ms'], s1 + s2)
            want = [round(x * 1000, 3) for x in ops_agent.rest_offsets(11, op, 10)]
            for code, (offs, sleeps) in cells.items():
                self.assertEqual(offs, want, (op, code))
                # The sleep before each timed call is the rest plus the offset
                # (or the offset alone for an op with no rest of its own).
                base = ops_agent.SETUP_REST_S if op in ops_agent.OWN_REST_OPS else 0.0
                self.assertEqual([round((s[-1] - base) * 1000, 3) for s in sleeps], want, (op, code))

    def test_no_offset_left_for_the_next_op(self):
        t = tool('nv')
        run(t, 'click', 5, 0)
        self.assertEqual(t.jitter_s, 0.0)


class OpsCells(unittest.TestCase):
    def test_mean_beside_median_and_one_frame_share(self):
        cell = {'runs_ms': [17.0, 17.5, 31.0, 30.5, 17.2, 31.2, 16.9, 30.8, 17.1, 31.0],
                'calls': 1, 'ok': True, 'errors': []}
        ladder.finish_ops_cell(cell, 10, [], 'screenshot-720')
        self.assertEqual(cell['one_frame'], 5)
        self.assertEqual(cell['mean_ms'], 24.02)
        self.assertEqual(cell['median_ms'], 24.0)
        self.assertEqual(ladder.ops_rank_ms('screenshot-720', cell), 24.02)
        other = {'runs_ms': [5.0, 6.0, 7.0], 'calls': 1, 'ok': True, 'errors': []}
        ladder.finish_ops_cell(other, 3, [], 'click')
        self.assertNotIn('one_frame', other)
        self.assertEqual(ladder.ops_rank_ms('click', other), 6.0)


if __name__ == '__main__':
    unittest.main()

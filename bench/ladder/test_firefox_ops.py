"""Firefox ops plumbing tests, without installed browsers or network."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE / 'contenders'))
import ops_agent
spec = importlib.util.spec_from_file_location('ladder', HERE / 'ladder.py')
ladder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ladder)


def args(tool='nv', engine='firefox'):
    return SimpleNamespace(tool=tool, engine=engine, tag='', session='test',
                           navigera='/bin/navigera', chrome='/bin/chrome', own_browser=False)


class FirefoxOps(unittest.TestCase):
    def test_navigera_launch_selects_engine_and_binary(self):
        tool = ops_agent.Tool(args(), 'http://fixture', {}, Path('/tmp'))
        with patch.dict('os.environ', {'NAVIGERA_FIREFOX': '/bin/firefox'}), patch.object(tool, 'cli', return_value=(1, '')) as cli:
            tool.start()
        self.assertEqual(cli.call_args.args[0], ['/bin/navigera', '--session', 'test', 'start',
                                               '--engine', 'firefox', '--executable', '/bin/firefox'])

    def test_vibium_uses_exact_css_not_description_or_model_lookup(self):
        source = (HERE / 'contenders' / 'firefox_library.mjs').read_text()
        self.assertIn('page.find({ css: selector }).fill(value)', source)
        self.assertIn('page.find({ css: selector }).click()', source)
        self.assertNotIn('page.find(selector)', source)

    def test_unsupported_is_never_zero_or_failure(self):
        for code in ('ppff', 'wdioff'):
            tool = ops_agent.Tool(args(code), 'http://fixture', {}, Path('/tmp'))
            result = tool.run_op('snapshot', 10)
            self.assertIsNone(result['median_ms'])
            self.assertEqual(result['runs_ms'], [])
            self.assertIn('snapshot API', result['not_run'])
            self.assertNotIn('errors', result)

    def test_library_fill_uses_same_reset_and_verification(self):
        tool = ops_agent.Tool(args('pwff-node'), 'http://fixture', {}, Path('/tmp'))
        with patch.object(tool, 'setup') as reset, patch.object(tool, 'library_call', return_value=(2, None)) as call, patch.object(tool, 'eval_value', return_value='widget'):
            self.assertEqual(tool.op_fill(0), (2, 1, ''))
            reset.assert_called_once()
            call.assert_called_once_with('fill', selector='#q', value='widget')

    def test_vibium_names_are_not_synthesized_from_dom(self):
        tool = ops_agent.Tool(args('vibiumff'), 'http://fixture', {}, Path('/tmp'))
        tree = {'role': 'document', 'children': [{'role': 'heading', 'level': 1},
                                               {'role': 'button'}, {'role': 'generic', 'name': 'x' * 100}]}
        with patch.object(tool, 'library_call', return_value=(2, json.dumps(tree))):
            with self.assertRaisesRegex(RuntimeError, 'heading'):
                tool.op_snapshot(0)
        tree['children'][0]['name'] = 'Search results'
        tree['children'][1]['name'] = 'Search'
        with patch.object(tool, 'library_call', return_value=(2, json.dumps(tree))):
            self.assertEqual(tool.op_snapshot(0)[0], 2)

    def test_publish_preserves_unsupported_reason_and_browser_build(self):
        cell = {'median_ms': None, 'runs_ms': [], 'ok': False,
                'not_run': 'no snapshot API', 'calls': 0}
        ops = {'test': {'ok': True, 'cells': {'snapshot': cell},
                        'browser_info': [{'browser': 'Firefox test'}]}}
        with tempfile.TemporaryDirectory() as d:
            ladder.write_publish_json(Path(d), {}, {}, 1, 'test', ops_results=ops)
            doc = json.loads((Path(d) / 'publish.json').read_text())
        self.assertEqual(doc['ops'][0]['cells']['snapshot']['not_run'], 'no snapshot API')
        self.assertIsNone(doc['ops'][0]['cells']['snapshot']['median_ms'])
        self.assertEqual(doc['ops'][0]['browser_info'][0]['browser'], 'Firefox test')

    def test_round_merge_preserves_not_run_and_browser_build(self):
        def fake_run(cmd, env, timeout, tag):
            dest = Path(cmd[cmd.index('--out') + 1])
            slug = cmd[cmd.index('--tag') + 1]
            doc = {'ops': {op: {'runs_ms': [3] * ladder.OPS_REPS_PER_ROUND,
                                'ok': True, 'calls': 1} for op in ladder.OPS},
                   'browser_info': {'browser': 'Firefox test', 'browser_build': 'test build'}}
            doc['ops']['snapshot'] = {'runs_ms': [], 'ok': False, 'calls': 0,
                                      'not_run': 'no snapshot API'}
            (dest / f'ops-{slug}.json').write_text(json.dumps(doc))
            return 0, 0, 0, b'{}'
        with tempfile.TemporaryDirectory() as d, patch.dict('os.environ', {'NAVIGERA_FIREFOX': '/bin/firefox'}), patch('shutil.which', return_value=None), patch.object(ladder, 'run_once', side_effect=fake_run), patch.object(ladder, 'kill_stray_chrome'), patch.object(ladder, 'kill_agent_daemons'):
            results, _ = ladder.bench_ops({'nv-serve': {}}, {}, Path(d), 'test', {})
        cell = results['navigera Firefox']['cells']['snapshot']
        self.assertIsNone(cell['median_ms'])
        self.assertEqual(cell['not_run'], 'no snapshot API')
        self.assertNotIn('errors', cell)
        self.assertFalse(cell['complete'])
        self.assertEqual(results['navigera Firefox']['browser_info'][0]['browser'], 'Firefox test')
        self.assertEqual(results['navigera Firefox']['cells']['fill']['reps'], 10)


def png(path, w, h):
    """A minimal valid RGB PNG of w x h."""
    import struct
    import zlib

    def chunk(t, data):
        return struct.pack('>I', len(data)) + t + data + struct.pack('>I', zlib.crc32(t + data))
    raw = b''.join(b'\0' + b'\0' * 3 * w for _ in range(h))
    path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
                     + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b''))


class Screenshot720(unittest.TestCase):
    def tool(self, d, viewport):
        tool = ops_agent.Tool(args('ab', 'chrome'), 'http://fixture', {}, Path(d))
        calls = []
        state = {'vp': viewport}

        def cli(argv, env, op, **kw):
            calls.append((op, argv[3:]))
            if argv[3:5] == ['set', 'viewport']:
                state['vp'] = (int(argv[5]), int(argv[6]))
            elif argv[3] == 'screenshot':
                png(Path(argv[4]), *state['vp'])
            return 1.5, ''
        tool.cli = cli
        tool.eval_value = lambda expr: f"{state['vp'][0]}x{state['vp'][1]}@1"
        return tool, calls, state

    def test_resizes_captures_at_720_and_undoes(self):
        with tempfile.TemporaryDirectory() as d, patch.object(ops_agent, 'SETUP_REST_S', 0):
            tool, calls, state = self.tool(d, (1440, 813))
            ms, n, detail = tool.op_screenshot_720(0)
        self.assertEqual((ms, n), (1.5, 1))
        self.assertIn('1280x720', detail)
        self.assertEqual(tool.shot_info, {'png': '1280x720', 'viewport': '1280x720', 'dpr': 1.0})
        self.assertEqual([c[0] for c in calls], ['setup', 'screenshot', 'setup'])
        self.assertEqual(calls[0][1], ['set', 'viewport', '1280', '720'])
        self.assertEqual(calls[2][1], ['set', 'viewport', '1440', '813'])
        self.assertEqual(state['vp'], (1440, 813))

    def test_a_resize_that_misses_720_fails_the_rep_and_is_undone(self):
        with tempfile.TemporaryDirectory() as d, patch.object(ops_agent, 'SETUP_REST_S', 0):
            tool, calls, state = self.tool(d, (800, 600))
            real = tool.cli

            def short(argv, env, op, **kw):
                ms, out = real(argv, env, op, **kw)
                if argv[3:5] == ['set', 'viewport'] and argv[5] == '1280':
                    state['vp'] = (1280, 577)
                return ms, out
            tool.cli = short
            with self.assertRaisesRegex(RuntimeError, '1280x577 at the capture, expected 1280x720'):
                tool.op_screenshot_720(0)
        self.assertEqual(state['vp'], (800, 600))

    def test_cell_and_publish_carry_png_size_and_dpr(self):
        with tempfile.TemporaryDirectory() as d, patch.object(ops_agent, 'SETUP_REST_S', 0):
            tool, _, _ = self.tool(d, (1280, 577))
            cell = tool.run_op('screenshot', 2)
            self.assertEqual((cell['png'], cell['dpr']), (['1280x577'], [1.0]))
            ops = {'agent-browser': {'ok': True, 'cells': {'screenshot': {**cell, 'complete': True}}}}
            ladder.write_publish_json(Path(d), {}, {}, 1, 'test', ops_results=ops)
            doc = json.loads((Path(d) / 'publish.json').read_text())
        self.assertEqual(doc['ops'][0]['cells']['screenshot']['png'], ['1280x577'])
        self.assertEqual(doc['ops'][0]['cells']['screenshot']['dpr'], [1.0])
        self.assertIn('screenshot-720', ladder.OPS)
        self.assertEqual(ladder.OPS, ops_agent.OPS)


if __name__ == '__main__':
    unittest.main()

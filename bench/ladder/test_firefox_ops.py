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


if __name__ == '__main__':
    unittest.main()

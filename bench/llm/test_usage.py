"""Offline usage fixtures only. Never sends an API request or starts a browser."""
import io
import json
import tempfile
import unittest
import urllib.error
from pathlib import Path
from unittest.mock import patch
import llm_bench as b


class Response:
    status = 200
    def __init__(self, obj): self.obj = obj
    def __enter__(self): return self
    def __exit__(self, *args): pass
    def read(self): return json.dumps(self.obj).encode()


class UsageTest(unittest.TestCase):
    def fixture(self, **changes):
        u = {"prompt_tokens": 1000, "completion_tokens": 20,
             "prompt_tokens_details": {"cached_tokens": 800}}
        u.update(changes)
        return u

    def test_cached_included_and_components(self):
        raw = self.fixture(); r = b.usage_record(raw, b.MODEL_DEFAULT)
        self.assertEqual(r['raw_usage'], raw)
        self.assertEqual(r['tokens']['input_total'], 1000)
        self.assertEqual(r['tokens']['input_uncached'], 200)
        self.assertEqual(r['tokens']['output_total'], 20)
        self.assertIsNone(r['tokens']['output_cached'])
        self.assertEqual(r['token_status']['output_cached'], 'unsupported')
        self.assertAlmostEqual(r['estimated_cost_usd'], .0000444)
        self.assertEqual(r['provider_charge_status'], 'not_reported')
        self.assertIsNone(r['provider_charge_usd'])
        self.assertEqual(r['rates']['read_at'], '2026-11-05')
        self.assertEqual(r['rates']['source'], 'https://www.siliconflow.com/pricing')

    def test_missing_is_not_zero(self):
        for raw in [None, {}, {'prompt_tokens':1000, 'completion_tokens':20}]:
            r = b.usage_record(raw, b.MODEL_DEFAULT)
            self.assertIsNone(r['estimated_cost_usd'])
            self.assertIsNone(r['tokens']['input_cached'])
            self.assertEqual(r['token_status']['input_cached'], 'missing')

    def test_explicit_zero_usage_and_cache(self):
        r = b.usage_record(self.fixture(prompt_tokens=0, completion_tokens=0,
                            prompt_tokens_details={'cached_tokens':0}), b.MODEL_DEFAULT)
        self.assertEqual(r['estimated_cost_usd'], 0)
        self.assertEqual(r['tokens']['input_cached'], 0)
        self.assertEqual(r['token_status']['input_cached'], 'reported')

    def test_invalid_usage_never_negative_or_falsely_free(self):
        for raw in [self.fixture(prompt_tokens=10),self.fixture(prompt_tokens=True),
                    self.fixture(completion_tokens=-1)]:
            self.assertIsNone(b.usage_record(raw,b.MODEL_DEFAULT)['estimated_cost_usd'])

    def test_reported_output_cache_has_no_invented_rate(self):
        r = b.usage_record(self.fixture(completion_tokens_details={'cached_tokens':5}),b.MODEL_DEFAULT)
        self.assertEqual(r['tokens']['output_cached'],5)
        self.assertEqual(r['tokens']['output_uncached'],15)
        self.assertIsNone(r['estimated_cost_usd'])
        self.assertIsNone(r['rates']['output_cached'])

    def test_other_model_does_not_inherit_rate(self):
        r = b.usage_record(self.fixture(), 'other-model')
        self.assertIsNone(r['estimated_cost_usd'])
        self.assertIsNone(r['rates']['source'])

    def call(self, response=None, error=None):
        with tempfile.TemporaryDirectory() as d:
            j={'calls':[], 'path':str(Path(d)/'usage.jsonl'),
               'identity':{'arm':'fixture','task':'form','rep':1}}
            budget=b.Budget(1)
            with patch.object(b.urllib.request,'urlopen',side_effect=error,
                              return_value=Response(response)) as fake:
                if error:
                    with self.assertRaises(RuntimeError):
                        b.chat('FAKE',b.MODEL_DEFAULT,[],budget,j)
                else:
                    b.chat('FAKE',b.MODEL_DEFAULT,[],budget,j)
                self.assertEqual(fake.call_count,1)  # no invisible retries
            rows=[json.loads(x) for x in Path(j['path']).read_text().splitlines()]
            self.assertEqual(rows,j['calls']);self.assertEqual(rows[0]['retry_count'],0)
            self.assertNotIn('FAKE',Path(j['path']).read_text())
            return j['calls'],budget

    def test_successful_fake_provider_journal(self):
        calls,budget=self.call({'id':'fixture','model':b.MODEL_DEFAULT,
                  'usage':self.fixture(),'choices':[{'message':{'content':'DONE'}}]})
        self.assertEqual(calls[0]['http_status'],200)
        self.assertEqual(calls[0]['raw_usage'],self.fixture())
        self.assertEqual(calls[0]['provider_request_id'],'fixture')
        self.assertEqual(b.session_usage(calls)['unknown_cost_calls'],0)
        self.assertAlmostEqual(budget.spent,.0000444)

    def test_http_error_and_timeout_unknown_not_free(self):
        for error in [TimeoutError(),urllib.error.HTTPError('http://fixture',429,'busy',{},None)]:
            calls,budget=self.call(error=error)
            self.assertEqual(calls[0]['status'],'cost_unknown')
            self.assertIsNone(calls[0]['provider_charge_usd'])
            u=b.session_usage(calls)
            self.assertIsNone(u['estimated_cost_usd'])
            self.assertEqual(u['unknown_cost_calls'],1)
            self.assertGreater(u['unknown_exposure_bound_usd'],0)
            self.assertFalse(calls[0]['bound_verified'])
            self.assertGreater(budget.spent,0)  # admission consumption, not actual cost

    def test_success_missing_usage_unknown_and_retained(self):
        calls,budget=self.call({'choices':[{'message':{'content':'DONE'}}]})
        self.assertEqual(calls[0]['status'],'usage_unknown')
        self.assertIsNone(calls[0]['raw_usage'])
        self.assertIsNone(b.session_usage(calls)['estimated_cost_usd'])
        self.assertGreater(budget.spent,0)

    def test_malformed_answer_still_persists_usage(self):
        with tempfile.TemporaryDirectory() as d:
            j={'calls':[],'path':str(Path(d)/'u.jsonl'),'identity':{}}
            with patch.object(b.urllib.request,'urlopen',return_value=Response({'usage':self.fixture(),'choices':[]})):
                with self.assertRaises(RuntimeError):b.chat('FAKE',b.MODEL_DEFAULT,[],b.Budget(1),j)
            self.assertEqual(j['calls'][0]['raw_usage'],self.fixture())

    def test_decimal_sum_does_not_accumulate_binary_roundoff(self):
        self.assertEqual(b.complete_sum([.1,.2]), .3)
        self.assertEqual(b.decimal_sum([.1,.2]), .3)

    def test_no_per_session_rounding_and_complete_coverage(self):
        a=b.usage_record(self.fixture(),b.MODEL_DEFAULT)
        a.update(status='ok',unknown_exposure_bound_usd=0)
        u=b.session_usage([a,a,a]);self.assertAlmostEqual(u['estimated_cost_usd'],.0001332)
        rec={'arm':'fixture','task':'form','rep':1,'success':True,'turns':3,
             'usage':u,'wall_ms':10,'stop':'answered'}
        line=b.result_line(rec)
        self.assertIn('input=3000',line);self.assertIn('cached_input=2400',line)
        self.assertIn('cached_output=unsupported',line)
        self.assertNotIn('actual_cost',line)
        self.assertFalse(b.run_usage([rec],2)['coverage_complete'])
        failed=dict(rec,success=False)
        self.assertIn('0.0002664',b.table([rec,failed],['fixture'],['form']))

class SessionTest(unittest.TestCase):
    def test_full_session_and_summary_without_network_or_browser(self):
        class Site:
            def reset(self, seed): pass
        response = {"usage": {"prompt_tokens":1000,"completion_tokens":20,
                    "prompt_tokens_details":{"cached_tokens":800}},
                    "choices":[{"message":{"content":"DONE: fixture"}}]}
        with tempfile.TemporaryDirectory() as d:
            with patch.object(b,'setup_arm',return_value=None), \
                 patch.object(b,'cleanup'), \
                 patch.object(b,'task_def',return_value=('fixture',lambda answer:True)), \
                 patch.object(b.urllib.request,'urlopen',return_value=Response(response)):
                rec=b.run_session('FAKE',b.MODEL_DEFAULT,'nav-default','form',1,{},Site(),b.Budget(1),d)
            self.assertTrue(rec['success'])
            self.assertEqual(rec['prompt_tokens'],1000)
            self.assertEqual(rec['usage']['attempted_calls'],1)
            self.assertEqual(len(list(Path(d).glob('*.usage.jsonl'))),1)
            self.assertEqual(json.loads((Path(d)/'nav-default-form-1.json').read_text())['usage'],rec['usage'])
            summary=b.run_usage([rec],1)
            self.assertTrue(summary['coverage_complete'])
            self.assertAlmostEqual(summary['estimated_cost_usd'],.0000444)
            self.assertIsNone(summary['provider_charge_usd'])


class ReserveTest(unittest.TestCase):
    def test_reserve_covers_serialized_body_bytes(self):
        msgs = [{"role": "user", "content": "x" * 5000}]
        body = json.dumps({"model": "m", "messages": msgs, "max_tokens": 16})
        amount = b.reserve_for(body, msgs)
        # Conditional ceiling: reserved input-priced tokens >= body bytes.
        input_tokens_covered = (amount * 1e6 - b.MAX_TOKENS * b.PRICE_OUT) / b.PRICE_IN
        self.assertGreaterEqual(input_tokens_covered, len(body.encode()))

    def test_reserve_floor_and_scales_with_body(self):
        small = json.dumps({"model": "m", "messages": [], "max_tokens": 16})
        big = json.dumps({"model": "m", "messages": [{"role": "user", "content": "y" * 100000}], "max_tokens": 16})
        self.assertGreaterEqual(b.reserve_for(small, []), b.MIN_RESERVE_USD)
        self.assertGreater(b.reserve_for(big, [{}]), b.reserve_for(small, []))

if __name__=='__main__': unittest.main(verbosity=2)

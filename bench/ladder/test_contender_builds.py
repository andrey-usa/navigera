"""Compiled-contender build records: a failed build is reported, never
measured from an older binary. No compiler, browser or network needed."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import build_contender
spec = importlib.util.spec_from_file_location('ladder', HERE / 'ladder.py')
ladder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ladder)

CARGO_LINES = "\n".join([
    json.dumps({"reason": "compiler-artifact",
                "package_id": "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.228"}),
    json.dumps({"reason": "compiler-artifact",
                "package_id": "registry+https://github.com/rust-lang/crates.io-index#chromey@3.0.0"}),
    json.dumps({"reason": "build-finished", "success": True}),
])


class Builds(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.builds = self.tmp / 'builds'
        self.binary = self.tmp / 'ladder-chromey'
        ladder.BUILD_FAILED.clear()

    def spec(self, script):
        return {"dir": self.tmp, "binary": self.binary,
                "commands": [[sys.executable, "-c", script]], "version": ("cargo", "chromey")}

    def test_a_failed_build_deletes_the_old_binary_and_records_its_exit(self):
        # The binary an earlier run left (what the Cargo cache restores).
        self.binary.write_bytes(b'old build')
        record = build_contender.build('chromey', self.spec('import sys; sys.exit(101)'), self.builds)
        self.assertFalse(self.binary.exists())
        self.assertEqual(record['exit'], 101)
        self.assertIsNone(record['version'])
        self.assertEqual(json.loads((self.builds / 'chromey.json').read_text())['exit'], 101)

    def test_a_passing_build_records_the_version_it_compiled_and_its_hash(self):
        script = (f"import pathlib; pathlib.Path({str(self.binary)!r}).write_bytes(b'new'); "
                  f"print({CARGO_LINES!r})")
        record = build_contender.build('chromey', self.spec(script), self.builds)
        self.assertEqual(record['exit'], 0)
        self.assertEqual(record['version'], '3.0.0')
        self.assertEqual(ladder.build_failure(record, str(self.binary)), None)

    def test_a_build_that_writes_no_binary_is_a_failure(self):
        record = build_contender.build('chromey', self.spec('pass'), self.builds)
        self.assertIsNone(record['exit'])
        self.assertIn('no binary', ladder.build_failure(record, str(self.binary)))

    def test_older_cargo_package_id_form(self):
        line = json.dumps({"reason": "compiler-artifact",
                           "package_id": "chromiumoxide 0.9.1 (registry+https://github.com/rust-lang/crates.io-index)"})
        self.assertEqual(build_contender.cargo_version(line, 'chromiumoxide'), '0.9.1')

    def test_failed_build_cells_say_so_and_no_version_is_reported(self):
        self.binary.write_bytes(b'cached binary from an older run')
        gorod = self.tmp / 'ladder-gorod'
        gorod.write_bytes(b'gorod')
        records = {
            'chromey': {'name': 'chromey', 'exit': 101, 'version': None},
            'gorod': {'name': 'gorod', 'exit': 0, 'version': 'v0.116.2',
                      'sha256': __import__('hashlib').sha256(b'gorod').hexdigest()},
            'chromedp': {'name': 'chromedp', 'exit': 0, 'version': 'v0.24.0', 'sha256': 'f' * 64},
        }
        contenders = {'puppeteer': ['node', 'x.mjs'], 'chromey': [str(self.binary)],
                      'gorod': [str(gorod)], 'gorod-shell': ['env', 'CHROME_BIN=/s', str(gorod)],
                      'chromedp': [str(self.tmp / 'ladder-chromedp')],
                      'chromiumoxide': [str(self.tmp / 'ladder-chromiumoxide')]}
        ladder.drop_failed_builds(contenders, records)
        self.assertEqual(sorted(contenders), ['gorod', 'gorod-shell', 'puppeteer'])
        self.assertEqual(ladder.BUILD_FAILED['chromey'], 'build failed (exit 101)')
        # A binary that is not the recorded build (here: none at all).
        self.assertIn('chromedp', ladder.BUILD_FAILED)
        self.assertEqual(ladder.BUILD_FAILED['chromiumoxide'],
                         'build failed (no build record from this run)')
        rows = ladder.build_failed_rows(['chromey'], 7)
        self.assertEqual(rows, ['| `chromey` | build failed (exit 101) |' + ' — |' * 6])
        self.assertEqual(rows[0].count('|'), 9)  # 8 columns, as the session table
        versions = ladder.build_versions(records)
        self.assertNotIn('chromey', versions)
        self.assertNotIn('chromedp', versions)
        self.assertEqual(versions['go-rod'], 'v0.116.2')

    def test_a_replaced_binary_is_not_the_build(self):
        self.binary.write_bytes(b'built')
        record = {'exit': 0, 'sha256': __import__('hashlib').sha256(b'built').hexdigest()}
        self.assertIsNone(ladder.build_failure(record, str(self.binary)))
        self.binary.write_bytes(b'swapped')
        self.assertIn('not the one this run built', ladder.build_failure(record, str(self.binary)))


class Lockfiles(unittest.TestCase):
    """Every compiled contender builds from its committed lockfile; a missing
    or stale one fails the build with the command that refreshes it."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.builds = self.tmp / 'builds'
        self.binary = self.tmp / 'ladder-chromey'

    def spec(self, script, lockfiles=('Cargo.lock',)):
        return {"dir": self.tmp, "binary": self.binary, "commands": [[sys.executable, "-c", script]],
                "version": ("cargo", "chromey"), "lockfiles": list(lockfiles),
                "refresh": "cargo update --workspace --manifest-path x/Cargo.toml, then commit x/Cargo.lock"}

    def test_every_contender_has_its_lockfiles_committed(self):
        for name, spec in build_contender.SPECS.items():
            for f in spec["lockfiles"]:
                self.assertTrue((spec["dir"] / f).is_file(), f"{name}: {f} missing")

    def test_builds_never_resolve_afresh(self):
        for name, spec in build_contender.SPECS.items():
            argv = [" ".join(c) for c in spec["commands"]]
            self.assertFalse(any("mod tidy" in c or "generate-lockfile" in c for c in argv), name)
            kind = spec["version"][0]
            self.assertTrue(all(("--locked" if kind == "cargo" else "-mod=readonly") in c for c in argv), name)

    def test_the_go_refresh_command_names_the_module_dir(self):
        self.assertIn("(cd bench/ladder/contenders/gorod && go mod tidy)", build_contender.SPECS["gorod"]["refresh"])
        self.assertIn("cargo update --workspace --manifest-path bench/ladder/contenders/chromey/Cargo.toml",
                      build_contender.SPECS["chromey"]["refresh"])

    def test_a_missing_lockfile_fails_before_building_and_says_how_to_create_it(self):
        self.binary.write_bytes(b'old build')
        ran = self.tmp / 'ran'
        record = build_contender.build('chromey', self.spec(f"open({str(ran)!r}, 'w')"), self.builds)
        self.assertFalse(ran.exists())
        self.assertFalse(self.binary.exists())
        self.assertIsNone(record['exit'])
        self.assertIn('no committed Cargo.lock', record['error'])
        self.assertIn('cargo update --workspace', record['error'])
        self.assertIn('no committed Cargo.lock', ladder.build_failure(record, str(self.binary)))

    def test_a_stale_lockfile_fails_with_the_refresh_command(self):
        (self.tmp / 'Cargo.lock').write_text('# stale\n')
        script = ("import sys; sys.stderr.write('error: the lock file x/Cargo.lock needs to be updated "
                  "but --locked was passed to prevent this\\n'); sys.exit(101)")
        record = build_contender.build('chromey', self.spec(script), self.builds)
        self.assertEqual(record['exit'], 101)
        self.assertIn('is stale for this build', record['error'])
        self.assertIn('cargo update --workspace --manifest-path x/Cargo.toml', record['error'])
        self.assertIn('cargo update --workspace', json.loads((self.builds / 'chromey.json').read_text())['error'])

    def test_a_stale_go_sum_fails_with_the_refresh_command(self):
        for f in ('go.mod', 'go.sum'):
            (self.tmp / f).write_text('')
        script = ("import sys; sys.stderr.write('main.go:5:2: missing go.sum entry for module providing "
                  "package github.com/go-rod/rod\\n'); sys.exit(1)")
        record = build_contender.build('gorod', self.spec(script, ('go.mod', 'go.sum')), self.builds)
        self.assertEqual(record['exit'], 1)
        self.assertIn('refresh it with', record['error'])

    def test_an_ordinary_compile_error_is_not_called_a_stale_lockfile(self):
        (self.tmp / 'Cargo.lock').write_text('')
        script = "import sys; sys.stderr.write('error[E0425]: cannot find value `x`\\n'); sys.exit(101)"
        record = build_contender.build('chromey', self.spec(script), self.builds)
        self.assertEqual(record['exit'], 101)
        self.assertNotIn('error', record)


if __name__ == '__main__':
    unittest.main()

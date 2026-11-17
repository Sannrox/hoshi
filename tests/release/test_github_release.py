"""Tests for scripts/release/github-release.sh against a fake gh."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class ReleasePublication(unittest.TestCase):
    def test_retry_preserves_published_manifest_and_rejects_replacement(self):
        with tempfile.TemporaryDirectory(prefix=f'hoshi-publish-test-{os.getpid()}-') as temporary:
            directory = Path(temporary)
            (directory / 'dist').mkdir()
            (directory / 'dist/latest.json').write_text('signed manifest')
            published = directory / 'published'
            published.write_text('signed manifest')
            fake = directory / 'gh'
            fake.write_text("""#!/usr/bin/env python3
import os, pathlib, shutil, sys
args = sys.argv[1:]
if args[:2] == ['release', 'view']:
    if '--json' in args: print('false')
    sys.exit(0)
if args[:2] == ['release', 'download']:
    shutil.copyfile(os.environ['PUBLISHED'], pathlib.Path(args[args.index('--dir') + 1]) / 'latest.json')
    sys.exit(0)
raise SystemExit('unexpected mutation: ' + repr(args))
""")
            fake.chmod(0o755)
            env = {**os.environ, 'PATH': str(directory) + os.pathsep + os.environ['PATH'],
                   'PUBLISHED': str(published)}
            command = ['bash', str(ROOT / 'scripts/release/github-release.sh'), 'v0.2.0', 'title', 'notes', 'stable']
            result = subprocess.run(command, cwd=directory, env=env, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            binary = directory / 'dist/hoshi-test-target'
            binary.write_text('binary')
            result = subprocess.run(command, cwd=directory, env=env, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            binary.unlink()
            (directory / 'dist/latest.json').write_text('replacement')
            result = subprocess.run(command, cwd=directory, env=env, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(published.read_text(), 'signed manifest')


if __name__ == '__main__':
    unittest.main()

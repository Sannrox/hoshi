"""Tests for scripts/release/publish_metadata.py against a fake gh."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class MetadataPublication(unittest.TestCase):
    def test_immutable_lookup_requires_confirmed_absence(self):
        with tempfile.TemporaryDirectory(prefix=f'hoshi-metadata-test-{os.getpid()}-') as temporary:
            directory = Path(temporary)
            metadata = directory / 'latest.json'
            metadata.write_text(json.dumps({'payload': json.dumps({'version': '0.2.0', 'channel': 'stable'})}))
            capture = directory / 'published'
            fake = directory / 'gh'
            fake.write_text("""#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
path = args[1]
if path == 'graphql':
    pathlib.Path(os.environ['CAPTURE']).write_text(sys.stdin.read())
    print(json.dumps({'data': {'createCommitOnBranch': {'commit': {'oid': 'published'}}}}))
elif '/git/ref/' in path:
    print(json.dumps({'object': {'sha': 'base'}}))
elif '/contents/' in path:
    status = os.environ['IMMUTABLE_STATUS'] if '/versions/' in path else '404'
    print('HTTP ' + status, file=sys.stderr)
    sys.exit(1)
else:
    raise SystemExit('unexpected request')
""")
            fake.chmod(0o755)
            env = {**os.environ, 'PATH': str(directory) + os.pathsep + os.environ['PATH'],
                   'GITHUB_REPOSITORY': 'example/hoshi', 'CAPTURE': str(capture)}
            command = ['python3', str(ROOT / 'scripts/release/publish_metadata.py'), str(metadata)]
            for status in ['503', '401', '404']:
                result = subprocess.run(command, env={**env, 'IMMUTABLE_STATUS': status}, capture_output=True)
                if status == '404':
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertTrue(capture.exists())
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(capture.exists())


if __name__ == '__main__':
    unittest.main()

#!/usr/bin/env python3
"""Exercise release signing with a deterministic test-only key, without publishing."""
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import pty

ROOT = Path(__file__).resolve().parent.parent


class ReleaseSigning(unittest.TestCase):
    def test_exact_payload_checksums_and_wrong_key_refusal(self):
        with tempfile.TemporaryDirectory(prefix='hoshi-sign-test-') as temporary:
            directory = Path(temporary)
            # Fixed fixture material, never a generated release identity.
            der = bytes.fromhex('302e020100300506032b657004220420') + bytes([9]) * 32
            private = '-----BEGIN PRIVATE KEY-----\n' + base64.b64encode(der).decode() + '\n-----END PRIVATE KEY-----\n'
            keyfile = directory / 'test-key.pem'
            keyfile.write_text(private)
            public_der = subprocess.check_output(['openssl', 'pkey', '-in', str(keyfile), '-pubout', '-outform', 'DER'])
            public = directory / 'public.der'
            public.write_bytes(public_der)
            asset = directory / 'hoshi-aarch64-apple-darwin'
            asset.write_bytes(b'test binary')
            checksums = directory / 'SHA256SUMS'
            checksums.write_text(f'{hashlib.sha256(asset.read_bytes()).hexdigest()}  {asset.name}\n')
            env = {**os.environ, 'GITHUB_ACTIONS': 'true', 'HOSHI_RELEASE_SIGNING_KEY': private,
                   'HOSHI_RELEASE_PUBLIC_KEY': public_der[-32:].hex()}
            command = ['python3', 'scripts/sign-release-metadata.py', '--dist', str(directory),
                       '--version', '0.2.0', '--channel', 'stable', '--version-output', 'hoshi 0.2.0']
            subprocess.run(command, cwd=ROOT, env=env, check=True)
            envelope = json.loads((directory / 'latest.json').read_text())
            release = json.loads(envelope['payload'])
            self.assertEqual(release['assets']['aarch64-apple-darwin']['sha256'], hashlib.sha256(asset.read_bytes()).hexdigest())
            payload = directory / 'payload'
            signature = directory / 'payload.sig'
            payload.write_text(envelope['payload'])
            signature.write_bytes(bytes.fromhex(envelope['signature']))
            verify = ['openssl', 'pkeyutl', '-verify', '-rawin', '-pubin', '-keyform', 'DER', '-inkey', str(public)]
            subprocess.run(verify + ['-in', str(payload), '-sigfile', str(signature)], check=True, capture_output=True)
            subprocess.run(verify + ['-in', str(checksums), '-sigfile', str(directory / 'SHA256SUMS.sig')], check=True, capture_output=True)
            payload.write_text(envelope['payload'].replace('0.2.0', '0.3.0'))
            self.assertNotEqual(subprocess.run(verify + ['-in', str(payload), '-sigfile', str(signature)], capture_output=True).returncode, 0)
            env['HOSHI_RELEASE_PUBLIC_KEY'] = '00' * 32
            self.assertNotEqual(subprocess.run(command, cwd=ROOT, env=env, capture_output=True).returncode, 0)


class ManagedInstallHandoff(unittest.TestCase):
    def test_pipe_install_forwards_selection_and_uses_the_terminal(self):
        for selection, expected in [({'HOSHI_VERSION': 'v0.2.0'}, ['--channel', 'stable', '--version', 'v0.2.0']),
                                    ({'HOSHI_CHANNEL': 'nightly'}, ['--channel', 'nightly'])]:
            with self.subTest(selection=selection), tempfile.TemporaryDirectory(prefix='hoshi-install-test-') as temporary:
                directory = Path(temporary)
                fake = directory / 'hoshi'
                fake.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$CAPTURE"\n')
                fake.chmod(0o755)
                capture = directory / 'arguments'
                env = {key: value for key, value in os.environ.items() if key not in {'HOSHI_VERSION', 'HOSHI_CHANNEL'}}
                env.update(selection)
                env.update(PATH=str(directory) + os.pathsep + env['PATH'], HOSHI_MANAGED_DIR=str(directory / 'managed'), CAPTURE=str(capture))
                pid, terminal = pty.fork()
                if pid == 0:
                    os.execve('/bin/sh', ['/bin/sh', str(ROOT / 'scripts/install.sh')], env)
                try:
                    while True:
                        try:
                            if not os.read(terminal, 4096):
                                break
                        except OSError:
                            break
                finally:
                    os.close(terminal)
                _, status = os.waitpid(pid, 0)
                self.assertEqual(os.waitstatus_to_exitcode(status), 0)
                self.assertEqual(capture.read_text().splitlines(), ['update', '--install-dir', str(directory / 'managed'), *expected])


class MetadataPublication(unittest.TestCase):
    def test_immutable_lookup_requires_confirmed_absence(self):
        with tempfile.TemporaryDirectory(prefix='hoshi-metadata-test-') as temporary:
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
            command = ['python3', str(ROOT / 'scripts/publish-release-metadata.py'), str(metadata)]
            for status in ['503', '401', '404']:
                result = subprocess.run(command, env={**env, 'IMMUTABLE_STATUS': status}, capture_output=True)
                if status == '404':
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertTrue(capture.exists())
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(capture.exists())


class ReleasePublication(unittest.TestCase):
    def test_retry_preserves_published_manifest_and_rejects_replacement(self):
        with tempfile.TemporaryDirectory(prefix='hoshi-publish-test-') as temporary:
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
            command = ['bash', str(ROOT / 'scripts/publish-release.sh'), 'v0.2.0', 'title', 'notes', 'stable']
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

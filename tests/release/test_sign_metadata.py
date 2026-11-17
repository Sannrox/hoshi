"""Tests for scripts/release/sign_metadata.py with a deterministic test-only key."""
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class ReleaseSigning(unittest.TestCase):
    def test_exact_payload_checksums_and_wrong_key_refusal(self):
        with tempfile.TemporaryDirectory(prefix=f'hoshi-sign-test-{os.getpid()}-') as temporary:
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
            command = ['python3', 'scripts/release/sign_metadata.py', '--dist', str(directory),
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


if __name__ == '__main__':
    unittest.main()

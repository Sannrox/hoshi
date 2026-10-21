#!/usr/bin/env python3
"""Sign exact metadata and checksum bytes. The private PEM is supplied only by CI."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument('--dist', type=Path, required=True)
parser.add_argument('--version', required=True)
parser.add_argument('--channel', choices=['stable', 'nightly'], required=True)
parser.add_argument('--version-output', required=True)
parser.add_argument('--note', default='')
args = parser.parse_args()

if os.environ.get('GITHUB_ACTIONS') != 'true':
    raise SystemExit('release signing runs only in CI')
key = os.environ.pop('HOSHI_RELEASE_SIGNING_KEY', '')
public = os.environ.get('HOSHI_RELEASE_PUBLIC_KEY', '')
if not key or not re.fullmatch(r'[0-9a-fA-F]{64}', public):
    raise SystemExit('release signing key and public key must be provisioned')


def constant(path, name):
    match = re.search(r'pub const ' + name + r': [^=]+ = (\d+);', Path(path).read_text())
    if not match:
        raise SystemExit(f'missing {name}')
    return int(match[1])


with tempfile.TemporaryDirectory() as temporary:
    private = Path(temporary) / 'key.pem'
    private.write_text(key)
    private.chmod(0o600)
    del key
    actual = subprocess.check_output(['openssl', 'pkey', '-in', str(private), '-pubout', '-outform', 'DER'])[-32:].hex()
    if actual != public.lower():
        raise SystemExit('release signing key does not match compiled public key')

    def sign(data):
        source = Path(temporary) / 'payload'
        source.write_bytes(data)
        return subprocess.check_output(['openssl', 'pkeyutl', '-sign', '-rawin', '-inkey', str(private), '-in', str(source)])

    assets = {}
    for path in sorted(args.dist.glob('hoshi-*')):
        target = path.name.removeprefix('hoshi-').removesuffix('.exe')
        assets[target] = {'name': path.name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
    if not assets:
        raise SystemExit('no release binaries')
    metadata = {
        'version': args.version.removeprefix('v').replace('~nightly.', '-nightly.'),
        'channel': args.channel,
        'version_output': args.version_output,
        'published_at': int(subprocess.check_output(['git', 'show', '-s', '--format=%ct', 'HEAD']).strip()),
        'note': args.note or None,
        'save_schema': constant('crates/hoshi-core/src/save.rs', 'SCHEMA_VERSION'),
        'cli_contract': constant('crates/hoshi-tui/src/cli.rs', 'SCHEMA_VERSION'),
        'server_contracts': ['v1'],
        'assets': assets,
    }
    payload = json.dumps(metadata, separators=(',', ':'), ensure_ascii=False)
    envelope = {'payload': payload, 'signature': sign(payload.encode()).hex()}
    (args.dist / 'latest.json').write_text(json.dumps(envelope, separators=(',', ':')) + '\n')
    sums = args.dist / 'SHA256SUMS'
    (args.dist / 'SHA256SUMS.sig').write_bytes(sign(sums.read_bytes()))

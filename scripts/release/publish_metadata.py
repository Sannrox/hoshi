#!/usr/bin/env python3
"""Atomically publish latest and immutable version metadata after release assets exist."""
import base64
import json
import os
import re
from pathlib import Path
import subprocess
import sys

metadata = Path(sys.argv[1]).read_bytes()
release = json.loads(json.loads(metadata)['payload'])
repo = os.environ['GITHUB_REPOSITORY']
branch = 'update-metadata'


def api(*args, data=None):
    return subprocess.run(['gh', 'api', *args], input=data, stdout=subprocess.PIPE, check=True).stdout


# Bootstrap without a force push. A racing publisher must retry after inspecting the ref.
ref = subprocess.run(['gh', 'api', f'repos/{repo}/git/ref/heads/{branch}'], capture_output=True)
if ref.returncode and b'HTTP 404' not in ref.stderr:
    raise SystemExit('cannot read metadata branch; refusing to publish')
if ref.returncode:
    base = json.loads(api(f'repos/{repo}/git/ref/heads/main'))['object']['sha']
    api('-X', 'POST', f'repos/{repo}/git/refs', '-f', f'ref=refs/heads/{branch}', '-f', f'sha={base}')
head = json.loads(api(f'repos/{repo}/git/ref/heads/{branch}'))['object']['sha']
immutable = f'versions/v{release["version"]}/latest.json'
existing = subprocess.run(['gh', 'api', f'repos/{repo}/contents/{immutable}?ref={branch}'], capture_output=True)
if existing.returncode == 0:
    previous = base64.b64decode(json.loads(existing.stdout)['content'])
    if previous != metadata:
        raise SystemExit('version metadata already exists with different bytes; publish a new version')
elif b'HTTP 404' not in existing.stderr:
    raise SystemExit('cannot read immutable version metadata; refusing to publish')

def version_key(version):
    match = re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?', version)
    if not match:
        raise SystemExit('invalid release version')
    core = tuple(int(value) for value in match.groups()[:3])
    prerelease = match.group(4)
    identifiers = tuple((0, int(value)) if value.isdigit() else (1, value) for value in prerelease.split('.')) if prerelease else ()
    return core, int(prerelease is None), identifiers


paths = [immutable]
latest_path = f'{release["channel"]}/latest.json'
latest = subprocess.run(['gh', 'api', f'repos/{repo}/contents/{latest_path}?ref={branch}'], capture_output=True)
if latest.returncode == 0:
    old_envelope = json.loads(base64.b64decode(json.loads(latest.stdout)['content']))
    old_release = json.loads(old_envelope['payload'])
    if version_key(release['version']) > version_key(old_release['version']):
        paths.append(latest_path)
elif b'HTTP 404' in latest.stderr:
    paths.append(latest_path)
else:
    raise SystemExit('cannot read current latest metadata; refusing to overwrite it')

query = 'mutation($input:CreateCommitOnBranchInput!){createCommitOnBranch(input:$input){commit{oid}}}'
request = {'query': query, 'variables': {'input': {
    'branch': {'repositoryNameWithOwner': repo, 'branchName': branch},
    'expectedHeadOid': head,
    'message': {'headline': f'Publish {release["channel"]} metadata for {release["version"]}'},
    'fileChanges': {'additions': [{'path': path, 'contents': base64.b64encode(metadata).decode()} for path in paths]},
}}}
result = json.loads(api('graphql', '--input', '-', data=json.dumps(request).encode()))
if result.get('errors'):
    raise SystemExit('metadata publication failed; inspect branch before retrying')
print(result['data']['createCommitOnBranch']['commit']['oid'])

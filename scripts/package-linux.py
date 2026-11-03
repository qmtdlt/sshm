#!/usr/bin/env python3
"""Build a static Linux x64 binary in Docker on macOS, Windows or Linux."""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser()
parser.add_argument('--oauth-config', type=Path, help='Existing desktop OAuth JSON outside the repository')
args = parser.parse_args()
env = os.environ.copy()
if args.oauth_config:
    config = json.loads(args.oauth_config.read_text())['installed']
    env['SSHM_GOOGLE_CLIENT_ID'] = config['client_id']
    env['SSHM_GOOGLE_CLIENT_SECRET'] = config.get('client_secret', '')
if not env.get('SSHM_GOOGLE_CLIENT_ID', '').endswith('.apps.googleusercontent.com'):
    parser.error('Supply --oauth-config or the existing SSHM_GOOGLE_CLIENT_ID build environment')
env.setdefault('SSHM_GOOGLE_CLIENT_SECRET', '')
fingerprint = hashlib.sha256(json.dumps([env['SSHM_GOOGLE_CLIENT_ID'], env['SSHM_GOOGLE_CLIENT_SECRET']]).encode()).hexdigest()
version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
output = ROOT / 'dist/linux'
output.mkdir(parents=True, exist_ok=True)
subprocess.run([
    'docker', 'buildx', 'build', '--platform', 'linux/amd64',
    '--file', 'scripts/linux/Dockerfile', '--target', 'artifact',
    '--secret', 'id=google_client_id,env=SSHM_GOOGLE_CLIENT_ID',
    '--secret', 'id=google_client_secret,env=SSHM_GOOGLE_CLIENT_SECRET',
    '--build-arg', 'SSHM_CONFIG_FINGERPRINT=' + fingerprint,
    '--output', 'type=local,dest=' + str(output), '.',
], cwd=ROOT, env=env, check=True)
binary = (output / 'sshm').read_bytes()
# ELF64, little-endian, EM_X86_64. Docker additionally rejects dynamic loaders.
if binary[:5] != b'\x7fELF\x02' or binary[5] != 1 or int.from_bytes(binary[18:20], 'little') != 62:
    raise SystemExit('Expected Linux x86_64 ELF binary')
archive = output / 'sshm-linux-x86_64.tar.gz'
with archive.open('wb') as raw:
    with gzip.GzipFile(filename='', mode='wb', fileobj=raw, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode='w') as tar:
            info = tarfile.TarInfo('sshm')
            info.size = len(binary)
            info.mode = 0o755
            info.mtime = 0
            tar.addfile(info, io.BytesIO(binary))
checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
(output / 'linux-SHA256SUMS').write_text(f'version={version}\n{checksum}  {archive.name}\n')
# Do not leave an old signature after any rebuild.
(output / 'linux-SHA256SUMS.sig').unlink(missing_ok=True)
print(f'Built {archive}; sign linux-SHA256SUMS before distribution.')

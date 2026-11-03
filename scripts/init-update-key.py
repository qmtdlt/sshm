#!/usr/bin/env python3
"""Create a persistent release-signing seed outside the repository, without printing it."""
import argparse
import base64
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser()
parser.add_argument('path', type=Path)
args = parser.parse_args()
path = args.path.expanduser().absolute()
if path.resolve().is_relative_to(ROOT):
    parser.error('release private keys must be stored outside the repository')
path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
# Never replace an existing trust anchor.
fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
seed = base64.b64encode(os.urandom(32)).decode('ascii')
with os.fdopen(fd, 'w') as stream:
    stream.write(seed + '\n')
    stream.flush()
    os.fsync(stream.fileno())
env = os.environ.copy()
env['SSHM_UPDATE_SIGNING_KEY'] = seed
public = subprocess.check_output(['cargo', 'run', '--quiet', '--locked', '--example', 'sign_update', '--', '--public-key'], cwd=ROOT, env=env, text=True).strip()
path.with_suffix('.pub').write_text(public + '\n')
print(f'Private seed saved with mode 0600: {path}')
print(f'Public key: {path.with_suffix(".pub")}')

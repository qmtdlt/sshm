#!/usr/bin/env python3
"""Inspect a built package without installing it or reading user credentials."""
import argparse
from pathlib import Path
import platform
import subprocess
import tempfile
import tomllib
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--arch', choices=['aarch64', 'x86_64'], default='aarch64')
    args = parser.parse_args()
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
    package = ROOT / 'dist' / f'sshm-{version}-macos-{args.arch}.pkg'
    with tempfile.TemporaryDirectory(prefix='sshm-inspect-') as temp:
        expanded = Path(temp) / 'expanded'
        subprocess.run(['pkgutil', '--expand-full', str(package), str(expanded)], check=True)
        ET.parse(expanded / 'Distribution')
        component = expanded / 'component.pkg'
        info = ET.parse(component / 'PackageInfo').getroot()
        assert info.attrib['identifier'] == 'com.qmtdlt.sshm'
        assert info.attrib['version'] == version
        payload = component / 'Payload'
        relative = Path('usr/local/libexec/sshm/versions') / version
        files = {str(p.relative_to(payload)) for p in payload.rglob('*') if p.is_file()}
        assert files == {str(relative / 'sshm'), str(relative / 'manage')}, files
        binary = payload / relative / 'sshm'
        machine = 'arm64' if args.arch == 'aarch64' else 'x86_64'
        subprocess.run(['lipo', str(binary), '-verify_arch', machine], check=True)
        for name in ['preinstall', 'postinstall']:
            script = component / 'Scripts' / name
            assert '@VERSION@' not in script.read_text()
            assert '@MACHINE@' not in script.read_text()
            subprocess.run(['/bin/sh', '-n', str(script)], check=True)
        if platform.machine() == machine:
            result = subprocess.check_output([str(binary), '--version'], text=True).strip()
            assert result == 'sshm ' + version, result
            subprocess.run([str(binary), 'update', '--help'], check=True, stdout=subprocess.DEVNULL)
        print(f'PASS: payload allowlist, architecture, metadata, shell syntax ({args.arch}); native CLI: ' + ('passed' if platform.machine() == machine else 'not run on this host'))

if __name__ == '__main__':
    main()

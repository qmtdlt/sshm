#!/usr/bin/env python3
"""Build an ARM64/Intel pkg; signing and notarization are explicit opt-ins."""
import argparse
import base64
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent

def run(*args, **kwargs):
    subprocess.run(args, check=True, cwd=ROOT, **kwargs)

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--arch', choices=['aarch64', 'x86_64'], default='aarch64')
    parser.add_argument('--oauth-config', type=Path)
    parser.add_argument('--update-key', type=Path, help='Local base64 Ed25519 seed file; only public key is embedded')
    parser.add_argument('--sign', action='store_true', help='Requires signing identities, pinned update key and team ID')
    parser.add_argument('--notary-profile', help='Explicitly submit to Apple using this Keychain profile')
    args = parser.parse_args()
    if platform.system() != 'Darwin':
        parser.error('macOS is required')
    if args.notary_profile and not args.sign:
        parser.error('--notary-profile requires --sign')
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
    if not re.fullmatch(r'\d+\.\d+\.\d+', version):
        parser.error('package version must be a stable three-part version')
    env = os.environ.copy()
    if args.oauth_config:
        config = json.loads(args.oauth_config.read_text())['installed']
        env['SSHM_GOOGLE_CLIENT_ID'] = config['client_id']
        env['SSHM_GOOGLE_CLIENT_SECRET'] = config.get('client_secret', '')
    if not env.get('SSHM_GOOGLE_CLIENT_ID', '').endswith('.apps.googleusercontent.com'):
        parser.error('OAuth build configuration is required; supply --oauth-config or build environment')
    if args.update_key:
        signer_env = env.copy()
        signer_env['SSHM_UPDATE_SIGNING_KEY'] = args.update_key.read_text().strip()
        env['SSHM_UPDATE_PUBLIC_KEY'] = subprocess.check_output(
            ['cargo', 'run', '--quiet', '--locked', '--example', 'sign_update', '--', '--public-key'],
            cwd=ROOT, env=signer_env, text=True).strip()
    if env.get('SSHM_UPDATE_PUBLIC_KEY'):
        try:
            if len(base64.b64decode(env['SSHM_UPDATE_PUBLIC_KEY'], validate=True)) != 32:
                raise ValueError('wrong length')
        except ValueError:
            parser.error('SSHM_UPDATE_PUBLIC_KEY must be a base64 32-byte Ed25519 public key')
    if args.sign:
        for name in ['SSHM_CODESIGN_IDENTITY', 'SSHM_INSTALLER_IDENTITY', 'SSHM_UPDATE_PUBLIC_KEY', 'SSHM_APPLE_TEAM_ID']:
            if not env.get(name):
                parser.error(f'{name} is required for signed distribution')
        if not re.fullmatch(r'[A-Z0-9]{10}', env['SSHM_APPLE_TEAM_ID']):
            parser.error('invalid SSHM_APPLE_TEAM_ID')
    target = f'{args.arch}-apple-darwin'
    env['MACOSX_DEPLOYMENT_TARGET'] = '11.0'
    run('cargo', 'build', '--release', '--locked', '--target', target, '--bin', 'sshm', env=env)
    output = ROOT / 'dist'
    output.mkdir(exist_ok=True)
    name = f'sshm-{version}-macos-{args.arch}.pkg'
    package = output / name
    with tempfile.TemporaryDirectory(prefix='sshm-package-') as temp:
        work = Path(temp)
        payload = work / 'root'
        destination = payload / 'usr/local/libexec/sshm/versions' / version
        destination.mkdir(parents=True)
        binary = destination / 'sshm'
        shutil.copy2(ROOT / 'target' / target / 'release/sshm', binary)
        shutil.copy2(ROOT / 'scripts/macos/manage.sh', destination / 'manage')
        for path in [binary, destination / 'manage']:
            path.chmod(0o755)
        machine = 'arm64' if args.arch == 'aarch64' else 'x86_64'
        run('lipo', str(binary), '-verify_arch', machine)
        if args.sign:
            run('codesign', '--force', '--options', 'runtime', '--timestamp', '--identifier', 'com.qmtdlt.sshm', '--sign', env['SSHM_CODESIGN_IDENTITY'], str(binary))
            run('codesign', '--verify', '--strict', str(binary))
            identity = subprocess.run(['codesign', '-dv', str(binary)], capture_output=True, text=True, check=True)
            if f"TeamIdentifier={env['SSHM_APPLE_TEAM_ID']}" not in identity.stderr.splitlines():
                parser.error('code signing identity does not match SSHM_APPLE_TEAM_ID')
        scripts = work / 'scripts'
        scripts.mkdir()
        for name in ['preinstall', 'postinstall']:
            text = (ROOT / 'scripts/macos' / name).read_text().replace('@VERSION@', version).replace('@MACHINE@', machine)
            (scripts / name).write_text(text)
            (scripts / name).chmod(0o755)
            run('/bin/sh', '-n', str(scripts / name))
        component = work / 'component.pkg'
        run('pkgbuild', '--root', str(payload), '--scripts', str(scripts), '--identifier', 'com.qmtdlt.sshm', '--version', version, '--install-location', '/', '--ownership', 'recommended', str(component))
        distribution = work / 'distribution.xml'
        distribution.write_text(f'''<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="2">
<title>sshm {version}</title>
<options customize="never" require-scripts="true" hostArchitectures="{machine}"/>
<domains enable_localSystem="true" enable_currentUserHome="false" enable_anywhere="false"/>
<allowed-os-versions><os-version min="11.0"/></allowed-os-versions>
<choices-outline><line choice="default"/></choices-outline>
<choice id="default" visible="false"><pkg-ref id="com.qmtdlt.sshm"/></choice>
<pkg-ref id="com.qmtdlt.sshm" version="{version}" onConclusion="none">component.pkg</pkg-ref>
</installer-gui-script>''')
        command = ['productbuild', '--distribution', str(distribution), '--package-path', str(work)]
        if args.sign:
            command += ['--sign', env['SSHM_INSTALLER_IDENTITY'], '--timestamp']
        run(*command, str(package))
        if args.sign:
            signature = subprocess.check_output(['pkgutil', '--check-signature', str(package)], text=True)
            expected = re.compile(r'Developer ID Installer: .*\(' + re.escape(env['SSHM_APPLE_TEAM_ID']) + r'\)$', re.MULTILINE)
            if not expected.search(signature):
                parser.error('installer signing identity does not match SSHM_APPLE_TEAM_ID')
        if args.notary_profile:
            run('xcrun', 'notarytool', 'submit', str(package), '--keychain-profile', args.notary_profile, '--wait')
            run('xcrun', 'stapler', 'staple', str(package))
            run('xcrun', 'stapler', 'validate', str(package))
            run('spctl', '--assess', '--type', 'install', str(package))
    print(f'Package: {package}')
    print('Signed distribution' if args.sign else 'LOCAL TRIAL ONLY: no Developer ID signature or notarization')

if __name__ == '__main__':
    main()

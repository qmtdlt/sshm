#!/usr/bin/env python3
"""Linux-only installer tests. Real OpenSSL signatures, temporary paths, mocked downloads."""
import hashlib
import io
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent

class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='sshm-linux-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.fixture = self.root / 'release'
        self.fixture.mkdir()
        self.bin = self.root / 'mock'
        self.bin.mkdir()
        self.destination = self.root / 'space 中文' / 'bin'
        self.key = self.root / 'key'
        subprocess.run(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', str(self.key)], check=True, capture_output=True)
        public = subprocess.check_output(['openssl', 'pkey', '-in', str(self.key), '-pubout'], text=True)
        installer = (ROOT / 'install.sh').read_text()
        installer = re.sub(r'-----BEGIN PUBLIC KEY-----.*?-----END PUBLIC KEY-----', public.strip(), installer, flags=re.S)
        # Allow fault injection only in this temporary copy; production uses a fixed root PATH.
        installer = installer.replace('PATH=/usr/bin:/bin:/usr/sbin:/sbin', f'PATH={self.bin}:/usr/bin:/bin:/usr/sbin:/sbin')
        self.installer = self.root / 'install.sh'
        self.installer.write_text(installer)
        self.executable(self.bin / 'uname', '#!/bin/sh\nif [ "$1" = -s ]; then echo Linux; else echo "${TEST_ARCH:-x86_64}"; fi\n')
        self.executable(self.bin / 'ssh', '#!/bin/sh\nexit 0\n')
        self.executable(self.bin / 'curl', '''#!/usr/bin/env python3
import os,sys,shutil
from pathlib import Path
args=sys.argv[1:]
assert args[args.index('--proto')+1]=='=https'
assert args[args.index('--proto-redir')+1]=='=https'
name=args[-1].rsplit('/',1)[1]
if name == os.environ.get('FAIL_DOWNLOAD'): sys.exit(22)
shutil.copyfile(Path(os.environ['FIXTURE'])/name, args[args.index('--output')+1])
''')
        self.env = os.environ.copy()
        self.env.update(PATH=str(self.bin)+':'+os.environ['PATH'], FIXTURE=str(self.fixture), SSHM_INSTALL_DIR=str(self.destination))
        self.env.pop('SSHM_VERSION', None)
        self.make_release()

    def executable(self, path, text):
        path.write_text(text)
        path.chmod(0o755)

    def make_release(self, version='0.4.0', payload=None, extra=False):
        if payload is None: payload = f'#!/bin/sh\necho "sshm {version}"\n'.encode()
        archive = self.fixture / 'sshm-linux-x86_64.tar.gz'
        with tarfile.open(archive, 'w:gz') as tar:
            info = tarfile.TarInfo('sshm')
            info.mode = 0o755
            info.size = len(payload)
            tar.addfile(info, io.BytesIO(payload))
            if extra:
                info = tarfile.TarInfo('../../unexpected')
                info.size = 1
                tar.addfile(info, io.BytesIO(b'x'))
        checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
        (self.fixture / 'linux-SHA256SUMS').write_text(f'version={version}\n{checksum}  {archive.name}\n')
        subprocess.run(['openssl','pkeyutl','-sign','-rawin','-inkey',str(self.key),'-in',str(self.fixture/'linux-SHA256SUMS'),'-out',str(self.fixture/'linux-SHA256SUMS.sig')],check=True,capture_output=True)

    def run_installer(self, **env):
        return subprocess.run(['/bin/sh', str(self.installer)], env=self.env|env, capture_output=True, text=True)

    def install(self):
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        return (self.destination / 'sshm').read_bytes()

    def test_install_repeat_and_replace_preserve_user_data(self):
        data = self.root / 'connections.json'
        data.write_bytes(b'unchanged')
        old = self.install()
        self.assertEqual(self.install(), old)
        self.make_release('0.5.0')
        self.assertNotEqual(self.install(), old)
        self.assertEqual(data.read_bytes(), b'unchanged')
        self.assertEqual((self.destination/'sshm').stat().st_mode & 0o777, 0o755)

    def test_invalid_signature_keeps_old(self):
        old = self.install()
        (self.fixture/'linux-SHA256SUMS').write_text('tampered')
        self.assertNotEqual(self.run_installer().returncode, 0)
        self.assertEqual((self.destination/'sshm').read_bytes(), old)

    def test_corrupt_archive_keeps_old(self):
        old = self.install()
        (self.fixture/'sshm-linux-x86_64.tar.gz').write_bytes(b'corrupt')
        self.assertNotEqual(self.run_installer().returncode, 0)
        self.assertEqual((self.destination/'sshm').read_bytes(), old)

    def test_download_failure_keeps_old(self):
        old = self.install()
        self.assertNotEqual(self.run_installer(FAIL_DOWNLOAD='sshm-linux-x86_64.tar.gz').returncode, 0)
        self.assertEqual((self.destination/'sshm').read_bytes(), old)

    def test_version_arch_and_extra_archive_entries_rejected(self):
        self.assertNotEqual(self.run_installer(TEST_ARCH='aarch64').returncode, 0)
        self.assertNotEqual(self.run_installer(SSHM_VERSION='0.5.0').returncode, 0)
        self.make_release(extra=True)
        self.assertNotEqual(self.run_installer().returncode, 0)
        self.assertFalse(self.destination.exists())
        self.assertFalse((self.root/'unexpected').exists())

    def test_broken_binary_is_not_installed(self):
        self.make_release(payload=b'#!/bin/sh\nexit 1\n')
        self.assertNotEqual(self.run_installer().returncode, 0)
        self.assertFalse(self.destination.exists())

    def test_foreign_or_modified_install_is_not_replaced(self):
        self.destination.mkdir(parents=True)
        self.executable(self.destination/'sshm', '#!/bin/sh\necho foreign\n')
        self.assertNotEqual(self.run_installer().returncode, 0)
        (self.destination/'sshm').unlink()
        self.install()
        (self.destination/'sshm').write_bytes(b'modified')
        self.assertNotEqual(self.run_installer().returncode, 0)
        self.assertEqual((self.destination/'sshm').read_bytes(), b'modified')

    def test_lock_prevents_concurrent_install(self):
        old = self.install()
        (self.destination/'.sshm-install.lock').mkdir()
        self.make_release('0.5.0')
        self.assertNotEqual(self.run_installer().returncode, 0)
        self.assertEqual((self.destination/'sshm').read_bytes(), old)

    def test_receipt_commit_failure_restores_old(self):
        old = self.install()
        self.make_release('0.5.0')
        self.executable(self.bin/'mv', '#!/bin/sh\ncase "$*" in *new-receipt*) exit 1;; esac\nexec /bin/mv "$@"\n')
        self.assertNotEqual(self.run_installer().returncode, 0)
        self.assertEqual((self.destination/'sshm').read_bytes(), old)
        self.assertEqual((self.destination/'.sshm-install.sha256').read_text().strip(), hashlib.sha256(old).hexdigest())

if __name__ == '__main__':
    unittest.main()

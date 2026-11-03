#!/usr/bin/env python3
"""Run installer transitions in temporary directories, never /usr/local.
Only ownership queries are mocked because the test runs without root.
"""
import hashlib
from pathlib import Path
import platform
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent

class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='sshm-installer-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.local = self.root / 'local'
        self.base = self.local / 'libexec/sshm'
        self.versions = self.base / 'versions'
        self.versions.mkdir(parents=True)
        (self.local / 'bin').mkdir()
        self.entry = self.local / 'bin/sshm'
        self.mock = self.root / 'mock'
        self.mock.mkdir()
        self.executable(self.mock / 'stat', '''#!/usr/bin/env python3
import os,sys
from pathlib import Path
path=Path(sys.argv[-1])
if sys.argv[-2] == '%u': print(501 if (path/'.untrusted').exists() else 0)
else: print(oct(path.stat().st_mode & 0o777)[2:])
''')
        self.executable(self.mock / 'id', '#!/bin/sh\necho 0\n')
        self.executable(self.mock / 'pkgutil', '#!/bin/sh\nexit 0\n')

    def executable(self, path, text):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        path.chmod(0o755)

    def binary(self, version, good=True):
        path = self.versions / version / 'sshm'
        self.executable(path, f'#!/bin/sh\necho "sshm {version}"\nexit {0 if good else 1}\n')
        return path

    def run_script(self, name, version='0.5.0', *args):
        text = (ROOT / 'scripts/macos' / name).read_text()
        text = text.replace('/usr/local', str(self.local)).replace('/private/tmp', str(self.root))
        text = text.replace('PATH=/usr/bin:/bin:/usr/sbin:/sbin', f'PATH={self.mock}:/usr/bin:/bin:/usr/sbin:/sbin')
        text = text.replace('@VERSION@', version).replace('@MACHINE@', platform.machine())
        script = self.root / name
        script.write_text(text)
        return subprocess.run(['/bin/sh', str(script), *args], capture_output=True, text=True)

    def test_fresh_install_upgrade_and_rollback_preserve_data(self):
        data = self.root / 'user-data'
        data.write_bytes(b'connections and keys unchanged')
        self.assertEqual(self.run_script('preinstall', '0.4.0').returncode, 0)
        old = self.binary('0.4.0')
        self.assertEqual(self.run_script('postinstall', '0.4.0').returncode, 0)
        self.assertEqual(self.entry.resolve(), old)
        self.assertEqual(self.run_script('preinstall').returncode, 0)
        new = self.binary('0.5.0')
        self.assertEqual(self.run_script('postinstall').returncode, 0)
        self.assertEqual(self.entry.resolve(), new)
        self.assertEqual((self.base / 'previous').resolve(), old)
        self.assertEqual(self.run_script('manage.sh', '0.5.0', 'rollback').returncode, 0)
        self.assertEqual(self.entry.resolve(), old)
        self.assertEqual(self.run_script('manage.sh', '0.5.0', 'uninstall').returncode, 0)
        self.assertFalse(self.entry.exists())
        self.assertFalse(self.base.exists())
        self.assertEqual(data.read_bytes(), b'connections and keys unchanged')

    def test_broken_new_binary_keeps_old_entry(self):
        old = self.binary('0.4.0')
        self.entry.symlink_to(old)
        self.binary('0.5.0', good=False)
        self.assertNotEqual(self.run_script('postinstall').returncode, 0)
        self.assertEqual(self.entry.resolve(), old)

    def test_failed_check_after_switch_restores_old_entry(self):
        old = self.binary('0.4.0')
        self.entry.symlink_to(old)
        counter = self.root / 'started'
        new = self.binary('0.5.0')
        self.executable(new, f"#!/bin/sh\n[ ! -e '{counter}' ] || exit 1\ntouch '{counter}'\necho 'sshm 0.5.0'\n")
        self.assertNotEqual(self.run_script('postinstall').returncode, 0)
        self.assertEqual(self.entry.resolve(), old)

    def test_privileged_install_checks_hash_team_and_apple_assessment(self):
        package = self.root / 'update.pkg'
        package.write_bytes(b'test package')
        digest = hashlib.sha256(package.read_bytes()).hexdigest()
        marker = self.root / 'installer-called'
        self.executable(self.mock / 'installer', f"#!/bin/sh\ntouch '{marker}'\n")
        self.executable(self.mock / 'spctl', '#!/bin/sh\nexit 0\n')
        self.executable(self.mock / 'pkgutil', "#!/bin/sh\necho '1. Developer ID Installer: Test (ABCDEFGHIJ)'\n")
        args = (str(package), digest, 'ABCDEFGHIJ')
        self.assertNotEqual(self.run_script('install-update.sh', '', str(package), '0' * 64, 'ABCDEFGHIJ').returncode, 0)
        self.assertFalse(marker.exists())
        self.assertNotEqual(self.run_script('install-update.sh', '', str(package), digest, 'WRONGTEAM1').returncode, 0)
        self.assertFalse(marker.exists())
        self.executable(self.mock / 'spctl', '#!/bin/sh\nexit 1\n')
        self.assertNotEqual(self.run_script('install-update.sh', '', *args).returncode, 0)
        self.assertFalse(marker.exists())
        self.executable(self.mock / 'spctl', '#!/bin/sh\nexit 0\n')
        result = self.run_script('install-update.sh', '', *args)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(marker.exists())

    def test_existing_manual_install_is_not_overwritten(self):
        self.executable(self.entry, '#!/bin/sh\nexit 0\n')
        self.assertNotEqual(self.run_script('preinstall').returncode, 0)
        self.assertFalse(self.entry.is_symlink())

    def test_installer_does_not_downgrade_a_newer_install(self):
        newer = self.binary('0.6.0')
        self.entry.symlink_to(newer)
        self.assertNotEqual(self.run_script('preinstall', '0.5.0').returncode, 0)
        self.assertEqual(self.entry.resolve(), newer)

    def test_same_version_is_not_overwritten(self):
        self.binary('0.5.0')
        self.assertNotEqual(self.run_script('preinstall').returncode, 0)

    def test_untrusted_directories_are_rejected(self):
        (self.base / '.untrusted').touch()
        self.assertNotEqual(self.run_script('preinstall').returncode, 0)
        (self.base / '.untrusted').unlink()
        self.base.chmod(0o777)
        self.assertNotEqual(self.run_script('preinstall').returncode, 0)
        self.base.chmod(0o755)

    def test_symlink_directory_is_rejected(self):
        self.versions.rmdir()
        self.versions.symlink_to(self.root, target_is_directory=True)
        self.assertNotEqual(self.run_script('preinstall').returncode, 0)

if __name__ == '__main__':
    unittest.main()

#!/usr/bin/env python3
"""Run only inside a disposable Linux x64 container, as root.
Serve real signed artifacts over local HTTPS and exercise curl | sh installation.
"""
import functools
import hashlib
import http.server
import os
import fcntl
import pty
import re
import select
import struct
import termios
import time
from pathlib import Path
import shutil
import ssl
import subprocess
import tempfile
import threading

assert Path('/.dockerenv').exists() and os.geteuid() == 0, 'disposable root Docker container required'
ROOT = Path('/workspace')
ARTIFACTS = Path('/artifact')
with tempfile.TemporaryDirectory(prefix='sshm-real-install-') as temp:
    work = Path(temp)
    served = work/'release'
    served.mkdir()
    for name in ['sshm-linux-x86_64.tar.gz', 'linux-SHA256SUMS', 'linux-SHA256SUMS.sig']:
        shutil.copy2(ARTIFACTS/name, served/name)
    certificate, key = work/'cert.pem', work/'tls.key'
    subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-keyout',str(key),'-out',str(certificate),'-days','1','-subj','/CN=localhost','-addext','subjectAltName=IP:127.0.0.1'],check=True,capture_output=True)
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(served))
    server = http.server.ThreadingHTTPServer(('127.0.0.1',0),handler)
    tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    tls.load_cert_chain(certificate,key)
    server.socket = tls.wrap_socket(server.socket,server_side=True)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    url = f'https://127.0.0.1:{server.server_port}'
    installer = (ROOT/'install.sh').read_text()
    release_url = 'https://github.com/qmtdlt/sshm/releases/latest/download'
    assert installer.count(release_url) == 1, 'installer download URL changed; refuse to test a public release'
    installer = installer.replace(release_url,url)
    (served/'install.sh').write_text(installer)
    data = work/'config'
    data.mkdir()
    sentinel = data/'keep-me'
    sentinel.write_bytes(b'user data is not installer input')
    env = os.environ.copy()
    env.update(CURL_CA_BUNDLE=str(certificate), SSHM_CONFIG_DIR=str(data), SSHM_UPDATE_CHECK='0')
    env.pop('SSHM_INSTALL_DIR',None)
    env.pop('SSHM_VERSION',None)
    command = f'curl --fail --silent --show-error --cacert "{certificate}" {url}/install.sh | sh'
    installed = Path('/usr/local/bin/sshm')
    assert not installed.exists(), 'fresh container required'
    try:
        subprocess.run(['/bin/sh','-c',command],env=env,check=True)
        assert installed.read_bytes() == (ARTIFACTS/'sshm').read_bytes()
        subprocess.run(['/bin/sh','-lc','command -v sshm && sshm --version && sshm list'],env=env,check=True)
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 120, 0, 0))
        terminal_env = env | {'TERM': 'xterm-256color', 'SSHM_GOOGLE_OAUTH_CONFIG': str(work/'no-oauth.json')}
        process = subprocess.Popen([str(installed)], stdin=slave, stdout=slave, stderr=slave, env=terminal_env, start_new_session=True)
        os.close(slave)
        output, plain, scan = b'', b'', b''
        try:
            deadline = time.monotonic()+15
            title = 'SSH 连接管理'.encode()
            while title not in plain and time.monotonic()<deadline:
                if select.select([master], [], [], 0.2)[0]:
                    chunk = os.read(master, 8192)
                    if not chunk: break
                    output += chunk
                    # Cursor positioning can split Chinese wide characters.
                    plain = re.sub(rb'\x1b\[[0-?]*[ -/]*[@-~]', b'', output)
                    scan += chunk
                    for _ in range(scan.count(b'\x1b[6n')): os.write(master, b'\x1b[1;1R')
                    scan = scan[-3:]
            assert title in plain, repr(plain[-1000:])
            os.write(master, b'q')
            assert process.wait(timeout=5)==0
        finally:
            if process.poll() is None: process.kill()
            os.close(master)
        subprocess.run(['/bin/sh','-c',command],env=env,check=True)
        # Exercise non-root personal installation without granting sudo rights.
        work.chmod(0o755)
        certificate.chmod(0o644)
        personal = work/'personal'
        personal.mkdir()
        os.chown(personal, 65534, 65534)
        personal_env = env | {'SSHM_INSTALL_DIR': str(personal/'bin')}
        subprocess.run(['/bin/sh','-c',command],env=personal_env,user=65534,group=65534,check=True)
        assert (personal/'bin/sshm').read_bytes()==installed.read_bytes()
        assert (personal/'bin/sshm').stat().st_uid==65534
        denied = subprocess.run(['/bin/sh','-c',command],env=env,user=65534,group=65534,capture_output=True,text=True)
        assert denied.returncode != 0, 'non-root system installation must require administrator authorization'
        before = hashlib.sha256(installed.read_bytes()).hexdigest()
        (served/'sshm-linux-x86_64.tar.gz').write_bytes(b'corrupt download')
        result = subprocess.run(['/bin/sh','-c',command],env=env,capture_output=True,text=True)
        assert result.returncode != 0
        assert hashlib.sha256(installed.read_bytes()).hexdigest() == before
        assert sentinel.read_bytes() == b'user data is not installer input'
        print('PASS: HTTPS curl | sh, pinned release signature, system PATH, interactive TUI, non-root personal install, administrator boundary, repeat install, corruption refusal, data retention')
    finally:
        server.shutdown()

"""Focused real Linux worker auth/lease checks; only owned temporary processes/config."""
import json, os, pathlib, secrets, subprocess, tempfile, time, urllib.request, urllib.error

binary = pathlib.Path(os.environ['CARGO_TARGET_DIR']) / 'debug/agent-workspace-backend'
with tempfile.TemporaryDirectory(prefix='awm-worker-check-') as directory:
    env = {**os.environ, 'AWM_CONFIG': directory + '/config.json'}
    env.pop('AWM_TOKEN', None)
    def start():
        child = subprocess.Popen([str(binary), '--desktop-worker'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
        token, nonce = secrets.token_hex(32), secrets.token_hex(32)
        child.stdin.write(json.dumps(dict(token=token, nonce=nonce, protocol=1)).encode() + b'\n')
        child.stdin.flush()
        ready = json.loads(child.stdout.readline())
        return child, token, nonce, ready['port']
    def request(port, path, token=None, origin=None):
        headers = {}
        if token: headers['Authorization'] = 'Bearer ' + token
        if origin: headers['Origin'] = origin
        try:
            with urllib.request.urlopen(urllib.request.Request(f'http://127.0.0.1:{port}/api/v1/{path}', headers=headers), timeout=3) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            return error.code, json.load(error)
    child, token, nonce, port = start()
    try:
        assert request(port, 'runtime/handshake')[0] == 401
        assert request(port, 'runtime/handshake', 'x' * 64)[0] == 401
        assert request(port, 'runtime/handshake', token, 'null')[0] == 403
        assert request(port, 'runtime/handshake', token, 'http://foreign.invalid')[0] == 403
        status, handshake = request(port, 'runtime/handshake', token)
        assert status == 200 and handshake['nonce'] == nonce and handshake['protocol'] == 1 and handshake['version'] == '0.1.0'
        assert handshake['environment']['distro'] == 'Ubuntu'
        assert request(port, 'projects?environment_id=00000000-0000-0000-0000-000000000000', token)[0] == 400
        assert request(port, 'projects?environment_id=' + handshake['environment']['id'], token) == (200, [])
        child.stdin.close()
        assert child.wait(timeout=5) == 0
        assert token.encode() not in child.stdout.read() + child.stderr.read()
    finally:
        if child.poll() is None: child.kill(); child.wait()
    child, token, nonce, port2 = start()
    try:
        assert request(port2, 'runtime/handshake', token)[1]['nonce'] == nonce
        # Keep pipe open without heartbeats: lease must still stop worker.
        assert child.wait(timeout=13) == 0
        child.stdin.close()
    finally:
        if child.poll() is None: child.kill(); child.wait()
    child = subprocess.Popen([str(binary), '--desktop-worker'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    child.communicate(json.dumps(dict(token='x' * 64, nonce='0' * 64, protocol=999)).encode() + b'\n', timeout=5)
    assert child.returncode != 0
print('PASS worker auth/Origin/scope/nonce/version, EOF/restart, lease and incompatible startup')

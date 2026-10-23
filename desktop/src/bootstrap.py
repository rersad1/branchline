# Fixed argv program. No shell, distro selection or secret environment variables.
# The binary comes from the host's embedded asset, verified before execution.
import errno, subprocess

def exact(n):
    data = bytearray()
    while len(data) < n:
        part = os.read(0, min(n - len(data), 65536))
        if not part:
            raise Failure('helper_asset')
        data.extend(part)
    return data

def main():
    prerequisites()
    frame = bytearray()
    while True:
        b = exact(1)
        if b == b'\n':
            break
        frame.extend(b)
        if len(frame) > 256:
            raise Failure('helper_asset')
    header = json.loads(frame)
    size = header['size']
    if type(size) is not int or not 0 < size <= MAX_ASSET:
        raise Failure('helper_asset')
    binary = exact(size)
    digest = hashlib.sha256(binary).hexdigest()
    if digest != header['sha256']:
        raise Failure('helper_asset')
    if binary[:6] != b'\x7fELF\x02\x01' or binary[18:20] != b'\x3e\x00':
        raise Failure('helper_platform')
    state = ledger()
    lease = None
    worker_fd = None
    receipt = None
    try:
        recover(state)
        path, worker_fd, lease, receipt = install_asset(state, binary, digest)
        try:
            child = subprocess.Popen([path, '--desktop-worker'], executable='/proc/self/fd/' + str(worker_fd),
                                     stdin=0, stdout=1, stderr=subprocess.DEVNULL, pass_fds=(lease, worker_fd))
        except PermissionError:
            raise Failure('helper_temp') from None
        except OSError as error:
            raise Failure('helper_platform' if error.errno in [errno.ENOEXEC, errno.ENOENT] else 'helper_startup') from None
        return child.wait()
    finally:
        if worker_fd is not None:
            os.close(worker_fd)
        if lease is not None:
            os.close(lease)
        if receipt is not None:
            cleanup(state, receipt)
        os.close(state)


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except (Failure, OSError, ValueError, KeyError, TypeError) as error:
        code = str(error) if isinstance(error, Failure) else 'helper_startup'
        print(json.dumps(dict(error=code)), flush=True)
        raise SystemExit(1) from None

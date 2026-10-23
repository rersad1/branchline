# Concatenated with bootstrap.py at build time. No runtime imports from disk.
import fcntl
import hashlib
import json
import os
import platform
import re
import stat
import sys
import time
import uuid

MAX_ASSET = 64 * 1024 * 1024
DIRECTORY = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
READ = os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK
NAME = re.compile(r'awm-runtime-v2-[0-9a-f]{32}')


class Failure(Exception):
    pass


def identity(meta):
    return [meta.st_dev, meta.st_ino]


def directory(path, create=False):
    """Open every component without following symlinks, then use relative fds."""
    if not os.path.isabs(path) or '..' in path.split('/'):
        raise Failure('helper_storage')
    fd = os.open('/', DIRECTORY)
    try:
        for part in path.split('/'):
            if not part or part == '.':
                continue
            if create:
                try:
                    os.mkdir(part, 0o700, dir_fd=fd)
                except FileExistsError:
                    pass
            child = os.open(part, DIRECTORY, dir_fd=fd)
            os.close(fd)
            fd = child
        return fd
    except BaseException:
        os.close(fd)
        raise


def ledger():
    base = os.environ.get('XDG_STATE_HOME') or os.path.expanduser('~/.local/state')
    fd = directory(os.path.join(base, 'branchline', 'helper-assets-v1'), True)
    meta = os.fstat(fd)
    if meta.st_uid != os.getuid() or stat.S_IMODE(meta.st_mode) != 0o700:
        os.close(fd)
        raise Failure('helper_storage')
    return fd


def owned_file(fd, expected, mode):
    meta = os.fstat(fd)
    return (identity(meta) == expected and stat.S_ISREG(meta.st_mode)
            and meta.st_uid == os.getuid() and meta.st_nlink == 1
            and stat.S_IMODE(meta.st_mode) == mode)


def cleanup(state, receipt):
    """Authority: private receipt + inode/content checks + unlocked lease.

    No process signals, recursion, prefix scan of /tmp or credential reads.
    Unknown/modified entries and active sessions are deliberately retained.
    """
    descriptors = []
    try:
        if not receipt.endswith('.json') or not NAME.fullmatch(receipt[:-5]):
            return False
        record_fd = os.open(receipt, READ, dir_fd=state)
        descriptors.append(record_fd)
        record_meta = os.fstat(record_fd)
        if not owned_file(record_fd, identity(record_meta), 0o600) or record_meta.st_size > 8192:
            return False
        record = json.loads(os.read(record_fd, 8193))
        if record['schema'] != 1 or record['name'] != receipt[:-5]:
            return False
        parent = directory(record['parent'])
        descriptors.append(parent)
        if identity(os.fstat(parent)) != record['parent_identity']:
            return False
        root = os.open(record['name'], DIRECTORY, dir_fd=parent)
        descriptors.append(root)
        meta = os.fstat(root)
        if (identity(meta) != record['directory_identity'] or meta.st_uid != os.getuid()
                or stat.S_IMODE(meta.st_mode) != 0o700 or sorted(os.listdir(root)) != ['lease', 'worker']):
            return False
        lease = os.open('lease', READ, dir_fd=root)
        descriptors.append(lease)
        if not owned_file(lease, record['lease_identity'], 0o600) or os.fstat(lease).st_size != 0:
            return False
        fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
        worker = os.open('worker', READ, dir_fd=root)
        descriptors.append(worker)
        if not owned_file(worker, record['worker_identity'], 0o700):
            return False
        size = os.fstat(worker).st_size
        if not 0 < size <= MAX_ASSET or size != record['size']:
            return False
        digest = hashlib.sha256()
        remaining = size
        while remaining:
            chunk = os.read(worker, min(remaining, 65536))
            if not chunk:
                return False
            digest.update(chunk)
            remaining -= len(chunk)
        if digest.hexdigest() != record['sha256']:
            return False
        for name, fd in [('worker', worker), ('lease', lease)]:
            if identity(os.stat(name, dir_fd=root, follow_symlinks=False)) != identity(os.fstat(fd)):
                return False
        if (identity(os.stat(record['name'], dir_fd=parent, follow_symlinks=False)) != identity(meta)
                or identity(os.stat(receipt, dir_fd=state, follow_symlinks=False)) != identity(record_meta)
                or sorted(os.listdir(root)) != ['lease', 'worker']):
            return False
        os.unlink('worker', dir_fd=root)
        os.unlink('lease', dir_fd=root)
        os.rmdir(record['name'], dir_fd=parent)
        os.unlink(receipt, dir_fd=state)
        return True
    except (OSError, ValueError, KeyError, TypeError):
        return False
    finally:
        for fd in reversed(descriptors):
            os.close(fd)


def recover(state):
    # Bound startup work; enumerate private receipts, never arbitrary temp dirs.
    started = time.monotonic()
    for receipt in sorted(os.listdir(state))[:128]:
        if time.monotonic() - started > 2:
            break
        cleanup(state, receipt)


def prerequisites():
    if sys.version_info < (3, 10):
        raise Failure('helper_prerequisite')
    try:
        libc, version = platform.libc_ver()
        # Distribution branding/version is validation metadata, not an ABI requirement.
        # In particular Ubuntu 24.04's glibc 2.39 can run this same shipped ELF.
        if (sys.platform != 'linux' or platform.machine() != 'x86_64'
                or libc != 'glibc' or tuple(map(int, version.split('.')[:2])) < (2, 39)):
            raise Failure('helper_platform')
        if not all(os.access(p, os.X_OK) for p in ['/usr/bin/git', '/bin/bash']):
            raise Failure('helper_prerequisite')
    except (OSError, ValueError):
        raise Failure('helper_platform') from None


def install_asset(state, binary, digest):
    import tempfile
    parent_path = os.path.abspath(tempfile.gettempdir())
    parent = directory(parent_path)
    try:
        name = 'awm-runtime-v2-' + uuid.uuid4().hex
        os.mkdir(name, 0o700, dir_fd=parent)
        root = os.open(name, DIRECTORY, dir_fd=parent)
        try:
            lease = os.open('lease', os.O_CREAT | os.O_EXCL | os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600, dir_fd=root)
            try:
                fcntl.flock(lease, fcntl.LOCK_EX)
                worker = os.open('worker', os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW | os.O_CLOEXEC, 0o700, dir_fd=root)
                with os.fdopen(worker, 'wb') as output:
                    output.write(binary)
                    output.flush()
                    os.fsync(output.fileno())
                    worker_identity = identity(os.fstat(output.fileno()))
                record = dict(schema=1, parent=parent_path, parent_identity=identity(os.fstat(parent)),
                              name=name, directory_identity=identity(os.fstat(root)),
                              lease_identity=identity(os.fstat(lease)), worker_identity=worker_identity,
                              size=len(binary), sha256=digest)
                receipt = name + '.json'
                fd = os.open(receipt, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600, dir_fd=state)
                with os.fdopen(fd, 'w') as output:
                    json.dump(record, output)
                    output.flush()
                    os.fsync(output.fileno())
                os.fsync(state)
                worker_fd = os.open('worker', READ, dir_fd=root)
                return os.path.join(parent_path, name, 'worker'), worker_fd, lease, receipt
            except BaseException:
                os.close(lease)
                raise
        finally:
            os.close(root)
    finally:
        os.close(parent)

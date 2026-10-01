#!/usr/bin/env python3
"""Offline backup restore: one native lease spans copy and durable installation."""

import ctypes
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import sys
import tempfile
from typing import Callable

LOCK = "world.lock"
IDENTITY = ".mcgo-world-backup-v1.json"
CHUNK = 1024 * 1024


class Refusal(Exception):
    def __init__(self, code: str, reason: str):
        super().__init__(reason)
        self.code = code


def _refuse(code: str, reason: str):
    raise Refusal(code, reason)


def _json(path: Path, limit: int, code: str) -> dict:
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                _refuse(code, "duplicate JSON field: " + key)
            result[key] = value
        return result

    try:
        _regular(path, code)
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
        with os.fdopen(fd, "rb") as handle:
            raw = handle.read(limit + 1)
    except OSError as error:
        _refuse(code, "JSON record is unavailable: " + str(error))
    if len(raw) > limit:
        _refuse(code, "JSON record exceeds its byte bound")
    try:
        result = json.loads(raw, object_pairs_hook=pairs)
    except (ValueError, UnicodeError) as error:
        _refuse(code, "JSON record does not decode: " + str(error))
    if type(result) is not dict:
        _refuse(code, "JSON record is not an object")
    return result


def _regular(path: Path, code: str = "restore_failed"):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_nlink <= 0:
        _refuse(code, "nonregular or unlinked file: " + str(path))
    return info


def _directory(path: Path, code: str = "restore_failed"):
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode):
        _refuse(code, "non-directory root: " + str(path))
    return info


def _id(info) -> str:
    return f"{info.st_dev}:{info.st_ino}"


def _entries(root: Path) -> list[Path]:
    _directory(root)
    files = []
    for directory, dirs, names in os.walk(root, followlinks=False):
        for name in dirs:
            _directory(Path(directory) / name)
        for name in names:
            path = Path(directory) / name
            _regular(path)
            files.append(path)
    return sorted(files, key=lambda path: path.relative_to(root).as_posix())


def _tree(root: Path, *, backup: bool = False) -> str:
    digest = hashlib.sha256()
    for path in _entries(root):
        if path.name == LOCK or (backup and path.name == IDENTITY):
            continue
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
        with os.fdopen(fd, "rb") as handle:
            info = os.fstat(handle.fileno())
            if not stat.S_ISREG(info.st_mode):
                _refuse("restore_failed", "tree file changed type")
            digest.update(path.relative_to(root).as_posix().encode("utf-8"))
            digest.update(b"\0")
            digest.update(info.st_size.to_bytes(8, "little"))
            read = 0
            while piece := handle.read(CHUNK):
                digest.update(piece)
                read += len(piece)
            if read != info.st_size:
                _refuse("restore_failed", "tree file changed size")
    return digest.hexdigest()


def _sync(path: Path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def _publish(path: Path, manifest: dict):
    fd, temporary = tempfile.mkstemp(prefix=path.name + ".restore.", dir=path.parent)
    # A failed publication retains its private file for diagnosis; it never
    # replaces the last proven checkpoint until the file barrier succeeds.
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, indent=2, sort_keys=True)
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, path)
    _sync(path.parent)


class _NativeRename:
    """Exact libc operations; an unavailable exchange has no delete-gap fallback."""

    def __init__(self):
        libc = ctypes.CDLL(None, use_errno=True)
        if sys.platform.startswith("linux"):
            self.function = getattr(libc, "renameat2", None)
            self.exchange, self.no_replace = 2, 1
            if self.function is not None:
                self.function.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int,
                                          ctypes.c_char_p, ctypes.c_uint]
                self.function.restype = ctypes.c_int
            self.linux = True
        elif sys.platform == "darwin":
            self.function = getattr(libc, "renamex_np", None)
            self.exchange, self.no_replace = 2, 4
            if self.function is not None:
                self.function.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
                self.function.restype = ctypes.c_int
            self.linux = False
        else:
            self.function = None
        if self.function is None:
            _refuse("restore_failed", "native atomic directory exchange is unavailable")

    def rename(self, old: Path, new: Path, *, exchange: bool):
        flag = self.exchange if exchange else self.no_replace
        if self.linux:
            result = self.function(-100, os.fsencode(old), -100, os.fsencode(new), flag)
        else:
            result = self.function(os.fsencode(old), os.fsencode(new), flag)
        if result != 0:
            cause = ctypes.get_errno()
            raise OSError(cause, os.strerror(cause), str(old), str(new))


def _copy(backup: Path, staged: Path):
    _entries(backup)
    staged.mkdir()
    for directory, dirs, files in os.walk(backup, followlinks=False):
        destination = staged / Path(directory).relative_to(backup)
        for name in dirs:
            (destination / name).mkdir()
        for name in files:
            if Path(directory) == backup and name in (LOCK, IDENTITY):
                continue
            source, target = Path(directory) / name, destination / name
            fd = os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
            with os.fdopen(fd, "rb") as reader, target.open("xb") as writer:
                shutil.copyfileobj(reader, writer, CHUNK)
                writer.flush()
                shutil.copystat(source, target, follow_symlinks=False)
                os.fsync(writer.fileno())
    # Children reach their barriers before the parent directory that names them.
    for directory, _, _ in os.walk(staged, topdown=False):
        target = Path(directory)
        shutil.copystat(backup / target.relative_to(staged), target, follow_symlinks=False)
        _sync(target)


def _restore(manifest_path: Path, world: Path, backup: Path, nonce: str, *,
             boundary: Callable[[str], None] | None = None) -> Path:
    """Keep all flock descriptors until every actual filesystem boundary returns.

    The private callback observes real durable operations for owned crash tests;
    the CLI never supplies it and has no fault-control environment surface.
    """
    descriptors = []
    try:
        paths = (manifest_path, world, backup)
        for path in paths:
            if not path.is_absolute() or path.resolve() != path:
                _refuse("invalid_manifest", "managed paths must be canonical without symlink aliases")
        run = manifest_path.parent
        _directory(run, "invalid_manifest")
        temporary_roots = (Path(tempfile.gettempdir()).resolve(), Path("/tmp").resolve(),
                           Path("/var/tmp").resolve())
        if not any(run != root and run.is_relative_to(root) for root in temporary_roots):
            _refuse("invalid_manifest", "run directory escapes temporary roots")
        if (world == run or backup == run or not world.is_relative_to(run)
                or not backup.is_relative_to(run) or world.is_relative_to(backup)
                or backup.is_relative_to(world) or manifest_path.is_relative_to(world)
                or manifest_path.is_relative_to(backup)):
            _refuse("invalid_manifest", "managed restore trees overlap or escape the run directory")
        if re.fullmatch(r"[0-9a-f]{32}", nonce) is None:
            _refuse("invalid_manifest", "invalid restore nonce")
        manifest = _json(manifest_path, CHUNK, "invalid_manifest")
        if (manifest.get("schema_version") != 1 or manifest.get("world_path") != str(world)
                or manifest.get("backup_path") != str(backup)
                or manifest.get("start_nonce") != nonce):
            _refuse("identity_mismatch", "restore arguments differ from the manifest")
        if "restore_stage" not in manifest:
            _refuse("invalid_manifest", "restore stage is absent")
        stage = manifest["restore_stage"]
        if stage not in (None, "staged", "swapped", "old_retired", "backup_installed"):
            _refuse("invalid_manifest", "unrecognized restore stage")
        backup_hash = manifest.get("backup_tree_sha256")
        if not isinstance(backup_hash, str) or re.fullmatch(r"[0-9a-f]{64}", backup_hash) is None:
            _refuse("invalid_manifest", "invalid backup digest")
        try:
            _directory(backup, "backup_mismatch")
            identity = _json(backup / IDENTITY, 4096, "backup_mismatch")
            if identity.get("migration_version") != 1 or not identity.get("source"):
                _refuse("backup_mismatch", "backup identity does not bind its source")
            if _tree(backup, backup=True) != backup_hash:
                _refuse("backup_mismatch", "backup tree differs from the manifest")
        except OSError as error:
            _refuse("backup_mismatch", str(error))
        except Refusal as error:
            _refuse("backup_mismatch", str(error))
        staged = Path(f"{world}.restore.{nonce}")
        retired = Path(f"{world}.retired.{nonce}")
        for path in (world, staged, retired):
            if path.resolve() != path:
                _refuse("invalid_manifest", "restore sibling is a symlink alias")
            if path.exists():
                _directory(path, "invalid_manifest")
        missing_current = not world.exists()
        if missing_current and stage not in ("staged", "old_retired"):
            _refuse("invalid_manifest", "current world is absent outside a legacy checkpoint")
        owner_root = retired if missing_current else world
        if not owner_root.exists():
            _refuse("invalid_manifest", "original world and lease are absent")

        protected = set()

        def hold(path: Path, expected: str | None = None):
            try:
                before = _regular(path, "identity_mismatch")
                fd = os.open(path, os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC)
            except OSError as error:
                _refuse("identity_mismatch", "world lease is unavailable: " + str(error))
            descriptors.append(fd)
            info = os.fstat(fd)
            if not stat.S_ISREG(info.st_mode) or info.st_nlink <= 0 or _id(before) != _id(info):
                _refuse("identity_mismatch", "opened lease differs from its regular pathname")
            if expected is not None and expected != "world.lock:" + _id(info):
                _refuse("identity_mismatch", "original world lease differs from the manifest")
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError as error:
                if error.errno in (errno.EAGAIN, errno.EACCES):
                    _refuse("writer_live", "native world lease is already owned")
                raise
            if _id(_regular(path, "identity_mismatch")) != _id(info):
                _refuse("identity_mismatch", "lease pathname changed after flock")
            protected.add(_id(info))
            return info

        held = hold(owner_root / LOCK, manifest.get("lease_identity", ""))
        if manifest.get("lease_identity") != "world.lock:" + _id(held):
            _refuse("identity_mismatch", "original world lease differs from the manifest")

        def observe(name):
            if boundary is not None:
                boundary(name)

        def lease(root):
            if _id(_regular(root / LOCK, "identity_mismatch")) != _id(held):
                _refuse("identity_mismatch", "managed world has a separate lock lineage")

        def protect_stage():
            # A stale copied lock may have its own real writer. Hold that inode
            # too before deleting its directory or rebinding its pathname.
            if staged.exists():
                _entries(staged)
                lock = staged / LOCK
                if lock.exists() and _id(_regular(lock, "identity_mismatch")) not in protected:
                    hold(lock)

        def bind_stage():
            protect_stage()
            lock = staged / LOCK
            if lock.exists():
                if _id(_regular(lock, "identity_mismatch")) == _id(held):
                    return
                lock.unlink()
            os.link(world / LOCK, lock, follow_symlinks=False)
            _sync(staged)
            _sync(world.parent)

        def role(root, directory_id, digest, *, installed=False):
            if _id(_directory(root)) != directory_id:
                _refuse("restore_failed", "original directory identity changed")
            lease(root)
            if _tree(root, backup=installed) != digest:
                _refuse("restore_failed", "recorded directory role hash changed")

        def installed():
            _directory(world)
            lease(world)
            if _tree(world) != backup_hash:
                _refuse("restore_failed", "installed world differs from the backup")

        observe("locked")
        native = _NativeRename()
        original_id = manifest.get("restore_directory_identity")
        original_hash = manifest.get("restore_original_tree_sha256")
        if (original_id is None) != (original_hash is None):
            _refuse("invalid_manifest", "incomplete original restore role checkpoint")
        if original_id is not None and (
                not isinstance(original_id, str) or re.fullmatch(r"[0-9]+:[0-9]+", original_id) is None
                or not isinstance(original_hash, str)
                or re.fullmatch(r"[0-9a-f]{64}", original_hash) is None):
            _refuse("invalid_manifest", "invalid original restore role checkpoint")
        if stage == "backup_installed":
            installed()
            if retired.exists():
                lease(retired)
                if _id(_directory(retired)) == _id(_directory(world)):
                    _refuse("identity_mismatch", "retired and current directory identities coincide")
                if original_id is not None:
                    role(retired, original_id, original_hash)
                else:
                    manifest.update(restore_directory_identity=_id(_directory(retired)),
                                    restore_original_tree_sha256=_tree(retired))
                    _publish(manifest_path, manifest)
            # Physical roles can survive an interrupted barrier. Requalify
            # their parent entries and the already-published checkpoint before
            # returning idempotently; existence alone is not durability.
            _sync(world.parent)
            _sync(manifest_path.parent)
            observe("installed")
            return retired
        if missing_current:
            # Historical retire-first crashes return the original owner to the
            # canonical name before using the continuous-presence algorithm.
            if original_id is not None:
                role(retired, original_id, original_hash)
            else:
                _entries(retired)
            protect_stage()
            if staged.exists() and _tree(staged, backup=True) != backup_hash:
                _refuse("restore_failed", "legacy staged copy differs from backup")
            native.rename(retired, world, exchange=False)
            _sync(world.parent)
            stage = "staged"
        elif stage == "old_retired":
            _refuse("restore_failed", "legacy retired and current worlds collide")
        if stage is None:
            if retired.exists():
                _refuse("restore_failed", "fresh restore has an existing retired world")
            protect_stage()
            if staged.exists():
                shutil.rmtree(staged)
                _sync(world.parent)
            _copy(backup, staged)
            original_id = original_hash = None
            stage = "staged"
        if stage == "staged" and original_id is None:
            if retired.exists():
                _refuse("restore_failed", "legacy current and retired worlds collide")
            original_id = _id(_directory(world))
            original_hash = _tree(world)
            if not staged.exists():
                if not missing_current:
                    _refuse("restore_failed", "staged copy vanished")
                _copy(backup, staged)
            if _tree(staged, backup=True) != backup_hash:
                _refuse("restore_failed", "staged copy differs from backup")
            bind_stage()
            record = staged / IDENTITY
            if record.exists():
                _regular(record)
                record.unlink()
                _sync(staged)
            shutil.copystat(backup, staged, follow_symlinks=False)
            _sync(staged)
            manifest.update(restore_stage="staged", restore_directory_identity=original_id,
                            restore_original_tree_sha256=original_hash)
            _publish(manifest_path, manifest)
            observe("staged")
        if stage == "staged":
            if retired.exists():
                _refuse("restore_failed", "retired sibling collides before exchange")
            current_id = _id(_directory(world))
            staged_id = _id(_directory(staged))
            if current_id == original_id:
                role(world, original_id, original_hash)
                if _tree(staged, backup=True) != backup_hash:
                    _refuse("restore_failed", "staged backup differs before exchange")
                bind_stage()
                lease(staged)
                native.rename(world, staged, exchange=True)
                _sync(world.parent)
                observe("swapped")
            elif staged_id != original_id:
                _refuse("restore_failed", "neither directory owns the recorded original role")
            role(staged, original_id, original_hash)
            installed()
            _sync(world.parent)
            manifest["restore_stage"] = "swapped"
            _publish(manifest_path, manifest)
            stage = "swapped"
        if stage == "swapped":
            installed()
            if staged.exists():
                role(staged, original_id, original_hash)
                if retired.exists():
                    _refuse("restore_failed", "retired sibling collides before retirement")
                native.rename(staged, retired, exchange=False)
                _sync(world.parent)
                observe("retired")
            role(retired, original_id, original_hash)
            installed()
            _sync(world.parent)
            manifest.update(restore_stage="backup_installed", world_tree_sha256=backup_hash)
            _publish(manifest_path, manifest)
            observe("installed")
            return retired
        _refuse("invalid_manifest", "restore checkpoint cannot continue")
    except Refusal:
        raise
    except (OSError, ValueError, TypeError) as error:
        _refuse("restore_failed", str(error))
    finally:
        # Closing once releases both the original guard and any guarded stale
        # stage lineage, including after uncertain barriers and native errors.
        for fd in reversed(descriptors):
            os.close(fd)


if __name__ == "__main__":
    try:
        if len(sys.argv) != 5:
            _refuse("invalid_manifest", "restore requires manifest, world, backup and nonce")
        _restore(Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3]), sys.argv[4])
    except Refusal as error:
        print(f"FAIL {error.code} {error}", file=sys.stderr)
        sys.exit(1)

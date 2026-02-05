"""
Shared fixtures for FerresDB end-to-end tests.

Provides: port per worker (xdist-safe), temp storage dir, server process,
and paths to corpus and repo roots.
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Generator

import pytest

# Repo root: conftest in tests/e2e/ -> parent=tests/e2e, parent.parent=ferres-db-core (Cargo.toml, crates/)
_conftest_dir = Path(__file__).resolve().parent
REPO_ROOT = _conftest_dir.parent.parent
E2E_DIR = _conftest_dir
BINARY_NAME = "ferres-db-server"
# On Windows, cargo produces ferres-db-server.exe (and sometimes ferres_db_server.exe)
BINARY_SUFFIX = ".exe" if sys.platform == "win32" else ""
# Cargo may emit underscore variant on Windows (hyphen -> underscore in artifact name)
BINARY_NAME_UNDERSCORE = BINARY_NAME.replace("-", "_")


def _worker_id() -> str:
    """Return pytest-xdist worker id or 'master' when not distributed."""
    return os.environ.get("PYTEST_XDIST_WORKER", "master")


def _worker_port_offset() -> int:
    """Numeric offset for port based on worker (e.g. gw0 -> 0, gw1 -> 1)."""
    wid = _worker_id()
    if wid == "master":
        return 0
    # gw0, gw1, ... -> 0, 1, ...
    if wid.startswith("gw"):
        try:
            return int(wid[2:])
        except ValueError:
            pass
    return hash(wid) % 100


@pytest.fixture(scope="session")
def e2e_port_base() -> int:
    """Base port for e2e server (3010 + worker offset)."""
    return 3010 + (_worker_port_offset() % 200)


@pytest.fixture
def e2e_port(e2e_port_base: int, request: pytest.FixtureRequest) -> int:
    """Unique port for this test (base + item index when possible)."""
    # Use base for simplicity; multiple tests in same worker may run sequentially
    # so same port is ok if tests don't overlap. For parallel tests we rely on xdist
    # giving each worker a different e2e_port_base.
    return e2e_port_base


@pytest.fixture
def e2e_storage_dir(tmp_path: Path) -> Path:
    """Temporary directory for server storage (isolated per test)."""
    storage = tmp_path / "ferres_data"
    storage.mkdir(parents=True, exist_ok=True)
    return storage


def _server_binary_path(release: bool = True, use_underscore_name: bool = False) -> Path:
    """Path to server binary (with .exe on Windows)."""
    subdir = "release" if release else "debug"
    name = BINARY_NAME_UNDERSCORE if use_underscore_name else BINARY_NAME
    return REPO_ROOT / "target" / subdir / f"{name}{BINARY_SUFFIX}"


def _find_server_binary() -> Path | None:
    """Return path to release binary or None if not built."""
    for release in (True, False):
        for use_underscore in (False, True):
            binary = _server_binary_path(release=release, use_underscore_name=use_underscore)
            if binary.exists():
                return binary
    return None


def _ensure_server_built() -> Path:
    """Build server if needed; return path to binary."""
    binary = _find_server_binary()
    if binary is not None:
        return binary
    cwd = str(REPO_ROOT.resolve())
    result = subprocess.run(
        ["cargo", "build", "--release", "--bin", BINARY_NAME],
        cwd=cwd,
        capture_output=True,
        text=True,
    )
    binary = _find_server_binary()
    if binary is not None:
        return binary
    # Build may have succeeded but artifact name varies; list what we looked for
    candidates = [
        _server_binary_path(release=True, use_underscore_name=False),
        _server_binary_path(release=True, use_underscore_name=True),
    ]
    release_dir = REPO_ROOT / "target" / "release"
    try:
        existing = list(release_dir.glob("*.exe")) if release_dir.exists() else []
    except OSError:
        existing = []
    msg = (
        "Server binary not found after build. "
        f"Looked for: {[str(p) for p in candidates]}. "
        f"Release dir contents: {[p.name for p in existing]}."
    )
    if result.returncode != 0:
        msg += f" Cargo stderr: {result.stderr or result.stdout or 'none'}"
    raise RuntimeError(msg)


def _wait_for_server(base_url: str, timeout_sec: float = 15.0) -> bool:
    """Poll GET /health until 200 or timeout."""
    import urllib.request
    import urllib.error

    health = f"{base_url.rstrip('/')}/health"
    deadline = time.monotonic() + timeout_sec
    while time.monotonic() < deadline:
        try:
            req = urllib.request.Request(health, method="GET")
            with urllib.request.urlopen(req, timeout=2) as r:
                if r.status == 200:
                    return True
        except (OSError, urllib.error.URLError, TimeoutError):
            time.sleep(0.2)
    return False


class FerresServer:
    """Handle to a running FerresDB server subprocess."""

    def __init__(self, process: subprocess.Popen, base_url: str, storage_path: Path):
        self.process = process
        self.base_url = base_url
        self.storage_path = storage_path

    def stop(self) -> None:
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)


@pytest.fixture
def ferres_server(e2e_port: int, e2e_storage_dir: Path) -> Generator[FerresServer, None, None]:
    """Start FerresDB server in background; yield handle; stop on teardown."""
    binary = _ensure_server_built()
    env = {**os.environ, "PORT": str(e2e_port), "STORAGE_PATH": str(e2e_storage_dir)}
    proc = subprocess.Popen(
        [str(binary)],
        cwd=REPO_ROOT,
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    base_url = f"http://127.0.0.1:{e2e_port}"
    if not _wait_for_server(base_url):
        proc.terminate()
        proc.wait(timeout=5)
        pytest.fail("Server did not become ready in time")
    handle = FerresServer(proc, base_url, e2e_storage_dir)
    yield handle
    handle.stop()


@pytest.fixture(scope="session")
def corpus_10_path() -> Path:
    """Path to the 10-document corpus fixture."""
    return E2E_DIR / "fixtures" / "corpus_10"


@pytest.fixture(scope="session")
def ingestion_root() -> Path:
    """Path to examples/ingestion (for ingest.py)."""
    return REPO_ROOT / "examples" / "ingestion"


@pytest.fixture(scope="session")
def simple_rag_root() -> Path:
    """Path to examples/simple_rag (for app.py)."""
    return REPO_ROOT / "examples" / "simple_rag"

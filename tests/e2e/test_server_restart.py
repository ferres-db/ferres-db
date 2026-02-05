"""
E2E: server restart — insert 1000 points, kill server, restart with same storage,
validate points persisted and search returns results.
"""

from __future__ import annotations

import math
import random
import subprocess
from pathlib import Path

import pytest
import requests

import os

from conftest import REPO_ROOT, _ensure_server_built, _wait_for_server

DIMENSION = 384
NUM_POINTS = 1000
# Keep each request under server body limit (413); 50 points × 384 dims fits comfortably
BATCH_SIZE = 50


def _normalized_vector(dimension: int, seed: int | None = None) -> list[float]:
    """Deterministic normalized vector for cosine similarity."""
    rng = random.Random(seed)
    v = [rng.gauss(0, 1) for _ in range(dimension)]
    norm = math.sqrt(sum(x * x for x in v))
    if norm == 0:
        v[0] = 1.0
        norm = 1.0
    return [x / norm for x in v]


def _make_points(n: int, dimension: int, offset: int = 0) -> list[dict]:
    """Generate n points with deterministic vectors and metadata."""
    points = []
    for i in range(n):
        point_id = f"restart-test-{offset + i}"
        vector = _normalized_vector(dimension, seed=offset + i)
        metadata = {"index": offset + i, "test": "server_restart"}
        points.append({"id": point_id, "vector": vector, "metadata": metadata})
    return points


def test_server_restart(ferres_server, e2e_port, e2e_storage_dir):
    """Insert 1000 points, kill server, restart, validate persistence and search."""
    base_url = ferres_server.base_url
    collection = "restart_collection"
    storage_path = e2e_storage_dir

    # 1. Create collection
    resp = requests.post(
        f"{base_url.rstrip('/')}/api/v1/collections",
        json={"name": collection, "dimension": DIMENSION, "distance": "Cosine"},
        timeout=5,
    )
    assert resp.status_code in (200, 201), f"create collection failed: {resp.text}"

    # 2. Insert 1000 points in batches of 250
    for batch_start in range(0, NUM_POINTS, BATCH_SIZE):
        points = _make_points(BATCH_SIZE, DIMENSION, offset=batch_start)
        r = requests.post(
            f"{base_url.rstrip('/')}/api/v1/collections/{collection}/points",
            json={"points": points},
            timeout=30,
        )
        assert r.status_code == 200, f"upsert batch failed: {r.text}"
        data = r.json()
        assert data.get("upserted", 0) == BATCH_SIZE, f"expected {BATCH_SIZE} upserted: {data}"

    # 3. Verify count before kill
    stats_resp = requests.get(
        f"{base_url.rstrip('/')}/api/v1/collections/{collection}/stats",
        timeout=5,
    )
    assert stats_resp.status_code == 200
    assert stats_resp.json()["num_points"] == NUM_POINTS

    # 4. Force persist to disk (server only auto-saves every 30s or on graceful shutdown)
    save_resp = requests.post(f"{base_url.rstrip('/')}/api/v1/save", timeout=5)
    assert save_resp.status_code == 200, f"save failed: {save_resp.text}"

    # 5. Kill server
    ferres_server.stop()

    # 6. Restart server with same port and storage
    binary = _ensure_server_built()
    env = {
        **os.environ,
        "PORT": str(e2e_port),
        "STORAGE_PATH": str(storage_path),
    }
    proc = subprocess.Popen(
        [str(binary)],
        cwd=str(REPO_ROOT),
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    if not _wait_for_server(base_url):
        proc.terminate()
        proc.wait(timeout=5)
        pytest.fail("Server did not become ready after restart")

    try:
        # 7. Validate persistence: num_points == 1000
        stats_resp2 = requests.get(
            f"{base_url.rstrip('/')}/api/v1/collections/{collection}/stats",
            timeout=5,
        )
        assert stats_resp2.status_code == 200, f"stats after restart: {stats_resp2.text}"
        assert stats_resp2.json()["num_points"] == NUM_POINTS, (
            f"Expected {NUM_POINTS} points after restart, got {stats_resp2.json()['num_points']}"
        )

        # 8. Query and validate results (search with first point's vector)
        query_vector = _normalized_vector(DIMENSION, seed=0)
        search_resp = requests.post(
            f"{base_url.rstrip('/')}/api/v1/collections/{collection}/search",
            json={"vector": query_vector, "limit": 5},
            timeout=10,
        )
        assert search_resp.status_code == 200, f"search after restart: {search_resp.text}"
        results = search_resp.json().get("results", [])
        assert len(results) >= 1, "Expected at least one search result after restart"
        assert results[0].get("id") and "score" in results[0]
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)

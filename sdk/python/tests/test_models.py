"""Tests for models."""

import pytest
from vector_db_client import (
    Point,
    Collection,
    CollectionListItem,
    SearchResult,
    UpsertResult,
    DistanceMetric,
)


def test_point_to_dict():
    """Test Point.to_dict()."""
    point = Point(
        id="test-1",
        vector=[0.1, 0.2, 0.3],
        metadata={"text": "hello", "score": 0.95},
    )
    
    result = point.to_dict()
    
    assert result == {
        "id": "test-1",
        "vector": [0.1, 0.2, 0.3],
        "metadata": {"text": "hello", "score": 0.95},
    }


def test_point_from_dict():
    """Test Point.from_dict()."""
    data = {
        "id": "test-1",
        "vector": [0.1, 0.2, 0.3],
        "metadata": {"text": "hello"},
    }
    
    point = Point.from_dict(data)
    
    assert point.id == "test-1"
    assert point.vector == [0.1, 0.2, 0.3]
    assert point.metadata == {"text": "hello"}


def test_point_from_dict_no_metadata():
    """Test Point.from_dict() with missing metadata."""
    data = {
        "id": "test-1",
        "vector": [0.1, 0.2, 0.3],
    }
    
    point = Point.from_dict(data)
    
    assert point.metadata == {}


def test_collection_to_dict():
    """Test Collection.to_dict()."""
    collection = Collection(
        name="test-collection",
        dimension=128,
        distance=DistanceMetric.COSINE,
    )
    
    result = collection.to_dict()
    
    assert result == {
        "name": "test-collection",
        "dimension": 128,
        "distance": "Cosine",
    }


def test_collection_list_item_from_dict():
    """Test CollectionListItem.from_dict()."""
    data = {
        "name": "test-collection",
        "dimension": 128,
        "num_points": 100,
        "created_at": 1234567890,
    }
    
    item = CollectionListItem.from_dict(data)
    
    assert item.name == "test-collection"
    assert item.dimension == 128
    assert item.num_points == 100
    assert item.created_at == 1234567890


def test_search_result_from_dict():
    """Test SearchResult.from_dict()."""
    data = {
        "id": "test-1",
        "score": 0.95,
        "metadata": {"text": "hello"},
    }
    
    result = SearchResult.from_dict(data)
    
    assert result.id == "test-1"
    assert result.score == 0.95
    assert result.metadata == {"text": "hello"}


def test_upsert_result_from_dict():
    """Test UpsertResult.from_dict()."""
    data = {
        "upserted": 5,
        "failed": [
            {"id": "bad-1", "reason": "dimension mismatch"},
            {"id": "bad-2", "reason": "invalid vector"},
        ],
    }
    
    result = UpsertResult.from_dict(data)
    
    assert result.upserted == 5
    assert len(result.failed) == 2
    assert result.failed[0]["id"] == "bad-1"
    assert result.failed[1]["reason"] == "invalid vector"


def test_upsert_result_from_dict_no_failed():
    """Test UpsertResult.from_dict() with missing failed field."""
    data = {
        "upserted": 5,
    }
    
    result = UpsertResult.from_dict(data)
    
    assert result.upserted == 5
    assert result.failed == []


def test_distance_metric_values():
    """Test DistanceMetric enum values."""
    assert DistanceMetric.COSINE.value == "Cosine"
    assert DistanceMetric.DOT_PRODUCT.value == "DotProduct"
    assert DistanceMetric.EUCLIDEAN.value == "Euclidean"

"""Tests for exceptions."""

import pytest
from vector_db_client import (
    VectorDBError,
    CollectionNotFoundError,
    CollectionAlreadyExistsError,
    InvalidDimensionError,
    InvalidPayloadError,
    InternalError,
    ConnectionError,
)


def test_vector_db_error():
    """Test base VectorDBError."""
    error = VectorDBError("Test error", code=500)
    
    assert str(error) == "Test error"
    assert error.message == "Test error"
    assert error.code == 500


def test_collection_not_found_error():
    """Test CollectionNotFoundError."""
    error = CollectionNotFoundError("test-collection")
    
    assert "test-collection" in str(error)
    assert error.code == 404


def test_collection_already_exists_error():
    """Test CollectionAlreadyExistsError."""
    error = CollectionAlreadyExistsError("test-collection")
    
    assert "test-collection" in str(error)
    assert error.code == 409


def test_invalid_dimension_error():
    """Test InvalidDimensionError."""
    error = InvalidDimensionError("dimension must be > 0")
    
    assert "dimension" in str(error)
    assert error.code == 400


def test_invalid_payload_error():
    """Test InvalidPayloadError."""
    error = InvalidPayloadError("invalid JSON format")
    
    assert "invalid JSON" in str(error)
    assert error.code == 400


def test_internal_error():
    """Test InternalError."""
    error = InternalError("database connection failed")
    
    assert "database connection" in str(error)
    assert error.code == 500


def test_connection_error():
    """Test ConnectionError."""
    error = ConnectionError("connection timeout")
    
    assert "connection timeout" in str(error)
    assert error.code is None

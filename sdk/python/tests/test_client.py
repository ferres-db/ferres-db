"""Tests for VectorDBClient."""

import pytest
import httpx
from unittest.mock import AsyncMock, patch
from typing import Dict, Any

from vector_db_client import (
    VectorDBClient,
    Point,
    Collection,
    CollectionListItem,
    SearchResult,
    UpsertResult,
    DistanceMetric,
    CollectionNotFoundError,
    CollectionAlreadyExistsError,
    InvalidDimensionError,
    InvalidPayloadError,
    ConnectionError,
)


@pytest.fixture
def client():
    """Create a test client."""
    return VectorDBClient(base_url="http://localhost:3000", timeout=30)


@pytest.fixture
def mock_response():
    """Create a mock response helper."""
    def _create(status_code: int, json_data: Dict[str, Any] = None):
        response = AsyncMock(spec=httpx.Response)
        response.status_code = status_code
        response.is_success = 200 <= status_code < 300
        if json_data:
            # Make json() an async function that returns the data
            async def json_mock():
                return json_data
            response.json = json_mock
        else:
            async def json_mock():
                return {}
            response.json = json_mock
        response.text = ""
        return response
    return _create


@pytest.mark.asyncio
async def test_create_collection_success(client, mock_response):
    """Test successful collection creation."""
    response_data = {
        "name": "test-collection",
        "dimension": 128,
        "distance": "Cosine",
        "created_at": 1234567890,
    }
    
    with patch.object(client.client, "request", new_callable=AsyncMock) as mock_request:
        mock_request.return_value = mock_response(201, response_data)
        
        collection = await client.create_collection(
            name="test-collection",
            dimension=128,
            distance=DistanceMetric.COSINE,
        )
        
        assert collection.name == "test-collection"
        assert collection.dimension == 128
        assert collection.distance == DistanceMetric.COSINE
        assert collection.created_at == 1234567890
        
        # Verify request was made correctly
        mock_request.assert_called_once()
        call_args = mock_request.call_args
        assert call_args.kwargs["method"] == "POST"
        assert "/api/v1/collections" in call_args.kwargs["url"]
        assert call_args.kwargs["json"] == {
            "name": "test-collection",
            "dimension": 128,
            "distance": "Cosine",
        }


@pytest.mark.asyncio
async def test_create_collection_already_exists(client, mock_response):
    """Test collection creation when collection already exists."""
    error_data = {
        "error": "collection_already_exists",
        "message": "collection 'test-collection' already exists",
        "code": 409,
    }
    
    with patch.object(client.client, "request", new_callable=AsyncMock) as mock_request:
        mock_request.return_value = mock_response(409, error_data)
        
        with pytest.raises(CollectionAlreadyExistsError) as exc_info:
            await client.create_collection(
                name="test-collection",
                dimension=128,
                distance=DistanceMetric.COSINE,
            )
        
        assert "test-collection" in str(exc_info.value)


@pytest.mark.asyncio
async def test_list_collections(client, mock_response):
    """Test listing collections."""
    response_data = {
        "collections": [
            {
                "name": "collection-1",
                "dimension": 128,
                "num_points": 100,
                "created_at": 1234567890,
            },
            {
                "name": "collection-2",
                "dimension": 256,
                "num_points": 50,
                "created_at": 1234567891,
            },
        ]
    }
    
    with patch.object(client.client, "request", new_callable=AsyncMock) as mock_request:
        mock_request.return_value = mock_response(200, response_data)
        
        collections = await client.list_collections()
        
        assert len(collections) == 2
        assert collections[0].name == "collection-1"
        assert collections[0].dimension == 128
        assert collections[0].num_points == 100
        assert collections[1].name == "collection-2"
        assert collections[1].dimension == 256


@pytest.mark.asyncio
async def test_delete_collection(client, mock_response):
    """Test deleting a collection."""
    with patch.object(client.client, "request", new_callable=AsyncMock) as mock_request:
        mock_request.return_value = mock_response(204)
        
        await client.delete_collection("test-collection")
        
        mock_request.assert_called_once()
        call_args = mock_request.call_args
        assert call_args.kwargs["method"] == "DELETE"
        assert "/api/v1/collections/test-collection" in call_args.kwargs["url"]


@pytest.mark.asyncio
async def test_delete_collection_not_found(client, mock_response):
    """Test deleting a non-existent collection."""
    error_data = {
        "error": "collection_not_found",
        "message": "collection 'test-collection' not found",
        "code": 404,
    }
    
    with patch.object(client.client, "request", new_callable=AsyncMock) as mock_request:
        mock_request.return_value = mock_response(404, error_data)
        
        with pytest.raises(CollectionNotFoundError) as exc_info:
            await client.delete_collection("test-collection")
        
        assert "test-collection" in str(exc_info.value)


@pytest.mark.asyncio
async def test_upsert_points_single_batch(client, mock_response):
    """Test upserting points in a single batch."""
    points = [
        Point(id="1", vector=[0.1, 0.2, 0.3], metadata={"text": "hello"}),
        Point(id="2", vector=[0.4, 0.5, 0.6], metadata={"text": "world"}),
    ]
    
    response_data = {
        "upserted": 2,
        "failed": [],
    }
    
    with patch.object(client.client, "request", new_callable=AsyncMock) as mock_request:
        mock_request.return_value = mock_response(200, response_data)
        
        result = await client.upsert_points("test-collection", points)
        
        assert result.upserted == 2
        assert len(result.failed) == 0
        
        # Verify request payload
        call_args = mock_request.call_args
        assert call_args.kwargs["json"]["points"] == [
            {"id": "1", "vector": [0.1, 0.2, 0.3], "metadata": {"text": "hello"}},
            {"id": "2", "vector": [0.4, 0.5, 0.6], "metadata": {"text": "world"}},
        ]


@pytest.mark.asyncio
async def test_upsert_points_large_batch(client, mock_response):
    """Test upserting points with automatic batching."""
    # Create 2500 points (should be split into 3 batches: 1000, 1000, 500)
    points = [
        Point(id=str(i), vector=[0.1, 0.2, 0.3], metadata={"index": i})
        for i in range(2500)
    ]
    
    response_data = {
        "upserted": 1000,
        "failed": [],
    }
    
    with patch.object(client.client, "request", new_callable=AsyncMock) as mock_request:
        mock_request.return_value = mock_response(200, response_data)
        
        result = await client.upsert_points("test-collection", points)
        
        # Should have been called 3 times (1000 + 1000 + 500)
        assert mock_request.call_count == 3
        assert result.upserted == 3000  # 3 batches * 1000
        assert len(result.failed) == 0


@pytest.mark.asyncio
async def test_upsert_points_empty_list(client):
    """Test upserting empty list."""
    result = await client.upsert_points("test-collection", [])
    
    assert result.upserted == 0
    assert len(result.failed) == 0


@pytest.mark.asyncio
async def test_upsert_points_with_failures(client, mock_response):
    """Test upserting points with some failures."""
    points = [
        Point(id="1", vector=[0.1, 0.2, 0.3], metadata={}),
        Point(id="2", vector=[0.4, 0.5, 0.6], metadata={}),
    ]
    
    response_data = {
        "upserted": 1,
        "failed": [
            {"id": "2", "reason": "dimension mismatch: expected 3, got 3"},
        ],
    }
    
    with patch.object(client.client, "request", new_callable=AsyncMock) as mock_request:
        mock_request.return_value = mock_response(200, response_data)
        
        result = await client.upsert_points("test-collection", points)
        
        assert result.upserted == 1
        assert len(result.failed) == 1
        assert result.failed[0]["id"] == "2"


@pytest.mark.asyncio
async def test_delete_points(client, mock_response):
    """Test deleting points."""
    ids = ["1", "2", "3"]
    
    with patch.object(client.client, "request", new_callable=AsyncMock) as mock_request:
        mock_request.return_value = mock_response(200)
        
        await client.delete_points("test-collection", ids)
        
        call_args = mock_request.call_args
        assert call_args.kwargs["json"] == {"ids": ids}


@pytest.mark.asyncio
async def test_delete_points_empty_list(client):
    """Test deleting points with empty list."""
    with pytest.raises(InvalidPayloadError) as exc_info:
        await client.delete_points("test-collection", [])
    
    assert "cannot be empty" in str(exc_info.value).lower()


@pytest.mark.asyncio
async def test_search(client, mock_response):
    """Test searching for similar vectors."""
    response_data = {
        "results": [
            {
                "id": "1",
                "score": 0.95,
                "metadata": {"text": "hello"},
            },
            {
                "id": "2",
                "score": 0.85,
                "metadata": {"text": "world"},
            },
        ],
        "took_ms": 10,
    }
    
    with patch.object(client.client, "request", new_callable=AsyncMock) as mock_request:
        mock_request.return_value = mock_response(200, response_data)
        
        results = await client.search(
            collection="test-collection",
            vector=[0.1, 0.2, 0.3],
            limit=10,
        )
        
        assert len(results) == 2
        assert results[0].id == "1"
        assert results[0].score == 0.95
        assert results[0].metadata == {"text": "hello"}
        
        # Verify request payload
        call_args = mock_request.call_args
        assert call_args.kwargs["json"]["vector"] == [0.1, 0.2, 0.3]
        assert call_args.kwargs["json"]["limit"] == 10


@pytest.mark.asyncio
async def test_search_with_filter(client, mock_response):
    """Test searching with metadata filter."""
    response_data = {
        "results": [
            {
                "id": "1",
                "score": 0.95,
                "metadata": {"category": "A"},
            },
        ],
        "took_ms": 5,
    }
    
    with patch.object(client.client, "request", new_callable=AsyncMock) as mock_request:
        mock_request.return_value = mock_response(200, response_data)
        
        results = await client.search(
            collection="test-collection",
            vector=[0.1, 0.2, 0.3],
            limit=10,
            filter={"category": "A"},
        )
        
        assert len(results) == 1
        
        # Verify filter was included in request
        call_args = mock_request.call_args
        assert call_args.kwargs["json"]["filter"] == {"category": "A"}


@pytest.mark.asyncio
async def test_retry_on_server_error(client, mock_response):
    """Test automatic retry on server errors."""
    # First two calls return 500, third succeeds
    responses = [
        mock_response(500),
        mock_response(500),
        mock_response(200, {"collections": []}),
    ]
    
    with patch.object(client.client, "request", new_callable=AsyncMock) as mock_request:
        mock_request.side_effect = responses
        
        with patch("asyncio.sleep", new_callable=AsyncMock) as mock_sleep:
            collections = await client.list_collections()
            
            # Should have retried twice (3 total calls)
            assert mock_request.call_count == 3
            # Should have slept twice (exponential backoff)
            assert mock_sleep.call_count == 2
            assert len(collections) == 0


@pytest.mark.asyncio
async def test_retry_on_timeout(client, mock_response):
    """Test automatic retry on timeout."""
    import httpx
    
    # First call times out, second succeeds
    timeout_error = httpx.TimeoutException("Request timeout")
    
    with patch.object(client.client, "request", new_callable=AsyncMock) as mock_request:
        mock_request.side_effect = [
            timeout_error,
            mock_response(200, {"collections": []}),
        ]
        
        with patch("asyncio.sleep", new_callable=AsyncMock):
            collections = await client.list_collections()
            
            assert mock_request.call_count == 2
            assert len(collections) == 0


@pytest.mark.asyncio
async def test_no_retry_on_client_error(client, mock_response):
    """Test that client errors (4xx) are not retried."""
    error_data = {
        "error": "collection_not_found",
        "message": "collection 'test' not found",
        "code": 404,
    }
    
    with patch.object(client.client, "request", new_callable=AsyncMock) as mock_request:
        mock_request.return_value = mock_response(404, error_data)
        
        with pytest.raises(CollectionNotFoundError):
            await client.delete_collection("test")
        
        # Should not retry on 4xx errors
        assert mock_request.call_count == 1


@pytest.mark.asyncio
async def test_context_manager(client, mock_response):
    """Test using client as async context manager."""
    # Create a new client for this test
    new_client = VectorDBClient(base_url="http://localhost:3000")
    with patch.object(new_client.client, "aclose", new_callable=AsyncMock) as mock_close:
        async with new_client as ctx_client:
            assert isinstance(ctx_client, VectorDBClient)
        
        # Should close on exit
        mock_close.assert_called_once()


@pytest.mark.asyncio
async def test_close_method(client):
    """Test explicit close method."""
    with patch.object(client.client, "aclose", new_callable=AsyncMock) as mock_close:
        await client.close()
        mock_close.assert_called_once()

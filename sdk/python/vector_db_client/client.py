"""VectorDB client for Python."""

import asyncio
import time
import json
from typing import List, Optional, Dict, Any
from urllib.parse import urljoin

import httpx
import structlog

from .models import (
    Point,
    Collection,
    CollectionListItem,
    SearchResult,
    UpsertResult,
    DistanceMetric,
)
from .exceptions import (
    VectorDBError,
    CollectionNotFoundError,
    CollectionAlreadyExistsError,
    InvalidDimensionError,
    InvalidPayloadError,
    InternalError,
    ConnectionError,
)

# Configure structlog
logger = structlog.get_logger()


class VectorDBClient:
    """Client for interacting with FerresDB vector database."""
    
    # Maximum batch size for upsert operations
    MAX_BATCH_SIZE = 1000
    
    def __init__(self, base_url: str, timeout: int = 30):
        """
        Initialize the VectorDB client.
        
        Args:
            base_url: Base URL of the FerresDB server (e.g., "http://localhost:3000")
            timeout: Request timeout in seconds (default: 30)
        """
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout
        self.client = httpx.AsyncClient(
            base_url=self.base_url,
            timeout=timeout,
        )
        self.logger = logger.bind(client="VectorDBClient")
    
    async def __aenter__(self):
        """Async context manager entry."""
        return self
    
    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Async context manager exit."""
        await self.close()
    
    async def close(self):
        """Close the HTTP client."""
        await self.client.aclose()
    
    async def _handle_error(self, response: httpx.Response) -> None:
        """Handle API errors and raise appropriate exceptions."""
        try:
            error_data = await response.json()
            error_type = error_data.get("error", "unknown")
            message = error_data.get("message", "Unknown error")
            code = error_data.get("code", response.status_code)
        except Exception:
            error_type = "unknown"
            message = response.text or f"HTTP {response.status_code}"
            code = response.status_code
        
        error_map = {
            "collection_not_found": CollectionNotFoundError,
            "collection_already_exists": CollectionAlreadyExistsError,
            "invalid_dimension": InvalidDimensionError,
            "invalid_payload": InvalidPayloadError,
            "internal_error": InternalError,
        }
        
        error_class = error_map.get(error_type, VectorDBError)
        
        if error_class == CollectionNotFoundError:
            # Try to extract collection name from message
            if "collection '" in message:
                collection_name = message.split("collection '")[1].split("'")[0]
                raise CollectionNotFoundError(collection_name)
        
        if error_class == CollectionAlreadyExistsError:
            if "collection '" in message:
                collection_name = message.split("collection '")[1].split("'")[0]
                raise CollectionAlreadyExistsError(collection_name)
        
        raise error_class(message)
    
    async def _request(
        self,
        method: str,
        path: str,
        json_data: Optional[dict] = None,
        max_retries: int = 3,
    ) -> httpx.Response:
        """
        Make an HTTP request with automatic retry and exponential backoff.
        
        Args:
            method: HTTP method (GET, POST, DELETE, etc.)
            path: API path (e.g., "/api/v1/collections")
            json_data: Optional JSON payload
            max_retries: Maximum number of retry attempts
        
        Returns:
            HTTP response
        
        Raises:
            ConnectionError: If connection fails after retries
            VectorDBError: For API errors
        """
        url = urljoin(self.base_url + "/", path.lstrip("/"))
        
        for attempt in range(max_retries + 1):
            try:
                self.logger.debug(
                    "making_request",
                    method=method,
                    url=url,
                    attempt=attempt + 1,
                )
                
                response = await self.client.request(
                    method=method,
                    url=url,
                    json=json_data,
                )
                
                # Success
                if response.is_success:
                    self.logger.debug(
                        "request_success",
                        method=method,
                        url=url,
                        status_code=response.status_code,
                    )
                    return response
                
                # Client errors (4xx) should not be retried
                if 400 <= response.status_code < 500:
                    await self._handle_error(response)
                
                # Server errors (5xx) should be retried
                if response.status_code >= 500:
                    if attempt < max_retries:
                        wait_time = 2 ** attempt  # Exponential backoff
                        self.logger.warning(
                            "server_error_retrying",
                            status_code=response.status_code,
                            attempt=attempt + 1,
                            wait_time=wait_time,
                        )
                        await asyncio.sleep(wait_time)
                        continue
                    else:
                        await self._handle_error(response)
                
                # Other errors
                await self._handle_error(response)
                
            except httpx.TimeoutException as e:
                if attempt < max_retries:
                    wait_time = 2 ** attempt
                    self.logger.warning(
                        "timeout_retrying",
                        attempt=attempt + 1,
                        wait_time=wait_time,
                    )
                    await asyncio.sleep(wait_time)
                    continue
                else:
                    raise ConnectionError(f"Request timeout after {max_retries + 1} attempts: {e}")
            
            except httpx.RequestError as e:
                if attempt < max_retries:
                    wait_time = 2 ** attempt
                    self.logger.warning(
                        "connection_error_retrying",
                        error=str(e),
                        attempt=attempt + 1,
                        wait_time=wait_time,
                    )
                    await asyncio.sleep(wait_time)
                    continue
                else:
                    raise ConnectionError(f"Connection error after {max_retries + 1} attempts: {e}")
        
        raise ConnectionError(f"Request failed after {max_retries + 1} attempts")
    
    async def create_collection(
        self,
        name: str,
        dimension: int,
        distance: DistanceMetric,
    ) -> Collection:
        """
        Create a new collection.
        
        Args:
            name: Collection name (alphanumeric, hyphens, underscores only)
            dimension: Vector dimension (1-4096)
            distance: Distance metric to use
        
        Returns:
            Created collection
        
        Raises:
            CollectionAlreadyExistsError: If collection already exists
            InvalidDimensionError: If dimension is invalid
            InvalidPayloadError: If payload is invalid
        """
        payload = {
            "name": name,
            "dimension": dimension,
            "distance": distance.value,
        }
        
        response = await self._request("POST", "/api/v1/collections", json_data=payload)
        data = await response.json()
        
        return Collection(
            name=data["name"],
            dimension=data["dimension"],
            distance=DistanceMetric(data["distance"]),
            created_at=data.get("created_at"),
        )
    
    async def list_collections(self) -> List[CollectionListItem]:
        """
        List all collections.
        
        Returns:
            List of collections
        """
        response = await self._request("GET", "/api/v1/collections")
        data = await response.json()
        
        return [
            CollectionListItem.from_dict(item)
            for item in data["collections"]
        ]
    
    async def delete_collection(self, name: str) -> None:
        """
        Delete a collection.
        
        Args:
            name: Collection name
        
        Raises:
            CollectionNotFoundError: If collection doesn't exist
        """
        await self._request("DELETE", f"/api/v1/collections/{name}")
    
    async def upsert_points(
        self,
        collection: str,
        points: List[Point],
    ) -> UpsertResult:
        """
        Upsert points into a collection.
        
        Automatically batches points if the list exceeds MAX_BATCH_SIZE (1000).
        
        Args:
            collection: Collection name
            points: List of points to upsert
        
        Returns:
            Upsert result with upserted count and failed points
        
        Raises:
            CollectionNotFoundError: If collection doesn't exist
            InvalidDimensionError: If vector dimensions don't match
        """
        if not points:
            return UpsertResult(upserted=0, failed=[])
        
        # Batch points if necessary
        if len(points) <= self.MAX_BATCH_SIZE:
            return await self._upsert_batch(collection, points)
        
        # Split into batches
        total_upserted = 0
        all_failed = []
        
        for i in range(0, len(points), self.MAX_BATCH_SIZE):
            batch = points[i:i + self.MAX_BATCH_SIZE]
            result = await self._upsert_batch(collection, batch)
            total_upserted += result.upserted
            all_failed.extend(result.failed)
        
        return UpsertResult(upserted=total_upserted, failed=all_failed)
    
    async def _upsert_batch(
        self,
        collection: str,
        points: List[Point],
    ) -> UpsertResult:
        """Upsert a single batch of points."""
        payload = {
            "points": [point.to_dict() for point in points],
        }
        
        response = await self._request(
            "POST",
            f"/api/v1/collections/{collection}/points",
            json_data=payload,
        )
        data = await response.json()
        
        return UpsertResult.from_dict(data)
    
    async def delete_points(
        self,
        collection: str,
        ids: List[str],
    ) -> None:
        """
        Delete points from a collection by IDs.
        
        Args:
            collection: Collection name
            ids: List of point IDs to delete
        
        Raises:
            CollectionNotFoundError: If collection doesn't exist
            InvalidPayloadError: If ids list is empty
        """
        if not ids:
            raise InvalidPayloadError("ids cannot be empty")
        
        payload = {"ids": ids}
        
        await self._request(
            "DELETE",
            f"/api/v1/collections/{collection}/points",
            json_data=payload,
        )
    
    async def search(
        self,
        collection: str,
        vector: List[float],
        limit: int = 10,
        filter: Optional[Dict[str, Any]] = None,
    ) -> List[SearchResult]:
        """
        Search for similar vectors in a collection.
        
        Args:
            collection: Collection name
            vector: Query vector
            limit: Maximum number of results (default: 10)
            filter: Optional metadata filter (equality matching)
        
        Returns:
            List of search results sorted by similarity
        
        Raises:
            CollectionNotFoundError: If collection doesn't exist
            InvalidDimensionError: If vector dimension doesn't match collection
        """
        payload = {
            "vector": vector,
            "limit": limit,
        }
        
        if filter is not None:
            payload["filter"] = filter
        
        response = await self._request(
            "POST",
            f"/api/v1/collections/{collection}/search",
            json_data=payload,
        )
        data = await response.json()
        
        return [
            SearchResult.from_dict(result)
            for result in data["results"]
        ]

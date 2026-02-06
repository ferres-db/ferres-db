# FerresDB Python SDK

Python SDK for interacting with FerresDB vector database.

## Installation

```bash
pip install ferres-db-python
```

Or install from source:

```bash
cd sdk/python
pip install -e .
```

## Quick Start

```python
import asyncio
from vector_db_client import VectorDBClient, Point, DistanceMetric

async def main():
    # Create client
    async with VectorDBClient(base_url="http://localhost:3000") as client:
        # Create a collection
        collection = await client.create_collection(
            name="my-collection",
            dimension=128,
            distance=DistanceMetric.COSINE,
        )

        # Upsert points
        points = [
            Point(id="1", vector=[0.1, 0.2, 0.3], metadata={"text": "hello"}),
            Point(id="2", vector=[0.4, 0.5, 0.6], metadata={"text": "world"}),
        ]
        result = await client.upsert_points("my-collection", points)
        print(f"Upserted {result.upserted} points")

        # Search for similar vectors
        results = await client.search(
            collection="my-collection",
            vector=[0.1, 0.2, 0.3],
            limit=10,
        )
        for result in results:
            print(f"ID: {result.id}, Score: {result.score}")

asyncio.run(main())
```

## Features

- **Type hints**: Full type annotations for better IDE support
- **Automatic retry**: Exponential backoff for transient failures
- **Structured logging**: Uses structlog for better observability
- **Automatic batching**: Large upsert operations are automatically split into batches
- **Async/await**: Built on httpx for async operations

## API Reference

### VectorDBClient

#### `__init__(base_url: str, timeout: int = 30)`

Initialize the client.

- `base_url`: Base URL of the FerresDB server (e.g., "http://localhost:3000")
- `timeout`: Request timeout in seconds

#### `create_collection(name: str, dimension: int, distance: DistanceMetric) -> Collection`

Create a new collection.

#### `list_collections() -> List[CollectionListItem]`

List all collections.

#### `delete_collection(name: str) -> None`

Delete a collection.

#### `upsert_points(collection: str, points: List[Point]) -> UpsertResult`

Upsert points into a collection. Automatically batches if more than 1000 points.

#### `delete_points(collection: str, ids: List[str]) -> None`

Delete points by IDs.

#### `search(collection: str, vector: List[float], limit: int = 10, filter: dict = None) -> List[SearchResult]`

Search for similar vectors.

## Models

### Point

- `id: str`: Point identifier
- `vector: List[float]`: Vector coordinates
- `metadata: Dict[str, Any]`: Arbitrary metadata

### Collection

- `name: str`: Collection name
- `dimension: int`: Vector dimension
- `distance: DistanceMetric`: Distance metric

### SearchResult

- `id: str`: Point ID
- `score: float`: Similarity score
- `metadata: Dict[str, Any]`: Point metadata

## Exceptions

- `VectorDBError`: Base exception
- `CollectionNotFoundError`: Collection not found (404)
- `CollectionAlreadyExistsError`: Collection already exists (409)
- `InvalidDimensionError`: Invalid dimension (400)
- `InvalidPayloadError`: Invalid payload (400)
- `InternalError`: Internal server error (500)
- `ConnectionError`: Connection error

## Development

Install development dependencies:

```bash
pip install -e ".[dev]"
```

Run tests:

```bash
pytest
```

Format code:

```bash
black vector_db_client tests
```

Type checking:

```bash
mypy vector_db_client
```

"""VectorDB Python SDK."""

from .client import VectorDBClient
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

__all__ = [
    "VectorDBClient",
    "Point",
    "Collection",
    "CollectionListItem",
    "SearchResult",
    "UpsertResult",
    "DistanceMetric",
    "VectorDBError",
    "CollectionNotFoundError",
    "CollectionAlreadyExistsError",
    "InvalidDimensionError",
    "InvalidPayloadError",
    "InternalError",
    "ConnectionError",
]

__version__ = "0.1.0"

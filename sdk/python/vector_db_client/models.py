"""Models for the VectorDB client."""

from dataclasses import dataclass
from typing import List, Optional, Dict, Any
from enum import Enum
import json


class DistanceMetric(str, Enum):
    """Distance metrics supported by FerresDB."""
    
    COSINE = "Cosine"
    DOT_PRODUCT = "DotProduct"
    EUCLIDEAN = "Euclidean"


@dataclass
class Point:
    """Represents a vector point with ID, vector, and metadata."""
    
    id: str
    vector: List[float]
    metadata: Dict[str, Any]
    
    def to_dict(self) -> dict:
        """Convert point to dictionary for API requests."""
        return {
            "id": self.id,
            "vector": self.vector,
            "metadata": self.metadata,
        }
    
    @classmethod
    def from_dict(cls, data: dict) -> "Point":
        """Create a Point from a dictionary."""
        return cls(
            id=data["id"],
            vector=data["vector"],
            metadata=data.get("metadata", {}),
        )


@dataclass
class Collection:
    """Represents a collection configuration."""
    
    name: str
    dimension: int
    distance: DistanceMetric
    created_at: Optional[int] = None
    
    def to_dict(self) -> dict:
        """Convert collection to dictionary for API requests."""
        return {
            "name": self.name,
            "dimension": self.dimension,
            "distance": self.distance.value,
        }


@dataclass
class CollectionListItem:
    """Represents a collection in a list."""
    
    name: str
    dimension: int
    num_points: int
    created_at: int
    
    @classmethod
    def from_dict(cls, data: dict) -> "CollectionListItem":
        """Create a CollectionListItem from a dictionary."""
        return cls(
            name=data["name"],
            dimension=data["dimension"],
            num_points=data["num_points"],
            created_at=data["created_at"],
        )


@dataclass
class SearchResult:
    """Represents a search result."""
    
    id: str
    score: float
    metadata: Dict[str, Any]
    
    @classmethod
    def from_dict(cls, data: dict) -> "SearchResult":
        """Create a SearchResult from a dictionary."""
        return cls(
            id=data["id"],
            score=data["score"],
            metadata=data["metadata"],
        )


@dataclass
class UpsertResult:
    """Result of an upsert operation."""
    
    upserted: int
    failed: List[Dict[str, str]]
    
    @classmethod
    def from_dict(cls, data: dict) -> "UpsertResult":
        """Create an UpsertResult from a dictionary."""
        return cls(
            upserted=data["upserted"],
            failed=data.get("failed", []),
        )

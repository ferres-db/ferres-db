"""Exceptions for the VectorDB client."""


class VectorDBError(Exception):
    """Base exception for all VectorDB errors."""
    
    def __init__(self, message: str, code: int = None):
        self.message = message
        self.code = code
        super().__init__(self.message)


class CollectionNotFoundError(VectorDBError):
    """Raised when a collection is not found."""
    
    def __init__(self, collection_name: str):
        message = f"collection '{collection_name}' not found"
        super().__init__(message, code=404)


class CollectionAlreadyExistsError(VectorDBError):
    """Raised when trying to create a collection that already exists."""
    
    def __init__(self, collection_name: str):
        message = f"collection '{collection_name}' already exists"
        super().__init__(message, code=409)


class InvalidDimensionError(VectorDBError):
    """Raised when there's a dimension mismatch or invalid dimension."""
    
    def __init__(self, message: str):
        super().__init__(message, code=400)


class InvalidPayloadError(VectorDBError):
    """Raised when the request payload is invalid."""
    
    def __init__(self, message: str):
        super().__init__(message, code=400)


class InternalError(VectorDBError):
    """Raised when an internal server error occurs."""
    
    def __init__(self, message: str):
        super().__init__(message, code=500)


class ConnectionError(VectorDBError):
    """Raised when there's a connection error."""
    
    def __init__(self, message: str):
        super().__init__(message, code=None)

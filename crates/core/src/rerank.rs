//! # Re-ranking — Cross-Encoder via ONNX Runtime
//!
//! Optional re-ranking of HNSW candidates using a Cross-Encoder model (e.g. BGE-Reranker-style).
//! When the `rerank` feature is enabled, the crate `ort` (ONNX Runtime) is used to load and run
//! a model that scores (query_embedding, document_embedding) pairs.
//!
//! The model is expected to accept one input of shape `[1, 2*dimension]` (query and document
//! vectors concatenated) and produce one output of shape `[1]` (relevance score; higher = more relevant).

use crate::error::FerresError;

/// Trait for a re-ranker that scores (query, document) vector pairs.
///
/// Used by [`crate::collection::Collection::search_with_rerank`] to re-score HNSW candidates.
/// Implementations typically load an ONNX model (e.g. Cross-Encoder) and run inference.
pub trait Reranker: Send + Sync {
    /// Returns the vector dimension expected by this reranker (query and doc must match).
    fn dimension(&self) -> usize;

    /// Scores a single (query, document) pair. Higher score = more relevant.
    ///
    /// # Errors
    /// - Dimension mismatch if `query.len() != dimension()` or `doc.len() != dimension()`.
    /// - Inference errors if the model fails.
    fn score(&self, query: &[f32], doc: &[f32]) -> Result<f32, FerresError>;
}

#[cfg(feature = "rerank")]
mod ort_impl {
    use std::path::Path;
    use std::sync::Mutex;

    use ort::session::Session;
    use ort::value::Tensor;
    use tracing::debug;

    use super::Reranker;
    use crate::error::FerresError;

    /// Cross-Encoder re-ranker backed by ONNX Runtime.
    ///
    /// Loads an ONNX model that accepts one input tensor of shape `[1, 2*dimension]`
    /// (query and document vectors concatenated) and returns one output tensor of shape `[1]`
    /// (relevance score). Input/output names can be specified when loading; otherwise
    /// the first input and first output from the model metadata are used.
    pub struct CrossEncoderOrt {
        session: Mutex<Session>,
        dimension: usize,
        input_name: String,
        output_name: String,
    }

    impl CrossEncoderOrt {
        /// Loads a Cross-Encoder model from an ONNX file.
        ///
        /// - `path`: path to the .onnx file.
        /// - `dimension`: vector dimension (query and doc must both have this length).
        /// - `input_name`: optional name of the input tensor (default: first input from model).
        /// - `output_name`: optional name of the output tensor (default: first output from model).
        pub fn load(
            path: impl AsRef<Path>,
            dimension: usize,
            input_name: Option<String>,
            output_name: Option<String>,
        ) -> Result<Self, FerresError> {
            let path = path.as_ref();
            let session = Session::builder()
                .map_err(|e| FerresError::Storage(format!("ONNX session builder: {e}")))?
                .commit_from_file(path)
                .map_err(|e| FerresError::Storage(format!("load ONNX model {}: {e}", path.display())))?;

            // Use provided names or defaults; the ONNX model must have input/output with these names.
            let input_name = input_name.unwrap_or_else(|| "input".to_string());
            let output_name = output_name.unwrap_or_else(|| "output".to_string());

            debug!(
                path = %path.display(),
                dimension,
                input = %input_name,
                output = %output_name,
                "cross-encoder reranker loaded"
            );

            Ok(Self {
                session: Mutex::new(session),
                dimension,
                input_name,
                output_name,
            })
        }
    }

    impl Reranker for CrossEncoderOrt {
        fn dimension(&self) -> usize {
            self.dimension
        }

        fn score(&self, query: &[f32], doc: &[f32]) -> Result<f32, FerresError> {
            if query.len() != self.dimension {
                return Err(FerresError::InvalidVector {
                    reason: format!(
                        "query dimension {} does not match reranker dimension {}",
                        query.len(),
                        self.dimension
                    ),
                });
            }
            if doc.len() != self.dimension {
                return Err(FerresError::InvalidVector {
                    reason: format!(
                        "doc dimension {} does not match reranker dimension {}",
                        doc.len(),
                        self.dimension
                    ),
                });
            }

            let mut input_vec = Vec::with_capacity(2 * self.dimension);
            input_vec.extend_from_slice(query);
            input_vec.extend_from_slice(doc);

            let shape = [1_usize, 2 * self.dimension];
            let input_tensor = Tensor::from_array((shape, input_vec))
                .map_err(|e| FerresError::Storage(format!("reranker input tensor: {e}")))?;

            let mut guard = self
                .session
                .lock()
                .map_err(|_| FerresError::Storage("reranker session lock poisoned".to_string()))?;

            let outputs = guard
                .run(ort::inputs![self.input_name.as_str() => input_tensor])
                .map_err(|e| FerresError::Storage(format!("reranker inference: {e}")))?;

            let output = outputs
                .get(self.output_name.as_str())
                .ok_or_else(|| FerresError::Storage("reranker output not found".to_string()))?;

            let arr = output
                .try_extract_array::<f32>()
                .map_err(|e| FerresError::Storage(format!("reranker output extract: {e}")))?;

            let score = *arr
                .iter()
                .next()
                .ok_or_else(|| FerresError::Storage("reranker output empty".to_string()))?;

            Ok(score)
        }
    }
}

#[cfg(feature = "rerank")]
pub use ort_impl::CrossEncoderOrt;

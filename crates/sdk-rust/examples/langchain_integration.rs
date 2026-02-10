//! Exemplo: FerresDB como VectorStore para pipelines RAG (LangChain / LlamaIndex).
//!
//! Mostra como usar o SDK em modo "VectorStore": garantir coleção, inserir
//! vetores com metadados (ex. texto) e consultar por similaridade.
//!
//! A API de pontos também suporta: `namespace` (multitenancy), `vectors` (multi-vector por ponto),
//! `ttl` (expiração em segundos) e filtros de metadata na busca. Inserção via `upsert_points`
//! com body `{ "points": [{ "id", "vector", "metadata?", "namespace?", "ttl?", "vectors?" }] }`;
//! busca via `search_points` com `query_vector`, `limit` e opcionalmente `filter`, `namespace`, `vector_field`.
//!
//! Requer servidor FerresDB rodando em `http://localhost:8080`:
//!
//! ```bash
//! cargo run --example langchain_integration
//! ```

use ferres_db_sdk::integrations::{FerresDbVectorStore, VectorStore};
use ferres_db_sdk::FerresDbClient;

const COLLECTION: &str = "rag_docs";
const DIM: u32 = 4;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base_url = std::env::var("FERRESDB_URL").unwrap_or_else(|_| "http://localhost:8080".into());
    let client = FerresDbClient::new(&base_url);

    // Cria o VectorStore e a coleção se não existir (interface LangChain/LlamaIndex-style)
    let store = FerresDbVectorStore::ensure_collection(
        client.clone(),
        COLLECTION,
        DIM,
        "Cosine",
    )
    .await?;
    println!("VectorStore pronto: coleção '{}' (dim={})", COLLECTION, DIM);

    // Ingestão: vetores + metadados (ex. texto para RAG)
    let ids = ["doc-1", "doc-2", "doc-3"]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
    let vectors: Vec<Vec<f32>> = vec![
        vec![0.1, 0.2, 0.3, 0.4],
        vec![0.2, 0.3, 0.4, 0.5],
        vec![0.5, 0.4, 0.3, 0.2],
    ];
    let metadatas: Vec<serde_json::Value> = vec![
        serde_json::json!({ "text": "Primeiro documento sobre deploy em produção." }),
        serde_json::json!({ "text": "Segundo documento sobre monitoramento e logs." }),
        serde_json::json!({ "text": "Terceiro documento sobre CI/CD e pipelines." }),
    ];
    let n = store
        .add_vectors(&ids, &vectors, Some(&metadatas))
        .await?;
    println!("Inseridos {} documentos.", n);

    // Consulta por similaridade (como em LangChain VectorStore.similarity_search)
    let query = vec![0.15, 0.25, 0.35, 0.45f32];
    let docs = store.similarity_search(&query, 2).await?;
    println!("similarity_search (top 2):");
    for d in &docs {
        println!("  id={} metadata={}", d.id, d.metadata);
    }

    // Com scores (similarity_search_with_score)
    let with_scores = store.similarity_search_with_score(&query, 2).await?;
    println!("similarity_search_with_score (top 2):");
    for (d, score) in &with_scores {
        println!("  id={} score={:.4} metadata={}", d.id, score, d.metadata);
    }

    Ok(())
}

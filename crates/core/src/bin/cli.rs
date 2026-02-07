//! # FerresDB CLI
//!
//! Interface de linha de comando para gerenciar coleções vetoriais.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use ferres_db_core::{
    CollectionConfig, DistanceMetric, FerresError, Point, VectorDB,
};

use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "ferres-db")]
#[command(about = "FerresDB - Vector Database CLI", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Caminho do diretório de storage (padrão: ./data)
    #[arg(short, long, global = true)]
    storage: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Inicializa um novo diretório de storage
    Init {
        /// Caminho do diretório de storage
        path: PathBuf,
    },
    /// Cria uma nova coleção
    CreateCollection {
        /// Nome da coleção
        name: String,
        /// Dimensão dos vetores
        #[arg(long)]
        dimension: usize,
        /// Métrica de distância (cosine, euclidean, dotproduct)
        #[arg(long, default_value = "cosine")]
        distance: String,
    },
    /// Insere pontos de um arquivo JSONL
    Insert {
        /// Nome da coleção
        collection: String,
        /// Arquivo JSONL com pontos (formato: {"id": "...", "vector": [...], "metadata": {...}})
        #[arg(long)]
        from_file: PathBuf,
    },
    /// Busca pontos similares
    Search {
        /// Nome da coleção
        collection: String,
        /// Vetor de consulta (valores separados por vírgula)
        #[arg(long)]
        vector: String,
        /// Número de resultados
        #[arg(long, default_value = "10")]
        limit: usize,
    },
    /// Exibe estatísticas de uma coleção
    Stats {
        /// Nome da coleção
        collection: String,
    },
    /// Deleta uma coleção
    Delete {
        /// Nome da coleção
        collection: String,
    },
}

fn main() {
    // Inicializa tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    // Determina o caminho de storage
    let storage_path = cli.storage.unwrap_or_else(|| PathBuf::from("./data"));

    let result = match cli.command {
        Commands::Init { path } => init_storage(&path),
        Commands::CreateCollection {
            name,
            dimension,
            distance,
        } => create_collection(&storage_path, name, dimension, distance),
        Commands::Insert {
            collection,
            from_file,
        } => insert_points(&storage_path, collection, from_file),
        Commands::Search {
            collection,
            vector,
            limit,
        } => search_points(&storage_path, collection, vector, limit),
        Commands::Stats { collection } => show_stats(&storage_path, collection),
        Commands::Delete { collection } => delete_collection(&storage_path, collection),
    };

    if let Err(e) = result {
        error!(error = %e, "command failed");
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn init_storage(path: &PathBuf) -> Result<(), FerresError> {
    info!(path = %path.display(), "initializing storage directory");
    
    fs::create_dir_all(path).map_err(|e| {
        FerresError::Storage(format!(
            "failed to create storage directory {}: {e}",
            path.display()
        ))
    })?;

    println!("✓ Storage directory initialized at: {}", path.display());
    Ok(())
}

fn create_collection(
    storage_path: &PathBuf,
    name: String,
    dimension: usize,
    distance_str: String,
) -> Result<(), FerresError> {
    let distance = parse_distance(&distance_str)?;

    let mut db = VectorDB::new(storage_path.clone())?;

    let config = CollectionConfig {
        name: name.clone(),
        dimension,
        distance,
        hnsw: Default::default(),
        search_cache_size: 100, // Cache padrão de 100 queries
        enable_bm25: false,
        bm25_text_field: "text".to_string(),
        quantization: Default::default(),
        tiered_storage: Default::default(),
    };

    db.create_collection(config)?;

    println!("✓ Collection '{}' created successfully", name);
    println!("  Dimension: {}", dimension);
    println!("  Distance metric: {:?}", distance);
    Ok(())
}

fn insert_points(
    storage_path: &PathBuf,
    collection: String,
    file_path: PathBuf,
) -> Result<(), FerresError> {
    info!(
        collection = %collection,
        file = %file_path.display(),
        "inserting points from file"
    );

    let mut db = VectorDB::new(storage_path.clone())?;

    // Lê o arquivo JSONL
    let file = fs::File::open(&file_path).map_err(|e| {
        FerresError::Storage(format!("failed to open file {}: {e}", file_path.display()))
    })?;

    let reader = BufReader::new(file);
    let mut points = Vec::new();
    let mut line_num = 0;

    for line in reader.lines() {
        line_num += 1;
        let line = line.map_err(|e| {
            FerresError::Storage(format!(
                "failed to read line {} from {}: {e}",
                line_num,
                file_path.display()
            ))
        })?;

        if line.trim().is_empty() {
            continue;
        }

        // Parse do JSON
        let json: serde_json::Value = serde_json::from_str(&line).map_err(|e| {
            FerresError::Storage(format!(
                "invalid JSON at line {} in {}: {e}",
                line_num,
                file_path.display()
            ))
        })?;

        // Extrai campos
        let id = json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                FerresError::Storage(format!(
                    "missing 'id' field at line {} in {}",
                    line_num,
                    file_path.display()
                ))
            })?
            .to_string();

        let vector = json
            .get("vector")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                FerresError::Storage(format!(
                    "missing or invalid 'vector' field at line {} in {}",
                    line_num,
                    file_path.display()
                ))
            })?
            .iter()
            .map(|v| {
                v.as_f64()
                    .ok_or_else(|| {
                        FerresError::Storage(format!(
                            "invalid vector value at line {} in {}",
                            line_num,
                            file_path.display()
                        ))
                    })
                    .map(|f| f as f32)
            })
            .collect::<Result<Vec<f32>, FerresError>>()?;

        let metadata = json.get("metadata").cloned().unwrap_or(serde_json::Value::Null);

        let point = Point::new(id, vector, metadata)?;
        points.push(point);
    }

    if points.is_empty() {
        return Err(FerresError::Storage(format!(
            "no valid points found in {}",
            file_path.display()
        )));
    }

    db.upsert_points(&collection, points.clone())?;

    println!("✓ Inserted {} points into collection '{}'", points.len(), collection);
    Ok(())
}

fn search_points(
    storage_path: &PathBuf,
    collection: String,
    vector_str: String,
    limit: usize,
) -> Result<(), FerresError> {
    info!(
        collection = %collection,
        limit,
        "performing search"
    );

    let db = VectorDB::new(storage_path.clone())?;

    // Parse do vetor (valores separados por vírgula)
    let vector: Vec<f32> = vector_str
        .split(',')
        .map(|s| {
            s.trim()
                .parse::<f32>()
                .map_err(|e| FerresError::Storage(format!("invalid float value: {e}")))
        })
        .collect::<Result<Vec<f32>, FerresError>>()?;

    let results = db.search(&collection, vector, limit)?;

    println!("\nSearch results ({} found):", results.len());
    println!("{:-<80}", "");
    for (i, result) in results.iter().enumerate() {
        println!("\n[{}] ID: {}", i + 1, result.id);
        println!("    Score: {:.6}", result.score);
        println!("    Metadata: {}", result.metadata);
    }
    println!("\n{:-<80}", "");

    Ok(())
}

fn show_stats(storage_path: &PathBuf, collection: String) -> Result<(), FerresError> {
    let db = VectorDB::new(storage_path.clone())?;

    let stats = db.get_collection_stats(&collection)?;

    println!("\nCollection: {}", collection);
    println!("{:-<50}", "");
    println!("Points:           {}", stats.num_points);
    println!("Index size:       {} bytes ({:.2} MB)", 
        stats.index_size_bytes,
        stats.index_size_bytes as f64 / 1_048_576.0
    );
    println!("Last updated:     {}", 
        chrono::DateTime::from_timestamp(stats.last_updated as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "unknown".to_string())
    );
    println!("{:-<50}", "");

    Ok(())
}

fn delete_collection(storage_path: &PathBuf, collection: String) -> Result<(), FerresError> {
    let mut db = VectorDB::new(storage_path.clone())?;

    db.delete_collection(&collection)?;

    println!("✓ Collection '{}' deleted successfully", collection);
    Ok(())
}

fn parse_distance(s: &str) -> Result<DistanceMetric, FerresError> {
    match s.to_lowercase().as_str() {
        "cosine" => Ok(DistanceMetric::Cosine),
        "euclidean" => Ok(DistanceMetric::Euclidean),
        "dotproduct" | "dot" => Ok(DistanceMetric::DotProduct),
        _ => Err(FerresError::Storage(format!(
            "invalid distance metric: {}. Valid options: cosine, euclidean, dotproduct",
            s
        ))),
    }
}


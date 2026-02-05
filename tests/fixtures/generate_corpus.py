#!/usr/bin/env python3
"""
Script para gerar corpus de embeddings para testes do FerresDB.

Gera arquivos JSONL com vetores de embeddings e metadados.
"""

import json
import uuid
import random
from pathlib import Path
from typing import List, Dict, Any
import argparse

try:
    from sentence_transformers import SentenceTransformer
    import numpy as np
except ImportError:
    print("Erro: sentence-transformers e numpy são necessários.")
    print("Instale com: pip install sentence-transformers numpy")
    exit(1)


# Categorias e templates de sentenças para gerar dados variados
CATEGORIES = {
    "technology": [
        "Machine learning algorithms are transforming industries",
        "Artificial intelligence enables new possibilities",
        "Cloud computing provides scalable infrastructure",
        "Blockchain technology ensures data integrity",
        "Quantum computing promises revolutionary advances",
        "Cybersecurity protects digital assets",
        "Data science extracts insights from information",
        "Software engineering builds reliable systems",
        "Network protocols enable communication",
        "Database systems store structured information",
    ],
    "science": [
        "The universe expands at an accelerating rate",
        "DNA contains genetic information",
        "Photosynthesis converts light into energy",
        "Evolution explains biological diversity",
        "Atoms consist of protons and electrons",
        "Gravity affects all matter",
        "Climate change impacts ecosystems",
        "Cells are the basic units of life",
        "Chemical reactions transform substances",
        "Physics describes natural phenomena",
    ],
    "business": [
        "Marketing strategies target customer segments",
        "Financial planning ensures long-term stability",
        "Supply chains optimize logistics",
        "Entrepreneurship drives innovation",
        "Management coordinates team efforts",
        "Sales processes convert leads to customers",
        "Investments generate returns over time",
        "Operations improve efficiency",
        "Strategy guides decision making",
        "Leadership inspires organizational change",
    ],
    "health": [
        "Exercise improves cardiovascular health",
        "Nutrition affects overall well-being",
        "Mental health requires attention",
        "Preventive care reduces disease risk",
        "Sleep quality impacts daily performance",
        "Stress management enhances resilience",
        "Hydration maintains body functions",
        "Meditation promotes mindfulness",
        "Vaccines prevent infectious diseases",
        "Medical research advances treatments",
    ],
    "education": [
        "Learning requires active engagement",
        "Teachers facilitate knowledge transfer",
        "Critical thinking develops reasoning skills",
        "Education empowers individuals",
        "Research expands understanding",
        "Curiosity drives discovery",
        "Practice improves mastery",
        "Feedback enhances learning",
        "Collaboration builds knowledge",
        "Assessment measures progress",
    ],
}


def generate_sentences(count: int) -> List[Dict[str, str]]:
    """Gera uma lista de sentenças com categorias."""
    sentences = []
    all_templates = []
    
    # Coleta todos os templates
    for category, templates in CATEGORIES.items():
        for template in templates:
            all_templates.append((template, category))
    
    # Gera sentenças variadas
    for i in range(count):
        # Seleciona um template base
        base_text, category = random.choice(all_templates)
        
        # Adiciona variações simples para diversidade
        variations = [
            base_text,
            base_text.lower(),
            base_text.capitalize(),
            base_text.replace(".", "!"),
        ]
        
        text = random.choice(variations)
        
        # Adiciona pequenas modificações para gerar mais diversidade
        if random.random() < 0.3:
            # Adiciona prefixos/sufixos ocasionais
            prefixes = ["Modern ", "Advanced ", "Traditional ", "Innovative "]
            suffixes = [" effectively", " efficiently", " successfully"]
            if random.random() < 0.5 and not text.startswith(("Modern", "Advanced", "Traditional", "Innovative")):
                text = random.choice(prefixes) + text.lower()
            elif random.random() < 0.5:
                text = text.rstrip(".!") + random.choice(suffixes) + "."
        
        sentences.append({"text": text, "category": category})
    
    return sentences


def generate_embeddings(sentences: List[Dict[str, str]], model) -> List[np.ndarray]:
    """Gera embeddings para uma lista de sentenças."""
    texts = [s["text"] for s in sentences]
    embeddings = model.encode(texts, show_progress_bar=True, normalize_embeddings=True)
    return embeddings


def create_jsonl_entry(point_id: str, vector: np.ndarray, metadata: Dict[str, Any]) -> Dict[str, Any]:
    """Cria uma entrada JSONL no formato esperado pelo FerresDB."""
    return {
        "id": point_id,
        "vector": vector.tolist(),
        "metadata": metadata,
    }


def generate_corpus(output_dir: Path, sizes: List[int], model_name: str = "all-MiniLM-L6-v2"):
    """Gera arquivos de corpus JSONL com diferentes tamanhos."""
    print(f"Carregando modelo: {model_name}")
    model = SentenceTransformer(model_name)
    
    # Verifica dimensão do modelo
    test_embedding = model.encode(["test"])
    dimension = test_embedding.shape[1]
    print(f"Dimensão dos embeddings: {dimension}")
    
    if dimension != 384:
        print(f"AVISO: Modelo gera vetores de {dimension} dimensões, não 384.")
        print("O corpus será gerado com a dimensão do modelo.")
    
    output_dir.mkdir(parents=True, exist_ok=True)
    
    for size in sizes:
        filename = output_dir / f"corpus_{size//1000}k.jsonl"
        print(f"\nGerando {filename.name} com {size} pontos...")
        
        # Gera sentenças
        sentences = generate_sentences(size)
        
        # Gera embeddings
        print("Gerando embeddings...")
        embeddings = generate_embeddings(sentences, model)
        
        # Escreve arquivo JSONL
        print(f"Escrevendo {filename}...")
        with open(filename, "w", encoding="utf-8") as f:
            for sentence, embedding in zip(sentences, embeddings):
                point_id = str(uuid.uuid4())
                entry = create_jsonl_entry(
                    point_id,
                    embedding,
                    {
                        "text": sentence["text"],
                        "category": sentence["category"],
                    },
                )
                f.write(json.dumps(entry, ensure_ascii=False) + "\n")
        
        print(f"✓ {filename.name} criado com sucesso ({size} pontos)")


def main():
    parser = argparse.ArgumentParser(
        description="Gera corpus de embeddings para testes do FerresDB"
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path(__file__).parent,
        help="Diretório de saída para os arquivos JSONL (padrão: tests/fixtures)",
    )
    parser.add_argument(
        "--sizes",
        type=int,
        nargs="+",
        default=[1000, 10000, 100000],
        help="Tamanhos dos corpus a gerar (padrão: 1000 10000 100000)",
    )
    parser.add_argument(
        "--model",
        type=str,
        default="all-MiniLM-L6-v2",
        help="Modelo sentence-transformers a usar (padrão: all-MiniLM-L6-v2)",
    )
    
    args = parser.parse_args()
    
    print("=" * 60)
    print("Gerador de Corpus para FerresDB")
    print("=" * 60)
    
    generate_corpus(args.output_dir, args.sizes, args.model)
    
    print("\n" + "=" * 60)
    print("Geração concluída!")
    print("=" * 60)


if __name__ == "__main__":
    main()


//! # BM25 — índice de busca por palavras-chave
//!
//! Implementação in-memory de BM25 para busca lexical. Usada em conjunto
//! com o índice vetorial para busca híbrida (RRF).
//!
//! Texto é tokenizado por whitespace e normalizado (lowercase). Documentos
//! sem texto não são indexados.

use std::collections::{HashMap, HashSet};

/// Constante k1 do BM25 (saturação de tf). Típico: 1.2.
const BM25_K1: f64 = 1.2;
/// Constante b do BM25 (normalização por comprimento). Típico: 0.75.
const BM25_B: f64 = 0.75;

/// Tokenização simples: split em whitespace, lowercase, ignora vazios.
fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|s| s.to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Contagem de termos em um documento (term -> count).
fn term_freqs(tokens: &[String]) -> HashMap<String, u32> {
    let mut freqs = HashMap::new();
    for t in tokens {
        *freqs.entry(t.clone()).or_insert(0) += 1;
    }
    freqs
}

/// Índice BM25 in-memory.
///
/// Mantém por documento: comprimento e frequências de termos.
/// Estatísticas globais: N (número de documentos), soma dos comprimentos
/// para avgdl, e document frequency por termo (para IDF).
pub struct BM25Index {
    /// document id -> (length, term -> count)
    docs: HashMap<String, (u64, HashMap<String, u32>)>,
    /// term -> number of documents containing the term
    doc_freq: HashMap<String, u64>,
    /// total number of documents
    n: u64,
    /// sum of document lengths (for avgdl)
    total_len: u64,
}

impl BM25Index {
    /// Cria um índice BM25 vazio.
    pub fn new() -> Self {
        Self {
            docs: HashMap::new(),
            doc_freq: HashMap::new(),
            n: 0,
            total_len: 0,
        }
    }

    /// Índice ou atualiza um documento. Texto vazio remove o documento do índice.
    pub fn index_document(&mut self, id: &str, text: &str) {
        let id = id.to_string();
        if let Some((len, freqs)) = self.docs.remove(&id) {
            self.n = self.n.saturating_sub(1);
            self.total_len = self.total_len.saturating_sub(len);
            for term in freqs.keys() {
                if let Some(df) = self.doc_freq.get_mut(term) {
                    *df = df.saturating_sub(1);
                }
            }
        }
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let tokens = tokenize(text);
        if tokens.is_empty() {
            return;
        }
        let freqs = term_freqs(&tokens);
        let len = tokens.len() as u64;
        for term in freqs.keys() {
            *self.doc_freq.entry(term.clone()).or_insert(0) += 1;
        }
        self.docs.insert(id, (len, freqs));
        self.n += 1;
        self.total_len += len;
    }

    /// Remove um documento do índice.
    pub fn remove_document(&mut self, id: &str) {
        let id = id.to_string();
        if let Some((len, freqs)) = self.docs.remove(&id) {
            self.n = self.n.saturating_sub(1);
            self.total_len = self.total_len.saturating_sub(len);
            for term in freqs.keys() {
                if let Some(df) = self.doc_freq.get_mut(term) {
                    *df = df.saturating_sub(1);
                }
            }
        }
    }

    /// Busca os top-k documentos por score BM25 (maior = melhor).
    pub fn search(&self, query: &str, k: usize) -> Vec<(String, f32)> {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() || self.n == 0 {
            return Vec::new();
        }
        let avgdl = self.total_len as f64 / self.n as f64;
        let mut scores: HashMap<String, f64> = HashMap::new();
        for term in query_tokens.iter().collect::<HashSet<_>>() {
            let n_t = *self.doc_freq.get(term).unwrap_or(&0) as f64;
            let idf = ((self.n as f64 - n_t + 0.5) / (n_t + 0.5) + 1.0).ln();
            if idf <= 0.0 {
                continue;
            }
            for (doc_id, (len, freqs)) in &self.docs {
                let tf = *freqs.get(term).unwrap_or(&0) as f64;
                if tf == 0.0 {
                    continue;
                }
                let len = *len as f64;
                let norm = 1.0 - BM25_B + BM25_B * (len / avgdl);
                let term_score = idf * (tf * (BM25_K1 + 1.0) / (tf + BM25_K1 * norm));
                *scores.entry(doc_id.clone()).or_insert(0.0) += term_score;
            }
        }
        let mut results: Vec<(String, f32)> = scores
            .into_iter()
            .map(|(id, s)| (id, s as f32))
            .collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);
        results
    }

    /// Número de documentos indexados.
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }
}

impl Default for BM25Index {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_index_returns_empty_search() {
        let index = BM25Index::new();
        assert!(index.search("hello", 5).is_empty());
    }

    #[test]
    fn index_and_search() {
        let mut index = BM25Index::new();
        index.index_document("d1", "hello world hello");
        index.index_document("d2", "world foo");
        index.index_document("d3", "hello foo bar");
        let results = index.search("hello world", 5);
        assert!(results.len() <= 5);
        let ids: Vec<_> = results.iter().map(|(id, _)| id.as_str()).collect();
        assert!(ids.contains(&"d1"));
        assert!(ids.contains(&"d2"));
        assert!(ids.contains(&"d3"));
    }

    #[test]
    fn remove_document() {
        let mut index = BM25Index::new();
        index.index_document("d1", "hello world");
        index.index_document("d2", "foo bar");
        assert_eq!(index.len(), 2);
        index.remove_document("d1");
        assert_eq!(index.len(), 1);
        let results = index.search("hello", 5);
        assert!(results.is_empty() || !results.iter().any(|(id, _)| id == "d1"));
    }

    #[test]
    fn empty_text_not_indexed() {
        let mut index = BM25Index::new();
        index.index_document("d1", "  ");
        index.index_document("d2", "");
        assert_eq!(index.len(), 0);
    }

    #[test]
    fn update_document() {
        let mut index = BM25Index::new();
        index.index_document("d1", "hello world");
        index.index_document("d1", "foo bar baz");
        assert_eq!(index.len(), 1);
        let results = index.search("hello", 5);
        assert!(results.is_empty());
        let results = index.search("foo", 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "d1");
    }
}

//! # Quantization — compressão de vetores para redução de memória
//!
//! Implementa **Scalar Quantization (SQ8)**: comprime vetores `f32` para `u8`
//! com perda mínima de recall. Cada dimensão é mapeada independentemente de
//! `[min, max]` para `[0, 255]`, reduzindo o consumo de memória em ~4×.
//!
//! ## Decisões arquiteturais
//!
//! - **Calibração por percentil**: usa `quantile` (default 99.5%) para clampar
//!   outliers antes de definir `min`/`max` por dimensão. Isso evita que um
//!   único vetor extremo distorça a faixa de quantização.
//!
//! - **Distância assimétrica**: na busca, o query permanece em `f32` e só os
//!   candidatos são quantizados. Isso preserva mais precisão que quantizar ambos.
//!
//! - **`always_ram`**: opção para manter vetores originais em memória para
//!   re-ranking final dos top-K, melhorando recall a custo de mais memória.

use serde::{Deserialize, Serialize};

use crate::search::DistanceMetric;

// ─── QuantizationConfig ─────────────────────────────────────────────

/// Configuração de quantização para uma coleção.
///
/// `None` mantém vetores em `f32` (padrão, sem perda).
/// `Scalar` ativa SQ8 — compressão `f32` → `u8` por dimensão.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum QuantizationConfig {
    /// Sem quantização (padrão). Vetores armazenados como `Vec<f32>`.
    #[default]
    None,
    /// Scalar Quantization (SQ8): cada dimensão é mapeada de `[min,max]` para `[0,255]`.
    Scalar(ScalarQuantizationConfig),
}

/// Configuração específica para Scalar Quantization (SQ8).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScalarQuantizationConfig {
    /// Tipo de quantização escalar. Atualmente apenas `Int8`.
    pub dtype: ScalarType,
    /// Se `true`, mantém vetores originais (`f32`) em memória para re-ranking
    /// dos top-K resultados. Melhora recall mas usa mais memória.
    #[serde(default)]
    pub always_ram: bool,
    /// Percentil para clampar outliers antes de calcular `min`/`max` por dimensão.
    /// Valor em [0, 100]. Default: 99.5.
    #[serde(default = "default_quantile")]
    pub quantile: f64,
}

fn default_quantile() -> f64 {
    99.5
}

impl Default for ScalarQuantizationConfig {
    fn default() -> Self {
        Self {
            dtype: ScalarType::Int8,
            always_ram: false,
            quantile: 99.5,
        }
    }
}

/// Tipo de quantização escalar suportado.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ScalarType {
    /// Mapeia cada dimensão `f32` para `u8` (0..255). 4× compressão.
    Int8,
}

// ─── ScalarQuantizationParams ───────────────────────────────────────

/// Parâmetros aprendidos durante calibração de SQ8.
///
/// Armazena `min`, `max` e `scale` por dimensão, usados para converter
/// entre `f32` e `u8`.
///
/// Mapeamento: `quantized[d] = clamp((v[d] - min[d]) * scale[d], 0, 255)`
/// onde `scale[d] = 255.0 / (max[d] - min[d])`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScalarQuantizationParams {
    /// Mínimo por dimensão (percentil inferior).
    pub mins: Vec<f32>,
    /// Máximo por dimensão (percentil superior).
    pub maxs: Vec<f32>,
    /// Escala por dimensão: `255.0 / (max - min)`.
    pub scales: Vec<f32>,
}

/// Tamanho máximo da amostra para calibração de parâmetros.
/// Limita a `10_000` vetores para manter calibração rápida.
const MAX_CALIBRATION_SAMPLE: usize = 10_000;

impl ScalarQuantizationParams {
    /// Calibra os parâmetros de quantização a partir de uma amostra de vetores.
    ///
    /// Usa percentis (`quantile`) para robustez contra outliers:
    /// - `min[d]` = percentil `(100 - quantile)` da dimensão `d`
    /// - `max[d]` = percentil `quantile` da dimensão `d`
    ///
    /// # Panics
    ///
    /// Se `vectors` estiver vazio ou contiver vetores de tamanhos diferentes.
    ///
    /// # Exemplo
    ///
    /// ```rust,ignore
    /// let vectors: Vec<&[f32]> = points.iter().map(|p| p.vector.as_slice()).collect();
    /// let params = ScalarQuantizationParams::calibrate(&vectors, 99.5);
    /// ```
    pub fn calibrate(vectors: &[&[f32]], quantile: f64) -> Self {
        assert!(!vectors.is_empty(), "cannot calibrate with empty vectors");
        let dim = vectors[0].len();
        assert!(dim > 0, "vector dimension must be > 0");

        // Limita a amostra para performance
        let sample: Vec<&[f32]> = if vectors.len() > MAX_CALIBRATION_SAMPLE {
            // Amostra uniforme: pega a cada `step` vetores
            let step = vectors.len() / MAX_CALIBRATION_SAMPLE;
            vectors.iter().step_by(step).take(MAX_CALIBRATION_SAMPLE).copied().collect()
        } else {
            vectors.to_vec()
        };

        let n = sample.len();
        let lower_pct = (100.0 - quantile) / 100.0;
        let upper_pct = quantile / 100.0;

        let mut mins = Vec::with_capacity(dim);
        let mut maxs = Vec::with_capacity(dim);
        let mut scales = Vec::with_capacity(dim);

        // Buffer reutilizável para coletar valores por dimensão
        let mut dim_values = Vec::with_capacity(n);

        for d in 0..dim {
            dim_values.clear();
            for v in &sample {
                dim_values.push(v[d]);
            }
            dim_values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

            // Calcula percentis
            let lower_idx = ((n as f64 * lower_pct).floor() as usize).min(n.saturating_sub(1));
            let upper_idx = ((n as f64 * upper_pct).ceil() as usize).min(n.saturating_sub(1));

            let min_val = dim_values[lower_idx];
            let max_val = dim_values[upper_idx];

            // Evita divisão por zero se min == max
            let range = max_val - min_val;
            let scale = if range.abs() < f32::EPSILON {
                0.0 // Dimensão constante, quantiza tudo para 0
            } else {
                255.0 / range
            };

            mins.push(min_val);
            maxs.push(max_val);
            scales.push(scale);
        }

        Self { mins, maxs, scales }
    }

    /// Quantiza um vetor `f32` para `Vec<u8>`.
    ///
    /// Cada dimensão é mapeada de `[min, max]` para `[0, 255]` usando os
    /// parâmetros calibrados. Valores fora da faixa são clampados.
    #[inline]
    pub fn quantize(&self, vector: &[f32]) -> Vec<u8> {
        debug_assert_eq!(vector.len(), self.mins.len());
        vector
            .iter()
            .enumerate()
            .map(|(d, &v)| {
                let scaled = (v - self.mins[d]) * self.scales[d];
                scaled.round().clamp(0.0, 255.0) as u8
            })
            .collect()
    }

    /// Desquantiza `Vec<u8>` de volta para `Vec<f32>` (aproximado).
    ///
    /// Operação inversa da quantização. O resultado é uma aproximação
    /// do vetor original (erro de quantização ~1/255 da faixa por dimensão).
    #[inline]
    pub fn dequantize(&self, quantized: &[u8]) -> Vec<f32> {
        debug_assert_eq!(quantized.len(), self.mins.len());
        quantized
            .iter()
            .enumerate()
            .map(|(d, &q)| {
                if self.scales[d].abs() < f32::EPSILON {
                    self.mins[d] // Dimensão constante
                } else {
                    self.mins[d] + (q as f32) / self.scales[d]
                }
            })
            .collect()
    }

    /// Calcula distância assimétrica entre um query `f32` e um vetor quantizado `u8`.
    ///
    /// **Assimétrica** significa que o query não é quantizado — isso preserva
    /// mais precisão do que quantizar ambos (distância simétrica).
    ///
    /// Suporta as 3 métricas: Cosine, DotProduct e Euclidean.
    pub fn asymmetric_distance(
        &self,
        query: &[f32],
        quantized: &[u8],
        metric: DistanceMetric,
    ) -> f32 {
        debug_assert_eq!(query.len(), quantized.len());
        debug_assert_eq!(query.len(), self.mins.len());

        match metric {
            DistanceMetric::Euclidean => {
                // L2² (distância euclidiana ao quadrado)
                let mut sum = 0.0f64;
                for d in 0..query.len() {
                    let dequant = if self.scales[d].abs() < f32::EPSILON {
                        self.mins[d]
                    } else {
                        self.mins[d] + (quantized[d] as f32) / self.scales[d]
                    };
                    let diff = (query[d] - dequant) as f64;
                    sum += diff * diff;
                }
                sum as f32
            }
            DistanceMetric::Cosine => {
                // Distância cosseno: 1 - cos(a,b)
                // cos(a,b) = dot(a,b) / (|a| * |b|)
                let mut dot = 0.0f64;
                let mut norm_q = 0.0f64;
                let mut norm_d = 0.0f64;

                for d in 0..query.len() {
                    let dequant = if self.scales[d].abs() < f32::EPSILON {
                        self.mins[d]
                    } else {
                        self.mins[d] + (quantized[d] as f32) / self.scales[d]
                    };
                    let qd = query[d] as f64;
                    let dd = dequant as f64;
                    dot += qd * dd;
                    norm_q += qd * qd;
                    norm_d += dd * dd;
                }

                let denom = norm_q.sqrt() * norm_d.sqrt();
                if denom < f64::EPSILON {
                    1.0 // Máxima distância se algum vetor é zero
                } else {
                    (1.0 - dot / denom) as f32
                }
            }
            DistanceMetric::DotProduct => {
                // Negação do produto escalar (para que menor = mais similar)
                let mut dot = 0.0f64;
                for d in 0..query.len() {
                    let dequant = if self.scales[d].abs() < f32::EPSILON {
                        self.mins[d]
                    } else {
                        self.mins[d] + (quantized[d] as f32) / self.scales[d]
                    };
                    dot += (query[d] as f64) * (dequant as f64);
                }
                // hnsw_rs DistDot calcula 1 - dot(a,b) para vetores normalizados
                (1.0 - dot) as f32
            }
        }
    }

    /// Número de dimensões para o qual os parâmetros foram calibrados.
    pub fn dimension(&self) -> usize {
        self.mins.len()
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Testa calibração com vetores conhecidos: verifica min/max corretos.
    #[test]
    fn test_sq8_calibration() {
        let v1 = vec![0.0, 10.0, -5.0];
        let v2 = vec![1.0, 20.0, -3.0];
        let v3 = vec![2.0, 30.0, -1.0];
        let v4 = vec![3.0, 40.0, 0.0];
        let v5 = vec![4.0, 50.0, 5.0];

        let vectors: Vec<&[f32]> = vec![&v1, &v2, &v3, &v4, &v5];
        // Com quantile 100%, deve pegar min e max absolutos
        let params = ScalarQuantizationParams::calibrate(&vectors, 100.0);

        // Verifica dimensão
        assert_eq!(params.dimension(), 3);

        // Dim 0: min=0, max=4
        assert!((params.mins[0] - 0.0).abs() < 0.01, "min[0]={}", params.mins[0]);
        assert!((params.maxs[0] - 4.0).abs() < 0.01, "max[0]={}", params.maxs[0]);

        // Dim 1: min=10, max=50
        assert!((params.mins[1] - 10.0).abs() < 0.01, "min[1]={}", params.mins[1]);
        assert!((params.maxs[1] - 50.0).abs() < 0.01, "max[1]={}", params.maxs[1]);

        // Dim 2: min=-5, max=5
        assert!((params.mins[2] - (-5.0)).abs() < 0.01, "min[2]={}", params.mins[2]);
        assert!((params.maxs[2] - 5.0).abs() < 0.01, "max[2]={}", params.maxs[2]);

        // Verifica scales
        assert!((params.scales[0] - 255.0 / 4.0).abs() < 0.01);
        assert!((params.scales[1] - 255.0 / 40.0).abs() < 0.01);
        assert!((params.scales[2] - 255.0 / 10.0).abs() < 0.01);
    }

    /// Testa roundtrip: quantize -> dequantize, erro < 1%.
    #[test]
    fn test_sq8_roundtrip() {
        let vectors: Vec<Vec<f32>> = (0..100)
            .map(|i| {
                (0..64)
                    .map(|d| ((i * 64 + d) as f32) / 6400.0 * 2.0 - 1.0) // [-1, 1]
                    .collect()
            })
            .collect();

        let refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();
        let params = ScalarQuantizationParams::calibrate(&refs, 100.0);

        for v in &vectors {
            let quantized = params.quantize(v);
            let dequantized = params.dequantize(&quantized);

            // Verifica que o comprimento é preservado
            assert_eq!(dequantized.len(), v.len());

            // Verifica erro relativo por dimensão
            for d in 0..v.len() {
                let range = params.maxs[d] - params.mins[d];
                if range.abs() < f32::EPSILON {
                    continue; // Dimensão constante, skip
                }
                let error = (v[d] - dequantized[d]).abs();
                let relative_error = error / range;
                assert!(
                    relative_error < 0.01, // < 1% da faixa
                    "dim {}: original={}, dequantized={}, error={}, relative={}",
                    d, v[d], dequantized[d], error, relative_error
                );
            }
        }
    }

    /// Testa que Vec<u8> usa ~4× menos memória que Vec<f32>.
    #[test]
    fn test_sq8_memory() {
        let dim = 384;
        let n = 1000;

        // Memória para Vec<f32>
        let f32_bytes = n * dim * std::mem::size_of::<f32>(); // 4 bytes
        // Memória para Vec<u8>
        let u8_bytes = n * dim * std::mem::size_of::<u8>(); // 1 byte

        // Razão de compressão
        let ratio = f32_bytes as f64 / u8_bytes as f64;
        assert!(
            (ratio - 4.0).abs() < 0.01,
            "expected ~4x compression, got {:.2}x",
            ratio
        );

        // Verifica valores absolutos
        assert_eq!(f32_bytes, n * dim * 4);
        assert_eq!(u8_bytes, n * dim * 1);
    }

    /// Testa distância assimétrica Euclidean.
    #[test]
    fn test_asymmetric_distance_euclidean() {
        let v1 = vec![0.0, 0.0, 0.0];
        let v2 = vec![1.0, 1.0, 1.0];
        let v3 = vec![0.5, 0.5, 0.5];

        let vectors: Vec<&[f32]> = vec![&v1, &v2, &v3];
        let params = ScalarQuantizationParams::calibrate(&vectors, 100.0);

        let q_v2 = params.quantize(&v2);

        // Distância entre v1 e v2 quantizado deve ser ~3.0 (L2²)
        let dist = params.asymmetric_distance(&v1, &q_v2, DistanceMetric::Euclidean);
        assert!(
            (dist - 3.0).abs() < 0.1,
            "expected ~3.0, got {dist}"
        );

        // Distância entre v2 e v2 quantizado deve ser ~0.0
        let dist_self = params.asymmetric_distance(&v2, &q_v2, DistanceMetric::Euclidean);
        assert!(
            dist_self < 0.01,
            "expected ~0.0, got {dist_self}"
        );
    }

    /// Testa distância assimétrica Cosine.
    #[test]
    fn test_asymmetric_distance_cosine() {
        let v1 = vec![1.0, 0.0];
        let v2 = vec![0.0, 1.0];
        let v3 = vec![1.0, 1.0];

        let vectors: Vec<&[f32]> = vec![&v1, &v2, &v3];
        let params = ScalarQuantizationParams::calibrate(&vectors, 100.0);

        // Mesmo vetor: cos distance ~= 0
        let q_v1 = params.quantize(&v1);
        let dist = params.asymmetric_distance(&v1, &q_v1, DistanceMetric::Cosine);
        assert!(dist < 0.1, "same vector cosine distance should be ~0, got {dist}");

        // Vetores ortogonais: cos distance ~= 1
        let q_v2 = params.quantize(&v2);
        let dist_orth = params.asymmetric_distance(&v1, &q_v2, DistanceMetric::Cosine);
        assert!(
            (dist_orth - 1.0).abs() < 0.1,
            "orthogonal vectors cosine distance should be ~1.0, got {dist_orth}"
        );
    }

    /// Testa que a configuração padrão é None (sem quantização).
    #[test]
    fn test_quantization_config_default() {
        let config = QuantizationConfig::default();
        assert_eq!(config, QuantizationConfig::None);
    }

    /// Testa serialização/desserialização da config.
    #[test]
    fn test_quantization_config_serde() {
        let config = QuantizationConfig::Scalar(ScalarQuantizationConfig {
            dtype: ScalarType::Int8,
            always_ram: true,
            quantile: 99.0,
        });

        let json = serde_json::to_string(&config).unwrap();
        let restored: QuantizationConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, restored);
    }

    /// Testa que a config None desserializa corretamente de "None".
    #[test]
    fn test_quantization_config_none_serde() {
        let config = QuantizationConfig::None;
        let json = serde_json::to_string(&config).unwrap();
        let restored: QuantizationConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, restored);
    }

    /// Testa calibração com percentil que exclui outliers.
    #[test]
    fn test_calibration_with_outliers() {
        // 98 vetores normais + 2 outliers extremos
        let mut vectors: Vec<Vec<f32>> = (0..98)
            .map(|i| vec![i as f32 / 100.0]) // [0, 0.97]
            .collect();
        vectors.push(vec![100.0]); // outlier alto
        vectors.push(vec![-100.0]); // outlier baixo

        let refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();
        let params = ScalarQuantizationParams::calibrate(&refs, 95.0);

        // Com 95% quantile, os outliers devem ser clampados
        // max deveria estar perto dos valores normais, não em 100.0
        assert!(
            params.maxs[0] < 50.0,
            "max should be < 50 with 95% quantile, got {}",
            params.maxs[0]
        );
        assert!(
            params.mins[0] > -50.0,
            "min should be > -50 with 95% quantile, got {}",
            params.mins[0]
        );
    }
}

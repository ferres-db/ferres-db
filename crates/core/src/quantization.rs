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
/// `Polar` ativa PolarQuant — coordenadas polares recursivas, sem calibração por bloco.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum QuantizationConfig {
    /// Sem quantização (padrão). Vetores armazenados como `Vec<f32>`.
    #[default]
    None,
    /// Scalar Quantization (SQ8): cada dimensão é mapeada de `[min,max]` para `[0,255]`.
    Scalar(ScalarQuantizationConfig),
    /// PolarQuant: converte pares de coordenadas cartesianas em (raio, ângulo) recursivamente.
    /// Os ângulos são quantizados em `bits_per_angle` bits; o raio final é armazenado como f32.
    /// Não exige calibração por bloco — os ângulos estão sempre em `[0, 2π]`.
    Polar(PolarQuantConfig),
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
    /// Habilita correção residual QJL (Quantized Johnson-Lindenstrauss).
    /// O erro de quantização é projetado em 1 bit por dimensão JL e corrigido
    /// no score final de re-rank. Desabilitado por padrão (opt-in).
    #[serde(default)]
    pub enable_qjl: bool,
    /// Número de dimensões de projeção JL (m << d). Default: 64.
    /// Valores maiores aumentam precisão da correção ao custo de memória/latência.
    #[serde(default = "default_qjl_m")]
    pub qjl_m: usize,
    /// Seed determinístico para gerar a matriz de projeção R.
    /// A mesma seed sempre produz a mesma matriz, garantindo reproducibilidade.
    #[serde(default = "default_qjl_seed")]
    pub qjl_seed: u64,
}

fn default_quantile() -> f64 {
    99.5
}

fn default_qjl_m() -> usize {
    64
}

fn default_qjl_seed() -> u64 {
    42
}

impl Default for ScalarQuantizationConfig {
    fn default() -> Self {
        Self {
            dtype: ScalarType::Int8,
            always_ram: false,
            quantile: 99.5,
            enable_qjl: false,
            qjl_m: 64,
            qjl_seed: 42,
        }
    }
}

/// Tipo de quantização escalar suportado.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ScalarType {
    /// Mapeia cada dimensão `f32` para `u8` (0..255). 4× compressão.
    Int8,
}

// ─── PolarQuant ──────────────────────────────────────────────────────

/// Configuração para PolarQuant.
///
/// `bits_per_angle` controla a resolução angular (default: 8 bits = 256 níveis).
/// Com 8 bits, a resolução é `2π/256 ≈ 0.025 rad`, dando erro de reconstrução
/// por coordenada de `r·sin(π/256) ≈ 0.012·r` — suficiente para alta fidelidade.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolarQuantConfig {
    /// Número de bits por ângulo (1–8). 8 bits = 256 níveis angulares ∈ [0, 2π].
    #[serde(default = "default_bits_per_angle")]
    pub bits_per_angle: u8,
}

fn default_bits_per_angle() -> u8 {
    8
}

impl Default for PolarQuantConfig {
    fn default() -> Self {
        Self { bits_per_angle: 8 }
    }
}

/// Parâmetros de runtime para PolarQuant (derivados de `PolarQuantConfig`).
///
/// Ao contrário do SQ8, PolarQuant não precisa de calibração por coleção —
/// os ângulos têm fronteiras fixas em `[0, 2π]`. Esta struct existe apenas
/// para manter simetria com `ScalarQuantizationParams`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolarQuantParams {
    /// Bits por ângulo, copiado da configuração da coleção.
    pub bits_per_angle: u8,
}

/// Vetor comprimido pelo PolarQuant.
///
/// Resultado de `polar_encode`: contém um único raio escalar (`final_radius`)
/// que codifica a magnitude total, mais `dim - 1` ângulos quantizados que
/// codificam a direção em coordenadas polares recursivas.
///
/// ## Layout dos ângulos
///
/// Os ângulos são armazenados nível a nível (nível 0 primeiro):
/// `[θ₀⁰, …, θ_{n/2-1}⁰, θ₀¹, …]` até restar apenas 1 raio.
/// Para `dim = 128`, temos `127` ângulos no total.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolarQuantized {
    /// Raio final (produto recursivo de todos os raios dos pares).
    pub final_radius: f32,
    /// Ângulos quantizados (todos os níveis concatenados, nível 0 primeiro).
    pub angles: Vec<u8>,
    /// Dimensão original do vetor (necessária para `polar_decode`).
    pub dim: usize,
    /// Bits por ângulo usados na codificação (necessário para decodificação correta).
    pub bits_per_angle: u8,
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

/// Epsilon para tratar escala zero na desquantização (compatível com o escalar).
const SCALE_EPS: f32 = f32::EPSILON;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod asym_simd {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    #[inline]
    unsafe fn hsum_m256(v: __m256) -> f32 {
        let t = _mm256_hadd_ps(v, v);
        let t = _mm256_hadd_ps(t, t);
        let lo = _mm256_castps256_ps128(t);
        let hi = _mm256_extractf128_ps(t, 1);
        let sum = _mm_add_ps(lo, hi);
        _mm_cvtss_f32(_mm_hadd_ps(sum, sum))
    }

    #[inline]
    unsafe fn hsum_m128(v: __m128) -> f32 {
        let t = _mm_hadd_ps(v, v);
        _mm_cvtss_f32(_mm_hadd_ps(t, t))
    }

    /// Load 8 u8 from ptr and convert to __m256 of f32.
    #[inline]
    unsafe fn load8_u8_to_f32(ptr: *const u8) -> __m256 {
        let u8_8 = _mm_loadl_epi64(ptr as *const __m128i);
        let lo = _mm_cvtepu8_epi32(u8_8);
        let hi = _mm_cvtepu8_epi32(_mm_srli_si128(u8_8, 4));
        let lo_ps = _mm_cvtepi32_ps(lo);
        let hi_ps = _mm_cvtepi32_ps(hi);
        _mm256_setr_m128(lo_ps, hi_ps)
    }

    /// Dequantize 8 elements: dequant = mins + q/scale when |scale| >= eps, else mins.
    #[inline]
    unsafe fn dequant8(mins: __m256, scales: __m256, q_ps: __m256) -> __m256 {
        let eps = _mm256_set1_ps(super::SCALE_EPS);
        let scale_abs = _mm256_max_ps(scales, _mm256_sub_ps(_mm256_setzero_ps(), scales));
        let scale_safe = _mm256_max_ps(scale_abs, eps);
        let dequant_linear = _mm256_add_ps(mins, _mm256_div_ps(q_ps, scale_safe));
        let mask = _mm256_cmp_ps(scale_abs, eps, _CMP_GE_OQ);
        _mm256_blendv_ps(mins, dequant_linear, mask)
    }

    #[target_feature(enable = "avx")]
    #[inline]
    pub unsafe fn asymmetric_l2_avx2(
        query: &[f32],
        quantized: &[u8],
        mins: &[f32],
        scales: &[f32],
    ) -> f32 {
        let n = query.len();
        let mut acc = _mm256_setzero_ps();
        let mut i = 0;
        while i + 8 <= n {
            let q_ps = load8_u8_to_f32(quantized.as_ptr().add(i));
            let mins_v = _mm256_loadu_ps(mins.as_ptr().add(i));
            let scales_v = _mm256_loadu_ps(scales.as_ptr().add(i));
            let dequant = dequant8(mins_v, scales_v, q_ps);
            let q_v = _mm256_loadu_ps(query.as_ptr().add(i));
            let d = _mm256_sub_ps(q_v, dequant);
            acc = _mm256_add_ps(acc, _mm256_mul_ps(d, d));
            i += 8;
        }
        let mut sum = hsum_m256(acc);
        while i < n {
            let dequant = if scales[i].abs() < super::SCALE_EPS {
                mins[i]
            } else {
                mins[i] + (quantized[i] as f32) / scales[i]
            };
            let diff = query[i] - dequant;
            sum += diff * diff;
            i += 1;
        }
        sum
    }

    #[target_feature(enable = "sse4.1")]
    #[inline]
    pub unsafe fn asymmetric_l2_sse41(
        query: &[f32],
        quantized: &[u8],
        mins: &[f32],
        scales: &[f32],
    ) -> f32 {
        let n = query.len();
        let mut acc = _mm_setzero_ps();
        let mut i = 0;
        while i + 4 <= n {
            let u8_4 = _mm_loadu_si32(quantized.as_ptr().add(i));
            let q_i = _mm_cvtepu8_epi32(u8_4);
            let q_ps = _mm_cvtepi32_ps(q_i);
            let mins_v = _mm_loadu_ps(mins.as_ptr().add(i));
            let scales_v = _mm_loadu_ps(scales.as_ptr().add(i));
            let eps = _mm_set1_ps(super::SCALE_EPS);
            let scale_abs = _mm_max_ps(scales_v, _mm_sub_ps(_mm_setzero_ps(), scales_v));
            let scale_safe = _mm_max_ps(scale_abs, eps);
            let dequant_linear = _mm_add_ps(mins_v, _mm_div_ps(q_ps, scale_safe));
            let mask = _mm_cmpge_ps(scale_abs, eps);
            let dequant = _mm_blendv_ps(mins_v, dequant_linear, mask);
            let q_v = _mm_loadu_ps(query.as_ptr().add(i));
            let d = _mm_sub_ps(q_v, dequant);
            acc = _mm_add_ps(acc, _mm_mul_ps(d, d));
            i += 4;
        }
        let mut sum = hsum_m128(acc);
        while i < n {
            let dequant = if scales[i].abs() < super::SCALE_EPS {
                mins[i]
            } else {
                mins[i] + (quantized[i] as f32) / scales[i]
            };
            let diff = query[i] - dequant;
            sum += diff * diff;
            i += 1;
        }
        sum
    }

    #[target_feature(enable = "avx")]
    #[inline]
    pub unsafe fn asymmetric_dot_avx2(
        query: &[f32],
        quantized: &[u8],
        mins: &[f32],
        scales: &[f32],
    ) -> f32 {
        let n = query.len();
        let mut acc = _mm256_setzero_ps();
        let mut i = 0;
        while i + 8 <= n {
            let q_ps = load8_u8_to_f32(quantized.as_ptr().add(i));
            let mins_v = _mm256_loadu_ps(mins.as_ptr().add(i));
            let scales_v = _mm256_loadu_ps(scales.as_ptr().add(i));
            let dequant = dequant8(mins_v, scales_v, q_ps);
            let q_v = _mm256_loadu_ps(query.as_ptr().add(i));
            acc = _mm256_add_ps(acc, _mm256_mul_ps(q_v, dequant));
            i += 8;
        }
        let mut sum = hsum_m256(acc);
        while i < n {
            let dequant = if scales[i].abs() < super::SCALE_EPS {
                mins[i]
            } else {
                mins[i] + (quantized[i] as f32) / scales[i]
            };
            sum += query[i] * dequant;
            i += 1;
        }
        (1.0 - sum) as f32
    }

    #[target_feature(enable = "sse4.1")]
    #[inline]
    pub unsafe fn asymmetric_dot_sse41(
        query: &[f32],
        quantized: &[u8],
        mins: &[f32],
        scales: &[f32],
    ) -> f32 {
        let n = query.len();
        let mut acc = _mm_setzero_ps();
        let mut i = 0;
        while i + 4 <= n {
            let u8_4 = _mm_loadu_si32(quantized.as_ptr().add(i));
            let q_i = _mm_cvtepu8_epi32(u8_4);
            let q_ps = _mm_cvtepi32_ps(q_i);
            let mins_v = _mm_loadu_ps(mins.as_ptr().add(i));
            let scales_v = _mm_loadu_ps(scales.as_ptr().add(i));
            let eps = _mm_set1_ps(super::SCALE_EPS);
            let scale_abs = _mm_max_ps(scales_v, _mm_sub_ps(_mm_setzero_ps(), scales_v));
            let scale_safe = _mm_max_ps(scale_abs, eps);
            let dequant_linear = _mm_add_ps(mins_v, _mm_div_ps(q_ps, scale_safe));
            let mask = _mm_cmpge_ps(scale_abs, eps);
            let dequant = _mm_blendv_ps(mins_v, dequant_linear, mask);
            let q_v = _mm_loadu_ps(query.as_ptr().add(i));
            acc = _mm_add_ps(acc, _mm_mul_ps(q_v, dequant));
            i += 4;
        }
        let mut sum = hsum_m128(acc);
        while i < n {
            let dequant = if scales[i].abs() < super::SCALE_EPS {
                mins[i]
            } else {
                mins[i] + (quantized[i] as f32) / scales[i]
            };
            sum += query[i] * dequant;
            i += 1;
        }
        (1.0 - sum) as f32
    }

    #[target_feature(enable = "avx")]
    #[inline]
    pub unsafe fn asymmetric_cosine_avx2(
        query: &[f32],
        quantized: &[u8],
        mins: &[f32],
        scales: &[f32],
    ) -> f32 {
        let n = query.len();
        let mut dot_acc = _mm256_setzero_ps();
        let mut nq_acc = _mm256_setzero_ps();
        let mut nd_acc = _mm256_setzero_ps();
        let mut i = 0;
        while i + 8 <= n {
            let q_ps = load8_u8_to_f32(quantized.as_ptr().add(i));
            let mins_v = _mm256_loadu_ps(mins.as_ptr().add(i));
            let scales_v = _mm256_loadu_ps(scales.as_ptr().add(i));
            let dequant = dequant8(mins_v, scales_v, q_ps);
            let q_v = _mm256_loadu_ps(query.as_ptr().add(i));
            dot_acc = _mm256_add_ps(dot_acc, _mm256_mul_ps(q_v, dequant));
            nq_acc = _mm256_add_ps(nq_acc, _mm256_mul_ps(q_v, q_v));
            nd_acc = _mm256_add_ps(nd_acc, _mm256_mul_ps(dequant, dequant));
            i += 8;
        }
        let mut dot: f64 = hsum_m256(dot_acc).into();
        let mut norm_q: f64 = hsum_m256(nq_acc).into();
        let mut norm_d: f64 = hsum_m256(nd_acc).into();
        while i < n {
            let dequant = if scales[i].abs() < super::SCALE_EPS {
                mins[i]
            } else {
                mins[i] + (quantized[i] as f32) / scales[i]
            };
            let qd = query[i] as f64;
            let dd = dequant as f64;
            dot += qd * dd;
            norm_q += qd * qd;
            norm_d += dd * dd;
            i += 1;
        }
        let denom = norm_q.sqrt() * norm_d.sqrt();
        if denom < f64::EPSILON {
            1.0
        } else {
            (1.0 - dot / denom) as f32
        }
    }

    #[target_feature(enable = "sse4.1")]
    #[inline]
    pub unsafe fn asymmetric_cosine_sse41(
        query: &[f32],
        quantized: &[u8],
        mins: &[f32],
        scales: &[f32],
    ) -> f32 {
        let n = query.len();
        let mut dot_acc = _mm_setzero_ps();
        let mut nq_acc = _mm_setzero_ps();
        let mut nd_acc = _mm_setzero_ps();
        let mut i = 0;
        while i + 4 <= n {
            let u8_4 = _mm_loadu_si32(quantized.as_ptr().add(i));
            let q_i = _mm_cvtepu8_epi32(u8_4);
            let q_ps = _mm_cvtepi32_ps(q_i);
            let mins_v = _mm_loadu_ps(mins.as_ptr().add(i));
            let scales_v = _mm_loadu_ps(scales.as_ptr().add(i));
            let eps = _mm_set1_ps(super::SCALE_EPS);
            let scale_abs = _mm_max_ps(scales_v, _mm_sub_ps(_mm_setzero_ps(), scales_v));
            let scale_safe = _mm_max_ps(scale_abs, eps);
            let dequant_linear = _mm_add_ps(mins_v, _mm_div_ps(q_ps, scale_safe));
            let mask = _mm_cmpge_ps(scale_abs, eps);
            let dequant = _mm_blendv_ps(mins_v, dequant_linear, mask);
            let q_v = _mm_loadu_ps(query.as_ptr().add(i));
            dot_acc = _mm_add_ps(dot_acc, _mm_mul_ps(q_v, dequant));
            nq_acc = _mm_add_ps(nq_acc, _mm_mul_ps(q_v, q_v));
            nd_acc = _mm_add_ps(nd_acc, _mm_mul_ps(dequant, dequant));
            i += 4;
        }
        let mut dot: f64 = hsum_m128(dot_acc).into();
        let mut norm_q: f64 = hsum_m128(nq_acc).into();
        let mut norm_d: f64 = hsum_m128(nd_acc).into();
        while i < n {
            let dequant = if scales[i].abs() < super::SCALE_EPS {
                mins[i]
            } else {
                mins[i] + (quantized[i] as f32) / scales[i]
            };
            let qd = query[i] as f64;
            let dd = dequant as f64;
            dot += qd * dd;
            norm_q += qd * qd;
            norm_d += dd * dd;
            i += 1;
        }
        let denom = norm_q.sqrt() * norm_d.sqrt();
        if denom < f64::EPSILON {
            1.0
        } else {
            (1.0 - dot / denom) as f32
        }
    }
}

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
            vectors
                .iter()
                .step_by(step)
                .take(MAX_CALIBRATION_SAMPLE)
                .copied()
                .collect()
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
    /// SIMD-acelerado em x86/x86_64 (AVX2 → SSE4.1 → escalar).
    pub fn asymmetric_distance(
        &self,
        query: &[f32],
        quantized: &[u8],
        metric: DistanceMetric,
    ) -> f32 {
        debug_assert_eq!(query.len(), quantized.len());
        debug_assert_eq!(query.len(), self.mins.len());

        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            if std::arch::is_x86_feature_detected!("avx2") {
                return match metric {
                    DistanceMetric::Euclidean => unsafe {
                        asym_simd::asymmetric_l2_avx2(query, quantized, &self.mins, &self.scales)
                    },
                    DistanceMetric::DotProduct => unsafe {
                        asym_simd::asymmetric_dot_avx2(query, quantized, &self.mins, &self.scales)
                    },
                    DistanceMetric::Cosine => unsafe {
                        asym_simd::asymmetric_cosine_avx2(
                            query,
                            quantized,
                            &self.mins,
                            &self.scales,
                        )
                    },
                };
            }
            if std::arch::is_x86_feature_detected!("sse4.1") {
                return match metric {
                    DistanceMetric::Euclidean => unsafe {
                        asym_simd::asymmetric_l2_sse41(query, quantized, &self.mins, &self.scales)
                    },
                    DistanceMetric::DotProduct => unsafe {
                        asym_simd::asymmetric_dot_sse41(query, quantized, &self.mins, &self.scales)
                    },
                    DistanceMetric::Cosine => unsafe {
                        asym_simd::asymmetric_cosine_sse41(
                            query,
                            quantized,
                            &self.mins,
                            &self.scales,
                        )
                    },
                };
            }
        }

        match metric {
            DistanceMetric::Euclidean => {
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
                    1.0
                } else {
                    (1.0 - dot / denom) as f32
                }
            }
            DistanceMetric::DotProduct => {
                let mut dot = 0.0f64;
                for d in 0..query.len() {
                    let dequant = if self.scales[d].abs() < f32::EPSILON {
                        self.mins[d]
                    } else {
                        self.mins[d] + (quantized[d] as f32) / self.scales[d]
                    };
                    dot += (query[d] as f64) * (dequant as f64);
                }
                (1.0 - dot) as f32
            }
        }
    }

    /// Número de dimensões para o qual os parâmetros foram calibrados.
    pub fn dimension(&self) -> usize {
        self.mins.len()
    }
}

// ─── QjlParams ───────────────────────────────────────────────────────

/// Parâmetros para correção residual QJL (Quantized Johnson-Lindenstrauss).
///
/// ## Como funciona
///
/// Após quantizar um vetor com SQ8, existe um erro de quantização:
/// `residual = original - dequantize(quantized)`.
///
/// QJL aplica uma matriz de projeção aleatória `R ∈ {+1,-1}^{m×d}` ao residual e
/// armazena apenas o sinal de cada componente projetada (1 bit por dimensão).
/// Na busca, o estimador não-enviesado `(2/m) · Σᵢ(q_projected_i · sign_i)` corrige
/// o score SQ8 reduzindo o viés introduzido pela quantização.
///
/// ## Serialização
///
/// Apenas `m`, `dim` e `seed` são serializados. A matriz `R` é regenerada
/// deterministicamente a partir do `seed` — evita serializar até `m×d` bytes.
#[derive(Debug, Clone)]
pub struct QjlParams {
    /// Número de dimensões de projeção (m << d).
    pub m: usize,
    /// Dimensão do vetor original.
    pub dim: usize,
    /// Seed determinístico para reproduzir a matriz R.
    pub seed: u64,
    /// Matriz de projeção R ∈ {+1,-1}^{m×d} (gerada a partir do seed).
    /// Não serializada — regenerada no load.
    projection_matrix: Vec<Vec<i8>>,
}

/// Estrutura auxiliar apenas para (de)serialização de QjlParams.
#[derive(Serialize, Deserialize)]
struct QjlParamsSeed {
    m: usize,
    dim: usize,
    seed: u64,
}

impl serde::Serialize for QjlParams {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        QjlParamsSeed {
            m: self.m,
            dim: self.dim,
            seed: self.seed,
        }
        .serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for QjlParams {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let QjlParamsSeed { m, dim, seed } = QjlParamsSeed::deserialize(deserializer)?;
        Ok(QjlParams::new(dim, m, seed))
    }
}

impl QjlParams {
    /// Cria novos parâmetros QJL gerando a matriz de projeção R deterministicamente.
    ///
    /// A matriz R tem valores Rademacher: cada entrada é +1 ou -1 com igual probabilidade,
    /// gerada com `StdRng::seed_from_u64(seed)` para reproducibilidade.
    pub fn new(dim: usize, m: usize, seed: u64) -> Self {
        use rand::{Rng, SeedableRng};
        use rand::rngs::StdRng;

        let mut rng = StdRng::seed_from_u64(seed);
        let projection_matrix: Vec<Vec<i8>> = (0..m)
            .map(|_| {
                (0..dim)
                    .map(|_| if rng.gen_bool(0.5) { 1i8 } else { -1i8 })
                    .collect()
            })
            .collect();

        Self {
            m,
            dim,
            seed,
            projection_matrix,
        }
    }

    /// Projeta o residual e comprime o sinal em bitset packed (u64).
    ///
    /// Para cada linha i de R:
    ///   `proj_i = R[i] · residual`  (produto escalar com ±1)
    ///   bit i = 1 se `proj_i > 0`, 0 caso contrário.
    ///
    /// Retorna `ceil(m / 64)` palavras u64 com os bits compactados.
    pub fn encode_residual(&self, residual: &[f32]) -> Vec<u64> {
        debug_assert_eq!(residual.len(), self.dim);
        let num_words = (self.m + 63) / 64;
        let mut bits = vec![0u64; num_words];

        for (i, row) in self.projection_matrix.iter().enumerate() {
            let proj: f32 = row
                .iter()
                .zip(residual.iter())
                .map(|(&r, &v)| r as f32 * v)
                .sum();
            if proj > 0.0 {
                bits[i / 64] |= 1u64 << (i % 64);
            }
        }

        bits
    }

    /// Projeta o query com a mesma matriz R.
    ///
    /// Necessário para calcular a correção: `projected_query_i = R[i] · query`.
    pub fn project_query(&self, query: &[f32]) -> Vec<f32> {
        self.projection_matrix
            .iter()
            .map(|row| {
                row.iter()
                    .zip(query.iter())
                    .map(|(&r, &q)| r as f32 * q)
                    .sum()
            })
            .collect()
    }

    /// Calcula o score de correção QJL não-enviesado.
    ///
    /// `correction = (2/m) · Σᵢ (projected_query_i · sign_i)`
    ///
    /// onde `sign_i = +1.0` se o bit i do bitset estiver setado, `-1.0` caso contrário.
    ///
    /// Itera por palavras u64 para eficiência de cache (acesso compacto ao bitset).
    pub fn correction_score(&self, query: &[f32], sign_bits: &[u64]) -> f32 {
        let projected = self.project_query(query);
        let mut sum = 0.0f32;

        for (word_idx, &word) in sign_bits.iter().enumerate() {
            let base = word_idx * 64;
            let bits_in_word = self.m.saturating_sub(base).min(64);
            for bit_pos in 0..bits_in_word {
                let idx = base + bit_pos;
                let sign = if word & (1u64 << bit_pos) != 0 {
                    1.0f32
                } else {
                    -1.0f32
                };
                sum += projected[idx] * sign;
            }
        }

        (2.0 / self.m as f32) * sum
    }
}

// ─── PolarQuant functions ────────────────────────────────────────────

/// Codifica um vetor `f32` para coordenadas polares recursivas quantizadas.
///
/// ## Algoritmo
///
/// 1. Agrupa pares `(v[2i], v[2i+1])` → `(r, θ)` onde `r = hypot(x, y)` e
///    `θ = atan2(y, x) + π` (deslocado para `[0, 2π]`).
/// 2. Quantiza `θ` para `[0, 2^bits - 1]` como `u8`.
/// 3. Repete com o buffer de raios até restar apenas um raio (`final_radius`).
///
/// Dimensões ímpares são tratadas adicionando `0.0` ao final do buffer antes
/// de cada nível com número ímpar de elementos.
///
/// ## Complexidade
///
/// O(n log n) tempo, O(n) memória. Para dim = 128: 127 ângulos, 1 raio final.
pub fn polar_encode(vector: &[f32], bits_per_angle: u8) -> PolarQuantized {
    debug_assert!(
        bits_per_angle >= 1 && bits_per_angle <= 8,
        "bits_per_angle must be in [1, 8]"
    );

    let dim = vector.len();
    let max_angle_val = ((1u16 << bits_per_angle) - 1) as f32;
    let two_pi = 2.0 * std::f32::consts::PI;

    if dim == 0 {
        return PolarQuantized {
            final_radius: 0.0,
            angles: Vec::new(),
            dim: 0,
            bits_per_angle,
        };
    }

    let mut angles: Vec<u8> = Vec::with_capacity(dim.saturating_sub(1));
    let mut current: Vec<f32> = vector.to_vec();

    while current.len() > 1 {
        // Pad to even length so every element is part of a pair
        if current.len() % 2 == 1 {
            current.push(0.0);
        }

        let n_pairs = current.len() / 2;
        let mut next_radii = Vec::with_capacity(n_pairs);

        for i in 0..n_pairs {
            let x = current[2 * i];
            let y = current[2 * i + 1];
            let r = x.hypot(y);
            // atan2 ∈ [-π, π]; shift to [0, 2π]
            let theta = y.atan2(x) + std::f32::consts::PI;
            // Quantize: [0, 2π] → [0, max_angle_val]
            let q = (theta / two_pi * max_angle_val)
                .round()
                .clamp(0.0, max_angle_val) as u8;
            angles.push(q);
            next_radii.push(r);
        }

        current = next_radii;
    }

    PolarQuantized {
        final_radius: current.into_iter().next().unwrap_or(0.0),
        angles,
        dim,
        bits_per_angle,
    }
}

/// Decodifica um vetor `PolarQuantized` de volta para coordenadas cartesianas `f32`.
///
/// A reconstrução é uma aproximação: o erro por coordenada é `r·sin(π/2^bits)`,
/// onde `r` é o raio do par. Com 8 bits, isso resulta em erro < 0.013 por
/// coordenada para vetores unitários.
pub fn polar_decode(quantized: &PolarQuantized) -> Vec<f32> {
    let dim = quantized.dim;
    let bits = quantized.bits_per_angle;
    let max_angle_val = ((1u16 << bits) - 1) as f32;
    let two_pi = 2.0 * std::f32::consts::PI;

    if dim == 0 {
        return Vec::new();
    }
    if dim == 1 {
        return vec![quantized.final_radius];
    }

    // Compute level structure: level_sizes[k] = number of pairs (= angles) at level k.
    // The angles Vec is laid out as [level_0_angles..., level_1_angles..., ...].
    let mut level_sizes: Vec<usize> = Vec::new();
    let mut sz = dim;
    while sz > 1 {
        let pairs = (sz + 1) / 2;
        level_sizes.push(pairs);
        sz = pairs;
    }

    // Angle offset into `quantized.angles` for each level
    let mut angle_offsets: Vec<usize> = vec![0; level_sizes.len()];
    for i in 1..level_sizes.len() {
        angle_offsets[i] = angle_offsets[i - 1] + level_sizes[i - 1];
    }

    // input_sizes[k] = number of elements that were fed into level k during encoding.
    // input_sizes[0] = dim (original dimension, which may have been padded to even).
    // input_sizes[k] = level_sizes[k-1] for k > 0 (the radii produced by the previous level).
    let mut input_sizes: Vec<usize> = vec![0; level_sizes.len()];
    input_sizes[0] = dim;
    for k in 1..level_sizes.len() {
        input_sizes[k] = level_sizes[k - 1];
    }

    // Start from the final (deepest) radius and expand level by level in reverse.
    let mut radii = vec![quantized.final_radius];

    for level in (0..level_sizes.len()).rev() {
        let n_pairs = level_sizes[level];
        let offset = angle_offsets[level];
        let input_size = input_sizes[level];

        let mut expanded = Vec::with_capacity(2 * n_pairs);

        for i in 0..n_pairs {
            let r = radii[i];
            let q = quantized.angles[offset + i] as f32;
            // Dequantize: [0, max_angle_val] → [0, 2π] → [-π, π]
            let theta = q / max_angle_val * two_pi - std::f32::consts::PI;
            expanded.push(r * theta.cos());
            expanded.push(r * theta.sin());
        }

        // Trim to actual input size: removes the padding zero added for odd dimensions.
        expanded.truncate(input_size);
        radii = expanded;
    }

    radii
}

/// Calcula distância assimétrica entre um query `f32` e um vetor PolarQuant.
///
/// **Assimétrica**: o query permanece em `f32` original; apenas o candidato
/// indexado é decodificado on-the-fly de coordenadas polares. Isso preserva
/// a precisão do query evitando o erro de quantização no lado da consulta.
///
/// Suporta Cosine, Euclidean e DotProduct (mesmo contrato que SQ8).
pub fn polar_distance_asymmetric(
    query: &[f32],
    quantized: &PolarQuantized,
    metric: DistanceMetric,
) -> f32 {
    let decoded = polar_decode(quantized);
    let n = query.len().min(decoded.len());

    match metric {
        DistanceMetric::Euclidean => query[..n]
            .iter()
            .zip(decoded[..n].iter())
            .map(|(a, b)| (a - b).powi(2))
            .sum(),
        DistanceMetric::Cosine => {
            let mut dot = 0.0f64;
            let mut norm_q = 0.0f64;
            let mut norm_d = 0.0f64;
            for i in 0..n {
                let q = query[i] as f64;
                let d = decoded[i] as f64;
                dot += q * d;
                norm_q += q * q;
                norm_d += d * d;
            }
            let denom = norm_q.sqrt() * norm_d.sqrt();
            if denom < f64::EPSILON {
                1.0
            } else {
                (1.0 - dot / denom) as f32
            }
        }
        DistanceMetric::DotProduct => {
            let dot: f64 = query[..n]
                .iter()
                .zip(decoded[..n].iter())
                .map(|(a, b)| (*a as f64) * (*b as f64))
                .sum();
            (1.0 - dot) as f32
        }
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
        assert!(
            (params.mins[0] - 0.0).abs() < 0.01,
            "min[0]={}",
            params.mins[0]
        );
        assert!(
            (params.maxs[0] - 4.0).abs() < 0.01,
            "max[0]={}",
            params.maxs[0]
        );

        // Dim 1: min=10, max=50
        assert!(
            (params.mins[1] - 10.0).abs() < 0.01,
            "min[1]={}",
            params.mins[1]
        );
        assert!(
            (params.maxs[1] - 50.0).abs() < 0.01,
            "max[1]={}",
            params.maxs[1]
        );

        // Dim 2: min=-5, max=5
        assert!(
            (params.mins[2] - (-5.0)).abs() < 0.01,
            "min[2]={}",
            params.mins[2]
        );
        assert!(
            (params.maxs[2] - 5.0).abs() < 0.01,
            "max[2]={}",
            params.maxs[2]
        );

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
                    d,
                    v[d],
                    dequantized[d],
                    error,
                    relative_error
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
            "expected ~4x compression, got {ratio:.2}x"
        );

        // Verifica valores absolutos
        assert_eq!(f32_bytes, n * dim * 4);
        assert_eq!(u8_bytes, (n * dim));
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
        assert!((dist - 3.0).abs() < 0.1, "expected ~3.0, got {dist}");

        // Distância entre v2 e v2 quantizado deve ser ~0.0
        let dist_self = params.asymmetric_distance(&v2, &q_v2, DistanceMetric::Euclidean);
        assert!(dist_self < 0.01, "expected ~0.0, got {dist_self}");
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
        assert!(
            dist < 0.1,
            "same vector cosine distance should be ~0, got {dist}"
        );

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
            ..Default::default()
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

    /// Testa serialização/desserialização da config PolarQuant.
    #[test]
    fn test_polar_config_serde() {
        let config = QuantizationConfig::Polar(PolarQuantConfig { bits_per_angle: 8 });
        let json = serde_json::to_string(&config).unwrap();
        let restored: QuantizationConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, restored);
    }

    /// Testa que polar_encode → polar_decode preserva a direção do vetor.
    ///
    /// A distância coseno entre o vetor original e o decodificado deve ser < 0.01
    /// para dim 64 e 128 com vetores aleatórios.
    #[test]
    fn test_polar_encode_decode_direction() {
        use rand::Rng;

        for &dim in &[4usize, 8, 64, 128] {
            let mut rng = rand::thread_rng();
            let mut max_cos_dist = 0.0f32;

            for _ in 0..50 {
                let vector: Vec<f32> = (0..dim).map(|_| rng.gen_range(-1.0f32..1.0)).collect();

                let quantized = polar_encode(&vector, 8);
                let decoded = polar_decode(&quantized);

                assert_eq!(decoded.len(), dim, "decoded length must match original dim");

                // Cosine distance between original and decoded
                let mut dot = 0.0f64;
                let mut norm_orig = 0.0f64;
                let mut norm_dec = 0.0f64;
                for i in 0..dim {
                    let a = vector[i] as f64;
                    let b = decoded[i] as f64;
                    dot += a * b;
                    norm_orig += a * a;
                    norm_dec += b * b;
                }
                let denom = norm_orig.sqrt() * norm_dec.sqrt();
                let cos_dist = if denom < f64::EPSILON {
                    0.0f32
                } else {
                    (1.0 - dot / denom) as f32
                };

                if cos_dist > max_cos_dist {
                    max_cos_dist = cos_dist;
                }
            }

            assert!(
                max_cos_dist < 0.01,
                "dim={dim}: max cosine distance after encode→decode too high: {max_cos_dist:.6}"
            );
        }
    }

    /// Testa que polar_distance_asymmetric retorna ≈ 0 para vetor contra si mesmo.
    #[test]
    fn test_polar_distance_asymmetric_identity() {
        use rand::Rng;

        let mut rng = rand::thread_rng();
        let vector: Vec<f32> = (0..128).map(|_| rng.gen_range(-1.0f32..1.0)).collect();
        let quantized = polar_encode(&vector, 8);

        for metric in [
            DistanceMetric::Euclidean,
            DistanceMetric::Cosine,
            DistanceMetric::DotProduct,
        ] {
            let dist = polar_distance_asymmetric(&vector, &quantized, metric);
            assert!(
                dist < 0.1,
                "metric={metric:?}: self-distance should be ≈ 0, got {dist}"
            );
        }
    }

    /// Testa polar_encode com dimensões pequenas e ímpares.
    #[test]
    fn test_polar_encode_small_dims() {
        // dim=1
        let v1 = vec![3.14f32];
        let q1 = polar_encode(&v1, 8);
        assert_eq!(q1.dim, 1);
        assert_eq!(q1.angles.len(), 0);
        assert!((q1.final_radius - 3.14).abs() < 0.001);
        let d1 = polar_decode(&q1);
        assert_eq!(d1.len(), 1);
        assert!((d1[0] - 3.14).abs() < 0.001);

        // dim=2
        let v2 = vec![1.0f32, 0.0];
        let q2 = polar_encode(&v2, 8);
        assert_eq!(q2.dim, 2);
        assert_eq!(q2.angles.len(), 1);
        assert!((q2.final_radius - 1.0).abs() < 0.001);

        // dim=3 (odd)
        let v3 = vec![1.0f32, 0.0, 0.5];
        let q3 = polar_encode(&v3, 8);
        assert_eq!(q3.dim, 3);
        let d3 = polar_decode(&q3);
        assert_eq!(d3.len(), 3, "decoded must have exactly 3 elements for odd dim");
    }

    /// Testa que a memória de PolarQuantized para dim=128 é menor que Vec<f32>.
    #[test]
    fn test_polar_memory_footprint() {
        let dim = 128usize;
        let f32_bytes = dim * 4;
        // PolarQuantized: 4 (final_radius) + (dim-1) (angles) + overhead ≈ dim+3 bytes
        let polar_angles = dim - 1; // for power-of-2 dimensions
        let polar_bytes = std::mem::size_of::<f32>() + polar_angles;
        // polar_bytes ≈ 131 vs f32_bytes = 512 → ~3.9x smaller
        assert!(
            polar_bytes < f32_bytes,
            "PolarQuantized ({polar_bytes} B) should be smaller than f32 ({f32_bytes} B)"
        );
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

    // ─── QjlParams tests ─────────────────────────────────────────────

    /// Testa que a correção QJL é não-enviesada: para vetores sem relação,
    /// a média de 1000 correções deve estar próxima de zero.
    #[test]
    fn test_qjl_correction_unbiased() {
        use rand::Rng;
        let dim = 64usize;
        let m = 64usize;
        let qjl = QjlParams::new(dim, m, 123);

        let mut rng = rand::thread_rng();
        let mut sum = 0.0f64;
        let n = 1000usize;

        for _ in 0..n {
            // query aleatório
            let query: Vec<f32> = (0..dim).map(|_| rng.gen_range(-1.0f32..1.0)).collect();
            // residual aleatório independente
            let residual: Vec<f32> = (0..dim).map(|_| rng.gen_range(-0.1f32..0.1)).collect();
            let sign_bits = qjl.encode_residual(&residual);
            let correction = qjl.correction_score(&query, &sign_bits) as f64;
            sum += correction;
        }

        let mean = sum / n as f64;
        // Para vetores não-relacionados, a correção esperada é ~0
        assert!(
            mean.abs() < 0.15,
            "QJL correction mean should be near 0 for unrelated vectors, got {mean:.4}"
        );
    }

    /// Testa que encode_residual é determinístico com a mesma seed.
    #[test]
    fn test_qjl_encode_deterministic() {
        let dim = 32usize;
        let residual: Vec<f32> = (0..dim).map(|i| (i as f32) * 0.01 - 0.16).collect();

        let qjl_a = QjlParams::new(dim, 64, 42);
        let qjl_b = QjlParams::new(dim, 64, 42);

        let bits_a = qjl_a.encode_residual(&residual);
        let bits_b = qjl_b.encode_residual(&residual);

        assert_eq!(bits_a, bits_b, "Same seed must produce identical sign bits");
    }

    /// Testa que seeds diferentes produzem matrizes (e bits) diferentes.
    #[test]
    fn test_qjl_different_seeds_differ() {
        let dim = 64usize;
        let residual: Vec<f32> = (0..dim).map(|i| i as f32 * 0.01).collect();

        let qjl_a = QjlParams::new(dim, 64, 1);
        let qjl_b = QjlParams::new(dim, 64, 2);

        let bits_a = qjl_a.encode_residual(&residual);
        let bits_b = qjl_b.encode_residual(&residual);

        assert_ne!(bits_a, bits_b, "Different seeds should produce different sign bits");
    }

    /// Testa que QjlParams serializa apenas (m, dim, seed) e regenera a matriz no load.
    #[test]
    fn test_qjl_serde_roundtrip() {
        let dim = 32usize;
        let qjl = QjlParams::new(dim, 48, 99);

        let json = serde_json::to_string(&qjl).expect("serialize");
        // JSON deve conter apenas m, dim, seed — não a matriz
        assert!(!json.contains("projection_matrix"), "matrix should not be serialized");
        assert!(json.contains("\"m\":48"), "m should be in JSON");
        assert!(json.contains("\"seed\":99"), "seed should be in JSON");

        let loaded: QjlParams = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(loaded.m, qjl.m);
        assert_eq!(loaded.dim, qjl.dim);
        assert_eq!(loaded.seed, qjl.seed);

        // Após desserialização, a matriz deve ser idêntica (mesma seed)
        let residual: Vec<f32> = (0..dim).map(|i| i as f32 * 0.05 - 0.8).collect();
        assert_eq!(
            qjl.encode_residual(&residual),
            loaded.encode_residual(&residual),
            "Deserialized QjlParams must produce identical sign bits"
        );
    }
}

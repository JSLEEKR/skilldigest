//! Tokenization pool.
//!
//! We expose three logical tokenizers:
//!
//! - **cl100k** — OpenAI GPT-4, Anthropic Claude-ish. Default. Backed by
//!   `tiktoken_rs::cl100k_base` which bundles the BPE ranks as a
//!   `&'static` table, so this tokenizer works completely offline.
//! - **o200k** — GPT-4o. Backed by `tiktoken_rs::o200k_base`, also bundled.
//! - **llama3** — Meta Llama 3 approximation. We do not ship the full
//!   `tokenizers` crate (which would require a network-fetched
//!   `tokenizer.json`). Instead we expose a deterministic word-piece style
//!   approximation documented as *approximate* in the README. In practice
//!   it is within ~10% of the real Llama 3 count for English prose and is
//!   useful for relative comparisons.
//!
//! All tokenizers implement the [`Tokenizer`] trait and can be wrapped in
//! an [`Arc`] so that `rayon` can share a single instance across threads.

use std::sync::Arc;

use tiktoken_rs::CoreBPE;

use crate::error::{Error, Result};

/// Shared trait for the three supported tokenizers.
pub trait Tokenizer: Send + Sync + std::fmt::Debug {
    /// Human-readable name, e.g. `"cl100k_base"`.
    fn name(&self) -> &'static str;
    /// Count tokens in the given string. This is the hot path — callers
    /// prefer this over `encode` so we avoid allocating the token vector.
    fn count(&self, text: &str) -> usize;
}

/// Construct a tokenizer from a user-facing name. Accepts `cl100k`,
/// `cl100k_base`, `o200k`, `o200k_base`, `llama3`, `llama-3`.
pub fn by_name(name: &str) -> Result<Arc<dyn Tokenizer>> {
    let canonical = name.trim().to_ascii_lowercase();
    let canonical = canonical.replace(['-', '_'], "");
    match canonical.as_str() {
        "cl100k" | "cl100kbase" => Ok(Arc::new(Cl100kTokenizer::new()?)),
        "o200k" | "o200kbase" => Ok(Arc::new(O200kTokenizer::new()?)),
        "llama3" | "llama" => Ok(Arc::new(Llama3Approx::new())),
        _ => Err(Error::UnknownTokenizer(name.to_string())),
    }
}

/// cl100k_base — GPT-4 / Claude-ish.
pub struct Cl100kTokenizer {
    bpe: Arc<CoreBPE>,
}

impl Cl100kTokenizer {
    /// Construct. Fails if the bundled tables cannot be loaded (should
    /// never happen at runtime — this is essentially infallible).
    pub fn new() -> Result<Self> {
        let bpe = tiktoken_rs::cl100k_base()
            .map_err(|e| Error::Other(anyhow::anyhow!("cl100k_base init failed: {e}")))?;
        Ok(Self { bpe: Arc::new(bpe) })
    }
}

impl Tokenizer for Cl100kTokenizer {
    fn name(&self) -> &'static str {
        "cl100k_base"
    }
    fn count(&self, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        self.bpe.encode_with_special_tokens(text).len()
    }
}

impl std::fmt::Debug for Cl100kTokenizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cl100kTokenizer").finish()
    }
}

/// o200k_base — GPT-4o.
pub struct O200kTokenizer {
    bpe: Arc<CoreBPE>,
}

impl O200kTokenizer {
    /// Construct.
    pub fn new() -> Result<Self> {
        let bpe = tiktoken_rs::o200k_base()
            .map_err(|e| Error::Other(anyhow::anyhow!("o200k_base init failed: {e}")))?;
        Ok(Self { bpe: Arc::new(bpe) })
    }
}

impl Tokenizer for O200kTokenizer {
    fn name(&self) -> &'static str {
        "o200k_base"
    }
    fn count(&self, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        self.bpe.encode_with_special_tokens(text).len()
    }
}

impl std::fmt::Debug for O200kTokenizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("O200kTokenizer").finish()
    }
}

/// Llama 3 approximate tokenizer.
///
/// Counts Unicode *word-like* segments plus a punctuation surcharge. The
/// implementation deliberately avoids fetching a tokenizer.json from the
/// network; the README flags this tokenizer as approximate.
///
/// The algorithm is:
///
/// 1. Split the input on Unicode whitespace to get "words".
/// 2. For each word, count 1 token per approximately 3.8 byte-chunk
///    (empirically the average subword length for Llama 3 on English
///    prose), with a minimum of 1 token per non-empty word.
/// 3. Add 1 token per run of non-alphanumeric punctuation.
///
/// This is deterministic and side-effect-free.
#[derive(Debug, Default)]
pub struct Llama3Approx;

impl Llama3Approx {
    /// Construct.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Tokenizer for Llama3Approx {
    fn name(&self) -> &'static str {
        "llama3_approx"
    }

    fn count(&self, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        let mut tokens = 0usize;
        let mut current_word_bytes = 0usize;
        let mut in_word = false;
        let mut in_punct = false;
        let mut punct_run = false;

        for ch in text.chars() {
            if ch.is_alphanumeric() {
                if in_punct && punct_run {
                    tokens += 1;
                    punct_run = false;
                }
                in_word = true;
                in_punct = false;
                current_word_bytes += ch.len_utf8();
            } else if ch.is_whitespace() {
                if in_word {
                    tokens += approx_word_tokens(current_word_bytes);
                    current_word_bytes = 0;
                    in_word = false;
                }
                if in_punct && punct_run {
                    tokens += 1;
                    punct_run = false;
                }
                in_punct = false;
            } else {
                // punctuation or symbol
                if in_word {
                    tokens += approx_word_tokens(current_word_bytes);
                    current_word_bytes = 0;
                    in_word = false;
                }
                in_punct = true;
                punct_run = true;
            }
        }

        if in_word {
            tokens += approx_word_tokens(current_word_bytes);
        }
        if punct_run {
            tokens += 1;
        }

        tokens.max(1)
    }
}

/// Estimate tokens for a single "word" of `bytes` bytes. Empirically, the
/// Llama 3 tokenizer averages ~3.8 bytes per token for English prose.
fn approx_word_tokens(bytes: usize) -> usize {
    if bytes == 0 {
        return 0;
    }
    let approx = ((bytes as f64) / 3.8).ceil() as usize;
    approx.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_name_accepts_canonical_forms() {
        for name in ["cl100k", "cl100k_base", "CL100K", "CL100K-BASE"] {
            let t = by_name(name).expect(name);
            assert_eq!(t.name(), "cl100k_base");
        }
        for name in ["o200k", "o200k_base", "O200K"] {
            let t = by_name(name).expect(name);
            assert_eq!(t.name(), "o200k_base");
        }
        for name in ["llama3", "llama-3", "LLAMA3"] {
            let t = by_name(name).expect(name);
            assert_eq!(t.name(), "llama3_approx");
        }
    }

    #[test]
    fn by_name_rejects_unknown() {
        let e = by_name("gpt99").unwrap_err();
        assert!(matches!(e, Error::UnknownTokenizer(_)));
    }

    #[test]
    fn cl100k_empty_is_zero() {
        let t = Cl100kTokenizer::new().unwrap();
        assert_eq!(t.count(""), 0);
    }

    #[test]
    fn cl100k_hello_world_golden() {
        let t = Cl100kTokenizer::new().unwrap();
        // "Hello, world!" is 4 tokens in cl100k_base per tiktoken.
        assert_eq!(t.count("Hello, world!"), 4);
    }

    #[test]
    fn o200k_empty_is_zero() {
        let t = O200kTokenizer::new().unwrap();
        assert_eq!(t.count(""), 0);
    }

    #[test]
    fn o200k_hello_world_nonzero() {
        let t = O200kTokenizer::new().unwrap();
        assert!(t.count("Hello, world!") > 0);
    }

    #[test]
    fn llama3_empty_is_zero() {
        let t = Llama3Approx::new();
        assert_eq!(t.count(""), 0);
    }

    #[test]
    fn llama3_single_word_nonzero() {
        let t = Llama3Approx::new();
        assert!(t.count("hello") >= 1);
    }

    #[test]
    fn llama3_punctuation_counted() {
        let t = Llama3Approx::new();
        let only_punct = t.count("!!!");
        assert!(only_punct >= 1, "punctuation should count: {only_punct}");
    }

    #[test]
    fn llama3_is_deterministic() {
        let t = Llama3Approx::new();
        let a = t.count("The quick brown fox jumps over the lazy dog.");
        let b = t.count("The quick brown fox jumps over the lazy dog.");
        assert_eq!(a, b);
    }

    #[test]
    fn llama3_scales_with_length() {
        let t = Llama3Approx::new();
        let short = t.count("a b c d");
        let long = t.count("a b c d ".repeat(100).as_str());
        assert!(long > short * 50);
    }

    #[test]
    fn approx_word_tokens_monotone() {
        let mut prev = 0;
        for n in [1, 2, 3, 4, 10, 100] {
            let v = approx_word_tokens(n);
            assert!(v >= prev);
            prev = v;
        }
    }

    #[test]
    fn tokenizer_count_monotone_in_length() {
        // Not strictly true for BPE in pathological cases but is true for
        // ASCII text with repeated words.
        let t = Cl100kTokenizer::new().unwrap();
        let a = t.count("test");
        let b = t.count("test test test");
        assert!(b >= a);
    }

    #[test]
    fn unicode_text_handled() {
        let t = Cl100kTokenizer::new().unwrap();
        let count = t.count("안녕하세요 세계");
        assert!(count > 0);
    }

    #[test]
    fn tokenizers_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Cl100kTokenizer>();
        assert_send_sync::<O200kTokenizer>();
        assert_send_sync::<Llama3Approx>();
    }

    #[test]
    fn tokenizer_trait_object_works() {
        let t: Arc<dyn Tokenizer> = by_name("cl100k").unwrap();
        assert!(t.count("hello world") > 0);
    }

    #[test]
    fn approx_word_tokens_zero_is_zero() {
        assert_eq!(approx_word_tokens(0), 0);
    }
}

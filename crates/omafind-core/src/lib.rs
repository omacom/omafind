//! Shared types for Omafind: prompts, model pins, paths, the kinds of files and chunks, and ranking.

pub mod config;
pub mod paths;
pub mod query;
pub mod ranking;
pub mod types;

pub use types::*;

/// EmbeddingGemma 2's task prompts (model card). Media inputs take no prompt.
pub mod prompts {
    /// A search query.
    pub fn query(text: &str) -> String {
        format!("task: search result | query: {text}")
    }

    /// A search of the code index: the model card's prompt for finding code by what it does.
    pub fn code_query(text: &str) -> String {
        format!("task: code retrieval | query: {text}")
    }

    /// A document, or a passage of one. `title` may be empty.
    pub fn document(title: Option<&str>, text: &str) -> String {
        let title = match title {
            Some(title) if !title.is_empty() => title,
            _ => "none",
        };
        format!("title: {title} | text: {text}")
    }
}

/// The EmbeddingGemma 2 files Omafind runs: ggml-org's Q8_0 GGUF conversion at one pinned revision.
pub mod model {
    pub const REPOSITORY: &str = "ggml-org/embeddinggemma-2-GGUF";
    pub const REVISION: &str = "bfcd298762cc34d0357ece5ebdd31791a3a374d8";
    /// The text model (310 MB): all that queries need.
    pub const TEXT_FILE: &str = "embeddinggemma-2-Q8_0.gguf";
    /// The image and audio encoders (555 MB).
    pub const MEDIA_FILE: &str = "mmproj-embeddinggemma-2-Q8_0.gguf";
    pub const DIM: usize = 768;
    /// Which vectors the pipeline makes (model files, engine, image sizing, prompts). Bumped only when the golden
    /// check shows an engine update changed them; every index then embeds again.
    pub const VECTOR_VERSION: u32 = 1;

    /// Where the model files live: `$OMAFIND_MODELS`, else `~/.local/share/omafind/models`.
    pub fn dir() -> std::path::PathBuf {
        if let Some(dir) = std::env::var_os("OMAFIND_MODELS") {
            return dir.into();
        }
        super::paths::data_dir().join("models")
    }
}

#[cfg(test)]
mod tests {
    use super::prompts;

    #[test]
    fn prompts_match_the_model_card() {
        assert_eq!(prompts::query("dog"), "task: search result | query: dog");
        assert_eq!(prompts::document(None, "x"), "title: none | text: x");
        assert_eq!(prompts::document(Some(""), "x"), "title: none | text: x");
        assert_eq!(
            prompts::document(Some("Lease"), "x"),
            "title: Lease | text: x"
        );
    }
}

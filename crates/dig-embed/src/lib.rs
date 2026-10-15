//! EmbeddingGemma 2 on llama.cpp, in this process: the image and audio encoders (mtmd) feed the text model, and its
//! mean-pooled, L2-normalized output is the vector.
//!
//! Inputs are framed as the reference (mlx-vlm, transformers) frames them: `<bos> text <eos>`,
//! `<bos> <|image> soft tokens <image|> <eos>` and the same for audio. The model is non-causal with no KV cache, so
//! each input, token ids and encoder rows together, goes through one `llama_process` call. Audio past 30 s is dropped,
//! as in the reference.

pub mod sizing;

#[allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code, unsafe_op_in_unsafe_fn, unnecessary_transmutes)]
pub mod sys {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Once;

use anyhow::{Context, Result, anyhow, bail};

/// The llama.cpp version this crate was built against (`pkg-config --modversion llama`), for logs.
pub const LLAMA_VERSION: &str = env!("DIG_LLAMA_VERSION");

/// Where the model files are.
#[derive(Debug, Clone)]
pub struct Models {
    pub text: PathBuf,
    /// None while only the text part is there.
    pub media: Option<PathBuf>,
}

impl Models {
    /// The files in `dir` (see `dig_core::model::dir`). Missing text model is an error; missing media is text-only.
    pub fn locate(dir: &Path) -> Result<Self> {
        let text = dir.join(dig_core::model::TEXT_FILE);
        if !text.exists() {
            bail!("no {} in {}: run scripts/fetch-models.sh", dig_core::model::TEXT_FILE, dir.display());
        }
        let media = dir.join(dig_core::model::MEDIA_FILE);
        Ok(Self { text, media: media.exists().then_some(media) })
    }
}

/// How the model runs.
#[derive(Debug, Clone)]
pub struct Options {
    /// Just the text model (queries): 512-token inputs, ~0.3 GB.
    pub text_only: bool,
    /// Leave the GPU out even when a backend finds one.
    pub cpu_only: bool,
    /// Threads for CPU work. 0 = every logical processor up to 8.
    pub threads: i32,
}

impl Default for Options {
    fn default() -> Self {
        Self { text_only: false, cpu_only: false, threads: 0 }
    }
}

/// One input's vector, or why it has none (an unreadable file must not sink its neighbours).
#[derive(Debug, Clone)]
pub struct Embedding {
    pub vector: Option<Vec<f32>>,
    pub error: Option<String>,
}

static BACKEND: Once = Once::new();
static mut GPU_FOUND: bool = false;
static mut BACKEND_NAMES: Vec<String> = Vec::new();

unsafe extern "C" fn log_callback(level: sys::ggml_log_level, text: *const std::os::raw::c_char, _: *mut std::ffi::c_void) {
    // Warnings and errors only; llama.cpp logs a lot at load time. CONT continues the previous message.
    static mut LAST: sys::ggml_log_level = sys::GGML_LOG_LEVEL_INFO;
    unsafe {
        if level != sys::GGML_LOG_LEVEL_CONT {
            LAST = level;
        }
        if LAST >= sys::GGML_LOG_LEVEL_WARN && !text.is_null() {
            eprint!("{}", CStr::from_ptr(text).to_string_lossy());
        }
    }
}

fn is_gpu(device: sys::ggml_backend_dev_t) -> bool {
    let kind = unsafe { sys::ggml_backend_dev_type(device) };
    kind == sys::GGML_BACKEND_DEVICE_TYPE_GPU || kind == sys::GGML_BACKEND_DEVICE_TYPE_IGPU
}

/// llama.cpp's process-wide setup, once: the log, the backend modules and the backend init. Upstream's prebuilt
/// engine ships its backends (libggml-vulkan.so, libggml-cpu-*.so) as modules beside libggml: they're loaded from
/// `$DIG_BACKENDS` if set, else from the engine directory this crate was built against, else from wherever ggml
/// looks on its own (a packaged ggml, or one with the backend compiled in).
fn init_backend(cpu_only: bool) {
    BACKEND.call_once(|| unsafe {
        sys::llama_log_set(Some(log_callback), ptr::null_mut());
        sys::mtmd_helper_log_set(Some(log_callback), ptr::null_mut());
        let chosen = std::env::var_os("DIG_BACKENDS").filter(|v| !v.is_empty()).map(PathBuf::from);
        let built_against = Some(PathBuf::from(env!("DIG_ENGINE_LIB"))).filter(|p| !p.as_os_str().is_empty());
        for folder in chosen.into_iter().chain(built_against) {
            if sys::ggml_backend_dev_count() > 0 {
                break;
            }
            let path = CString::new(folder.to_string_lossy().as_bytes()).unwrap();
            sys::ggml_backend_load_all_from_path(path.as_ptr());
        }
        if sys::ggml_backend_dev_count() == 0 {
            sys::ggml_backend_load_all();
        }
        GPU_FOUND = (0..sys::ggml_backend_dev_count()).any(|i| is_gpu(sys::ggml_backend_dev_get(i)));
        if cpu_only {
            // Unloaded rather than just unused: a GPU backend would still set itself up (Vulkan: seconds).
            for index in (0..sys::ggml_backend_reg_count()).rev() {
                let registry = sys::ggml_backend_reg_get(index);
                let has_gpu = (0..sys::ggml_backend_reg_dev_count(registry))
                    .any(|i| is_gpu(sys::ggml_backend_reg_dev_get(registry, i)));
                if has_gpu {
                    sys::ggml_backend_unload(registry);
                }
            }
        }
        BACKEND_NAMES = (0..sys::ggml_backend_reg_count())
            .map(|i| CStr::from_ptr(sys::ggml_backend_reg_name(sys::ggml_backend_reg_get(i))).to_string_lossy().into_owned())
            .collect();
        sys::llama_backend_init();
    });
}

/// The ggml backends that loaded (after `Embedder::new`), e.g. `["Vulkan", "CPU"]`.
pub fn backend_names() -> Vec<String> {
    unsafe { (*ptr::addr_of!(BACKEND_NAMES)).clone() }
}

/// A GPU llama.cpp can run on here, before `cpu_only` leaves it out.
pub fn gpu_found() -> bool {
    unsafe { GPU_FOUND }
}

/// Loads the backends without a model: (their names, whether one is a GPU). For `dig doctor`.
pub fn probe() -> (Vec<String>, bool) {
    init_backend(false);
    (backend_names(), gpu_found())
}

/// EmbeddingGemma 2 loaded: text model, optional media encoders, one reusable batch. One thread at a time; dropping
/// it frees the model (about 1 GB, or 0.25 GB text-only).
pub struct Embedder {
    model: *mut sys::llama_model,
    context: *mut sys::llama_context,
    media: *mut sys::mtmd_context,
    batch: *mut sys::llama_batch_ext,
    vocab: *const sys::llama_vocab,
    /// 768: the output projection.
    pub dim: usize,
    /// The width of encoder rows fed to the text model.
    row_size: usize,
    max_tokens: usize,
    sequences: i32,
    tokens: usize,
    position: sys::llama_pos,
    pub text_only: bool,
    pub load_seconds: f64,
}

const MAX_SEQUENCES: i32 = 16;

// The llama.cpp handles may move between threads; they are never used from two at once (every caller holds
// `&mut self`, and the daemon keeps its embedders behind a mutex).
unsafe impl Send for Embedder {}

impl Embedder {
    pub fn new(models: &Models, options: &Options) -> Result<Self> {
        let started = std::time::Instant::now();
        init_backend(options.cpu_only);
        if !options.text_only && models.media.is_none() {
            bail!("{} (images and audio) isn't downloaded yet", dig_core::model::MEDIA_FILE);
        }
        let threads = if options.threads > 0 {
            options.threads
        } else {
            std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8) as i32
        };
        unsafe {
            let mut model_params = sys::llama_model_default_params();
            model_params.n_gpu_layers = if options.cpu_only { 0 } else { 999 };
            let text_path = CString::new(models.text.to_string_lossy().as_bytes())?;
            let model = sys::llama_model_load_from_file(text_path.as_ptr(), model_params);
            if model.is_null() {
                bail!("can't load {}", models.text.display());
            }
            // One input must fit one micro-batch: 2048 tokens (the reference's cut), which also holds a 1120-token
            // image or a 750-token 30 s clip. Queries are short, so the query encoder reserves less and cuts at 512.
            let max_tokens: usize = if options.text_only { 512 } else { 2048 };
            let mut params = sys::llama_context_default_params();
            params.n_ctx = max_tokens as u32;
            params.n_batch = max_tokens as u32;
            params.n_ubatch = max_tokens as u32;
            params.n_seq_max = MAX_SEQUENCES as u32;
            params.kv_unified = true; // each input may use the whole context, as in llama-embedding
            params.embeddings = true;
            params.pooling_type = sys::LLAMA_POOLING_TYPE_MEAN;
            params.n_threads = threads;
            params.n_threads_batch = threads;
            let context = sys::llama_init_from_model(model, params);
            if context.is_null() {
                sys::llama_model_free(model);
                bail!("can't create a llama.cpp context");
            }
            let mut media = ptr::null_mut();
            if let (Some(media_file), false) = (&models.media, options.text_only) {
                let mut media_params = sys::mtmd_context_params_default();
                media_params.use_gpu = !options.cpu_only;
                media_params.print_timings = false;
                media_params.warmup = true;
                media_params.image_min_tokens = 1; // never rescale: images arrive at their final size (sizing)
                media_params.image_max_tokens = 1120;
                media_params.n_threads = threads;
                let media_path = CString::new(media_file.to_string_lossy().as_bytes())?;
                media = sys::mtmd_init_from_file(media_path.as_ptr(), model, media_params);
                if media.is_null() {
                    sys::llama_free(context);
                    sys::llama_model_free(model);
                    bail!("can't load {}", media_file.display());
                }
            }
            let mut embedder = Self {
                model,
                context,
                media,
                batch: sys::llama_batch_ext_init(context),
                vocab: sys::llama_model_get_vocab(model),
                dim: sys::llama_model_n_embd_out(model) as usize,
                row_size: sys::llama_model_n_embd_inp(model) as usize,
                max_tokens,
                sequences: 0,
                tokens: 0,
                position: 0,
                text_only: options.text_only,
                load_seconds: 0.0,
            };
            // The first calls build the GPU pipelines: pay that now.
            for _ in 0..2 {
                embedder.embed_texts(&[dig_core::prompts::query("warm up")])?;
            }
            embedder.load_seconds = started.elapsed().as_secs_f64();
            Ok(embedder)
        }
    }

    /// One L2-normalized vector per text, in order. Texts are batched up to 16 at a time within `max_tokens`.
    pub fn embed_texts(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.reset();
        let mut vectors = Vec::with_capacity(texts.len());
        for text in texts {
            let tokens = self.tokenize(text);
            if self.sequences == MAX_SEQUENCES || self.tokens + tokens.len() > self.max_tokens {
                vectors.extend(self.run()?);
            }
            self.begin();
            for token in tokens {
                self.add_token(token)?;
            }
        }
        if self.sequences > 0 {
            vectors.extend(self.run()?);
        }
        Ok(vectors)
    }

    /// Images at `budget` soft tokens (one of `sizing::BUDGETS`).
    pub fn embed_images(&mut self, paths: &[PathBuf], budget: usize) -> Result<Vec<Embedding>> {
        if self.media.is_null() {
            bail!("this embedder was started text-only");
        }
        if !sizing::BUDGETS.contains(&budget) {
            bail!("budget must be one of {:?}", sizing::BUDGETS);
        }
        Ok(paths.iter().map(|path| self.embedding(|s| s.embed_media(path, true, budget))).collect())
    }

    /// Audio files (wav, mp3, flac), the first 30 s of each.
    pub fn embed_audio(&mut self, paths: &[PathBuf]) -> Result<Vec<Embedding>> {
        if self.media.is_null() {
            bail!("this embedder was started text-only");
        }
        Ok(paths.iter().map(|path| self.embedding(|s| s.embed_media(path, false, 0))).collect())
    }

    fn embedding(&mut self, make: impl FnOnce(&mut Self) -> Result<Vec<f32>>) -> Embedding {
        match make(self) {
            Ok(vector) => Embedding { vector: Some(vector), error: None },
            Err(error) => {
                let mut message = format!("{error:#}");
                message.truncate(300);
                Embedding { vector: None, error: Some(message) }
            }
        }
    }

    fn embed_media(&mut self, path: &Path, image: bool, budget: usize) -> Result<Vec<f32>> {
        unsafe {
            let bitmap = self.load(path, image, budget)?;
            let chunks = sys::mtmd_input_chunks_init();
            let result = (|| -> Result<Vec<f32>> {
                let marker = sys::mtmd_default_marker();
                let text = sys::mtmd_input_text {
                    text: marker,
                    text_len: CStr::from_ptr(marker).to_bytes().len(),
                    add_special: true,
                    parse_special: true,
                };
                let bitmaps: [*const sys::mtmd_bitmap; 1] = [bitmap];
                let status = sys::mtmd_tokenize(self.media, chunks, &text, bitmaps.as_ptr(), 1);
                if status != 0 {
                    bail!("can't tokenize {} ({status})", path.display());
                }
                self.reset();
                self.begin();
                for index in 0..sys::mtmd_input_chunks_size(chunks) {
                    let chunk = sys::mtmd_input_chunks_get(chunks, index);
                    if sys::mtmd_input_chunk_get_type(chunk) == sys::MTMD_INPUT_CHUNK_TYPE_TEXT {
                        let mut count: usize = 0;
                        let tokens = sys::mtmd_input_chunk_get_tokens_text(chunk, &mut count);
                        if tokens.is_null() {
                            continue;
                        }
                        for i in 0..count {
                            self.add_token(*tokens.add(i))?;
                        }
                    } else {
                        if sys::mtmd_encode_chunk(self.media, chunk) != 0 {
                            bail!("the encoder failed on {}", path.display());
                        }
                        let rows = sys::mtmd_get_output_embd(self.media);
                        if rows.is_null() {
                            bail!("the encoder failed on {}", path.display());
                        }
                        self.add_rows(rows, sys::mtmd_input_chunk_get_n_tokens(chunk))?;
                    }
                }
                let mut vectors = self.run()?;
                vectors.pop().ok_or_else(|| anyhow!("no vector for {}", path.display()))
            })();
            sys::mtmd_input_chunks_free(chunks);
            sys::mtmd_bitmap_free(bitmap);
            result
        }
    }

    /// Reads an image or audio file with mtmd's helpers (stb_image, miniaudio at 16 kHz), resizes images for `budget`
    /// and cuts audio at 30 s. The caller frees the bitmap.
    unsafe fn load(&mut self, path: &Path, image: bool, budget: usize) -> Result<*mut sys::mtmd_bitmap> {
        unsafe {
            let c_path = CString::new(path.to_string_lossy().as_bytes())?;
            let wrapper = sys::mtmd_helper_bitmap_init_from_file(self.media, c_path.as_ptr(), false, sys::mtmd_helper_init_opt_default());
            if !wrapper.video_ctx.is_null() {
                if !wrapper.bitmap.is_null() {
                    sys::mtmd_bitmap_free(wrapper.bitmap);
                }
                sys::mtmd_helper_video_free(wrapper.video_ctx);
                bail!("video goes in as frames and sound, not as a file: {}", path.display());
            }
            let bitmap = wrapper.bitmap;
            if bitmap.is_null() {
                bail!("can't read {}", path.display());
            }
            if sys::mtmd_bitmap_is_audio(bitmap) == image {
                sys::mtmd_bitmap_free(bitmap);
                bail!("not {}: {}", if image { "an image" } else { "audio" }, path.display());
            }
            if image {
                let (width, height) = (sys::mtmd_bitmap_get_nx(bitmap) as usize, sys::mtmd_bitmap_get_ny(bitmap) as usize);
                let Some((th, tw)) = sizing::target_size(height, width, budget) else {
                    sys::mtmd_bitmap_free(bitmap);
                    bail!("image too thin to embed: {}", path.display());
                };
                if (th, tw) == (height, width) {
                    return Ok(bitmap);
                }
                let data = std::slice::from_raw_parts(sys::mtmd_bitmap_get_data(bitmap), width * height * 3);
                let pixels = sizing::resize_bicubic(data, width, height, tw, th);
                sys::mtmd_bitmap_free(bitmap);
                let resized = sys::mtmd_bitmap_init(tw as u32, th as u32, pixels.as_ptr());
                if resized.is_null() {
                    bail!("can't resize {}", path.display());
                }
                return Ok(resized);
            }
            let samples = sys::mtmd_bitmap_get_n_bytes(bitmap) / std::mem::size_of::<f32>();
            let limit = 30 * sys::mtmd_get_audio_sample_rate(self.media) as usize;
            if samples <= limit {
                return Ok(bitmap);
            }
            let prefix = sys::mtmd_bitmap_init_from_audio(limit, sys::mtmd_bitmap_get_data(bitmap) as *const f32);
            sys::mtmd_bitmap_free(bitmap);
            if prefix.is_null() {
                bail!("can't cut {} at 30 s", path.display());
            }
            Ok(prefix)
        }
    }

    /// `<bos> text <eos>`, cut to `max_tokens` the way transformers cuts (keeping both special tokens).
    fn tokenize(&self, text: &str) -> Vec<sys::llama_token> {
        unsafe {
            let bytes = text.as_bytes();
            let mut tokens: Vec<sys::llama_token> = vec![0; bytes.len() + 8];
            let mut count = sys::llama_tokenize(
                self.vocab, bytes.as_ptr() as *const _, bytes.len() as i32, tokens.as_mut_ptr(), tokens.len() as i32, true, true,
            );
            if count < 0 {
                tokens = vec![0; (-count) as usize];
                count = sys::llama_tokenize(
                    self.vocab, bytes.as_ptr() as *const _, bytes.len() as i32, tokens.as_mut_ptr(), tokens.len() as i32, true, true,
                );
            }
            tokens.truncate(count.max(0) as usize);
            if tokens.len() > self.max_tokens {
                tokens.truncate(self.max_tokens - 1);
                tokens.push(sys::llama_vocab_eos(self.vocab));
            }
            tokens
        }
    }

    /// Starts the next input in the batch (each input is its own sequence).
    fn begin(&mut self) {
        self.sequences += 1;
        self.position = 0;
    }

    /// Drops everything added so far, including an input that failed halfway.
    fn reset(&mut self) {
        unsafe { sys::llama_batch_ext_clear(self.batch) };
        self.sequences = 0;
        self.tokens = 0;
    }

    fn add_token(&mut self, token: sys::llama_token) -> Result<()> {
        let index = unsafe { sys::llama_batch_ext_add_token(self.batch, self.sequences - 1, token) };
        self.place(index)
    }

    unsafe fn add_rows(&mut self, rows: *const f32, count: usize) -> Result<()> {
        for row in 0..count {
            let embd = sys::llama_embd { data: unsafe { rows.add(row * self.row_size) }, n_rows: 1, n_embd: self.row_size };
            let index = unsafe { sys::llama_batch_ext_add_embd(self.batch, self.sequences - 1, embd) }; // copied by the batch
            self.place(index)?;
        }
        Ok(())
    }

    fn place(&mut self, index: i32) -> Result<()> {
        if index < 0 {
            bail!("batch full ({index})");
        }
        let pos: [sys::llama_pos; 4] = [self.position, 0, 0, 0];
        unsafe {
            if !sys::llama_batch_ext_set_pos(self.batch, index, pos.as_ptr()) || !sys::llama_batch_ext_set_output_embd(self.batch, index, true) {
                bail!("can't place token {index}");
            }
        }
        self.position += 1;
        self.tokens += 1;
        Ok(())
    }

    /// Runs the text model over the batch and returns one L2-normalized vector per input, in order.
    fn run(&mut self) -> Result<Vec<Vec<f32>>> {
        let result = unsafe {
            let status = sys::llama_process(self.context, sys::LLAMA_PROCESS_TYPE_ENCODE, self.batch);
            if status != 0 {
                Err(anyhow!("llama_process failed ({status})"))
            } else {
                (0..self.sequences)
                    .map(|sequence| {
                        let pooled = sys::llama_get_embeddings_seq(self.context, sequence);
                        if pooled.is_null() {
                            bail!("no pooled embedding for input {sequence}");
                        }
                        let raw = std::slice::from_raw_parts(pooled, self.dim);
                        let sum: f64 = raw.iter().map(|&v| v as f64 * v as f64).sum();
                        let scale = if sum > 0.0 { (1.0 / sum.sqrt()) as f32 } else { 0.0 };
                        Ok(raw.iter().map(|&v| v * scale).collect())
                    })
                    .collect()
            }
        };
        self.reset();
        result
    }
}

impl Drop for Embedder {
    fn drop(&mut self) {
        unsafe {
            sys::llama_batch_ext_free(self.batch);
            if !self.media.is_null() {
                sys::mtmd_free(self.media);
            }
            sys::llama_free(self.context);
            sys::llama_model_free(self.model);
        }
    }
}

/// Cosine of two normalized vectors (a plain dot product).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The models in `dig_core::model::dir()`.
pub fn default_models() -> Result<Models> {
    Models::locate(&dig_core::model::dir()).context("locating the model files")
}

# Third-party notices

Omafind's own code is MIT (see `LICENSE`). It runs on the work below, which keeps its own license.

## EmbeddingGemma 2

The model that reads text, pictures and sound is **EmbeddingGemma 2** by Google DeepMind.

- Source model: <https://huggingface.co/google/embeddinggemma-2>
- Files Omafind uses: the GGUF conversion by ggml-org, `ggml-org/embeddinggemma-2-GGUF` at revision
  `bfcd298762cc34d0357ece5ebdd31791a3a374d8` (`embeddinggemma-2-Q8_0.gguf`, `mmproj-embeddinggemma-2-Q8_0.gguf`)
- License: Apache License 2.0 (<https://www.apache.org/licenses/LICENSE-2.0>; Google's page for it:
  <https://ai.google.dev/gemma/docs/gemma_4_license>)
- Use is also subject to the Gemma Prohibited Use Policy: <https://ai.google.dev/gemma/prohibited_use_policy>

Omafind does not include or redistribute the model. `omafind setup`, or the window on a first run, downloads the files
from Hugging Face into `~/.local/share/omafind/models`, unmodified and checked against their SHA-256.

## llama.cpp

The engine that runs the model is llama.cpp (with ggml), by the ggml authors, under the MIT License:
<https://github.com/ggml-org/llama.cpp>. The package bundles llama.cpp's own prebuilt release and installs its license
as `/usr/share/licenses/omafind/LICENSE.llama.cpp`.
